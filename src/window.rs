// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! A window shows one model: the viewer under a header bar, its controls floating over it,
//! and its properties in a sidebar. The view settings (lighting, display mode, grid, axes,
//! rotation) are the app's GSettings, so every window follows the same ones.

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gdk, gio, glib};

use crate::config::APP_ID;
use crate::formats::{FORMATS, Format, fill, format_count, format_dimensions, format_modified, format_of};
use crate::model::{self, Stats, srgb_to_linear};
use crate::render::{Display, Lighting};
use crate::viewer::Viewer;

const VIEW_SETTINGS: [&str; 6] =
    ["auto-rotate", "lighting", "display-mode", "show-grid", "show-axes", "show-frame-rate"];
const MODEL_ACTIONS: [&str; 5] = ["reset-view", "copy-image", "save-image", "show-in-files", "properties"];

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/jackicus/Turntable/window.ui")]
    pub struct Window {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub window_title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub split_view: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub viewer: TemplateChild<Viewer>,
        #[template_child]
        pub spinner: TemplateChild<adw::Spinner>,
        #[template_child]
        pub controls: TemplateChild<gtk::Box>,
        #[template_child]
        pub animation_controls: TemplateChild<gtk::Box>,
        #[template_child]
        pub play_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub animation_scale: TemplateChild<gtk::Scale>,
        #[template_child]
        pub clip_dropdown: TemplateChild<gtk::DropDown>,
        #[template_child]
        pub error_page: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub drop_area: TemplateChild<gtk::Box>,
        #[template_child]
        pub dimensions_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub vertices_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub triangles_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub points_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub meshes_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub materials_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub animations_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub format_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub size_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub folder_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub modified_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub frame_rate: TemplateChild<gtk::Label>,

        pub settings: RefCell<Option<gio::Settings>>,
        pub file: RefCell<Option<gio::File>>,
        /// Counts openings, so a slow one that's been overtaken is dropped
        pub loading: Cell<u64>,
        pub clips: RefCell<Vec<(String, f32)>>,
        pub playing: Cell<bool>,
        /// When the model being opened was asked for
        pub opened: Cell<Option<std::time::Instant>>,
        /// Updates the frame rate readout while it's shown
        pub frame_rate_timer: RefCell<Option<glib::SourceId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Window {
        const NAME: &'static str = "TurntableWindow";
        type Type = super::Window;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            Viewer::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Window {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().set_up();
        }
    }

    impl WidgetImpl for Window {}

    impl WindowImpl for Window {
        fn close_request(&self) -> glib::Propagation {
            self.obj().save_size();
            self.parent_close_request()
        }
    }

    impl ApplicationWindowImpl for Window {}
    impl AdwApplicationWindowImpl for Window {}
}

glib::wrapper! {
    pub struct Window(ObjectSubclass<imp::Window>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl Window {
    pub fn new(application: &impl IsA<gtk::Application>) -> Self {
        glib::Object::builder().property("application", application).build()
    }

    fn settings(&self) -> gio::Settings {
        self.imp().settings.borrow().clone().expect("settings")
    }

    /// True while the window shows nothing, or an error.
    pub fn is_empty(&self) -> bool {
        self.imp().file.borrow().is_none()
    }

    /// True once a model has been opened and shown (or failed to be).
    pub fn is_settled(&self) -> bool {
        !self.is_empty() && !self.imp().spinner.is_visible()
    }

    pub fn viewer(&self) -> Viewer {
        self.imp().viewer.get()
    }

    pub fn set_show_properties(&self, show: bool) {
        self.imp().split_view.set_show_sidebar(show);
    }

    fn set_up(&self) {
        let imp = self.imp();
        let settings = gio::Settings::new(APP_ID);
        imp.settings.replace(Some(settings.clone()));

        let viewer = imp.viewer.get();
        viewer.connect_animation(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, playing, time| window.on_animation(playing, time)
        ));
        viewer.connect_ready(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| {
                let imp = window.imp();
                imp.spinner.set_visible(false);
                window.set_model_actions_enabled(true);
                if let Some(opened) = imp.opened.take() {
                    glib::g_debug!(
                        "turntable",
                        "Shown {:.0} ms after opening",
                        opened.elapsed().as_secs_f64() * 1000.0
                    );
                }
            }
        ));
        // The GL context went, and the model with it
        viewer.connect_lost(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| {
                if let Some(file) = window.imp().file.borrow().clone() {
                    window.open_file(&file);
                }
            }
        ));

        self.set_up_actions();
        self.set_up_drop();
        self.set_up_animation_controls();

        let style = adw::StyleManager::default();
        for property in ["dark", "accent-color-rgba"] {
            style.connect_notify_local(
                Some(property),
                glib::clone!(
                    #[weak(rename_to = window)]
                    self,
                    move |_, _| window.send_theme()
                ),
            );
        }
        self.send_theme();
        settings.connect_changed(
            None,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, key| window.apply_setting(key)
            ),
        );
        for key in VIEW_SETTINGS {
            self.apply_setting(key);
        }

        self.set_default_size(settings.int("window-width"), settings.int("window-height"));
        if settings.boolean("window-maximized") {
            self.maximize();
        }
        self.set_model_actions_enabled(false);
    }

    fn set_up_actions(&self) {
        let settings = self.settings();
        let action = |name: &str, f: fn(&Self)| {
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| f(&window)
            ));
            self.add_action(&action);
        };
        action("open", |w| w.choose_file());
        action("close", |w| w.close());
        action("reset-view", |w| w.imp().viewer.reset_view());
        action("copy-image", |w| w.copy_image());
        action("save-image", |w| w.save_image());
        action("show-in-files", |w| w.show_in_files());
        action("toggle-playback", |w| w.toggle_playback());
        action("toggle-spin", |w| {
            let settings = w.settings();
            let _ = settings.set_boolean("auto-rotate", !settings.boolean("auto-rotate"));
        });
        for key in VIEW_SETTINGS {
            self.add_action(&settings.create_action(key));
        }

        let split_view = self.imp().split_view.get();
        let properties = gio::SimpleAction::new_stateful("properties", None, &false.to_variant());
        properties.connect_change_state(glib::clone!(
            #[weak]
            split_view,
            move |_, value| {
                if let Some(show) = value.and_then(|v| v.get::<bool>()) {
                    split_view.set_show_sidebar(show);
                }
            }
        ));
        split_view.connect_show_sidebar_notify(glib::clone!(
            #[weak]
            properties,
            move |split_view| properties.set_state(&split_view.shows_sidebar().to_variant())
        ));
        self.add_action(&properties);
    }

    fn set_model_actions_enabled(&self, enabled: bool) {
        for name in MODEL_ACTIONS {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(enabled);
            }
        }
    }

    /// While a drag is over the window, a drop target covers the content, highlighted.
    fn set_up_drop(&self) {
        let drop_area = self.imp().drop_area.get();
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        target.connect_drop(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                window.imp().drop_area.set_can_target(false);
                let Some(file) = value.get::<gdk::FileList>().ok().and_then(|list| list.files().into_iter().next())
                else {
                    return false;
                };
                window.open_file(&file);
                true
            }
        ));
        drop_area.add_controller(target);

        let motion = gtk::DropControllerMotion::new();
        motion.connect_enter(glib::clone!(
            #[weak]
            drop_area,
            move |_, _, _| drop_area.set_can_target(true)
        ));
        motion.connect_leave(glib::clone!(
            #[weak]
            drop_area,
            move |_| drop_area.set_can_target(false)
        ));
        self.add_controller(motion);
    }

    fn send_theme(&self) {
        let style = adw::StyleManager::default();
        let accent = style.accent_color_rgba();
        let accent = [accent.red(), accent.green(), accent.blue()].map(srgb_to_linear);
        self.imp().viewer.set_theme(style.is_dark(), accent);
    }

    fn apply_setting(&self, key: &str) {
        let settings = self.settings();
        let viewer = &self.imp().viewer;
        match key {
            "auto-rotate" => viewer.set_spin(settings.boolean(key)),
            "lighting" => viewer.set_lighting(Lighting::from_name(&settings.string(key))),
            "display-mode" => viewer.set_display(Display::from_name(&settings.string(key))),
            "show-grid" => viewer.set_grid(settings.boolean(key)),
            "show-axes" => viewer.set_axes(settings.boolean(key)),
            "show-frame-rate" => self.show_frame_rate(settings.boolean(key)),
            _ => {}
        }
    }

    /// Frames drawn a second, measured every half second. The view draws only while something
    /// moves, so a still model reads as idle.
    fn show_frame_rate(&self, show: bool) {
        let imp = self.imp();
        imp.frame_rate.set_visible(show);
        if let Some(timer) = imp.frame_rate_timer.take() {
            timer.remove();
        }
        if !show {
            return;
        }
        imp.frame_rate.set_label(&gettext("Idle"));
        let mut last = (std::time::Instant::now(), imp.viewer.frames_drawn());
        let timer = glib::timeout_add_local(
            std::time::Duration::from_millis(500),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    let now = (std::time::Instant::now(), window.imp().viewer.frames_drawn());
                    let frames = now.1 - last.1;
                    let rate = frames as f64 / now.0.duration_since(last.0).as_secs_f64();
                    last = now;
                    let label = if frames == 0 {
                        gettext("Idle")
                    } else {
                        // Translators: frames drawn a second, as a number
                        fill(&gettext("%s fps"), &[&format_count(rate.round() as u64)])
                    };
                    window.imp().frame_rate.set_label(&label);
                    glib::ControlFlow::Continue
                }
            ),
        );
        imp.frame_rate_timer.replace(Some(timer));
    }

    fn save_size(&self) {
        let settings = self.settings();
        let maximized = self.is_maximized();
        let _ = settings.set_boolean("window-maximized", maximized);
        if !maximized {
            let (width, height) = self.default_size();
            let _ = settings.set_int("window-width", width);
            let _ = settings.set_int("window-height", height);
        }
    }

    fn choose_file(&self) {
        let models = gtk::FileFilter::new();
        models.set_name(Some(&gettext("3D Models")));
        for format in FORMATS {
            models.add_suffix(format.extension);
            for mime_type in format.mime_types {
                models.add_mime_type(mime_type);
            }
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&models);
        let dialog =
            gtk::FileDialog::builder().title(gettext("Open Model")).filters(&filters).default_filter(&models).build();
        if let Some(folder) = self.imp().file.borrow().as_ref().and_then(|f| f.parent()) {
            dialog.set_initial_folder(Some(&folder));
        }
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                match dialog.open_future(Some(&window)).await {
                    Ok(file) => window.open_file(&file),
                    Err(e) if !e.matches(gtk::DialogError::Dismissed) => crate::warn!("{e}"),
                    Err(_) => {}
                }
            }
        ));
    }

    /// Shows `file`; what's shown before it goes once it's ready, or fails.
    pub fn open_file(&self, file: &gio::File) {
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[strong]
            file,
            async move { window.open(file).await }
        ));
    }

    async fn open(&self, file: gio::File) {
        let imp = self.imp();
        let loading = imp.loading.get() + 1;
        imp.loading.set(loading);
        imp.opened.set(Some(std::time::Instant::now()));
        let name = file.basename().map(|b| b.display().to_string()).unwrap_or_default();
        imp.file.replace(Some(file.clone()));
        self.set_title_text(&name);
        self.set_model_actions_enabled(false);
        imp.controls.set_visible(false);
        imp.animation_controls.set_visible(false);
        let Some(format) = format_of(&name) else {
            imp.spinner.set_visible(false);
            self.show_error(&gettext("3D Viewer can’t open this kind of file."));
            return;
        };

        imp.stack.set_visible_child_name("viewer");
        imp.spinner.set_visible(true);
        // Read and measured on another thread, while the file's details come in here
        let path = file.path();
        let loader = format.loader;
        let loaded = gio::spawn_blocking(move || match path {
            Some(path) => model::load(&path, loader),
            None => Err(model::Error::Unreadable("not a local file".to_owned())),
        });
        let info = file
            .query_info_future(
                "standard::display-name,standard::size,time::modified",
                gio::FileQueryInfoFlags::NONE,
                glib::Priority::DEFAULT,
            )
            .await;
        let loaded = loaded.await;
        if loading != imp.loading.get() {
            return;
        }

        let loaded = match loaded {
            Ok(loaded) => loaded,
            Err(_) => Err(model::Error::Unsupported("the loader stopped".to_owned())),
        };
        let (info, loaded) = match (info, loaded) {
            (Ok(info), Ok(loaded)) => (info, loaded),
            (Err(e), _) if e.matches(gio::IOErrorEnum::NotFound) => {
                self.show_error(&gettext("The file doesn’t exist any more."));
                return;
            }
            (_, Err(model::Error::NotFound)) => {
                self.show_error(&gettext("The file doesn’t exist any more."));
                return;
            }
            (Err(e), _) => {
                crate::warn!("Could not open {}: {e}", file.uri());
                self.show_error(&gettext("The file couldn’t be read."));
                return;
            }
            (_, Err(model::Error::Unreadable(e))) => {
                crate::warn!("Could not open {}: {e}", file.uri());
                self.show_error(&gettext("The file couldn’t be read."));
                return;
            }
            (_, Err(model::Error::Outside(e))) => {
                crate::warn!("Could not show {}: {e} is outside its folder", file.uri());
                self.show_error(&gettext("The model refers to files outside its folder."));
                return;
            }
            (_, Err(model::Error::Missing(e))) => {
                crate::warn!("Could not show {}: {e} is missing", file.uri());
                self.show_error(&gettext("Some files the model needs are missing."));
                return;
            }
            (_, Err(model::Error::Empty)) => {
                self.show_error(&gettext("The model is empty."));
                return;
            }
            (_, Err(model::Error::Unsupported(e))) => {
                crate::warn!("Could not show {}: {e}", file.uri());
                self.show_error(&gettext("The file may be damaged, or use something 3D Viewer doesn’t support."));
                return;
            }
        };

        self.set_title_text(&info.display_name());
        self.show_properties(&file, &info, format, &loaded.stats);
        self.show_animations(loaded.stats.clips.clone());
        // The spinner stays until the model's on the GPU and drawn (see set_up)
        imp.viewer.show(loaded);
        imp.controls.set_visible(true);
        imp.viewer.grab_focus();
    }

    fn set_title_text(&self, name: &str) {
        self.imp().window_title.set_title(name);
        self.set_title(Some(name));
    }

    /// The sidebar stays open from one model to the next, as in Image Viewer; only an error
    /// closes it.
    fn show_error(&self, message: &str) {
        let imp = self.imp();
        imp.spinner.set_visible(false);
        imp.viewer.clear();
        imp.split_view.set_show_sidebar(false);
        imp.error_page.set_description(Some(message));
        imp.stack.set_visible_child_name("error");
    }

    fn show_properties(&self, file: &gio::File, info: &gio::FileInfo, format: &Format, stats: &Stats) {
        let imp = self.imp();
        let set = |row: &adw::ActionRow, value: Option<String>| {
            row.set_visible(value.is_some());
            row.set_subtitle(&value.unwrap_or_default());
        };
        let count = |n: u64| (n > 0).then(|| format_count(n));
        set(&imp.dimensions_row, Some(format_dimensions(stats.size)));
        set(&imp.vertices_row, count(stats.vertices));
        set(&imp.triangles_row, count(stats.triangles));
        set(&imp.points_row, count(stats.points));
        set(&imp.meshes_row, count(stats.meshes));
        set(&imp.materials_row, count(stats.materials));
        set(&imp.animations_row, count(stats.clips.len() as u64));
        set(&imp.format_row, Some(format.name()));
        set(&imp.size_row, Some(glib::format_size(info.size().max(0) as u64).to_string()));
        let folder = file.parent().map(|f| match f.path() {
            Some(path) => glib::filename_display_basename(path).to_string(),
            None => f.uri().to_string(),
        });
        set(&imp.folder_row, folder);
        set(&imp.modified_row, info.modification_date_time().map(|d| format_modified(d.to_unix())));
    }

    // Animation controls: shown when the model has clips, a drop-down when it has more than one.

    fn set_up_animation_controls(&self) {
        let imp = self.imp();
        // Clip names are the file's own and can be long: the button shows the start of one
        let label_factory = |ellipsize: bool| {
            let factory = gtk::SignalListItemFactory::new();
            factory.connect_setup(move |_, item| {
                let label = gtk::Label::builder().xalign(0.0).build();
                if ellipsize {
                    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    label.set_max_width_chars(12);
                }
                item.downcast_ref::<gtk::ListItem>().unwrap().set_child(Some(&label));
            });
            factory.connect_bind(|_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                let string = item.item().and_downcast::<gtk::StringObject>().map(|s| s.string()).unwrap_or_default();
                item.child().and_downcast::<gtk::Label>().unwrap().set_label(&string);
            });
            factory
        };
        imp.clip_dropdown.set_factory(Some(&label_factory(true)));
        imp.clip_dropdown.set_list_factory(Some(&label_factory(false)));

        imp.animation_scale.connect_change_value(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, _, value| {
                window.imp().viewer.seek(value);
                glib::Propagation::Proceed
            }
        ));
        imp.clip_dropdown.connect_selected_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |dropdown| {
                let index = dropdown.selected() as usize;
                let Some(duration) = window.imp().clips.borrow().get(index).map(|c| c.1) else { return };
                window.imp().animation_scale.adjustment().set_upper(duration as f64);
                window.imp().viewer.select_clip(index);
            }
        ));
    }

    fn show_animations(&self, clips: Vec<(String, f32)>) {
        let imp = self.imp();
        imp.animation_controls.set_visible(!clips.is_empty());
        imp.clip_dropdown.set_visible(clips.len() > 1);
        let first = clips.first().map(|c| c.1);
        let names: Vec<&str> = clips.iter().map(|c| c.0.as_str()).collect();
        let model = gtk::StringList::new(&names);
        imp.clips.replace(clips);
        let Some(duration) = first else { return };
        imp.clip_dropdown.set_model(Some(&model));
        imp.clip_dropdown.set_selected(0);
        imp.animation_scale.adjustment().set_upper(duration as f64);
        self.on_animation(true, 0.0);
    }

    fn on_animation(&self, playing: bool, time: f64) {
        let imp = self.imp();
        if imp.playing.replace(playing) != playing || imp.play_button.icon_name().is_none() {
            imp.play_button.set_icon_name(if playing {
                "media-playback-pause-symbolic"
            } else {
                "media-playback-start-symbolic"
            });
            imp.play_button.set_tooltip_text(Some(&if playing { gettext("Pause") } else { gettext("Play") }));
        }
        imp.animation_scale.set_value(time);
    }

    fn toggle_playback(&self) {
        if !self.imp().clips.borrow().is_empty() {
            self.imp().viewer.set_playing(!self.imp().playing.get());
        }
    }

    // Pictures and the file manager

    fn copy_image(&self) {
        let Some(texture) = self.imp().viewer.snapshot() else { return };
        self.clipboard().set_texture(&texture);
        self.imp().toast_overlay.add_toast(adw::Toast::new(&gettext("Image copied")));
    }

    fn save_image(&self) {
        let name = self
            .imp()
            .file
            .borrow()
            .as_ref()
            .and_then(|f| f.basename())
            .and_then(|b| b.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let dialog =
            gtk::FileDialog::builder().title(gettext("Save Image")).initial_name(format!("{name}.png")).build();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                let file = match dialog.save_future(Some(&window)).await {
                    Ok(file) => file,
                    Err(e) if !e.matches(gtk::DialogError::Dismissed) => return crate::warn!("{e}"),
                    Err(_) => return,
                };
                let saved = match window.imp().viewer.snapshot() {
                    Some(texture) => file
                        .replace_contents_future(
                            texture.save_to_png_bytes(),
                            None,
                            false,
                            gio::FileCreateFlags::REPLACE_DESTINATION,
                        )
                        .await
                        .map(|_| ())
                        .map_err(|(_, e)| e.to_string()),
                    None => Err("nothing to save".to_owned()),
                };
                if let Err(e) = saved {
                    crate::warn!("Could not save the image: {e}");
                    window.imp().toast_overlay.add_toast(adw::Toast::new(&gettext("Could not save the image")));
                }
            }
        ));
    }

    fn show_in_files(&self) {
        let Some(file) = self.imp().file.borrow().clone() else { return };
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                if let Err(e) = gtk::FileLauncher::new(Some(&file)).open_containing_folder_future(Some(&window)).await {
                    if !e.matches(gtk::DialogError::Dismissed) {
                        crate::warn!("{e}");
                    }
                }
            }
        ));
    }
}
