// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! The camera, orbiting the model as three.js's OrbitControls did: dragging turns it, moves it
//! or zooms it, a flick coasts to a stop, and it can spin slowly on its own. The coasting
//! decays with time rather than frames, so it's the same at any refresh rate.

use std::f32::consts::{PI, TAU};

use glam::camera::rh::{proj::opengl, view};
use glam::{Mat4, Vec3};

/// The vertical field of view, in degrees.
const FOV: f32 = 35.0;
/// OrbitControls' units: a turn every 15 seconds.
const SPIN_SPEED: f32 = 4.0;
/// An arrow key turns the model 15°.
pub const KEY_STEP: f32 = PI / 12.0;

pub struct OrbitCamera {
    pub target: Vec3,
    pub position: Vec3,
    /// Width over height
    pub aspect: f32,
    near: f32,
    far: f32,
    min_distance: f32,
    max_distance: f32,
    /// Of the model's bounding sphere: what counts as moving is relative to it
    radius: f32,
    // What's still to come of a drag, released a share each frame
    theta_delta: f32,
    phi_delta: f32,
    pan_offset: Vec3,
    scale: f32,
    /// Turning on its own, like a turntable
    pub spin: bool,
    /// The spin waits while a drag is going on
    pub dragging: bool,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        let mut camera = Self {
            target: Vec3::ZERO,
            position: Vec3::Z,
            aspect: 1.0,
            near: 0.01,
            far: 100.0,
            min_distance: 0.0,
            max_distance: f32::INFINITY,
            radius: 1.0,
            theta_delta: 0.0,
            phi_delta: 0.0,
            pan_offset: Vec3::ZERO,
            scale: 1.0,
            spin: false,
            dragging: false,
        };
        camera.reset(1.0, Vec3::ONE);
        camera
    }
}

fn spherical(offset: Vec3) -> (f32, f32, f32) {
    let radius = offset.length();
    if radius == 0.0 {
        return (0.0, 0.0, PI / 2.0);
    }
    (radius, offset.x.atan2(offset.z), (offset.y / radius).clamp(-1.0, 1.0).acos())
}

fn from_spherical(radius: f32, theta: f32, phi: f32) -> Vec3 {
    Vec3::new(radius * phi.sin() * theta.sin(), radius * phi.cos(), radius * phi.sin() * theta.cos())
}

impl OrbitCamera {
    /// Back to the start: the whole model in view, from a little above and to the side.
    /// `size` is the model's box, standing on the floor at the origin; `radius` is of the
    /// sphere around it.
    pub fn reset(&mut self, radius: f32, size: Vec3) {
        self.radius = radius;
        self.target = Vec3::new(0.0, size.y / 2.0, 0.0);
        let vertical = FOV.to_radians();
        let horizontal = 2.0 * ((vertical / 2.0).tan() * self.aspect).atan();
        let direction = from_spherical(1.0, 30f32.to_radians(), 70f32.to_radians());
        // Far enough for the sphere around the model to fit…
        let sphere = radius / (vertical.min(horizontal) / 2.0).sin();
        // …or, nearer for a flat or long model, for every corner of its box to
        let forward = -direction;
        let right = forward.cross(Vec3::Y).normalize();
        let up = right.cross(forward);
        let (tan_h, tan_v) = ((horizontal / 2.0).tan(), (vertical / 2.0).tan());
        let mut fit: f32 = 0.0;
        for corner in 0..8 {
            let pick = |bit: u32, extent: f32| if corner & bit == 0 { -extent / 2.0 } else { extent / 2.0 };
            let p = Vec3::new(pick(1, size.x), pick(2, size.y), pick(4, size.z));
            let toward = p.dot(direction);
            fit = fit.max(toward + p.dot(right).abs() / tan_h).max(toward + p.dot(up).abs() / tan_v);
        }
        let distance = if fit > 0.0 { sphere.min(fit) } else { sphere } * 1.3;
        self.near = radius / 100.0;
        self.far = distance + radius * 50.0;
        self.position = self.target + direction * distance;
        self.min_distance = radius * 0.2;
        self.max_distance = sphere * 1.3 * 8.0;
        self.theta_delta = 0.0;
        self.phi_delta = 0.0;
        self.pan_offset = Vec3::ZERO;
        self.scale = 1.0;
    }

    /// A drag of `dx`, `dy` pixels in a view `height` pixels tall: the whole height is a turn.
    pub fn rotate(&mut self, dx: f32, dy: f32, height: f32) {
        self.theta_delta -= TAU * dx / height;
        self.phi_delta -= TAU * dy / height;
    }

    /// A drag that moves the model with the pointer.
    pub fn pan(&mut self, dx: f32, dy: f32, height: f32) {
        let distance = (self.position - self.target).length() * (FOV.to_radians() / 2.0).tan();
        let forward = (self.target - self.position).normalize_or_zero();
        let right = forward.cross(Vec3::Y).normalize_or_zero();
        let up = right.cross(forward);
        self.pan_offset += -right * (2.0 * dx * distance / height) + up * (2.0 * dy * distance / height);
    }

    /// Zooms out by `scale` (in, under 1) on the next update.
    pub fn zoom(&mut self, scale: f32) {
        self.scale *= scale;
    }

    /// Turns the camera at once, as the arrow keys do.
    pub fn orbit_now(&mut self, left: f32, up: f32) {
        let (radius, theta, phi) = spherical(self.position - self.target);
        let phi = (phi + up).clamp(0.01, PI - 0.01);
        self.position = self.target + from_spherical(radius, theta + left, phi);
    }

    /// Zooms at once, as the + and − keys do.
    pub fn dolly_now(&mut self, scale: f32) {
        let offset = self.position - self.target;
        let distance = (offset.length() * scale).clamp(self.min_distance, self.max_distance);
        self.position = self.target + offset.normalize_or_zero() * distance;
    }

    /// Moves on by `dt` seconds: a share of what's left of the drags, and the spin. True if it
    /// moved enough to see.
    pub fn update(&mut self, dt: f32) -> bool {
        // 0.05 a frame at 60 Hz
        let damping = 1.0 - 0.95f32.powf(dt * 60.0);
        let (radius, mut theta, mut phi) = spherical(self.position - self.target);
        if self.spin && !self.dragging {
            self.theta_delta -= TAU / 60.0 * SPIN_SPEED * dt;
        }
        theta += self.theta_delta * damping;
        phi = (phi + self.phi_delta * damping).clamp(1e-4, PI - 1e-4);
        let old_target = self.target;
        let old_position = self.position;
        self.target += self.pan_offset * damping;
        let distance = (radius * self.scale).clamp(self.min_distance, self.max_distance);
        // At the nearest or farthest, rounding would otherwise count as zooming every frame
        let zoomed = (distance - radius).abs() > 1e-5 * self.radius;
        self.position = self.target + from_spherical(distance, theta, phi);

        self.theta_delta *= 1.0 - damping;
        self.phi_delta *= 1.0 - damping;
        self.pan_offset *= 1.0 - damping;
        self.scale = 1.0;

        let threshold = 1e-6 * self.radius * self.radius;
        zoomed
            || self.position.distance_squared(old_position) > threshold
            || self.target.distance_squared(old_target) > threshold
    }

    pub fn view(&self) -> Mat4 {
        view::look_at_mat4(self.position, self.target, Vec3::Y)
    }

    pub fn projection(&self) -> Mat4 {
        opengl::perspective(FOV.to_radians(), self.aspect.max(1e-3), self.near, self.far)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_fits_the_model() {
        for (aspect, size) in
            [(2.0, Vec3::new(1.0, 2.0, 1.0)), (0.5, Vec3::new(4.0, 0.2, 4.0)), (1.5, Vec3::new(10.0, 1.0, 0.5))]
        {
            let mut camera = OrbitCamera { aspect, ..OrbitCamera::default() };
            let radius = size.length() / 2.0;
            camera.reset(radius, size);
            assert_eq!(camera.target, Vec3::new(0.0, size.y / 2.0, 0.0));
            // No farther than the sphere needs
            let vertical = FOV.to_radians();
            let horizontal = 2.0 * ((vertical / 2.0).tan() * aspect).atan();
            let sphere = radius / (vertical.min(horizontal) / 2.0).sin() * 1.3;
            assert!(camera.position.distance(camera.target) <= sphere + 1e-3);
            // Every corner in view, with room to spare
            let clip = camera.projection() * camera.view();
            for corner in 0..8 {
                let pick = |bit: u32, e: f32| if corner & bit == 0 { -e / 2.0 } else { e / 2.0 };
                let p = Vec3::new(pick(1, size.x), pick(2, size.y) + size.y / 2.0, pick(4, size.z));
                let ndc = clip.project_point3(p);
                assert!(ndc.x.abs() < 0.95 && ndc.y.abs() < 0.95, "{size}: corner at {ndc}");
            }
        }
    }

    #[test]
    fn a_flick_coasts_the_same_at_any_refresh_rate() {
        let turn = |hz: f32| {
            let mut camera = OrbitCamera::default();
            camera.rotate(100.0, 0.0, 500.0);
            let start = spherical(camera.position - camera.target).1;
            for _ in 0..(hz as usize * 2) {
                camera.update(1.0 / hz);
            }
            spherical(camera.position - camera.target).1 - start
        };
        let (at60, at240) = (turn(60.0), turn(240.0));
        assert!((at60 - at240).abs() < 0.01, "{at60} vs {at240}");
        assert!((at60 + TAU / 5.0).abs() < 0.05, "a fifth of the height turns a fifth of the way: {at60}");
    }

    #[test]
    fn spins_a_turn_every_fifteen_seconds() {
        let mut camera = OrbitCamera { spin: true, ..OrbitCamera::default() };
        let mut turned = 0.0;
        let mut last = spherical(camera.position - camera.target).1;
        for _ in 0..240 * 30 {
            camera.update(1.0 / 240.0);
            let theta = spherical(camera.position - camera.target).1;
            let mut step = theta - last;
            if step > PI {
                step -= TAU;
            } else if step < -PI {
                step += TAU;
            }
            turned += step;
            last = theta;
        }
        // Two turns in 30 seconds, less the moment it takes to get going
        assert!((turned.abs() - 2.0 * TAU).abs() < 0.2, "{turned}");
    }
}
