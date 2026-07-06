#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    let end = offset.checked_add(4).expect("read offset overflow");
    assert!(end <= data.len());
    // SAFETY: The assertion proves offset..offset+4 is inside data, and
    // read_unaligned accepts any alignment.
    u32::from_le(unsafe { (data.as_ptr().add(offset) as *const u32).read_unaligned() })
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn read_u64_le(data: &[u8], offset: usize) -> u64 {
    let end = offset.checked_add(8).expect("read offset overflow");
    assert!(end <= data.len());
    // SAFETY: The assertion proves offset..offset+8 is inside data, and
    // read_unaligned accepts any alignment.
    u64::from_le(unsafe { (data.as_ptr().add(offset) as *const u64).read_unaligned() })
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn read_u64_le(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(super) fn bulk_rounds(data: &[u8], v1: &mut u64, v2: &mut u64, v3: &mut u64, v4: &mut u64) {
    let len = data.len();
    debug_assert!(len >= 32);

    // SAFETY: bulk_end and unroll_end are rounded down from len, so every
    // pointer stays within data. Each loop body reads complete 8-byte lanes
    // before advancing beyond those rounded limits.
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

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(super) fn bulk_rounds(data: &[u8], v1: &mut u64, v2: &mut u64, v3: &mut u64, v4: &mut u64) {
    let len = data.len();
    debug_assert!(len >= 32);

    #[inline(always)]
    fn rd64(data: &[u8], offset: usize) -> u64 {
        u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
    }

    let bulk_end = len & !31;
    let unroll_end = len & !127;
    let mut offset = 0;

    while offset < unroll_end {
        *v1 = super::xxh64_round(*v1, rd64(data, offset));
        *v2 = super::xxh64_round(*v2, rd64(data, offset + 8));
        *v3 = super::xxh64_round(*v3, rd64(data, offset + 16));
        *v4 = super::xxh64_round(*v4, rd64(data, offset + 24));

        *v1 = super::xxh64_round(*v1, rd64(data, offset + 32));
        *v2 = super::xxh64_round(*v2, rd64(data, offset + 40));
        *v3 = super::xxh64_round(*v3, rd64(data, offset + 48));
        *v4 = super::xxh64_round(*v4, rd64(data, offset + 56));

        *v1 = super::xxh64_round(*v1, rd64(data, offset + 64));
        *v2 = super::xxh64_round(*v2, rd64(data, offset + 72));
        *v3 = super::xxh64_round(*v3, rd64(data, offset + 80));
        *v4 = super::xxh64_round(*v4, rd64(data, offset + 88));

        *v1 = super::xxh64_round(*v1, rd64(data, offset + 96));
        *v2 = super::xxh64_round(*v2, rd64(data, offset + 104));
        *v3 = super::xxh64_round(*v3, rd64(data, offset + 112));
        *v4 = super::xxh64_round(*v4, rd64(data, offset + 120));

        offset += 128;
    }

    while offset < bulk_end {
        *v1 = super::xxh64_round(*v1, rd64(data, offset));
        *v2 = super::xxh64_round(*v2, rd64(data, offset + 8));
        *v3 = super::xxh64_round(*v3, rd64(data, offset + 16));
        *v4 = super::xxh64_round(*v4, rd64(data, offset + 24));
        offset += 32;
    }
}
