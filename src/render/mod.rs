// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! The renderer: draws the model in the viewer's GL area much as three.js drew it before.
//! Metallic-roughness PBR lit by an environment and a key light, a shadow caught by an
//! invisible floor, an optional grid and wireframe; 4× multisampled, on a transparent
//! background so the window's own shows through. It draws when asked, never on its own.

mod environment;
mod gpu;
mod programs;

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::camera::rh::{proj::opengl, view};
use glam::{Mat4, Vec3};
use glow::HasContext;

pub use environment::Lighting;
use gpu::{GpuModel, Item, Limits};
use programs::{FRAME_BLOCK, Key, Kind, MATERIAL_BLOCK, Program, Programs, attribute, unit};

use crate::model::{Map, Model};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Display {
    #[default]
    Shaded,
    Wireframe,
    ShadedWireframe,
}

impl Display {
    /// By the name the settings use.
    pub fn from_name(name: &str) -> Self {
        match name {
            "wireframe" => Self::Wireframe,
            "shaded-wireframe" => Self::ShadedWireframe,
            _ => Self::Shaded,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub lighting: Lighting,
    pub display: Display,
    pub grid: bool,
    pub dark: bool,
    /// The accent colour, linear RGB: the wireframe's
    pub accent: [f32; 3],
    /// Device pixels a logical pixel
    pub scale: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lighting: Lighting::Studio,
            display: Display::Shaded,
            grid: false,
            dark: false,
            accent: crate::model::srgb_hex(0x3584e4),
            scale: 1.0,
        }
    }
}

pub struct Camera {
    pub view: Mat4,
    pub projection: Mat4,
    pub position: Vec3,
}

/// The model as it stands this frame. `version` changes whenever the pose does.
pub struct Frame<'a> {
    pub model: &'a Model,
    pub worlds: &'a [Mat4],
    pub weights: &'a [Vec<f32>],
    pub version: u64,
}

/// The frame's uniform block, as common.glsl declares it (std140).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameBlock {
    view: [f32; 16],
    projection: [f32; 16],
    light_matrix: [f32; 16],
    camera: [f32; 4],
    light_direction: [f32; 4],
    light_color: [f32; 4],
    params: [f32; 4],
}

const SHADOW_SIZE: i32 = 2048;

/// The multisampled framebuffer everything is drawn in, then resolved to the output.
struct Target {
    framebuffer: glow::Framebuffer,
    color: glow::Renderbuffer,
    depth: glow::Renderbuffer,
    width: i32,
    height: i32,
}

struct Grid {
    vao: glow::VertexArray,
    buffer: glow::Buffer,
    count: i32,
    radius: f32,
}

pub struct Renderer {
    gl: glow::Context,
    programs: Programs,
    limits: Limits,
    samples: i32,
    /// The environment can be drawn in half floats
    hdr: bool,
    frame: glow::Buffer,
    empty: glow::VertexArray,
    white: glow::Texture,
    floor: (glow::VertexArray, glow::Buffer),
    grid: Option<Grid>,
    shadow: (glow::Framebuffer, glow::Texture),
    /// The shadow is drawn again only when the model, its pose or the light changes
    shadow_dirty: bool,
    shadow_lighting: Option<Lighting>,
    environments: HashMap<Lighting, glow::Texture>,
    target: Option<Target>,
    model: Option<GpuModel>,
    // Kept from frame to frame for sorting what's see-through
    sorted: Vec<(f32, Item)>,
    order: Vec<Item>,
    radius: f32,
    height: f32,
}

/// GL's functions, for whichever context is current.
pub fn load_gl() -> Result<glow::Context, String> {
    programs::load()
}

impl Renderer {
    pub fn gl(&self) -> &glow::Context {
        &self.gl
    }

    /// For the GL context that's current: GL 3.3 or GLES 3.0 at least.
    pub fn new(gl: glow::Context) -> Result<Self, String> {
        let version = gl.version();
        let es = version.is_embedded;
        if (es && version.major < 3) || (!es && (version.major, version.minor) < (3, 3)) {
            return Err(format!("OpenGL {}.{} is too old", version.major, version.minor));
        }
        unsafe {
            if !es {
                gl.enable(glow::TEXTURE_CUBE_MAP_SEAMLESS);
                gl.enable(glow::PROGRAM_POINT_SIZE);
            }
            let extensions = gl.supported_extensions();
            let anisotropy = if extensions.contains("GL_EXT_texture_filter_anisotropic")
                || extensions.contains("GL_ARB_texture_filter_anisotropic")
            {
                gl.get_parameter_f32(glow::MAX_TEXTURE_MAX_ANISOTROPY_EXT).min(4.0)
            } else {
                1.0
            };
            let limits =
                Limits { max_texture_size: gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE).max(1) as u32, anisotropy };
            let samples = gl.get_parameter_i32(glow::MAX_SAMPLES).clamp(1, 4);
            // GLES 3.0 draws into float textures only with an extension
            let hdr = !es
                || extensions.contains("GL_EXT_color_buffer_half_float")
                || extensions.contains("GL_EXT_color_buffer_float");

            let frame = gl.create_buffer()?;
            gl.bind_buffer(glow::UNIFORM_BUFFER, Some(frame));
            gl.buffer_data_size(glow::UNIFORM_BUFFER, size_of::<FrameBlock>() as i32, glow::DYNAMIC_DRAW);
            gl.bind_buffer(glow::UNIFORM_BUFFER, None);

            let empty = gl.create_vertex_array()?;
            let white = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(white));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                1,
                1,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&[255; 4])),
            );

            // The floor: a square on the ground, scaled to the model when drawn
            let floor_vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(floor_vao));
            let floor_buffer = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(floor_buffer));
            let corners: [f32; 18] =
                [-0.5, 0.0, -0.5, -0.5, 0.0, 0.5, 0.5, 0.0, 0.5, -0.5, 0.0, -0.5, 0.5, 0.0, 0.5, 0.5, 0.0, -0.5];
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck::cast_slice(&corners), glow::STATIC_DRAW);
            gl.enable_vertex_attrib_array(attribute::POSITION);
            gl.vertex_attrib_pointer_f32(attribute::POSITION, 3, glow::FLOAT, false, 0, 0);
            gl.bind_vertex_array(None);

            // The shadow map: depth only, compared in hardware
            let shadow_texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(shadow_texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::DEPTH_COMPONENT24 as i32,
                SHADOW_SIZE,
                SHADOW_SIZE,
                0,
                glow::DEPTH_COMPONENT,
                glow::UNSIGNED_INT,
                glow::PixelUnpackData::Slice(None),
            );
            for (parameter, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_COMPARE_MODE, glow::COMPARE_REF_TO_TEXTURE),
                (glow::TEXTURE_COMPARE_FUNC, glow::LEQUAL),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, parameter, value as i32);
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            let shadow_framebuffer = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(shadow_framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::DEPTH_ATTACHMENT,
                glow::TEXTURE_2D,
                Some(shadow_texture),
                0,
            );
            gl.draw_buffers(&[glow::NONE]);
            gl.read_buffer(glow::NONE);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);

            // What a primitive without colours has for them
            gl.vertex_attrib_4_f32(attribute::COLOR, 1.0, 1.0, 1.0, 1.0);

            let mut programs = Programs::new(es);
            programs.prepare(&gl)?;

            Ok(Self {
                programs,
                limits,
                samples,
                hdr,
                frame,
                empty,
                white,
                floor: (floor_vao, floor_buffer),
                grid: None,
                shadow: (shadow_framebuffer, shadow_texture),
                shadow_dirty: true,
                shadow_lighting: None,
                environments: HashMap::new(),
                target: None,
                model: None,
                sorted: Vec::new(),
                order: Vec::new(),
                radius: 1.0,
                height: 1.0,
                gl,
            })
        }
    }

    /// Puts `model` on the GPU, in place of any other. `radius` and `height` are of its
    /// bounding sphere and box: the shadow, floor and grid are sized to them.
    pub fn set_model(&mut self, model: &Model, radius: f32, height: f32) {
        self.clear_model();
        let gpu = GpuModel::new(&self.gl, model, self.limits);
        // Every program the model needs, now, rather than a stall on the frame that first needs one
        let mut keys = std::collections::HashSet::new();
        for item in gpu.surfaces().chain(&gpu.points).chain(&gpu.lines) {
            let (skin, morph) = (item.skin.is_some(), gpu.primitives[item.primitive].morph.is_some());
            for kind in [Kind::Pbr, Kind::Depth, Kind::Solid] {
                keys.insert(Key { kind, skin, morph });
            }
        }
        for key in keys {
            self.programs.get(&self.gl, key);
        }
        self.model = Some(gpu);
        self.radius = radius;
        self.height = height;
        self.shadow_dirty = true;
    }

    /// Goes on putting the model's textures on the GPU until `deadline`: true once it's all
    /// there. The model isn't drawn until then.
    pub fn upload(&mut self, model: &mut Model, deadline: std::time::Instant) -> bool {
        match self.model.as_mut() {
            Some(gpu) => gpu.upload_textures(&self.gl, model, deadline),
            None => true,
        }
    }

    pub fn clear_model(&mut self) {
        if let Some(model) = self.model.take() {
            model.destroy(&self.gl);
        }
        self.shadow_dirty = true;
    }

    /// Draws a frame into `output` (None: the default framebuffer), `width` × `height` pixels.
    pub fn render(
        &mut self,
        output: Option<glow::Framebuffer>,
        width: i32,
        height: i32,
        camera: &Camera,
        settings: &Settings,
        frame: Option<&Frame>,
    ) {
        if width <= 0 || height <= 0 {
            return;
        }
        let preset = settings.lighting.preset();
        let environment = self.environment(settings.lighting, &preset);
        if self.shadow_lighting != Some(settings.lighting) {
            self.shadow_lighting = Some(settings.lighting);
            self.shadow_dirty = true;
        }
        let frame = frame.filter(|_| self.model.as_ref().is_some_and(GpuModel::is_ready));
        if let (Some(gpu), Some(frame)) = (self.model.as_mut(), frame) {
            if gpu.update_pose(&self.gl, frame.model, frame.worlds, frame.version) {
                self.shadow_dirty = true;
            }
        }

        // The key light looks at the model's middle from four radii away
        let r = self.radius;
        let center = Vec3::new(0.0, self.height / 2.0, 0.0);
        let up = if preset.key_direction.cross(Vec3::Y).length_squared() < 1e-6 { Vec3::Z } else { Vec3::Y };
        let light_view = view::look_at_mat4(center + preset.key_direction * r * 4.0, center, up);
        let light_projection = opengl::orthographic(-2.5 * r, 2.5 * r, -2.5 * r, 2.5 * r, 0.5 * r, 8.0 * r);
        let light_matrix = light_projection * light_view;
        let mut block = FrameBlock {
            view: light_view.to_cols_array(),
            projection: light_projection.to_cols_array(),
            light_matrix: light_matrix.to_cols_array(),
            camera: camera.position.extend(1.0).to_array(),
            light_direction: preset.key_direction.extend(0.0).to_array(),
            light_color: preset.key_color.extend(1.0).to_array(),
            params: [preset.intensity, (environment::LEVELS - 1) as f32, 2.0 * settings.scale, 0.0],
        };

        unsafe {
            let gl = &self.gl;
            gl.bind_buffer_base(glow::UNIFORM_BUFFER, FRAME_BLOCK, Some(self.frame));
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.color_mask(true, true, true, true);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::STENCIL_TEST);

            if let (true, Some(frame)) = (self.shadow_dirty, frame) {
                self.write_frame(&block);
                self.draw_shadow(frame);
                self.shadow_dirty = false;
            }

            block.view = camera.view.to_cols_array();
            block.projection = camera.projection.to_cols_array();
            self.write_frame(&block);
            self.ensure_target(width, height);
            let gl = &self.gl;
            let target = self.target.as_ref().unwrap().framebuffer;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target));
            gl.viewport(0, 0, width, height);
            gl.depth_mask(true);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            gl.active_texture(glow::TEXTURE0 + unit::ENVIRONMENT);
            gl.bind_texture(glow::TEXTURE_CUBE_MAP, Some(environment));
            gl.bind_sampler(unit::ENVIRONMENT, None);
            gl.active_texture(glow::TEXTURE0 + unit::SHADOW);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.shadow.1));
            gl.bind_sampler(unit::SHADOW, None);

            if let Some(frame) = frame {
                let shaded = settings.display != Display::Wireframe;
                if shaded {
                    self.draw_opaque(frame);
                }
                self.draw_points_and_lines(frame);
                self.begin_transparent();
                if shaded {
                    self.draw_floor(&preset, settings);
                }
                if settings.grid {
                    self.draw_grid(settings);
                }
                if shaded {
                    self.draw_blended(frame, camera);
                }
                match settings.display {
                    Display::Shaded => {}
                    Display::Wireframe => self.draw_wireframe(frame, settings, 0.9),
                    Display::ShadedWireframe => self.draw_wireframe(frame, settings, 0.45),
                }
                self.end_transparent();
            }

            let gl = &self.gl;
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(target));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, output);
            gl.blit_framebuffer(0, 0, width, height, 0, 0, width, height, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, output);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }

    /// A frame as RGBA bytes (premultiplied, top row first), drawn off screen.
    pub fn snapshot(
        &mut self,
        width: i32,
        height: i32,
        camera: &Camera,
        settings: &Settings,
        frame: Option<&Frame>,
    ) -> Vec<u8> {
        let mut pixels = vec![0u8; (width * height * 4).max(0) as usize];
        unsafe {
            let gl = &self.gl;
            let framebuffer = gl.create_framebuffer().expect("framebuffer");
            let color = gl.create_renderbuffer().expect("renderbuffer");
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(color));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::RGBA8, width, height);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::RENDERBUFFER, Some(color));
            self.render(Some(framebuffer), width, height, camera, settings, frame);
            let gl = &self.gl;
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(framebuffer));
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.read_pixels(
                0,
                0,
                width,
                height,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.delete_framebuffer(framebuffer);
            gl.delete_renderbuffer(color);
        }
        // GL's rows start at the bottom
        let row = width as usize * 4;
        let mut flipped = Vec::with_capacity(pixels.len());
        for line in pixels.chunks_exact(row).rev() {
            flipped.extend_from_slice(line);
        }
        flipped
    }

    /// Frees everything on the GPU; the context must be current.
    pub fn destroy(mut self) {
        self.clear_model();
        let gl = &self.gl;
        unsafe {
            self.programs.destroy(gl);
            for (_, texture) in self.environments.drain() {
                gl.delete_texture(texture);
            }
            if let Some(target) = self.target.take() {
                gl.delete_framebuffer(target.framebuffer);
                gl.delete_renderbuffer(target.color);
                gl.delete_renderbuffer(target.depth);
            }
            if let Some(grid) = self.grid.take() {
                gl.delete_vertex_array(grid.vao);
                gl.delete_buffer(grid.buffer);
            }
            gl.delete_framebuffer(self.shadow.0);
            gl.delete_texture(self.shadow.1);
            gl.delete_vertex_array(self.floor.0);
            gl.delete_buffer(self.floor.1);
            gl.delete_texture(self.white);
            gl.delete_vertex_array(self.empty);
            gl.delete_buffer(self.frame);
        }
    }

    fn environment(&mut self, lighting: Lighting, preset: &environment::Preset) -> glow::Texture {
        if let Some(&texture) = self.environments.get(&lighting) {
            return texture;
        }
        let texture = environment::bake(&self.gl, &mut self.programs, self.empty, preset, self.hdr);
        self.environments.insert(lighting, texture);
        texture
    }

    fn write_frame(&self, block: &FrameBlock) {
        unsafe {
            self.gl.bind_buffer(glow::UNIFORM_BUFFER, Some(self.frame));
            self.gl.buffer_sub_data_u8_slice(glow::UNIFORM_BUFFER, 0, bytemuck::bytes_of(block));
            self.gl.bind_buffer(glow::UNIFORM_BUFFER, None);
        }
    }

    fn ensure_target(&mut self, width: i32, height: i32) {
        if self.target.as_ref().is_some_and(|t| t.width == width && t.height == height) {
            return;
        }
        let gl = &self.gl;
        unsafe {
            if let Some(old) = self.target.take() {
                gl.delete_framebuffer(old.framebuffer);
                gl.delete_renderbuffer(old.color);
                gl.delete_renderbuffer(old.depth);
            }
            let framebuffer = gl.create_framebuffer().expect("framebuffer");
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            let renderbuffer = |format, attachment| {
                let buffer = gl.create_renderbuffer().expect("renderbuffer");
                gl.bind_renderbuffer(glow::RENDERBUFFER, Some(buffer));
                gl.renderbuffer_storage_multisample(glow::RENDERBUFFER, self.samples, format, width, height);
                gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, attachment, glow::RENDERBUFFER, Some(buffer));
                buffer
            };
            let color = renderbuffer(glow::RGBA8, glow::COLOR_ATTACHMENT0);
            let depth = renderbuffer(glow::DEPTH_COMPONENT24, glow::DEPTH_ATTACHMENT);
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);
            self.target = Some(Target { framebuffer, color, depth, width, height });
        }
    }

    unsafe fn begin_transparent(&self) {
        unsafe {
            self.gl.enable(glow::BLEND);
            // Every shader writes premultiplied colour
            self.gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            self.gl.depth_mask(false);
        }
    }

    unsafe fn end_transparent(&self) {
        unsafe {
            self.gl.disable(glow::BLEND);
            self.gl.depth_mask(true);
        }
    }

    unsafe fn draw_shadow(&mut self, frame: &Frame) {
        let Some(gpu) = self.model.as_mut() else { return };
        // The lists are borrowed out of the model while drawing, not copied
        let (opaque, blended) = (std::mem::take(&mut gpu.opaque), std::mem::take(&mut gpu.blended));
        unsafe {
            let gl = &self.gl;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.shadow.0));
            gl.viewport(0, 0, SHADOW_SIZE, SHADOW_SIZE);
            gl.depth_mask(true);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.disable(glow::BLEND);
            // Back faces into the shadow map, as three.js does, keep a surface off its own shadow
            self.draw_surfaces(frame, &opaque, Kind::Depth, glow::FRONT);
            self.draw_surfaces(frame, &blended, Kind::Depth, glow::FRONT);
        }
        let gpu = self.model.as_mut().unwrap();
        (gpu.opaque, gpu.blended) = (opaque, blended);
    }

    unsafe fn draw_opaque(&mut self, frame: &Frame) {
        let Some(gpu) = self.model.as_mut() else { return };
        let opaque = std::mem::take(&mut gpu.opaque);
        unsafe {
            self.gl.disable(glow::BLEND);
            // Keeps the surface just behind its own wireframe
            self.gl.enable(glow::POLYGON_OFFSET_FILL);
            self.gl.polygon_offset(1.0, 1.0);
            self.draw_surfaces(frame, &opaque, Kind::Pbr, glow::BACK);
            self.gl.disable(glow::POLYGON_OFFSET_FILL);
        }
        self.model.as_mut().unwrap().opaque = opaque;
    }

    /// See-through surfaces, farthest first.
    unsafe fn draw_blended(&mut self, frame: &Frame, camera: &Camera) {
        let Some(gpu) = self.model.as_ref() else { return };
        let mut sorted = std::mem::take(&mut self.sorted);
        sorted.clear();
        sorted.extend(gpu.blended.iter().map(|item| {
            let center = frame.worlds[item.node].transform_point3(gpu.primitives[item.primitive].center);
            (camera.view.transform_point3(center).z, *item)
        }));
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut order = std::mem::take(&mut self.order);
        order.clear();
        order.extend(sorted.iter().map(|(_, item)| *item));
        unsafe {
            self.gl.enable(glow::POLYGON_OFFSET_FILL);
            self.gl.polygon_offset(1.0, 1.0);
            self.draw_surfaces(frame, &order, Kind::Pbr, glow::BACK);
            self.gl.disable(glow::POLYGON_OFFSET_FILL);
        }
        (self.sorted, self.order) = (sorted, order);
    }

    /// Surfaces with their materials: lit, or into the shadow map. `cull` is the side single
    /// sided ones don't draw.
    unsafe fn draw_surfaces(&mut self, frame: &Frame, items: &[Item], kind: Kind, cull: u32) {
        let Some(gpu) = self.model.as_ref() else { return };
        let gl = &self.gl;
        let mut current: Option<Key> = None;
        let mut material: Option<usize> = None;
        // What each map's unit has, so a material change rebinds only what differs
        let mut bound: [Option<(glow::Texture, Option<glow::Sampler>)>; Map::COUNT] = [None; Map::COUNT];
        unsafe {
            gl.cull_face(cull);
            for item in items {
                let primitive = &gpu.primitives[item.primitive];
                let key = Key { kind, skin: item.skin.is_some(), morph: primitive.morph.is_some() };
                let program = self.programs.get(gl, key);
                if current != Some(key) {
                    gl.use_program(Some(program.program));
                    current = Some(key);
                }
                if material != Some(primitive.material) {
                    material = Some(primitive.material);
                    let Some(m) = gpu.materials.get(primitive.material).or(gpu.materials.first()) else { continue };
                    gl.bind_buffer_base(glow::UNIFORM_BUFFER, MATERIAL_BLOCK, Some(m.ubo));
                    for (slot, map) in m.maps.iter().enumerate() {
                        let wanted = (map.map_or(self.white, |(t, _)| t), map.map(|(_, s)| s));
                        if bound[slot] != Some(wanted) {
                            gl.active_texture(glow::TEXTURE0 + slot as u32);
                            gl.bind_texture(glow::TEXTURE_2D, Some(wanted.0));
                            gl.bind_sampler(slot as u32, wanted.1);
                            bound[slot] = Some(wanted);
                        }
                    }
                    if m.double_sided {
                        gl.disable(glow::CULL_FACE);
                    } else {
                        gl.enable(glow::CULL_FACE);
                    }
                }
                gl.uniform_1_i32(program.flat.as_ref(), primitive.flat as i32);
                place(gl, program, gpu, item, frame);
                draw(gl, primitive.vao, primitive.mode, primitive.count, primitive.index_type);
            }
            gl.disable(glow::CULL_FACE);
        }
    }
}

/// Sets what puts `item` in the world: its node's matrices, or its skin, and its morph.
unsafe fn place(gl: &glow::Context, program: &Program, gpu: &GpuModel, item: &Item, frame: &Frame) {
    let primitive = &gpu.primitives[item.primitive];
    unsafe {
        if let Some(skin) = item.skin {
            gl.active_texture(glow::TEXTURE0 + unit::JOINTS);
            gl.bind_texture(glow::TEXTURE_2D, Some(gpu.skins[skin]));
            gl.front_face(glow::CCW);
        } else {
            gl.uniform_matrix_4_f32_slice(program.model.as_ref(), false, &frame.worlds[item.node].to_cols_array());
            gl.uniform_matrix_3_f32_slice(
                program.normal_matrix.as_ref(),
                false,
                &gpu.normal_matrices[item.node].to_cols_array(),
            );
            gl.front_face(if gpu.mirrored[item.node] { glow::CW } else { glow::CCW });
        }
        if let Some(morph) = &primitive.morph {
            gl.active_texture(glow::TEXTURE0 + unit::MORPHS);
            gl.bind_texture(glow::TEXTURE_2D, Some(morph.texture));
            let weights = &frame.weights[item.node];
            let count = (morph.targets as usize).min(weights.len());
            gl.uniform_1_i32(program.morph_count.as_ref(), count as i32);
            gl.uniform_1_i32(program.morph_stride.as_ref(), morph.stride);
            gl.uniform_1_i32(program.vertex_count.as_ref(), morph.vertices);
            if count > 0 {
                gl.uniform_1_f32_slice(program.morph_weights.as_ref(), &weights[..count]);
            }
        }
    }
}

impl Renderer {
    /// Points and lines, unlit, in their material's and their own colours; blended, for those
    /// that are see-through.
    unsafe fn draw_points_and_lines(&mut self, frame: &Frame) {
        let Some(gpu) = self.model.as_ref() else { return };
        let gl = &self.gl;
        unsafe {
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            for item in gpu.points.iter().chain(&gpu.lines) {
                let primitive = &gpu.primitives[item.primitive];
                let key = Key { kind: Kind::Solid, skin: item.skin.is_some(), morph: primitive.morph.is_some() };
                let program = self.programs.get(gl, key);
                gl.use_program(Some(program.program));
                let color = gpu.materials.get(primitive.material).map_or([1.0; 4], |m| m.base_color);
                gl.uniform_4_f32_slice(program.color.as_ref(), &color);
                gl.uniform_1_i32(program.vertex_colors.as_ref(), primitive.vertex_colors as i32);
                place(gl, program, gpu, item, frame);
                draw(gl, primitive.vao, primitive.mode, primitive.count, primitive.index_type);
            }
        }
    }

    /// Every surface's edges in the accent colour, over what's drawn.
    unsafe fn draw_wireframe(&mut self, frame: &Frame, settings: &Settings, opacity: f32) {
        let Some(gpu) = self.model.as_mut() else { return };
        // Made the first time they're shown
        for i in 0..gpu.opaque.len() + gpu.blended.len() {
            let item = if i < gpu.opaque.len() { gpu.opaque[i] } else { gpu.blended[i - gpu.opaque.len()] };
            gpu.wire(&self.gl, frame.model, item.primitive);
        }
        let gpu = self.model.as_ref().unwrap();
        let gl = &self.gl;
        let [r, g, b] = settings.accent;
        unsafe {
            for item in gpu.surfaces() {
                let primitive = &gpu.primitives[item.primitive];
                let wire = primitive.wire.as_ref().unwrap();
                let key = Key { kind: Kind::Solid, skin: item.skin.is_some(), morph: primitive.morph.is_some() };
                let program = self.programs.get(gl, key);
                gl.use_program(Some(program.program));
                gl.uniform_4_f32_slice(program.color.as_ref(), &[r, g, b, opacity]);
                gl.uniform_1_i32(program.vertex_colors.as_ref(), 0);
                place(gl, program, gpu, item, frame);
                draw(gl, wire.vao, glow::LINES, wire.count, Some(wire.index_type));
            }
        }
    }

    unsafe fn draw_floor(&mut self, preset: &environment::Preset, settings: &Settings) {
        let gl = &self.gl;
        let r = self.radius;
        let program = self.programs.get(gl, Key::new(Kind::Floor));
        let matrix = Mat4::from_translation(Vec3::new(0.0, -r * 0.002, 0.0)) * Mat4::from_scale(Vec3::splat(r * 12.0));
        unsafe {
            gl.use_program(Some(program.program));
            gl.uniform_matrix_4_f32_slice(program.model.as_ref(), false, &matrix.to_cols_array());
            gl.uniform_1_f32(program.opacity.as_ref(), preset.shadow * if settings.dark { 1.3 } else { 1.0 });
            draw(gl, self.floor.0, glow::TRIANGLES, 6, None);
        }
    }

    unsafe fn draw_grid(&mut self, settings: &Settings) {
        self.ensure_grid();
        let gl = &self.gl;
        let Some(grid) = &self.grid else { return };
        let program = self.programs.get(gl, Key::new(Kind::Solid));
        let color = if settings.dark { [1.0, 1.0, 1.0, 0.14] } else { [0.0, 0.0, 0.0, 0.12] };
        unsafe {
            gl.use_program(Some(program.program));
            gl.uniform_matrix_4_f32_slice(program.model.as_ref(), false, &Mat4::IDENTITY.to_cols_array());
            gl.uniform_4_f32_slice(program.color.as_ref(), &color);
            gl.uniform_1_i32(program.vertex_colors.as_ref(), 0);
            draw(gl, grid.vao, glow::LINES, grid.count, None);
        }
    }

    /// A round number of cells about a tenth of the model's size wide.
    fn ensure_grid(&mut self) {
        if self.grid.as_ref().is_some_and(|g| g.radius == self.radius) {
            return;
        }
        let gl = &self.gl;
        let radius = self.radius;
        let cell = 10f32.powf((radius / 2.0).log10().floor());
        let divisions = ((radius * 3.0 / cell).ceil() as usize * 2).clamp(2, 2000);
        let size = divisions as f32 * cell;
        let half = size / 2.0;
        let mut vertices: Vec<[f32; 3]> = Vec::with_capacity((divisions + 1) * 4);
        for i in 0..=divisions {
            let k = -half + i as f32 * cell;
            vertices.extend([[-half, 0.0, k], [half, 0.0, k], [k, 0.0, -half], [k, 0.0, half]]);
        }
        unsafe {
            if let Some(old) = self.grid.take() {
                gl.delete_vertex_array(old.vao);
                gl.delete_buffer(old.buffer);
            }
            let vao = gl.create_vertex_array().expect("vertex array");
            gl.bind_vertex_array(Some(vao));
            let buffer = gl.create_buffer().expect("buffer");
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck::cast_slice(&vertices), glow::STATIC_DRAW);
            gl.enable_vertex_attrib_array(attribute::POSITION);
            gl.vertex_attrib_pointer_f32(attribute::POSITION, 3, glow::FLOAT, false, 0, 0);
            gl.bind_vertex_array(None);
            self.grid = Some(Grid { vao, buffer, count: vertices.len() as i32, radius });
        }
    }
}

unsafe fn draw(gl: &glow::Context, vao: glow::VertexArray, mode: u32, count: i32, index_type: Option<u32>) {
    unsafe {
        gl.bind_vertex_array(Some(vao));
        match index_type {
            Some(kind) => gl.draw_elements(mode, count, kind, 0),
            None => gl.draw_arrays(mode, 0, count),
        }
    }
}
