#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
# SPDX-FileCopyrightText: 2026 Jack Tully

# Meson's step that builds the binary: cargo build, then the binary where Meson wants it.
#   cargo.sh CARGO MANIFEST TARGET_DIR OUTPUT
set -eu
cargo="$1" manifest="$2" target="$3" output="$4"
"$cargo" build --release --manifest-path "$manifest" --target-dir "$target" --bin turntable
cp "$target/release/turntable" "$output"
