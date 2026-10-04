# SPDX-License-Identifier: GPL-2.0-or-later
# SPDX-FileCopyrightText: 2026 Jack Tully

project(
  'turntable',
  version: '0.1.0',
  meson_version: '>= 1.2.0',
  license: 'GPL-2.0-or-later',
)

gnome = import('gnome')
i18n = import('i18n')

app_id = 'io.github.jackicus.Turntable'

gjs = dependency('gjs-1.0', version: '>= 1.80')  # Gio.Application.runAsync, top-level await
dependency('gtk4', version: '>= 4.18')
dependency('libadwaita-1', version: '>= 1.8')    # shortcuts-dialog.ui, AdwShortcutsDialog
dependency('webkitgtk-6.0', version: '>= 2.44')

pkgdatadir = get_option('prefix') / get_option('datadir') / meson.project_name()

subdir('data')
subdir('src')
subdir('po')

test('unit', find_program('gjs'),
  args: ['-m', meson.project_source_root() / 'tests' / 'run.js'],
  workdir: meson.project_source_root(),
)

gnome.post_install(
  glib_compile_schemas: true,
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
