/**
 * @module
 *
 * Pure Rust zstd codec compiled to WebAssembly. Levels -7 through 4
 * (Fast and DFast strategies). Optimized for encode throughput in transfer
 * pipelines that need standard zstd frames at high speed.
 *
 * Automatically detects WASM SIMD support and loads the appropriate binary.
 *
 * ```ts
 * import { init, compress, decompress } from "@paddor/zrip";
 *
 * await init();
 *
 * const data = new TextEncoder().encode("hello world".repeat(1000));
 * const compressed = compress(data, 1);
 * const original = decompress(compressed);
 * ```
 *
 * Reusable contexts amortize internal allocations across calls:
 *
 * ```ts
 * import { init, Compressor, Decompressor } from "@paddor/zrip";
 *
 * await init();
 *
 * const compressor = new Compressor(1);
 * const c1 = compressor.compress(data1);
 * const c2 = compressor.compress(data2);
 * compressor.free();
 *
 * const decompressor = new Decompressor();
 * const d1 = decompressor.decompress(c1);
 * const d2 = decompressor.decompress(c2);
 * decompressor.free();
 * ```
 *
 * Dictionary compression for small-message workloads:
 *
 * ```ts
 * import { init, compress, compressWithDict, decompressWithDict, Dictionary } from "@paddor/zrip";
 *
 * await init();
 *
 * const dict = new Dictionary(dictBytes);
 * const compressed = compressWithDict(data, 1, dict);
 * const original = decompressWithDict(compressed, dict);
 * ```
 */

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
 * Use when you have already loaded the WASM bytes (e.g. via `Deno.readFileSync`
 * or `fs.readFileSync` in Node.js).
 */
export function initSyncFromBytes(bytes: BufferSource): void {
  if (initialized) return;
  initSync({ module: new WebAssembly.Module(bytes) });
  initialized = true;
}

/**
 * Compress data at the given zstd level. Returns a standard zstd frame.
 *
 * @param input The data to compress.
 * @param level Compression level from -7 (fastest) to 4 (best ratio). Default: 1.
 * @returns Compressed zstd frame as a `Uint8Array`.
 *
 * @example
 * ```ts
 * const compressed = compress(data);           // level 1
 * const fast = compress(data, -7);             // fastest
 * const best = compress(data, 4);              // best ratio
 * ```
 */
export function compress(input: Uint8Array, level = 1): Uint8Array {
  return wasmCompress(input, level);
}

/**
 * Decompress a zstd frame.
 *
 * @param input Compressed zstd frame.
 * @returns Decompressed data as a `Uint8Array`.
 * @throws On invalid, truncated, or corrupted input.
 */
export function decompress(input: Uint8Array): Uint8Array {
  return wasmDecompress(input);
}

/**
 * Upper bound on compressed size for a given input length.
 * Useful for pre-allocating output buffers.
 */
export function compressBound(inputLen: number): number {
  return wasmCompressBound(inputLen);
}

/**
 * Compress with a pre-parsed dictionary. Dictionaries improve compression
 * ratio on small messages (log lines, JSON records, RPC payloads) that
 * share common structure.
 *
 * @param input The data to compress.
 * @param level Compression level from -7 to 4.
 * @param dict A {@linkcode Dictionary} instance.
 * @returns Compressed zstd frame as a `Uint8Array`.
 */
export function compressWithDict(
  input: Uint8Array,
  level: number,
  dict: Dictionary,
): Uint8Array {
  return wasmCompressWithDict(input, level, dict);
}

/**
 * Decompress a zstd frame that was compressed with a dictionary.
 *
 * @param input Compressed zstd frame.
 * @param dict The same {@linkcode Dictionary} used during compression.
 * @returns Decompressed data as a `Uint8Array`.
 * @throws On invalid input or dictionary mismatch.
 */
export function decompressWithDict(
  input: Uint8Array,
  dict: Dictionary,
): Uint8Array {
  return wasmDecompressWithDict(input, dict);
}
