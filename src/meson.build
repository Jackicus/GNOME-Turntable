# SPDX-License-Identifier: GPL-2.0-or-later
# SPDX-FileCopyrightText: 2026 Jack Tully

# blueprint-compiler from PATH, else the wrap's (run from its source tree: its configured
# wrapper can't find its own module uninstalled).
blueprint_compiler = find_program('blueprint-compiler', version: '>= 0.22', required: false)
if not blueprint_compiler.found()
  subproject('blueprint-compiler')
  blueprint_compiler = find_program(
    meson.project_source_root() / 'subprojects' / 'blueprint-compiler' / 'blueprint-compiler.py',
  )
endif

blueprints = []
foreach blueprint : files('window.blp', 'shortcuts-dialog.blp')
  blueprints += custom_target(
    input: blueprint,
    output: '@BASENAME@.ui',
    command: [blueprint_compiler, 'compile', '--output', '@OUTPUT@', '@INPUT@'],
  )
endforeach

# The About dialog is built from the metainfo, so the gresource carries a copy.
metainfo_resource = custom_target(
  input: metainfo_file,
  output: 'metainfo.xml',
  command: ['cp', '@INPUT@', '@OUTPUT@'],
)

gnome.compile_resources(
  'turntable',
  'turntable.gresource.xml',
  gresource_bundle: true,
  install: true,
  install_dir: pkgdatadir,
  dependencies: blueprints + [metainfo_resource],
)

# Cargo builds the binary; it decides itself whether anything changed.
custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: meson.project_name(),
  console: true,
  install: true,
  install_dir: get_option('bindir'),
  env: cargo_env,
  command: [
    meson.project_source_root() / 'build-aux' / 'cargo.sh',
    cargo,
    meson.project_source_root() / 'Cargo.toml',
    cargo_target_dir,
    '@OUTPUT@',
  ],
)
