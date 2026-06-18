#![forbid(unsafe_code)]

pub const PRIME32_1: u32 = 0x9E37_79B1;
pub const PRIME32_2: u32 = 0x85EB_CA77;
pub const PRIME64_1: u64 = 0x9E37_79B1_85EB_CA87;

#[inline]
pub fn hash4(value: u32, hash_log: u32) -> u32 {
    (value.wrapping_mul(PRIME32_1)) >> (32 - hash_log)
}

#[inline]
pub fn hash5(value: u64, hash_log: u32) -> u32 {
    ((value << 24).wrapping_mul(PRIME64_1) >> (64 - hash_log)) as u32
}

#[inline]
pub fn hash6(value: u64, hash_log: u32) -> u32 {
    ((value << 16).wrapping_mul(PRIME64_1) >> (64 - hash_log)) as u32
}

#[inline]
pub fn hash7(value: u64, hash_log: u32) -> u32 {
    ((value << 8).wrapping_mul(PRIME64_1) >> (64 - hash_log)) as u32
}

#[inline]
pub fn hash8(value: u64, hash_log: u32) -> u32 {
    (value.wrapping_mul(PRIME64_1) >> (64 - hash_log)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash4_distributes() {
        let mut buckets = [0u32; 256];
        for i in 0..4096u32 {
            let h = hash4(i, 8) as usize;
            buckets[h] += 1;
        }
        let used = buckets.iter().filter(|&&c| c > 0).count();
        assert!(used > 200, "hash4 only hit {used}/256 buckets");
    }

    #[test]
    fn hash8_distributes() {
        let mut buckets = [0u32; 256];
        for i in 0..4096u64 {
            let h = hash8(i, 8) as usize;
            buckets[h] += 1;
        }
        let used = buckets.iter().filter(|&&c| c > 0).count();
        assert!(used > 200, "hash8 only hit {used}/256 buckets");
    }
}
