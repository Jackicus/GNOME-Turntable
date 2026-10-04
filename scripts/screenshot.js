#!/usr/bin/env -S gjs -m
// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

// A PNG of a window of the installed build (scripts/run.sh builds it), with a model open or
// not. Run it on the headless display, with settings in memory:
//   scripts/headless.sh scripts/screenshot.js build/shot.png [MODEL] [--light] [--size WxH]
//       [--properties] [--set KEY=VALUE]… [--wait SECONDS]

import Adw from 'gi://Adw?version=1';
import Gdk from 'gi://Gdk?version=4.0';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import Graphene from 'gi://Graphene';
import Gtk from 'gi://Gtk?version=4.0';
import System from 'system';

const root = GLib.path_get_dirname(GLib.path_get_dirname(System.programPath));
const install = `${root}/build/install`;
GLib.setenv('GSETTINGS_SCHEMA_DIR', `${install}/share/glib-2.0/schemas`, true);
GLib.setenv('GSETTINGS_BACKEND', 'memory', true);
GLib.setenv('XDG_DATA_DIRS', `${install}/share:${GLib.getenv('XDG_DATA_DIRS') ?? '/usr/share'}`, true);

const args = [...ARGV];
const option = name => {
    const i = args.indexOf(name);
    return i < 0 ? null : args.splice(i, 2)[1];
};
const flag = name => {
    const i = args.indexOf(name);
    return i < 0 ? false : Boolean(args.splice(i, 1));
};
const sets = [];
for (let value; (value = option('--set'));)
    sets.push(value.split('='));
const light = flag('--light');
const properties = flag('--properties');
const [width, height] = (option('--size') ?? '1000x720').split('x').map(Number);
const wait = Number(option('--wait') ?? 2);
const [out, model] = args;
if (!out) {
    printerr('usage: screenshot.js OUT.png [MODEL] [--light] [--size WxH] [--properties] [--set KEY=VALUE] [--wait S]');
    System.exit(2);
}

Gio.resources_register(Gio.Resource.load(`${install}/share/turntable/turntable.gresource`));
const {Application} = await import('resource:///io/github/jackicus/Turntable/js/application.js');
const app = new Application({version: 'screenshot'});
app.settings.set_int('window-width', width);
app.settings.set_int('window-height', height);
for (const [key, value] of sets) {
    const type = app.settings.get_value(key).get_type_string();
    app.settings.set_value(key, type === 's' ? new GLib.Variant('s', value)
        : GLib.Variant.parse(new GLib.VariantType(type), value, null, null));
}
app.connect('startup', () => {
    // XDG_DATA_DIRS is read before this script can set it: give GTK the app's icons directly
    Gtk.IconTheme.get_for_display(Gdk.Display.get_default()).add_search_path(`${install}/share/icons`);
    Adw.StyleManager.get_default().color_scheme = light
        ? Adw.ColorScheme.FORCE_LIGHT : Adw.ColorScheme.FORCE_DARK;
});

function capture(window) {
    const w = window.get_width(), h = window.get_height();
    const snapshot = new Gtk.Snapshot();
    new Gtk.WidgetPaintable({widget: window}).snapshot(snapshot, w, h);
    const node = snapshot.to_node();
    const rect = new Graphene.Rect().init(0, 0, w, h);
    window.get_renderer().render_texture(node, rect).save_to_png(out);
    print(`screenshot: ${out} (${w}×${h})`);
}

app.connect('window-added', (_app, window) => {
    const started = Date.now();
    GLib.timeout_add(GLib.PRIORITY_DEFAULT, 100, () => {
        const settled = !model || (!window._spinner.visible && window._file !== null &&
            Date.now() - started > 500);
        if (!settled && Date.now() - started < 30000)
            return GLib.SOURCE_CONTINUE;
        if (properties)
            window._split_view.show_sidebar = true;
        // Copying the view's texture while WebKit draws the next frame can hang GTK's renderer
        // here, so nothing may move when it's taken.
        if (window._playing)
            window._viewer.send('pause');
        window._viewer.send('spin', {enabled: false});
        GLib.timeout_add(GLib.PRIORITY_DEFAULT, wait * 1000, () => {
            capture(window);
            app.quit();
            return GLib.SOURCE_REMOVE;
        });
        return GLib.SOURCE_REMOVE;
    });
});
System.exit(await app.runAsync([System.programInvocationName, ...(model ? [model] : [])]));
