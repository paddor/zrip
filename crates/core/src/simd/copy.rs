#[cfg(feature = "alloc")]
use alloc::vec::Vec;

#[cfg(all(
    any(target_arch = "x86_64", target_arch = "wasm32"),
    not(feature = "paranoid")
))]
use super::CpuTier;

#[inline]
pub fn append_literals(dst: &mut Vec<u8>, src: &[u8]) {
    if src.is_empty() {
        return;
    }

    #[cfg(not(feature = "paranoid"))]
    {
        let len = src.len();
        dst.reserve(len + 32);
        let dst_offset = dst.len();

        #[cfg(target_arch = "x86_64")]
        {
            let tier = super::cpu_tier();
            if tier >= CpuTier::Avx2 && len >= 32 {
                unsafe {
                    let dst_ptr = dst.as_mut_ptr().add(dst_offset);
                    super::x86_64::avx2::wildcopy_avx2(src.as_ptr(), dst_ptr, len);
                    dst.set_len(dst_offset + len);
                }
                return;
            }
            if tier >= CpuTier::Sse2 && len >= 16 {
                unsafe {
                    let dst_ptr = dst.as_mut_ptr().add(dst_offset);
                    super::x86_64::sse2::wildcopy_sse2(src.as_ptr(), dst_ptr, len);
                    dst.set_len(dst_offset + len);
                }
                return;
            }
        }

        #[cfg(target_arch = "aarch64")]
        {
            if len >= 16 {
                unsafe {
                    let dst_ptr = dst.as_mut_ptr().add(dst_offset);
                    super::aarch64::neon::wildcopy_neon(src.as_ptr(), dst_ptr, len);
                    dst.set_len(dst_offset + len);
                }
                return;
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            if super::cpu_tier() >= CpuTier::Wasm32Simd128 && len >= 16 {
                unsafe {
                    let dst_ptr = dst.as_mut_ptr().add(dst_offset);
                    super::wasm32::simd128::wildcopy_wasm32(src.as_ptr(), dst_ptr, len);
                    dst.set_len(dst_offset + len);
                }
                return;
            }
        }

        if len >= 8 {
            unsafe {
                let dst_ptr = dst.as_mut_ptr().add(dst_offset);
                super::scalar::wildcopy_nonoverlap(src.as_ptr(), dst_ptr, len);
                dst.set_len(dst_offset + len);
            }
        } else {
            dst.extend_from_slice(src);
        }
    }

    #[cfg(feature = "paranoid")]
    dst.extend_from_slice(src);
}

#[inline]
pub fn append_match(dst: &mut Vec<u8>, offset: usize, match_length: usize) {
    if match_length == 0 {
        return;
    }

    debug_assert!(offset <= dst.len());
    debug_assert!(offset > 0);

    #[cfg(not(feature = "paranoid"))]
    {
        dst.reserve(match_length + 32);
        let dst_len = dst.len();
        unsafe {
            let base = dst.as_mut_ptr();
            let write_ptr = base.add(dst_len);
            super::scalar::copy_match(write_ptr, offset, match_length);
            dst.set_len(dst_len + match_length);
        }
    }

    #[cfg(feature = "paranoid")]
    {
        dst.reserve(match_length);
        let start = dst.len() - offset;
        if offset >= match_length {
            let len = dst.len();
            dst.resize(len + match_length, 0);
            dst.copy_within(start..start + match_length, len);
        } else {
            for i in 0..match_length {
                let b = dst[start + i % offset];
                dst.push(b);
            }
        }
    }
}

#[inline]
pub fn common_prefix_len(a: &[u8], b: &[u8]) -> usize {
    #[cfg(all(target_arch = "x86_64", not(feature = "paranoid")))]
    {
        let tier = super::cpu_tier();
        if tier >= CpuTier::Avx2 && a.len() >= 32 && b.len() >= 32 {
            return unsafe { super::x86_64::avx2::common_prefix_len_avx2(a, b) };
        }
    }
    #[cfg(all(target_arch = "aarch64", not(feature = "paranoid")))]
    {
        if a.len() >= 16 && b.len() >= 16 {
            return unsafe { super::aarch64::neon::common_prefix_len_neon(a, b) };
        }
    }
    #[cfg(all(target_arch = "wasm32", not(feature = "paranoid")))]
    {
        if super::cpu_tier() >= CpuTier::Wasm32Simd128 && a.len() >= 16 && b.len() >= 16 {
            return unsafe { super::wasm32::simd128::common_prefix_len_wasm32(a, b) };
        }
    }
    super::scalar::common_prefix_len(a, b)
}

#[inline]
pub fn append_match_with_history(
    dst: &mut Vec<u8>,
    history: &[u8],
    offset: usize,
    match_length: usize,
) {
    if match_length == 0 {
        return;
    }

    let out_len = dst.len();

    if offset <= out_len {
        append_match(dst, offset, match_length);
    } else {
        let history_reach = offset - out_len;
        let history_start = history.len() - history_reach;
        let from_history = history_reach.min(match_length);

        dst.extend_from_slice(&history[history_start..history_start + from_history]);

        let remaining = match_length - from_history;
        if remaining > 0 {
            append_match(dst, offset, remaining);
        }
    }
}
