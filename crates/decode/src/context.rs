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
/// Useful when decompressing many small frames in a loop. Needs only the
/// `alloc` feature, so it also works in `no_std` builds.
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
        decompress_into_with_workspace(
            input,
            &mut self.output,
            max_output,
            self.dict.as_ref(),
            &mut self.ws,
        )?;
        Ok(Cow::Borrowed(&self.output))
    }

    /// Decompresses `input` into a caller-owned output buffer while reusing
    /// this context's decoder tables and workspace.
    ///
    /// Output is appended to `output`. The returned value is the number of
    /// bytes written by this call. Unlike [`Self::decompress`], the result can
    /// outlive the next use of this context without copying.
    pub fn decompress_into(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<usize, DecompressError> {
        self.decompress_into_with_limit(input, output, zrip_core::DEFAULT_DECOMPRESS_LIMIT)
    }

    /// Decompresses `input` into a caller-owned output buffer with an explicit
    /// limit on the number of bytes appended by this call.
    ///
    /// This retains decoder workspace across calls without retaining ownership
    /// of the decoded bytes. It is useful for pipelines that move each decoded
    /// buffer to another task before decoding the next frame.
    pub fn decompress_into_with_limit(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
        max_output: usize,
    ) -> Result<usize, DecompressError> {
        decompress_into_with_workspace(input, output, max_output, self.dict.as_ref(), &mut self.ws)
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
        Ok(Cow::Borrowed(&self.output))
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

fn decompress_into_with_workspace(
    input: &[u8],
    output: &mut Vec<u8>,
    max_output: usize,
    dict: Option<&Dictionary>,
    ws: &mut BlockDecodeWorkspace,
) -> Result<usize, DecompressError> {
    let start = output.len();
    let mut offset = 0;

    // Keep the common single-frame path from reparsing the magic number.
    if input.len() >= 4 {
        let magic = u32::from_le_bytes([input[0], input[1], input[2], input[3]]);
        if magic == zrip_core::frame::ZSTD_MAGIC {
            offset =
                4 + super::decompress_frame_after_magic(&input[4..], output, max_output, dict, ws)?;
        }
    }

    while offset < input.len() {
        let remaining = &input[offset..];
        if let Some(skip_len) = super::skip_skippable_frame(remaining) {
            offset += skip_len;
            continue;
        }
        let frame_limit = super::remaining_output_limit(output.len(), start, max_output)?;
        let consumed = super::decompress_frame(remaining, output, frame_limit, dict, ws)?;
        offset += consumed;
    }

    Ok(output.len() - start)
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

    /// A malformed frame without its magic, found by fuzzing ozlrip. Its
    /// first decode fails with `InvalidOffset`.
    const MALFORMED_FRAME_AFTER_MAGIC: [u8; 47] = [
        0x00, 0x00, 0x0a, 0x43, 0x00, 0x00, 0x7c, 0x00, 0x00, 0xd5, 0x31, 0xd7, 0xb1, 0x00, 0x04,
        0x00, 0x00, 0xd7, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x16, 0x00, 0x24,
        0x00, 0x00, 0x35, 0x01, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x7c, 0x00, 0x00, 0x11, 0x00,
        0xd7, 0x09,
    ];

    #[test]
    fn failed_frame_does_not_change_the_next_decode() {
        let fresh = DecompressContext::new()
            .decompress_after_magic_into(&MALFORMED_FRAME_AFTER_MAGIC, &mut Vec::new(), usize::MAX)
            .unwrap_err();

        let mut ctx = DecompressContext::new();
        for attempt in 0..3 {
            let err = ctx
                .decompress_after_magic_into(
                    &MALFORMED_FRAME_AFTER_MAGIC,
                    &mut Vec::new(),
                    usize::MAX,
                )
                .unwrap_err();
            assert_eq!(err, fresh, "attempt {attempt}");
        }
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

    #[test]
    fn decompress_fast_path_continues_after_first_frame() {
        fn raw_frame(bytes: &[u8]) -> Vec<u8> {
            let mut frame = Vec::new();
            frame.extend_from_slice(&zrip_core::frame::ZSTD_MAGIC.to_le_bytes());
            frame.push(0x20);
            frame.push(bytes.len() as u8);
            push_block_header(&mut frame, true, 0, bytes.len());
            frame.extend_from_slice(bytes);
            frame
        }

        let mut stream = raw_frame(b"hello");
        stream.extend_from_slice(&raw_frame(b"there"));

        let mut ctx = DecompressContext::new();
        let output = ctx.decompress(&stream).unwrap();
        assert_eq!(&*output, b"hellothere");
    }

    #[test]
    fn decompress_into_appends_caller_owned_output_across_calls() {
        fn raw_frame(bytes: &[u8]) -> Vec<u8> {
            let mut frame = Vec::new();
            frame.extend_from_slice(&zrip_core::frame::ZSTD_MAGIC.to_le_bytes());
            frame.push(0x20);
            frame.push(bytes.len() as u8);
            push_block_header(&mut frame, true, 0, bytes.len());
            frame.extend_from_slice(bytes);
            frame
        }

        let mut first_stream = raw_frame(b"hello");
        first_stream.extend_from_slice(&raw_frame(b"there"));

        let mut ctx = DecompressContext::new();
        let mut first = b"prefix:".to_vec();
        assert_eq!(ctx.decompress_into(&first_stream, &mut first).unwrap(), 10);
        assert_eq!(first, b"prefix:hellothere");

        let mut second = Vec::new();
        assert_eq!(
            ctx.decompress_into(&raw_frame(b"again"), &mut second)
                .unwrap(),
            5
        );
        assert_eq!(second, b"again");
    }

    #[test]
    fn decompress_into_limit_counts_only_new_output() {
        let mut frame = Vec::new();
        frame.extend_from_slice(&zrip_core::frame::ZSTD_MAGIC.to_le_bytes());
        frame.push(0x20);
        frame.push(5);
        push_block_header(&mut frame, true, 0, 5);
        frame.extend_from_slice(b"hello");

        let mut ctx = DecompressContext::new();
        let mut output = b"existing".to_vec();
        assert!(matches!(
            ctx.decompress_into_with_limit(&frame, &mut output, 4),
            Err(DecompressError::OutputTooSmall)
        ));
    }
}
