import {
  assertEquals,
  assert,
  assertThrows,
} from "https://deno.land/std@0.224.0/assert/mod.ts";
import {
  init,
  compress,
  decompress,
  compressBound,
  compressWithDict,
  decompressWithDict,
  Compressor,
  Decompressor,
  Dictionary,
} from "./mod.ts";

Deno.test("init", async () => {
  await init();
});

// --- One-shot ---

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

Deno.test("incompressible data", () => {
  const random = crypto.getRandomValues(new Uint8Array(4096));
  const compressed = compress(random);
  const decompressed = decompress(compressed);
  assertEquals(decompressed, random);
});

// --- Stateful ---

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

// --- Dictionary ---

function trainDict(): { dict: Dictionary; samples: Uint8Array } {
  const sampleStrings: string[] = [];
  for (let i = 0; i < 200; i++) {
    sampleStrings.push(
      JSON.stringify({
        id: i,
        user: `user_${i % 20}`,
        action: ["click", "view", "scroll", "type"][i % 4],
        ts: 1700000000 + i * 60,
      }),
    );
  }
  const samples = new TextEncoder().encode(sampleStrings.join("\n"));

  const tmpDir = Deno.makeTempDirSync();
  for (let i = 0; i < sampleStrings.length; i++) {
    Deno.writeTextFileSync(`${tmpDir}/s${i}`, sampleStrings[i]);
  }

  const result = new Deno.Command("zstd", {
    args: ["--train", ...Array.from({ length: sampleStrings.length }, (_, i) => `${tmpDir}/s${i}`), "-o", `${tmpDir}/dict`],
    stdout: "piped",
    stderr: "piped",
  }).outputSync();

  if (result.code !== 0) {
    throw new Error(`zstd --train failed: ${new TextDecoder().decode(result.stderr)}`);
  }

  const dictBytes = Deno.readFileSync(`${tmpDir}/dict`);

  for (let i = 0; i < sampleStrings.length; i++) Deno.removeSync(`${tmpDir}/s${i}`);
  Deno.removeSync(`${tmpDir}/dict`);
  Deno.removeSync(tmpDir);

  return { dict: new Dictionary(dictBytes), samples };
}

let _dictCache: { dict: Dictionary; samples: Uint8Array } | null = null;
function getDict(): { dict: Dictionary; samples: Uint8Array } {
  if (!_dictCache) {
    _dictCache = trainDict();
  }
  return _dictCache;
}

Deno.test("dictionary one-shot round-trip", () => {
  const { dict, samples } = getDict();
  const compressed = compressWithDict(samples, 1, dict);
  const decompressed = decompressWithDict(compressed, dict);
  assertEquals(decompressed, samples);
});

Deno.test("dictionary all levels", () => {
  const { dict, samples } = getDict();
  for (let level = -7; level <= 4; level++) {
    const compressed = compressWithDict(samples, level, dict);
    const decompressed = decompressWithDict(compressed, dict);
    assertEquals(decompressed, samples, `dict round-trip failed at level ${level}`);
  }
});

Deno.test("dictionary improves ratio", () => {
  const { dict } = getDict();
  // Compress a single sample that matches the dict's training data
  const sample = new TextEncoder().encode(
    JSON.stringify({ id: 999, user: "user_5", action: "click", ts: 1700060000 }),
  );
  const withDict = compressWithDict(sample, 1, dict);
  const withoutDict = compress(sample, 1);
  assert(
    withDict.length <= withoutDict.length,
    `dict should help: ${withDict.length} > ${withoutDict.length}`,
  );
});

Deno.test("dictionary id", () => {
  const { dict } = getDict();
  assert(dict.id > 0, "dict id should be nonzero");
});

Deno.test("stateful compressor with dict", () => {
  const { dict } = getDict();
  const compressor = Compressor.withDict(1, dict);
  const sample = new TextEncoder().encode(
    JSON.stringify({ id: 42, user: "user_1", action: "view", ts: 1700002520 }),
  );

  const compressed = compressor.compress(sample);
  const decompressor = Decompressor.withDict(dict);
  const decompressed = decompressor.decompress(compressed);
  assertEquals(decompressed, sample);

  compressor.free();
  decompressor.free();
});

Deno.test("stateful compressor compressWithDict", () => {
  const { dict } = getDict();
  const compressor = new Compressor(1);
  const sample = new TextEncoder().encode(
    JSON.stringify({ id: 77, user: "user_10", action: "scroll", ts: 1700004620 }),
  );

  const compressed = compressor.compressWithDict(sample, dict);
  const decompressor = Decompressor.withDict(dict);
  const decompressed = decompressor.decompress(compressed);
  assertEquals(decompressed, sample);

  compressor.free();
  decompressor.free();
});

// --- Error paths ---

Deno.test("decompress invalid data throws", () => {
  assertThrows(
    () => decompress(new Uint8Array([0, 1, 2, 3, 4, 5])),
    Error,
  );
});

Deno.test("decompress truncated frame throws", () => {
  const data = new TextEncoder().encode("hello".repeat(100));
  const compressed = compress(data);
  // Truncate the compressed data
  assertThrows(
    () => decompress(compressed.slice(0, compressed.length / 2)),
    Error,
  );
});

Deno.test("decompress corrupted frame throws", () => {
  const data = new TextEncoder().encode("test data for corruption".repeat(50));
  const compressed = compress(data);
  // Flip some bytes in the middle
  const corrupted = new Uint8Array(compressed);
  corrupted[compressed.length / 2] ^= 0xff;
  corrupted[compressed.length / 2 + 1] ^= 0xff;
  assertThrows(
    () => decompress(corrupted),
    Error,
  );
});

Deno.test("invalid compression level throws", () => {
  const data = new TextEncoder().encode("test");
  assertThrows(
    () => compress(data, 99),
    Error,
  );
});
