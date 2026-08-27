#!/bin/sh
set -e
cd "$(dirname "$0")"

JSR_PKG=../jsr/src/pkg
NPM_PKG=src/pkg

if [ ! -f "$JSR_PKG/zrip_wasm.js" ] ||
   [ ! -f "$JSR_PKG/zrip_wasm.d.ts" ] ||
   [ ! -f "$JSR_PKG/zrip_wasm_bg.wasm.d.ts" ] ||
   [ ! -f "$JSR_PKG/zrip_wasm_bg.wasm" ] ||
   [ ! -f "$JSR_PKG/zrip_simd.wasm" ]; then
  (cd ../jsr && bash build.sh)
fi

rm -rf "$NPM_PKG"
mkdir -p "$NPM_PKG"
cp "$JSR_PKG/zrip_wasm.js" "$NPM_PKG/"
cp "$JSR_PKG/zrip_wasm.d.ts" "$NPM_PKG/"
cp "$JSR_PKG/zrip_wasm_bg.wasm.d.ts" "$NPM_PKG/"
cp "$JSR_PKG/zrip_wasm_bg.wasm" "$NPM_PKG/"
cp "$JSR_PKG/zrip_simd.wasm" "$NPM_PKG/"
