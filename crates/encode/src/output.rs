//! Where frame writers put their bytes: a growable `Vec` or a fixed slice.
#![forbid(unsafe_code)]

use zrip_core::error::CompressError;

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

/// A frame byte sink. A full slice fails with
/// `CompressError::OutputTooSmall`; a `Vec` grows.
pub(crate) trait OutputSink {
    fn push(&mut self, byte: u8) -> Result<(), CompressError>;
    fn extend_from_slice(&mut self, data: &[u8]) -> Result<(), CompressError>;

    /// Reserves room for at least `additional` more bytes, where the sink can
    /// grow.
    #[cfg(feature = "alloc")]
    #[inline]
    fn reserve(&mut self, _additional: usize) {}
}

#[cfg(feature = "alloc")]
impl OutputSink for Vec<u8> {
    #[inline]
    fn push(&mut self, byte: u8) -> Result<(), CompressError> {
        Vec::push(self, byte);
        Ok(())
    }

    #[inline]
    fn extend_from_slice(&mut self, data: &[u8]) -> Result<(), CompressError> {
        Vec::extend_from_slice(self, data);
        Ok(())
    }

    #[cfg(feature = "alloc")]
    #[inline]
    fn reserve(&mut self, additional: usize) {
        Vec::reserve(self, additional);
    }
}

/// Writes to the start of a caller's slice.
pub(crate) struct SliceSink<'a> {
    output: &'a mut [u8],
    pos: usize,
}

impl<'a> SliceSink<'a> {
    #[inline]
    pub(crate) fn new(output: &'a mut [u8]) -> Self {
        Self { output, pos: 0 }
    }

    /// Bytes written so far.
    #[inline]
    pub(crate) fn pos(&self) -> usize {
        self.pos
    }
}

impl OutputSink for SliceSink<'_> {
    #[inline]
    fn push(&mut self, byte: u8) -> Result<(), CompressError> {
        if self.pos >= self.output.len() {
            return Err(CompressError::OutputTooSmall);
        }
        self.output[self.pos] = byte;
        self.pos += 1;
        Ok(())
    }

    #[inline]
    fn extend_from_slice(&mut self, data: &[u8]) -> Result<(), CompressError> {
        let end = self
            .pos
            .checked_add(data.len())
            .ok_or(CompressError::OutputTooSmall)?;
        if end > self.output.len() {
            return Err(CompressError::OutputTooSmall);
        }
        self.output[self.pos..end].copy_from_slice(data);
        self.pos = end;
        Ok(())
    }
}
