// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! The application: windows for the files it's given, the app actions and their shortcuts,
//! and About.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};

use crate::config::{APP_ID, RESOURCE_BASE, VERSION};
use crate::window::Window;

const ACCELS: &[(&str, &[&str])] = &[
    ("app.quit", &["<Control>q"]),
    ("app.new-window", &["<Control>n"]),
    ("win.open", &["<Control>o"]),
    ("win.close", &["<Control>w"]),
    ("win.copy-image", &["<Control>c"]),
    ("win.save-image", &["<Control>s"]),
    ("win.reset-view", &["<Control>0"]),
    ("win.properties", &["F9"]),
];

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Application;

    #[glib::object_subclass]
    impl ObjectSubclass for Application {
        const NAME: &'static str = "TurntableApplication";
        type Type = super::Application;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for Application {
        fn constructed(&self) {
            self.parent_constructed();
            let app = self.obj();
            let actions = [
                gio::ActionEntry::builder("new-window")
                    .activate(|app: &super::Application, _, _| Window::new(app).present())
                    .build(),
                gio::ActionEntry::builder("about").activate(|app: &super::Application, _, _| app.show_about()).build(),
                gio::ActionEntry::builder("quit")
                    .activate(|app: &super::Application, _, _| {
                        for window in app.windows() {
                            window.close();
                        }
                    })
                    .build(),
            ];
            app.add_action_entries(actions);
            for (action, accels) in ACCELS {
                app.set_accels_for_action(action, accels);
            }
        }
    }

    impl ApplicationImpl for Application {
        fn activate(&self) {
            let app = self.obj();
            match app.active_window() {
                Some(window) => window.present(),
                None => Window::new(&*app).present(),
            }
        }

        // Files from the file manager or the command line: the first goes to the active window
        // if it shows nothing yet, the rest to windows of their own.
        fn open(&self, files: &[gio::File], _hint: &str) {
            let app = self.obj();
            for file in files {
                let window = match app.active_window().and_downcast::<Window>() {
                    Some(window) if window.is_empty() => window,
                    _ => Window::new(&*app),
                };
                window.open_file(file);
                window.present();
            }
        }
    }

    impl GtkApplicationImpl for Application {}
    impl AdwApplicationImpl for Application {}
}

glib::wrapper! {
    pub struct Application(ObjectSubclass<imp::Application>)
        @extends adw::Application, gtk::Application, gio::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl Default for Application {
    fn default() -> Self {
        glib::Object::builder()
            .property("application-id", APP_ID)
            .property("flags", gio::ApplicationFlags::HANDLES_OPEN)
            .property("resource-base-path", RESOURCE_BASE)
            .build()
    }
}

impl Application {
    fn show_about(&self) {
        let about = adw::AboutDialog::from_appdata(&format!("{RESOURCE_BASE}/metainfo.xml"), Some(VERSION));
        about.set_version(VERSION);
        about.set_developers(&["Jack Tully"]);
        about.set_copyright("© 2026 Jack Tully");
        // Translators: credit yourself here, one name per line
        about.set_translator_credits(&gettext("translator-credits"));
        // The renderer's shading and camera controls are ported from three.js
        about.add_legal_section("three.js", Some("© 2010–2026 three.js authors"), gtk::License::MitX11, None);
        about.add_legal_section("ufbx", Some("© 2020 Samuli Raivio"), gtk::License::MitX11, None);
        about.add_legal_section("Draco", Some("© 2017 The Draco Authors"), gtk::License::Apache20, None);
        about.add_legal_section("meshoptimizer", Some("© 2016–2025 Arseny Kapoulkine"), gtk::License::MitX11, None);
        about.present(self.active_window().as_ref());
    }
}
