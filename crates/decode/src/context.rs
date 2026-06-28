#[cfg(feature = "alloc")]
use alloc::borrow::Cow;
#[cfg(feature = "alloc")]
use alloc::boxed::Box;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::BlockDecodeWorkspace;
use zrip_core::dict::Dictionary;
use zrip_core::error::DecompressError;

/// Reusable decompression context that amortizes buffer allocations.
///
/// Holds internal buffers (output, Huffman/FSE workspace) across calls.
/// Useful when decompressing many small frames in a loop.
///
/// ```
/// let data = b"repeated decompression".repeat(100);
/// let compressed = zrip::compress(&data, 1).unwrap();
///
/// let mut ctx = zrip::DecompressContext::new();
/// for _ in 0..10 {
///     let output = ctx.decompress(&compressed).unwrap();
///     assert_eq!(&*output, &data[..]);
/// }
/// ```
pub struct DecompressContext {
    dict: Option<Dictionary>,
    output: Vec<u8>,
    ws: Box<BlockDecodeWorkspace>,
}

impl Default for DecompressContext {
    fn default() -> Self {
        Self::new()
    }
}

impl DecompressContext {
    /// Creates a new context without a dictionary.
    pub fn new() -> Self {
        Self {
            dict: None,
            output: Vec::new(),
            ws: Box::new(BlockDecodeWorkspace::new()),
        }
    }

    /// Creates a new context with a pre-loaded dictionary.
    pub fn with_dict(dict: Dictionary) -> Self {
        let mut ws = Box::new(BlockDecodeWorkspace::new());
        ws.cache_dict(&dict);
        Self {
            dict: Some(dict),
            output: Vec::new(),
            ws,
        }
    }

    /// Decompresses `input` using [`DEFAULT_DECOMPRESS_LIMIT`](zrip_core::DEFAULT_DECOMPRESS_LIMIT).
    pub fn decompress(&mut self, input: &[u8]) -> Result<Cow<'_, [u8]>, DecompressError> {
        self.decompress_with_limit(input, zrip_core::DEFAULT_DECOMPRESS_LIMIT)
    }

    /// Decompresses `input` with an explicit output size limit.
    ///
    /// Returns [`DecompressError::OutputTooSmall`] if the decompressed output
    /// would exceed `max_output` bytes.
    pub fn decompress_with_limit(
        &mut self,
        input: &[u8],
        max_output: usize,
    ) -> Result<Cow<'_, [u8]>, DecompressError> {
        self.output.clear();
        let dict_ref = self.dict.as_ref();
        let mut offset = 0;
        while offset < input.len() {
            let remaining = &input[offset..];
            if let Some(skip_len) = super::skip_skippable_frame(remaining) {
                offset += skip_len;
                continue;
            }
            let consumed = super::decompress_frame(
                remaining,
                &mut self.output,
                max_output,
                dict_ref,
                &mut self.ws,
            )?;
            offset += consumed;
        }
        if self.output.len() >= zrip_core::LARGE_OUTPUT_THRESHOLD {
            Ok(Cow::Owned(core::mem::take(&mut self.output)))
        } else {
            Ok(Cow::Borrowed(&self.output))
        }
    }
}
