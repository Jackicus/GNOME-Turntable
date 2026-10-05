// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

use gtk::prelude::*;
use gtk::{gio, glib};
use turntable::config::PKGDATADIR;

fn main() -> glib::ExitCode {
    // GTK's GL renderer takes the viewer's frames as they are. Its Vulkan renderer hands each
    // one over from GL first, which costs a few milliseconds: on a 240 Hz display that was
    // 150–230 frames a second instead of 240. Anyone can still choose with GSK_RENDERER.
    if std::env::var_os("GSK_RENDERER").is_none() {
        // SAFETY: no other thread is running yet
        unsafe { std::env::set_var("GSK_RENDERER", "ngl") };
    }
    turntable::init_gettext();
    let resources = gio::Resource::load(format!("{PKGDATADIR}/turntable.gresource")).expect("the app’s resources");
    gio::resources_register(&resources);
    turntable::application::Application::default().run()
}
