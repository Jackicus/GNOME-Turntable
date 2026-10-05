// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! The model formats the app opens, and how the properties sidebar writes what it measured.
//! No GTK here, so the tests run it without a display.

use std::ffi::CStr;

use gettextrs::gettext;
use gtk::glib;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loader {
    Gltf,
    Obj,
    Stl,
    Ply,
    Fbx,
    ThreeMf,
}

pub struct Format {
    pub extension: &'static str,
    pub loader: Loader,
    name: &'static str,
    /// What the file chooser and the desktop file offer (PLY's and FBX's are the app's own,
    /// data/*.mime.xml).
    pub mime_types: &'static [&'static str],
}

impl Format {
    /// The name shown, translated.
    pub fn name(&self) -> String {
        gettext(self.name)
    }
}

/// Marks a string for translation where it can't be translated yet; gettext() it when shown.
#[allow(non_snake_case)]
const fn N_(text: &str) -> &str {
    text
}

pub const FORMATS: &[Format] = &[
    Format { extension: "glb", loader: Loader::Gltf, name: N_("glTF Binary"), mime_types: &["model/gltf-binary"] },
    Format { extension: "gltf", loader: Loader::Gltf, name: N_("glTF"), mime_types: &["model/gltf+json"] },
    Format { extension: "obj", loader: Loader::Obj, name: N_("Wavefront OBJ"), mime_types: &["model/obj"] },
    Format { extension: "stl", loader: Loader::Stl, name: N_("STL"), mime_types: &["model/stl"] },
    Format { extension: "ply", loader: Loader::Ply, name: N_("PLY"), mime_types: &["model/x-ply"] },
    Format { extension: "fbx", loader: Loader::Fbx, name: N_("FBX"), mime_types: &["model/x-fbx"] },
    Format { extension: "3mf", loader: Loader::ThreeMf, name: N_("3MF"), mime_types: &["model/3mf"] },
];

pub fn extension_of(name: &str) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => name[dot + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// The format of a file name, or None when the app can't open it.
pub fn format_of(name: &str) -> Option<&'static Format> {
    let extension = extension_of(name);
    FORMATS.iter().find(|f| f.extension == extension)
}

/// Fills each %s in a translated string with the next value.
pub fn fill(template: &str, values: &[&str]) -> String {
    let mut values = values.iter();
    let mut parts = template.split("%s");
    let mut out = parts.next().unwrap_or_default().to_owned();
    for part in parts {
        out.push_str(values.next().copied().unwrap_or_default());
        out.push_str(part);
    }
    out
}

/// printf(3) a number in the user's locale: its digit grouping and decimal point.
fn printf(format: &CStr, decimals: i32, value: f64) -> String {
    let mut buffer = [0u8; 64];
    // SAFETY: the format takes one int and one double, and snprintf stays in the buffer
    let n = unsafe { libc::snprintf(buffer.as_mut_ptr().cast(), buffer.len(), format.as_ptr(), decimals, value) };
    String::from_utf8_lossy(&buffer[..n.clamp(0, buffer.len() as i32 - 1) as usize]).into_owned()
}

pub fn format_count(n: u64) -> String {
    printf(c"%'.*f", 0, n as f64)
}

/// At most three significant digits, as JavaScript's toLocaleString wrote them.
pub fn format_significant(value: f32) -> String {
    let value = value as f64;
    if value == 0.0 || !value.is_finite() {
        return printf(c"%'.*f", 0, 0.0);
    }
    let magnitude = value.abs().log10().floor() as i32;
    let decimals = (2 - magnitude).max(0);
    let scale = 10f64.powi(2 - magnitude);
    let rounded = (value * scale).round() / scale;
    let text = printf(c"%'.*f", decimals, rounded);
    if decimals == 0 {
        return text;
    }
    // Trailing zeros go, and the decimal point if nothing's left after it
    // SAFETY: localeconv's result is valid until the next setlocale
    let point = unsafe { CStr::from_ptr((*libc::localeconv()).decimal_point) }.to_string_lossy().into_owned();
    match text.rfind(&point) {
        Some(at) => {
            let trimmed = text.trim_end_matches('0');
            if trimmed.len() == at + point.len() { trimmed[..at].to_owned() } else { trimmed.to_owned() }
        }
        None => text,
    }
}

/// Width × height × depth in the model's own units, which a file rarely names.
pub fn format_dimensions([x, y, z]: [f32; 3]) -> String {
    // Translators: a model's width × height × depth, in the units it was made in
    fill(&gettext("%s × %s × %s"), &[&format_significant(x), &format_significant(y), &format_significant(z)])
}

pub fn format_modified(unix_seconds: i64) -> String {
    // Translators: when a file was last changed, in strftime(3) format
    let format = gettext("%-d %B %Y, %H:%M");
    glib::DateTime::from_unix_local(unix_seconds).and_then(|d| d.format(&format)).map(Into::into).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_by_extension_any_case() {
        assert_eq!(format_of("Helmet.GLB").unwrap().loader, Loader::Gltf);
        assert_eq!(format_of("part.3mf").unwrap().loader, Loader::ThreeMf);
        assert!(format_of("notes.txt").is_none());
        assert!(format_of(".obj").is_none());
        assert_eq!(extension_of("archive.tar.gz"), "gz");
    }

    #[test]
    fn fill_and_dimensions() {
        assert_eq!(fill("%s of %s", &["one", "two"]), "one of two");
        // The tests run in the C locale: no grouping, a full stop
        assert_eq!(format_dimensions([1.0, 2.5, 0.123456]), "1 × 2.5 × 0.123");
        assert_eq!(format_significant(1234.5), "1230");
        assert_eq!(format_significant(9.996), "10");
        assert_eq!(format_significant(0.000_123_4), "0.000123");
        assert_eq!(format_count(1_234_567), "1234567");
    }

    fn file(path: &str) -> String {
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/").to_owned() + path).unwrap()
    }

    #[test]
    fn the_desktop_file_and_metainfo_offer_the_formats() {
        let mut expected: Vec<&str> = FORMATS.iter().flat_map(|f| f.mime_types.iter().copied()).collect();
        expected.sort();
        let desktop = file("data/io.github.jackicus.Turntable.desktop.in");
        let line = desktop.lines().find_map(|l| l.strip_prefix("MimeType=")).unwrap();
        let mut offered: Vec<&str> = line.split(';').filter(|s| !s.is_empty()).collect();
        offered.sort();
        assert_eq!(offered, expected);

        let metainfo = file("data/io.github.jackicus.Turntable.metainfo.xml.in");
        let mut provided: Vec<&str> = metainfo
            .lines()
            .filter_map(|l| l.trim().strip_prefix("<mediatype>")?.strip_suffix("</mediatype>"))
            .collect();
        provided.sort();
        assert_eq!(provided, expected);
    }
}
