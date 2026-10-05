// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! GL itself, through libepoxy as GTK has it, and the shader programs: compiled the first
//! time each is needed, every one from the same few sources.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::OnceLock;

use glow::HasContext;

/// Where each vertex attribute goes, for every program.
pub mod attribute {
    pub const POSITION: u32 = 0;
    pub const NORMAL: u32 = 1;
    pub const UV0: u32 = 2;
    pub const UV1: u32 = 3;
    pub const COLOR: u32 = 4;
    pub const JOINTS: u32 = 5;
    pub const WEIGHTS: u32 = 6;
}

/// The texture unit of each sampler. A material's maps take the first ones, a unit each
/// (uMap0, uMap1…, in the order of model::Map).
pub mod unit {
    pub const ENVIRONMENT: u32 = 13;
    pub const SHADOW: u32 = 14;
    pub const JOINTS: u32 = 15;
    pub const MORPHS: u32 = 16;
    pub const SOURCE: u32 = 17;
}

/// Uniform block binding points.
pub const FRAME_BLOCK: u32 = 0;
pub const MATERIAL_BLOCK: u32 = 1;

/// GL's functions, through the libepoxy GTK uses: it sends each call to the current
/// context, GL or GLES, EGL or GLX.
pub fn load() -> Result<glow::Context, String> {
    static EPOXY: OnceLock<Result<libloading::Library, String>> = OnceLock::new();
    let library = EPOXY
        .get_or_init(|| unsafe { libloading::Library::new("libepoxy.so.0") }.map_err(|e| e.to_string()))
        .as_ref()
        .map_err(Clone::clone)?;
    // SAFETY: each epoxy_gl* symbol is a variable holding the function's dispatch pointer
    Ok(unsafe {
        glow::Context::from_loader_function(|name| {
            let symbol = format!("epoxy_{name}\0");
            library.get::<*const *const c_void>(symbol.as_bytes()).map(|pointer| **pointer).unwrap_or(std::ptr::null())
        })
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// The model's surfaces
    Pbr,
    /// The model into the shadow map
    Depth,
    /// Wireframes, the grid, points and lines
    Solid,
    /// The floor that catches the shadow
    Floor,
    /// The environment's faces
    Environment,
    /// The environment blurred for a roughness
    Prefilter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub kind: Kind,
    pub skin: bool,
    pub morph: bool,
}

impl Key {
    pub fn new(kind: Kind) -> Self {
        Self { kind, skin: false, morph: false }
    }
}

type Location = Option<glow::UniformLocation>;

#[derive(Clone)]
pub struct Program {
    pub program: glow::Program,
    pub model: Location,
    pub normal_matrix: Location,
    pub flat: Location,
    pub color: Location,
    pub vertex_colors: Location,
    pub opacity: Location,
    pub morph_count: Location,
    pub morph_stride: Location,
    pub vertex_count: Location,
    pub morph_weights: Location,
    pub face: Location,
    pub size: Location,
    pub kind: Location,
    pub sky: Location,
    pub horizon: Location,
    pub ground: Location,
    pub source_size: Location,
    pub roughness: Location,
}

const COMMON: &str = include_str!("shaders/common.glsl");
const CUBE: &str = include_str!("shaders/cube.glsl");
const MESH_VERT: &str = include_str!("shaders/mesh.vert");
const SCREEN_VERT: &str = include_str!("shaders/screen.vert");
const PBR_FRAG: &str = include_str!("shaders/pbr.frag");
const SOLID_FRAG: &str = include_str!("shaders/solid.frag");
const FLOOR_FRAG: &str = include_str!("shaders/floor.frag");
const ENVIRONMENT_FRAG: &str = include_str!("shaders/environment.frag");
const PREFILTER_FRAG: &str = include_str!("shaders/prefilter.frag");

pub struct Programs {
    header: &'static str,
    cache: HashMap<Key, Program>,
}

impl Programs {
    pub fn new(es: bool) -> Self {
        let header = if es {
            "#version 300 es\nprecision highp float;\nprecision highp int;\nprecision highp sampler2D;\n\
             precision highp samplerCube;\nprecision highp sampler2DShadow;\n"
        } else {
            "#version 330 core\n"
        };
        Self { header, cache: HashMap::new() }
    }

    /// Compiles the plain programs, so a driver that can't is found out before anything's drawn.
    pub fn prepare(&mut self, gl: &glow::Context) -> Result<(), String> {
        for kind in [Kind::Pbr, Kind::Depth, Kind::Solid, Kind::Floor, Kind::Environment, Kind::Prefilter] {
            let key = Key::new(kind);
            let program = build(gl, self.header, key).map_err(|e| format!("shader {kind:?}: {e}"))?;
            self.cache.insert(key, program);
        }
        Ok(())
    }

    /// The program for `key`, compiled the first time. If a skinned or morphed one won't
    /// compile, the plain one draws instead.
    pub fn get(&mut self, gl: &glow::Context, key: Key) -> &Program {
        if !self.cache.contains_key(&key) {
            let program = build(gl, self.header, key).unwrap_or_else(|e| {
                crate::warn!("Shader {key:?} didn’t compile: {e}");
                self.cache[&Key::new(key.kind)].clone()
            });
            self.cache.insert(key, program);
        }
        &self.cache[&key]
    }

    pub fn destroy(&mut self, gl: &glow::Context) {
        let mut programs: Vec<glow::Program> = self.cache.drain().map(|(_, p)| p.program).collect();
        programs.sort_by_key(|p| p.0);
        programs.dedup();
        for program in programs {
            unsafe { gl.delete_program(program) };
        }
    }
}

fn build(gl: &glow::Context, header: &str, key: Key) -> Result<Program, String> {
    let mut defines = String::new();
    if key.skin {
        defines.push_str("#define SKIN\n");
    }
    if key.morph {
        defines.push_str("#define MORPH\n");
    }
    if key.kind == Kind::Depth {
        defines.push_str("#define DEPTH\n");
    }
    let (vertex, fragment): (&[&str], &[&str]) = match key.kind {
        Kind::Pbr | Kind::Depth => (&[MESH_VERT], &[PBR_FRAG]),
        Kind::Solid => (&[MESH_VERT], &[SOLID_FRAG]),
        Kind::Floor => (&[MESH_VERT], &[FLOOR_FRAG]),
        Kind::Environment => (&[SCREEN_VERT], &[CUBE, ENVIRONMENT_FRAG]),
        Kind::Prefilter => (&[SCREEN_VERT], &[CUBE, PREFILTER_FRAG]),
    };
    let source = |parts: &[&str]| {
        [header, &defines, COMMON].into_iter().chain(parts.iter().copied()).collect::<Vec<_>>().join("\n")
    };

    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (stage, parts) in [(glow::VERTEX_SHADER, vertex), (glow::FRAGMENT_SHADER, fragment)] {
            let shader = gl.create_shader(stage)?;
            gl.shader_source(shader, &source(parts));
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                return Err(gl.get_shader_info_log(shader));
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        for (location, name) in [
            (attribute::POSITION, "aPosition"),
            (attribute::NORMAL, "aNormal"),
            (attribute::UV0, "aUv0"),
            (attribute::UV1, "aUv1"),
            (attribute::COLOR, "aColor"),
            (attribute::JOINTS, "aJoints"),
            (attribute::WEIGHTS, "aWeights"),
        ] {
            gl.bind_attrib_location(program, location, name);
        }
        gl.link_program(program);
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        if !gl.get_program_link_status(program) {
            return Err(gl.get_program_info_log(program));
        }

        if let Some(index) = gl.get_uniform_block_index(program, "Frame") {
            gl.uniform_block_binding(program, index, FRAME_BLOCK);
        }
        if let Some(index) = gl.get_uniform_block_index(program, "Material") {
            gl.uniform_block_binding(program, index, MATERIAL_BLOCK);
        }
        gl.use_program(Some(program));
        for map in 0..crate::model::Map::COUNT as u32 {
            if let Some(location) = gl.get_uniform_location(program, &format!("uMap{map}")) {
                gl.uniform_1_i32(Some(&location), map as i32);
            }
        }
        for (name, unit) in [
            ("uEnvironment", unit::ENVIRONMENT),
            ("uShadowMap", unit::SHADOW),
            ("uJoints", unit::JOINTS),
            ("uMorphs", unit::MORPHS),
            ("uSource", unit::SOURCE),
        ] {
            if let Some(location) = gl.get_uniform_location(program, name) {
                gl.uniform_1_i32(Some(&location), unit as i32);
            }
        }
        let at = |name: &str| gl.get_uniform_location(program, name);
        Ok(Program {
            program,
            model: at("uModel"),
            normal_matrix: at("uNormalMatrix"),
            flat: at("uFlat"),
            color: at("uColor"),
            vertex_colors: at("uVertexColors"),
            opacity: at("uOpacity"),
            morph_count: at("uMorphCount"),
            morph_stride: at("uMorphStride"),
            vertex_count: at("uVertexCount"),
            morph_weights: at("uMorphWeights"),
            face: at("uFace"),
            size: at("uSize"),
            kind: at("uKind"),
            sky: at("uSky"),
            horizon: at("uHorizon"),
            ground: at("uGround"),
            source_size: at("uSourceSize"),
            roughness: at("uRoughness"),
        })
    }
}
