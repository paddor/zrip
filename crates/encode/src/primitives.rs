#[cfg(feature = "alloc")]
use alloc::vec::Vec;

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) unsafe fn rd32(src: &[u8], pos: usize) -> u32 {
    debug_assert!(pos + 4 <= src.len());
    // SAFETY: The caller guarantees pos..pos+4 is inside src. read_unaligned
    // permits any byte alignment.
    unsafe { (src.as_ptr().add(pos) as *const u32).read_unaligned() }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn rd32(src: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes(*src[pos..].first_chunk::<4>().unwrap())
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) unsafe fn rd64(src: &[u8], pos: usize) -> u64 {
    debug_assert!(pos + 8 <= src.len());
    // SAFETY: The caller guarantees pos..pos+8 is inside src. read_unaligned
    // permits any byte alignment.
    unsafe { (src.as_ptr().add(pos) as *const u64).read_unaligned() }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn rd64(src: &[u8], pos: usize) -> u64 {
    u64::from_le_bytes(*src[pos..].first_chunk::<8>().unwrap())
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) unsafe fn hash_load(table: &[u32], idx: usize) -> u32 {
    debug_assert!(idx < table.len());
    // SAFETY: The caller guarantees idx is in bounds.
    unsafe { *table.get_unchecked(idx) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn hash_load(table: &[u32], idx: usize) -> u32 {
    table[idx]
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) unsafe fn hash_store(table: &mut [u32], idx: usize, val: u32) {
    debug_assert!(idx < table.len());
    // SAFETY: The caller guarantees idx is in bounds.
    unsafe { *table.get_unchecked_mut(idx) = val }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn hash_store(table: &mut [u32], idx: usize, val: u32) {
    table[idx] = val;
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) unsafe fn match_at<const MLS: usize>(src: &[u8], a: usize, b: usize) -> bool {
    if MLS >= 7 {
        // SAFETY: The caller guarantees both reads are in bounds.
        unsafe { rd64(src, a) == rd64(src, b) }
    } else if MLS >= 5 {
        // SAFETY: The caller guarantees both reads are in bounds.
        let va = unsafe { rd64(src, a) };
        let vb = unsafe { rd64(src, b) };
        ((va ^ vb) << (64 - 8 * MLS)) == 0
    } else {
        // SAFETY: The caller guarantees both reads are in bounds.
        unsafe { rd32(src, a) == rd32(src, b) }
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn match_at<const MLS: usize>(src: &[u8], a: usize, b: usize) -> bool {
    if MLS >= 7 {
        rd64(src, a) == rd64(src, b)
    } else if MLS >= 5 {
        let va = rd64(src, a);
        let vb = rd64(src, b);
        ((va ^ vb) << (64 - 8 * MLS)) == 0
    } else {
        rd32(src, a) == rd32(src, b)
    }
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) unsafe fn count_match(src: &[u8], p1: usize, p2: usize, limit: usize) -> usize {
    debug_assert!(p1 <= limit && limit <= src.len());
    debug_assert!(p2 < p1, "match position must be behind cursor");
    debug_assert!(p2 < src.len());
    let src_ptr = src.as_ptr();
    // SAFETY: p1..limit and p2.. are inside src, and p2 is behind p1. The raw
    // matcher only reads as far as p1 reaches limit.
    unsafe { count_match_raw(src_ptr.add(p1), src_ptr.add(p2), src_ptr.add(limit)) }
}

/// # Safety
///
/// `p_in..p_in_limit` must be inside one allocation, and `p_match` must point
/// to an earlier position in the same allocation with enough trailing bytes to
/// compare until `p_in_limit`.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn count_match_raw(
    mut p_in: *const u8,
    mut p_match: *const u8,
    p_in_limit: *const u8,
) -> usize {
    debug_assert!(p_match < p_in);
    debug_assert!(p_in <= p_in_limit);
    // SAFETY: The caller supplies pointers from one slice with p_in <=
    // p_in_limit. fast_len is rounded down from that in-bounds span, so no
    // pointer is formed before the allocation for short matches.
    unsafe {
        let p_start = p_in;
        let max_len = p_in_limit.offset_from(p_in) as usize;
        let fast_len = max_len & !7;
        let fast_limit = p_in.add(fast_len);

        while p_in < fast_limit {
            let diff =
                (p_in as *const u64).read_unaligned() ^ (p_match as *const u64).read_unaligned();
            if diff != 0 {
                return p_in.offset_from(p_start) as usize + (diff.trailing_zeros() >> 3) as usize;
            }
            p_in = p_in.add(8);
            p_match = p_match.add(8);
        }
        while p_in < p_in_limit {
            if *p_in != *p_match {
                break;
            }
            p_in = p_in.add(1);
            p_match = p_match.add(1);
        }
        p_in.offset_from(p_start) as usize
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn count_match(src: &[u8], p1: usize, p2: usize, limit: usize) -> usize {
    debug_assert!(p1 <= limit && limit <= src.len());
    debug_assert!(p2 < p1, "match position must be behind cursor");
    let max_len = limit - p1;
    let mut i = 0;
    while i + 8 <= max_len {
        let a = u64::from_le_bytes(*src[p1 + i..].first_chunk::<8>().unwrap());
        let b = u64::from_le_bytes(*src[p2 + i..].first_chunk::<8>().unwrap());
        let diff = a ^ b;
        if diff != 0 {
            return i + (diff.trailing_zeros() >> 3) as usize;
        }
        i += 8;
    }
    while i < max_len {
        if src[p1 + i] != src[p2 + i] {
            break;
        }
        i += 1;
    }
    i
}

#[inline(always)]
pub(crate) fn assert_rep_valid(r0: u32, r1: u32) {
    if r0 == 0 || r1 == 0 {
        cold_rep_panic(r0, r1);
    }
}

#[cold]
#[inline(never)]
fn cold_rep_panic(r0: u32, r1: u32) -> ! {
    panic!("rep offsets must be non-zero: r0={r0}, r1={r1}");
}

#[cfg(all(target_arch = "x86_64", not(miri), not(feature = "paranoid")))]
#[inline(always)]
pub(crate) fn prefetch_ht(table: &[u32], idx: usize) {
    if let Some(slot) = table.get(idx) {
        // SAFETY: slot comes from a valid shared reference. Prefetch is only a
        // cache hint and does not mutate through the pointer.
        unsafe {
            core::arch::x86_64::_mm_prefetch(
                core::ptr::from_ref(slot).cast::<i8>(),
                core::arch::x86_64::_MM_HINT_T0,
            );
        }
    }
}

#[cfg(all(target_arch = "aarch64", not(miri), not(feature = "paranoid")))]
#[inline(always)]
pub(crate) fn prefetch_ht(table: &[u32], idx: usize) {
    if let Some(slot) = table.get(idx) {
        // SAFETY: slot comes from a valid shared reference, and the inline
        // assembly emits only an AArch64 prefetch hint for that address.
        unsafe {
            let ptr = core::ptr::from_ref(slot).cast::<u8>();
            core::arch::asm!("prfm pldl1keep, [{x}]", x = in(reg) ptr, options(nostack, preserves_flags));
        }
    }
}

#[cfg(any(
    all(
        any(miri, feature = "paranoid"),
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    not(any(target_arch = "x86_64", target_arch = "aarch64"))
))]
#[inline(always)]
#[allow(dead_code)]
pub(crate) fn prefetch_ht(_table: &[u32], _idx: usize) {}

#[cfg(feature = "alloc")]
pub(crate) struct BitstreamScratch<'a> {
    buf: &'a mut Vec<u8>,
    initialized: usize,
}

#[cfg(feature = "alloc")]
impl<'a> BitstreamScratch<'a> {
    #[inline(always)]
    pub(crate) fn new(buf: &'a mut Vec<u8>, reserve: usize) -> Self {
        buf.clear();
        buf.reserve(reserve);
        Self {
            buf,
            initialized: 0,
        }
    }

    #[inline(always)]
    pub(crate) fn flush(&mut self, pos: usize, bits: u64) {
        let needed = pos + 8;
        self.ensure_capacity(needed);

        #[cfg(not(feature = "paranoid"))]
        {
            // SAFETY: ensure_capacity proves the 8-byte write fits in the Vec
            // allocation. initialized tracks the largest written range before
            // finish exposes bytes through the Vec length.
            unsafe {
                (self.buf.as_mut_ptr().add(pos) as *mut u64).write_unaligned(bits.to_le());
            }
        }

        #[cfg(feature = "paranoid")]
        {
            if self.buf.len() < needed {
                self.buf.resize(needed, 0);
            }
            self.buf[pos..needed].copy_from_slice(&bits.to_le_bytes());
        }

        self.initialized = self.initialized.max(needed);
    }

    #[inline(always)]
    pub(crate) fn write_byte(&mut self, pos: usize, val: u8) {
        let needed = pos + 1;
        self.ensure_capacity(needed);

        #[cfg(not(feature = "paranoid"))]
        {
            // SAFETY: ensure_capacity proves the byte write fits in the Vec
            // allocation. initialized tracks the byte before finish exposes it.
            unsafe { *self.buf.as_mut_ptr().add(pos) = val }
        }

        #[cfg(feature = "paranoid")]
        {
            if self.buf.len() < needed {
                self.buf.resize(needed, 0);
            }
            self.buf[pos] = val;
        }

        self.initialized = self.initialized.max(needed);
    }

    #[inline(always)]
    pub(crate) fn finish(&mut self, len: usize) {
        assert!(len <= self.initialized);

        #[cfg(not(feature = "paranoid"))]
        {
            // SAFETY: flush and write_byte initialized every byte range that
            // callers expose. finish refuses to expose bytes beyond that range.
            unsafe { self.buf.set_len(len) }
        }

        #[cfg(feature = "paranoid")]
        {
            self.buf.truncate(len);
        }
    }

    #[inline(always)]
    pub(crate) fn as_slice(&self) -> &[u8] {
        self.buf
    }

    #[inline(always)]
    fn ensure_capacity(&mut self, needed: usize) {
        if needed > self.buf.capacity() {
            self.buf.reserve(needed - self.buf.capacity());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::count_match;

    #[cfg(not(feature = "paranoid"))]
    fn test_count_match(src: &[u8], p1: usize, p2: usize, limit: usize) -> usize {
        // SAFETY: test cases pass in-bounds positions with p2 behind p1.
        unsafe { count_match(src, p1, p2, limit) }
    }

    #[cfg(feature = "paranoid")]
    fn test_count_match(src: &[u8], p1: usize, p2: usize, limit: usize) -> usize {
        count_match(src, p1, p2, limit)
    }

    #[test]
    fn count_match_handles_short_limits() {
        let src = b"abcdabcx";

        assert_eq!(test_count_match(src, 4, 0, 4), 0);
        assert_eq!(test_count_match(src, 4, 0, 5), 1);
        assert_eq!(test_count_match(src, 4, 0, 7), 3);
        assert_eq!(test_count_match(src, 4, 0, 8), 3);
    }

    #[test]
    fn count_match_handles_exact_eight_byte_match() {
        let src = b"abcdefghabcdefghq";

        assert_eq!(test_count_match(src, 8, 0, 16), 8);
        assert_eq!(test_count_match(src, 8, 0, 17), 8);
    }
}
