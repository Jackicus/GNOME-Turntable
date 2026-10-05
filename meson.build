# SPDX-License-Identifier: GPL-2.0-or-later
# SPDX-FileCopyrightText: 2026 Jack Tully

project(
  'turntable',
  version: '0.2.0',
  meson_version: '>= 1.2.0',
  license: 'GPL-2.0-or-later',
)

gnome = import('gnome')
i18n = import('i18n')

app_id = 'io.github.jackicus.Turntable'

dependency('gtk4', version: '>= 4.18')
dependency('libadwaita-1', version: '>= 1.8')    # shortcuts-dialog.ui, AdwShortcutsDialog
dependency('epoxy')                              # GL, as GTK loads it
cargo = find_program('cargo')

pkgdatadir = get_option('prefix') / get_option('datadir') / meson.project_name()
localedir = get_option('prefix') / get_option('localedir')
cargo_target_dir = meson.project_build_root() / 'target'
cargo_env = {
  'TURNTABLE_PKGDATADIR': pkgdatadir,
  'TURNTABLE_LOCALEDIR': localedir,
}

subdir('data')
subdir('src')
subdir('po')

test('cargo', cargo,
  args: ['test', '--release', '--manifest-path', meson.project_source_root() / 'Cargo.toml',
    '--target-dir', cargo_target_dir],
  env: cargo_env,
  timeout: 1200,
  suite: 'turntable',
)

gnome.post_install(
  glib_compile_schemas: true,
  gtk_update_icon_cache: true,
  update_desktop_database: true,
  update_mime_database: true,
)
