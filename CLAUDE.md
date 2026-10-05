# 3D Viewer (Turntable)

A native GNOME 3D model viewer: Rust, GTK 4 (gtk4-rs), libadwaita, Blueprint, Meson + Cargo,
gettext; its own OpenGL renderer (glow, through libepoxy) in a GtkGLArea. GPL-2.0-or-later. App ID
`io.github.jackicus.Turntable`, resource base `/io/github/jackicus/Turntable`, binary `turntable`.
Shown everywhere as "3D Viewer" (like Loupe → "Image Viewer"); "Turntable" is only the codename.
It should feel like a GNOME core app: minimal, nothing a viewer doesn't need, the HIG followed.
It must draw at the display's refresh rate (240 Hz on the owner's NVIDIA GTX 1080).

## Layout

```
src/main.rs             the binary: GSK_RENDERER, gettext, the gresource, runs Application
src/lib.rs              the crate (the screenshot example uses it too), warn!()
src/config.rs           APP_ID, paths (Meson sets them through env at build time)
src/application.rs      AdwApplication: open/activate, app actions, accelerators, About
src/window.rs + .blp    a window: header bar, the viewer, floating controls, properties sidebar
src/viewer.rs           TurntableViewer, a GLArea: input, camera, animation, frame-clock ticks
src/camera.rs           orbit camera, three.js's OrbitControls in Rust (no GTK: tested)
src/formats.rs          formats by extension, property formatting (no GTK: tested)
src/model/              the CPU-side model: mod.rs types, bounds, stats; a loader per format
                        (gltf.rs with meshopt and Draco, fbx.rs for FBX and OBJ through ufbx,
                        stl, ply, threemf);
                        files.rs keeps referenced files inside the model's folder
src/render/             mod.rs the passes; gpu.rs uploads; programs.rs GL and shaders;
                        environment.rs lighting presets; shaders/*.glsl
src/shortcuts-dialog.blp  Adw.ShortcutsDialog, loaded by AdwApplication (app.shortcuts)
data/                   desktop file, metainfo, gschema, icons, MIME types for PLY and FBX
tests/                  cargo integration tests; tests/models/ small invented models
examples/screenshot.rs  a PNG of a window, for scripts/screenshot.sh
scripts/                run.sh, check.sh, headless.sh, screenshot.sh
build-aux/cargo.sh      Meson's cargo build step
build-aux/flatpak/      the manifest, and cargo-sources.json: regenerate it whenever Cargo.lock
                        changes (flatpak-cargo-generator.py Cargo.lock -o …; tests check)
```

A module with translatable strings goes in `po/POTFILES.in` (tests/project.rs checks).

## Conventions

- Draw on demand: `Viewer::invalidate()` queues a render and keeps a tick callback going while
  something moves (coasting, spin, animation); never a free-running loop. Time comes from the
  frame clock, and damping is per second, not per frame.
- main.rs sets `GSK_RENDERER=ngl` unless the user set it: GTK's Vulkan renderer costs a few ms
  per GLArea frame on NVIDIA (150–230 fps instead of 240). Keep it unless that's measured gone.
- Loading happens on a thread (`gio::spawn_blocking`); GL only on the main thread, in the
  GLArea's context. A loader returns a `Model`; `render()` uploads its geometry, then its
  textures ~10 ms a frame (each image freed once uploaded); the model is drawn only when it's
  all there, and the viewer's `ready` signal takes the window's spinner away.
- Loaders never trust a size from the file before checking it against the data (a 2 GiB
  budget for glTF accessors, counts against bytes); tests/models.rs fuzzes them.
- Shaders: GLSL that compiles as `#version 330 core` and `#version 300 es` (GTK may pick
  either). Every shader writes premultiplied colour through `finalColor()` (Neutral tone
  mapping, sRGB). Material maps are slots (`model::Map`), units 0–12, uniforms `uMapN`.
- The view settings are GSettings keys exposed as `win.*` actions (`settings.create_action`).
- libadwaita widgets and style classes before custom CSS; CSS only in `src/style.css`.
- Strings through gettext (`gettext("…")`, `N_()` for constants; `_("…")` in Blueprint);
  `formats::fill()` for `%s`.
- Every source file starts with the two SPDX lines (tests/project.rs checks). rustfmt.toml:
  120 columns. Warnings go through `crate::warn!`; frame rates through `g_debug!`.

## Checking a change

1. `scripts/check.sh` prints `check: ok` (build, rustfmt, clippy -D warnings, cargo tests,
   desktop/metainfo/schema validation). Cargo is in `~/.cargo/bin`; the scripts add it to PATH.
2. Anything visible: `scripts/headless.sh scripts/screenshot.sh build/x.png MODEL`, looked at,
   also with `--light`, `--size 360x640`, `--properties`, `--time S` (animations), `--view`
   (the Copy Image picture). The headless session runs on default settings (blue accent).
3. Frame rate: `G_MESSAGES_DEBUG=turntable` logs frames a second while moving. Headless:
   `mutter --virtual-monitor 1920x1200@240`; GTK's GL renderer reaches 240.0 there.
4. `data/screenshots/` (README and metainfo, 1000×700) use CC0/CC-BY Khronos samples credited
   in README.md; retake them when the window's look changes. Never use a model with a
   non-commercial licence (DamagedHelmet). Models outside tests/models stay out of the repo.
