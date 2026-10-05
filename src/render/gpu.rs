// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! A model on the GPU: a vertex array for each primitive, a uniform block for each material,
//! textures, and what moves it: joint matrices and morph targets in textures. The draw lists
//! are sorted out once, when it's uploaded.

use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasherDefault, Hasher};
use std::time::Instant;

use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Vec3};
use glow::HasContext;

use super::programs::attribute;
use crate::model::{self, AlphaMode, Map, Model, Topology};

/// The slots, in order.
const MAPS: [Map; Map::COUNT] = [
    Map::BaseColor,
    Map::MetallicRoughness,
    Map::Normal,
    Map::Occlusion,
    Map::Emissive,
    Map::SheenColor,
    Map::SheenRoughness,
    Map::Clearcoat,
    Map::ClearcoatRoughness,
    Map::ClearcoatNormal,
    Map::Specular,
    Map::SpecularColor,
    Map::Transmission,
];

/// What the GPU can do that uploading depends on.
#[derive(Clone, Copy)]
pub struct Limits {
    pub max_texture_size: u32,
    pub anisotropy: f32,
}

/// One draw: a primitive, placed by a node.
#[derive(Clone, Copy)]
pub struct Item {
    pub node: usize,
    pub primitive: usize,
    /// The skin moving it, if it's skinned
    pub skin: Option<usize>,
}

#[derive(Clone, Copy)]
struct Attribute {
    location: u32,
    buffer: glow::Buffer,
    size: i32,
    kind: u32,
    integer: bool,
}

pub struct Morph {
    pub texture: glow::Texture,
    pub targets: i32,
    pub stride: i32,
    pub vertices: i32,
}

pub struct Wire {
    pub vao: glow::VertexArray,
    ebo: glow::Buffer,
    pub count: i32,
    pub index_type: u32,
}

pub struct Primitive {
    pub vao: glow::VertexArray,
    attributes: Vec<Attribute>,
    ebo: Option<glow::Buffer>,
    /// Indices drawn, or vertices when there are none
    pub count: i32,
    pub index_type: Option<u32>,
    pub mode: u32,
    pub material: usize,
    pub flat: bool,
    pub vertex_colors: bool,
    pub morph: Option<Morph>,
    pub wire: Option<Wire>,
    /// The middle of its own box, for sorting what's see-through
    pub center: Vec3,
    /// Where it came from, for building its wireframe
    source: (usize, usize),
}

/// A material's uniform block, as pbr.frag declares it (std140).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaterialBlock {
    base_color: [f32; 4],
    emissive: [f32; 4],
    factors: [f32; 4],
    alpha: [f32; 4],
    sheen: [f32; 4],
    clearcoat: [f32; 4],
    specular: [f32; 4],
    extra: [f32; 4],
    maps: [[i32; 4]; 4],
    transforms: [[f32; 4]; Map::COUNT * 2],
}

pub struct Material {
    pub ubo: glow::Buffer,
    /// A texture and sampler for each slot it has a map in
    pub maps: [Option<(glow::Texture, glow::Sampler)>; Map::COUNT],
    pub base_color: [f32; 4],
    pub blend: bool,
    pub double_sided: bool,
}

pub struct GpuModel {
    pub primitives: Vec<Primitive>,
    pub materials: Vec<Material>,
    textures: HashMap<(usize, bool), glow::Texture>,
    samplers: HashMap<[u32; 4], glow::Sampler>,
    /// A joint matrix a row, for each skin
    pub skins: Vec<glow::Texture>,
    /// Opaque and cut-out surfaces
    pub opaque: Vec<Item>,
    /// See-through surfaces, drawn last, farthest first
    pub blended: Vec<Item>,
    pub points: Vec<Item>,
    pub lines: Vec<Item>,
    /// Each node's normal matrix, and whether it turns the model inside out
    pub normal_matrices: Vec<Mat3>,
    pub mirrored: Vec<bool>,
    pose: Option<u64>,
    joints: Vec<Mat4>,
    joint_data: Vec<f32>,
    /// Textures waiting for their pixels: texture, image, sRGB
    pending: VecDeque<(glow::Texture, usize, bool)>,
    limits: Limits,
}

impl GpuModel {
    pub fn new(gl: &glow::Context, model: &Model, limits: Limits) -> Self {
        let mut gpu = Self {
            primitives: Vec::new(),
            materials: Vec::new(),
            textures: HashMap::new(),
            samplers: HashMap::new(),
            skins: Vec::new(),
            opaque: Vec::new(),
            blended: Vec::new(),
            points: Vec::new(),
            lines: Vec::new(),
            normal_matrices: Vec::new(),
            mirrored: Vec::new(),
            pose: None,
            joints: Vec::new(),
            joint_data: Vec::new(),
            pending: VecDeque::new(),
            limits,
        };
        let materials: Vec<Material> = model.materials.iter().map(|m| gpu.material(gl, model, m, limits)).collect();
        gpu.materials = materials;

        let mut ranges = Vec::with_capacity(model.meshes.len());
        for (m, mesh) in model.meshes.iter().enumerate() {
            let start = gpu.primitives.len();
            for (p, primitive) in mesh.primitives.iter().enumerate() {
                gpu.primitives.push(upload_primitive(gl, primitive, (m, p)));
            }
            ranges.push(start..gpu.primitives.len());
        }

        for (n, node) in model.nodes.iter().enumerate() {
            let Some(mesh) = node.mesh else { continue };
            for (i, cpu) in ranges[mesh].clone().zip(&model.meshes[mesh].primitives) {
                let item = Item { node: n, primitive: i, skin: node.skin.filter(|_| !cpu.joints.is_empty()) };
                match cpu.topology {
                    Topology::Points => gpu.points.push(item),
                    Topology::Lines => gpu.lines.push(item),
                    Topology::Triangles if gpu.materials.get(cpu.material).is_some_and(|m| m.blend) => {
                        gpu.blended.push(item)
                    }
                    Topology::Triangles => gpu.opaque.push(item),
                }
            }
        }
        // Fewer switches: opaque surfaces by material
        gpu.opaque.sort_by_key(|item| (item.skin.is_some(), gpu.primitives[item.primitive].material));

        // A row of four texels for each joint, filled in as the pose changes
        for skin in &model.skins {
            unsafe {
                let texture = data_texture(gl);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA32F as i32,
                    4,
                    skin.joints.len().max(1) as i32,
                    0,
                    glow::RGBA,
                    glow::FLOAT,
                    glow::PixelUnpackData::Slice(None),
                );
                gl.bind_texture(glow::TEXTURE_2D, None);
                gpu.skins.push(texture);
            }
        }
        gpu
    }

    /// Every surface, whether opaque or not.
    pub fn surfaces(&self) -> impl Iterator<Item = &Item> {
        self.opaque.iter().chain(&self.blended)
    }

    fn material(&mut self, gl: &glow::Context, model: &Model, m: &model::Material, limits: Limits) -> Material {
        let [cc, ccr, ccs] = m.clearcoat;
        let mut block = MaterialBlock {
            base_color: m.base_color,
            emissive: [m.emissive[0], m.emissive[1], m.emissive[2], 0.0],
            factors: [m.metallic, m.roughness, m.normal_scale, m.occlusion_strength],
            alpha: [
                if let AlphaMode::Mask(cutoff) = m.alpha { cutoff } else { -1.0 },
                (m.alpha == AlphaMode::Blend) as i32 as f32,
                m.unlit as i32 as f32,
                m.double_sided as i32 as f32,
            ],
            sheen: m.sheen,
            clearcoat: [cc, ccr, ccs, 0.0],
            specular: m.specular,
            extra: [m.ior, m.transmission, 0.0, 0.0],
            maps: [[-1; 4]; 4],
            transforms: [[0.0; 4]; Map::COUNT * 2],
        };
        let mut maps = [None; Map::COUNT];
        for (slot, reference) in m.maps.iter().enumerate() {
            let [row0, row1] = reference.as_ref().map_or(model::TextureRef::IDENTITY, |r| r.transform);
            block.transforms[slot * 2] = [row0[0], row0[1], row0[2], 0.0];
            block.transforms[slot * 2 + 1] = [row1[0], row1[1], row1[2], 0.0];
            let Some(reference) = reference else { continue };
            let Some(image) = model.images.get(reference.image) else { continue };
            if image.pixels.is_empty() {
                continue;
            }
            let texture = self.texture(gl, reference.image, MAPS[slot].is_srgb());
            let sampler = self.sampler(gl, reference.sampler, limits);
            maps[slot] = Some((texture, sampler));
            block.maps[slot / 4][slot % 4] = reference.uv.min(1) as i32;
        }
        unsafe {
            let ubo = gl.create_buffer().expect("buffer");
            gl.bind_buffer(glow::UNIFORM_BUFFER, Some(ubo));
            gl.buffer_data_u8_slice(glow::UNIFORM_BUFFER, bytemuck::bytes_of(&block), glow::STATIC_DRAW);
            gl.bind_buffer(glow::UNIFORM_BUFFER, None);
            Material { ubo, maps, base_color: m.base_color, blend: m.is_blended(), double_sided: m.double_sided }
        }
    }

    /// A texture for an image, its pixels uploaded later by `upload_textures`.
    fn texture(&mut self, gl: &glow::Context, index: usize, srgb: bool) -> glow::Texture {
        if let Some(&texture) = self.textures.get(&(index, srgb)) {
            return texture;
        }
        let texture = unsafe { gl.create_texture().expect("texture") };
        self.textures.insert((index, srgb), texture);
        self.pending.push_back((texture, index, srgb));
        texture
    }

    /// Uploads textures until `deadline` (one at least), and frees each image's pixels once
    /// the GPU has all it needs of them. True when every texture is up.
    pub fn upload_textures(&mut self, gl: &glow::Context, model: &mut Model, deadline: Instant) -> bool {
        while let Some((texture, index, srgb)) = self.pending.pop_front() {
            upload_texture(gl, texture, &model.images[index], srgb, self.limits);
            if !self.pending.iter().any(|p| p.1 == index) {
                model.images[index].pixels = Vec::new();
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        self.pending.is_empty()
    }

    pub fn is_ready(&self) -> bool {
        self.pending.is_empty()
    }

    fn sampler(&mut self, gl: &glow::Context, s: model::Sampler, limits: Limits) -> glow::Sampler {
        let key = [s.mag_filter, s.min_filter, s.wrap_s, s.wrap_t];
        if let Some(&sampler) = self.samplers.get(&key) {
            return sampler;
        }
        let valid = |value: u32, allowed: &[u32], default: u32| if allowed.contains(&value) { value } else { default };
        let filters = [glow::NEAREST, glow::LINEAR];
        let min_filters = [
            glow::NEAREST,
            glow::LINEAR,
            glow::NEAREST_MIPMAP_NEAREST,
            glow::LINEAR_MIPMAP_NEAREST,
            glow::NEAREST_MIPMAP_LINEAR,
            glow::LINEAR_MIPMAP_LINEAR,
        ];
        let wraps = [glow::REPEAT, glow::CLAMP_TO_EDGE, glow::MIRRORED_REPEAT];
        unsafe {
            let sampler = gl.create_sampler().expect("sampler");
            gl.sampler_parameter_i32(
                sampler,
                glow::TEXTURE_MAG_FILTER,
                valid(s.mag_filter, &filters, glow::LINEAR) as i32,
            );
            gl.sampler_parameter_i32(
                sampler,
                glow::TEXTURE_MIN_FILTER,
                valid(s.min_filter, &min_filters, glow::LINEAR_MIPMAP_LINEAR) as i32,
            );
            gl.sampler_parameter_i32(sampler, glow::TEXTURE_WRAP_S, valid(s.wrap_s, &wraps, glow::REPEAT) as i32);
            gl.sampler_parameter_i32(sampler, glow::TEXTURE_WRAP_T, valid(s.wrap_t, &wraps, glow::REPEAT) as i32);
            if limits.anisotropy > 1.0 {
                gl.sampler_parameter_f32(sampler, glow::TEXTURE_MAX_ANISOTROPY_EXT, limits.anisotropy);
            }
            self.samplers.insert(key, sampler);
            sampler
        }
    }

    /// Brings the normal matrices and joint matrices up to `version` of the pose. True if
    /// anything moved since the last call.
    pub fn update_pose(&mut self, gl: &glow::Context, model: &Model, worlds: &[Mat4], version: u64) -> bool {
        if self.pose == Some(version) {
            return false;
        }
        self.pose = Some(version);
        self.normal_matrices.clear();
        self.mirrored.clear();
        for world in worlds {
            let linear = Mat3::from_mat4(*world);
            let det = linear.determinant();
            self.normal_matrices.push(if det.abs() > f32::MIN_POSITIVE {
                linear.inverse().transpose()
            } else {
                Mat3::IDENTITY
            });
            self.mirrored.push(det < 0.0);
        }
        for (skin, &texture) in self.skins.iter().enumerate() {
            model.joint_matrices(skin, worlds, &mut self.joints);
            if self.joints.is_empty() {
                continue;
            }
            self.joint_data.clear();
            self.joint_data.extend(self.joints.iter().flat_map(|m| m.to_cols_array()));
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    4,
                    self.joints.len() as i32,
                    glow::RGBA,
                    glow::FLOAT,
                    glow::PixelUnpackData::Slice(Some(bytemuck::cast_slice(&self.joint_data))),
                );
                gl.bind_texture(glow::TEXTURE_2D, None);
            }
        }
        true
    }

    /// The primitive's edges, made the first time they're asked for.
    pub fn wire(&mut self, gl: &glow::Context, model: &Model, index: usize) -> &Wire {
        let primitive = &mut self.primitives[index];
        if primitive.wire.is_none() {
            let (mesh, p) = primitive.source;
            let cpu = &model.meshes[mesh].primitives[p];
            // Vertices that bones or morphs move are their own, even where they start together
            let moves =
                primitive.morph.is_some() || primitive.attributes.iter().any(|a| a.location == attribute::JOINTS);
            let edges = wire_indices(&cpu.positions, &cpu.indices, !moves);
            unsafe {
                let vao = gl.create_vertex_array().expect("vertex array");
                gl.bind_vertex_array(Some(vao));
                for a in &primitive.attributes {
                    bind_attribute(gl, a);
                }
                let (ebo, index_type) = index_buffer(gl, &edges, cpu.positions.len());
                gl.bind_vertex_array(None);
                primitive.wire = Some(Wire { vao, ebo, count: edges.len() as i32, index_type });
            }
        }
        primitive.wire.as_ref().unwrap()
    }

    pub fn destroy(self, gl: &glow::Context) {
        unsafe {
            for p in self.primitives {
                gl.delete_vertex_array(p.vao);
                let mut buffers: Vec<glow::Buffer> = p.attributes.iter().map(|a| a.buffer).collect();
                buffers.dedup();
                for buffer in buffers.into_iter().chain(p.ebo) {
                    gl.delete_buffer(buffer);
                }
                if let Some(morph) = p.morph {
                    gl.delete_texture(morph.texture);
                }
                if let Some(wire) = p.wire {
                    gl.delete_vertex_array(wire.vao);
                    gl.delete_buffer(wire.ebo);
                }
            }
            for m in self.materials {
                gl.delete_buffer(m.ubo);
            }
            for (_, texture) in self.textures {
                gl.delete_texture(texture);
            }
            for (_, sampler) in self.samplers {
                gl.delete_sampler(sampler);
            }
            for texture in self.skins {
                gl.delete_texture(texture);
            }
        }
    }
}

unsafe fn bind_attribute(gl: &glow::Context, a: &Attribute) {
    unsafe {
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(a.buffer));
        gl.enable_vertex_attrib_array(a.location);
        if a.integer {
            gl.vertex_attrib_pointer_i32(a.location, a.size, a.kind, 0, 0);
        } else {
            gl.vertex_attrib_pointer_f32(a.location, a.size, a.kind, false, 0, 0);
        }
    }
}

/// An image's pixels into `texture`, with mipmaps; halved until it fits the GPU if need be.
fn upload_texture(gl: &glow::Context, texture: glow::Texture, image: &model::Image, srgb: bool, limits: Limits) {
    let (mut width, mut height) = (image.width, image.height);
    let mut pixels = std::borrow::Cow::Borrowed(&image.pixels[..]);
    if width > limits.max_texture_size || height > limits.max_texture_size {
        while width > limits.max_texture_size || height > limits.max_texture_size {
            width = (width / 2).max(1);
            height = (height / 2).max(1);
        }
        let view = image::ImageBuffer::<image::Rgba<u8>, &[u8]>::from_raw(image.width, image.height, &image.pixels)
            .expect("image size");
        pixels = image::imageops::resize(&view, width, height, image::imageops::FilterType::Triangle).into_raw().into();
    }
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            if srgb { glow::SRGB8_ALPHA8 } else { glow::RGBA8 } as i32,
            width as i32,
            height as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&pixels)),
        );
        gl.generate_mipmap(glow::TEXTURE_2D);
        gl.bind_texture(glow::TEXTURE_2D, None);
    }
}

/// Indices as 16 bits when the vertices allow it, else 32.
unsafe fn index_buffer(gl: &glow::Context, indices: &[u32], vertices: usize) -> (glow::Buffer, u32) {
    unsafe {
        let ebo = gl.create_buffer().expect("buffer");
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
        if vertices <= u16::MAX as usize + 1 {
            let short: Vec<u16> = indices.iter().map(|&i| i as u16).collect();
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytemuck::cast_slice(&short), glow::STATIC_DRAW);
            (ebo, glow::UNSIGNED_SHORT)
        } else {
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytemuck::cast_slice(indices), glow::STATIC_DRAW);
            (ebo, glow::UNSIGNED_INT)
        }
    }
}

/// An empty RGBA float texture, sampled exactly.
unsafe fn data_texture(gl: &glow::Context) -> glow::Texture {
    unsafe {
        let texture = gl.create_texture().expect("texture");
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
        gl.bind_texture(glow::TEXTURE_2D, None);
        texture
    }
}

fn upload_primitive(gl: &glow::Context, p: &model::Primitive, source: (usize, usize)) -> Primitive {
    unsafe {
        let vao = gl.create_vertex_array().expect("vertex array");
        gl.bind_vertex_array(Some(vao));
        let mut attributes = Vec::new();
        let mut add = |location, data: &[u8], size, kind, integer| {
            if data.is_empty() {
                return;
            }
            let buffer = gl.create_buffer().expect("buffer");
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, data, glow::STATIC_DRAW);
            let a = Attribute { location, buffer, size, kind, integer };
            bind_attribute(gl, &a);
            attributes.push(a);
        };
        add(attribute::POSITION, bytemuck::cast_slice(&p.positions), 3, glow::FLOAT, false);
        add(attribute::NORMAL, bytemuck::cast_slice(&p.normals), 3, glow::FLOAT, false);
        add(attribute::UV0, bytemuck::cast_slice(&p.uv0), 2, glow::FLOAT, false);
        add(attribute::UV1, bytemuck::cast_slice(&p.uv1), 2, glow::FLOAT, false);
        add(attribute::COLOR, bytemuck::cast_slice(&p.colors), 4, glow::FLOAT, false);
        add(attribute::JOINTS, bytemuck::cast_slice(&p.joints), 4, glow::UNSIGNED_SHORT, true);
        add(attribute::WEIGHTS, bytemuck::cast_slice(&p.weights), 4, glow::FLOAT, false);

        let (ebo, index_type) = if p.indices.is_empty() {
            (None, None)
        } else {
            let (ebo, kind) = index_buffer(gl, &p.indices, p.positions.len());
            (Some(ebo), Some(kind))
        };
        gl.bind_vertex_array(None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);

        let morph = (!p.targets.is_empty()).then(|| upload_morph(gl, p));
        let center = p.bounds().map_or(Vec3::ZERO, |(min, max)| (min + max) / 2.0);
        Primitive {
            vao,
            attributes,
            ebo,
            count: p.element_count() as i32,
            index_type,
            mode: match p.topology {
                Topology::Triangles => glow::TRIANGLES,
                Topology::Lines => glow::LINES,
                Topology::Points => glow::POINTS,
            },
            material: p.material,
            flat: p.flat,
            vertex_colors: !p.colors.is_empty(),
            morph,
            wire: None,
            center,
            source,
        }
    }
}

/// The targets' offsets in a texture 4096 texels wide: for each target, each vertex's
/// position offset, then its normal offset if any target has them.
unsafe fn upload_morph(gl: &glow::Context, p: &model::Primitive) -> Morph {
    const WIDTH: usize = 4096;
    let vertices = p.positions.len();
    let targets = p.targets.len().min(64);
    let stride = if p.targets.iter().any(|t| !t.normals.is_empty()) { 2 } else { 1 };
    let texels = targets * vertices * stride;
    let height = texels.div_ceil(WIDTH).max(1);
    let mut data = vec![0.0f32; WIDTH * height * 4];
    for (t, target) in p.targets.iter().take(targets).enumerate() {
        for v in 0..vertices {
            let at = ((t * vertices + v) * stride) * 4;
            data[at..at + 3].copy_from_slice(&target.positions[v]);
            if let Some(normal) = target.normals.get(v) {
                data[at + 4..at + 7].copy_from_slice(normal);
            }
        }
    }
    unsafe {
        let texture = data_texture(gl);
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA32F as i32,
            WIDTH as i32,
            height as i32,
            0,
            glow::RGBA,
            glow::FLOAT,
            glow::PixelUnpackData::Slice(Some(bytemuck::cast_slice(&data))),
        );
        gl.bind_texture(glow::TEXTURE_2D, None);
        Morph { texture, targets: targets as i32, stride: stride as i32, vertices: vertices as i32 }
    }
}

/// A quick hash for vertex positions: the default one is made to resist attacks, not to be fast.
#[derive(Default)]
struct QuickHasher(u64);

impl Hasher for QuickHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
    }

    fn write_u32(&mut self, n: u32) {
        self.write_u64(n as u64);
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// Each edge of the triangles once, as pairs of indices. With `weld`, corners at the same place
/// count as one vertex, so where a seam splits vertices the edge isn't drawn twice.
pub fn wire_indices(positions: &[[f32; 3]], indices: &[u32], weld: bool) -> Vec<u32> {
    let canonical: Vec<u32> = if weld {
        let mut seen: HashMap<[u32; 3], u32, BuildHasherDefault<QuickHasher>> = HashMap::default();
        seen.reserve(positions.len());
        positions.iter().enumerate().map(|(i, p)| *seen.entry(p.map(f32::to_bits)).or_insert(i as u32)).collect()
    } else {
        (0..positions.len() as u32).collect()
    };

    let corner = |i: usize| if indices.is_empty() { i as u32 } else { indices[i] };
    let triangles = if indices.is_empty() { positions.len() } else { indices.len() } / 3;
    let mut edges: Vec<u64> = Vec::with_capacity(triangles * 3);
    for t in 0..triangles {
        let [a, b, c] = [0, 1, 2].map(|k| canonical[corner(t * 3 + k) as usize]);
        for (x, y) in [(a, b), (b, c), (c, a)] {
            if x != y {
                edges.push((x.min(y) as u64) << 32 | x.max(y) as u64);
            }
        }
    }
    edges.sort_unstable();
    edges.dedup();
    edges.into_iter().flat_map(|e| [(e >> 32) as u32, e as u32]).collect()
}

#[cfg(test)]
mod tests {
    use super::wire_indices;

    #[test]
    fn a_quad_has_five_edges() {
        let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
        assert_eq!(wire_indices(&positions, &[0, 1, 2, 0, 2, 3], true).len(), 10);
        // The same quad as separate triangles: still five
        let split = [positions[0], positions[1], positions[2], positions[0], positions[2], positions[3]];
        assert_eq!(wire_indices(&split, &[], true).len(), 10);
        // Unwelded, as for a skinned mesh: the shared edge is each triangle's own
        assert_eq!(wire_indices(&split, &[], false).len(), 12);
    }
}
