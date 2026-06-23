use wasm_bindgen::prelude::*;

// === One-shot functions ===

#[wasm_bindgen]
pub fn compress(input: &[u8], level: i32) -> Result<Vec<u8>, JsError> {
    zrip::compress(input, level).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen]
pub fn decompress(input: &[u8]) -> Result<Vec<u8>, JsError> {
    zrip::decompress(input).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "compressBound")]
pub fn compress_bound(input_len: usize) -> usize {
    zrip::compress_bound(input_len)
}

// === Dictionary ===

#[wasm_bindgen(js_name = "Dictionary")]
pub struct ZstdDictionary {
    inner: zrip::Dictionary,
}

#[wasm_bindgen(js_class = "Dictionary")]
impl ZstdDictionary {
    #[wasm_bindgen(constructor)]
    pub fn new(data: &[u8]) -> Result<ZstdDictionary, JsError> {
        zrip::Dictionary::from_bytes(data)
            .map(|d| ZstdDictionary { inner: d })
            .map_err(|e| JsError::new(&e.to_string()))
    }

    #[wasm_bindgen(getter)]
    pub fn id(&self) -> u32 {
        self.inner.id()
    }
}

// === One-shot with dict ===

#[wasm_bindgen(js_name = "compressWithDict")]
pub fn compress_with_dict(
    input: &[u8],
    level: i32,
    dict: &ZstdDictionary,
) -> Result<Vec<u8>, JsError> {
    zrip::compress_with_dict(input, level, &dict.inner)
        .map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "decompressWithDict")]
pub fn decompress_with_dict(input: &[u8], dict: &ZstdDictionary) -> Result<Vec<u8>, JsError> {
    zrip::decompress_with_dict(input, &dict.inner)
        .map_err(|e| JsError::new(&e.to_string()))
}

// === Stateful Compressor ===

#[wasm_bindgen]
pub struct Compressor {
    ctx: zrip::CompressContext,
}

#[wasm_bindgen]
impl Compressor {
    #[wasm_bindgen(constructor)]
    pub fn new(level: i32) -> Result<Compressor, JsError> {
        zrip::CompressContext::new(level)
            .map(|ctx| Compressor { ctx })
            .map_err(|e| JsError::new(&e.to_string()))
    }

    #[wasm_bindgen(js_name = "withDict")]
    pub fn with_dict(level: i32, dict: &ZstdDictionary) -> Result<Compressor, JsError> {
        zrip::CompressContext::with_dict(level, dict.inner.clone())
            .map(|ctx| Compressor { ctx })
            .map_err(|e| JsError::new(&e.to_string()))
    }

    pub fn compress(&mut self, input: &[u8]) -> Result<Vec<u8>, JsError> {
        self.ctx
            .compress(input)
            .map(|cow| cow.into_owned())
            .map_err(|e| JsError::new(&e.to_string()))
    }

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

#[wasm_bindgen]
pub struct Decompressor {
    ctx: zrip::DecompressContext,
}

#[wasm_bindgen]
impl Decompressor {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Decompressor {
        Decompressor {
            ctx: zrip::DecompressContext::new(),
        }
    }

    #[wasm_bindgen(js_name = "withDict")]
    pub fn with_dict(dict: &ZstdDictionary) -> Decompressor {
        Decompressor {
            ctx: zrip::DecompressContext::with_dict(dict.inner.clone()),
        }
    }

    pub fn decompress(&mut self, input: &[u8]) -> Result<Vec<u8>, JsError> {
        self.ctx
            .decompress(input)
            .map(|cow| cow.into_owned())
            .map_err(|e| JsError::new(&e.to_string()))
    }
}
