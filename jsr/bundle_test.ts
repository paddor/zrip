import { strictEqual as assertEquals } from "node:assert";

Deno.test("bundled initialization and roundtrips", async (test) => {
  const directory = await Deno.makeTempDir();
  try {
    const output = `${directory}/bundle.js`;
    const bundle = await new Deno.Command(Deno.execPath(), {
      args: [
        "bundle",
        "--no-config",
        "--output",
        output,
        new URL("./testdata/bundle.ts", import.meta.url).pathname,
      ],
    }).output();
    assertEquals(bundle.code, 0, new TextDecoder().decode(bundle.stderr));

    for (const mode of ["async", "scalar", "sync", "race"]) {
      await test.step(mode, async () => {
        // Default initialization gets no file or network permissions. Only the
        // explicit bytes API may read a caller-supplied binary.
        const sync = mode === "sync" || mode === "race";
        const wasmPath =
          new URL("./src/pkg/zrip_wasm_bg.wasm", import.meta.url).pathname;
        const run = await new Deno.Command(Deno.execPath(), {
          args: [
            "run",
            "--no-config",
            "--no-prompt",
            ...(sync ? [`--allow-read=${wasmPath}`] : []),
            output,
            mode,
            ...(sync ? [wasmPath] : []),
          ],
          cwd: directory,
        }).output();
        assertEquals(run.code, 0, new TextDecoder().decode(run.stderr));
        assertEquals(
          new TextDecoder().decode(run.stdout).trim(),
          "roundtrip passed",
        );
      });
    }
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});
