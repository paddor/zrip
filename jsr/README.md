# @paddor/zrip

Pure Rust zstd codec compiled to WebAssembly. Decodes standard zstd blocks and
frames produced at any compression level. Encodes levels -8 through 4 for fast
transfer pipelines. First-class dictionary support.

Automatically detects WASM SIMD support and loads the appropriate binary.

## Usage

```ts
import { compress, decompress, init } from "@paddor/zrip";

await init();

const data = new TextEncoder().encode("hello world".repeat(1000));
const compressed = compress(data, 1);
const original = decompress(compressed);
```

### Compression levels

|  Level | Strategy | Notes                                |
| -----: | :------- | :----------------------------------- |
|     -8 | Fast     | zrip-specific, fastest, lowest ratio |
|     -7 | Fast     |                                      |
| -6..-1 | Fast     |                                      |
|      0 |          | Alias for level 1                    |
|      1 | Fast     | Default                              |
|      2 | Fast     |                                      |
|      3 | DFast    | Dual hash tables                     |
|      4 | DFast    | Best ratio                           |

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
const original = decompressWithDict(compressed, dict);

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

const wasmBytes = Deno.readFileSync("path/to/zrip_simd.wasm");
initSyncFromBytes(wasmBytes);

const compressed = compress(data);
```

## Performance

WASM benchmark at level 1 (geomean across Silesia corpus, 4 MiB cap per file):

| Impl                | Encode MB/s | Decode MB/s |
| :------------------ | ----------: | ----------: |
| zrip                |   **153.6** |   **306.1** |
| C zstd (zstd-codec) |       133.7 |       269.5 |

zrip WASM is 15% faster encode, 14% faster decode than C zstd compiled to WASM
via Emscripten.

## Source

Rust source and native benchmarks:
[github.com/paddor/zrip](https://github.com/paddor/zrip)
