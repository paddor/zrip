# @paddor/zrip

Pure Rust zstd codec compiled to WebAssembly. Decodes standard zstd blocks and
frames produced at any compression level. Encodes levels -8 through 4 for fast
transfer pipelines. First-class dictionary support.

Automatically detects WASM SIMD support and loads the appropriate binary.

## Usage

```js
import { compress, decompress, init } from "@paddor/zrip";

await init();

const data = new TextEncoder().encode("hello world".repeat(1000));
const compressed = compress(data, 1);
const original = decompress(compressed);
const bounded = decompress(compressed, { maxDecompressedSize: data.length });
```

## Source

Rust source and native benchmarks:
[github.com/paddor/zrip](https://github.com/paddor/zrip)
