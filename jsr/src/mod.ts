import {
  initSync,
  compress as wasmCompress,
  decompress as wasmDecompress,
  compressBound as wasmCompressBound,
  compressWithDict as wasmCompressWithDict,
  decompressWithDict as wasmDecompressWithDict,
  Compressor,
  Decompressor,
  Dictionary,
} from "./pkg/zrip_wasm.js";

export { Compressor, Decompressor, Dictionary };

// Minimal valid WASM module that uses a v128 instruction.
// WebAssembly.validate() returns true only if the engine supports simd128.
const SIMD_TEST = new Uint8Array([
  0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60,
  0x00, 0x01, 0x7b, 0x03, 0x02, 0x01, 0x00, 0x0a, 0x0a, 0x01, 0x08, 0x00,
  0x41, 0x00, 0xfd, 0x0f, 0xfd, 0x62, 0x0b,
]);

let initialized = false;

/**
 * Initialize the WASM module. Must be called before any other function.
 * Automatically detects WASM SIMD support and loads the appropriate binary.
 */
export async function init(): Promise<void> {
  if (initialized) return;

  const simd = WebAssembly.validate(SIMD_TEST);
  const wasmFile = simd ? "zrip_simd.wasm" : "zrip_scalar.wasm";
  const wasmUrl = new URL(`./pkg/${wasmFile}`, import.meta.url);
  const response = await fetch(wasmUrl);
  const bytes = await response.arrayBuffer();
  initSync({ module: new WebAssembly.Module(bytes) });
  initialized = true;
}

/**
 * Initialize synchronously with a pre-loaded WASM binary.
 * Use when you have already loaded the WASM bytes (e.g. via fs.readFileSync in Node.js).
 */
export function initSyncFromBytes(bytes: BufferSource): void {
  if (initialized) return;
  initSync({ module: new WebAssembly.Module(bytes) });
  initialized = true;
}

/**
 * Compress data at the given level (-7 to 4). Default level is 1.
 * Returns a standard zstd frame.
 */
export function compress(input: Uint8Array, level = 1): Uint8Array {
  return wasmCompress(input, level);
}

/** Decompress a zstd frame. */
export function decompress(input: Uint8Array): Uint8Array {
  return wasmDecompress(input);
}

/** Upper bound on compressed size for a given input length. */
export function compressBound(inputLen: number): number {
  return wasmCompressBound(inputLen);
}

/** Compress with a pre-parsed dictionary. */
export function compressWithDict(
  input: Uint8Array,
  level: number,
  dict: Dictionary,
): Uint8Array {
  return wasmCompressWithDict(input, level, dict);
}

/** Decompress with a pre-parsed dictionary. */
export function decompressWithDict(
  input: Uint8Array,
  dict: Dictionary,
): Uint8Array {
  return wasmDecompressWithDict(input, dict);
}
