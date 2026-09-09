/**
 * @module
 *
 * Pure Rust zstd codec compiled to WebAssembly. Decodes standard zstd blocks
 * and frames produced at any compression level. Encodes levels -8 through 4
 * for fast transfer pipelines. First-class dictionary support.
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
 * const original = decompress(compressed, { maxDecompressedSize: data.length });
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
 * const original = decompressWithDict(compressed, dict, {
 *   maxDecompressedSize: data.length,
 * });
 * ```
 */

import * as wasmBindings from "./pkg/zrip_wasm_bg.js";
import type * as Wasm from "./pkg/zrip_wasm.js";

// Use generated types for the bindings, including dynamically added methods
// such as Symbol.dispose. The type-only import does not initialize WASM.
const {
  compress: wasmCompress,
  compressBound: wasmCompressBound,
  Compressor: _Compressor,
  compressWithDict: wasmCompressWithDict,
  decompress: wasmDecompress,
  Decompressor: WasmDecompressor,
  decompressWithDict: wasmDecompressWithDict,
  Dictionary: _Dictionary,
} = wasmBindings as unknown as typeof Wasm;

/**
 * Reusable compression context. Amortizes internal allocations across
 * multiple compress calls. Call {@linkcode Compressor.free | .free()} when done,
 * or use `using` for automatic disposal.
 *
 * @example
 * ```ts
 * const compressor = new Compressor(1);
 * const c1 = compressor.compress(data1);
 * const c2 = compressor.compress(data2);
 * compressor.free();
 * ```
 */
export const Compressor: typeof Wasm.Compressor = _Compressor;
/** Type alias for {@linkcode Compressor} instances. */
export type Compressor = Wasm.Compressor;

/**
 * Pre-parsed zstd dictionary for use with dictionary compression.
 * Construct from raw dictionary bytes (trained externally via `zstd --train`
 * or similar). Reuse across compress/decompress calls.
 *
 * @example
 * ```ts
 * const dict = new Dictionary(dictBytes);
 * const compressed = compressWithDict(data, 1, dict);
 * dict.free();
 * ```
 */
export const Dictionary: typeof Wasm.Dictionary = _Dictionary;
/** Type alias for {@linkcode Dictionary} instances. */
export type Dictionary = Wasm.Dictionary;

/** Options for decompression calls. */
export interface DecompressOptions {
  /** Maximum decompressed bytes allowed across the full input stream. */
  maxDecompressedSize?: number;
}

const MAX_WASM_USIZE = 0xffff_ffff;

function maxDecompressedSize(options?: DecompressOptions): number | undefined {
  const max = options?.maxDecompressedSize;
  if (max === undefined) return undefined;
  if (!Number.isSafeInteger(max) || max < 0 || max > MAX_WASM_USIZE) {
    throw new RangeError(
      "maxDecompressedSize must be an integer from 0 to 4294967295",
    );
  }
  return max;
}

const decompressorInner = new WeakMap<Decompressor, Wasm.Decompressor>();

function getDecompressorInner(decompressor: Decompressor): Wasm.Decompressor {
  const inner = decompressorInner.get(decompressor);
  if (!inner) {
    throw new TypeError("invalid or freed Decompressor");
  }
  return inner;
}

/**
 * Reusable decompression context. Amortizes internal allocations across
 * multiple decompress calls. Call {@linkcode Decompressor.free | .free()} when done,
 * or use `using` for automatic disposal.
 *
 * @example
 * ```ts
 * const decompressor = new Decompressor();
 * const d1 = decompressor.decompress(c1);
 * const d2 = decompressor.decompress(c2, { maxDecompressedSize: data2.length });
 * decompressor.free();
 * ```
 */
export class Decompressor {
  constructor() {
    decompressorInner.set(this, new WasmDecompressor());
  }

  static withDict(dict: Dictionary): Decompressor {
    const decompressor = new Decompressor();
    getDecompressorInner(decompressor).free();
    decompressorInner.set(decompressor, WasmDecompressor.withDict(dict));
    return decompressor;
  }

  decompress(
    input: Uint8Array,
    options?: DecompressOptions,
  ): Uint8Array {
    return getDecompressorInner(this).decompress(
      input,
      maxDecompressedSize(options),
    );
  }

  free(): void {
    const inner = decompressorInner.get(this);
    if (!inner) return;
    decompressorInner.delete(this);
    inner.free();
  }

  [Symbol.dispose](): void {
    this.free();
  }
}

// Minimal valid WASM module that uses a v128 instruction.
// WebAssembly.validate() returns true only if the engine supports simd128.
const SIMD_TEST = new Uint8Array([
  0x00,
  0x61,
  0x73,
  0x6d,
  0x01,
  0x00,
  0x00,
  0x00,
  0x01,
  0x05,
  0x01,
  0x60,
  0x00,
  0x01,
  0x7b,
  0x03,
  0x02,
  0x01,
  0x00,
  0x0a,
  0x0a,
  0x01,
  0x08,
  0x00,
  0x41,
  0x00,
  0xfd,
  0x0f,
  0xfd,
  0x62,
  0x0b,
]);

let initialized = false;
let initialization: Promise<void> | undefined;

function finishInitialization(wasm: Record<string, unknown>): void {
  // A synchronous caller may have initialized while the import was pending.
  if (initialized) return;
  wasmBindings.__wbg_set_wasm(wasm);
  // wasm-bindgen emits this initializer when the module needs startup work.
  if (typeof wasm.__wbindgen_start === "function") wasm.__wbindgen_start();
  initialized = true;
}

/**
 * Initialize the WASM module. Must be called before compression or decoding.
 * Automatically detects WASM SIMD support and loads the appropriate binary.
 */
export function init(): Promise<void> {
  if (initialized) return Promise.resolve();
  if (initialization) return initialization;

  initialization = (async () => {
    // Literal imports let bundlers include WASM and its generated JS bindings.
    const wasm = WebAssembly.validate(SIMD_TEST)
      ? await import("./pkg/zrip_simd.wasm")
      : await import("./pkg/zrip_wasm_bg.wasm");
    finishInitialization(wasm);
  })().catch((error) => {
    initialization = undefined;
    throw error;
  });
  return initialization;
}

/** Initialize synchronously from preloaded WASM bytes. */
export function initSyncFromBytes(bytes: BufferSource): void {
  if (initialized) return;
  const module = new WebAssembly.Module(bytes);
  const instance = new WebAssembly.Instance(module, {
    "./zrip_wasm_bg.js": wasmBindings,
  });
  finishInitialization(instance.exports);
}

/**
 * Compress data at the given zstd level. Returns a standard zstd frame.
 *
 * @param input The data to compress.
 * @param level Compression level from -8 (fastest) to 4 (best ratio). Default: 1.
 * @returns Compressed zstd frame as a `Uint8Array`.
 *
 * @example
 * ```ts
 * const compressed = compress(data);           // level 1
 * const fast = compress(data, -8);             // fastest
 * const best = compress(data, 4);              // best ratio
 * ```
 */
export function compress(input: Uint8Array, level = 1): Uint8Array {
  return wasmCompress(input, level);
}

/**
 * Decompress a zstd frame or concatenated zstd frame stream.
 *
 * @param input Compressed zstd frame.
 * @param options Optional decompression limits.
 * @returns Decompressed data as a `Uint8Array`.
 * @throws On invalid, truncated, or corrupted input.
 */
export function decompress(
  input: Uint8Array,
  options?: DecompressOptions,
): Uint8Array {
  return wasmDecompress(input, maxDecompressedSize(options));
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
 * @param level Compression level from -8 to 4.
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
 * Decompress a zstd frame or concatenated zstd frame stream that was
 * compressed with a dictionary.
 *
 * @param input Compressed zstd frame.
 * @param dict The same {@linkcode Dictionary} used during compression.
 * @param options Optional decompression limits.
 * @returns Decompressed data as a `Uint8Array`.
 * @throws On invalid input or dictionary mismatch.
 */
export function decompressWithDict(
  input: Uint8Array,
  dict: Dictionary,
  options?: DecompressOptions,
): Uint8Array {
  return wasmDecompressWithDict(input, dict, maxDecompressedSize(options));
}
