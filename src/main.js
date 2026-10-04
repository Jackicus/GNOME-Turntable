// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

import Gettext from 'gettext';

import {Application} from './application.js';

export async function main(argv, config) {
    Gettext.bindtextdomain('turntable', config.localedir);
    Gettext.bind_textdomain_codeset?.('turntable', 'UTF-8');
    Gettext.textdomain('turntable');
    return new Application(config).runAsync(argv);
}
