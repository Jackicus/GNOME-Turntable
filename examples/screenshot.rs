// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! A PNG of a window of the app, with a model open or not: scripts/screenshot.sh runs it on
//! the headless display. The app is the installed build's (its resources and schema), so
//! scripts/run.sh or `meson install` first.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, gio, glib, graphene};
use turntable::application::Application;
use turntable::window::Window;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

struct Options {
    out: String,
    model: Option<String>,
    light: bool,
    properties: bool,
    size: (i32, i32),
    sets: Vec<(String, String)>,
    wait: f64,
    /// Where in the animation, in seconds
    time: Option<f64>,
    /// Only the view, as Copy Image takes it
    view: bool,
}

fn options() -> Options {
    let mut args = std::env::args().skip(1);
    let mut options = Options {
        out: String::new(),
        model: None,
        light: false,
        properties: false,
        size: (1000, 720),
        sets: Vec::new(),
        wait: 2.0,
        time: None,
        view: false,
    };
    let mut positional = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--light" => options.light = true,
            "--properties" => options.properties = true,
            "--view" => options.view = true,
            "--size" => {
                let size = args.next().unwrap_or_default();
                let (w, h) = size.split_once('x').expect("--size WxH");
                options.size = (w.parse().expect("width"), h.parse().expect("height"));
            }
            "--set" => {
                let set = args.next().unwrap_or_default();
                let (key, value) = set.split_once('=').expect("--set KEY=VALUE");
                options.sets.push((key.to_owned(), value.to_owned()));
            }
            "--wait" => options.wait = args.next().and_then(|w| w.parse().ok()).expect("--wait SECONDS"),
            "--time" => options.time = Some(args.next().and_then(|t| t.parse().ok()).expect("--time SECONDS")),
            _ => positional.push(arg),
        }
    }
    let mut positional = positional.into_iter();
    options.out = positional.next().unwrap_or_else(|| {
        eprintln!("usage: screenshot OUT.png [MODEL] [--light] [--size WxH] [--properties] [--set KEY=VALUE] [--wait S] [--time S] [--view]");
        std::process::exit(2);
    });
    options.model = positional.next();
    options
}

fn capture(window: &Window, out: &str) {
    let (w, h) = (window.width(), window.height());
    let snapshot = gtk::Snapshot::new();
    gtk::WidgetPaintable::new(Some(window)).snapshot(&snapshot, w as f64, h as f64);
    let node = snapshot.to_node().expect("something drawn");
    let renderer = window.renderer().expect("a renderer");
    let texture = renderer.render_texture(&node, Some(&graphene::Rect::new(0.0, 0.0, w as f32, h as f32)));
    texture.save_to_png(out).expect("saved");
    println!("screenshot: {out} ({w}×{h})");
}

fn main() -> glib::ExitCode {
    let options = options();
    let install = format!("{ROOT}/build/install");
    // SAFETY: nothing else runs yet
    unsafe {
        std::env::set_var("GSETTINGS_SCHEMA_DIR", format!("{install}/share/glib-2.0/schemas"));
        std::env::set_var("GSETTINGS_BACKEND", "memory");
        let data = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/share".to_owned());
        std::env::set_var("XDG_DATA_DIRS", format!("{install}/share:{data}"));
    }
    turntable::init_gettext();
    let resources = gio::Resource::load(format!("{install}/share/turntable/turntable.gresource"))
        .expect("run scripts/run.sh first");
    gio::resources_register(&resources);

    let settings = gio::Settings::new(turntable::config::APP_ID);
    settings.set_int("window-width", options.size.0).unwrap();
    settings.set_int("window-height", options.size.1).unwrap();
    for (key, value) in &options.sets {
        let kind = settings.value(key).type_().to_string();
        let value = if kind == "s" {
            value.to_variant()
        } else {
            glib::Variant::parse(Some(glib::VariantTy::new(&kind).unwrap()), value).expect("a value")
        };
        settings.set_value(key, &value).expect("a setting");
    }

    let app = Application::default();
    let light = options.light;
    app.connect_startup(move |_| {
        adw::StyleManager::default().set_color_scheme(if light {
            adw::ColorScheme::ForceLight
        } else {
            adw::ColorScheme::ForceDark
        });
        gtk::IconTheme::for_display(&gdk::Display::default().unwrap())
            .add_search_path(format!("{ROOT}/build/install/share/icons"));
    });
    let model = options.model.clone();
    let has_model = model.is_some();
    let options = Rc::new(options);
    app.connect_window_added(move |app, window| {
        let Some(window) = window.downcast_ref::<Window>().cloned() else { return };
        let started = Instant::now();
        let options = options.clone();
        let app = app.clone();
        let done = Rc::new(Cell::new(false));
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let settled = !has_model || (window.is_settled() && started.elapsed() > Duration::from_millis(500));
            if done.get() || (!settled && started.elapsed() < Duration::from_secs(30)) {
                return glib::ControlFlow::Continue;
            }
            done.set(true);
            window.set_show_properties(options.properties);
            // Still, so the picture is of one moment
            window.viewer().set_playing(false);
            window.viewer().set_spin(false);
            if let Some(time) = options.time {
                window.viewer().seek(time);
            }
            let (window, app, options) = (window.clone(), app.clone(), options.clone());
            glib::timeout_add_local_once(Duration::from_secs_f64(options.wait), move || {
                if options.view {
                    let texture = window.viewer().snapshot().expect("a picture of the view");
                    texture.save_to_png(&options.out).expect("saved");
                    println!("screenshot: {} ({}×{}, the view)", options.out, texture.width(), texture.height());
                } else {
                    capture(&window, &options.out);
                }
                app.quit();
            });
            glib::ControlFlow::Break
        });
    });
    let args: Vec<String> = std::iter::once("turntable".to_owned()).chain(model).collect();
    app.run_with_args(&args)
}
