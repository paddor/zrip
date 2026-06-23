import { init, compress, decompress } from "./mod.ts";
import { ZstdCodec } from "npm:zstd-codec";

function bench(
  fn: () => void,
  warmup: number,
  iters: number,
): number {
  for (let i = 0; i < warmup; i++) fn();
  const t0 = performance.now();
  for (let i = 0; i < iters; i++) fn();
  const elapsed = performance.now() - t0;
  return elapsed / iters;
}

interface CZstd {
  compress(data: Uint8Array, level: number): Uint8Array;
  decompress(data: Uint8Array): Uint8Array;
}

async function initCZstd(): Promise<CZstd> {
  return new Promise((resolve) => {
    ZstdCodec.run((zstd: { Simple: new () => CZstd }) => {
      resolve(new zstd.Simple());
    });
  });
}

const CORPUS_DIR = "../bench/corpus";

const FILES = [
  "dickens.txt",
  "hdfs.json",
  "reymont.pdf",
  "xml_collection.xml",
  "silesia/mr",
  "silesia/mozilla",
  "silesia/nci",
  "silesia/osdb",
  "silesia/samba",
  "silesia/sao",
  "silesia/webster",
  "silesia/x-ray",
];

function fmtSize(bytes: number): string {
  if (bytes >= 1024 * 1024) return (bytes / 1024 / 1024).toFixed(1) + " MB";
  return (bytes / 1024).toFixed(0) + " KB";
}

async function main() {
  await init();
  const czstd = await initCZstd();
  const level = 1;

  console.log(
    `${"File".padEnd(22)} ${"Size".padStart(9)}  ` +
      `${"Impl".padEnd(10)} ${"Ratio".padStart(6)} ` +
      `${"Enc MB/s".padStart(10)} ${"Dec MB/s".padStart(10)}`,
  );
  console.log("-".repeat(78));

  const zripEncs: number[] = [];
  const czstdEncs: number[] = [];
  const zripDecs: number[] = [];
  const czstdDecs: number[] = [];
  const sizes: number[] = [];

  for (const file of FILES) {
    const path = `${CORPUS_DIR}/${file}`;
    const full = await Deno.readFile(path);
    const data = full.length > 4 * 1024 * 1024 ? full.slice(0, 4 * 1024 * 1024) : full;
    const name = file.replace("silesia/", "");
    sizes.push(data.length);

    const zripCompressed = compress(data, level);

    let czstdCompressed: Uint8Array | null = null;
    try {
      czstdCompressed = czstd.compress(data, level);
    } catch {
      // Emscripten OOM on large files
    }

    const zripRatio = zripCompressed.length / data.length;

    // Scale iterations to file size
    const iters = Math.max(5, Math.floor(50_000_000 / data.length));
    const warmup = Math.max(2, Math.floor(iters / 4));

    const mbps = (bytes: number, ms: number) =>
      (bytes / 1024 / 1024) / (ms / 1000);

    const zripEncMs = bench(() => compress(data, level), warmup, iters);
    const zripDecMs = bench(() => decompress(zripCompressed), warmup, iters);
    const zEnc = mbps(data.length, zripEncMs);
    const zDec = mbps(data.length, zripDecMs);
    zripEncs.push(zEnc);
    zripDecs.push(zDec);

    console.log(
      `${name.padEnd(22)} ${fmtSize(data.length).padStart(9)}  ` +
        `${"zrip".padEnd(10)} ${zripRatio.toFixed(3).padStart(6)} ` +
        `${zEnc.toFixed(1).padStart(10)} ${zDec.toFixed(1).padStart(10)}`,
    );

    if (czstdCompressed) {
      const czstdRatio = czstdCompressed.length / data.length;
      const czstdEncMs = bench(() => czstd.compress(data, level), warmup, iters);
      const czstdDecMs = bench(
        () => czstd.decompress(czstdCompressed!),
        warmup,
        iters,
      );
      const cEnc = mbps(data.length, czstdEncMs);
      const cDec = mbps(data.length, czstdDecMs);
      czstdEncs.push(cEnc);
      czstdDecs.push(cDec);

      console.log(
        `${"".padEnd(22)} ${"".padStart(9)}  ` +
          `${"C zstd".padEnd(10)} ${czstdRatio.toFixed(3).padStart(6)} ` +
          `${cEnc.toFixed(1).padStart(10)} ${cDec.toFixed(1).padStart(10)}`,
      );
    } else {
      console.log(
        `${"".padEnd(22)} ${"".padStart(9)}  ` +
          `${"C zstd".padEnd(10)} ${"OOM".padStart(6)}`,
      );
    }
    console.log("");
  }

  // Geometric means
  const geomean = (arr: number[]) =>
    Math.exp(arr.reduce((s, v) => s + Math.log(v), 0) / arr.length);

  console.log("-".repeat(78));
  console.log(
    `${"GEOMEAN".padEnd(22)} ${"".padStart(9)}  ` +
      `${"zrip".padEnd(10)} ${"".padStart(6)} ` +
      `${geomean(zripEncs).toFixed(1).padStart(10)} ${geomean(zripDecs).toFixed(1).padStart(10)}`,
  );
  console.log(
    `${"".padEnd(22)} ${"".padStart(9)}  ` +
      `${"C zstd".padEnd(10)} ${"".padStart(6)} ` +
      `${geomean(czstdEncs).toFixed(1).padStart(10)} ${geomean(czstdDecs).toFixed(1).padStart(10)}`,
  );
}

main();
