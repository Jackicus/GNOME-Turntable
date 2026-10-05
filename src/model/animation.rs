// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! Playing a clip: each channel sampled at a time sets one property of one node.

use glam::{Quat, Vec3, Vec4};

use super::{Channel, Clip, Interpolation, Property, Transform};

/// Every node's local transform and morph weights at one moment.
#[derive(Clone, Debug, Default)]
pub struct Pose {
    pub transforms: Vec<Transform>,
    pub weights: Vec<Vec<f32>>,
}

impl Clip {
    /// Sets what the clip animates in `pose` to how it is at `time` seconds. What it doesn't
    /// animate is left alone.
    pub fn apply(&self, time: f32, pose: &mut Pose) {
        for channel in &self.channels {
            channel.apply(time, pose);
        }
    }
}

impl Channel {
    fn apply(&self, time: f32, pose: &mut Pose) {
        let keys = self.times.len();
        if keys == 0 || self.node >= pose.transforms.len() {
            return;
        }
        let spline = self.interpolation == Interpolation::CubicSpline;
        let width = self.values.len() / keys / if spline { 3 } else { 1 };
        if width == 0 {
            return;
        }
        // A key's value; for a spline, its in-tangent, value and out-tangent are in a row
        let value = |key: usize, part: usize| {
            let start = if spline { (key * 3 + part) * width } else { key * width };
            &self.values[start..start + width]
        };

        let mut out = [0.0f32; 4];
        let mut weights = Vec::new();
        let target: &mut [f32] = if self.property == Property::Weights {
            weights.resize(width, 0.0);
            &mut weights
        } else {
            &mut out[..width.min(4)]
        };

        let last = keys - 1;
        if keys == 1 || time <= self.times[0] {
            target.copy_from_slice(&value(0, 1)[..target.len()]);
        } else if time >= self.times[last] {
            target.copy_from_slice(&value(last, 1)[..target.len()]);
        } else {
            let next = self.times.partition_point(|&t| t <= time).clamp(1, last);
            let key = next - 1;
            let span = self.times[next] - self.times[key];
            let u = if span > 0.0 { (time - self.times[key]) / span } else { 0.0 };
            match self.interpolation {
                Interpolation::Step => target.copy_from_slice(&value(key, 1)[..target.len()]),
                Interpolation::Linear if self.property == Property::Rotation && width == 4 => {
                    let a = Quat::from_slice(value(key, 1));
                    let b = Quat::from_slice(value(next, 1));
                    target.copy_from_slice(&a.slerp(b, u).to_array());
                }
                Interpolation::Linear => {
                    let (a, b) = (value(key, 1), value(next, 1));
                    for (i, v) in target.iter_mut().enumerate() {
                        *v = a[i] + (b[i] - a[i]) * u;
                    }
                }
                Interpolation::CubicSpline => {
                    let (u2, u3) = (u * u, u * u * u);
                    let (p0, m0) = (value(key, 1), value(key, 2));
                    let (m1, p1) = (value(next, 0), value(next, 1));
                    for (i, v) in target.iter_mut().enumerate() {
                        *v = (2.0 * u3 - 3.0 * u2 + 1.0) * p0[i]
                            + (u3 - 2.0 * u2 + u) * span * m0[i]
                            + (-2.0 * u3 + 3.0 * u2) * p1[i]
                            + (u3 - u2) * span * m1[i];
                    }
                }
            }
        }

        let transform = &mut pose.transforms[self.node];
        match self.property {
            Property::Translation if width == 3 => transform.translation = Vec3::from_slice(&out),
            Property::Scale if width == 3 => transform.scale = Vec3::from_slice(&out),
            Property::Rotation if width == 4 => {
                let q = Vec4::from_array(out).normalize_or_zero();
                if q != Vec4::ZERO {
                    transform.rotation = Quat::from_vec4(q);
                }
            }
            Property::Weights => {
                let node = &mut pose.weights[self.node];
                let n = node.len().min(weights.len());
                node[..n].copy_from_slice(&weights[..n]);
            }
            _ => {}
        }
    }
}
