// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! The loaders on the small invented models in tests/models, and on files made here.

use std::io::Write;
use std::path::{Path, PathBuf};

use glam::Vec3;
use turntable::formats::format_of;
use turntable::model::{Error, Loaded, Map, Pose, load};

fn models() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/models")
}

fn open(path: &Path) -> Result<Loaded, Error> {
    let name = path.file_name().unwrap().to_str().unwrap();
    load(path, format_of(name).expect("a format").loader)
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// Every model stands on the floor, centred over the origin.
fn assert_on_the_floor(loaded: &Loaded) {
    let (min, max) = loaded.model.bounds().unwrap();
    assert!(near(min.y, 0.0), "bottom at {}", min.y);
    let center = (min + max) / 2.0;
    assert!(near(center.x, 0.0) && near(center.z, 0.0), "centred at {center}");
}

#[test]
fn obj_with_its_material() {
    let loaded = open(&models().join("cube.obj")).unwrap();
    let stats = &loaded.stats;
    assert_eq!((stats.triangles, stats.meshes, stats.materials), (12, 1, 1));
    assert_eq!(stats.size, [1.0, 1.0, 1.0]);
    // Kd 1.0 0.47 0.0 from cube.mtl, as linear
    let material = loaded.model.materials.iter().find(|m| m.base_color[0] == 1.0).expect("the orange material");
    assert!(material.base_color[1] > 0.15 && material.base_color[1] < 0.25);
    assert_on_the_floor(&loaded);
}

#[test]
fn stl_is_turned_upright() {
    let loaded = open(&models().join("icosahedron.stl")).unwrap();
    assert_eq!((loaded.stats.triangles, loaded.stats.vertices), (20, 60));
    assert!(near(loaded.height, loaded.stats.size[1]));
    assert_on_the_floor(&loaded);
}

#[test]
fn ply_without_faces_is_points() {
    let loaded = open(&models().join("spiral.ply")).unwrap();
    assert_eq!((loaded.stats.points, loaded.stats.meshes, loaded.stats.triangles), (2000, 0, 0));
    let primitive = &loaded.model.meshes[0].primitives[0];
    assert_eq!(primitive.colors.len(), 2000);
}

#[test]
fn damaged_and_missing_files() {
    assert!(matches!(open(&models().join("broken.glb")), Err(Error::Unsupported(_))));
    assert!(matches!(open(&models().join("nothing-here.glb")), Err(Error::NotFound)));
}

/// A glTF with a triangle that moves up two units in a second, its buffer inline.
fn moving_triangle() -> String {
    let mut buffer = Vec::new();
    for v in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
        buffer.extend(v.to_le_bytes());
    }
    // Key times, then where it is at each
    for v in [0.0f32, 1.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0] {
        buffer.extend(v.to_le_bytes());
    }
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.encode(&buffer);
    format!(
        r#"{{
        "asset": {{"version": "2.0"}},
        "scene": 0, "scenes": [{{"nodes": [0]}}],
        "nodes": [{{"mesh": 0, "name": "triangle"}}],
        "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0}}}}]}}],
        "animations": [{{"name": "Rise", "channels": [{{"sampler": 0, "target": {{"node": 0, "path": "translation"}}}}],
            "samplers": [{{"input": 1, "output": 2}}]}}],
        "accessors": [
            {{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"}},
            {{"bufferView": 1, "componentType": 5126, "count": 2, "type": "SCALAR"}},
            {{"bufferView": 1, "byteOffset": 8, "componentType": 5126, "count": 2, "type": "VEC3"}}
        ],
        "bufferViews": [{{"buffer": 0, "byteLength": 36}}, {{"buffer": 0, "byteOffset": 36, "byteLength": 32}}],
        "buffers": [{{"byteLength": 68, "uri": "data:application/octet-stream;base64,{data}"}}]
    }}"#
    )
}

#[test]
fn gltf_with_an_animation() {
    let dir = tempdir();
    let path = dir.join("triangle.gltf");
    std::fs::write(&path, moving_triangle()).unwrap();
    let loaded = open(&path).unwrap();
    assert_eq!(loaded.stats.triangles, 1);
    assert_eq!(loaded.stats.clips, vec![("Rise".to_owned(), 1.0)]);
    // A primitive without normals is shaded flat
    assert!(loaded.model.meshes[0].primitives[0].flat);

    let model = &loaded.model;
    let mut pose: Pose = model.rest_pose();
    let mut worlds = Vec::new();
    model.clips[0].apply(0.5, &mut pose);
    model.world_matrices(&pose, &mut worlds);
    let rest = {
        let mut rest = Vec::new();
        model.world_matrices(&model.rest_pose(), &mut rest);
        rest[0].transform_point3(Vec3::ZERO)
    };
    let moved = worlds[0].transform_point3(Vec3::ZERO) - rest;
    assert!(near(moved.y, 1.0), "halfway up at half a second: {moved}");
}

#[test]
fn gltf_never_reads_outside_its_folder() {
    let dir = tempdir();
    let path = dir.join("escape.gltf");
    let json = moving_triangle().replace("data:application/octet-stream;base64,", "../outside.bin#");
    std::fs::write(&path, json).unwrap();
    assert!(matches!(open(&path), Err(Error::Outside(_))));
    let json = moving_triangle().replace("data:application/octet-stream;base64,", "triangle.bin#");
    std::fs::write(&path, json).unwrap();
    assert!(matches!(open(&path), Err(Error::Missing(_))));
}

#[test]
fn threemf_with_components_and_colours() {
    let model = r##"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <resources>
    <basematerials id="1"><base name="Red" displaycolor="#FF0000"/></basematerials>
    <object id="2" type="model" pid="1" pindex="0">
      <mesh>
        <vertices><vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="0" y="10" z="0"/><vertex x="0" y="0" z="5"/></vertices>
        <triangles><triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="1" v3="3"/><triangle v1="1" v2="2" v3="3"/><triangle v1="2" v2="0" v3="3"/></triangles>
      </mesh>
    </object>
    <object id="3" type="model"><components><component objectid="2" transform="1 0 0 0 1 0 0 0 1 20 0 0"/></components></object>
  </resources>
  <build><item objectid="2"/><item objectid="3"/></build>
</model>"##;
    let dir = tempdir();
    let path = dir.join("parts.3mf");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    zip.start_file("3D/3dmodel.model", zip::write::SimpleFileOptions::default()).unwrap();
    zip.write_all(model.as_bytes()).unwrap();
    zip.finish().unwrap();

    let loaded = open(&path).unwrap();
    // Two tetrahedra, the second 20 mm along: 30 wide, 5 tall (Z is up), 10 deep
    assert_eq!(loaded.stats.triangles, 8);
    let [w, h, d] = loaded.stats.size;
    assert!(near(w, 30.0) && near(h, 5.0) && near(d, 10.0), "{:?}", loaded.stats.size);
    let red = &loaded.model.materials[0];
    assert_eq!(red.base_color, [1.0, 0.0, 0.0, 1.0]);
    assert!(red.map(Map::BaseColor).is_none());
    assert_on_the_floor(&loaded);
}

/// Damaged copies of the test models (cut short, bytes changed, a glTF's numbers changed) are
/// refused, never a crash.
#[test]
fn damaged_files_never_crash() {
    let dir = tempdir();
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut originals: Vec<(String, Vec<u8>)> = ["cube.obj", "icosahedron.stl", "spiral.ply"]
        .iter()
        .map(|name| (name.to_string(), std::fs::read(models().join(name)).unwrap()))
        .collect();
    originals.push(("triangle.gltf".to_owned(), moving_triangle().into_bytes()));
    for (name, original) in &originals {
        let path = dir.join(name);
        for round in 0..200 {
            let mut data = original.clone();
            match round % 3 {
                0 => data.truncate(next() as usize % data.len()),
                1 => {
                    for _ in 0..1 + next() % 8 {
                        let at = next() as usize % data.len();
                        data[at] = next() as u8;
                    }
                }
                _ => {
                    // A digit becomes another, or a long run of them
                    let digits: Vec<usize> = (0..data.len()).filter(|&i| data[i].is_ascii_digit()).collect();
                    let at = digits[next() as usize % digits.len()];
                    let replacement = (next() % 4_294_967_295).to_string();
                    data.splice(at..at + 1, replacement.bytes().take(1 + next() as usize % 10));
                }
            }
            std::fs::write(&path, &data).unwrap();
            let result = std::panic::catch_unwind(|| open(&path));
            assert!(result.is_ok(), "{name}, round {round}, panicked");
        }
    }
}

/// A folder of its own for each test, gone when the test ends.
struct TempDir(PathBuf);

impl std::ops::Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn tempdir() -> TempDir {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir()
        .join(format!("turntable-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
        .join("models");
    std::fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}
