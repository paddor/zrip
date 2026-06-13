#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::*;

/// Extract bits [0..n) from `val` using BMI2 `_bzhi_u64`.
/// Equivalent to `val & ((1 << n) - 1)` but avoids branch on n==64.
///
/// # Safety
/// BMI2 must be available.
#[target_feature(enable = "bmi2")]
#[inline]
pub unsafe fn bzhi(val: u64, n: u32) -> u64 {
    debug_assert!(n <= 64);
    _bzhi_u64(val, n)
}

/// Parallel bit extract using BMI2 `_pext_u64`.
/// Extracts bits from `val` at positions where `mask` has 1-bits,
/// packing them into contiguous low bits of the result.
///
/// # Safety
/// BMI2 must be available.
#[target_feature(enable = "bmi2")]
#[inline]
pub unsafe fn pext(val: u64, mask: u64) -> u64 {
    _pext_u64(val, mask)
}

/// Parallel bit deposit using BMI2 `_pdep_u64`.
/// Deposits contiguous low bits of `val` into positions where `mask` has 1-bits.
///
/// # Safety
/// BMI2 must be available.
#[target_feature(enable = "bmi2")]
#[inline]
pub unsafe fn pdep(val: u64, mask: u64) -> u64 {
    _pdep_u64(val, mask)
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    #[test]
    fn bzhi_basic() {
        if !std::arch::is_x86_feature_detected!("bmi2") {
            return;
        }
        unsafe {
            assert_eq!(bzhi(0xFF, 4), 0x0F);
            assert_eq!(bzhi(0xFFFF_FFFF_FFFF_FFFF, 1), 1);
            assert_eq!(bzhi(0xFFFF_FFFF_FFFF_FFFF, 0), 0);
            assert_eq!(bzhi(0xDEAD_BEEF, 16), 0xBEEF);
        }
    }

    #[test]
    fn pext_basic() {
        if !std::arch::is_x86_feature_detected!("bmi2") {
            return;
        }
        unsafe {
            // mask=0xF0: extract bits 4-7 of 0xAA (=0b1010) -> 0b1010 = 0xA
            assert_eq!(pext(0b1010_1010, 0b1111_0000), 0b1010);
            // mask=0b1001_0110: positions 1,2,4,7. val=0b1100_0011:
            // pos1=1, pos2=0, pos4=0, pos7=1 -> packed = 0b1001 = 9
            assert_eq!(pext(0b1100_0011, 0b1001_0110), 0b1001);
        }
    }

    #[test]
    fn pdep_basic() {
        if !std::arch::is_x86_feature_detected!("bmi2") {
            return;
        }
        unsafe {
            assert_eq!(pdep(0b1010, 0b1111_0000), 0b1010_0000);
        }
    }
}
