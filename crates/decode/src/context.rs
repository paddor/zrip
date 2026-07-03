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
/// ```no_run
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

    /// Decompresses one zstd frame whose 4-byte magic number is stored out of band.
    ///
    /// OpenZL stores zstd payloads this way inside transform streams.
    pub fn decompress_after_magic_with_limit(
        &mut self,
        input: &[u8],
        max_output: usize,
    ) -> Result<Cow<'_, [u8]>, DecompressError> {
        self.output.clear();
        super::decompress_frame_after_magic(
            input,
            &mut self.output,
            max_output,
            self.dict.as_ref(),
            &mut self.ws,
        )?;
        if self.output.len() >= zrip_core::LARGE_OUTPUT_THRESHOLD {
            Ok(Cow::Owned(core::mem::take(&mut self.output)))
        } else {
            Ok(Cow::Borrowed(&self.output))
        }
    }

    /// Decompresses one zstd frame without its magic number into `output`.
    ///
    /// Appends to `output` and returns the number of bytes written.
    pub fn decompress_after_magic_into(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
        max_output: usize,
    ) -> Result<usize, DecompressError> {
        let start = output.len();
        super::decompress_frame_after_magic(
            input,
            output,
            max_output,
            self.dict.as_ref(),
            &mut self.ws,
        )?;
        Ok(output.len() - start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn push_block_header(out: &mut Vec<u8>, last: bool, block_type: u32, block_size: usize) {
        let raw = ((block_size as u32) << 3) | (block_type << 1) | u32::from(last);
        out.push(raw as u8);
        out.push((raw >> 8) as u8);
        out.push((raw >> 16) as u8);
    }

    #[test]
    fn decompress_after_magic_into_appends_output() {
        let mut frame = Vec::new();
        frame.push(0x20);
        frame.push(5);
        push_block_header(&mut frame, true, 0, 5);
        frame.extend_from_slice(b"hello");

        let mut ctx = DecompressContext::new();
        let mut output = b"prefix".to_vec();
        let written = ctx
            .decompress_after_magic_into(&frame, &mut output, usize::MAX)
            .unwrap();
        assert_eq!(written, 5);
        assert_eq!(output, b"prefixhello");
    }
}
