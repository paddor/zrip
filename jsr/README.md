# @paddor/zrip

Pure Rust zstd codec compiled to WebAssembly. Decodes standard zstd blocks and
frames produced at any compression level. Encodes levels -7 through 4 for fast
transfer pipelines. First-class dictionary support.

Automatically detects WASM SIMD support and loads the appropriate binary.

## Usage

```ts
import { compress, decompress, init } from "@paddor/zrip";

await init();

const data = new TextEncoder().encode("hello world".repeat(1000));
const compressed = compress(data, 1);
const original = decompress(compressed);
const bounded = decompress(compressed, { maxDecompressedSize: data.length });
```

### Compression levels

|  Level | Strategy | Notes                                  |
| -----: | :------- | :------------------------------------- |
|     -7 | Fast     | Fastest; raw literals and long matches |
| -6..-1 | Fast     | Fast encoding with Huffman literals    |
|      0 |          | Alias for level 1                      |
|      1 | Fast     | Default                                |
|      2 | Fast     |                                        |
|      3 | DFast    | Dual hash tables                       |
|      4 | DFast    | Best ratio                             |

### Reusable contexts

Amortize internal allocations across multiple compress/decompress calls:

```ts
import { Compressor, Decompressor, init } from "@paddor/zrip";

await init();

const compressor = new Compressor(1);
const c1 = compressor.compress(data1);
const c2 = compressor.compress(data2);
compressor.free();

const decompressor = new Decompressor();
const d1 = decompressor.decompress(c1);
const d2 = decompressor.decompress(c2);
decompressor.free();
```

### Dictionary compression

For small-message workloads (log lines, JSON records, RPC payloads) that share
common structure:

```ts
import {
  Compressor,
  compressWithDict,
  Decompressor,
  decompressWithDict,
  Dictionary,
  init,
} from "@paddor/zrip";

await init();

const dict = new Dictionary(dictBytes);
const compressed = compressWithDict(data, 1, dict);
const original = decompressWithDict(compressed, dict, {
  maxDecompressedSize: data.length,
});

// Stateful with dict
const compressor = Compressor.withDict(1, dict);
const c = compressor.compress(data);
compressor.free();

const decompressor = Decompressor.withDict(dict);
const d = decompressor.decompress(c);
decompressor.free();
```

### Synchronous initialization

When you have pre-loaded WASM bytes (e.g. bundled or read from disk):

```ts
import { compress, initSyncFromBytes } from "@paddor/zrip";

const wasmBytes = Deno.readFileSync("path/to/codec.wasm");
initSyncFromBytes(wasmBytes);

const compressed = compress(data);
```

## Performance

Level 1 benchmark through the JavaScript API, using the first 4 MiB of each of
the 12 Silesia files. Geometric mean, decimal MB/s. Measured on 2026-11-18 with
Deno 2.8.3, SIMD128, and an Intel Core i7-8700B Linux VM.

| Impl                      | Encode MB/s | Decode MB/s |
| :------------------------ | ----------: | ----------: |
| zrip                      |   **196.6** |   **413.9** |
| C zstd (zstd-codec 0.1.5) |       129.7 |       270.3 |

Run `deno run --allow-read src/bench.ts` from `jsr/` after building the WASM
package to reproduce this comparison. The
[WASI charts](https://github.com/paddor/zrip#performance) measure full files in
wasmtime.

## Source

Rust source and native benchmarks:
[github.com/paddor/zrip](https://github.com/paddor/zrip)
