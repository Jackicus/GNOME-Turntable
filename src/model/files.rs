// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! The files a model refers to: a glTF's buffers and images, the textures of an FBX or OBJ.
//! They're read only from the model's own folder (or below it), never from anywhere else on
//! the disk, and never from the network.

use std::path::{Path, PathBuf};

use base64::Engine;

/// The file `reference` names, relative to `folder`, or None if it would leave the folder.
/// `reference` is a relative URI as glTF writes them: percent-encoded, `/`-separated.
pub fn resolve(folder: &Path, reference: &str) -> Option<PathBuf> {
    if reference.is_empty() || reference.contains('\\') || reference.starts_with('/') {
        return None;
    }
    // A scheme (http:, file:) before any slash makes it absolute
    let first = reference.split('/').next().unwrap_or_default();
    if first.contains(':') {
        return None;
    }
    let decoded = percent_decode(reference.split(['?', '#']).next()?)?;
    let mut parts: Vec<&str> = Vec::new();
    for part in decoded.split('/') {
        match part {
            "." => {}
            "" => return None,
            ".." => {
                parts.pop()?;
            }
            _ if part.contains('\\') => return None,
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.iter().fold(folder.to_path_buf(), |path, part| path.join(part)))
}

/// Reads what `reference` names: a data: URI's contents, or a file in `folder`.
pub fn read(folder: &Path, reference: &str) -> anyhow::Result<Vec<u8>> {
    if let Some(data) = reference.strip_prefix("data:") {
        let (header, payload) = data.split_once(',').ok_or_else(|| anyhow::anyhow!("bad data URI"))?;
        return if header.ends_with(";base64") {
            Ok(base64::engine::general_purpose::STANDARD.decode(payload.trim())?)
        } else {
            percent_decode(payload).map(String::into_bytes).ok_or_else(|| anyhow::anyhow!("bad data URI"))
        };
    }
    let path = resolve(folder, reference).ok_or_else(|| Reference::Outside(reference.to_owned()))?;
    std::fs::read(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Reference::Missing(path.display().to_string()).into(),
        _ => anyhow::anyhow!("{}: {e}", path.display()),
    })
}

/// A file the model needs that it can't have.
#[derive(Debug)]
pub enum Reference {
    /// It names a file outside its own folder
    Outside(String),
    /// The file isn't there
    Missing(String),
}

impl std::fmt::Display for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Outside(reference) => write!(f, "{reference} is outside the model’s folder"),
            Self::Missing(path) => write!(f, "{path} is missing"),
        }
    }
}

impl std::error::Error for Reference {}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOLDER: &str = "/models/robot";

    fn resolved(reference: &str) -> Option<String> {
        resolve(Path::new(FOLDER), reference).map(|p| p.display().to_string())
    }

    #[test]
    fn resolve_inside_the_folder() {
        assert_eq!(resolved("robot.bin").as_deref(), Some("/models/robot/robot.bin"));
        assert_eq!(resolved("textures/skin.png").as_deref(), Some("/models/robot/textures/skin.png"));
        assert_eq!(resolved("./robot.bin").as_deref(), Some("/models/robot/robot.bin"));
        assert_eq!(resolved("a/../robot.bin").as_deref(), Some("/models/robot/robot.bin"));
        assert_eq!(resolved("my%20skin.png").as_deref(), Some("/models/robot/my skin.png"));
    }

    #[test]
    fn never_leave_the_folder() {
        for reference in [
            "../secret",
            "a/../../secret",
            "",
            ".",
            "/etc/passwd",
            "a//b",
            "..\\secret",
            "%2e%2e/secret",
            "file:///etc/passwd",
            "https://example.com/robot.bin",
            "C:/Windows/robot.bin",
        ] {
            assert_eq!(resolved(reference), None, "{reference}");
        }
    }

    #[test]
    fn data_uris() {
        let folder = Path::new(FOLDER);
        assert_eq!(read(folder, "data:application/octet-stream;base64,AAEC").unwrap(), [0, 1, 2]);
        assert_eq!(read(folder, "data:text/plain,a%20b").unwrap(), b"a b");
    }
}
