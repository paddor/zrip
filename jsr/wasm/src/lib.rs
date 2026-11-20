use wasm_bindgen::prelude::*;

/// `max_output_size` from JS, or no limit when it is absent.
fn output_limit(max_output_size: Option<usize>) -> usize {
    max_output_size.unwrap_or(zrip::DEFAULT_DECOMPRESS_LIMIT)
}

// === One-shot functions ===

/// Compresses `input` into one zstd frame at `level` (-7 to 4; 0 means 1).
#[wasm_bindgen]
pub fn compress(input: &[u8], level: i32) -> Result<Vec<u8>, JsError> {
    zrip::compress(input, level).map_err(|e| JsError::new(&e.to_string()))
}

/// Decompresses all frames in `input`. Throws if the output would exceed
/// `max_output_size` bytes.
#[wasm_bindgen]
pub fn decompress(input: &[u8], max_output_size: Option<usize>) -> Result<Vec<u8>, JsError> {
    zrip::decompress_with_limit(input, output_limit(max_output_size))
        .map_err(|e| JsError::new(&e.to_string()))
}

/// Largest frame `compress` can produce for `input_len` bytes.
#[wasm_bindgen(js_name = "compressBound")]
pub fn compress_bound(input_len: usize) -> usize {
    zrip::compress_bound(input_len)
}

// === Dictionary ===

/// A parsed zstd dictionary, as `zstd --train` writes it.
#[wasm_bindgen(js_name = "Dictionary")]
pub struct ZstdDictionary {
    inner: zrip::Dictionary,
}

#[wasm_bindgen(js_class = "Dictionary")]
impl ZstdDictionary {
    /// Parses dictionary bytes. Throws if they are not a valid dictionary.
    #[wasm_bindgen(constructor)]
    pub fn new(data: &[u8]) -> Result<ZstdDictionary, JsError> {
        zrip::Dictionary::from_bytes(data)
            .map(|d| ZstdDictionary { inner: d })
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// The dictionary ID that frames compressed with it carry.
    #[wasm_bindgen(getter)]
    pub fn id(&self) -> u32 {
        self.inner.id()
    }
}

// === One-shot with dict ===

/// `compress` with `dict`. Decoders need the same dictionary.
#[wasm_bindgen(js_name = "compressWithDict")]
pub fn compress_with_dict(
    input: &[u8],
    level: i32,
    dict: &ZstdDictionary,
) -> Result<Vec<u8>, JsError> {
    zrip::compress_with_dict(input, level, &dict.inner)
        .map_err(|e| JsError::new(&e.to_string()))
}

/// `decompress` for frames compressed with `dict`.
#[wasm_bindgen(js_name = "decompressWithDict")]
pub fn decompress_with_dict(
    input: &[u8],
    dict: &ZstdDictionary,
    max_output_size: Option<usize>,
) -> Result<Vec<u8>, JsError> {
    zrip::decompress_with_dict_and_limit(input, &dict.inner, output_limit(max_output_size))
        .map_err(|e| JsError::new(&e.to_string()))
}

// === Stateful Compressor ===

/// Compression context that reuses its tables and buffers across calls.
#[wasm_bindgen]
pub struct Compressor {
    ctx: zrip::CompressContext,
}

#[wasm_bindgen]
impl Compressor {
    /// Creates a context for `level` (-7 to 4; 0 means 1).
    #[wasm_bindgen(constructor)]
    pub fn new(level: i32) -> Result<Compressor, JsError> {
        zrip::CompressContext::new(level)
            .map(|ctx| Compressor { ctx })
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Creates a context that compresses every input with `dict`.
    #[wasm_bindgen(js_name = "withDict")]
    pub fn with_dict(level: i32, dict: &ZstdDictionary) -> Result<Compressor, JsError> {
        zrip::CompressContext::with_dict(level, dict.inner.clone())
            .map(|ctx| Compressor { ctx })
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Compresses `input` into one zstd frame.
    pub fn compress(&mut self, input: &[u8]) -> Result<Vec<u8>, JsError> {
        self.ctx
            .compress(input)
            .map(|cow| cow.into_owned())
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Compresses `input` with `dict` instead of the context dictionary.
    #[wasm_bindgen(js_name = "compressWithDict")]
    pub fn compress_with_dict(
        &mut self,
        input: &[u8],
        dict: &ZstdDictionary,
    ) -> Result<Vec<u8>, JsError> {
        self.ctx
            .compress_with_dict(input, &dict.inner)
            .map(|cow| cow.into_owned())
            .map_err(|e| JsError::new(&e.to_string()))
    }
}

// === Stateful Decompressor ===

/// Decompression context that reuses its buffers across calls.
#[wasm_bindgen]
pub struct Decompressor {
    ctx: zrip::DecompressContext,
}

impl Default for Decompressor {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl Decompressor {
    /// Creates a context without a dictionary.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Decompressor {
        Decompressor {
            ctx: zrip::DecompressContext::new(),
        }
    }

    /// Creates a context for frames compressed with `dict`.
    #[wasm_bindgen(js_name = "withDict")]
    pub fn with_dict(dict: &ZstdDictionary) -> Decompressor {
        Decompressor {
            ctx: zrip::DecompressContext::with_dict(dict.inner.clone()),
        }
    }

    /// Decompresses all frames in `input`. Throws if the output would exceed
    /// `max_output_size` bytes.
    pub fn decompress(
        &mut self,
        input: &[u8],
        max_output_size: Option<usize>,
    ) -> Result<Vec<u8>, JsError> {
        self.ctx
            .decompress_with_limit(input, output_limit(max_output_size))
            .map(|cow| cow.into_owned())
            .map_err(|e| JsError::new(&e.to_string()))
    }
}
