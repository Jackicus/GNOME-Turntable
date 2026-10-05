// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! 3MF: a zip holding an XML model of meshes, components and build items. Base materials
//! colour whole triangles, colour groups their corners. 3MF is Z-up.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::io::{Cursor, Read};

use anyhow::{Context, bail};
use glam::Mat4;

use super::{AlphaMode, Material, Mesh, Model, Node, Primitive, smooth_normals, srgb_to_linear};

const CORE: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
const MATERIALS: &str = "http://schemas.microsoft.com/3dmanufacturing/material/2015/02";

pub fn load(bytes: &[u8]) -> anyhow::Result<Model> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context("not a zip file")?;
    let path = root_model_path(&mut zip).unwrap_or_else(|| "3D/3dmodel.model".to_owned());
    // Unzipped, a model may not be more than a gigabyte of XML
    const LIMIT: u64 = 1 << 30;
    let mut xml = String::new();
    zip.by_name(path.trim_start_matches('/'))
        .context("no model in the package")?
        .take(LIMIT + 1)
        .read_to_string(&mut xml)?;
    if xml.len() as u64 > LIMIT {
        bail!("the model is too big");
    }
    let document = roxmltree::Document::parse(&xml)?;
    Reader::default().read(&document)
}

/// The model the package's relationships point to.
fn root_model_path(zip: &mut zip::ZipArchive<Cursor<&[u8]>>) -> Option<String> {
    let mut rels = String::new();
    zip.by_name("_rels/.rels").ok()?.take(1 << 20).read_to_string(&mut rels).ok()?;
    let document = roxmltree::Document::parse(&rels).ok()?;
    document
        .descendants()
        .find(|n| n.tag_name().name() == "Relationship" && n.attribute("Type").is_some_and(|t| t.ends_with("/3dmodel")))
        .and_then(|n| n.attribute("Target"))
        .map(str::to_owned)
}

/// An object: the model's mesh for it, and the objects it's made of, placed.
#[derive(Clone)]
struct Object {
    mesh: Option<usize>,
    components: Vec<(String, Mat4)>,
}

/// A colour group's or base material group's colours, by index.
type Colors = Vec<[f32; 4]>;

#[derive(Default)]
struct Reader {
    base_materials: HashMap<String, Colors>,
    color_groups: HashMap<String, Colors>,
    /// By id
    objects: HashMap<String, Object>,
    model: Model,
    /// Base material (group id, index) → the model's material
    materials: HashMap<(String, usize), usize>,
}

impl Reader {
    fn read(mut self, document: &roxmltree::Document) -> anyhow::Result<Model> {
        let root = document.root_element();
        let resources = root.children().find(|n| is_core(n, "resources")).context("no resources")?;
        for node in resources.children().filter(|n| n.is_element()) {
            match (node.tag_name().namespace(), node.tag_name().name()) {
                (Some(CORE), "basematerials") => {
                    let colors = node
                        .children()
                        .filter(|n| is_core(n, "base"))
                        .map(|n| parse_color(n.attribute("displaycolor").unwrap_or("#FFFFFF")))
                        .collect();
                    self.base_materials.insert(id(&node)?, colors);
                }
                (Some(MATERIALS), "colorgroup") => {
                    let colors = node
                        .children()
                        .filter(|n| n.is_element() && n.tag_name().name() == "color")
                        .map(|n| parse_color(n.attribute("color").unwrap_or("#FFFFFF")))
                        .collect();
                    self.color_groups.insert(id(&node)?, colors);
                }
                (Some(CORE), "object") => self.object(&node)?,
                _ => {}
            }
        }

        let build = root.children().find(|n| is_core(n, "build")).context("no build")?;
        for item in build.children().filter(|n| is_core(n, "item")) {
            let object = item.attribute("objectid").context("item without an object")?;
            let transform = item.attribute("transform").map(parse_transform).transpose()?.unwrap_or(Mat4::IDENTITY);
            self.place(object, transform, None, 0)?;
        }
        self.model.root = Mat4::from_rotation_x(-FRAC_PI_2);
        Ok(self.model)
    }

    /// A node for `object` under `parent`, and nodes for its components under it.
    fn place(&mut self, object: &str, transform: Mat4, parent: Option<usize>, depth: usize) -> anyhow::Result<()> {
        if depth > 32 {
            bail!("components nested too deep");
        }
        // Components can repeat each other, doubling at every level
        if self.model.nodes.len() >= 1 << 20 {
            bail!("too many parts");
        }
        let Object { mesh, components } = self.objects.get(object).cloned().context("no such object")?;
        let index = self.model.nodes.len();
        self.model.nodes.push(Node {
            parent,
            transform: super::Transform::from_matrix(transform),
            mesh,
            ..Node::default()
        });
        for (child, transform) in components {
            self.place(&child, transform, Some(index), depth + 1)?;
        }
        Ok(())
    }

    fn object(&mut self, node: &roxmltree::Node) -> anyhow::Result<()> {
        let object_id = id(node)?;
        let mut components = Vec::new();
        if let Some(list) = node.children().find(|n| is_core(n, "components")) {
            for component in list.children().filter(|n| is_core(n, "component")) {
                let child = component.attribute("objectid").context("component without an object")?;
                let transform =
                    component.attribute("transform").map(parse_transform).transpose()?.unwrap_or(Mat4::IDENTITY);
                components.push((child.to_owned(), transform));
            }
        }
        let mesh = match node.children().find(|n| is_core(n, "mesh")) {
            Some(mesh) => Some(self.mesh(&mesh, node.attribute("pid"), node.attribute("pindex"))?),
            None => None,
        };
        self.objects.insert(object_id, Object { mesh, components });
        Ok(())
    }

    fn mesh(
        &mut self,
        mesh: &roxmltree::Node,
        object_pid: Option<&str>,
        object_pindex: Option<&str>,
    ) -> anyhow::Result<usize> {
        let mut positions = Vec::new();
        if let Some(vertices) = mesh.children().find(|n| is_core(n, "vertices")) {
            for v in vertices.children().filter(|n| is_core(n, "vertex")) {
                let coordinate = |name| -> anyhow::Result<f32> {
                    Ok(v.attribute(name).context("vertex without a coordinate")?.parse()?)
                };
                positions.push([coordinate("x")?, coordinate("y")?, coordinate("z")?]);
            }
        }

        // Triangles sorted by how they're coloured: a base material each, or corner colours
        let mut by_material: HashMap<Option<usize>, Vec<u32>> = HashMap::new();
        let mut cornered = Primitive::default();
        let triangles = mesh.children().find(|n| is_core(n, "triangles"));
        for t in triangles.iter().flat_map(|t| t.children()).filter(|n| is_core(n, "triangle")) {
            let corner =
                |name| -> anyhow::Result<u32> { Ok(t.attribute(name).context("triangle without a vertex")?.parse()?) };
            let v = [corner("v1")?, corner("v2")?, corner("v3")?];
            if v.iter().any(|&i| i as usize >= positions.len()) {
                bail!("triangle vertex out of range");
            }
            let pid = t.attribute("pid").or(object_pid);
            let p1 = t.attribute("p1").or(object_pindex).and_then(|p| p.parse::<usize>().ok()).unwrap_or(0);
            if let Some(colors) = pid.and_then(|pid| self.color_groups.get(pid)) {
                // Colours at the corners, the second and third defaulting to the first
                let p2 = t.attribute("p2").and_then(|p| p.parse().ok()).unwrap_or(p1);
                let p3 = t.attribute("p3").and_then(|p| p.parse().ok()).unwrap_or(p1);
                for (&vertex, p) in v.iter().zip([p1, p2, p3]) {
                    cornered.positions.push(positions[vertex as usize]);
                    cornered.colors.push(colors.get(p).copied().unwrap_or([1.0; 4]));
                }
                continue;
            }
            let material = pid.and_then(|pid| self.base_material(pid, p1));
            by_material.entry(material).or_default().extend(v);
        }

        let mut primitives = Vec::new();
        let mut groups: Vec<_> = by_material.into_iter().collect();
        groups.sort_by_key(|(m, _)| *m);
        for (material, indices) in groups {
            let material = material.unwrap_or_else(|| self.neutral());
            primitives.push(Primitive {
                normals: smooth_normals(&positions, &indices),
                positions: positions.clone(),
                indices,
                material,
                ..Primitive::default()
            });
        }
        if !cornered.positions.is_empty() {
            cornered.normals = smooth_normals(&cornered.positions, &[]);
            cornered.material = self.white();
            primitives.push(cornered);
        }
        self.model.meshes.push(Mesh { primitives });
        Ok(self.model.meshes.len() - 1)
    }

    fn base_material(&mut self, group: &str, index: usize) -> Option<usize> {
        let color = *self.base_materials.get(group)?.get(index)?;
        let key = (group.to_owned(), index);
        if let Some(&m) = self.materials.get(&key) {
            return Some(m);
        }
        let material = Material {
            base_color: color,
            alpha: if color[3] < 1.0 { AlphaMode::Blend } else { AlphaMode::Opaque },
            ..Material::neutral()
        };
        self.model.materials.push(material);
        self.materials.insert(key, self.model.materials.len() - 1);
        Some(self.model.materials.len() - 1)
    }

    fn neutral(&mut self) -> usize {
        self.shared(String::new(), Material::neutral())
    }

    fn white(&mut self) -> usize {
        self.shared("white".to_owned(), Material { base_color: [1.0; 4], ..Material::neutral() })
    }

    fn shared(&mut self, name: String, material: Material) -> usize {
        let key = (name, usize::MAX);
        if let Some(&m) = self.materials.get(&key) {
            return m;
        }
        self.model.materials.push(material);
        self.materials.insert(key, self.model.materials.len() - 1);
        self.model.materials.len() - 1
    }
}

fn is_core(node: &roxmltree::Node, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name && node.tag_name().namespace() == Some(CORE)
}

fn id(node: &roxmltree::Node) -> anyhow::Result<String> {
    Ok(node.attribute("id").context("resource without an id")?.to_owned())
}

/// #RRGGBB or #RRGGBBAA, to linear RGBA.
fn parse_color(text: &str) -> [f32; 4] {
    let hex = text.trim_start_matches('#');
    let channel = |i: usize| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()).map(|v| v as f32 / 255.0);
    let [r, g, b] = [0, 2, 4].map(|i| srgb_to_linear(channel(i).unwrap_or(1.0)));
    [r, g, b, channel(6).unwrap_or(1.0)]
}

/// Twelve numbers, a 4×3 matrix for row vectors: the last three are the translation.
fn parse_transform(text: &str) -> anyhow::Result<Mat4> {
    let values: Vec<f32> = text.split_ascii_whitespace().map(str::parse).collect::<Result<_, _>>()?;
    let [m00, m01, m02, m10, m11, m12, m20, m21, m22, m30, m31, m32] = values[..] else {
        bail!("a transform needs twelve numbers")
    };
    Ok(Mat4::from_cols_array(&[
        m00, m01, m02, 0.0, //
        m10, m11, m12, 0.0, //
        m20, m21, m22, 0.0, //
        m30, m31, m32, 1.0,
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn transforms_are_row_vector_matrices() {
        let m = parse_transform("1 0 0 0 1 0 0 0 1 10 20 30").unwrap();
        assert_eq!(m.transform_point3(Vec3::ZERO), Vec3::new(10.0, 20.0, 30.0));
        let m = parse_transform("0 1 0 -1 0 0 0 0 1 0 0 0").unwrap();
        assert!((m.transform_point3(Vec3::X) - Vec3::Y).length() < 1e-6);
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#FFFFFF80")[3], 128.0 / 255.0);
        assert_eq!(parse_color("#000000"), [0.0, 0.0, 0.0, 1.0]);
    }
}
