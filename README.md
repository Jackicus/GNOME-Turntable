<div align="center">

<img src="data/icons/io.github.jackicus.Turntable.svg" width="128" alt="">

# 3D Viewer

**Look at 3D models from every side.**

Open a model, turn it around, see it in good light. A simple, native 3D viewer for GNOME.

<img src="data/screenshots/chair.png" alt="3D Viewer showing an orange chair in studio light">

</div>

## What it does

- **Opens the common formats:** glTF, GLB, OBJ, STL, PLY, FBX and 3MF. Open one from Files,
  drag it into the window, or press **Open**.
- **Moves smoothly:** it draws at your display's full refresh rate, 240 Hz included, and only
  while something moves. **Show Frame Rate** in the main menu shows it.
- **Lights it nicely:** Studio, Soft, Sunlight or Dramatic lighting, with a soft shadow.
- **Shows how it's built:** switch to wireframe, or draw it over the shaded model, and add
  a floor grid and axes.
- **Plays its animations:** play, pause, scrub, and pick between them.
- **Spins on its own,** slowly, like a turntable.
- **Tells you about it:** size, vertices, triangles, meshes, materials and file details.
- **Takes a picture:** copy or save exactly what you see, transparent background included.

It fits right in: light and dark styles, your accent colour, and windows down to phone size.

<table>
  <tr>
    <td><img src="data/screenshots/fox.png" alt="An animated fox with the properties sidebar open"></td>
    <td><img src="data/screenshots/lantern.png" alt="A lantern drawn as a wireframe over its shaded surface, on a grid with axes"></td>
  </tr>
</table>

## Moving around

| To | Mouse or touchpad | Touchscreen | Keyboard |
|---|---|---|---|
| Turn | Drag | Drag with one finger | Arrow keys |
| Move | Right-drag, or Shift-drag | Drag with two fingers | |
| Zoom | Scroll | Pinch | <kbd>+</kbd> <kbd>−</kbd> |
| Start over | | | <kbd>Ctrl</kbd> <kbd>0</kbd> |
| Spin on its own | | | <kbd>R</kbd> |
| Play or pause | | | <kbd>Space</kbd> |
| Properties | | | <kbd>F9</kbd> |

<kbd>Ctrl</kbd> <kbd>?</kbd> shows every shortcut.

## Getting it

It isn't on Flathub yet. To build and install it yourself, you need Rust (with Cargo),
GTK 4.18, libadwaita 1.8 and Meson, which any recent GNOME desktop already has or can install:

```sh
git clone https://github.com/Jackicus/GNOME-Turntable.git
cd GNOME-Turntable
meson setup build --prefix=/usr
sudo meson install -C build
```

Then look for **3D Viewer** in your apps. Want to try it without installing? `scripts/run.sh`
builds and runs it from the source folder.

## For developers

Rust, GTK 4 and libadwaita for the window, and a renderer of its own: OpenGL in a GTK GL area,
drawing on the window's frame clock, so it keeps time with the display. Its shading is ported
from [three.js](https://threejs.org), which drew the model in a WebKit view before 0.2.0.
Models are read on another thread and their textures go to the GPU a few at a time, so the
window never stalls; nothing a model refers to is read from outside its own folder.

`scripts/check.sh` builds and tests it, `scripts/run.sh` runs it (with
`G_MESSAGES_DEBUG=turntable` it logs its frame rate), and [CLAUDE.md](CLAUDE.md) explains the
layout. The codename **Turntable** is used for the app ID (`io.github.jackicus.Turntable`), the
binary and this repository.

## Credits

- [three.js](https://threejs.org), MIT licence: the renderer's shading and camera controls are
  ported from it
- [ufbx](https://github.com/ufbx/ufbx), MIT licence, reads FBX and OBJ
- [meshoptimizer](https://github.com/zeux/meshoptimizer), MIT licence, and
  [Draco](https://google.github.io/draco/), Apache licence 2.0, decode compressed glTF
- Models in the screenshots, from the
  [Khronos glTF Sample Assets](https://github.com/KhronosGroup/glTF-Sample-Assets):
  *Sheen Chair* by Eric Chadwick (CC0); *Fox* by PixelMannen (CC0), rigged and animated by
  tomkranis (CC BY 4.0), converted to glTF by AsoboStudio and scurest (CC BY 4.0);
  *Lantern* by sbtron and Frank Galligan (CC0)

## Licence

GPL-2.0-or-later
