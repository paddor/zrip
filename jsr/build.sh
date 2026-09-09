#!/bin/sh
set -e
cd "$(dirname "$0")"

# The npm package uses web bindings; JSR uses native WASM imports.
BINDGEN_TARGET=${1:-bundler}
PKG=${2:-src/pkg}
TMP=src/pkg-tmp

rm -rf "$PKG" "$TMP"
mkdir -p "$PKG"

echo "==> Building scalar WASM..."
cd wasm
RUSTFLAGS="" wasm-pack build --target "$BINDGEN_TARGET" --release --out-dir "../$TMP"
cd ..

cp "$TMP/"*.js "$PKG/"
cp "$TMP/zrip_wasm.d.ts" "$PKG/"
cp "$TMP/zrip_wasm_bg.wasm.d.ts" "$PKG/"
mv "$TMP/zrip_wasm_bg.wasm" "$PKG/"

echo "==> Building simd128 WASM..."
cd wasm
RUSTFLAGS="-C target-feature=+simd128" wasm-pack build --target "$BINDGEN_TARGET" --release --out-dir "../$TMP"
cd ..

mv "$TMP/zrip_wasm_bg.wasm" "$PKG/zrip_simd.wasm"

echo "==> Verifying JS glue is identical..."
if ! diff -q "$TMP/zrip_wasm.js" "$PKG/zrip_wasm.js" > /dev/null 2>&1; then
    echo "WARNING: JS glue differs between scalar and simd128 builds!"
    exit 1
fi
if [ "$BINDGEN_TARGET" = bundler ]; then
    diff "$TMP/zrip_wasm_bg.js" "$PKG/zrip_wasm_bg.js"
fi

rm -rf "$TMP"

SCALAR_SIZE=$(wc -c < "$PKG/zrip_wasm_bg.wasm")
SIMD_SIZE=$(wc -c < "$PKG/zrip_simd.wasm")
echo "==> Done. scalar: ${SCALAR_SIZE} bytes, simd128: ${SIMD_SIZE} bytes"
