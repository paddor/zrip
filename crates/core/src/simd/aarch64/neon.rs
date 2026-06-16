#[cfg(target_arch = "aarch64")]
use core::arch::aarch64::*;

/// Copy `len` bytes from `src` to `dst` using 16-byte NEON loads/stores.
/// Overshoots by up to 15 bytes.
///
/// # Safety
/// - `src` must be valid for reads of `len + 15` bytes.
/// - `dst` must be valid for writes of `len + 15` bytes.
/// - Regions must NOT overlap.
#[inline]
pub unsafe fn wildcopy_neon(mut src: *const u8, mut dst: *mut u8, len: usize) {
    debug_assert!(len > 0);
    unsafe {
        let end = dst.add(len);
        loop {
            let chunk = vld1q_u8(src);
            vst1q_u8(dst, chunk);
            src = src.add(16);
            dst = dst.add(16);
            if dst >= end {
                break;
            }
        }
    }
}

/// Copy match with NEON 16-byte copies. For offset >= 16, uses non-overlapping
/// 16-byte copies. For smaller offsets, falls back to scalar overlap handling.
///
/// # Safety
/// - `dst` must have at least `len + 15` bytes of writable space.
/// - `dst - offset` must be valid for reading.
#[inline]
pub unsafe fn copy_match_neon(dst: *mut u8, offset: usize, len: usize) {
    debug_assert!(offset > 0);
    debug_assert!(len > 0);
    unsafe {
        if offset >= 16 {
            wildcopy_neon(dst.sub(offset), dst, len);
        } else {
            super::super::scalar::copy_match(dst, offset, len);
        }
    }
}

/// Count common prefix length using NEON.
/// Compares 16 bytes at a time, returns the byte offset of the first mismatch.
///
/// # Safety
/// Caller must ensure `a` and `b` slices are accessible for reads up to
/// their length (no overshoot beyond slice bounds in the NEON path since
/// we only enter when 16+ bytes remain).
#[inline]
pub unsafe fn common_prefix_len_neon(a: &[u8], b: &[u8]) -> usize {
    let len = a.len().min(b.len());
    let mut i = 0;

    unsafe {
        while i + 16 <= len {
            let va = vld1q_u8(a.as_ptr().add(i));
            let vb = vld1q_u8(b.as_ptr().add(i));
            let cmp = vceqq_u8(va, vb);
            let min_byte = vminvq_u8(cmp);
            if min_byte != 0xFF {
                let diff = vmvnq_u8(cmp);
                let as_u64: uint64x2_t = vreinterpretq_u64_u8(diff);
                let lo = vgetq_lane_u64(as_u64, 0);
                if lo != 0 {
                    return i + (lo.trailing_zeros() as usize / 8);
                }
                let hi = vgetq_lane_u64(as_u64, 1);
                return i + 8 + (hi.trailing_zeros() as usize / 8);
            }
            i += 16;
        }
    }

    while i < len {
        if a[i] != b[i] {
            return i;
        }
        i += 1;
    }
    i
}

#[cfg(all(test, target_arch = "aarch64"))]
mod tests {
    use super::*;
    extern crate alloc;
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn wildcopy_neon_basic() {
        let src: Vec<u8> = (0..64).collect();
        let mut dst = vec![0u8; 80];
        unsafe {
            wildcopy_neon(src.as_ptr(), dst.as_mut_ptr(), 64);
        }
        assert_eq!(&dst[..64], &src[..64]);
    }

    #[test]
    fn copy_match_neon_large_offset() {
        let mut buf = vec![0u8; 128];
        for i in 0..32 {
            buf[i] = (i + 1) as u8;
        }
        unsafe {
            copy_match_neon(buf.as_mut_ptr().add(32), 32, 32);
        }
        assert_eq!(&buf[32..64], &buf[..32]);
    }

    #[test]
    fn copy_match_neon_small_offset() {
        let mut buf = vec![0u8; 64];
        buf[0..3].copy_from_slice(&[1, 2, 3]);
        unsafe {
            copy_match_neon(buf.as_mut_ptr().add(3), 3, 12);
        }
        assert_eq!(&buf[..15], &[1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3]);
    }

    #[test]
    fn common_prefix_len_neon_basic() {
        let a = vec![0x55u8; 200];
        let mut b = vec![0x55u8; 200];
        b[73] = 0xAA;
        unsafe {
            assert_eq!(common_prefix_len_neon(&a, &b), 73);
        }
    }

    #[test]
    fn common_prefix_len_neon_identical() {
        let a = vec![0x42u8; 100];
        let b = vec![0x42u8; 100];
        unsafe {
            assert_eq!(common_prefix_len_neon(&a, &b), 100);
        }
    }

    #[test]
    fn common_prefix_len_neon_first_byte() {
        let a = vec![1u8; 64];
        let b = vec![2u8; 64];
        unsafe {
            assert_eq!(common_prefix_len_neon(&a, &b), 0);
        }
    }
}
