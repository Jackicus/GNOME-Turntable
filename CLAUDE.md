# 3D Viewer (Turntable)

A native GNOME 3D model viewer: GJS (ES modules), GTK 4, libadwaita, Blueprint, Meson, gettext;
three.js renders in a WebKitGTK 6 view. GPL-2.0-or-later. App ID `io.github.jackicus.Turntable`,
resource base `/io/github/jackicus/Turntable`, binary `turntable`. Shown everywhere as
"3D Viewer" (like Loupe → "Image Viewer"); "Turntable" is only the codename. It should feel like
a GNOME core app: minimal, nothing a viewer doesn't need, the HIG followed.

## Layout

```
src/turntable.in        launcher: registers the gresource, imports js/main.js
src/main.js             gettext, runs the Application
src/application.js      Adw.Application: open/activate, app actions, accelerators, About
src/window.js + .blp    a window: header bar, the view, floating controls, properties sidebar
src/viewer.js           the WebKit view (a canvas only) and call()/send() into the renderer
src/scheme.js           turntable:// — the renderer from the gresource, the model's folder
src/formats.js          formats by extension, property formatting (no GTK: tested)
src/renderer/           index.html + renderer.js (the three.js scene), three/ (vendored)
src/shortcuts-dialog.blp  Adw.ShortcutsDialog, loaded by AdwApplication (app.shortcuts)
data/                   desktop file, metainfo, gschema, icons, MIME types for PLY and FBX
tests/run.js            unit tests (gjs -m tests/run.js); tests/models/ small invented models
scripts/                run.sh, check.sh, headless.sh, screenshot.js, update-three.sh
```

A new JS module goes in `src/turntable.gresource.xml` (aliased under `js/`) and, if it has
strings, `po/POTFILES.in`. Every vendored three.js file is listed in the gresource
(tests/run.js checks).

## Conventions

- The window ↔ renderer boundary: `viewer.call(name, args)` (awaits the result) or
  `viewer.send()` (fire and forget, failures logged) run `turntable.command()` in the page;
  the page answers with `{type: 'ready'|'action'|'animation'|'log'}` messages. Add a command in
  renderer.js's `commands`, never by evaluating ad-hoc JS.
- The renderer draws on demand (`invalidate()`), never in a free-running loop.
- The view settings are GSettings keys exposed as `win.*` actions (`settings.create_action`).
- libadwaita widgets and style classes before custom CSS; CSS only in `src/style.css`.
- Strings through `gettext` (`_()`, `_("…")` in Blueprint); `fill()` for `%s`, not `.format()`.
- Every source file starts with the two SPDX lines. 4-space indents in JS.

## Checking a change

1. `scripts/check.sh` prints `check: ok`.
2. Anything visible: `scripts/headless.sh scripts/screenshot.js build/x.png MODEL`, looked at,
   also with `--light`, `--size 360x640`, `--properties`. The headless session runs on default
   settings (blue accent), so shots look the same everywhere. Models outside tests/models
   (Khronos samples) stay out of the repo.
3. `data/screenshots/` (README and metainfo, 1000×700) use CC0/CC-BY Khronos samples credited
   in README.md; retake them when the window's look changes. Never use a model with a
   non-commercial licence (DamagedHelmet).
