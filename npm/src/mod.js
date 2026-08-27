import {
  compress as wasmCompress,
  compressBound as wasmCompressBound,
  Compressor as _Compressor,
  compressWithDict as wasmCompressWithDict,
  decompress as wasmDecompress,
  Decompressor as WasmDecompressor,
  decompressWithDict as wasmDecompressWithDict,
  Dictionary as _Dictionary,
  initSync,
} from "./pkg/zrip_wasm.js";

export const Compressor = _Compressor;
export const Dictionary = _Dictionary;

const MAX_WASM_USIZE = 0xffff_ffff;

function maxDecompressedSize(options) {
  const max = options?.maxDecompressedSize;
  if (max === undefined) return undefined;
  if (!Number.isSafeInteger(max) || max < 0 || max > MAX_WASM_USIZE) {
    throw new RangeError(
      "maxDecompressedSize must be an integer from 0 to 4294967295",
    );
  }
  return max;
}

const decompressorInner = new WeakMap();

function getDecompressorInner(decompressor) {
  const inner = decompressorInner.get(decompressor);
  if (!inner) {
    throw new TypeError("invalid or freed Decompressor");
  }
  return inner;
}

export class Decompressor {
  constructor() {
    decompressorInner.set(this, new WasmDecompressor());
  }

  static withDict(dict) {
    const decompressor = new Decompressor();
    getDecompressorInner(decompressor).free();
    decompressorInner.set(decompressor, WasmDecompressor.withDict(dict));
    return decompressor;
  }

  decompress(input, options) {
    return getDecompressorInner(this).decompress(
      input,
      maxDecompressedSize(options),
    );
  }

  free() {
    const inner = decompressorInner.get(this);
    if (!inner) return;
    decompressorInner.delete(this);
    inner.free();
  }

  [Symbol.dispose]() {
    this.free();
  }
}

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

async function loadWasmBytes(url) {
  if (url.protocol === "file:" && globalThis.process?.versions?.node) {
    const { readFile } = await import("node:fs/promises");
    return readFile(url);
  }

  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`failed to fetch ${url}: HTTP ${response.status}`);
  }
  return response.arrayBuffer();
}

export async function init() {
  if (initialized) return;

  const simd = WebAssembly.validate(SIMD_TEST);
  const wasmFile = simd ? "zrip_simd.wasm" : "zrip_wasm_bg.wasm";
  const wasmUrl = new URL(`./pkg/${wasmFile}`, import.meta.url);
  const bytes = await loadWasmBytes(wasmUrl);
  initSync({ module: new WebAssembly.Module(bytes) });
  initialized = true;
}

export function initSyncFromBytes(bytes) {
  if (initialized) return;
  initSync({ module: new WebAssembly.Module(bytes) });
  initialized = true;
}

export function compress(input, level = 1) {
  return wasmCompress(input, level);
}

export function decompress(input, options) {
  return wasmDecompress(input, maxDecompressedSize(options));
}

export function compressBound(inputLen) {
  return wasmCompressBound(inputLen);
}

export function compressWithDict(input, level, dict) {
  return wasmCompressWithDict(input, level, dict);
}

export function decompressWithDict(input, dict, options) {
  return wasmDecompressWithDict(input, dict, maxDecompressedSize(options));
}
