// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

// The model formats the renderer opens, and how the properties sidebar writes what it measured.
// GLib and gettext only, so the tests run it without a display.

import GLib from 'gi://GLib';
import {gettext as _} from 'gettext';

// By file extension: the renderer's loader, the name shown, and the MIME types the file
// chooser and the desktop file offer (PLY's and FBX's are the app's own, data/*.mime.xml).
export const FORMATS = {
    glb: {loader: 'gltf', name: _('glTF Binary'), mimeTypes: ['model/gltf-binary']},
    gltf: {loader: 'gltf', name: _('glTF'), mimeTypes: ['model/gltf+json']},
    obj: {loader: 'obj', name: _('Wavefront OBJ'), mimeTypes: ['model/obj']},
    stl: {loader: 'stl', name: _('STL'), mimeTypes: ['model/stl']},
    ply: {loader: 'ply', name: _('PLY'), mimeTypes: ['model/x-ply']},
    fbx: {loader: 'fbx', name: _('FBX'), mimeTypes: ['model/x-fbx']},
    '3mf': {loader: '3mf', name: _('3MF'), mimeTypes: ['model/3mf']},
};

export function extensionOf(name) {
    const dot = name.lastIndexOf('.');
    return dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
}

// The format of a file name, or null when the renderer can't open it.
export function formatOf(name) {
    return FORMATS[extensionOf(name)] ?? null;
}

// Fills each %s in a translated string with the next argument.
export function fill(template, ...values) {
    return template.replace(/%s/g, () => values.shift());
}

export function formatCount(n) {
    return n.toLocaleString();
}

// Width × height × depth in the model's own units, which a file rarely names.
export function formatDimensions([x, y, z]) {
    const fmt = v => v.toLocaleString(undefined, {maximumSignificantDigits: 3});
    // Translators: a model's width × height × depth, in the units it was made in
    return fill(_('%s × %s × %s'), fmt(x), fmt(y), fmt(z));
}

export function formatModified(unixSeconds) {
    const date = GLib.DateTime.new_from_unix_local(unixSeconds);
    // Translators: when a file was last changed, in strftime(3) format
    return date?.format(_('%-d %B %Y, %H:%M')) ?? '';
}
