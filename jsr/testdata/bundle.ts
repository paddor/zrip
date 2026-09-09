import {
  compress,
  Compressor,
  decompress,
  Decompressor,
  init,
  initSyncFromBytes,
} from "../src/mod.ts";

const mode = Deno.args[0];
if (mode === "scalar") {
  WebAssembly.validate = () => false;
}
const pending = mode === "race" ? init() : undefined;
if (mode === "sync" || mode === "race") {
  let rejected = false;
  try {
    initSyncFromBytes(new Uint8Array());
  } catch (error) {
    if (!(error instanceof WebAssembly.CompileError)) throw error;
    rejected = true;
  }
  if (!rejected) throw new Error("invalid WASM bytes were accepted");
  initSyncFromBytes(Deno.readFileSync(Deno.args[1]));
}

// Keep an allocated context alive while asynchronous initialization finishes.
const early = mode === "race" ? new Compressor() : undefined;
await Promise.all([pending, init(), init(), init()]);
const compressor = early ?? new Compressor();
const decompressor = new Decompressor();
for (const text of ["", "bundled compression 😀".repeat(1000)]) {
  const data = new TextEncoder().encode(text);
  const compressed = compress(data);
  const decoded = decompress(compressed);
  const reused = decompressor.decompress(compressor.compress(data));
  for (const output of [decoded, reused]) {
    if (
      output.length !== data.length ||
      output.some((byte, i) => byte !== data[i])
    ) {
      throw new Error("roundtrip mismatch");
    }
  }
  await init();
}
compressor.free();
decompressor.free();
console.log("roundtrip passed");
