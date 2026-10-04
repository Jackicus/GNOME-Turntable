#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-or-later
# SPDX-FileCopyrightText: 2026 Jack Tully

# Build into build/install and run it, with its settings in memory (the desktop's are never
# touched). TURNTABLE_DEVTOOLS=1 gives the view WebKit's inspector (right-click).
#   scripts/run.sh [MODEL…]
set -euo pipefail
cd "$(dirname "$0")/.."
[ -d build ] || meson setup build --prefix="$PWD/build/install" >/dev/null
meson install -C build --quiet >/dev/null
export XDG_DATA_DIRS="$PWD/build/install/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export GSETTINGS_SCHEMA_DIR="$PWD/build/install/share/glib-2.0/schemas"
export GSETTINGS_BACKEND="${GSETTINGS_BACKEND:-memory}"
exec build/install/bin/turntable "$@"
