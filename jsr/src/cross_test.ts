import { assertEquals } from "https://deno.land/std@0.224.0/assert/mod.ts";
import { init, compress, decompress } from "./mod.ts";

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
