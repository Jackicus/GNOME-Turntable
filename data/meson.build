# SPDX-License-Identifier: GPL-2.0-or-later
# SPDX-FileCopyrightText: 2026 Jack Tully

desktop_file = i18n.merge_file(
  input: app_id + '.desktop.in',
  output: app_id + '.desktop',
  type: 'desktop',
  po_dir: '../po',
  install: true,
  install_dir: get_option('datadir') / 'applications',
)

metainfo_file = i18n.merge_file(
  input: app_id + '.metainfo.xml.in',
  output: app_id + '.metainfo.xml',
  po_dir: '../po',
  install: true,
  install_dir: get_option('datadir') / 'metainfo',
)

install_data(app_id + '.mime.xml',
  install_dir: get_option('datadir') / 'mime' / 'packages',
)

install_data(app_id + '.gschema.xml',
  install_dir: get_option('datadir') / 'glib-2.0' / 'schemas',
)

install_data('icons' / app_id + '.svg',
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'scalable' / 'apps',
)
install_data('icons' / app_id + '-symbolic.svg',
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'symbolic' / 'apps',
)

desktop_validate = find_program('desktop-file-validate', required: false)
if desktop_validate.found()
  test('desktop', desktop_validate, args: [desktop_file], suite: 'data')
endif
appstreamcli = find_program('appstreamcli', required: false)
if appstreamcli.found()
  test('metainfo', appstreamcli, args: ['validate', '--no-net', '--explain', metainfo_file],
    suite: 'data')
endif
compile_schemas = find_program('glib-compile-schemas', required: false)
if compile_schemas.found()
  test('schema', compile_schemas, args: ['--strict', '--dry-run', meson.current_source_dir()],
    suite: 'data')
endif
