//! Little-endian loads and the XXH64 stripe loop.

#[inline(always)]
pub(crate) fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(*data[offset..].first_chunk::<4>().unwrap())
}

#[inline(always)]
pub(crate) fn read_u64_le(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(*data[offset..].first_chunk::<8>().unwrap())
}

/// Runs the four lane accumulators over every whole 32-byte stripe of
/// `data`, which holds at least one. The caller hashes the tail.
#[inline(always)]
pub(super) fn bulk_rounds(data: &[u8], v1: &mut u64, v2: &mut u64, v3: &mut u64, v4: &mut u64) {
    debug_assert!(data.len() >= 32);

    /// One stripe: 32 bytes as four little-endian lanes. Whole arrays need
    /// no bounds checks.
    #[inline(always)]
    fn stripe(acc: &mut [u64; 4], s: &[u8; 32]) {
        let (lanes, _) = s.as_chunks::<8>();
        for (a, lane) in acc.iter_mut().zip(lanes) {
            *a = super::xxh64_round(*a, u64::from_le_bytes(*lane));
        }
    }

    let mut acc = [*v1, *v2, *v3, *v4];
    let (blocks, rest) = data.as_chunks::<128>();
    for block in blocks {
        for s in block.as_chunks::<32>().0 {
            stripe(&mut acc, s);
        }
    }
    for s in rest.as_chunks::<32>().0 {
        stripe(&mut acc, s);
    }
    [*v1, *v2, *v3, *v4] = acc;
}
