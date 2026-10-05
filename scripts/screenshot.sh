#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-or-later
# SPDX-FileCopyrightText: 2026 Jack Tully

# A PNG of a window of the installed build (scripts/run.sh builds it), with a model open or
# not. Run it on the headless display, with settings in memory:
#   scripts/headless.sh scripts/screenshot.sh build/shot.png [MODEL] [--light] [--size WxH]
#       [--properties] [--set KEY=VALUE]… [--wait SECONDS] [--time SECONDS] [--view]
set -euo pipefail
cd "$(dirname "$0")/.."
[ -d "$HOME/.cargo/bin" ] && export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --quiet --target-dir build/target --example screenshot
exec build/target/release/examples/screenshot "$@"
