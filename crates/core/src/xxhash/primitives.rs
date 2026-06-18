#[inline(always)]
pub(crate) fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    debug_assert!(offset + 4 <= data.len());
    u32::from_le(unsafe { (data.as_ptr().add(offset) as *const u32).read_unaligned() })
}

#[inline(always)]
pub(crate) fn read_u64_le(data: &[u8], offset: usize) -> u64 {
    debug_assert!(offset + 8 <= data.len());
    u64::from_le(unsafe { (data.as_ptr().add(offset) as *const u64).read_unaligned() })
}

#[inline(always)]
pub(super) fn bulk_rounds(data: &[u8], v1: &mut u64, v2: &mut u64, v3: &mut u64, v4: &mut u64) {
    let len = data.len();
    debug_assert!(len >= 32);

    unsafe {
        let mut p = data.as_ptr();
        let bulk_end = data.as_ptr().add(len & !31);
        let unroll_end = data.as_ptr().add(len & !127);

        while p < unroll_end {
            *v1 = super::xxh64_round(*v1, (p as *const u64).read_unaligned().to_le());
            *v2 = super::xxh64_round(*v2, (p.add(8) as *const u64).read_unaligned().to_le());
            *v3 = super::xxh64_round(*v3, (p.add(16) as *const u64).read_unaligned().to_le());
            *v4 = super::xxh64_round(*v4, (p.add(24) as *const u64).read_unaligned().to_le());

            *v1 = super::xxh64_round(*v1, (p.add(32) as *const u64).read_unaligned().to_le());
            *v2 = super::xxh64_round(*v2, (p.add(40) as *const u64).read_unaligned().to_le());
            *v3 = super::xxh64_round(*v3, (p.add(48) as *const u64).read_unaligned().to_le());
            *v4 = super::xxh64_round(*v4, (p.add(56) as *const u64).read_unaligned().to_le());

            *v1 = super::xxh64_round(*v1, (p.add(64) as *const u64).read_unaligned().to_le());
            *v2 = super::xxh64_round(*v2, (p.add(72) as *const u64).read_unaligned().to_le());
            *v3 = super::xxh64_round(*v3, (p.add(80) as *const u64).read_unaligned().to_le());
            *v4 = super::xxh64_round(*v4, (p.add(88) as *const u64).read_unaligned().to_le());

            *v1 = super::xxh64_round(*v1, (p.add(96) as *const u64).read_unaligned().to_le());
            *v2 = super::xxh64_round(*v2, (p.add(104) as *const u64).read_unaligned().to_le());
            *v3 = super::xxh64_round(*v3, (p.add(112) as *const u64).read_unaligned().to_le());
            *v4 = super::xxh64_round(*v4, (p.add(120) as *const u64).read_unaligned().to_le());

            p = p.add(128);
        }

        while p < bulk_end {
            *v1 = super::xxh64_round(*v1, (p as *const u64).read_unaligned().to_le());
            *v2 = super::xxh64_round(*v2, (p.add(8) as *const u64).read_unaligned().to_le());
            *v3 = super::xxh64_round(*v3, (p.add(16) as *const u64).read_unaligned().to_le());
            *v4 = super::xxh64_round(*v4, (p.add(24) as *const u64).read_unaligned().to_le());
            p = p.add(32);
        }
    }
}
