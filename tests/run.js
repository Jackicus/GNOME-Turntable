// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

// The unit tests: the modules without a display (formats.js, scheme.js), with plain asserts.
//   gjs -m tests/run.js      (meson test -C build runs it too)

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import System from 'system';

const root = GLib.path_get_dirname(GLib.path_get_dirname(System.programPath));
const {FORMATS, extensionOf, fill, formatDimensions, formatOf} = await import(`file://${root}/src/formats.js`);
const {contentTypeOf, resolveModelPath} = await import(`file://${root}/src/scheme.js`);

let failures = 0;
function test(name, body) {
    try {
        body();
        print(`ok      ${name}`);
    } catch (e) {
        failures++;
        print(`FAILED  ${name}\n        ${e.message}`);
    }
}
function equal(actual, expected) {
    if (actual !== expected)
        throw new Error(`expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
}

test('formats by extension, any case', () => {
    equal(formatOf('Helmet.GLB').loader, 'gltf');
    equal(formatOf('part.3mf').loader, '3mf');
    equal(formatOf('notes.txt'), null);
    equal(formatOf('.obj'), null);
    equal(extensionOf('archive.tar.gz'), 'gz');
});

test('every format has a loader the renderer knows', () => {
    const loaders = new Set(['gltf', 'obj', 'stl', 'ply', 'fbx', '3mf']);
    for (const format of Object.values(FORMATS))
        equal(loaders.has(format.loader), true);
});

test('the desktop file offers the formats with MIME types', () => {
    const [, bytes] = Gio.File.new_for_path(`${root}/data/io.github.jackicus.Turntable.desktop.in`)
        .load_contents(null);
    const line = new TextDecoder().decode(bytes).match(/^MimeType=(.*)$/m)[1];
    const offered = line.split(';').filter(Boolean).sort().join(';');
    const expected = Object.values(FORMATS).flatMap(f => f.mimeTypes).sort().join(';');
    equal(offered, expected);
});

test('fill and dimensions', () => {
    equal(fill('%s of %s', 'one', 'two'), 'one of two');
    equal(formatDimensions([1, 2.5, 0.123456]).replace(/\s/g, ' '), '1 × 2.5 × 0.123');
});

const folder = Gio.File.new_for_path('/models/robot');
const lookup = token => (token === 'abc' ? folder : undefined);

test('model paths resolve inside the model folder', () => {
    equal(resolveModelPath('/model/abc/robot.gltf', lookup).get_path(), '/models/robot/robot.gltf');
    equal(resolveModelPath('/model/abc/textures/skin.png', lookup).get_path(),
        '/models/robot/textures/skin.png');
});

test('model paths never leave the model folder', () => {
    equal(resolveModelPath('/model/abc/../secret', lookup), null);
    equal(resolveModelPath('/model/abc/a/../../secret', lookup), null);
    equal(resolveModelPath('/model/abc/./robot.gltf', lookup), null);
    equal(resolveModelPath('/model/abc/', lookup), null);
    equal(resolveModelPath('/model/abc', lookup), null);
    equal(resolveModelPath('/model/abc//etc/passwd', lookup), null);
    equal(resolveModelPath('/model/abc/..\\secret', lookup), null);
    equal(resolveModelPath('/model/unknown/robot.gltf', lookup), null);
    equal(resolveModelPath('/three/robot.gltf', lookup), null);
});

test('renderer files get the types WebKit needs', () => {
    equal(contentTypeOf('/renderer.js'), 'text/javascript');
    equal(contentTypeOf('/three/addons/libs/draco/gltf/draco_decoder.wasm'), 'application/wasm');
    equal(contentTypeOf('/index.html'), 'text/html');
});

test('the gresource lists every vendored three.js file', () => {
    const [, bytes] = Gio.File.new_for_path(`${root}/src/turntable.gresource.xml`).load_contents(null);
    const xml = new TextDecoder().decode(bytes);
    const missing = [];
    const walk = dir => {
        const children = dir.enumerate_children('standard::name,standard::type', 0, null);
        for (let info; (info = children.next_file(null));) {
            const child = dir.get_child(info.get_name());
            if (info.get_file_type() === Gio.FileType.DIRECTORY)
                walk(child);
            else if (!['LICENSE', 'VERSION'].includes(info.get_name()))
                missing.push(child);
        }
    };
    walk(Gio.File.new_for_path(`${root}/src/renderer/three`));
    const src = Gio.File.new_for_path(`${root}/src`);
    const unlisted = missing.map(f => src.get_relative_path(f)).filter(p => !xml.includes(`>${p}<`));
    equal(unlisted.join(', '), '');
});

print(failures ? `\n${failures} failed` : '\nall passed');
System.exit(failures ? 1 : 0);
