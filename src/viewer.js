// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

// The WebKit view the renderer (renderer/renderer.js) draws in, and the calls into it. The
// view is only a canvas: no menus, no navigation, no network, no disk but the model's folder.
// The renderer is loaded when the window is made, so opening a model doesn't wait for it.

import Gdk from 'gi://Gdk?version=4.0';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Gio from 'gi://Gio';
import WebKit from 'gi://WebKit?version=6.0';

import * as Scheme from './scheme.js';

Gio._promisify(WebKit.WebView.prototype, 'call_async_javascript_function');

let context = null;
let session = null;

function webContext() {
    if (!context) {
        context = new WebKit.WebContext();
        context.set_cache_model(WebKit.CacheModel.DOCUMENT_VIEWER);
        context.register_uri_scheme('turntable', Scheme.handleRequest);
        const security = context.get_security_manager();
        security.register_uri_scheme_as_secure('turntable');
        security.register_uri_scheme_as_cors_enabled('turntable');
        session = WebKit.NetworkSession.new_ephemeral();
    }
    return context;
}

export const Viewer = GObject.registerClass({
    Signals: {
        // A key the renderer leaves to the window: 'toggle-spin' or 'toggle-playback'.
        'action': {param_types: [GObject.TYPE_STRING]},
        // The animation's state, while it plays and whenever it changes.
        'animation': {param_types: [GObject.TYPE_BOOLEAN, GObject.TYPE_DOUBLE]},
        // The web process died: the view is blank until the next open().
        'crashed': {},
    },
}, class Viewer extends GObject.Object {
    constructor(params = {}) {
        super();
        const contentManager = new WebKit.UserContentManager();
        contentManager.register_script_message_handler('turntable', null);
        contentManager.connect('script-message-received::turntable', (_manager, value) => {
            this._onMessage(JSON.parse(value.to_json(0)));
        });
        this.view = new WebKit.WebView({
            web_context: webContext(),
            network_session: session,
            user_content_manager: contentManager,
            settings: new WebKit.Settings({
                enable_developer_extras: Boolean(params.devtools),
                enable_back_forward_navigation_gestures: false,
                enable_webgl: true,
                hardware_acceleration_policy: WebKit.HardwareAccelerationPolicy.ALWAYS,
            }),
            hexpand: true,
            vexpand: true,
        });
        this.view.set_background_color(new Gdk.RGBA({red: 0, green: 0, blue: 0, alpha: 0}));
        this.view.connect('context-menu', () => !params.devtools);
        this.view.connect('decide-policy', (_view, decision, type) => {
            if (type === WebKit.PolicyDecisionType.RESPONSE)
                return false;
            const uri = decision.get_navigation_action().get_request().get_uri();
            if (uri.startsWith(`${Scheme.ORIGIN}/`))
                return false;
            decision.ignore();
            return true;
        });
        this.view.connect('web-process-terminated', (_view, reason) => {
            console.warn(`The renderer stopped (${reason}); reloading it`);
            this._ready = this._load();
            this.emit('crashed');
        });
        this._shared = null;
        this._ready = this._load();
    }

    // Resolves when the renderer says it's ready: WebKit's load-changed comes before the
    // page's modules have run.
    _load() {
        return new Promise(resolve => {
            this._onReady = resolve;
            this.view.load_uri(`${Scheme.ORIGIN}/index.html`);
        });
    }

    _onMessage(message) {
        switch (message.type) {
        case 'ready':
            this._onReady?.();
            this._onReady = null;
            break;
        case 'action':
            this.emit('action', message.name);
            break;
        case 'animation':
            this.emit('animation', message.playing, message.time);
            break;
        case 'log':
            console.warn(`renderer: ${message.text}`);
            break;
        }
    }

    // Calls turntable.command(name, args) in the page; resolves to what it returns.
    async call(name, args = {}) {
        await this._ready;
        const value = await this.view.call_async_javascript_function(
            'return turntable.command(name, JSON.parse(args));', -1,
            new GLib.Variant('a{sv}', {
                name: new GLib.Variant('s', name),
                args: new GLib.Variant('s', JSON.stringify(args)),
            }), null, null, null);
        return value.is_undefined() || value.is_null() ? null : JSON.parse(value.to_json(0));
    }

    // call() for the commands nothing waits on: a failure is only logged.
    send(name, args = {}) {
        this.call(name, args).catch(e => console.warn(`renderer: ${name} failed: ${e.message}`));
    }

    // Shows `file`, whose format is `loader` (formats.js). Resolves to what the renderer
    // measured, or to {error: 'empty'|'failed', detail} when it shows nothing.
    async open(file, loader) {
        this._shared?.revoke();
        this._shared = Scheme.share(file);
        return this.call('load', {url: this._shared.url, format: loader});
    }

    async close() {
        this._shared?.revoke();
        this._shared = null;
        await this.call('clear');
    }

    // What the view shows, without the window around it, as a Gdk.Texture.
    async snapshot() {
        const dataUrl = await this.call('snapshot');
        const base64 = dataUrl.slice(dataUrl.indexOf(',') + 1);
        return Gdk.Texture.new_from_bytes(new GLib.Bytes(GLib.base64_decode(base64)));
    }

    destroy() {
        this._shared?.revoke();
        this._shared = null;
    }
});
