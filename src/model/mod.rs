// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! A model as the loaders read it, before the GPU sees it: nodes, meshes, materials, images,
//! skins and animation clips. Every format comes out as one of these; `load()` also stands it
//! on the floor and measures it for the properties sidebar. Nothing here needs a display.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use glam::{Mat4, Quat, Vec3};

use crate::formats::Loader;

pub mod animation;
mod fbx;
pub mod files;
mod gltf;
mod ply;
mod stl;
mod threemf;

pub use animation::Pose;

#[derive(Default)]
pub struct Model {
    /// Parents always come before their children.
    pub nodes: Vec<Node>,
    pub meshes: Vec<Mesh>,
    pub materials: Vec<Material>,
    pub images: Vec<Image>,
    pub skins: Vec<Skin>,
    pub clips: Vec<Clip>,
    /// Above the root nodes: turns a Z-up format upright, and stands the model on the floor.
    pub root: Mat4,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Transform {
    pub const IDENTITY: Self = Self { translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };

    pub fn from_matrix(matrix: Mat4) -> Self {
        let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
        Self { translation, rotation, scale }
    }

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[derive(Default)]
pub struct Node {
    pub parent: Option<usize>,
    pub transform: Transform,
    pub mesh: Option<usize>,
    pub skin: Option<usize>,
    /// Morph target weights, when the mesh has targets.
    pub weights: Vec<f32>,
}

#[derive(Default)]
pub struct Mesh {
    pub primitives: Vec<Primitive>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Topology {
    #[default]
    Triangles,
    Lines,
    Points,
}

/// One draw: vertex attributes and the material they're drawn with. Absent attributes are
/// empty; present ones have one entry per position.
#[derive(Default)]
pub struct Primitive {
    pub topology: Topology,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uv0: Vec<[f32; 2]>,
    pub uv1: Vec<[f32; 2]>,
    /// Linear RGBA.
    pub colors: Vec<[f32; 4]>,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    /// Empty when the vertices are drawn in order.
    pub indices: Vec<u32>,
    pub targets: Vec<MorphTarget>,
    pub material: usize,
    /// No normals in the file: the surface is shaded flat, facet by facet.
    pub flat: bool,
}

impl Primitive {
    /// The number of vertices drawn: one per index, or per position when there are none.
    pub fn element_count(&self) -> usize {
        if self.indices.is_empty() { self.positions.len() } else { self.indices.len() }
    }

    /// Its own box, grown to hold how far its morph targets can move it (as three.js has it).
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let (mut min, mut max) = bounds_of(self.positions.iter().map(|&p| Vec3::from(p)))?;
        for target in &self.targets {
            if let Some((low, high)) = bounds_of(target.positions.iter().map(|&p| Vec3::from(p))) {
                min = min.min(min + low);
                max = max.max(max + high);
            }
        }
        Some((min, max))
    }
}

#[derive(Default)]
pub struct MorphTarget {
    pub positions: Vec<[f32; 3]>,
    /// Empty, or one per position.
    pub normals: Vec<[f32; 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaMode {
    Opaque,
    /// Fragments under the cutoff are cut away.
    Mask(f32),
    Blend,
}

/// The textures a material can have, each in its own slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Map {
    BaseColor,
    /// Roughness in green, metalness in blue
    MetallicRoughness,
    Normal,
    Occlusion,
    Emissive,
    SheenColor,
    /// In alpha
    SheenRoughness,
    /// In red
    Clearcoat,
    /// In green
    ClearcoatRoughness,
    ClearcoatNormal,
    /// In alpha
    Specular,
    SpecularColor,
    /// In red
    Transmission,
}

impl Map {
    pub const COUNT: usize = 13;

    /// Colours are stored as sRGB; everything else is linear data.
    pub fn is_srgb(self) -> bool {
        matches!(self, Self::BaseColor | Self::Emissive | Self::SheenColor | Self::SpecularColor)
    }
}

/// Metallic-roughness PBR, as glTF has it and three.js's MeshPhysicalMaterial draws it; the
/// other formats map their materials onto it. Colours are linear.
#[derive(Clone, Debug)]
pub struct Material {
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub normal_scale: f32,
    pub occlusion_strength: f32,
    pub emissive: [f32; 3],
    /// Colour and roughness of a velvety sheen; black for none
    pub sheen: [f32; 4],
    /// Strength, roughness and normal scale of a clear coat over the surface
    pub clearcoat: [f32; 3],
    /// Colour and strength of the specular reflection
    pub specular: [f32; 4],
    pub ior: f32,
    /// How much light passes through, as through glass
    pub transmission: f32,
    pub maps: [Option<TextureRef>; Map::COUNT],
    pub alpha: AlphaMode,
    pub double_sided: bool,
    pub unlit: bool,
}

impl Material {
    /// For a model without materials of its own: a soft light grey.
    pub fn neutral() -> Self {
        let [r, g, b] = srgb_hex(0xdeddda);
        Self { base_color: [r, g, b, 1.0], metallic: 0.05, roughness: 0.55, ..Self::default() }
    }

    pub fn map(&self, map: Map) -> Option<&TextureRef> {
        self.maps[map as usize].as_ref()
    }

    pub fn set_map(&mut self, map: Map, texture: Option<TextureRef>) {
        self.maps[map as usize] = texture;
    }

    /// Drawn over what's behind it: see-through, or letting light through.
    pub fn is_blended(&self) -> bool {
        self.alpha == AlphaMode::Blend || self.transmission > 0.0
    }
}

impl Default for Material {
    /// glTF's default material: white, fully metallic and rough.
    fn default() -> Self {
        Self {
            base_color: [1.0; 4],
            metallic: 1.0,
            roughness: 1.0,
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            emissive: [0.0; 3],
            sheen: [0.0; 4],
            clearcoat: [0.0, 0.0, 1.0],
            specular: [1.0; 4],
            ior: 1.5,
            transmission: 0.0,
            maps: Default::default(),
            alpha: AlphaMode::Opaque,
            double_sided: false,
            unlit: false,
        }
    }
}

/// GL's sampler values, which glTF uses too.
pub mod sampler {
    pub const NEAREST: u32 = 0x2600;
    pub const LINEAR: u32 = 0x2601;
    pub const LINEAR_MIPMAP_LINEAR: u32 = 0x2703;
    pub const REPEAT: u32 = 0x2901;
    pub const CLAMP_TO_EDGE: u32 = 0x812F;
    pub const MIRRORED_REPEAT: u32 = 0x8370;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sampler {
    pub mag_filter: u32,
    pub min_filter: u32,
    pub wrap_s: u32,
    pub wrap_t: u32,
}

impl Default for Sampler {
    fn default() -> Self {
        Self {
            mag_filter: sampler::LINEAR,
            min_filter: sampler::LINEAR_MIPMAP_LINEAR,
            wrap_s: sampler::REPEAT,
            wrap_t: sampler::REPEAT,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TextureRef {
    pub image: usize,
    /// Which texture coordinates: 0 or 1.
    pub uv: u32,
    /// Rows of the 2D affine transform applied to the texture coordinates.
    pub transform: [[f32; 3]; 2],
    pub sampler: Sampler,
}

impl TextureRef {
    pub const IDENTITY: [[f32; 3]; 2] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

    pub fn new(image: usize) -> Self {
        Self { image, uv: 0, transform: Self::IDENTITY, sampler: Sampler::default() }
    }
}

/// Decoded pixels: RGBA, 8 bits a channel, straight alpha, top row first.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Image {
    /// The widest a texture is kept: more than a viewer shows, and four times the memory.
    pub const MAX_SIZE: u32 = 4096;

    /// Decodes an image file, halving it until it's no wider or taller than `MAX_SIZE`.
    pub fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        let mut image = image::load_from_memory(bytes)?.into_rgba8();
        let (mut width, mut height) = image.dimensions();
        if width > Self::MAX_SIZE || height > Self::MAX_SIZE {
            while width > Self::MAX_SIZE || height > Self::MAX_SIZE {
                width = (width / 2).max(1);
                height = (height / 2).max(1);
            }
            image = image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle);
        }
        Ok(Self { width: image.width(), height: image.height(), pixels: image.into_raw() })
    }
}

pub struct Skin {
    pub joints: Vec<usize>,
    pub inverse_binds: Vec<Mat4>,
}

pub struct Clip {
    pub name: String,
    pub duration: f32,
    pub channels: Vec<Channel>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Property {
    Translation,
    Rotation,
    Scale,
    Weights,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    Step,
    Linear,
    CubicSpline,
}

pub struct Channel {
    pub node: usize,
    pub property: Property,
    pub interpolation: Interpolation,
    pub times: Vec<f32>,
    /// Flattened, `values.len() / times.len()` floats a key (three times that for a cubic
    /// spline: in-tangent, value, out-tangent).
    pub values: Vec<f32>,
}

/// Why a file shows nothing.
#[derive(Debug)]
pub enum Error {
    NotFound,
    Unreadable(String),
    /// Damaged, or uses something the loaders don't support.
    Unsupported(String),
    /// Refers to a file outside its folder
    Outside(String),
    /// Refers to a file that isn't there
    Missing(String),
    Empty,
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::NotFound => Self::NotFound,
            _ => Self::Unreadable(error.to_string()),
        }
    }
}

impl From<anyhow::Error> for Error {
    fn from(error: anyhow::Error) -> Self {
        match error.downcast_ref::<files::Reference>() {
            Some(files::Reference::Outside(reference)) => Self::Outside(reference.clone()),
            Some(files::Reference::Missing(path)) => Self::Missing(path.clone()),
            None => Self::Unsupported(format!("{error:#}")),
        }
    }
}

/// What the properties sidebar shows of a model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    /// Width, height and depth, in the model's own units.
    pub size: [f32; 3],
    pub vertices: u64,
    pub triangles: u64,
    pub points: u64,
    pub meshes: u64,
    pub materials: u64,
    /// Name and duration in seconds.
    pub clips: Vec<(String, f32)>,
}

/// A model loaded, stood on the floor at the origin, and measured.
pub struct Loaded {
    pub model: Model,
    pub stats: Stats,
    /// Of the bounding sphere.
    pub radius: f32,
    pub height: f32,
}

/// Reads the model at `path`. Files it refers to (a glTF's buffers, textures) are read only
/// from its own folder.
pub fn load(path: &Path, loader: Loader) -> Result<Loaded, Error> {
    let folder = path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let bytes = std::fs::read(path)?;
    let mut model = match loader {
        Loader::Gltf => gltf::load(&bytes, &folder)?,
        Loader::Obj => fbx::load(&bytes, path, fbx::Kind::Obj)?,
        Loader::Fbx => fbx::load(&bytes, path, fbx::Kind::Fbx)?,
        Loader::Stl => stl::load(&bytes)?,
        Loader::Ply => ply::load(&bytes)?,
        Loader::ThreeMf => threemf::load(&bytes)?,
    };
    drop(bytes);
    let (min, max) = model.bounds().ok_or(Error::Empty)?;
    let size = max - min;
    let center = (min + max) / 2.0;
    // Stand the model on the floor, at the origin
    model.root = Mat4::from_translation(-Vec3::new(center.x, min.y, center.z)) * model.root;
    let stats = model.measure(size);
    Ok(Loaded { model, stats, radius: (size.length() / 2.0).max(1e-6), height: size.y })
}

impl Model {
    pub fn rest_pose(&self) -> Pose {
        Pose {
            transforms: self.nodes.iter().map(|n| n.transform).collect(),
            weights: self.nodes.iter().map(|n| n.weights.clone()).collect(),
        }
    }

    /// Each node's transform to the world, in `pose`.
    pub fn world_matrices(&self, pose: &Pose, worlds: &mut Vec<Mat4>) {
        worlds.clear();
        for (i, node) in self.nodes.iter().enumerate() {
            let local = pose.transforms[i].matrix();
            let parent = node.parent.map_or(self.root, |p| worlds[p]);
            worlds.push(parent * local);
        }
    }

    /// The joint matrices of `skin`: from the mesh's own space to the world.
    pub fn joint_matrices(&self, skin: usize, worlds: &[Mat4], out: &mut Vec<Mat4>) {
        let skin = &self.skins[skin];
        out.clear();
        out.extend(skin.joints.iter().zip(&skin.inverse_binds).map(|(&j, &ib)| worlds[j] * ib));
    }

    /// The box the model fills at rest. A skinned mesh is posed vertex by vertex; any other is
    /// measured by the corners of its own box, as three.js did.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let pose = self.rest_pose();
        let mut worlds = Vec::new();
        self.world_matrices(&pose, &mut worlds);
        let mut joints = Vec::new();
        let mut points = Vec::new();
        for (i, node) in self.nodes.iter().enumerate() {
            let Some(mesh) = node.mesh else { continue };
            for primitive in &self.meshes[mesh].primitives {
                if let Some(skin) = node.skin.filter(|_| !primitive.joints.is_empty()) {
                    self.joint_matrices(skin, &worlds, &mut joints);
                    for (v, p) in primitive.positions.iter().enumerate() {
                        let matrix = skin_matrix(&joints, primitive.joints[v], primitive.weights[v]);
                        points.push(matrix.transform_point3(Vec3::from(*p)));
                    }
                } else if let Some((min, max)) = primitive.bounds() {
                    for corner in 0..8 {
                        let pick = |bit: u32, lo: f32, hi: f32| if corner & bit == 0 { lo } else { hi };
                        let local = Vec3::new(pick(1, min.x, max.x), pick(2, min.y, max.y), pick(4, min.z, max.z));
                        points.push(worlds[i].transform_point3(local));
                    }
                }
            }
        }
        bounds_of(points.into_iter()).filter(|(min, max)| min.is_finite() && max.is_finite())
    }

    fn measure(&self, size: Vec3) -> Stats {
        let mut stats = Stats { size: size.to_array(), ..Stats::default() };
        let mut materials = HashSet::new();
        for node in &self.nodes {
            let Some(mesh) = node.mesh else { continue };
            for primitive in &self.meshes[mesh].primitives {
                let vertices = primitive.positions.len() as u64;
                match primitive.topology {
                    Topology::Triangles => {
                        stats.meshes += 1;
                        stats.vertices += vertices;
                        stats.triangles += primitive.element_count() as u64 / 3;
                        materials.insert(primitive.material);
                    }
                    Topology::Points => stats.points += vertices,
                    Topology::Lines => {}
                }
            }
        }
        stats.materials = materials.len() as u64;
        stats.clips = self
            .clips
            .iter()
            .enumerate()
            .map(|(i, clip)| {
                let name = if clip.name.is_empty() { (i + 1).to_string() } else { clip.name.clone() };
                (name, clip.duration)
            })
            .collect();
        stats
    }

    /// Frees what the GPU now has its own copy of. Positions and indices stay: the wireframe
    /// is made from them when it's first shown.
    pub fn release_gpu_data(&mut self) {
        for primitive in self.meshes.iter_mut().flat_map(|m| &mut m.primitives) {
            primitive.normals = Vec::new();
            primitive.uv0 = Vec::new();
            primitive.uv1 = Vec::new();
            primitive.colors = Vec::new();
            primitive.weights = Vec::new();
            primitive.joints = Vec::new();
            primitive.targets = Vec::new();
        }
        self.images = Vec::new();
    }
}

pub fn skin_matrix(joints: &[Mat4], indices: [u16; 4], weights: [f32; 4]) -> Mat4 {
    let mut matrix = Mat4::ZERO;
    for k in 0..4 {
        if weights[k] != 0.0 {
            matrix += joints.get(indices[k] as usize).copied().unwrap_or(Mat4::IDENTITY) * weights[k];
        }
    }
    matrix
}

fn bounds_of(points: impl Iterator<Item = Vec3>) -> Option<(Vec3, Vec3)> {
    points.fold(None, |bounds, p| match bounds {
        None => Some((p, p)),
        Some((min, max)) => Some((min.min(p), max.max(p))),
    })
}

/// Normals for an indexed triangle mesh, each the sum of the faces around it weighted by
/// their area (as three.js's computeVertexNormals).
pub fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    let mut add = |[a, b, c]: [usize; 3]| {
        let (Some(&pa), Some(&pb), Some(&pc)) = (positions.get(a), positions.get(b), positions.get(c)) else {
            return;
        };
        let pa = Vec3::from(pa);
        let n = (Vec3::from(pb) - pa).cross(Vec3::from(pc) - pa);
        normals[a] += n;
        normals[b] += n;
        normals[c] += n;
    };
    if indices.is_empty() {
        for t in 0..positions.len() / 3 {
            add([3 * t, 3 * t + 1, 3 * t + 2]);
        }
    } else {
        for t in indices.chunks_exact(3) {
            add([t[0] as usize, t[1] as usize, t[2] as usize]);
        }
    }
    normals.into_iter().map(|n| n.normalize_or_zero().to_array()).collect()
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// A colour written as 0xRRGGBB, in linear RGB.
pub fn srgb_hex(hex: u32) -> [f32; 3] {
    [16, 8, 0].map(|shift| srgb_to_linear(((hex >> shift) & 0xff) as f32 / 255.0))
}

/// Triangle list indices for a strip or a fan of `count` vertices (or of `indices`).
pub fn triangulate(mode: u32, indices: &[u32]) -> Vec<u32> {
    const TRIANGLE_STRIP: u32 = 5;
    const TRIANGLE_FAN: u32 = 6;
    let mut out = Vec::with_capacity(indices.len().saturating_sub(2) * 3);
    for i in 2..indices.len() {
        let triangle = match mode {
            TRIANGLE_STRIP if i % 2 == 0 => [indices[i - 2], indices[i - 1], indices[i]],
            TRIANGLE_STRIP => [indices[i - 1], indices[i - 2], indices[i]],
            TRIANGLE_FAN => [indices[0], indices[i - 1], indices[i]],
            _ => unreachable!(),
        };
        out.extend_from_slice(&triangle);
    }
    out
}
