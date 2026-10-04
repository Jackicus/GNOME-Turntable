// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

// A window shows one model: the renderer's view (viewer.js) under a header bar, its controls
// floating over it, and its properties in a sidebar. The view settings (lighting, display
// mode, grid, rotation) are the app's GSettings, so every window follows the same ones.

import Adw from 'gi://Adw?version=1';
import Gdk from 'gi://Gdk?version=4.0';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Gio from 'gi://Gio';
import Gtk from 'gi://Gtk?version=4.0';
import Pango from 'gi://Pango';
import {gettext as _} from 'gettext';

import {FORMATS, formatCount, formatDimensions, formatModified, formatOf} from './formats.js';
import {Viewer} from './viewer.js';

Gio._promisify(Gtk.FileDialog.prototype, 'open', 'open_finish');
Gio._promisify(Gtk.FileDialog.prototype, 'save', 'save_finish');
Gio._promisify(Gtk.FileLauncher.prototype, 'open_containing_folder',
    'open_containing_folder_finish');
Gio._promisify(Gio.File.prototype, 'query_info_async');
Gio._promisify(Gio.File.prototype, 'replace_contents_bytes_async', 'replace_contents_finish');

const VIEW_SETTINGS = ['auto-rotate', 'lighting', 'display-mode', 'show-grid'];

export const Window = GObject.registerClass({
    GTypeName: 'TurntableWindow',
    Template: 'resource:///io/github/jackicus/Turntable/window.ui',
    InternalChildren: [
        'toast_overlay', 'window_title', 'split_view', 'stack', 'viewer_overlay', 'spinner',
        'controls', 'animation_controls', 'play_button', 'animation_scale', 'clip_dropdown',
        'error_page', 'drop_area', 'dimensions_row', 'vertices_row', 'triangles_row',
        'points_row', 'meshes_row', 'materials_row', 'animations_row', 'format_row',
        'size_row', 'folder_row', 'modified_row',
    ],
}, class Window extends Adw.ApplicationWindow {
    constructor(application) {
        super({application});
        this._settings = application.settings;
        this._file = null;
        this._loading = 0;
        this._clips = [];

        this._viewer = new Viewer({devtools: application.devtools});
        this._viewer.view.update_property([Gtk.AccessibleProperty.LABEL], [_('Model')]);
        this._viewer_overlay.set_child(this._viewer.view);
        this._viewer.connect('action', (_viewer, name) => this.lookup_action(name)?.activate(null));
        this._viewer.connect('animation', (_viewer, playing, time) => this._onAnimation(playing, time));
        this._viewer.connect('crashed', () => {
            if (this._file)
                this.openFile(this._file).catch(logError);
        });

        this._setUpActions();
        this._setUpDrop();
        this._setUpAnimationControls();
        this._followStyle();
        for (const key of VIEW_SETTINGS) {
            this._settings.connect(`changed::${key}`, () => this._applySetting(key));
            this._applySetting(key);
        }

        this.set_default_size(this._settings.get_int('window-width'),
            this._settings.get_int('window-height'));
        if (this._settings.get_boolean('window-maximized'))
            this.maximize();
        this.connect('close-request', () => {
            this._saveSize();
            this._viewer.destroy();
            return false;
        });
        this._setModelActionsEnabled(false);
    }

    get isEmpty() {
        return this._file === null;
    }

    _setUpActions() {
        const actions = [
            ['open', () => this._chooseFile()],
            ['close', () => this.close()],
            ['reset-view', () => this._viewer.send('resetView')],
            ['copy-image', () => this._copyImage()],
            ['save-image', () => this._saveImage()],
            ['show-in-files', () => this._showInFiles()],
            ['toggle-playback', () => this._togglePlayback()],
            ['toggle-spin', () => {
                this._settings.set_boolean('auto-rotate', !this._settings.get_boolean('auto-rotate'));
            }],
        ];
        for (const [name, callback] of actions) {
            const action = new Gio.SimpleAction({name});
            action.connect('activate', callback);
            this.add_action(action);
        }
        for (const key of VIEW_SETTINGS)
            this.add_action(this._settings.create_action(key));

        const properties = new Gio.SimpleAction({
            name: 'properties', state: new GLib.Variant('b', false),
        });
        properties.connect('change-state', (action, value) => {
            this._split_view.show_sidebar = value.get_boolean();
        });
        this._split_view.connect('notify::show-sidebar', () => {
            properties.set_state(new GLib.Variant('b', this._split_view.show_sidebar));
        });
        this.add_action(properties);
    }

    _setModelActionsEnabled(enabled) {
        for (const name of ['reset-view', 'copy-image', 'save-image', 'show-in-files', 'properties'])
            this.lookup_action(name).enabled = enabled;
        if (!enabled)
            this._split_view.show_sidebar = false;
    }

    // While a drag is over the window, a drop target covers the content: WebKit would
    // otherwise take the drop and try to show the file as a page.
    _setUpDrop() {
        const target = Gtk.DropTarget.new(Gdk.FileList.$gtype, Gdk.DragAction.COPY);
        target.connect('drop', (_target, value) => {
            this._drop_area.can_target = false;
            const [file] = value.get_files();
            if (!file)
                return false;
            this.openFile(file).catch(logError);
            return true;
        });
        this._drop_area.add_controller(target);

        const motion = new Gtk.DropControllerMotion();
        motion.connect('enter', () => (this._drop_area.can_target = true));
        motion.connect('leave', () => (this._drop_area.can_target = false));
        this.add_controller(motion);
    }

    _followStyle() {
        const style = Adw.StyleManager.get_default();
        const send = () => {
            const accent = style.get_accent_color_rgba();
            const hex = [accent.red, accent.green, accent.blue]
                .map(c => Math.round(c * 255).toString(16).padStart(2, '0')).join('');
            this._viewer.send('theme', {dark: style.dark, accent: `#${hex}`});
        };
        style.connect('notify::dark', send);
        style.connect('notify::accent-color-rgba', send);
        send();
    }

    _applySetting(key) {
        switch (key) {
        case 'auto-rotate':
            this._viewer.send('spin', {enabled: this._settings.get_boolean(key)});
            break;
        case 'lighting':
            this._viewer.send('lighting', {name: this._settings.get_string(key)});
            break;
        case 'display-mode':
            this._viewer.send('display', {mode: this._settings.get_string(key)});
            break;
        case 'show-grid':
            this._viewer.send('grid', {visible: this._settings.get_boolean(key)});
            break;
        }
    }

    _saveSize() {
        const maximized = this.is_maximized();
        this._settings.set_boolean('window-maximized', maximized);
        if (!maximized) {
            const [width, height] = this.get_default_size();
            this._settings.set_int('window-width', width);
            this._settings.set_int('window-height', height);
        }
    }

    async _chooseFile() {
        const models = new Gtk.FileFilter({name: _('3D Models')});
        for (const [extension, format] of Object.entries(FORMATS)) {
            models.add_suffix(extension);
            format.mimeTypes.forEach(type => models.add_mime_type(type));
        }
        const filters = new Gio.ListStore({item_type: Gtk.FileFilter});
        filters.append(models);
        const dialog = new Gtk.FileDialog({
            title: _('Open Model'), filters, default_filter: models,
            initial_folder: this._file?.get_parent() ?? null,
        });
        let file;
        try {
            file = await dialog.open(this, null);
        } catch (e) {
            if (!e.matches(Gtk.DialogError, Gtk.DialogError.DISMISSED))
                logError(e);
            return;
        }
        this.openFile(file).catch(logError);
    }

    async openFile(file) {
        const loading = ++this._loading;
        const name = file.get_basename();
        this._file = file;
        this._setTitle(name);
        this._setModelActionsEnabled(false);
        this._controls.visible = false;
        this._animation_controls.visible = false;
        const format = formatOf(name);
        if (!format) {
            this._showError(_('3D Viewer can’t open this kind of file.'));
            return;
        }

        this._stack.visible_child_name = 'viewer';
        this._spinner.visible = true;
        let info, result;
        try {
            [info, result] = await Promise.all([
                file.query_info_async('standard::display-name,standard::size,time::modified',
                    Gio.FileQueryInfoFlags.NONE, GLib.PRIORITY_DEFAULT, null),
                this._viewer.open(file, format.loader),
            ]);
        } catch (e) {
            if (loading !== this._loading)
                return;
            this._spinner.visible = false;
            if (e.matches?.(Gio.IOErrorEnum, Gio.IOErrorEnum.NOT_FOUND))
                this._showError(_('The file doesn’t exist any more.'));
            else
                this._showError(_('The file couldn’t be read.'));
            console.warn(`Could not open ${file.get_uri()}: ${e.message}`);
            return;
        }
        if (loading !== this._loading)
            return;
        this._spinner.visible = false;
        if (result?.error) {
            console.warn(`Could not show ${file.get_uri()}: ${result.detail}`);
            this._showError(result.error === 'empty'
                ? _('The model is empty.')
                : _('The file may be damaged, or use something 3D Viewer doesn’t support.'));
            return;
        }

        this._setTitle(info.get_display_name());
        this._showProperties(file, info, format, result);
        this._showAnimations(result.animations);
        this._controls.visible = true;
        this._setModelActionsEnabled(true);
        this._viewer.view.grab_focus();
    }

    _setTitle(name) {
        this._window_title.title = name;
        this.title = name;
    }

    _showError(message) {
        this._viewer.close().catch(logError);
        this._error_page.description = message;
        this._stack.visible_child_name = 'error';
    }

    _showProperties(file, info, format, stats) {
        const set = (row, value) => {
            row.visible = Boolean(value);
            row.subtitle = value || '';
        };
        set(this._dimensions_row, formatDimensions(stats.size));
        set(this._vertices_row, stats.vertices && formatCount(stats.vertices));
        set(this._triangles_row, stats.triangles && formatCount(stats.triangles));
        set(this._points_row, stats.points && formatCount(stats.points));
        set(this._meshes_row, stats.meshes && formatCount(stats.meshes));
        set(this._materials_row, stats.materials && formatCount(stats.materials));
        set(this._animations_row, stats.animations.length && formatCount(stats.animations.length));
        set(this._format_row, format.name);
        set(this._size_row, GLib.format_size(info.get_size()));
        const folder = file.get_parent();
        set(this._folder_row, folder ? GLib.filename_display_basename(folder.get_path() ?? folder.get_uri()) : '');
        const modified = info.get_modification_date_time();
        set(this._modified_row, modified && formatModified(modified.to_unix()));
    }

    // Animation controls: shown when the model has clips, a drop-down when it has more than one.

    _setUpAnimationControls() {
        // Clip names are the file's own and can be long: the button shows the start of one
        const factory = new Gtk.SignalListItemFactory();
        factory.connect('setup', (_factory, item) => {
            item.child = new Gtk.Label({
                ellipsize: Pango.EllipsizeMode.END, max_width_chars: 12, xalign: 0,
            });
        });
        factory.connect('bind', (_factory, item) => (item.child.label = item.item.string));
        this._clip_dropdown.factory = factory;
        const listFactory = new Gtk.SignalListItemFactory();
        listFactory.connect('setup', (_factory, item) => (item.child = new Gtk.Label({xalign: 0})));
        listFactory.connect('bind', (_factory, item) => (item.child.label = item.item.string));
        this._clip_dropdown.list_factory = listFactory;

        this._animation_scale.connect('change-value', (_scale, _scroll, value) => {
            this._viewer.send('seek', {time: value});
            return false;
        });
        this._clip_dropdown.connect('notify::selected', () => {
            const index = this._clip_dropdown.selected;
            const clip = this._clips[index];
            if (!clip)
                return;
            this._animation_scale.adjustment.upper = clip.duration;
            this._viewer.send('clip', {index});
        });
    }

    _showAnimations(clips) {
        this._clips = clips;
        this._animation_controls.visible = clips.length > 0;
        this._clip_dropdown.visible = clips.length > 1;
        if (!clips.length)
            return;
        this._clip_dropdown.model = Gtk.StringList.new(clips.map(c => c.name));
        this._clip_dropdown.selected = 0;
        this._animation_scale.adjustment.upper = clips[0].duration;
        this._onAnimation(true, 0);
    }

    _onAnimation(playing, time) {
        this._playing = playing;
        this._play_button.icon_name = playing
            ? 'media-playback-pause-symbolic' : 'media-playback-start-symbolic';
        this._play_button.tooltip_text = playing ? _('Pause') : _('Play');
        this._animation_scale.set_value(time);
    }

    _togglePlayback() {
        if (this._clips.length)
            this._viewer.send(this._playing ? 'pause' : 'play');
    }

    // Pictures and the file manager

    async _copyImage() {
        const texture = await this._viewer.snapshot();
        this.get_clipboard().set_content(
            Gdk.ContentProvider.new_for_bytes('image/png', texture.save_to_png_bytes()));
        this._toast_overlay.add_toast(new Adw.Toast({title: _('Image copied')}));
    }

    async _saveImage() {
        const name = this._file.get_basename().replace(/\.[^.]+$/, '');
        const dialog = new Gtk.FileDialog({
            title: _('Save Image'),
            initial_name: `${name}.png`,
        });
        let file;
        try {
            file = await dialog.save(this, null);
        } catch (e) {
            if (!e.matches(Gtk.DialogError, Gtk.DialogError.DISMISSED))
                logError(e);
            return;
        }
        try {
            const texture = await this._viewer.snapshot();
            await file.replace_contents_bytes_async(texture.save_to_png_bytes(), null, false,
                Gio.FileCreateFlags.REPLACE_DESTINATION, null);
        } catch (e) {
            logError(e);
            this._toast_overlay.add_toast(new Adw.Toast({title: _('Could not save the image')}));
        }
    }

    async _showInFiles() {
        try {
            await new Gtk.FileLauncher({file: this._file}).open_containing_folder(this, null);
        } catch (e) {
            if (!e.matches(Gtk.DialogError, Gtk.DialogError.DISMISSED))
                logError(e);
        }
    }
});
