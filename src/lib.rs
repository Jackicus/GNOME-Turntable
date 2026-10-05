// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! 3D Viewer: a GNOME app that shows 3D models. The binary (main.rs) runs the Application;
//! the screenshot example drives the same code.

// Links meshoptimizer, whose decoders model/gltf.rs calls directly
extern crate meshopt;

pub mod application;
pub mod camera;
pub mod config;
pub mod formats;
pub mod model;
pub mod render;
pub mod viewer;
pub mod window;

use gettextrs::{LocaleCategory, bind_textdomain_codeset, bindtextdomain, setlocale, textdomain};

/// Logs a warning in the app's own domain.
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        gtk::glib::g_warning!("turntable", $($arg)*)
    };
}

/// The user's locale, and the app's translations.
pub fn init_gettext() {
    // SAFETY: before any other thread starts
    unsafe { setlocale(LocaleCategory::LcAll, "") };
    let _ = bindtextdomain(config::GETTEXT_PACKAGE, config::LOCALEDIR);
    let _ = bind_textdomain_codeset(config::GETTEXT_PACKAGE, "UTF-8");
    let _ = textdomain(config::GETTEXT_PACKAGE);
}
