import { assertEquals, assert } from "https://deno.land/std@0.224.0/assert/mod.ts";
import {
  init,
  compress,
  decompress,
  compressBound,
  Compressor,
  Decompressor,
} from "./mod.ts";

Deno.test("init", async () => {
  await init();
});

Deno.test("one-shot round-trip", () => {
  const data = new TextEncoder().encode("hello world, hello zstd!".repeat(100));
  const compressed = compress(data);
  assert(compressed.length < data.length);
  const decompressed = decompress(compressed);
  assertEquals(decompressed, data);
});

Deno.test("all levels", () => {
  const data = new TextEncoder().encode("test data for all levels".repeat(50));
  for (let level = -7; level <= 4; level++) {
    const compressed = compress(data, level);
    const decompressed = decompress(compressed);
    assertEquals(decompressed, data, `round-trip failed at level ${level}`);
  }
});

Deno.test("compressBound", () => {
  const bound = compressBound(1000);
  assert(bound >= 1000);
});

Deno.test("empty input", () => {
  const empty = new Uint8Array(0);
  const compressed = compress(empty);
  const decompressed = decompress(compressed);
  assertEquals(decompressed, empty);
});

Deno.test("stateful compressor", () => {
  const compressor = new Compressor(1);
  const data1 = new TextEncoder().encode("first message".repeat(50));
  const data2 = new TextEncoder().encode("second message".repeat(50));

  const c1 = compressor.compress(data1);
  const c2 = compressor.compress(data2);

  assertEquals(decompress(c1), data1);
  assertEquals(decompress(c2), data2);

  compressor.free();
});

Deno.test("stateful decompressor", () => {
  const data1 = new TextEncoder().encode("decompress test 1".repeat(50));
  const data2 = new TextEncoder().encode("decompress test 2".repeat(50));

  const c1 = compress(data1);
  const c2 = compress(data2);

  const decompressor = new Decompressor();
  assertEquals(decompressor.decompress(c1), data1);
  assertEquals(decompressor.decompress(c2), data2);

  decompressor.free();
});

Deno.test("incompressible data", () => {
  const random = crypto.getRandomValues(new Uint8Array(4096));
  const compressed = compress(random);
  const decompressed = decompress(compressed);
  assertEquals(decompressed, random);
});
