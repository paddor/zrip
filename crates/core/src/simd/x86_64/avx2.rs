#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::*;

/// Copy `len` bytes from `src` to `dst` using 32-byte AVX2 loads/stores.
/// Overshoots by up to 31 bytes.
///
/// # Safety
/// - `src` must be valid for reads of `len + 31` bytes.
/// - `dst` must be valid for writes of `len + 31` bytes.
/// - Regions must NOT overlap.
/// - AVX2 must be available.
#[target_feature(enable = "avx2")]
#[inline]
pub unsafe fn wildcopy_avx2(mut src: *const u8, mut dst: *mut u8, len: usize) {
    debug_assert!(len > 0);
    unsafe {
        let end = dst.add(len);
        loop {
            let chunk = _mm256_loadu_si256(src as *const __m256i);
            _mm256_storeu_si256(dst as *mut __m256i, chunk);
            src = src.add(32);
            dst = dst.add(32);
            if dst >= end {
                break;
            }
        }
    }
}

/// Copy match with unconditional 16-byte first copy for offset >= 8.
/// For offset < 8, uses pattern-based approach. For offset >= 32, uses
/// AVX2 wildcopy.
///
/// # Safety
/// - `dst` must have at least `len + 31` bytes of writable space.
/// - `dst - offset` must be valid for reading.
/// - AVX2 must be available.
#[target_feature(enable = "avx2")]
#[inline]
pub unsafe fn copy_match_avx2(dst: *mut u8, offset: usize, len: usize) {
    debug_assert!(offset > 0);
    debug_assert!(len > 0);

    unsafe {
        if offset >= 32 {
            wildcopy_avx2(dst.sub(offset), dst, len);
        } else if offset >= 8 {
            let src = dst.sub(offset);
            (dst as *mut u64).write_unaligned((src as *const u64).read_unaligned());
            (dst.add(8) as *mut u64).write_unaligned((src.add(8) as *const u64).read_unaligned());
            if len > 16 {
                let mut s = src.add(16);
                let mut d = dst.add(16);
                let end = dst.add(len);
                while d < end {
                    (d as *mut u64).write_unaligned((s as *const u64).read_unaligned());
                    s = s.add(8);
                    d = d.add(8);
                }
            }
        } else {
            super::super::scalar::copy_match(dst, offset, len);
        }
    }
}

/// Count common prefix length using AVX2.
/// Compares 32 bytes at a time, returns the byte offset of the first mismatch.
///
/// # Safety
/// AVX2 must be available.
#[target_feature(enable = "avx2")]
#[inline]
pub unsafe fn common_prefix_len_avx2(a: &[u8], b: &[u8]) -> usize {
    let len = a.len().min(b.len());
    let mut i = 0;

    unsafe {
        while i + 32 <= len {
            let va = _mm256_loadu_si256(a.as_ptr().add(i) as *const __m256i);
            let vb = _mm256_loadu_si256(b.as_ptr().add(i) as *const __m256i);
            let cmp = _mm256_cmpeq_epi8(va, vb);
            let mask = _mm256_movemask_epi8(cmp) as u32;
            if mask != 0xFFFF_FFFF {
                return i + (!mask).trailing_zeros() as usize;
            }
            i += 32;
        }
    }

    // Tail: use scalar for remaining bytes
    while i < len {
        if a[i] != b[i] {
            return i;
        }
        i += 1;
    }
    i
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    #[test]
    fn wildcopy_avx2_basic() {
        if !std::arch::is_x86_feature_detected!("avx2") {
            return;
        }
        let src: Vec<u8> = (0..128).map(|i| i as u8).collect();
        let mut dst = vec![0u8; 160];
        unsafe {
            wildcopy_avx2(src.as_ptr(), dst.as_mut_ptr(), 128);
        }
        assert_eq!(&dst[..128], &src[..128]);
    }

    #[test]
    fn copy_match_avx2_large_offset() {
        if !std::arch::is_x86_feature_detected!("avx2") {
            return;
        }
        let mut buf = vec![0u8; 256];
        for (i, b) in buf.iter_mut().enumerate().take(64) {
            *b = (i + 1) as u8;
        }
        unsafe {
            copy_match_avx2(buf.as_mut_ptr().add(64), 64, 64);
        }
        assert_eq!(&buf[64..128], &buf[..64]);
    }

    #[test]
    fn common_prefix_len_avx2_basic() {
        if !std::arch::is_x86_feature_detected!("avx2") {
            return;
        }
        let a = vec![0x55u8; 200];
        let mut b = vec![0x55u8; 200];
        b[137] = 0xAA;
        unsafe {
            assert_eq!(common_prefix_len_avx2(&a, &b), 137);
        }
    }

    #[test]
    fn common_prefix_len_avx2_identical() {
        if !std::arch::is_x86_feature_detected!("avx2") {
            return;
        }
        let a = vec![0x42u8; 100];
        let b = vec![0x42u8; 100];
        unsafe {
            assert_eq!(common_prefix_len_avx2(&a, &b), 100);
        }
    }

    #[test]
    fn common_prefix_len_avx2_first_byte_differs() {
        if !std::arch::is_x86_feature_detected!("avx2") {
            return;
        }
        let a = vec![1u8; 64];
        let b = vec![2u8; 64];
        unsafe {
            assert_eq!(common_prefix_len_avx2(&a, &b), 0);
        }
    }
}
