#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-or-later
# SPDX-FileCopyrightText: 2026 Jack Tully

# Vendor three.js into src/renderer/three: the core modules and only the addons the renderer
# imports (and theirs), unminified, as published on npm. Run by hand to update; the build
# never needs Node or the network.
#   scripts/update-three.sh 0.186.1
set -euo pipefail
cd "$(dirname "$0")/.."
version="${1:?usage: scripts/update-three.sh VERSION}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
(cd "$tmp" && npm pack "three@$version" --silent >/dev/null && tar xzf "three-$version.tgz")
src="$tmp/package"
dest=src/renderer/three
rm -rf "$dest"
mkdir -p "$dest"
cp "$src/LICENSE" "$src/build/three.core.js" "$src/build/three.module.js" "$dest/"
addons=(
  controls/OrbitControls.js
  environments/RoomEnvironment.js
  loaders/3MFLoader.js
  loaders/DRACOLoader.js
  loaders/FBXLoader.js
  loaders/GLTFLoader.js
  loaders/MTLLoader.js
  loaders/OBJLoader.js
  loaders/PLYLoader.js
  loaders/STLLoader.js
  curves/NURBSCurve.js
  curves/NURBSUtils.js
  utils/BufferGeometryUtils.js
  utils/SkeletonUtils.js
  libs/fflate.module.js
  libs/meshopt_decoder.module.js
  libs/draco/gltf/draco_decoder.wasm
  libs/draco/gltf/draco_wasm_wrapper.js
)
for f in "${addons[@]}"; do
  install -D -m 644 "$src/examples/jsm/$f" "$dest/addons/$f"
done
echo "$version" > "$dest/VERSION"
echo "three.js $version vendored into $dest"
