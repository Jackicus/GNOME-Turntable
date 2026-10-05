#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-or-later
# SPDX-FileCopyrightText: 2026 Jack Tully

# Everything that must pass before a change lands: the build (Blueprint included), clippy,
# the tests, and the desktop file, metainfo and schema validated. Prints `check: ok`.
set -euo pipefail
cd "$(dirname "$0")/.."
[ -d "$HOME/.cargo/bin" ] && export PATH="$HOME/.cargo/bin:$PATH"
[ -d build ] || meson setup build --prefix="$PWD/build/install" >/dev/null
meson compile -C build >/dev/null
meson install -C build --quiet >/dev/null
cargo fmt --check
cargo clippy --release --quiet --target-dir build/target --all-targets -- -D warnings
meson test -C build --suite turntable --suite data --print-errorlogs
echo "check: ok"
