// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! Where the app is installed. Meson sets these when it builds; a plain `cargo build` gets
//! the defaults of a /usr install.

pub const APP_ID: &str = "io.github.jackicus.Turntable";
pub const RESOURCE_BASE: &str = "/io/github/jackicus/Turntable";
pub const GETTEXT_PACKAGE: &str = "turntable";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const PKGDATADIR: &str = match option_env!("TURNTABLE_PKGDATADIR") {
    Some(dir) => dir,
    None => "/usr/share/turntable",
};

pub const LOCALEDIR: &str = match option_env!("TURNTABLE_LOCALEDIR") {
    Some(dir) => dir,
    None => "/usr/share/locale",
};
