// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! The project's files agree with each other.

use std::path::Path;

fn read(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}

fn sources(dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs" || e == "blp") {
            out.push(path.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap().display().to_string());
        }
    }
}

#[test]
fn every_file_with_strings_is_translated() {
    let potfiles = read("po/POTFILES.in");
    let mut files = Vec::new();
    sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    for file in files {
        let text = read(&file);
        // A call of gettext, N_ or Blueprint's _, not a name that ends with one
        let called = |name: &str| {
            text.match_indices(name).any(|(at, _)| !text[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_'))
        };
        let translatable = called("gettext(") || called("N_(\"") || called("_(\"");
        assert_eq!(potfiles.lines().any(|l| l == file), translatable, "{file} in po/POTFILES.in");
    }
}

#[test]
fn every_source_file_has_its_licence() {
    let mut files = Vec::new();
    for dir in ["src", "tests", "examples"] {
        sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join(dir), &mut files);
    }
    for file in files {
        let text = read(&file);
        assert!(text.contains("SPDX-License-Identifier: GPL-2.0-or-later"), "{file}");
        assert!(text.contains("SPDX-FileCopyrightText: 2026 Jack Tully"), "{file}");
    }
}

#[test]
fn one_version_everywhere() {
    let version = env!("CARGO_PKG_VERSION");
    assert!(read("meson.build").contains(&format!("version: '{version}'")), "meson.build");
    assert!(
        read("data/io.github.jackicus.Turntable.metainfo.xml.in").contains(&format!("<release version=\"{version}\"")),
        "the metainfo's newest release"
    );
}

#[test]
fn flatpak_vendors_every_crate() {
    // Regenerate with flatpak-cargo-generator.py Cargo.lock -o build-aux/flatpak/cargo-sources.json
    let sources = read("build-aux/flatpak/cargo-sources.json");
    let lock = read("Cargo.lock");
    for package in lock.split("[[package]]").skip(1) {
        let field = |name: &str| {
            package.lines().find_map(|l| l.strip_prefix(&format!("{name} = \"")).and_then(|v| v.strip_suffix('"')))
        };
        let (Some(name), Some(version)) = (field("name"), field("version")) else { continue };
        if field("source").is_none() {
            continue; // this crate itself
        }
        assert!(sources.contains(&format!("{name}-{version}")), "{name} {version} isn’t in cargo-sources.json");
    }
}
