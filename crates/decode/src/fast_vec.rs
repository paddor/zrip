#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use zrip_core::error::DecompressError;
use zrip_core::hint::unlikely;

const WILDCOPY_OVERLENGTH: usize = 64;

pub(crate) struct BlockOutput<'a> {
    vec: &'a mut Vec<u8>,
    block_limit: usize,
}

pub(crate) struct SequenceOutput<'block, 'vec> {
    output: &'block mut BlockOutput<'vec>,
    literal_len: usize,
    match_len: usize,
}

impl<'a> BlockOutput<'a> {
    #[inline(always)]
    pub(crate) fn new(vec: &'a mut Vec<u8>, max_block_size: usize) -> Self {
        vec.reserve(max_block_size + WILDCOPY_OVERLENGTH);
        let block_limit = vec.len() + max_block_size;
        Self { vec, block_limit }
    }

    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        self.vec.len()
    }

    #[inline(always)]
    pub(crate) fn begin_sequence(
        &mut self,
        literal_len: usize,
        match_len: usize,
    ) -> Result<SequenceOutput<'_, 'a>, DecompressError> {
        debug_assert!(self.vec.len() <= self.block_limit);
        let remaining = self.block_limit - self.vec.len();
        if unlikely(literal_len > remaining || match_len > remaining - literal_len) {
            return Err(DecompressError::CorruptSequences);
        }
        Ok(SequenceOutput {
            output: self,
            literal_len,
            match_len,
        })
    }

    #[inline(always)]
    fn ensure_block_space(&self, len: usize) -> Result<(), DecompressError> {
        debug_assert!(self.vec.len() <= self.block_limit);
        if unlikely(len > self.block_limit - self.vec.len()) {
            return Err(DecompressError::CorruptSequences);
        }
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn extend_literals_range(
        &mut self,
        src: &[u8],
        start: usize,
        len: usize,
    ) -> Result<(), DecompressError> {
        self.extend_slice_range(src, start, len, DecompressError::CorruptLiterals)
    }

    #[inline(always)]
    fn extend_slice_range(
        &mut self,
        src: &[u8],
        start: usize,
        len: usize,
        bounds_error: DecompressError,
    ) -> Result<(), DecompressError> {
        if unlikely(start > src.len() || len > src.len() - start) {
            return Err(bounds_error);
        }
        self.ensure_block_space(len)?;
        self.extend_slice_range_in_block(src, start, len);
        Ok(())
    }

    #[inline(always)]
    fn extend_slice_range_in_block(&mut self, src: &[u8], start: usize, len: usize) {
        if len == 0 {
            return;
        }
        #[cfg(not(feature = "paranoid"))]
        unsafe {
            debug_assert!(self.vec.len() + len + WILDCOPY_OVERLENGTH <= self.vec.capacity());
            // SAFETY: The caller proves `src[start..start + len]` is readable
            // and that this write stays in the reserved block output range.
            fast_extend_from_ptr(self.vec, src.as_ptr().add(start), len);
        }
        #[cfg(feature = "paranoid")]
        {
            self.vec.extend_from_slice(&src[start..start + len]);
        }
    }
}

impl SequenceOutput<'_, '_> {
    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        self.output.len()
    }

    #[inline(always)]
    pub(crate) fn extend_literals_range(
        &mut self,
        src: &[u8],
        start: usize,
    ) -> Result<(), DecompressError> {
        self.extend_slice_range(
            src,
            start,
            self.literal_len,
            DecompressError::CorruptLiterals,
        )
    }

    #[inline(always)]
    fn extend_slice_range(
        &mut self,
        src: &[u8],
        start: usize,
        len: usize,
        bounds_error: DecompressError,
    ) -> Result<(), DecompressError> {
        if unlikely(start > src.len() || len > src.len() - start) {
            return Err(bounds_error);
        }
        self.output.extend_slice_range_in_block(src, start, len);
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn copy_match(&mut self, offset: usize) -> Result<(), DecompressError> {
        self.copy_match_len(offset, self.match_len)
    }

    #[inline(always)]
    pub(crate) fn copy_match_16plus(&mut self, offset: usize) -> Result<(), DecompressError> {
        if unlikely(offset < 16 || offset > self.output.vec.len()) {
            return Err(DecompressError::InvalidOffset);
        }
        let len = self.match_len;
        if len == 0 {
            return Ok(());
        }

        #[cfg(not(feature = "paranoid"))]
        unsafe {
            debug_assert!(
                self.output.vec.len() + len + WILDCOPY_OVERLENGTH <= self.output.vec.capacity()
            );
            // SAFETY: `begin_sequence` proves capacity for the whole sequence.
            // The offset check proves an initialized source, and offset >= 16
            // is the precondition for this wider wildcopy path.
            wild_copy_match_16plus_unchecked(self.output.vec, offset, len);
        }
        #[cfg(feature = "paranoid")]
        {
            wild_copy_match_paranoid(self.output.vec, offset, len);
        }
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn copy_match_single(&mut self, offset: usize) -> Result<(), DecompressError> {
        if unlikely(offset == 0 || offset > self.output.vec.len()) {
            return Err(DecompressError::InvalidOffset);
        }
        let len = self.match_len;
        if len == 0 {
            return Ok(());
        }

        #[cfg(not(feature = "paranoid"))]
        unsafe {
            debug_assert!(
                self.output.vec.len() + len + WILDCOPY_OVERLENGTH <= self.output.vec.capacity()
            );
            // SAFETY: `begin_sequence` proves capacity for the whole sequence,
            // and the offset check proves an initialized source.
            wild_copy_match_single_unchecked(self.output.vec, offset, len);
        }
        #[cfg(feature = "paranoid")]
        {
            wild_copy_match_paranoid(self.output.vec, offset, len);
        }
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn copy_match_from_history(
        &mut self,
        history: &[u8],
        offset: usize,
        out_pos: usize,
    ) -> Result<(), DecompressError> {
        if unlikely(offset <= out_pos || offset > out_pos + history.len()) {
            return Err(DecompressError::InvalidOffset);
        }

        let history_reach = offset - out_pos;
        let history_start = history.len() - history_reach;
        let from_history = history_reach.min(self.match_len);
        self.extend_slice_range(
            history,
            history_start,
            from_history,
            DecompressError::InvalidOffset,
        )?;

        let remaining = self.match_len - from_history;
        if remaining > 0 {
            self.copy_match_len(offset, remaining)?;
        }
        Ok(())
    }

    #[inline(always)]
    fn copy_match_len(&mut self, offset: usize, len: usize) -> Result<(), DecompressError> {
        if unlikely(offset == 0 || offset > self.output.vec.len()) {
            return Err(DecompressError::InvalidOffset);
        }
        if len == 0 {
            return Ok(());
        }

        #[cfg(not(feature = "paranoid"))]
        unsafe {
            debug_assert!(
                self.output.vec.len() + len + WILDCOPY_OVERLENGTH <= self.output.vec.capacity()
            );
            // SAFETY: `begin_sequence` proves capacity for the whole sequence.
            // The offset check proves the match source starts in initialized
            // output. Each copy arm handles its overlap pattern.
            wild_copy_match_unchecked(self.output.vec, offset, len);
        }
        #[cfg(feature = "paranoid")]
        {
            wild_copy_match_paranoid(self.output.vec, offset, len);
        }
        Ok(())
    }
}

/// 16-byte copy via two u64 load/stores.
///
/// # Safety
///
/// `src..src+16` must be readable and `dst..dst+16` must be writable. The
/// regions may overlap because both reads complete before either write.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn copy_16(src: *const u8, dst: *mut u8) {
    debug_assert!(!src.is_null());
    debug_assert!(!dst.is_null());
    // SAFETY: The caller provides readable/writable 16-byte ranges. Unaligned
    // accesses are intentional and do not require pointer alignment.
    unsafe {
        let a = (src as *const u64).read_unaligned();
        let b = (src.add(8) as *const u64).read_unaligned();
        (dst as *mut u64).write_unaligned(a);
        (dst.add(8) as *mut u64).write_unaligned(b);
    }
}

/// # Safety
///
/// `src..src+8` must be readable and `dst..dst+8` must be writable.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn copy_8(src: *const u8, dst: *mut u8) {
    debug_assert!(!src.is_null());
    debug_assert!(!dst.is_null());
    // SAFETY: The caller provides readable/writable 8-byte ranges. Unaligned
    // accesses are intentional and do not require pointer alignment.
    unsafe {
        let v = (src as *const u64).read_unaligned();
        (dst as *mut u64).write_unaligned(v);
    }
}

/// # Safety
///
/// `src..src+32` must be readable and `dst..dst+32` must be writable.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn copy_32(src: *const u8, dst: *mut u8) {
    debug_assert!(!src.is_null());
    debug_assert!(!dst.is_null());
    // SAFETY: The caller provides readable/writable 32-byte ranges.
    unsafe {
        copy_16(src, dst);
        copy_16(src.add(16), dst.add(16));
    }
}

/// # Safety
///
/// `src..src+64` must be readable and `dst..dst+64` must be writable.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn copy_64(src: *const u8, dst: *mut u8) {
    debug_assert!(!src.is_null());
    debug_assert!(!dst.is_null());
    // SAFETY: The caller provides readable/writable 64-byte ranges.
    unsafe {
        copy_16(src, dst);
        copy_16(src.add(16), dst.add(16));
        copy_16(src.add(32), dst.add(32));
        copy_16(src.add(48), dst.add(48));
    }
}

/// Copy `src` into the end of `vec` using 16-byte chunk copies.
///
/// All reads stay within `src` bounds (no wild over-read).
///
/// # Safety
///
/// `sp..sp+len` must be readable, and `vec` must have at least `len + 16`
/// bytes of spare capacity from its current end.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn fast_extend_from_ptr(vec: &mut Vec<u8>, sp: *const u8, len: usize) {
    if len == 0 {
        return;
    }
    debug_assert!(vec.len() + len + 16 <= vec.capacity());
    // SAFETY: The caller supplies a readable source range and enough spare
    // capacity for the destination, including the short-copy headroom.
    unsafe {
        let dst = vec.as_mut_ptr().add(vec.len());
        if len >= 16 {
            let mut off = 0usize;
            while off + 16 <= len {
                copy_16(sp.add(off), dst.add(off));
                off += 16;
            }
            if off < len {
                copy_16(sp.add(len - 16), dst.add(len - 16));
            }
        } else if len >= 8 {
            copy_8(sp, dst);
            copy_8(sp.add(len - 8), dst.add(len - 8));
        } else if len >= 4 {
            let a = (sp as *const u32).read_unaligned();
            (dst as *mut u32).write_unaligned(a);
            let b = (sp.add(len - 4) as *const u32).read_unaligned();
            (dst.add(len - 4) as *mut u32).write_unaligned(b);
        } else if len == 3 {
            let a = (sp as *const u16).read_unaligned();
            (dst as *mut u16).write_unaligned(a);
            *dst.add(2) = *sp.add(2);
        } else if len == 2 {
            let a = (sp as *const u16).read_unaligned();
            (dst as *mut u16).write_unaligned(a);
        } else {
            *dst = *sp;
        }
        vec.set_len(vec.len() + len);
    }
}

/// Build an 8-byte repeating pattern from `offset` bytes at `src`.
/// Only reads the first `offset` bytes (no out-of-bounds access).
///
/// # Safety
///
/// `src..src+offset` must be readable, with offset in 2..=7.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn build_pattern_u64(src: *const u8, offset: usize) -> u64 {
    debug_assert!((2..=7).contains(&offset));
    let mut buf = [0u8; 8];
    // SAFETY: The caller provides offset readable bytes. The remaining copies
    // operate wholly inside the local 8-byte buffer.
    unsafe {
        let p = buf.as_mut_ptr();
        core::ptr::copy_nonoverlapping(src, p, offset);
        let mut have = offset;
        while have < 8 {
            let n = have.min(8 - have);
            core::ptr::copy_nonoverlapping(p, p.add(have), n);
            have += n;
        }
    }
    u64::from_ne_bytes(buf)
}

/// Copy a match of `len` bytes at `offset` bytes back from the end of `vec`.
///
/// Handles all cases: non-overlapping (offset >= 16), offset 8..15,
/// RLE (offset == 1), and overlapping small-offset copies (2..7).
///
/// # Safety contract (upheld by caller):
/// - `offset > 0 && offset <= vec.len()`
/// - `vec.len() + len + 16 <= vec.capacity()`
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn wild_copy_match_unchecked(vec: &mut Vec<u8>, offset: usize, len: usize) {
    debug_assert!(offset > 0 && offset <= vec.len());
    debug_assert!(vec.len() + len + 16 <= vec.capacity());
    // SAFETY: offset is validated above, and reserve ensures enough output
    // headroom. Each copy arm handles its overlap pattern.
    unsafe {
        let ptr = vec.as_mut_ptr();
        let op = ptr.add(vec.len());
        let src = op.sub(offset);

        if offset >= 16 {
            let mut off = 0usize;
            loop {
                copy_16(src.add(off), op.add(off));
                off += 16;
                if off >= len {
                    break;
                }
            }
        } else if offset == 1 {
            core::ptr::write_bytes(op, *src, len + 16);
        } else if offset >= 8 {
            // 8-byte copies tracking src; overlap at boundary is fine because
            // read_unaligned completes before write_unaligned.
            copy_8(src, op);
            copy_8(src.add(8), op.add(8));
            if len > 16 {
                let mut off = 16usize;
                loop {
                    copy_8(src.add(off), op.add(off));
                    copy_8(src.add(off + 8), op.add(off + 8));
                    off += 16;
                    if off >= len {
                        break;
                    }
                }
            }
        } else {
            // Offset 2..7: build 8-byte repeating pattern from individual source
            // bytes (avoids reading uninitialized destination memory), then stamp it
            // at offset-aligned positions.
            let pat64 = build_pattern_u64(src, offset);
            let mut off = 0usize;
            loop {
                (op.add(off) as *mut u64).write_unaligned(pat64);
                off += offset;
                if off >= len {
                    break;
                }
            }
        }
        vec.set_len(vec.len() + len);
    }
}

/// # Safety
///
/// `src..src+len` must be readable, `op..op+len+64` must be writable, and
/// `offset >= 16`.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn wild_copy_match_16plus_from_ptr(src: *const u8, op: *mut u8, offset: usize, len: usize) {
    debug_assert!(!src.is_null());
    debug_assert!(!op.is_null());
    debug_assert!(offset >= 16);
    // SAFETY: The caller guarantees source and destination headroom. offset >=
    // 16 makes these wide chunk copies valid for the overlap cases used here.
    unsafe {
        if len <= 16 {
            copy_16(src, op);
        } else if offset >= 64 {
            let mut off = 0usize;
            loop {
                copy_64(src.add(off), op.add(off));
                off += 64;
                if off >= len {
                    break;
                }
            }
        } else if offset >= 32 {
            let mut off = 0usize;
            loop {
                copy_32(src.add(off), op.add(off));
                off += 32;
                if off >= len {
                    break;
                }
            }
        } else {
            let mut off = 0usize;
            loop {
                copy_16(src.add(off), op.add(off));
                off += 16;
                if off >= len {
                    break;
                }
            }
        }
    }
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn wild_copy_match_16plus_unchecked(vec: &mut Vec<u8>, offset: usize, len: usize) {
    debug_assert!(offset >= 16 && offset <= vec.len());
    debug_assert!(vec.len() + len + 64 <= vec.capacity());
    // SAFETY: offset is validated above, and reserve ensures the 64 bytes of
    // wildcopy headroom required by this wider-copy variant.
    unsafe {
        let ptr = vec.as_mut_ptr();
        let op = ptr.add(vec.len());
        let src = op.sub(offset);
        wild_copy_match_16plus_from_ptr(src, op, offset, len);
        vec.set_len(vec.len() + len);
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn wild_copy_match_paranoid(vec: &mut Vec<u8>, offset: usize, len: usize) {
    let start = vec.len() - offset;
    if offset >= len {
        vec.extend_from_within(start..start + len);
    } else if offset == 1 {
        vec.resize(vec.len() + len, vec[start]);
    } else {
        vec.extend_from_within(start..start + offset);
        let mut copied = offset;
        while copied < len {
            let n = (len - copied).min(copied);
            let src = vec.len() - copied;
            vec.extend_from_within(src..src + n);
            copied += n;
        }
    }
}

/// Variant for one-sequence tiny frames. The caller reserves 64 bytes of
/// headroom, so non-overlapping copies can use wider chunks without changing
/// the generic multi-sequence path.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn wild_copy_match_single_unchecked(vec: &mut Vec<u8>, offset: usize, len: usize) {
    debug_assert!(offset > 0 && offset <= vec.len());
    debug_assert!(vec.len() + len + 64 <= vec.capacity());
    // SAFETY: offset is validated above, and reserve ensures the 64 bytes of
    // wildcopy headroom required by this single-sequence variant.
    unsafe {
        let ptr = vec.as_mut_ptr();
        let op = ptr.add(vec.len());
        let src = op.sub(offset);

        if len <= 16 && offset >= 16 {
            copy_16(src, op);
        } else if offset >= 64 {
            let mut off = 0usize;
            loop {
                copy_64(src.add(off), op.add(off));
                off += 64;
                if off >= len {
                    break;
                }
            }
        } else if offset >= 32 {
            let mut off = 0usize;
            loop {
                copy_32(src.add(off), op.add(off));
                off += 32;
                if off >= len {
                    break;
                }
            }
        } else {
            wild_copy_match_unchecked(vec, offset, len);
            return;
        }
        vec.set_len(vec.len() + len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_lengths(max_len: usize) -> Vec<usize> {
        (1..=max_len).collect()
    }

    fn expected_match(seed: &[u8], offset: usize, len: usize) -> Vec<u8> {
        let mut expected = seed.to_vec();
        let start = expected.len() - offset;
        for i in 0..len {
            expected.push(expected[start + i % offset]);
        }
        expected
    }

    fn check_wild_copy_match_offsets(start_offset: usize, end_offset: usize) {
        for offset in start_offset..=end_offset {
            for len in test_lengths(128) {
                let mut v = Vec::new();
                let seed: Vec<u8> = (0..offset).map(|i| (i as u8).wrapping_mul(37)).collect();
                v.extend_from_slice(&seed);
                let expected = expected_match(&v, offset, len);
                let mut output = BlockOutput::new(&mut v, len);
                let mut sequence = output.begin_sequence(0, len).unwrap();
                sequence.copy_match(offset).unwrap();
                assert_eq!(
                    &v[..offset + len],
                    &expected[..offset + len],
                    "offset={offset} len={len}"
                );
            }
        }
    }

    fn check_wild_copy_match_single_offsets(start_offset: usize, end_offset: usize) {
        for offset in start_offset..=end_offset {
            for len in test_lengths(256) {
                let mut v = Vec::new();
                let seed: Vec<u8> = (0..offset).map(|i| (i as u8).wrapping_mul(37)).collect();
                v.extend_from_slice(&seed);
                let expected = expected_match(&v, offset, len);
                let mut output = BlockOutput::new(&mut v, len);
                let mut sequence = output.begin_sequence(0, len).unwrap();
                sequence.copy_match_single(offset).unwrap();
                assert_eq!(
                    &v[..offset + len],
                    &expected[..offset + len],
                    "offset={offset} len={len}"
                );
            }
        }
    }

    #[test]
    fn fast_extend_from_slice_all_sizes() {
        for len in 0..=64 {
            let src: Vec<u8> = (0..len as u8).collect();
            let mut dst = Vec::new();
            let mut output = BlockOutput::new(&mut dst, len);
            output.extend_literals_range(&src, 0, len).unwrap();
            assert_eq!(dst, src, "len={len}");
        }
    }

    #[test]
    fn block_output_extends_literal_range() {
        let mut dst = Vec::new();
        {
            let mut output = BlockOutput::new(&mut dst, 16);
            output
                .extend_literals_range(b"abcdefgh", 2, 4)
                .expect("literal range should fit");
        }
        assert_eq!(dst, b"cdef");
    }

    #[test]
    fn block_output_rejects_literal_range_oob() {
        let mut dst = Vec::new();
        let mut output = BlockOutput::new(&mut dst, 16);
        let err = output
            .extend_literals_range(b"abc", 2, 4)
            .expect_err("literal range should be out of bounds");
        assert_eq!(err, DecompressError::CorruptLiterals);
    }

    #[test]
    fn block_output_rejects_block_overflow() {
        let mut dst = Vec::new();
        let mut output = BlockOutput::new(&mut dst, 4);
        let err = output
            .extend_literals_range(b"abcde", 0, 5)
            .expect_err("literal range should exceed the block limit");
        assert_eq!(err, DecompressError::CorruptSequences);
    }

    #[test]
    fn block_output_rejects_bad_match_offset() {
        let mut dst = b"abcd".to_vec();
        let mut output = BlockOutput::new(&mut dst, 16);
        let mut sequence = output
            .begin_sequence(0, 4)
            .expect("sequence should fit block");
        let err = sequence
            .copy_match(5)
            .expect_err("match offset should exceed output length");
        assert_eq!(err, DecompressError::InvalidOffset);
    }

    #[test]
    fn block_output_copies_match() {
        let mut dst = b"abcdefghijklmnop".to_vec();
        {
            let mut output = BlockOutput::new(&mut dst, 32);
            let mut sequence = output
                .begin_sequence(0, 20)
                .expect("sequence should fit block");
            sequence.copy_match_16plus(16).expect("match should fit");
        }
        assert_eq!(&dst[16..], b"abcdefghijklmnopabcd");
    }

    #[test]
    fn block_output_copies_match_from_history() {
        let history = b"abcdefghijklmnop";
        let mut dst = b"qrst".to_vec();
        let out_pos = dst.len();
        let offset = 8;
        let match_len = 20;

        let mut expected = dst.clone();
        for i in 0..match_len {
            let src = history.len() + out_pos - offset + i;
            let byte = if src < history.len() {
                history[src]
            } else {
                expected[src - history.len()]
            };
            expected.push(byte);
        }

        {
            let mut output = BlockOutput::new(&mut dst, 32);
            let mut sequence = output
                .begin_sequence(0, match_len)
                .expect("sequence should fit block");
            sequence
                .copy_match_from_history(history, offset, out_pos)
                .expect("history match should fit");
        }

        assert_eq!(dst, expected);
    }

    #[test]
    fn wild_copy_match_offsets_1_8() {
        check_wild_copy_match_offsets(1, 8);
    }

    #[test]
    fn wild_copy_match_offsets_9_16() {
        check_wild_copy_match_offsets(9, 16);
    }

    #[test]
    fn wild_copy_match_offsets_17_32() {
        check_wild_copy_match_offsets(17, 32);
    }

    #[test]
    fn wild_copy_match_offsets_33_64() {
        check_wild_copy_match_offsets(33, 64);
    }

    #[test]
    fn wild_copy_match_single_offsets_1_8() {
        check_wild_copy_match_single_offsets(1, 8);
    }

    #[test]
    fn wild_copy_match_single_offsets_9_16() {
        check_wild_copy_match_single_offsets(9, 16);
    }

    #[test]
    fn wild_copy_match_single_offsets_17_32() {
        check_wild_copy_match_single_offsets(17, 32);
    }

    #[test]
    fn wild_copy_match_single_offsets_33_64() {
        check_wild_copy_match_single_offsets(33, 64);
    }

    #[test]
    fn wild_copy_match_single_offsets_65_96() {
        check_wild_copy_match_single_offsets(65, 96);
    }

    #[test]
    fn wild_copy_match_single_offsets_97_128() {
        check_wild_copy_match_single_offsets(97, 128);
    }
}

// Run with: cargo kani -p zrip-decode -j4 --output-format terse
#[cfg(all(kani, not(feature = "paranoid")))]
mod kani_proofs {
    use super::*;

    // -- Arithmetic proof: BlockOutput capacity invariant --

    /// BlockOutput::new reserves block_size + WILDCOPY_OVERLENGTH spare
    /// capacity. Prove this is sufficient for any valid sequence's
    /// literal copy (needing 16 bytes headroom) and match copy (needing
    /// up to WILDCOPY_OVERLENGTH = 64 bytes headroom).
    #[kani::proof]
    fn block_output_capacity_sufficient() {
        let initial_len: usize = kani::any();
        let block_size: usize = kani::any();
        kani::assume(initial_len <= 64);
        kani::assume(block_size >= 1 && block_size <= 64);

        let capacity = initial_len + block_size + WILDCOPY_OVERLENGTH;
        let literal_len: usize = kani::any();
        let match_len: usize = kani::any();
        kani::assume(literal_len <= block_size);
        kani::assume(match_len <= block_size - literal_len);

        let vec_len_after_literals = initial_len + literal_len;

        // wild_copy_match_unchecked: 16-byte headroom
        assert!(vec_len_after_literals + match_len + 16 <= capacity);
        // wild_copy_match_{16plus,single}_unchecked: 64-byte headroom
        assert!(vec_len_after_literals + match_len + WILDCOPY_OVERLENGTH <= capacity);
    }

    // -- Primitive: build_pattern_u64 --

    /// The inner doubling loop fills exactly 8 bytes from `offset`
    /// source bytes (offset in 2..=7) without OOB reads or writes.
    #[kani::proof]
    #[kani::unwind(3)] // at most 2 doubling iterations
    fn build_pattern_u64_no_oob() {
        let src = [0u8; 7];
        let offset: usize = kani::any();
        kani::assume(offset >= 2 && offset <= 7);
        unsafe {
            build_pattern_u64(src.as_ptr(), offset);
        }
    }

    // -- Primitive: fast_extend_from_ptr --

    /// All copy tiers (1, 2, 3, 4..7, 8..15, 16+) stay within source
    /// and destination bounds given the documented precondition
    /// (len + 16 bytes of spare capacity).
    #[kani::proof]
    #[kani::unwind(5)] // 64/16 = 4 iterations max in the 16-byte loop
    fn fast_extend_from_ptr_no_oob() {
        let src = [0u8; 64];
        let len: usize = kani::any();
        kani::assume(len >= 1 && len <= 64);

        let mut vec = Vec::with_capacity(len + 16);
        unsafe {
            fast_extend_from_ptr(&mut vec, src.as_ptr(), len);
        }
        assert_eq!(vec.len(), len);
    }

    // -- wild_copy_match_unchecked --
    //
    // Split by offset tier to control loop-unwind bounds. Each harness
    // establishes the documented precondition (offset > 0, offset <=
    // vec.len(), capacity >= vec.len() + len + 16) and proves the
    // corresponding code path stays in-bounds.
    //
    // Pre-fill uses extend_from_slice (memcpy, no loop) to avoid
    // triggering unwind limits in Vec::resize's internal loop.

    /// offset >= 16: copy_16 loop
    #[kani::proof]
    #[kani::unwind(4)] // 48/16 = 3 iterations max
    fn wild_copy_match_offset_ge16_no_oob() {
        let offset: usize = kani::any();
        let len: usize = kani::any();
        kani::assume(offset >= 16 && offset <= 48);
        kani::assume(len >= 1 && len <= 48);

        let mut vec = Vec::with_capacity(48 + len + 16);
        vec.extend_from_slice(&[0u8; 48]);
        unsafe {
            wild_copy_match_unchecked(&mut vec, offset, len);
        }
        assert_eq!(vec.len(), 48 + len);
    }

    /// offset == 1: RLE via write_bytes (no loop in this path, but
    /// Kani still needs an unwind bound for loops in sibling branches)
    #[kani::proof]
    #[kani::unwind(2)]
    fn wild_copy_match_offset_1_no_oob() {
        let len: usize = kani::any();
        kani::assume(len >= 1 && len <= 64);

        let mut vec = Vec::with_capacity(4 + len + 16);
        vec.extend_from_slice(&[0xAAu8; 4]);
        unsafe {
            wild_copy_match_unchecked(&mut vec, 1, len);
        }
        assert_eq!(vec.len(), 4 + len);
    }

    /// offset 8..15: copy_8 pairs loop
    #[kani::proof]
    #[kani::unwind(4)] // (48-16)/16 = 2 iterations max in inner loop
    fn wild_copy_match_offset_8to15_no_oob() {
        let offset: usize = kani::any();
        let len: usize = kani::any();
        kani::assume(offset >= 8 && offset <= 15);
        kani::assume(len >= 1 && len <= 48);

        let mut vec = Vec::with_capacity(16 + len + 16);
        vec.extend_from_slice(&[0u8; 16]);
        unsafe {
            wild_copy_match_unchecked(&mut vec, offset, len);
        }
        assert_eq!(vec.len(), 16 + len);
    }

    /// offset 2..7: build_pattern_u64 + u64 stamp loop
    #[kani::proof]
    #[kani::unwind(13)] // 24/2 = 12 iterations max (offset=2, len=24)
    fn wild_copy_match_offset_2to7_no_oob() {
        let offset: usize = kani::any();
        let len: usize = kani::any();
        kani::assume(offset >= 2 && offset <= 7);
        kani::assume(len >= 1 && len <= 24);

        let mut vec = Vec::with_capacity(8 + len + 16);
        vec.extend_from_slice(&[0u8; 8]);
        unsafe {
            wild_copy_match_unchecked(&mut vec, offset, len);
        }
        assert_eq!(vec.len(), 8 + len);
    }

    // -- wild_copy_match_16plus_unchecked --

    /// offset >= 16 wide-copy path: copy_16/32/64 loops depending
    /// on offset magnitude.
    #[kani::proof]
    #[kani::unwind(5)] // 64/16 = 4 iterations max in narrowest loop
    fn wild_copy_match_16plus_no_oob() {
        let offset: usize = kani::any();
        let len: usize = kani::any();
        kani::assume(offset >= 16 && offset <= 48);
        kani::assume(len >= 1 && len <= 64);

        let mut vec = Vec::with_capacity(48 + len + WILDCOPY_OVERLENGTH);
        vec.extend_from_slice(&[0u8; 48]);
        unsafe {
            wild_copy_match_16plus_unchecked(&mut vec, offset, len);
        }
        assert_eq!(vec.len(), 48 + len);
    }

    // -- wild_copy_match_single_unchecked --
    //
    // Split into three harnesses matching the dispatch:
    //   len <= 16 && offset >= 16 → single copy_16
    //   offset >= 32             → copy_32 or copy_64 loop
    //   else                     → fallthrough to wild_copy_match_unchecked

    /// Short match with wide offset: single copy_16 (no loop in this
    /// path, but Kani needs an unwind bound for loops in sibling
    /// branches).
    #[kani::proof]
    #[kani::unwind(2)]
    fn wild_copy_match_single_short_no_oob() {
        let offset: usize = kani::any();
        let len: usize = kani::any();
        kani::assume(offset >= 16 && offset <= 48);
        kani::assume(len >= 1 && len <= 16);

        let mut vec = Vec::with_capacity(48 + len + WILDCOPY_OVERLENGTH);
        vec.extend_from_slice(&[0u8; 48]);
        unsafe {
            wild_copy_match_single_unchecked(&mut vec, offset, len);
        }
        assert_eq!(vec.len(), 48 + len);
    }

    /// Wide offset: copy_32 or copy_64 loops.
    #[kani::proof]
    #[kani::unwind(3)] // 64/32 = 2 iterations max
    fn wild_copy_match_single_wide_no_oob() {
        let offset: usize = kani::any();
        let len: usize = kani::any();
        kani::assume(offset >= 32 && offset <= 64);
        kani::assume(len >= 1 && len <= 64);

        let mut vec = Vec::with_capacity(64 + len + WILDCOPY_OVERLENGTH);
        vec.extend_from_slice(&[0u8; 64]);
        unsafe {
            wild_copy_match_single_unchecked(&mut vec, offset, len);
        }
        assert_eq!(vec.len(), 64 + len);
    }

    /// Fallthrough to wild_copy_match_unchecked (offset < 32,
    /// excluding the short + offset >= 16 fast path).
    #[kani::proof]
    #[kani::unwind(13)] // pattern stamp loop: 24/2 = 12 max
    fn wild_copy_match_single_fallthrough_no_oob() {
        let offset: usize = kani::any();
        let len: usize = kani::any();
        kani::assume(offset >= 1 && offset <= 31);
        kani::assume(len >= 1 && len <= 24);
        kani::assume(!(len <= 16 && offset >= 16));

        let mut vec = Vec::with_capacity(32 + len + WILDCOPY_OVERLENGTH);
        vec.extend_from_slice(&[0u8; 32]);
        unsafe {
            wild_copy_match_single_unchecked(&mut vec, offset, len);
        }
        assert_eq!(vec.len(), 32 + len);
    }
}
