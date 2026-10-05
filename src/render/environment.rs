// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! Lighting presets: an environment for reflections and soft light, and a key light for the
//! shadow. The environments are made on the GPU, not loaded, so the app ships no images: drawn
//! into a cube map, then blurred once for each roughness into the mip levels of another.

use glam::Vec3;
use glow::HasContext;

use super::programs::{Key, Kind, Programs, unit};
use crate::model::srgb_hex;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Lighting {
    #[default]
    Studio,
    Soft,
    Sunlight,
    Dramatic,
}

pub enum Environment {
    /// three.js's RoomEnvironment
    Room,
    /// Sky, horizon and ground, in linear RGB
    Gradient([f32; 3], [f32; 3], [f32; 3]),
}

pub struct Preset {
    pub environment: Environment,
    pub intensity: f32,
    /// Linear RGB, intensity included
    pub key_color: Vec3,
    /// Towards the light
    pub key_direction: Vec3,
    /// How dark the shadow on the floor is
    pub shadow: f32,
}

impl Lighting {
    /// By the name the settings use; Studio for one they don't know.
    pub fn from_name(name: &str) -> Self {
        match name {
            "soft" => Self::Soft,
            "sunlight" => Self::Sunlight,
            "dramatic" => Self::Dramatic,
            _ => Self::Studio,
        }
    }

    pub fn preset(self) -> Preset {
        let key = |hex, intensity: f32, direction: [f32; 3]| {
            (Vec3::from(srgb_hex(hex)) * intensity, Vec3::from(direction).normalize())
        };
        let gradient = |sky, horizon, ground| Environment::Gradient(srgb_hex(sky), srgb_hex(horizon), srgb_hex(ground));
        let (environment, intensity, (key_color, key_direction), shadow) = match self {
            Self::Studio => (Environment::Room, 1.0, key(0xffffff, 1.5, [0.5, 1.0, 0.6]), 0.2),
            Self::Soft => (gradient(0xffffff, 0xd0d0d0, 0x9a9996), 1.0, key(0xffffff, 0.4, [0.0, 1.0, 0.2]), 0.1),
            Self::Sunlight => (gradient(0x99c1f1, 0xf6f5f4, 0x865e3c), 0.7, key(0xfff1d6, 3.0, [-0.6, 1.0, 0.5]), 0.35),
            Self::Dramatic => (Environment::Room, 0.12, key(0xffffff, 4.0, [1.0, 1.2, -0.4]), 0.3),
        };
        Preset { environment, intensity, key_color, key_direction, shadow }
    }
}

/// The size of the environment's faces, and how many times it's halved for rougher surfaces.
const SIZE: i32 = 256;
pub const LEVELS: i32 = 6;

/// `hdr`: half floats, if the GPU can draw into them; else 8 bits a channel, which clips the
/// brightest light but still lights the model.
fn cube_map(gl: &glow::Context, levels: i32, hdr: bool) -> glow::Texture {
    unsafe {
        let texture = gl.create_texture().expect("texture");
        gl.bind_texture(glow::TEXTURE_CUBE_MAP, Some(texture));
        for level in 0..levels {
            let size = SIZE >> level;
            for face in 0..6 {
                gl.tex_image_2d(
                    glow::TEXTURE_CUBE_MAP_POSITIVE_X + face,
                    level,
                    if hdr { glow::RGBA16F } else { glow::RGBA8 } as i32,
                    size,
                    size,
                    0,
                    glow::RGBA,
                    if hdr { glow::HALF_FLOAT } else { glow::UNSIGNED_BYTE },
                    glow::PixelUnpackData::Slice(None),
                );
            }
        }
        gl.tex_parameter_i32(glow::TEXTURE_CUBE_MAP, glow::TEXTURE_MAX_LEVEL, levels - 1);
        gl.tex_parameter_i32(glow::TEXTURE_CUBE_MAP, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_CUBE_MAP, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        for wrap in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T, glow::TEXTURE_WRAP_R] {
            gl.tex_parameter_i32(glow::TEXTURE_CUBE_MAP, wrap, glow::CLAMP_TO_EDGE as i32);
        }
        texture
    }
}

/// Draws `preset`'s environment and blurs it: a cube map whose mip level n is for surfaces
/// of roughness n / (LEVELS − 1). Leaves the GL state as it found what it needs of it.
pub fn bake(
    gl: &glow::Context,
    programs: &mut Programs,
    empty: glow::VertexArray,
    preset: &Preset,
    hdr: bool,
) -> glow::Texture {
    unsafe {
        let framebuffer = gl.create_framebuffer().expect("framebuffer");
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
        gl.bind_vertex_array(Some(empty));
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::BLEND);
        gl.disable(glow::CULL_FACE);

        // The environment itself, every mip level of it for the blur to sample
        let full_levels = SIZE.ilog2() as i32 + 1;
        let source = cube_map(gl, full_levels, hdr);
        let program = programs.get(gl, Key::new(Kind::Environment));
        gl.use_program(Some(program.program));
        match preset.environment {
            Environment::Room => gl.uniform_1_i32(program.kind.as_ref(), 0),
            Environment::Gradient(sky, horizon, ground) => {
                gl.uniform_1_i32(program.kind.as_ref(), 1);
                gl.uniform_3_f32_slice(program.sky.as_ref(), &sky);
                gl.uniform_3_f32_slice(program.horizon.as_ref(), &horizon);
                gl.uniform_3_f32_slice(program.ground.as_ref(), &ground);
            }
        }
        gl.uniform_1_f32(program.size.as_ref(), SIZE as f32);
        gl.viewport(0, 0, SIZE, SIZE);
        for face in 0..6 {
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_CUBE_MAP_POSITIVE_X + face,
                Some(source),
                0,
            );
            gl.uniform_1_i32(program.face.as_ref(), face as i32);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }
        gl.bind_texture(glow::TEXTURE_CUBE_MAP, Some(source));
        gl.generate_mipmap(glow::TEXTURE_CUBE_MAP);

        // Blurred for each roughness
        let target = cube_map(gl, LEVELS, hdr);
        let program = programs.get(gl, Key::new(Kind::Prefilter));
        gl.use_program(Some(program.program));
        gl.active_texture(glow::TEXTURE0 + unit::SOURCE);
        gl.bind_texture(glow::TEXTURE_CUBE_MAP, Some(source));
        gl.uniform_1_f32(program.source_size.as_ref(), SIZE as f32);
        for level in 0..LEVELS {
            let size = SIZE >> level;
            gl.viewport(0, 0, size, size);
            gl.uniform_1_f32(program.size.as_ref(), size as f32);
            gl.uniform_1_f32(program.roughness.as_ref(), level as f32 / (LEVELS - 1) as f32);
            for face in 0..6 {
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_CUBE_MAP_POSITIVE_X + face,
                    Some(target),
                    level,
                );
                gl.uniform_1_i32(program.face.as_ref(), face as i32);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
        }

        gl.bind_texture(glow::TEXTURE_CUBE_MAP, None);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.delete_framebuffer(framebuffer);
        gl.delete_texture(source);
        target
    }
}
