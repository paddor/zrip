export interface DecompressOptions {
  maxDecompressedSize?: number;
}

export class Compressor {
  constructor(level: number);
  static withDict(level: number, dict: Dictionary): Compressor;
  compress(input: Uint8Array): Uint8Array;
  compressWithDict(input: Uint8Array, dict: Dictionary): Uint8Array;
  free(): void;
  [Symbol.dispose](): void;
}

export class Decompressor {
  constructor();
  static withDict(dict: Dictionary): Decompressor;
  decompress(input: Uint8Array, options?: DecompressOptions): Uint8Array;
  free(): void;
  [Symbol.dispose](): void;
}

export class Dictionary {
  constructor(data: Uint8Array);
  readonly id: number;
  free(): void;
  [Symbol.dispose](): void;
}

export function init(): Promise<void>;
export function initSyncFromBytes(bytes: BufferSource): void;
export function compress(input: Uint8Array, level?: number): Uint8Array;
export function decompress(
  input: Uint8Array,
  options?: DecompressOptions,
): Uint8Array;
export function compressBound(inputLen: number): number;
export function compressWithDict(
  input: Uint8Array,
  level: number,
  dict: Dictionary,
): Uint8Array;
export function decompressWithDict(
  input: Uint8Array,
  dict: Dictionary,
  options?: DecompressOptions,
): Uint8Array;
