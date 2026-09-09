#!/bin/sh
set -e
cd "$(dirname "$0")"

# Node initialization uses wasm-bindgen's web API, independently of JSR's
# native WASM imports. Build into the npm package without replacing JSR files.
(cd ../jsr && bash build.sh web ../npm/src/pkg)
