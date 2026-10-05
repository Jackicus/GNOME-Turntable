// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! STL, binary or ASCII: separate triangles, each with its face's normal. Binary files may
//! colour their faces (the "COLOR=" header of Materialise Magics and others). STL is Z-up.

use std::f32::consts::FRAC_PI_2;

use anyhow::{Context, bail, ensure};
use glam::{Mat4, Vec3};

use super::{Material, Mesh, Model, Node, Primitive};

pub fn load(bytes: &[u8]) -> anyhow::Result<Model> {
    let mut primitive = if is_binary(bytes) { binary(bytes)? } else { ascii(bytes)? };
    let mut material = Material::neutral();
    if let Some(alpha) = primitive.colors.first().map(|c| c[3]) {
        if alpha < 1.0 {
            material.alpha = super::AlphaMode::Blend;
        }
    }
    fill_missing_normals(&mut primitive);
    Ok(Model {
        nodes: vec![Node { mesh: Some(0), ..Node::default() }],
        meshes: vec![Mesh { primitives: vec![primitive] }],
        materials: vec![material],
        root: Mat4::from_rotation_x(-FRAC_PI_2),
        ..Model::default()
    })
}

/// As three.js decides: binary if the face count fits the size exactly, else ASCII if it
/// starts with "solid".
fn is_binary(bytes: &[u8]) -> bool {
    if bytes.len() >= 84 {
        let faces = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
        if 84 + faces * 50 == bytes.len() {
            return true;
        }
    }
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(0);
    !bytes[start..].starts_with(b"solid")
}

fn binary(bytes: &[u8]) -> anyhow::Result<Primitive> {
    ensure!(bytes.len() >= 84, "too short for STL");
    let faces = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
    ensure!(bytes.len() >= 84 + faces * 50, "STL shorter than its face count");

    // "COLOR=" and RGBA in the header: faces may have colours of their own
    let header = &bytes[..80];
    let default_color = header
        .windows(10)
        .find(|w| w.starts_with(b"COLOR="))
        .map(|w| [w[6], w[7], w[8], w[9]].map(|c| c as f32 / 255.0));

    let float = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let mut primitive = Primitive::default();
    primitive.positions.reserve(faces * 3);
    primitive.normals.reserve(faces * 3);
    for face in 0..faces {
        let at = 84 + face * 50;
        let normal = [float(at), float(at + 4), float(at + 8)];
        for v in 0..3 {
            let p = at + 12 + v * 12;
            primitive.positions.push([float(p), float(p + 4), float(p + 8)]);
            primitive.normals.push(normal);
        }
        if let Some(default) = default_color {
            let packed = u16::from_le_bytes([bytes[at + 48], bytes[at + 49]]);
            // Bit 15 clear: the face's own 5-bit red, green and blue
            let color = if packed & 0x8000 == 0 {
                let channel = |shift: u16| ((packed >> shift) & 31) as f32 / 31.0;
                [channel(0), channel(5), channel(10), default[3]]
            } else {
                default
            };
            primitive.colors.extend([color; 3]);
        }
    }
    Ok(primitive)
}

fn ascii(bytes: &[u8]) -> anyhow::Result<Primitive> {
    let text = std::str::from_utf8(bytes).context("STL is neither binary nor text")?;
    let mut primitive = Primitive::default();
    let mut normal = [0.0; 3];
    let mut words = text.split_ascii_whitespace();
    let numbers = |words: &mut std::str::SplitAsciiWhitespace| -> anyhow::Result<[f32; 3]> {
        let mut out = [0.0; 3];
        for v in &mut out {
            *v = words.next().context("truncated STL")?.parse().context("not a number")?;
        }
        Ok(out)
    };
    while let Some(word) = words.next() {
        match word {
            "facet" => {
                if words.next() == Some("normal") {
                    normal = numbers(&mut words)?;
                }
            }
            "vertex" => {
                primitive.positions.push(numbers(&mut words)?);
                primitive.normals.push(normal);
            }
            _ => {}
        }
    }
    if primitive.positions.len() % 3 != 0 {
        bail!("a facet without three vertices");
    }
    Ok(primitive)
}

/// Many exporters write zero normals: the face's own, from its vertices, instead.
fn fill_missing_normals(primitive: &mut Primitive) {
    for (positions, normals) in primitive.positions.chunks_exact(3).zip(primitive.normals.chunks_exact_mut(3)) {
        let n = Vec3::from(normals[0]);
        if n.length_squared() > 0.5 && n.is_finite() {
            continue;
        }
        let [a, b, c] = [0, 1, 2].map(|i| Vec3::from(positions[i]));
        let face = (b - a).cross(c - a).normalize_or_zero().to_array();
        normals.fill(face);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_and_binary_agree() {
        let text = b"solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n";
        let a = ascii(text).unwrap();
        let mut bin = vec![0u8; 80];
        bin.extend(1u32.to_le_bytes());
        for v in [0.0f32, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            bin.extend(v.to_le_bytes());
        }
        bin.extend([0, 0]);
        assert!(is_binary(&bin));
        assert!(!is_binary(text));
        let b = binary(&bin).unwrap();
        assert_eq!(a.positions, b.positions);
        assert_eq!(a.normals, b.normals);
    }
}
