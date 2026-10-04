#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-or-later
# SPDX-FileCopyrightText: 2026 Jack Tully

# Run a command on a private, invisible display: a headless mutter with a virtual monitor, in
# its own D-Bus session, so test windows never appear on (or take focus from) the desktop.
# Settings are in memory and at their defaults (GNOME's blue accent, for one): nothing the
# desktop's user has set reaches a test run or a screenshot.
#   scripts/headless.sh scripts/screenshot.js build/shot.png tests/models/cube.gltf
#   HEADLESS_SIZE=1280x800 scripts/headless.sh scripts/screenshot.js …
set -euo pipefail
cd "$(dirname "$0")/.."
if [ $# -eq 0 ]; then
  echo "usage: scripts/headless.sh COMMAND [ARGUMENT…]" >&2
  exit 2
fi
command -v mutter >/dev/null || { echo "headless.sh: mutter is not installed" >&2; exit 1; }
command -v dbus-run-session >/dev/null || { echo "headless.sh: dbus-run-session is not installed" >&2; exit 1; }
export GDK_BACKEND=wayland GIO_USE_VFS=local GSETTINGS_BACKEND=memory
unset DISPLAY
exec dbus-run-session -- mutter --headless --wayland --no-x11 \
  --wayland-display="turntable-headless-$$" \
  --virtual-monitor "${HEADLESS_SIZE:-1280x800}" -- "$@" 2> >(grep -v -e '^libmutter-Message' \
  -e 'dbus-daemon\[' -e 'xdg-desktop-portal-WARNING' -e 'high priority EGL context' \
  -e "connection to the bus can't be made" >&2)
