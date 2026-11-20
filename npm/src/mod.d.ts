/** Options for decompression calls. */
export interface DecompressOptions {
  /** Maximum decompressed bytes across all frames, 0 to 4294967295. */
  maxDecompressedSize?: number;
}

/** Compression context that reuses its tables and buffers across calls. */
export class Compressor {
  /** Creates a context for `level` (-7 to 4; 0 means 1). */
  constructor(level: number);
  /** Creates a context that compresses every input with `dict`. */
  static withDict(level: number, dict: Dictionary): Compressor;
  /** Compresses `input` into one zstd frame. */
  compress(input: Uint8Array): Uint8Array;
  /** Compresses `input` with `dict` instead of the context dictionary. */
  compressWithDict(input: Uint8Array, dict: Dictionary): Uint8Array;
  /** Releases the WASM memory. */
  free(): void;
  /** Calls `free()`, for `using`. */
  [Symbol.dispose](): void;
}

/** Decompression context that reuses its buffers across calls. */
export class Decompressor {
  /** Creates a context without a dictionary. */
  constructor();
  /** Creates a context for frames compressed with `dict`. */
  static withDict(dict: Dictionary): Decompressor;
  /**
   * Decompresses all frames in `input`.
   * @throws On invalid input, or output above `options.maxDecompressedSize`.
   */
  decompress(input: Uint8Array, options?: DecompressOptions): Uint8Array;
  /** Releases the WASM memory. Later calls throw. Safe to call twice. */
  free(): void;
  /** Calls `free()`, for `using`. */
  [Symbol.dispose](): void;
}

/** A parsed zstd dictionary, as `zstd --train` writes it. */
export class Dictionary {
  /** Parses dictionary bytes. Throws if they are not a valid dictionary. */
  constructor(data: Uint8Array);
  /** The dictionary ID that frames compressed with it carry. */
  readonly id: number;
  /** Releases the WASM memory. */
  free(): void;
  /** Calls `free()`, for `using`. */
  [Symbol.dispose](): void;
}

/**
 * Loads the WASM module, with SIMD if the engine supports it. Call it before
 * anything else.
 */
export function init(): Promise<void>;
/** Initializes synchronously from preloaded WASM bytes. */
export function initSyncFromBytes(bytes: BufferSource): void;
/** Compresses `input` into one zstd frame at `level` (-7 to 4, default 1). */
export function compress(input: Uint8Array, level?: number): Uint8Array;
/**
 * Decompresses all frames in `input`.
 * @throws On invalid input, or output above `options.maxDecompressedSize`.
 */
export function decompress(
  input: Uint8Array,
  options?: DecompressOptions,
): Uint8Array;
/** Largest frame `compress` can produce for `inputLen` bytes. */
export function compressBound(inputLen: number): number;
/** `compress` with `dict`. Decoders need the same dictionary. */
export function compressWithDict(
  input: Uint8Array,
  level: number,
  dict: Dictionary,
): Uint8Array;
/** `decompress` for frames compressed with `dict`. */
export function decompressWithDict(
  input: Uint8Array,
  dict: Dictionary,
  options?: DecompressOptions,
): Uint8Array;
