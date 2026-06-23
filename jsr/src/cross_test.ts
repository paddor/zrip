import { assertEquals } from "https://deno.land/std@0.224.0/assert/mod.ts";
import {
  init,
  compress,
  decompress,
  compressWithDict,
  decompressWithDict,
  Dictionary,
} from "./mod.ts";

await init();

Deno.test("cross-validate: zrip compress -> zstd decompress", async () => {
  const data = new TextEncoder().encode(
    "cross-validation test data".repeat(200),
  );
  const compressed = compress(data, 1);

  const proc = new Deno.Command("zstd", {
    args: ["-d", "--stdout"],
    stdin: "piped",
    stdout: "piped",
  });
  const child = proc.spawn();
  const writer = child.stdin.getWriter();
  await writer.write(compressed);
  await writer.close();
  const output = await child.output();
  assertEquals(output.code, 0);
  assertEquals(new Uint8Array(output.stdout), data);
});

Deno.test("cross-validate: zstd compress -> zrip decompress", async () => {
  const data = new TextEncoder().encode(
    "reverse cross-validation".repeat(200),
  );

  const proc = new Deno.Command("zstd", {
    args: ["-1", "--stdout"],
    stdin: "piped",
    stdout: "piped",
  });
  const child = proc.spawn();
  const writer = child.stdin.getWriter();
  await writer.write(data);
  await writer.close();
  const output = await child.output();
  assertEquals(output.code, 0);

  const decompressed = decompress(new Uint8Array(output.stdout));
  assertEquals(decompressed, data);
});

// Cross-validate all levels
for (const level of [-7, -3, -1, 1, 2, 3, 4]) {
  Deno.test(`cross-validate L${level}: zrip compress -> zstd decompress`, async () => {
    const data = new TextEncoder().encode(
      `level ${level} cross-validation data `.repeat(200),
    );
    const compressed = compress(data, level);

    const proc = new Deno.Command("zstd", {
      args: ["-d", "--stdout"],
      stdin: "piped",
      stdout: "piped",
    });
    const child = proc.spawn();
    const writer = child.stdin.getWriter();
    await writer.write(compressed);
    await writer.close();
    const output = await child.output();
    assertEquals(output.code, 0, `zstd failed to decompress zrip L${level} output`);
    assertEquals(new Uint8Array(output.stdout), data);
  });
}

// Cross-validate with dictionary
Deno.test("cross-validate: dict compress zrip -> decompress zstd", async () => {
  const samples: string[] = [];
  for (let i = 0; i < 200; i++) {
    samples.push(
      JSON.stringify({
        id: i,
        user: `user_${i % 20}`,
        action: ["click", "view", "scroll", "type"][i % 4],
        ts: 1700000000 + i * 60,
      }),
    );
  }

  // Train dict via zstd CLI
  const tmpDir = Deno.makeTempDirSync();
  for (let i = 0; i < samples.length; i++) {
    Deno.writeTextFileSync(`${tmpDir}/s${i}`, samples[i]);
  }
  const trainResult = new Deno.Command("zstd", {
    args: [
      "--train",
      ...Array.from({ length: samples.length }, (_, i) => `${tmpDir}/s${i}`),
      "-o",
      `${tmpDir}/dict`,
    ],
    stdout: "piped",
    stderr: "piped",
  }).outputSync();
  assertEquals(trainResult.code, 0, "zstd --train failed");

  const dictPath = `${tmpDir}/dict`;
  const dictBytes = Deno.readFileSync(dictPath);
  const dict = new Dictionary(dictBytes);

  // Compress with zrip + dict, decompress with C zstd + dict
  const input = new TextEncoder().encode(samples.slice(0, 10).join("\n"));
  const compressed = compressWithDict(input, 1, dict);

  const proc = new Deno.Command("zstd", {
    args: ["-d", "--stdout", "-D", dictPath],
    stdin: "piped",
    stdout: "piped",
  });
  const child = proc.spawn();
  const writer = child.stdin.getWriter();
  await writer.write(compressed);
  await writer.close();
  const output = await child.output();
  assertEquals(output.code, 0, "zstd failed to decompress zrip dict output");
  assertEquals(new Uint8Array(output.stdout), input);

  // Cleanup
  for (let i = 0; i < samples.length; i++) Deno.removeSync(`${tmpDir}/s${i}`);
  Deno.removeSync(dictPath);
  Deno.removeSync(tmpDir);
});

Deno.test("cross-validate: dict compress zstd -> decompress zrip", async () => {
  const samples: string[] = [];
  for (let i = 0; i < 200; i++) {
    samples.push(
      JSON.stringify({
        id: i,
        user: `user_${i % 20}`,
        action: ["click", "view", "scroll", "type"][i % 4],
        ts: 1700000000 + i * 60,
      }),
    );
  }

  const tmpDir = Deno.makeTempDirSync();
  for (let i = 0; i < samples.length; i++) {
    Deno.writeTextFileSync(`${tmpDir}/s${i}`, samples[i]);
  }
  const trainResult = new Deno.Command("zstd", {
    args: [
      "--train",
      ...Array.from({ length: samples.length }, (_, i) => `${tmpDir}/s${i}`),
      "-o",
      `${tmpDir}/dict`,
    ],
    stdout: "piped",
    stderr: "piped",
  }).outputSync();
  assertEquals(trainResult.code, 0, "zstd --train failed");

  const dictPath = `${tmpDir}/dict`;
  const dictBytes = Deno.readFileSync(dictPath);
  const dict = new Dictionary(dictBytes);

  // Compress with C zstd + dict, decompress with zrip + dict
  const input = new TextEncoder().encode(samples.slice(0, 10).join("\n"));
  Deno.writeFileSync(`${tmpDir}/input`, input);

  const proc = new Deno.Command("zstd", {
    args: ["-1", "--stdout", "-D", dictPath, `${tmpDir}/input`],
    stdout: "piped",
    stderr: "piped",
  });
  const result = proc.outputSync();
  assertEquals(result.code, 0, "zstd compress failed");

  const decompressed = decompressWithDict(new Uint8Array(result.stdout), dict);
  assertEquals(decompressed, input);

  // Cleanup
  for (let i = 0; i < samples.length; i++) Deno.removeSync(`${tmpDir}/s${i}`);
  Deno.removeSync(dictPath);
  Deno.removeSync(`${tmpDir}/input`);
  Deno.removeSync(tmpDir);
});
