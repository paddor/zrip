use core::ptr;

/// Copy `len` bytes from `src` to `dst` using 8-byte chunks.
/// Does NOT overshoot the source read.
/// Destination must have at least `len` bytes writable.
///
/// # Safety
/// - `src` must be valid for reads of `len` bytes.
/// - `dst` must be valid for writes of `len` bytes.
/// - Regions may NOT overlap.
#[inline(always)]
pub unsafe fn wildcopy_nonoverlap(src: *const u8, dst: *mut u8, len: usize) {
    debug_assert!(len > 0);
    unsafe {
        let mut offset = 0;
        while offset + 8 <= len {
            ptr::copy_nonoverlapping(src.add(offset), dst.add(offset), 8);
            offset += 8;
        }
        if offset < len {
            ptr::copy_nonoverlapping(src.add(offset), dst.add(offset), len - offset);
        }
    }
}

/// Copy `len` bytes from within the output buffer at `offset` bytes back.
/// Handles overlapping copies (offset < len) correctly by repeating the pattern.
///
/// # Safety
/// - `dst` must point to a buffer with at least `len + 7` bytes of writable space.
/// - `dst - offset` must be a valid readable position within the same allocation.
#[inline(always)]
pub unsafe fn copy_match(dst: *mut u8, offset: usize, len: usize) {
    debug_assert!(offset > 0);
    debug_assert!(len > 0);
    unsafe {
        let src = dst.sub(offset);
        if offset >= 8 {
            wildcopy_nonoverlap(src, dst, len);
        } else {
            copy_match_overlapping(dst, offset, len);
        }
    }
}

#[inline(always)]
unsafe fn copy_match_overlapping(mut dst: *mut u8, offset: usize, len: usize) {
    unsafe {
        let end = dst.add(len);
        let src = dst.sub(offset);

        if offset == 1 {
            ptr::write_bytes(dst, *src, len);
            return;
        }

        let mut pattern = [0u8; 8];
        for (i, p) in pattern.iter_mut().enumerate() {
            *p = *src.add(i % offset);
        }
        let pattern_word = u64::from_le_bytes(pattern);

        // For offsets that divide 8 (2, 4), the pattern aligns at 8-byte
        // boundaries so we can advance by 8 instead of by offset.
        let step = if offset == 2 || offset == 4 {
            8
        } else {
            offset
        };
        while dst < end {
            (dst as *mut u64).write_unaligned(pattern_word);
            dst = dst.add(step);
        }
    }
}

/// Count common prefix length between two byte slices.
#[inline]
pub fn common_prefix_len(a: &[u8], b: &[u8]) -> usize {
    let len = a.len().min(b.len());
    let mut i = 0;

    while i + 8 <= len {
        let a_chunk = u64::from_le_bytes(a[i..i + 8].try_into().unwrap());
        let b_chunk = u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        let xor = a_chunk ^ b_chunk;
        if xor != 0 {
            return i + (xor.trailing_zeros() as usize / 8);
        }
        i += 8;
    }

    while i < len {
        if a[i] != b[i] {
            return i;
        }
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use alloc::vec;

    #[test]
    fn wildcopy_basic() {
        let src = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let mut dst = [0u8; 20];
        unsafe {
            wildcopy_nonoverlap(src.as_ptr(), dst.as_mut_ptr(), 10);
        }
        assert_eq!(&dst[..10], &src);
    }

    #[test]
    fn copy_match_no_overlap() {
        let mut buf = vec![0u8; 64];
        buf[0..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        unsafe {
            copy_match(buf.as_mut_ptr().add(8), 8, 16);
        }
        assert_eq!(&buf[8..16], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&buf[16..24], &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn copy_match_offset_1() {
        let mut buf = vec![0u8; 32];
        buf[0] = 0xAB;
        unsafe {
            copy_match(buf.as_mut_ptr().add(1), 1, 15);
        }
        assert!(buf[..16].iter().all(|&b| b == 0xAB));
    }

    #[test]
    fn copy_match_offset_2() {
        let mut buf = vec![0u8; 32];
        buf[0..2].copy_from_slice(&[0xAA, 0xBB]);
        unsafe {
            copy_match(buf.as_mut_ptr().add(2), 2, 14);
        }
        for (i, &b) in buf.iter().enumerate().take(16) {
            assert_eq!(b, if i % 2 == 0 { 0xAA } else { 0xBB });
        }
    }

    #[test]
    fn copy_match_offset_3() {
        let mut buf = vec![0u8; 32];
        buf[0..3].copy_from_slice(&[1, 2, 3]);
        unsafe {
            copy_match(buf.as_mut_ptr().add(3), 3, 12);
        }
        assert_eq!(&buf[..15], &[1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3]);
    }

    #[test]
    fn copy_match_offset_7() {
        let mut buf = vec![0u8; 64];
        buf[0..7].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7]);
        unsafe {
            copy_match(buf.as_mut_ptr().add(7), 7, 21);
        }
        for (i, &b) in buf.iter().enumerate().take(28) {
            assert_eq!(b, (i % 7 + 1) as u8);
        }
    }

    #[test]
    fn common_prefix_len_basic() {
        assert_eq!(common_prefix_len(b"hello world", b"hello rust"), 6);
        assert_eq!(common_prefix_len(b"abcdef", b"abcdef"), 6);
        assert_eq!(common_prefix_len(b"abc", b"xyz"), 0);
        assert_eq!(common_prefix_len(b"", b"hello"), 0);
    }

    #[test]
    fn common_prefix_len_long() {
        let a = vec![0x55u8; 100];
        let mut b = vec![0x55u8; 100];
        b[73] = 0xAA;
        assert_eq!(common_prefix_len(&a, &b), 73);
    }
}
