// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! glTF 2.0, as .gltf (JSON, with its buffers and images beside it or inline) or .glb.
//! Meshopt- and Draco-compressed meshes are decoded.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ffi::{c_int, c_void};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, bail, ensure};
use glam::Mat4;
use serde::Deserialize;

use super::{
    AlphaMode, Channel, Clip, Image, Interpolation, Map, Material, Mesh, Model, MorphTarget, Node, Primitive, Property,
    Sampler, Skin, TextureRef, Topology, Transform, files, triangulate,
};

/// The extensions a file may require that the loader understands, or that only take away
/// detail when they're left out (lights, volume, iridescence: as three.js had them, nearly).
const SUPPORTED: &[&str] = &[
    "KHR_texture_transform",
    "KHR_mesh_quantization",
    "KHR_draco_mesh_compression",
    "EXT_meshopt_compression",
    "KHR_meshopt_compression",
    "EXT_mesh_gpu_instancing",
    "EXT_texture_webp",
    "KHR_lights_punctual",
    "KHR_materials_unlit",
    "KHR_materials_emissive_strength",
    "KHR_materials_sheen",
    "KHR_materials_clearcoat",
    "KHR_materials_specular",
    "KHR_materials_ior",
    "KHR_materials_transmission",
    "KHR_materials_volume",
    "KHR_materials_iridescence",
    "KHR_materials_anisotropy",
    "KHR_materials_dispersion",
    "KHR_materials_variants",
];

pub fn load(bytes: &[u8], folder: &Path) -> anyhow::Result<Model> {
    let (json, bin) = split_glb(bytes)?;
    let root: Root = serde_json::from_slice(json).context("not glTF")?;
    ensure!(root.asset.version.starts_with('2'), "glTF {} isn’t supported", root.asset.version);
    if let Some(missing) = root.extensions_required.iter().find(|e| !SUPPORTED.contains(&e.as_str())) {
        bail!("needs {missing}");
    }
    let data = Data::new(&root, bin, folder)?;
    Loader { root: &root, data: &data }.model(folder)
}

/// The JSON chunk and the binary chunk of a .glb, or the whole file as JSON.
fn split_glb(bytes: &[u8]) -> anyhow::Result<(&[u8], Option<&[u8]>)> {
    if !bytes.starts_with(b"glTF") {
        return Ok((bytes, None));
    }
    let word = |at: usize| -> anyhow::Result<u32> {
        let b = bytes.get(at..at + 4).context("truncated GLB")?;
        Ok(u32::from_le_bytes(b.try_into().unwrap()))
    };
    ensure!(word(4)? == 2, "GLB version {} isn’t supported", word(4)?);
    let length = (word(8)? as usize).min(bytes.len());
    let (mut json, mut bin) = (None, None);
    let mut at = 12;
    while at + 8 <= length {
        let size = word(at)? as usize;
        let kind = word(at + 4)?;
        let chunk = bytes.get(at + 8..at + 8 + size).context("truncated GLB chunk")?;
        match kind {
            0x4E4F_534A => json = json.or(Some(chunk)),
            0x004E_4942 => bin = bin.or(Some(chunk)),
            _ => {}
        }
        at += 8 + size.next_multiple_of(4);
    }
    Ok((json.context("GLB without JSON")?, bin))
}

// The parts of the glTF JSON the loader reads. Anything else in a file is ignored.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Root {
    asset: Asset,
    #[serde(default)]
    extensions_required: Vec<String>,
    #[serde(default)]
    accessors: Vec<Accessor>,
    #[serde(default)]
    animations: Vec<AnimationDef>,
    #[serde(default)]
    buffers: Vec<Buffer>,
    #[serde(default)]
    buffer_views: Vec<BufferView>,
    #[serde(default)]
    images: Vec<ImageDef>,
    #[serde(default)]
    materials: Vec<MaterialDef>,
    #[serde(default)]
    meshes: Vec<MeshDef>,
    #[serde(default)]
    nodes: Vec<NodeDef>,
    #[serde(default)]
    samplers: Vec<SamplerDef>,
    scene: Option<usize>,
    #[serde(default)]
    scenes: Vec<SceneDef>,
    #[serde(default)]
    skins: Vec<SkinDef>,
    #[serde(default)]
    textures: Vec<TextureDef>,
}

#[derive(Deserialize)]
struct Asset {
    version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Accessor {
    buffer_view: Option<usize>,
    #[serde(default)]
    byte_offset: usize,
    component_type: u32,
    #[serde(default)]
    normalized: bool,
    count: usize,
    #[serde(rename = "type")]
    kind: String,
    sparse: Option<Sparse>,
}

#[derive(Deserialize)]
struct Sparse {
    count: usize,
    indices: SparseIndices,
    values: SparseValues,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SparseIndices {
    buffer_view: usize,
    #[serde(default)]
    byte_offset: usize,
    component_type: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SparseValues {
    buffer_view: usize,
    #[serde(default)]
    byte_offset: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnimationDef {
    #[serde(default)]
    name: String,
    channels: Vec<ChannelDef>,
    samplers: Vec<AnimationSampler>,
}

#[derive(Deserialize)]
struct ChannelDef {
    sampler: usize,
    target: ChannelTarget,
}

#[derive(Deserialize)]
struct ChannelTarget {
    node: Option<usize>,
    path: String,
}

#[derive(Deserialize)]
struct AnimationSampler {
    input: usize,
    #[serde(default)]
    interpolation: Option<String>,
    output: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Buffer {
    uri: Option<String>,
    byte_length: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BufferView {
    buffer: usize,
    #[serde(default)]
    byte_offset: usize,
    byte_length: usize,
    byte_stride: Option<usize>,
    #[serde(default)]
    extensions: BufferViewExtensions,
}

#[derive(Default, Deserialize)]
struct BufferViewExtensions {
    #[serde(rename = "EXT_meshopt_compression", alias = "KHR_meshopt_compression")]
    meshopt: Option<Meshopt>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meshopt {
    buffer: usize,
    #[serde(default)]
    byte_offset: usize,
    byte_length: usize,
    byte_stride: usize,
    count: usize,
    mode: String,
    #[serde(default)]
    filter: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImageDef {
    uri: Option<String>,
    buffer_view: Option<usize>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct MaterialDef {
    pbr_metallic_roughness: Option<Pbr>,
    normal_texture: Option<TextureInfo>,
    occlusion_texture: Option<TextureInfo>,
    emissive_texture: Option<TextureInfo>,
    emissive_factor: Option<[f32; 3]>,
    alpha_mode: Option<String>,
    alpha_cutoff: Option<f32>,
    double_sided: bool,
    extensions: MaterialExtensions,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct MaterialExtensions {
    #[serde(rename = "KHR_materials_unlit")]
    unlit: Option<serde_json::Value>,
    #[serde(rename = "KHR_materials_emissive_strength")]
    emissive_strength: Option<EmissiveStrength>,
    #[serde(rename = "KHR_materials_sheen")]
    sheen: Option<Sheen>,
    #[serde(rename = "KHR_materials_clearcoat")]
    clearcoat: Option<Clearcoat>,
    #[serde(rename = "KHR_materials_specular")]
    specular: Option<Specular>,
    #[serde(rename = "KHR_materials_ior")]
    ior: Option<Ior>,
    #[serde(rename = "KHR_materials_transmission")]
    transmission: Option<Transmission>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Sheen {
    sheen_color_factor: Option<[f32; 3]>,
    sheen_color_texture: Option<TextureInfo>,
    sheen_roughness_factor: Option<f32>,
    sheen_roughness_texture: Option<TextureInfo>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Clearcoat {
    clearcoat_factor: Option<f32>,
    clearcoat_texture: Option<TextureInfo>,
    clearcoat_roughness_factor: Option<f32>,
    clearcoat_roughness_texture: Option<TextureInfo>,
    clearcoat_normal_texture: Option<TextureInfo>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Specular {
    specular_factor: Option<f32>,
    specular_texture: Option<TextureInfo>,
    specular_color_factor: Option<[f32; 3]>,
    specular_color_texture: Option<TextureInfo>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Ior {
    ior: Option<f32>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Transmission {
    transmission_factor: Option<f32>,
    transmission_texture: Option<TextureInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmissiveStrength {
    #[serde(default = "one")]
    emissive_strength: f32,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Pbr {
    base_color_factor: Option<[f32; 4]>,
    base_color_texture: Option<TextureInfo>,
    metallic_factor: Option<f32>,
    roughness_factor: Option<f32>,
    metallic_roughness_texture: Option<TextureInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TextureInfo {
    index: usize,
    #[serde(default)]
    tex_coord: u32,
    /// A normal texture's scale
    scale: Option<f32>,
    /// An occlusion texture's strength
    strength: Option<f32>,
    #[serde(default)]
    extensions: TextureInfoExtensions,
}

#[derive(Default, Deserialize)]
struct TextureInfoExtensions {
    #[serde(rename = "KHR_texture_transform")]
    transform: Option<TextureTransform>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TextureTransform {
    offset: Option<[f32; 2]>,
    rotation: Option<f32>,
    scale: Option<[f32; 2]>,
    tex_coord: Option<u32>,
}

#[derive(Deserialize)]
struct MeshDef {
    primitives: Vec<PrimitiveDef>,
    weights: Option<Vec<f32>>,
}

#[derive(Deserialize)]
struct PrimitiveDef {
    attributes: HashMap<String, usize>,
    indices: Option<usize>,
    material: Option<usize>,
    #[serde(default = "triangles")]
    mode: u32,
    #[serde(default)]
    targets: Vec<HashMap<String, usize>>,
    #[serde(default)]
    extensions: PrimitiveExtensions,
}

#[derive(Default, Deserialize)]
struct PrimitiveExtensions {
    #[serde(rename = "KHR_draco_mesh_compression")]
    draco: Option<DracoDef>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DracoDef {
    buffer_view: usize,
    /// Each attribute's id in the Draco data
    attributes: HashMap<String, u32>,
}

/// A primitive's Draco data, decoded: its indices, then each attribute, by Draco's id.
struct Draco {
    decoded: draco_decoder::MeshDecodeResult,
    ids: HashMap<String, u32>,
}

impl Draco {
    fn indices(&self) -> Vec<u32> {
        let config = &self.decoded.config;
        let bytes = &self.decoded.data[..(config.index_length() as usize).min(self.decoded.data.len())];
        if config.index_length() as usize == config.index_count() as usize * 2 {
            bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]]) as u32).collect()
        } else {
            bytes.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect()
        }
    }

    /// An attribute as floats, as an accessor would give them (normalized if it says so).
    fn floats(&self, name: &str, normalized: bool) -> Option<anyhow::Result<(Vec<f32>, usize)>> {
        use draco_decoder::AttributeDataType as Type;
        let id = *self.ids.get(name)?;
        let attribute = self.decoded.config.attributes().into_iter().find(|a| a.unique_id() == id)?;
        Some((|| {
            let width = attribute.dim() as usize;
            ensure!((1..=4).contains(&width), "a Draco attribute of {width} components");
            let start = attribute.offset() as usize;
            let data =
                self.decoded.data.get(start..start + attribute.lenght() as usize).context("Draco data out of range")?;
            let component_type = match attribute.data_type() {
                Type::Int8 => 5120,
                Type::UInt8 => 5121,
                Type::Int16 => 5122,
                Type::UInt16 => 5123,
                Type::UInt32 => 5125,
                Type::Float32 => 5126,
                Type::Int32 => bail!("32-bit signed Draco attributes aren’t supported"),
            };
            let count = data.len() / component_size(component_type)? / width;
            let mut out = vec![0.0; count * width];
            read_floats(&mut out, data, 0, 0, width, component_type, normalized)?;
            Ok((out, width))
        })())
    }
}

#[derive(Deserialize)]
struct NodeDef {
    #[serde(default)]
    children: Vec<usize>,
    matrix: Option<[f32; 16]>,
    translation: Option<[f32; 3]>,
    rotation: Option<[f32; 4]>,
    scale: Option<[f32; 3]>,
    mesh: Option<usize>,
    skin: Option<usize>,
    weights: Option<Vec<f32>>,
    #[serde(default)]
    extensions: NodeExtensions,
}

#[derive(Default, Deserialize)]
struct NodeExtensions {
    #[serde(rename = "EXT_mesh_gpu_instancing")]
    instancing: Option<Instancing>,
}

#[derive(Deserialize)]
struct Instancing {
    attributes: HashMap<String, usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SamplerDef {
    mag_filter: Option<u32>,
    min_filter: Option<u32>,
    wrap_s: Option<u32>,
    wrap_t: Option<u32>,
}

#[derive(Deserialize)]
struct SceneDef {
    #[serde(default)]
    nodes: Vec<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkinDef {
    inverse_bind_matrices: Option<usize>,
    joints: Vec<usize>,
}

#[derive(Deserialize)]
struct TextureDef {
    sampler: Option<usize>,
    source: Option<usize>,
    #[serde(default)]
    extensions: TextureExtensions,
}

#[derive(Default, Deserialize)]
struct TextureExtensions {
    #[serde(rename = "EXT_texture_webp")]
    webp: Option<TextureSource>,
}

#[derive(Deserialize)]
struct TextureSource {
    source: usize,
}

fn one() -> f32 {
    1.0
}

fn triangles() -> u32 {
    4
}

// Binary data: the buffers, and the buffer views decoded from meshopt compression.

struct Data<'a> {
    buffers: Vec<Cow<'a, [u8]>>,
    views: Vec<Option<Vec<u8>>>,
    /// Bytes the accessors have given so far: a file reusing one accessor everywhere, or
    /// declaring huge empty ones, stops here rather than exhausting memory
    spent: std::cell::Cell<usize>,
    root: &'a Root,
}

unsafe extern "C" {
    fn meshopt_decodeVertexBuffer(dst: *mut c_void, count: usize, size: usize, src: *const u8, len: usize) -> c_int;
    fn meshopt_decodeIndexBuffer(dst: *mut c_void, count: usize, size: usize, src: *const u8, len: usize) -> c_int;
    fn meshopt_decodeIndexSequence(dst: *mut c_void, count: usize, size: usize, src: *const u8, len: usize) -> c_int;
    fn meshopt_decodeFilterOct(buffer: *mut c_void, count: usize, stride: usize);
    fn meshopt_decodeFilterQuat(buffer: *mut c_void, count: usize, stride: usize);
    fn meshopt_decodeFilterExp(buffer: *mut c_void, count: usize, stride: usize);
    fn meshopt_decodeFilterColor(buffer: *mut c_void, count: usize, stride: usize);
}

impl<'a> Data<'a> {
    fn new(root: &'a Root, bin: Option<&'a [u8]>, folder: &Path) -> anyhow::Result<Self> {
        let mut buffers = Vec::with_capacity(root.buffers.len());
        for (i, buffer) in root.buffers.iter().enumerate() {
            let data: Cow<[u8]> = match &buffer.uri {
                Some(uri) => Cow::Owned(files::read(folder, uri)?),
                None if i == 0 && bin.is_some() => Cow::Borrowed(bin.unwrap()),
                // A meshopt fallback buffer: never read
                None => Cow::Borrowed(&[]),
            };
            ensure!(data.is_empty() || data.len() >= buffer.byte_length, "buffer {i} is shorter than it says");
            buffers.push(data);
        }
        let mut data = Self { buffers, views: Vec::new(), root, spent: std::cell::Cell::new(0) };
        data.views = root
            .buffer_views
            .iter()
            .map(|view| view.extensions.meshopt.as_ref().map(|m| data.decode_meshopt(m)).transpose())
            .collect::<anyhow::Result<_>>()?;
        Ok(data)
    }

    fn decode_meshopt(&self, m: &Meshopt) -> anyhow::Result<Vec<u8>> {
        let src = self
            .buffers
            .get(m.buffer)
            .and_then(|b| b.get(m.byte_offset..m.byte_offset + m.byte_length))
            .context("meshopt data out of range")?;
        // Never more than a buffer could hold: the size is checked before it's allocated
        let size = m.count.checked_mul(m.byte_stride).filter(|&s| s <= 1 << 31).context("meshopt data too big")?;
        let mut out = vec![0u8; size];
        let dst = out.as_mut_ptr().cast();
        // SAFETY: `out` holds count × stride bytes, which is all the decoders write
        let status = unsafe {
            match m.mode.as_str() {
                "ATTRIBUTES" => meshopt_decodeVertexBuffer(dst, m.count, m.byte_stride, src.as_ptr(), src.len()),
                "TRIANGLES" => meshopt_decodeIndexBuffer(dst, m.count, m.byte_stride, src.as_ptr(), src.len()),
                "INDICES" => meshopt_decodeIndexSequence(dst, m.count, m.byte_stride, src.as_ptr(), src.len()),
                mode => bail!("unknown meshopt mode {mode}"),
            }
        };
        ensure!(status == 0, "meshopt data is damaged");
        // SAFETY: as above; the filters keep to count × stride bytes
        unsafe {
            match m.filter.as_deref().unwrap_or("NONE") {
                "NONE" => {}
                "OCTAHEDRAL" => meshopt_decodeFilterOct(dst, m.count, m.byte_stride),
                "QUATERNION" => meshopt_decodeFilterQuat(dst, m.count, m.byte_stride),
                "EXPONENTIAL" => meshopt_decodeFilterExp(dst, m.count, m.byte_stride),
                "COLOR" => meshopt_decodeFilterColor(dst, m.count, m.byte_stride),
                filter => bail!("unknown meshopt filter {filter}"),
            }
        }
        Ok(out)
    }

    /// Counts `elements` floats or indices against the budget of 2 GiB.
    fn spend(&self, elements: usize) -> anyhow::Result<()> {
        let spent = elements.checked_mul(4).and_then(|b| b.checked_add(self.spent.get()));
        ensure!(spent.is_some_and(|s| s <= 2 << 30), "the model is too big");
        self.spent.set(spent.unwrap());
        Ok(())
    }

    fn view(&self, index: usize) -> anyhow::Result<&[u8]> {
        if let Some(Some(decoded)) = self.views.get(index) {
            return Ok(decoded);
        }
        let view = self.root.buffer_views.get(index).context("no such buffer view")?;
        self.buffers
            .get(view.buffer)
            .and_then(|b| b.get(view.byte_offset..view.byte_offset + view.byte_length))
            .context("buffer view out of range")
    }

    /// An accessor's values as floats (normalized integers scaled to 0…1 or −1…1), and how
    /// many make an element.
    fn floats(&self, index: usize) -> anyhow::Result<(Vec<f32>, usize)> {
        let accessor = self.root.accessors.get(index).context("no such accessor")?;
        let width = match accessor.kind.as_str() {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" | "MAT2" => 4,
            "MAT3" => 9,
            "MAT4" => 16,
            kind => bail!("unknown accessor type {kind}"),
        };
        // The data must be there before room is made for it
        let size = component_size(accessor.component_type)?;
        let source = match accessor.buffer_view {
            Some(view) => {
                let stride = self.root.buffer_views.get(view).context("no such buffer view")?.byte_stride.unwrap_or(0);
                let data = self.view(view)?;
                check_fits(data.len(), accessor.byte_offset, stride, accessor.count, size * width)?;
                Some((data, stride))
            }
            None => {
                ensure!(accessor.count <= 1 << 26, "accessor too big");
                None
            }
        };
        self.spend(accessor.count * width)?;
        let mut out = vec![0.0; accessor.count * width];
        if let Some((data, stride)) = source {
            read_floats(
                &mut out,
                data,
                accessor.byte_offset,
                stride,
                width,
                accessor.component_type,
                accessor.normalized,
            )?;
        }
        if let Some(sparse) = &accessor.sparse {
            ensure!(sparse.count <= accessor.count, "more sparse values than elements");
            let indices = read_indices(
                self.view(sparse.indices.buffer_view)?,
                sparse.indices.byte_offset,
                sparse.count,
                sparse.indices.component_type,
            )?;
            self.spend(sparse.count * width)?;
            let mut values = vec![0.0; sparse.count * width];
            read_floats(
                &mut values,
                self.view(sparse.values.buffer_view)?,
                sparse.values.byte_offset,
                0,
                width,
                accessor.component_type,
                accessor.normalized,
            )?;
            for (k, &i) in indices.iter().enumerate() {
                let i = i as usize;
                ensure!(i < accessor.count, "sparse index out of range");
                out[i * width..(i + 1) * width].copy_from_slice(&values[k * width..(k + 1) * width]);
            }
        }
        Ok((out, width))
    }

    fn indices(&self, index: usize) -> anyhow::Result<Vec<u32>> {
        let accessor = self.root.accessors.get(index).context("no such accessor")?;
        match accessor.buffer_view {
            Some(view) => {
                let indices =
                    read_indices(self.view(view)?, accessor.byte_offset, accessor.count, accessor.component_type)?;
                self.spend(indices.len())?;
                Ok(indices)
            }
            None => {
                ensure!(accessor.count <= 1 << 26, "accessor too big");
                self.spend(accessor.count)?;
                Ok(vec![0; accessor.count])
            }
        }
    }

    fn vec<const N: usize>(&self, index: Option<&usize>) -> anyhow::Result<Vec<[f32; N]>> {
        let Some(&index) = index else { return Ok(Vec::new()) };
        let (values, width) = self.floats(index)?;
        ensure!(width >= N, "accessor has {width} components, not {N}");
        Ok(values.chunks_exact(width).map(|c| std::array::from_fn(|i| c[i])).collect())
    }
}

fn component_size(component_type: u32) -> anyhow::Result<usize> {
    Ok(match component_type {
        5120 | 5121 => 1,
        5122 | 5123 => 2,
        5125 | 5126 => 4,
        other => bail!("unknown component type {other}"),
    })
}

/// Whether `count` elements of `element` bytes, `stride` apart from `offset`, fit in `len` bytes.
fn check_fits(len: usize, offset: usize, stride: usize, count: usize, element: usize) -> anyhow::Result<()> {
    if count == 0 {
        return Ok(());
    }
    let stride = if stride == 0 { element } else { stride };
    let end = stride.checked_mul(count - 1).and_then(|s| s.checked_add(offset)).and_then(|s| s.checked_add(element));
    ensure!(end.is_some_and(|end| end <= len), "accessor out of range");
    Ok(())
}

fn read_floats(
    out: &mut [f32],
    data: &[u8],
    offset: usize,
    stride: usize,
    width: usize,
    component_type: u32,
    normalized: bool,
) -> anyhow::Result<()> {
    let size = component_size(component_type)?;
    let element = size * width;
    let stride = if stride == 0 { element } else { stride };
    let count = out.len() / width;
    check_fits(data.len(), offset, stride, count, element)?;

    fn each<const S: usize>(
        out: &mut [f32],
        data: &[u8],
        offset: usize,
        stride: usize,
        width: usize,
        f: impl Fn([u8; S]) -> f32,
    ) {
        for (i, element) in out.chunks_exact_mut(width).enumerate() {
            let base = offset + i * stride;
            for (c, value) in element.iter_mut().enumerate() {
                let at = base + c * S;
                *value = f(data[at..at + S].try_into().unwrap());
            }
        }
    }
    match (component_type, normalized) {
        (5126, _) => each(out, data, offset, stride, width, f32::from_le_bytes),
        (5121, false) => each(out, data, offset, stride, width, |b: [u8; 1]| b[0] as f32),
        (5121, true) => each(out, data, offset, stride, width, |b: [u8; 1]| b[0] as f32 / 255.0),
        (5120, false) => each(out, data, offset, stride, width, |b: [u8; 1]| b[0] as i8 as f32),
        (5120, true) => each(out, data, offset, stride, width, |b: [u8; 1]| (b[0] as i8 as f32 / 127.0).max(-1.0)),
        (5123, false) => each(out, data, offset, stride, width, |b| u16::from_le_bytes(b) as f32),
        (5123, true) => each(out, data, offset, stride, width, |b| u16::from_le_bytes(b) as f32 / 65535.0),
        (5122, false) => each(out, data, offset, stride, width, |b| i16::from_le_bytes(b) as f32),
        (5122, true) => each(out, data, offset, stride, width, |b| (i16::from_le_bytes(b) as f32 / 32767.0).max(-1.0)),
        (5125, _) => each(out, data, offset, stride, width, |b| u32::from_le_bytes(b) as f32),
        _ => unreachable!(),
    }
    Ok(())
}

fn read_indices(data: &[u8], offset: usize, count: usize, component_type: u32) -> anyhow::Result<Vec<u32>> {
    let size = component_size(component_type)?;
    let bytes = data.get(offset..offset + count * size).context("indices out of range")?;
    Ok(match component_type {
        5121 => bytes.iter().map(|&b| b as u32).collect(),
        5123 => bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]]) as u32).collect(),
        5125 => bytes.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect(),
        other => bail!("indices can’t be of type {other}"),
    })
}

// From the glTF document to a Model

struct Loader<'a> {
    root: &'a Root,
    data: &'a Data<'a>,
}

impl Loader<'_> {
    fn model(&self, folder: &Path) -> anyhow::Result<Model> {
        let root = self.root;
        let mut model = Model { root: Mat4::IDENTITY, ..Model::default() };

        // Nodes: the scene's trees, depth first, so a parent always comes before its children
        let roots = match root.scene.or((!root.scenes.is_empty()).then_some(0)) {
            Some(scene) => root.scenes.get(scene).context("no such scene")?.nodes.clone(),
            None => {
                let children: HashSet<usize> = root.nodes.iter().flat_map(|n| n.children.iter().copied()).collect();
                (0..root.nodes.len()).filter(|i| !children.contains(i)).collect()
            }
        };
        let mut node_map: Vec<Option<usize>> = vec![None; root.nodes.len()];
        let mut stack: Vec<(usize, Option<usize>)> = roots.iter().rev().map(|&n| (n, None)).collect();
        while let Some((index, parent)) = stack.pop() {
            let Some(def) = root.nodes.get(index) else { bail!("no such node {index}") };
            if node_map[index].is_some() {
                continue;
            }
            node_map[index] = Some(model.nodes.len());
            let transform = match def.matrix {
                Some(m) => Transform::from_matrix(Mat4::from_cols_array(&m)),
                None => Transform {
                    translation: def.translation.unwrap_or_default().into(),
                    rotation: glam::Quat::from_array(def.rotation.unwrap_or([0.0, 0.0, 0.0, 1.0])).normalize(),
                    scale: def.scale.unwrap_or([1.0; 3]).into(),
                },
            };
            let me = model.nodes.len();
            model.nodes.push(Node { parent, transform, ..Node::default() });
            stack.extend(def.children.iter().rev().map(|&c| (c, Some(me))));
        }

        // Images: only those a material uses, decoded side by side
        let (images, image_map) = self.images(folder)?;
        model.images = images;

        // Materials, and a default one for primitives without
        model.materials = root.materials.iter().map(|m| self.material(m, &image_map)).collect();
        let default_material = model.materials.len();
        model.materials.push(Material::default());

        // Meshes, those the scene's nodes use
        let mut mesh_map: HashMap<usize, usize> = HashMap::new();
        for (index, def) in root.nodes.iter().enumerate() {
            let (Some(node), Some(mesh)) = (node_map[index], def.mesh) else { continue };
            let mine = match mesh_map.get(&mesh) {
                Some(&m) => m,
                None => {
                    let mesh_def = root.meshes.get(mesh).context("no such mesh")?;
                    let mut primitives = Vec::new();
                    for p in &mesh_def.primitives {
                        if let Some(primitive) = self.primitive(p, default_material)? {
                            primitives.push(primitive);
                        }
                    }
                    model.meshes.push(Mesh { primitives });
                    mesh_map.insert(mesh, model.meshes.len() - 1);
                    model.meshes.len() - 1
                }
            };
            let targets = model.meshes[mine].primitives.iter().map(|p| p.targets.len()).max().unwrap_or(0);
            let mut weights = def.weights.clone().or_else(|| root.meshes[mesh].weights.clone()).unwrap_or_default();
            weights.resize(targets, 0.0);
            let node = &mut model.nodes[node];
            node.mesh = Some(mine);
            node.weights = weights;
        }

        // Skins
        let mut skin_map: HashMap<usize, Option<usize>> = HashMap::new();
        for (index, def) in root.nodes.iter().enumerate() {
            let (Some(node), Some(skin)) = (node_map[index], def.skin) else { continue };
            let mine = match skin_map.get(&skin) {
                Some(&s) => s,
                None => {
                    let s = self.skin(skin, &node_map)?.map(|s| {
                        model.skins.push(s);
                        model.skins.len() - 1
                    });
                    skin_map.insert(skin, s);
                    s
                }
            };
            model.nodes[node].skin = mine;
        }

        self.instance(&mut model, &node_map)?;

        // Animations
        for def in &root.animations {
            model.clips.push(self.clip(def, &node_map)?);
        }
        Ok(model)
    }

    /// EXT_mesh_gpu_instancing: a node drawn many times becomes a node for each time.
    fn instance(&self, model: &mut Model, node_map: &[Option<usize>]) -> anyhow::Result<()> {
        for (index, def) in self.root.nodes.iter().enumerate() {
            let (Some(node), Some(instancing)) = (node_map[index], def.extensions.instancing.as_ref()) else {
                continue;
            };
            let Some(mesh) = model.nodes[node].mesh.take() else { continue };
            let read = |name: &str| -> anyhow::Result<Option<Vec<f32>>> {
                instancing.attributes.get(name).map(|&a| self.data.floats(a).map(|(v, _)| v)).transpose()
            };
            let (translations, rotations, scales) = (read("TRANSLATION")?, read("ROTATION")?, read("SCALE")?);
            let count = [
                translations.as_ref().map(|t| t.len() / 3),
                rotations.as_ref().map(|r| r.len() / 4),
                scales.as_ref().map(|s| s.len() / 3),
            ]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(0);
            let (skin, weights) = (model.nodes[node].skin, model.nodes[node].weights.clone());
            for i in 0..count {
                let transform = Transform {
                    translation: translations
                        .as_ref()
                        .map_or(glam::Vec3::ZERO, |t| glam::Vec3::from_slice(&t[i * 3..])),
                    rotation: rotations
                        .as_ref()
                        .map_or(glam::Quat::IDENTITY, |r| glam::Quat::from_slice(&r[i * 4..]).normalize()),
                    scale: scales.as_ref().map_or(glam::Vec3::ONE, |s| glam::Vec3::from_slice(&s[i * 3..])),
                };
                model.nodes.push(Node {
                    parent: Some(node),
                    transform,
                    mesh: Some(mesh),
                    skin,
                    weights: weights.clone(),
                });
            }
        }
        Ok(())
    }

    fn images(&self, folder: &Path) -> anyhow::Result<(Vec<Image>, HashMap<usize, usize>)> {
        let root = self.root;
        let mut used: Vec<usize> = Vec::new();
        let mut seen = HashSet::new();
        for m in &root.materials {
            for (_, info) in texture_infos(m) {
                if let Some(image) = self.texture_source(info.index) {
                    if seen.insert(image) {
                        used.push(image);
                    }
                }
            }
        }
        let mut sources = Vec::with_capacity(used.len());
        for &image in &used {
            let def = root.images.get(image).context("no such image")?;
            let bytes: Cow<[u8]> = match (&def.uri, def.buffer_view) {
                (Some(uri), _) => Cow::Owned(files::read(folder, uri)?),
                (None, Some(view)) => Cow::Borrowed(self.data.view(view)?),
                (None, None) => bail!("image {image} has no data"),
            };
            sources.push(bytes);
        }
        let decoded = decode_all(&sources);
        let mut images = Vec::new();
        let mut map = HashMap::new();
        for (index, result) in used.into_iter().zip(decoded) {
            match result {
                Ok(image) => {
                    map.insert(index, images.len());
                    images.push(image);
                }
                Err(e) => crate::warn!("Image {index} couldn’t be decoded: {e}"),
            }
        }
        Ok((images, map))
    }

    fn texture_source(&self, texture: usize) -> Option<usize> {
        let def = self.root.textures.get(texture)?;
        def.extensions.webp.as_ref().map(|w| w.source).or(def.source)
    }

    fn texture(&self, info: Option<&TextureInfo>, images: &HashMap<usize, usize>) -> Option<TextureRef> {
        let info = info?;
        let image = *images.get(&self.texture_source(info.index)?)?;
        let def = &self.root.textures[info.index];
        let sampler = def.sampler.and_then(|s| self.root.samplers.get(s)).map_or_else(Sampler::default, |s| {
            let defaults = Sampler::default();
            Sampler {
                mag_filter: s.mag_filter.unwrap_or(defaults.mag_filter),
                min_filter: s.min_filter.unwrap_or(defaults.min_filter),
                wrap_s: s.wrap_s.unwrap_or(defaults.wrap_s),
                wrap_t: s.wrap_t.unwrap_or(defaults.wrap_t),
            }
        });
        let mut texture = TextureRef { image, uv: info.tex_coord, transform: TextureRef::IDENTITY, sampler };
        if let Some(t) = &info.extensions.transform {
            // KHR_texture_transform: translation × rotation × scale
            let [ox, oy] = t.offset.unwrap_or([0.0, 0.0]);
            let [sx, sy] = t.scale.unwrap_or([1.0, 1.0]);
            let (s, c) = t.rotation.unwrap_or(0.0).sin_cos();
            texture.transform = [[c * sx, s * sy, ox], [-s * sx, c * sy, oy]];
            texture.uv = t.tex_coord.unwrap_or(info.tex_coord);
        }
        Some(texture)
    }

    fn material(&self, def: &MaterialDef, images: &HashMap<usize, usize>) -> Material {
        let defaults = Pbr::default();
        let pbr = def.pbr_metallic_roughness.as_ref().unwrap_or(&defaults);
        let extensions = &def.extensions;
        let strength = extensions.emissive_strength.as_ref().map_or(1.0, |e| e.emissive_strength);
        let [er, eg, eb] = def.emissive_factor.unwrap_or([0.0; 3]);
        let mut material = Material {
            base_color: pbr.base_color_factor.unwrap_or([1.0; 4]),
            metallic: pbr.metallic_factor.unwrap_or(1.0),
            roughness: pbr.roughness_factor.unwrap_or(1.0),
            normal_scale: def.normal_texture.as_ref().and_then(|t| t.scale).unwrap_or(1.0),
            occlusion_strength: def.occlusion_texture.as_ref().and_then(|t| t.strength).unwrap_or(1.0),
            emissive: [er * strength, eg * strength, eb * strength],
            alpha: match def.alpha_mode.as_deref() {
                Some("MASK") => AlphaMode::Mask(def.alpha_cutoff.unwrap_or(0.5)),
                Some("BLEND") => AlphaMode::Blend,
                _ => AlphaMode::Opaque,
            },
            double_sided: def.double_sided,
            unlit: extensions.unlit.is_some(),
            ..Material::default()
        };
        if let Some(sheen) = &extensions.sheen {
            let [r, g, b] = sheen.sheen_color_factor.unwrap_or([0.0; 3]);
            material.sheen = [r, g, b, sheen.sheen_roughness_factor.unwrap_or(0.0)];
        }
        if let Some(clearcoat) = &extensions.clearcoat {
            material.clearcoat = [
                clearcoat.clearcoat_factor.unwrap_or(0.0),
                clearcoat.clearcoat_roughness_factor.unwrap_or(0.0),
                clearcoat.clearcoat_normal_texture.as_ref().and_then(|t| t.scale).unwrap_or(1.0),
            ];
        }
        if let Some(specular) = &extensions.specular {
            let [r, g, b] = specular.specular_color_factor.unwrap_or([1.0; 3]);
            material.specular = [r, g, b, specular.specular_factor.unwrap_or(1.0)];
        }
        if let Some(ior) = extensions.ior.as_ref().and_then(|i| i.ior) {
            material.ior = ior;
        }
        if let Some(transmission) = &extensions.transmission {
            material.transmission = transmission.transmission_factor.unwrap_or(0.0);
        }
        for (map, info) in texture_infos(def) {
            material.set_map(map, self.texture(Some(info), images));
        }
        material
    }

    fn draco(&self, def: &DracoDef) -> anyhow::Result<Draco> {
        let bytes = self.data.view(def.buffer_view)?;
        // The decoder panics on data it can't read: that's a damaged file, not a crash
        let decoded = std::panic::catch_unwind(|| draco_decoder::decode_mesh_with_config_sync(bytes))
            .ok()
            .flatten()
            .context("the Draco data is damaged")?;
        self.data.spend(decoded.data.len() / 4)?;
        Ok(Draco { decoded, ids: def.attributes.clone() })
    }

    fn primitive(&self, def: &PrimitiveDef, default_material: usize) -> anyhow::Result<Option<Primitive>> {
        let data = self.data;
        let draco = match &def.extensions.draco {
            Some(d) if def.mode == 0 => {
                bail!("Draco point clouds aren’t supported ({} attributes)", d.attributes.len())
            }
            Some(d) => Some(self.draco(d)?),
            None => None,
        };
        // An attribute from the Draco data if it has it, else from its accessor
        let floats = |name: &str| -> anyhow::Result<Option<(Vec<f32>, usize)>> {
            let Some(&accessor) = def.attributes.get(name) else { return Ok(None) };
            let normalized = self.root.accessors.get(accessor).is_some_and(|a| a.normalized);
            if let Some(decoded) = draco.as_ref().and_then(|d| d.floats(name, normalized)) {
                return decoded.map(Some);
            }
            data.floats(accessor).map(Some)
        };
        fn vec<const N: usize>(found: Option<(Vec<f32>, usize)>) -> anyhow::Result<Vec<[f32; N]>> {
            let Some((values, width)) = found else { return Ok(Vec::new()) };
            ensure!(width >= N, "accessor has {width} components, not {N}");
            Ok(values.chunks_exact(width).map(|c| std::array::from_fn(|i| c[i])).collect())
        }
        let Some((values, width)) = floats("POSITION")? else { return Ok(None) };
        ensure!(width == 3, "positions must have three components");
        let positions: Vec<[f32; 3]> = values.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        let count = positions.len();

        let colors = match floats("COLOR_0")? {
            Some((values, width)) => {
                ensure!(width == 3 || width == 4, "colours must have three or four components");
                values.chunks_exact(width).map(|c| [c[0], c[1], c[2], if width == 4 { c[3] } else { 1.0 }]).collect()
            }
            None => Vec::new(),
        };
        let joints: Vec<[u16; 4]> = vec::<4>(floats("JOINTS_0")?)?.into_iter().map(|j| j.map(|v| v as u16)).collect();
        let mut primitive = Primitive {
            normals: vec(floats("NORMAL")?)?,
            uv0: vec(floats("TEXCOORD_0")?)?,
            uv1: vec(floats("TEXCOORD_1")?)?,
            colors,
            weights: if joints.is_empty() { Vec::new() } else { vec(floats("WEIGHTS_0")?)? },
            joints,
            material: def.material.filter(|&m| m < self.root.materials.len()).unwrap_or(default_material),
            positions,
            ..Primitive::default()
        };
        for attribute in [primitive.normals.len(), primitive.uv0.len(), primitive.uv1.len(), primitive.colors.len()] {
            ensure!(attribute == 0 || attribute == count, "attributes of different lengths");
        }
        if primitive.joints.len() != count || primitive.weights.len() != count {
            primitive.joints.clear();
            primitive.weights.clear();
        }

        let mut indices = match (&draco, def.indices) {
            (Some(draco), _) => draco.indices(),
            (None, Some(index)) => data.indices(index)?,
            (None, None) => Vec::new(),
        };
        ensure!(indices.iter().all(|&i| (i as usize) < count), "index out of range");
        let sequence = || (0..count as u32).collect::<Vec<_>>();
        primitive.topology = match def.mode {
            0 => Topology::Points,
            1 => Topology::Lines,
            2 | 3 => {
                // Line loop or strip: as separate lines
                let list = if indices.is_empty() { sequence() } else { indices };
                let mut lines: Vec<u32> = list.windows(2).flatten().copied().collect();
                if def.mode == 2 && list.len() > 2 {
                    lines.extend([list[list.len() - 1], list[0]]);
                }
                indices = lines;
                Topology::Lines
            }
            4 => Topology::Triangles,
            5 | 6 => {
                let list = if indices.is_empty() { sequence() } else { indices };
                indices = triangulate(def.mode, &list);
                Topology::Triangles
            }
            mode => bail!("unknown primitive mode {mode}"),
        };
        primitive.indices = indices;
        primitive.flat = primitive.topology == Topology::Triangles && primitive.normals.is_empty();

        for target in &def.targets {
            let positions = match target.get("POSITION") {
                Some(&i) => data.vec(Some(&i))?,
                None => vec![[0.0; 3]; count],
            };
            let normals = data.vec(target.get("NORMAL"))?;
            ensure!(
                positions.len() == count && (normals.is_empty() || normals.len() == count),
                "morph target of a different length"
            );
            primitive.targets.push(MorphTarget { positions, normals });
        }
        Ok(Some(primitive))
    }

    fn skin(&self, index: usize, node_map: &[Option<usize>]) -> anyhow::Result<Option<Skin>> {
        let def = self.root.skins.get(index).context("no such skin")?;
        let Some(joints) = def.joints.iter().map(|&j| node_map.get(j).copied().flatten()).collect::<Option<Vec<_>>>()
        else {
            crate::warn!("Skin {index} has joints outside the scene");
            return Ok(None);
        };
        let inverse_binds = match def.inverse_bind_matrices {
            Some(accessor) => {
                let (values, width) = self.data.floats(accessor)?;
                ensure!(width == 16, "inverse bind matrices must be 4×4");
                values.chunks_exact(16).map(Mat4::from_cols_slice).collect()
            }
            None => vec![Mat4::IDENTITY; joints.len()],
        };
        ensure!(inverse_binds.len() >= joints.len(), "too few inverse bind matrices");
        Ok(Some(Skin { joints, inverse_binds }))
    }

    fn clip(&self, def: &AnimationDef, node_map: &[Option<usize>]) -> anyhow::Result<Clip> {
        let mut clip = Clip { name: def.name.clone(), duration: 0.0, channels: Vec::new() };
        for channel in &def.channels {
            let Some(node) = channel.target.node.and_then(|n| node_map.get(n).copied().flatten()) else { continue };
            let property = match channel.target.path.as_str() {
                "translation" => Property::Translation,
                "rotation" => Property::Rotation,
                "scale" => Property::Scale,
                "weights" => Property::Weights,
                _ => continue,
            };
            let sampler = def.samplers.get(channel.sampler).context("no such animation sampler")?;
            let interpolation = match sampler.interpolation.as_deref() {
                Some("STEP") => Interpolation::Step,
                Some("CUBICSPLINE") => Interpolation::CubicSpline,
                _ => Interpolation::Linear,
            };
            let (times, _) = self.data.floats(sampler.input)?;
            let (values, _) = self.data.floats(sampler.output)?;
            if times.is_empty() {
                continue;
            }
            clip.duration = clip.duration.max(*times.last().unwrap());
            clip.channels.push(Channel { node, property, interpolation, times, values });
        }
        Ok(clip)
    }
}

/// Every texture a material has, and the slot it goes in.
fn texture_infos(m: &MaterialDef) -> Vec<(Map, &TextureInfo)> {
    let pbr = m.pbr_metallic_roughness.as_ref();
    let e = &m.extensions;
    let sheen = e.sheen.as_ref();
    let clearcoat = e.clearcoat.as_ref();
    let specular = e.specular.as_ref();
    [
        (Map::BaseColor, pbr.and_then(|p| p.base_color_texture.as_ref())),
        (Map::MetallicRoughness, pbr.and_then(|p| p.metallic_roughness_texture.as_ref())),
        (Map::Normal, m.normal_texture.as_ref()),
        (Map::Occlusion, m.occlusion_texture.as_ref()),
        (Map::Emissive, m.emissive_texture.as_ref()),
        (Map::SheenColor, sheen.and_then(|s| s.sheen_color_texture.as_ref())),
        (Map::SheenRoughness, sheen.and_then(|s| s.sheen_roughness_texture.as_ref())),
        (Map::Clearcoat, clearcoat.and_then(|c| c.clearcoat_texture.as_ref())),
        (Map::ClearcoatRoughness, clearcoat.and_then(|c| c.clearcoat_roughness_texture.as_ref())),
        (Map::ClearcoatNormal, clearcoat.and_then(|c| c.clearcoat_normal_texture.as_ref())),
        (Map::Specular, specular.and_then(|s| s.specular_texture.as_ref())),
        (Map::SpecularColor, specular.and_then(|s| s.specular_color_texture.as_ref())),
        (Map::Transmission, e.transmission.as_ref().and_then(|t| t.transmission_texture.as_ref())),
    ]
    .into_iter()
    .filter_map(|(map, info)| Some((map, info?)))
    .collect()
}

/// Decodes images on as many threads as there are cores, each result in its source's place.
pub fn decode_all(sources: &[Cow<[u8]>]) -> Vec<anyhow::Result<Image>> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(sources.len());
    if threads <= 1 {
        return sources.iter().map(|s| Image::decode(s)).collect();
    }
    let next = AtomicUsize::new(0);
    let mut results: Vec<(usize, anyhow::Result<Image>)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(source) = sources.get(i) else { break done };
                        done.push((i, Image::decode(source)));
                    }
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap()).collect()
    });
    results.sort_by_key(|(i, _)| *i);
    results.into_iter().map(|(_, r)| r).collect()
}
