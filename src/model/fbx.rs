// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! FBX (binary or ASCII) and Wavefront OBJ (with its .mtl), both read by ufbx. FBX scenes are
//! turned Y-up; their skins and animations come along, the animations baked to keyframes.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

use glam::{Mat4, Quat, Vec3};

use super::{
    AlphaMode, Channel, Clip, Error, Interpolation, Map, Material, Mesh, Model, Node, Primitive, Property, Sampler,
    Skin, TextureRef, Transform, files, gltf::decode_all, sampler, srgb_to_linear,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Fbx,
    Obj,
}

pub fn load(bytes: &[u8], path: &Path, kind: Kind) -> Result<Model, Error> {
    let folder = path.parent().unwrap_or(Path::new("."));
    let filename = path.to_string_lossy();
    let opts = ufbx::LoadOpts {
        // An OBJ's .mtl, found next to it
        load_external_files: true,
        ignore_missing_external_files: true,
        generate_missing_normals: true,
        geometry_transform_handling: ufbx::GeometryTransformHandling::HelperNodes,
        inherit_mode_handling: ufbx::InheritModeHandling::HelperNodes,
        target_axes: match kind {
            Kind::Fbx => ufbx::CoordinateAxes::right_handed_y_up(),
            Kind::Obj => ufbx::CoordinateAxes::default(),
        },
        file_format: match kind {
            Kind::Fbx => ufbx::FileFormat::Fbx,
            Kind::Obj => ufbx::FileFormat::Obj,
        },
        filename: filename.as_ref().into(),
        ..Default::default()
    };
    let scene = ufbx::load_memory(bytes, opts).map_err(|e| Error::Unsupported(format!("{}", e.description)))?;
    Converter {
        scene: &scene,
        folder,
        model: Model::default(),
        nodes: vec![usize::MAX; scene.nodes.len()],
        meshes: HashMap::new(),
        materials: HashMap::new(),
        skins: HashMap::new(),
        neutral: None,
        sources: Vec::new(),
        source_keys: HashMap::new(),
    }
    .convert()
}

struct Converter<'a> {
    scene: &'a ufbx::Scene,
    folder: &'a Path,
    model: Model,
    /// ufbx's node typed_id → the model's node
    nodes: Vec<usize>,
    meshes: HashMap<u32, usize>,
    materials: HashMap<u32, usize>,
    skins: HashMap<u32, usize>,
    neutral: Option<usize>,
    /// The images textures refer to: their bytes, by where they came from
    sources: Vec<Cow<'a, [u8]>>,
    source_keys: HashMap<String, usize>,
}

impl<'a> Converter<'a> {
    fn convert(mut self) -> Result<Model, Error> {
        self.model.root = Mat4::IDENTITY;

        // Nodes, parents first
        let mut order: Vec<&ufbx::Node> = self.scene.nodes.iter().map(|n| &**n).collect();
        order.sort_by_key(|n| n.node_depth);
        for node in &order {
            let parent = node.parent.as_ref().map(|p| self.nodes[p.element.typed_id as usize]);
            self.nodes[node.element.typed_id as usize] = self.model.nodes.len();
            self.model.nodes.push(Node {
                parent: parent.filter(|&p| p != usize::MAX),
                transform: transform(&node.local_transform),
                ..Node::default()
            });
        }

        // Meshes and their skins, on the nodes that show them
        for node in &order {
            let Some(mesh) = node.mesh.as_ref() else { continue };
            let index = self.nodes[node.element.typed_id as usize];
            let mesh_index = match self.meshes.get(&mesh.element.typed_id) {
                Some(&m) => m,
                None => {
                    let converted = self.mesh(mesh)?;
                    self.model.meshes.push(converted);
                    let m = self.model.meshes.len() - 1;
                    self.meshes.insert(mesh.element.typed_id, m);
                    m
                }
            };
            self.model.nodes[index].mesh = Some(mesh_index);
            if let Some(skin) = mesh.skin_deformers.iter().next() {
                self.model.nodes[index].skin = Some(self.skin(skin));
            }
        }

        // Images, decoded side by side; a texture whose image can't be decoded is dropped
        let decoded = decode_all(&self.sources);
        let mut remap = vec![None; decoded.len()];
        for (i, result) in decoded.into_iter().enumerate() {
            match result {
                Ok(image) => {
                    remap[i] = Some(self.model.images.len());
                    self.model.images.push(image);
                }
                Err(e) => crate::warn!("A texture couldn’t be decoded: {e}"),
            }
        }
        for material in &mut self.model.materials {
            for map in &mut material.maps {
                *map = map.take().and_then(|mut t| {
                    t.image = remap[t.image]?;
                    Some(t)
                });
            }
        }

        // Animations, one clip a stack
        for stack in &self.scene.anim_stacks {
            if let Some(clip) = self.clip(stack) {
                self.model.clips.push(clip);
            }
        }
        Ok(self.model)
    }

    fn mesh(&mut self, mesh: &'a ufbx::Mesh) -> Result<Mesh, Error> {
        let skin = mesh.skin_deformers.iter().next();
        let mut triangle = vec![0u32; mesh.max_face_triangles * 3];
        let mut primitives = Vec::new();
        for part in &mesh.material_parts {
            if part.num_triangles == 0 {
                continue;
            }
            let corners = part.num_triangles * 3;
            let mut positions = Vec::with_capacity(corners);
            let mut normals = Vec::with_capacity(corners);
            let mut uvs = Vec::new();
            let mut colors = Vec::new();
            let mut joints = Vec::new();
            let mut weights = Vec::new();
            for &face in &part.face_indices {
                let face = mesh.faces[face as usize];
                let count = ufbx::triangulate_face(&mut triangle, mesh, face) as usize;
                for &corner in &triangle[..count * 3] {
                    let c = corner as usize;
                    positions.push(vec3(mesh.vertex_position[c]));
                    if mesh.vertex_normal.exists {
                        normals.push(vec3(mesh.vertex_normal[c]));
                    }
                    if mesh.vertex_uv.exists {
                        // Their textures start at the bottom row; glTF's, and the renderer's, at the top
                        let uv = mesh.vertex_uv[c];
                        uvs.push([uv.x as f32, 1.0 - uv.y as f32]);
                    }
                    if mesh.vertex_color.exists {
                        let c = mesh.vertex_color[c];
                        colors.push([
                            srgb_to_linear(c.x as f32),
                            srgb_to_linear(c.y as f32),
                            srgb_to_linear(c.z as f32),
                            c.w as f32,
                        ]);
                    }
                    if let Some(skin) = skin {
                        let (j, w) = skin_weights(skin, mesh.vertex_indices[c] as usize);
                        joints.push(j);
                        weights.push(w);
                    }
                }
            }

            // Corners with everything the same become one vertex
            let mut indices = vec![0u32; positions.len()];
            let unique = {
                let mut streams = vec![ufbx::VertexStream::new(&mut positions)];
                if !normals.is_empty() {
                    streams.push(ufbx::VertexStream::new(&mut normals));
                }
                if !uvs.is_empty() {
                    streams.push(ufbx::VertexStream::new(&mut uvs));
                }
                if !colors.is_empty() {
                    streams.push(ufbx::VertexStream::new(&mut colors));
                }
                if !joints.is_empty() {
                    streams.push(ufbx::VertexStream::new(&mut joints));
                    streams.push(ufbx::VertexStream::new(&mut weights));
                }
                ufbx::generate_indices(&mut streams, &mut indices, ufbx::AllocatorOpts::default())
                    .map_err(|e| Error::Unsupported(format!("{}", e.description)))?
            };
            for attribute in [&mut positions, &mut normals] {
                attribute.truncate(unique);
            }
            uvs.truncate(unique);
            colors.truncate(unique);
            joints.truncate(unique);
            weights.truncate(unique);

            let material = match mesh.materials.get(part.index as usize) {
                Some(material) => self.material(material),
                None => self.neutral(),
            };
            primitives.push(Primitive {
                flat: normals.is_empty(),
                positions,
                normals,
                uv0: uvs,
                colors,
                joints,
                weights,
                indices,
                material,
                ..Primitive::default()
            });
        }
        Ok(Mesh { primitives })
    }

    fn neutral(&mut self) -> usize {
        *self.neutral.get_or_insert_with(|| {
            self.model.materials.push(Material::neutral());
            self.model.materials.len() - 1
        })
    }

    fn material(&mut self, material: &'a ufbx::Material) -> usize {
        if let Some(&index) = self.materials.get(&material.element.typed_id) {
            return index;
        }
        let pbr = &material.pbr;
        // An OBJ whose .mtl is missing names materials it doesn't describe
        let described =
            pbr.base_color.has_value || pbr.base_color.texture.is_some() || material.fbx.diffuse_color.has_value;
        let converted = if described {
            let value =
                |map: &ufbx::MaterialMap, default: f64| if map.has_value { map.value_vec4.x } else { default } as f32;
            let color = pbr.base_color.value_vec4;
            let factor = value(&pbr.base_factor, 1.0);
            let opacity = value(&pbr.opacity, 1.0).clamp(0.0, 1.0);
            let emission = pbr.emission_color.value_vec4;
            let emission_factor = value(&pbr.emission_factor, 1.0);
            let mut converted = Material {
                base_color: [
                    srgb_to_linear(color.x as f32) * factor,
                    srgb_to_linear(color.y as f32) * factor,
                    srgb_to_linear(color.z as f32) * factor,
                    opacity,
                ],
                metallic: value(&pbr.metalness, 0.0),
                roughness: value(&pbr.roughness, 0.5),
                emissive: [emission.x, emission.y, emission.z].map(|c| srgb_to_linear(c as f32) * emission_factor),
                alpha: if opacity < 1.0 { AlphaMode::Blend } else { AlphaMode::Opaque },
                ..Material::default()
            };
            converted.set_map(Map::BaseColor, self.texture(pbr.base_color.texture.as_deref()));
            converted.set_map(Map::Normal, self.texture(pbr.normal_map.texture.as_deref()));
            converted.set_map(Map::Emissive, self.texture(pbr.emission_color.texture.as_deref()));
            converted
        } else {
            Material::neutral()
        };
        self.model.materials.push(converted);
        let index = self.model.materials.len() - 1;
        self.materials.insert(material.element.typed_id, index);
        index
    }

    fn texture(&mut self, texture: Option<&'a ufbx::Texture>) -> Option<TextureRef> {
        let mut texture = texture?;
        if texture.type_ != ufbx::TextureType::File {
            texture = texture.file_textures.iter().next().map(|t| &**t)?;
        }
        let source = self.texture_source(texture)?;
        let wrap = |mode: ufbx::WrapMode| match mode {
            ufbx::WrapMode::Clamp => sampler::CLAMP_TO_EDGE,
            _ => sampler::REPEAT,
        };
        let mut reference = TextureRef::new(source);
        reference.sampler =
            Sampler { wrap_s: wrap(texture.wrap_u), wrap_t: wrap(texture.wrap_v), ..Sampler::default() };
        if texture.has_uv_transform {
            // uv_to_texture, for coordinates whose v the loader flipped
            let m = &texture.uv_to_texture;
            reference.transform = [
                [m.m00 as f32, -m.m01 as f32, (m.m01 + m.m03) as f32],
                [-m.m10 as f32, m.m11 as f32, (1.0 - m.m11 - m.m13) as f32],
            ];
        }
        Some(reference)
    }

    /// The index of a texture's image among the sources: its embedded bytes, or the file it
    /// names, looked for in the model's folder.
    fn texture_source(&mut self, texture: &'a ufbx::Texture) -> Option<usize> {
        let key = if texture.content.is_empty() {
            texture.relative_filename.replace('\\', "/")
        } else {
            format!("embedded:{}", texture.element.element_id)
        };
        if let Some(&index) = self.source_keys.get(&key) {
            return Some(index);
        }
        let bytes: Cow<'a, [u8]> = if texture.content.is_empty() {
            let basename = key.rsplit('/').next().unwrap_or_default().to_owned();
            let read = files::read(self.folder, &key).or_else(|_| files::read(self.folder, &basename));
            match read {
                Ok(bytes) => Cow::Owned(bytes),
                Err(e) => {
                    crate::warn!("No texture {key}: {e}");
                    return None;
                }
            }
        } else {
            Cow::Borrowed(&texture.content)
        };
        self.sources.push(bytes);
        self.source_keys.insert(key, self.sources.len() - 1);
        Some(self.sources.len() - 1)
    }

    fn skin(&mut self, skin: &ufbx::SkinDeformer) -> usize {
        if let Some(&index) = self.skins.get(&skin.element.typed_id) {
            return index;
        }
        let (joints, inverse_binds) = skin
            .clusters
            .iter()
            .map(|cluster| {
                let joint = cluster.bone_node.as_ref().map_or(0, |n| self.nodes[n.element.typed_id as usize]);
                (joint, matrix(&cluster.geometry_to_bone))
            })
            .unzip();
        self.model.skins.push(Skin { joints, inverse_binds });
        let index = self.model.skins.len() - 1;
        self.skins.insert(skin.element.typed_id, index);
        index
    }

    fn clip(&self, stack: &ufbx::AnimStack) -> Option<Clip> {
        let baked = match ufbx::bake_anim(self.scene, &stack.anim, ufbx::BakeOpts::default()) {
            Ok(baked) => baked,
            Err(e) => {
                crate::warn!("Animation {} couldn’t be baked: {}", stack.element.name, e.description);
                return None;
            }
        };
        let begin = baked.playback_time_begin;
        let mut channels = Vec::new();
        for node in &baked.nodes {
            let Some(&index) = self.nodes.get(node.typed_id as usize) else { continue };
            let times = |keys: &mut dyn Iterator<Item = f64>| keys.map(|t| (t - begin) as f32).collect::<Vec<_>>();
            let mut push = |property, times: Vec<f32>, values: Vec<f32>| {
                if !times.is_empty() {
                    channels.push(Channel {
                        node: index,
                        property,
                        interpolation: Interpolation::Linear,
                        times,
                        values,
                    });
                }
            };
            push(
                Property::Translation,
                times(&mut node.translation_keys.iter().map(|k| k.time)),
                node.translation_keys.iter().flat_map(|k| vec3(k.value)).collect(),
            );
            push(
                Property::Rotation,
                times(&mut node.rotation_keys.iter().map(|k| k.time)),
                node.rotation_keys
                    .iter()
                    .flat_map(|k| [k.value.x, k.value.y, k.value.z, k.value.w].map(|v| v as f32))
                    .collect(),
            );
            push(
                Property::Scale,
                times(&mut node.scale_keys.iter().map(|k| k.time)),
                node.scale_keys.iter().flat_map(|k| vec3(k.value)).collect(),
            );
        }
        if channels.is_empty() {
            return None;
        }
        Some(Clip { name: stack.element.name.to_string(), duration: baked.playback_duration.max(0.0) as f32, channels })
    }
}

/// The four strongest bones moving a vertex, their weights adding up to one.
fn skin_weights(skin: &ufbx::SkinDeformer, vertex: usize) -> ([u16; 4], [f32; 4]) {
    let Some(v) = skin.vertices.get(vertex) else { return ([0; 4], [0.0; 4]) };
    let begin = v.weight_begin as usize;
    let weights: &[ufbx::SkinWeight] = &skin.weights;
    let mut all: Vec<_> =
        weights[begin..begin + v.num_weights as usize].iter().map(|w| (w.cluster_index, w.weight)).collect();
    all.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut joints = [0u16; 4];
    let mut weights = [0f32; 4];
    for (k, &(cluster, weight)) in all.iter().take(4).enumerate() {
        joints[k] = cluster as u16;
        weights[k] = weight as f32;
    }
    let total: f32 = weights.iter().sum();
    if total > 0.0 {
        weights = weights.map(|w| w / total);
    }
    (joints, weights)
}

fn vec3(v: ufbx::Vec3) -> [f32; 3] {
    [v.x as f32, v.y as f32, v.z as f32]
}

fn transform(t: &ufbx::Transform) -> Transform {
    Transform {
        translation: Vec3::from(vec3(t.translation)),
        rotation: Quat::from_xyzw(t.rotation.x as f32, t.rotation.y as f32, t.rotation.z as f32, t.rotation.w as f32)
            .normalize(),
        scale: Vec3::from(vec3(t.scale)),
    }
}

fn matrix(m: &ufbx::Matrix) -> Mat4 {
    Mat4::from_cols_array(
        &[
            m.m00, m.m10, m.m20, 0.0, //
            m.m01, m.m11, m.m21, 0.0, //
            m.m02, m.m12, m.m22, 0.0, //
            m.m03, m.m13, m.m23, 1.0,
        ]
        .map(|v| v as f32),
    )
}
