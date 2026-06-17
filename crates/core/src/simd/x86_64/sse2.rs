#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::*;

/// Copy `len` bytes from `src` to `dst` using 16-byte SSE2 loads/stores.
/// Overshoots by up to 15 bytes.
///
/// # Safety
/// - `src` must be valid for reads of `len + 15` bytes.
/// - `dst` must be valid for writes of `len + 15` bytes.
/// - Regions must NOT overlap.
/// - SSE2 must be available (guaranteed on x86_64).
#[target_feature(enable = "sse2")]
#[inline]
pub unsafe fn wildcopy_sse2(mut src: *const u8, mut dst: *mut u8, len: usize) {
    debug_assert!(len > 0);
    unsafe {
        let end = dst.add(len);
        loop {
            let chunk = _mm_loadu_si128(src as *const __m128i);
            _mm_storeu_si128(dst as *mut __m128i, chunk);
            src = src.add(16);
            dst = dst.add(16);
            if dst >= end {
                break;
            }
        }
    }
}

/// Copy match with SSE2 16-byte copies. For offset >= 16, uses non-overlapping
/// 16-byte copies. For offset < 16, falls back to scalar overlap handling.
///
/// # Safety
/// - `dst` must have at least `len + 15` bytes of writable space.
/// - `dst - offset` must be valid for reading.
/// - SSE2 must be available.
#[target_feature(enable = "sse2")]
#[inline]
pub unsafe fn copy_match_sse2(dst: *mut u8, offset: usize, len: usize) {
    debug_assert!(offset > 0);
    debug_assert!(len > 0);
    unsafe {
        if offset >= 16 {
            wildcopy_sse2(dst.sub(offset), dst, len);
        } else {
            super::super::scalar::copy_match(dst, offset, len);
        }
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    #[test]
    fn wildcopy_sse2_basic() {
        if !std::arch::is_x86_feature_detected!("sse2") {
            return;
        }
        let src: Vec<u8> = (0..64).collect();
        let mut dst = vec![0u8; 80];
        unsafe {
            wildcopy_sse2(src.as_ptr(), dst.as_mut_ptr(), 64);
        }
        assert_eq!(&dst[..64], &src[..64]);
    }

    #[test]
    fn copy_match_sse2_large_offset() {
        if !std::arch::is_x86_feature_detected!("sse2") {
            return;
        }
        let mut buf = vec![0u8; 128];
        for (i, b) in buf.iter_mut().enumerate().take(32) {
            *b = (i + 1) as u8;
        }
        unsafe {
            copy_match_sse2(buf.as_mut_ptr().add(32), 32, 32);
        }
        assert_eq!(&buf[32..64], &buf[..32]);
    }

    #[test]
    fn copy_match_sse2_small_offset() {
        if !std::arch::is_x86_feature_detected!("sse2") {
            return;
        }
        let mut buf = vec![0u8; 64];
        buf[0..3].copy_from_slice(&[1, 2, 3]);
        unsafe {
            copy_match_sse2(buf.as_mut_ptr().add(3), 3, 12);
        }
        assert_eq!(&buf[..15], &[1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3]);
    }
}
