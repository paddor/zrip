#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

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

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn fast_extend_from_slice(vec: &mut Vec<u8>, src: &[u8]) {
    // SAFETY: src.as_ptr() is readable for src.len() bytes. The decoder reserves
    // wildcopy headroom before reaching this primitive.
    unsafe {
        fast_extend_from_ptr(vec, src.as_ptr(), src.len());
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn fast_extend_from_slice(vec: &mut Vec<u8>, src: &[u8]) {
    vec.extend_from_slice(src);
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn fast_extend_from_slice_range(
    vec: &mut Vec<u8>,
    src: &[u8],
    start: usize,
    len: usize,
) {
    if len == 0 {
        return;
    }
    debug_assert!(start + len <= src.len());
    // SAFETY: start..start+len is inside src, and the decoder reserves wildcopy
    // headroom before reaching this primitive.
    unsafe {
        fast_extend_from_ptr(vec, src.as_ptr().add(start), len);
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn fast_extend_from_slice_range(
    vec: &mut Vec<u8>,
    src: &[u8],
    start: usize,
    len: usize,
) {
    if len == 0 {
        return;
    }
    if len == 1 {
        vec.push(src[start]);
        return;
    }
    vec.extend_from_slice(&src[start..start + len]);
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
pub(crate) fn wild_copy_match(vec: &mut Vec<u8>, offset: usize, len: usize) {
    debug_assert!(offset > 0 && offset <= vec.len());
    debug_assert!(vec.len() + len + 16 <= vec.capacity());
    // SAFETY: The decoder validates offset and reserves enough output headroom
    // before calling this primitive. Each copy arm handles its overlap pattern.
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
pub(crate) fn wild_copy_match_16plus(vec: &mut Vec<u8>, offset: usize, len: usize) {
    debug_assert!(offset >= 16 && offset <= vec.len());
    debug_assert!(vec.len() + len + 64 <= vec.capacity());
    // SAFETY: The decoder validates offset and reserves 64 bytes of wildcopy
    // headroom before calling this wider-copy variant.
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
pub(crate) fn wild_copy_match(vec: &mut Vec<u8>, offset: usize, len: usize) {
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

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn wild_copy_match_16plus(vec: &mut Vec<u8>, offset: usize, len: usize) {
    wild_copy_match(vec, offset, len);
}

/// Variant for one-sequence tiny frames. The caller reserves 64 bytes of
/// headroom, so non-overlapping copies can use wider chunks without changing
/// the generic multi-sequence path.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn wild_copy_match_single(vec: &mut Vec<u8>, offset: usize, len: usize) {
    debug_assert!(offset > 0 && offset <= vec.len());
    debug_assert!(vec.len() + len + 64 <= vec.capacity());
    // SAFETY: The single-sequence path reserves 64 bytes of wildcopy headroom
    // and validates offset before calling this primitive.
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
            wild_copy_match(vec, offset, len);
            return;
        }
        vec.set_len(vec.len() + len);
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn wild_copy_match_single(vec: &mut Vec<u8>, offset: usize, len: usize) {
    wild_copy_match(vec, offset, len);
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
                let mut v = Vec::with_capacity(offset + len + 16);
                let seed: Vec<u8> = (0..offset).map(|i| (i as u8).wrapping_mul(37)).collect();
                v.extend_from_slice(&seed);
                let expected = expected_match(&v, offset, len);
                wild_copy_match(&mut v, offset, len);
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
                let mut v = Vec::with_capacity(offset + len + 64);
                let seed: Vec<u8> = (0..offset).map(|i| (i as u8).wrapping_mul(37)).collect();
                v.extend_from_slice(&seed);
                let expected = expected_match(&v, offset, len);
                wild_copy_match_single(&mut v, offset, len);
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
            let mut dst = Vec::with_capacity(len + 16);
            fast_extend_from_slice(&mut dst, &src);
            assert_eq!(dst, src, "len={len}");
        }
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
