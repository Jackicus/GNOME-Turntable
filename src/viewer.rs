// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

//! The view the model is drawn in: a GL area, the renderer inside it, and the camera the
//! pointer, touch and keys move. It draws only when something changes; while something moves
//! (a coasting flick, the spin, an animation) it draws on every tick of the window's frame
//! clock, which keeps time with the display, at whatever rate it refreshes.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use glam::Mat4;
use gtk::glib::subclass::Signal;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib};

use crate::camera::{KEY_STEP, OrbitCamera};
use crate::model::{Loaded, Model, Pose};
use crate::render::{Camera, Display, Frame, Lighting, Renderer, Settings};

/// How long frames go on after the last movement, so a slowing coast ends rather than stops.
const SETTLE: i64 = 250_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    Rotate,
    Pan,
    Dolly,
}

struct Shown {
    model: Model,
    radius: f32,
    /// Width, height and depth
    size: glam::Vec3,
    /// Waiting to go to the GPU, or there and freed here
    uploaded: bool,
    /// All of it on the GPU, textures included: drawn from now on
    ready: bool,
    rest: Pose,
    pose: Pose,
    worlds: Vec<Mat4>,
    /// Changes with the pose, so the renderer knows when to catch up
    version: u64,
    clip: Option<Playback>,
}

struct Playback {
    index: usize,
    duration: f32,
    time: f32,
    playing: bool,
}

#[derive(Default)]
struct Stats {
    since: Option<Instant>,
    frames: u32,
    drawing: Duration,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Viewer {
        pub(super) renderer: RefCell<Option<Renderer>>,
        pub(super) shown: RefCell<Option<Shown>>,
        /// The model went with the GL context: clear() before the next one
        pub(super) cleared: Cell<bool>,
        pub(super) camera: RefCell<OrbitCamera>,
        pub(super) settings: Cell<Settings>,
        /// The GL area's size in device pixels
        pub(super) size: Cell<(i32, i32)>,
        pub(super) tick: RefCell<Option<gtk::TickCallbackId>>,
        pub(super) last_frame: Cell<Option<i64>>,
        pub(super) last_change: Cell<i64>,
        pub(super) drag: Cell<Option<(Drag, f64, f64)>>,
        pub(super) pinch: Cell<Option<(f64, f64, f64)>>,
        pub(super) stats: RefCell<Stats>,
        /// Every frame drawn since the viewer was made
        pub(super) frames: Cell<u64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Viewer {
        const NAME: &'static str = "TurntableViewer";
        type Type = super::Viewer;
        type ParentType = gtk::GLArea;
    }

    impl ObjectImpl for Viewer {
        fn signals() -> &'static [Signal] {
            static SIGNALS: std::sync::OnceLock<Vec<Signal>> = std::sync::OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    // The animation's state: playing, and the time in it
                    Signal::builder("animation").param_types([bool::static_type(), f64::static_type()]).build(),
                    // The GL context was lost and the model with it: show it again
                    Signal::builder("lost").build(),
                    // The model shown is on the GPU and drawn
                    Signal::builder("ready").build(),
                ]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_auto_render(false);
            obj.set_has_depth_buffer(false);
            obj.set_focusable(true);
            obj.set_hexpand(true);
            obj.set_vexpand(true);
            obj.set_allowed_apis(gdk::GLAPI::GL | gdk::GLAPI::GLES);
            obj.set_up_input();
        }
    }

    impl WidgetImpl for Viewer {
        fn realize(&self) {
            self.parent_realize();
            let obj = self.obj();
            obj.make_current();
            if let Some(error) = obj.error() {
                crate::warn!("No OpenGL: {error}");
                return;
            }
            match crate::render::load_gl().and_then(Renderer::new) {
                Ok(renderer) => *self.renderer.borrow_mut() = Some(renderer),
                Err(error) => {
                    crate::warn!("The renderer couldn’t start: {error}");
                    obj.set_error(Some(&glib::Error::new(gdk::GLError::NotAvailable, &error)));
                    return;
                }
            }
            // A model shown before, whose data went to the context now gone
            if self.shown.borrow().as_ref().is_some_and(|s| s.uploaded) {
                self.shown.replace(None);
                obj.emit_by_name::<()>("lost", &[]);
            }
        }

        fn unrealize(&self) {
            let obj = self.obj();
            obj.make_current();
            if let Some(renderer) = self.renderer.take() {
                renderer.destroy();
            }
            self.parent_unrealize();
        }
    }

    impl GLAreaImpl for Viewer {
        fn resize(&self, width: i32, height: i32) {
            self.size.set((width, height));
            let mut camera = self.camera.borrow_mut();
            camera.aspect = width as f32 / height.max(1) as f32;
        }

        fn render(&self, _context: &gdk::GLContext) -> glib::Propagation {
            let started = Instant::now();
            let output = unsafe {
                use glow::HasContext;
                let renderer = self.renderer.borrow();
                let Some(renderer) = renderer.as_ref() else { return glib::Propagation::Stop };
                let id = renderer.gl().get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING);
                std::num::NonZeroU32::new(id as u32).map(glow::NativeFramebuffer)
            };
            let (width, height) = self.size.get();
            self.draw(|renderer, camera, settings, frame| {
                renderer.render(output, width, height, camera, settings, frame)
            });
            self.frames.set(self.frames.get() + 1);
            let mut stats = self.stats.borrow_mut();
            stats.frames += 1;
            stats.drawing += started.elapsed();
            glib::Propagation::Stop
        }
    }

    impl Viewer {
        /// Runs `f` with the renderer and the frame as it stands, the model uploaded first if
        /// it's waiting.
        pub(super) fn draw<R>(
            &self,
            f: impl FnOnce(&mut Renderer, &Camera, &Settings, Option<&Frame>) -> R,
        ) -> Option<R> {
            let mut renderer = self.renderer.borrow_mut();
            let renderer = renderer.as_mut()?;
            let mut shown = self.shown.borrow_mut();
            if self.cleared.take() {
                renderer.clear_model();
            }
            if let Some(shown) = shown.as_mut().filter(|s| !s.uploaded) {
                renderer.set_model(&shown.model, shown.radius, shown.size.y);
                shown.uploaded = true;
            }
            // Textures a few at a time, about 10 ms a frame, so the window keeps moving while a
            // big model's go up
            if let Some(shown) = shown.as_mut().filter(|s| s.uploaded && !s.ready) {
                if renderer.upload(&mut shown.model, Instant::now() + Duration::from_millis(10)) {
                    shown.ready = true;
                    shown.model.release_gpu_data();
                    // Said once this frame's done, not from inside it
                    let viewer = self.obj().downgrade();
                    glib::idle_add_local_once(move || {
                        if let Some(viewer) = viewer.upgrade() {
                            viewer.emit_by_name::<()>("ready", &[]);
                        }
                    });
                }
            }
            let camera = self.camera.borrow();
            let view = Camera { view: camera.view(), projection: camera.projection(), position: camera.position };
            let mut settings = self.settings.get();
            let width = self.obj().width().max(1);
            settings.scale = self.size.get().0 as f32 / width as f32;
            let frame = shown.as_ref().map(|s| Frame {
                model: &s.model,
                worlds: &s.worlds,
                weights: &s.pose.weights,
                version: s.version,
            });
            Some(f(renderer, &view, &settings, frame.as_ref()))
        }

        /// One tick of the frame clock: the camera and the animation move on.
        pub(super) fn tick(&self, clock: &gdk::FrameClock) -> glib::ControlFlow {
            let obj = self.obj();
            let now = clock.frame_time();
            let dt = self.last_frame.get().map_or(1.0 / 60.0, |last| ((now - last) as f32 / 1e6).clamp(0.0, 0.1));
            self.last_frame.set(Some(now));
            let before = {
                let c = self.camera.borrow();
                (c.position, c.target)
            };
            let mut moving = self.camera.borrow_mut().update(dt);
            // Something to draw: the camera moved at all, or the model's still going up
            let mut changed = {
                let c = self.camera.borrow();
                (c.position, c.target) != before
            };
            let mut report = None;
            if let Some(shown) = self.shown.borrow_mut().as_mut() {
                if let Some(clip) = shown.clip.as_mut().filter(|c| c.playing) {
                    clip.time = if clip.duration > 0.0 { (clip.time + dt) % clip.duration } else { 0.0 };
                    report = Some(clip.time);
                    shown.pose_at();
                    moving = true;
                }
                if !shown.ready {
                    moving = true;
                }
            }
            changed |= moving;
            if changed {
                obj.queue_render();
            }
            if let Some(time) = report {
                obj.emit_by_name::<()>("animation", &[&true, &(time as f64)]);
            }
            self.log_rate();

            if moving || self.camera.borrow().spin {
                self.last_change.set(now);
            }
            if now - self.last_change.get() < SETTLE {
                glib::ControlFlow::Continue
            } else {
                self.tick.replace(None);
                self.last_frame.set(None);
                glib::ControlFlow::Break
            }
        }

        /// With G_MESSAGES_DEBUG=turntable, how many frames a second it draws while moving.
        fn log_rate(&self) {
            let mut stats = self.stats.borrow_mut();
            let since = *stats.since.get_or_insert_with(Instant::now);
            let elapsed = since.elapsed();
            if elapsed >= Duration::from_secs(1) {
                if stats.frames > 0 {
                    glib::g_debug!(
                        "turntable",
                        "{:.1} frames a second, {:.2} ms each to draw",
                        stats.frames as f64 / elapsed.as_secs_f64(),
                        stats.drawing.as_secs_f64() * 1000.0 / stats.frames as f64
                    );
                }
                *stats = Stats { since: Some(Instant::now()), ..Stats::default() };
            }
        }
    }
}

glib::wrapper! {
    pub struct Viewer(ObjectSubclass<imp::Viewer>)
        @extends gtk::GLArea, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Shown {
    /// The pose at the clip's time, or at rest without a clip.
    fn pose_at(&mut self) {
        self.pose.clone_from(&self.rest);
        if let Some(clip) = &self.clip {
            self.model.clips[clip.index].apply(clip.time, &mut self.pose);
        }
        self.model.world_matrices(&self.pose, &mut self.worlds);
        self.version += 1;
    }
}

impl Default for Viewer {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl Viewer {
    /// Draws on the next frame, and goes on drawing while anything moves.
    fn invalidate(&self) {
        let imp = self.imp();
        self.queue_render();
        if imp.tick.borrow().is_none() {
            let id = self.add_tick_callback(|viewer, clock| viewer.imp().tick(clock));
            imp.tick.replace(Some(id));
        }
    }

    fn has_model(&self) -> bool {
        self.imp().shown.borrow().is_some()
    }

    /// Shows `loaded`, in place of anything shown before, from the start; its first
    /// animation plays.
    pub fn show(&self, loaded: Loaded) {
        let imp = self.imp();
        let Loaded { model, radius, stats, .. } = loaded;
        let size = glam::Vec3::from(stats.size);
        let rest = model.rest_pose();
        let clip = model.clips.first().map(|c| Playback { index: 0, duration: c.duration, time: 0.0, playing: true });
        let mut shown = Shown {
            model,
            radius,
            size,
            uploaded: false,
            ready: false,
            pose: rest.clone(),
            rest,
            worlds: Vec::new(),
            version: 0,
            clip,
        };
        shown.pose_at();
        imp.shown.replace(Some(shown));
        imp.cleared.set(false);
        imp.camera.borrow_mut().reset(radius, size);
        self.report_animation();
        self.invalidate();
        if self.error().is_some() {
            self.emit_by_name::<()>("ready", &[]);
        }
    }

    pub fn connect_ready<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure("ready", false, glib::closure_local!(move |viewer: &Self| f(viewer)))
    }

    pub fn clear(&self) {
        let imp = self.imp();
        if imp.shown.take().is_some() {
            imp.cleared.set(true);
        }
        self.invalidate();
    }

    pub fn reset_view(&self) {
        if let Some(shown) = self.imp().shown.borrow().as_ref() {
            self.imp().camera.borrow_mut().reset(shown.radius, shown.size);
        }
        self.invalidate();
    }

    pub fn set_spin(&self, spin: bool) {
        self.imp().camera.borrow_mut().spin = spin;
        self.invalidate();
    }

    fn update_settings(&self, f: impl FnOnce(&mut Settings)) {
        let mut settings = self.imp().settings.get();
        f(&mut settings);
        self.imp().settings.set(settings);
        self.invalidate();
    }

    pub fn set_lighting(&self, lighting: Lighting) {
        self.update_settings(|s| s.lighting = lighting);
    }

    pub fn set_display(&self, display: Display) {
        self.update_settings(|s| s.display = display);
    }

    pub fn set_grid(&self, grid: bool) {
        self.update_settings(|s| s.grid = grid);
    }

    /// The style's darkness, and the accent colour as linear RGB.
    pub fn set_theme(&self, dark: bool, accent: [f32; 3]) {
        self.update_settings(|s| {
            s.dark = dark;
            s.accent = accent;
        });
    }

    // Animation

    /// How many frames it has drawn, ever: the frame rate is how fast this goes up.
    pub fn frames_drawn(&self) -> u64 {
        self.imp().frames.get()
    }

    pub fn is_playing(&self) -> bool {
        self.imp().shown.borrow().as_ref().and_then(|s| s.clip.as_ref()).is_some_and(|c| c.playing)
    }

    fn update_clip(&self, f: impl FnOnce(&mut Playback, &Model)) {
        if let Some(shown) = self.imp().shown.borrow_mut().as_mut() {
            let Some(clip) = shown.clip.as_mut() else { return };
            f(clip, &shown.model);
            shown.pose_at();
        }
        self.report_animation();
        self.invalidate();
    }

    pub fn set_playing(&self, playing: bool) {
        self.update_clip(|clip, _| clip.playing = playing);
    }

    pub fn seek(&self, time: f64) {
        self.update_clip(|clip, _| clip.time = (time as f32).clamp(0.0, clip.duration));
    }

    /// Plays clip `index` from its start (or holds it there, if paused).
    pub fn select_clip(&self, index: usize) {
        self.update_clip(|clip, model| {
            if let Some(c) = model.clips.get(index) {
                clip.index = index;
                clip.duration = c.duration;
                clip.time = 0.0;
            }
        });
    }

    fn report_animation(&self) {
        let state = self.imp().shown.borrow().as_ref().and_then(|s| s.clip.as_ref().map(|c| (c.playing, c.time)));
        if let Some((playing, time)) = state {
            self.emit_by_name::<()>("animation", &[&playing, &(time as f64)]);
        }
    }

    pub fn connect_animation<F: Fn(&Self, bool, f64) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "animation",
            false,
            glib::closure_local!(move |viewer: &Self, playing: bool, time: f64| f(viewer, playing, time)),
        )
    }

    pub fn connect_lost<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure("lost", false, glib::closure_local!(move |viewer: &Self| f(viewer)))
    }

    /// What the view shows, without the window around it, background see-through.
    pub fn snapshot(&self) -> Option<gdk::Texture> {
        self.make_current();
        if self.error().is_some() {
            return None;
        }
        let (width, height) = self.imp().size.get();
        if width <= 0 || height <= 0 {
            return None;
        }
        let pixels = self
            .imp()
            .draw(|renderer, camera, settings, frame| renderer.snapshot(width, height, camera, settings, frame))?;
        let bytes = glib::Bytes::from_owned(pixels);
        let texture = gdk::MemoryTexture::new(
            width,
            height,
            gdk::MemoryFormat::R8g8b8a8Premultiplied,
            &bytes,
            width as usize * 4,
        );
        Some(texture.upcast())
    }

    // Input

    fn set_up_input(&self) {
        let drag = gtk::GestureDrag::builder().button(0).build();
        drag.connect_drag_begin(glib::clone!(
            #[weak(rename_to = viewer)]
            self,
            move |gesture, _, _| {
                if !viewer.has_model() {
                    gesture.set_state(gtk::EventSequenceState::Denied);
                    return;
                }
                viewer.grab_focus();
                let shifted = gesture.current_event_state().intersects(
                    gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::META_MASK,
                );
                let mode = match gesture.current_button() {
                    2 => Drag::Dolly,
                    3 => Drag::Pan,
                    _ if shifted => Drag::Pan,
                    _ => Drag::Rotate,
                };
                viewer.imp().drag.set(Some((mode, 0.0, 0.0)));
                viewer.imp().camera.borrow_mut().dragging = true;
            }
        ));
        drag.connect_drag_update(glib::clone!(
            #[weak(rename_to = viewer)]
            self,
            move |_, x, y| {
                let imp = viewer.imp();
                let Some((mode, last_x, last_y)) = imp.drag.get() else { return };
                imp.drag.set(Some((mode, x, y)));
                let (dx, dy) = ((x - last_x) as f32, (y - last_y) as f32);
                let height = viewer.height().max(1) as f32;
                let mut camera = imp.camera.borrow_mut();
                match mode {
                    Drag::Rotate => camera.rotate(dx, dy, height),
                    Drag::Pan => camera.pan(dx, dy, height),
                    Drag::Dolly if dy != 0.0 => {
                        let scale = 0.95f32.powf(dy.abs() * 0.01);
                        camera.zoom(if dy > 0.0 { 1.0 / scale } else { scale });
                    }
                    Drag::Dolly => {}
                }
                drop(camera);
                viewer.invalidate();
            }
        ));
        let end = glib::clone!(
            #[weak(rename_to = viewer)]
            self,
            move || {
                viewer.imp().drag.set(None);
                viewer.imp().camera.borrow_mut().dragging = false;
                viewer.invalidate();
            }
        );
        let end_drag = end.clone();
        drag.connect_drag_end(move |_, _, _| end_drag());
        let end_cancel = end.clone();
        drag.connect_cancel(move |_, _| end_cancel());
        self.add_controller(drag);

        // Two fingers, or a touchpad pinch: zoom, and move with the fingers
        let zoom = gtk::GestureZoom::new();
        zoom.connect_begin(glib::clone!(
            #[weak(rename_to = viewer)]
            self,
            move |gesture, _| {
                if !viewer.has_model() {
                    gesture.set_state(gtk::EventSequenceState::Denied);
                    return;
                }
                gesture.set_state(gtk::EventSequenceState::Claimed);
                let (x, y) = gesture.bounding_box_center().unwrap_or_default();
                viewer.imp().pinch.set(Some((1.0, x, y)));
                viewer.imp().camera.borrow_mut().dragging = true;
            }
        ));
        zoom.connect_scale_changed(glib::clone!(
            #[weak(rename_to = viewer)]
            self,
            move |gesture, scale| {
                let imp = viewer.imp();
                let Some((last_scale, last_x, last_y)) = imp.pinch.get() else { return };
                let (x, y) = gesture.bounding_box_center().unwrap_or((last_x, last_y));
                imp.pinch.set(Some((scale, x, y)));
                let height = viewer.height().max(1) as f32;
                let mut camera = imp.camera.borrow_mut();
                if scale > 0.0 && last_scale > 0.0 {
                    camera.zoom((last_scale / scale) as f32);
                }
                camera.pan((x - last_x) as f32, (y - last_y) as f32, height);
                drop(camera);
                viewer.invalidate();
            }
        ));
        zoom.connect_end(glib::clone!(
            #[weak(rename_to = viewer)]
            self,
            move |_, _| {
                viewer.imp().pinch.set(None);
                viewer.imp().camera.borrow_mut().dragging = false;
                viewer.invalidate();
            }
        ));
        self.add_controller(zoom);

        // Scrolling zooms: a wheel's notch by 5%, a touchpad as far as it's moved
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.connect_scroll(glib::clone!(
            #[weak(rename_to = viewer)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |controller, _, dy| {
                if !viewer.has_model() || dy == 0.0 {
                    return glib::Propagation::Proceed;
                }
                let pixels = if controller.unit() == gdk::ScrollUnit::Surface { dy } else { dy * 100.0 };
                let scale = 0.95f32.powf((pixels.abs() * 0.01) as f32);
                viewer.imp().camera.borrow_mut().zoom(if dy > 0.0 { 1.0 / scale } else { scale });
                viewer.invalidate();
                glib::Propagation::Stop
            }
        ));
        self.add_controller(scroll);

        // Keys the view handles while it has the focus; the window's shortcuts (with Ctrl)
        // reach the window first. R and Space are the window's actions.
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(glib::clone!(
            #[weak(rename_to = viewer)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, state| {
                let modifiers =
                    gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK;
                if state.intersects(modifiers) || !viewer.has_model() {
                    return glib::Propagation::Proceed;
                }
                let mut camera = viewer.imp().camera.borrow_mut();
                match key {
                    gdk::Key::Left | gdk::Key::KP_Left => camera.orbit_now(KEY_STEP, 0.0),
                    gdk::Key::Right | gdk::Key::KP_Right => camera.orbit_now(-KEY_STEP, 0.0),
                    gdk::Key::Up | gdk::Key::KP_Up => camera.orbit_now(0.0, KEY_STEP),
                    gdk::Key::Down | gdk::Key::KP_Down => camera.orbit_now(0.0, -KEY_STEP),
                    gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add => camera.dolly_now(0.8),
                    gdk::Key::minus | gdk::Key::underscore | gdk::Key::KP_Subtract => camera.dolly_now(1.25),
                    gdk::Key::r | gdk::Key::R => {
                        drop(camera);
                        let _ = viewer.activate_action("win.toggle-spin", None);
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::space => {
                        drop(camera);
                        let _ = viewer.activate_action("win.toggle-playback", None);
                        return glib::Propagation::Stop;
                    }
                    _ => return glib::Propagation::Proceed,
                }
                drop(camera);
                viewer.invalidate();
                glib::Propagation::Stop
            }
        ));
        self.add_controller(keys);
    }
}
