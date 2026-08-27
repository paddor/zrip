import assert from "node:assert/strict";
import test from "node:test";
import {
  compress,
  compressBound,
  Compressor,
  decompress,
  Decompressor,
  init,
} from "../src/mod.js";

await init();

test("one-shot round-trip", () => {
  const data = new TextEncoder().encode("hello world, hello zstd!".repeat(100));
  const compressed = compress(data);
  assert.ok(compressed.length < data.length);
  assert.deepEqual(decompress(compressed), data);
});

test("all levels", () => {
  const data = new TextEncoder().encode("test data for all levels".repeat(50));
  for (let level = -8; level <= 4; level++) {
    const compressed = compress(data, level);
    assert.deepEqual(decompress(compressed), data);
  }
});

test("compressBound", () => {
  assert.ok(compressBound(1000) >= 1000);
});

test("decompress limit applies", () => {
  const data = new TextEncoder().encode("limited output".repeat(20));
  const compressed = compress(data);

  assert.throws(() =>
    decompress(compressed, { maxDecompressedSize: data.length - 1 })
  );
  assert.deepEqual(
    decompress(compressed, { maxDecompressedSize: data.length }),
    data,
  );
});

test("decompress rejects invalid limit option", () => {
  const data = new TextEncoder().encode("test");
  const compressed = compress(data);

  assert.throws(
    () => decompress(compressed, { maxDecompressedSize: -1 }),
    RangeError,
  );
});

test("stateful compressor", () => {
  const compressor = new Compressor(1);
  const data = new TextEncoder().encode("stateful compression".repeat(50));
  const compressed = compressor.compress(data);

  assert.deepEqual(decompress(compressed), data);
  compressor.free();
});

test("stateful decompressor", () => {
  const data = new TextEncoder().encode("stateful decompression".repeat(50));
  const compressed = compress(data);
  const decompressor = new Decompressor();

  assert.deepEqual(decompressor.decompress(compressed), data);
  decompressor.free();
});
