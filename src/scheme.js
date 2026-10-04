// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

// The turntable: URI scheme the renderer is served on. turntable://app/… is the renderer
// itself, from the gresource; turntable://app/model/<token>/<path> is a model file, or a file
// beside it (a glTF's .bin, an OBJ's .mtl, textures), and never anything outside the model's
// folder. Nothing in the WebView reaches the network or the rest of the disk.

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

export const ORIGIN = 'turntable://app';
const RENDERER = '/io/github/jackicus/Turntable/renderer';

const CONTENT_TYPES = {
    html: 'text/html',
    js: 'text/javascript',
    css: 'text/css',
    wasm: 'application/wasm',
};

// token → the folder of a model being shown
const folders = new Map();

// Serves `file` and the files beside it under a new URL; returns the model's URL and a function
// that stops serving them.
export function share(file) {
    const token = GLib.uuid_string_random();
    folders.set(token, file.get_parent());
    const name = encodeURIComponent(file.get_basename());
    return {url: `${ORIGIN}/model/${token}/${name}`, revoke: () => folders.delete(token)};
}

// The file a decoded /model/… path names, or null if it would leave the model's folder.
export function resolveModelPath(path, lookup = token => folders.get(token)) {
    const parts = path.split('/');
    // ['', 'model', token, ...relative]
    if (parts.length < 4 || parts[1] !== 'model')
        return null;
    const folder = lookup(parts[2]);
    const relative = parts.slice(3);
    if (!folder || relative.some(p => p === '' || p === '.' || p === '..' || p.includes('\\')))
        return null;
    const file = folder.resolve_relative_path(relative.join('/'));
    return file.has_prefix(folder) ? file : null;
}

export function contentTypeOf(path) {
    const ext = path.slice(path.lastIndexOf('.') + 1);
    return CONTENT_TYPES[ext] ?? 'application/octet-stream';
}

function notFound(request) {
    request.finish_error(new GLib.Error(Gio.IOErrorEnum, Gio.IOErrorEnum.NOT_FOUND,
        `Not found: ${request.get_uri()}`));
}

// The WebKit.URISchemeRequest handler.
export function handleRequest(request) {
    let path;
    try {
        path = GLib.Uri.parse(request.get_uri(), GLib.UriFlags.NONE).get_path();
    } catch {
        notFound(request);
        return;
    }
    if (path.startsWith('/model/')) {
        serveModelFile(request, path);
        return;
    }
    try {
        const bytes = Gio.resources_lookup_data(RENDERER + path, Gio.ResourceLookupFlags.NONE);
        const stream = Gio.MemoryInputStream.new_from_bytes(bytes);
        request.finish(stream, bytes.get_size(), contentTypeOf(path));
    } catch {
        notFound(request);
    }
}

async function serveModelFile(request, path) {
    const file = resolveModelPath(path);
    if (!file) {
        notFound(request);
        return;
    }
    try {
        const info = await file.query_info_async('standard::size',
            Gio.FileQueryInfoFlags.NONE, GLib.PRIORITY_DEFAULT, null);
        const stream = await file.read_async(GLib.PRIORITY_DEFAULT, null);
        request.finish(stream, info.get_size(), 'application/octet-stream');
    } catch {
        notFound(request);
    }
}

Gio._promisify(Gio.File.prototype, 'query_info_async');
Gio._promisify(Gio.File.prototype, 'read_async');
