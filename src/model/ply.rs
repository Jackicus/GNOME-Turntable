// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! PLY, ASCII or binary of either byte order. With faces it's a mesh (smooth normals made if
//! it has none); without, a cloud of points. Vertex colours are kept.

use anyhow::{Context, bail, ensure};

use super::{Material, Mesh, Model, Node, Primitive, Topology, smooth_normals, srgb_hex, srgb_to_linear};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Ascii,
    LittleEndian,
    BigEndian,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn parse(name: &str) -> anyhow::Result<Self> {
        Ok(match name {
            "char" | "int8" => Self::I8,
            "uchar" | "uint8" => Self::U8,
            "short" | "int16" => Self::I16,
            "ushort" | "uint16" => Self::U16,
            "int" | "int32" => Self::I32,
            "uint" | "uint32" => Self::U32,
            "float" | "float32" => Self::F32,
            "double" | "float64" => Self::F64,
            _ => bail!("unknown PLY type {name}"),
        })
    }

    fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }
}

struct Property {
    name: String,
    scalar: Scalar,
    /// For a list: the type of its length.
    count: Option<Scalar>,
}

struct Element {
    name: String,
    count: usize,
    properties: Vec<Property>,
}

/// Values one after another, as text or binary.
enum Body<'a> {
    Text(std::str::SplitAsciiWhitespace<'a>),
    Binary { bytes: &'a [u8], at: usize, big: bool },
}

impl Body<'_> {
    fn read(&mut self, scalar: Scalar) -> anyhow::Result<f64> {
        match self {
            Self::Text(words) => Ok(words.next().context("truncated PLY")?.parse()?),
            Self::Binary { bytes, at, big } => {
                let size = scalar.size();
                let b = bytes.get(*at..*at + size).context("truncated PLY")?;
                *at += size;
                let mut buf = [0u8; 8];
                buf[..size].copy_from_slice(b);
                if *big {
                    buf[..size].reverse();
                }
                Ok(match scalar {
                    Scalar::I8 => buf[0] as i8 as f64,
                    Scalar::U8 => buf[0] as f64,
                    Scalar::I16 => i16::from_le_bytes([buf[0], buf[1]]) as f64,
                    Scalar::U16 => u16::from_le_bytes([buf[0], buf[1]]) as f64,
                    Scalar::I32 => i32::from_le_bytes(buf[..4].try_into().unwrap()) as f64,
                    Scalar::U32 => u32::from_le_bytes(buf[..4].try_into().unwrap()) as f64,
                    Scalar::F32 => f32::from_le_bytes(buf[..4].try_into().unwrap()) as f64,
                    Scalar::F64 => f64::from_le_bytes(buf),
                })
            }
        }
    }
}

pub fn load(bytes: &[u8]) -> anyhow::Result<Model> {
    let (format, elements, body_start) = header(bytes)?;
    let mut body = match format {
        Format::Ascii => Body::Text(std::str::from_utf8(&bytes[body_start..])?.split_ascii_whitespace()),
        _ => Body::Binary { bytes, at: body_start, big: format == Format::BigEndian },
    };

    let mut primitive = Primitive::default();
    let mut has_color = false;
    let mut values = Vec::new();
    for element in &elements {
        // Each element is at least a byte of the file: a count from the header is checked
        // against it, and an element of nothing can't be counted at all
        ensure!(element.count == 0 || !element.properties.is_empty(), "an element without properties");
        ensure!(element.count <= bytes.len(), "more elements than the file holds");
        let is_vertex = element.name == "vertex";
        let is_face = element.name == "face";
        // Where each vertex property goes
        let slots: Vec<Option<usize>> = element
            .properties
            .iter()
            .map(|p| match p.name.as_str() {
                "x" => Some(0),
                "y" => Some(1),
                "z" => Some(2),
                "nx" => Some(3),
                "ny" => Some(4),
                "nz" => Some(5),
                "red" | "diffuse_red" | "r" => Some(6),
                "green" | "diffuse_green" | "g" => Some(7),
                "blue" | "diffuse_blue" | "b" => Some(8),
                "alpha" | "a" => Some(9),
                _ => None,
            })
            .collect();
        let has = |slot: usize| slots.contains(&Some(slot));
        let has_normals = is_vertex && has(3) && has(4) && has(5);
        if is_vertex {
            has_color = has(6) && has(7) && has(8);
            primitive.positions.reserve(element.count);
        }
        // Colours as bytes are 0–255; as floats, 0–1
        let color_scale: Vec<f64> = element
            .properties
            .iter()
            .map(|p| match p.scalar {
                Scalar::U8 | Scalar::I8 => 255.0,
                Scalar::U16 | Scalar::I16 => 65535.0,
                _ => 1.0,
            })
            .collect();

        for _ in 0..element.count {
            let mut vertex = [0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
            for (i, property) in element.properties.iter().enumerate() {
                if let Some(count) = property.count {
                    let n = body.read(count)? as usize;
                    values.clear();
                    for _ in 0..n {
                        values.push(body.read(property.scalar)?);
                    }
                    if is_face && matches!(property.name.as_str(), "vertex_indices" | "vertex_index") {
                        // A polygon, as a fan of triangles
                        for k in 2..n {
                            primitive.indices.extend([values[0], values[k - 1], values[k]].map(|v| v as u32));
                        }
                    }
                } else {
                    let value = body.read(property.scalar)?;
                    if let (true, Some(slot)) = (is_vertex, slots[i]) {
                        vertex[slot] = if slot >= 6 { value / color_scale[i] } else { value };
                    }
                }
            }
            if is_vertex {
                primitive.positions.push([vertex[0], vertex[1], vertex[2]].map(|v| v as f32));
                if has_normals {
                    primitive.normals.push([vertex[3], vertex[4], vertex[5]].map(|v| v as f32));
                }
                if has_color {
                    let [r, g, b] = [vertex[6], vertex[7], vertex[8]].map(|c| srgb_to_linear(c as f32));
                    primitive.colors.push([r, g, b, vertex[9] as f32]);
                }
            }
        }
    }
    ensure!(!primitive.positions.is_empty(), "no vertices");
    let count = primitive.positions.len() as u32;
    ensure!(primitive.indices.iter().all(|&i| i < count), "face index out of range");

    let mut material = Material::neutral();
    if primitive.indices.is_empty() {
        // No faces: a point cloud, its points 2 pixels across
        primitive.topology = Topology::Points;
        primitive.normals.clear();
        let [r, g, b] = if has_color { [1.0; 3] } else { srgb_hex(0x77767b) };
        material.base_color = [r, g, b, 1.0];
        material.unlit = true;
    } else if primitive.normals.is_empty() {
        primitive.normals = smooth_normals(&primitive.positions, &primitive.indices);
    }
    Ok(Model {
        nodes: vec![Node { mesh: Some(0), ..Node::default() }],
        meshes: vec![Mesh { primitives: vec![primitive] }],
        materials: vec![material],
        ..Model::default()
    })
}

fn header(bytes: &[u8]) -> anyhow::Result<(Format, Vec<Element>, usize)> {
    const END: &[u8] = b"end_header";
    let end = bytes.windows(END.len()).position(|w| w == END).context("PLY without end_header")?;
    let mut body_start = end + END.len();
    // The line ends with \n or \r\n
    if bytes.get(body_start) == Some(&b'\r') {
        body_start += 1;
    }
    if bytes.get(body_start) == Some(&b'\n') {
        body_start += 1;
    }
    let text = std::str::from_utf8(&bytes[..end]).context("PLY header isn’t text")?;
    let mut lines = text.lines();
    ensure!(lines.next().map(str::trim) == Some("ply"), "not a PLY file");

    let mut format = None;
    let mut elements: Vec<Element> = Vec::new();
    for line in lines {
        let words: Vec<&str> = line.split_ascii_whitespace().collect();
        match words.as_slice() {
            ["format", name, ..] => {
                format = Some(match *name {
                    "ascii" => Format::Ascii,
                    "binary_little_endian" => Format::LittleEndian,
                    "binary_big_endian" => Format::BigEndian,
                    _ => bail!("unknown PLY format {name}"),
                })
            }
            ["element", name, count] => {
                elements.push(Element { name: name.to_string(), count: count.parse()?, properties: Vec::new() })
            }
            ["property", "list", count, scalar, name] => {
                elements.last_mut().context("property before element")?.properties.push(Property {
                    name: name.to_string(),
                    scalar: Scalar::parse(scalar)?,
                    count: Some(Scalar::parse(count)?),
                })
            }
            ["property", scalar, name] => elements
                .last_mut()
                .context("property before element")?
                .properties
                .push(Property { name: name.to_string(), scalar: Scalar::parse(scalar)?, count: None }),
            _ => {}
        }
    }
    Ok((format.context("PLY without a format")?, elements, body_start))
}
