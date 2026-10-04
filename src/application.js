// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

import Adw from 'gi://Adw?version=1';
import GObject from 'gi://GObject';
import Gio from 'gi://Gio';
import Gtk from 'gi://Gtk?version=4.0';
import {gettext as _} from 'gettext';

import {Window} from './window.js';

export const APP_ID = 'io.github.jackicus.Turntable';

const ACCELS = {
    'app.quit': ['<Control>q'],
    'app.new-window': ['<Control>n'],
    'win.open': ['<Control>o'],
    'win.close': ['<Control>w'],
    'win.copy-image': ['<Control>c'],
    'win.save-image': ['<Control>s'],
    'win.reset-view': ['<Control>0'],
    'win.properties': ['F9'],
};

export const Application = GObject.registerClass(
class Application extends Adw.Application {
    constructor(config) {
        super({
            application_id: APP_ID,
            flags: Gio.ApplicationFlags.HANDLES_OPEN,
            resource_base_path: '/io/github/jackicus/Turntable',
        });
        this.version = config.version;
        this.devtools = Boolean(config.devtools);
        this.settings = new Gio.Settings({schema_id: APP_ID});

        const actions = [
            ['new-window', () => new Window(this).present()],
            ['about', () => this._showAbout()],
            ['quit', () => this.get_windows().forEach(w => w.close())],
        ];
        for (const [name, callback] of actions) {
            const action = new Gio.SimpleAction({name});
            action.connect('activate', callback);
            this.add_action(action);
        }
        for (const [action, accels] of Object.entries(ACCELS))
            this.set_accels_for_action(action, accels);
    }

    vfunc_activate() {
        (this.active_window ?? new Window(this)).present();
    }

    // Files from the file manager or the command line: the first goes to the active window if
    // it shows nothing yet, the rest to windows of their own.
    vfunc_open(files) {
        for (const file of files) {
            const active = this.active_window;
            const window = active?.isEmpty ? active : new Window(this);
            window.openFile(file).catch(logError);
            window.present();
        }
    }

    _showAbout() {
        const about = Adw.AboutDialog.new_from_appdata(
            '/io/github/jackicus/Turntable/metainfo.xml', this.version);
        about.version = this.version;
        about.developers = ['Jack Tully'];
        about.copyright = '© 2026 Jack Tully';
        // Translators: credit yourself here, one name per line
        about.translator_credits = _('translator-credits');
        about.add_legal_section('three.js', '© 2010–2026 three.js authors',
            Gtk.License.MIT_X11, null);
        about.present(this.active_window);
    }
});
