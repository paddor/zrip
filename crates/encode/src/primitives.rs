//! Small helpers the match finders and sequence encoders share: byte reads,
//! table access, match counting, and the sequence bitstream buffer.
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

// Native byte order, like C zstd's `MEM_read32`.
#[inline(always)]
pub(crate) fn rd32(src: &[u8], pos: usize) -> u32 {
    u32::from_ne_bytes(*src[pos..].first_chunk::<4>().unwrap())
}

/// Eight bytes at `pos`, in native byte order like [`rd32`].
#[inline(always)]
pub(crate) fn rd64(src: &[u8], pos: usize) -> u64 {
    u64::from_ne_bytes(*src[pos..].first_chunk::<8>().unwrap())
}

/// `rd64(src, pos)` for a `pos` the caller knows is at most `limit`, where
/// the compiler sees `limit + 8 <= src.len()`. Reading at `pos.min(limit)`
/// turns both bounds checks into one conditional move.
#[inline(always)]
pub(crate) fn rd64_upto(src: &[u8], pos: usize, limit: usize) -> u64 {
    debug_assert!(pos <= limit);
    rd64(src, pos.min(limit))
}

/// The first four bytes in memory of eight bytes read with `rd64`, as `rd32`
/// would read them.
#[inline(always)]
pub(crate) fn first4(v: u64) -> u32 {
    let b = v.to_ne_bytes();
    u32::from_ne_bytes([b[0], b[1], b[2], b[3]])
}

/// Eight bytes at `pos`, read once for `rd64_again`.
#[inline(always)]
pub(crate) fn rd64_ahead(src: &[u8], pos: usize) -> u64 {
    rd64(src, pos)
}

/// `rd64(src, pos)` where `v` already holds it, returned to skip a bounds
/// check.
#[inline(always)]
pub(crate) fn rd64_again(_src: &[u8], _pos: usize, v: u64) -> u64 {
    v
}

/// `table` cut to exactly `1 << LOG` entries when `LOG` is a constant, so
/// the compiler sees that a `LOG`-bit hash indexes in bounds and drops the
/// checks of safe indexing.
#[inline(always)]
pub(crate) fn exact_table<const LOG: u32>(table: &mut [u32]) -> &mut [u32] {
    if LOG != 0 {
        &mut table[..1 << LOG]
    } else {
        table
    }
}

#[inline(always)]
pub(crate) fn hash_load(table: &[u32], idx: usize) -> u32 {
    table[idx]
}

#[inline(always)]
pub(crate) fn hash_store(table: &mut [u32], idx: usize, val: u32) {
    table[idx] = val;
}

/// Counts equal bytes forward from `p1` and `p2`, up to `limit` for `p1`.
/// `p2` must lie behind `p1`.
#[inline(always)]
pub(crate) fn count_match(src: &[u8], p1: usize, p2: usize, limit: usize) -> usize {
    debug_assert!(p1 <= limit && limit <= src.len());
    debug_assert!(p2 < p1, "match position must be behind cursor");
    // Two equal-length slices up front, so the chunk loop needs no checks.
    let a = &src[p1..limit];
    let b = &src[p2..][..a.len()];
    let mut i = 0;
    for (ca, cb) in a.as_chunks::<8>().0.iter().zip(b.as_chunks::<8>().0) {
        let diff = u64::from_le_bytes(*ca) ^ u64::from_le_bytes(*cb);
        if diff != 0 {
            return i + (diff.trailing_zeros() >> 3) as usize;
        }
        i += 8;
    }
    i + a[i..]
        .iter()
        .zip(&b[i..])
        .take_while(|(x, y)| x == y)
        .count()
}

/// Panics if `r0` or `r1` is zero. Rep checks read at `ip - rep`, which
/// must lie behind the cursor.
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

/// Forward bit-output buffer for sequence bitstreams, appended to the end of
/// the vector it is given.
///
/// `new` zero-fills `reserve` bytes past the vector's end, so every write
/// within them is a plain in-bounds slice store; a write past them grows the
/// vector. Positions count from where the stream starts, and `finish` cuts
/// the vector to the stream's end.
#[cfg(feature = "alloc")]
pub(crate) struct BitstreamScratch<'a> {
    buf: &'a mut Vec<u8>,
    base: usize,
}

#[cfg(feature = "alloc")]
impl<'a> BitstreamScratch<'a> {
    #[inline(always)]
    pub(crate) fn new(buf: &'a mut Vec<u8>, reserve: usize) -> Self {
        let base = buf.len();
        buf.resize(base + reserve, 0);
        Self { buf, base }
    }

    /// Stores all 64 `bits` at `pos`. The caller then advances `pos` by
    /// the whole bytes among them.
    #[inline(always)]
    pub(crate) fn flush(&mut self, pos: usize, bits: u64) {
        let at = self.base + pos;
        match self.buf.get_mut(at..at + 8) {
            Some(dst) => dst.copy_from_slice(&bits.to_le_bytes()),
            None => self.grow_and_flush(at, bits),
        }
    }

    #[cold]
    #[inline(never)]
    fn grow_and_flush(&mut self, at: usize, bits: u64) {
        let needed = (at + 8).max(self.buf.len() * 2);
        self.buf.resize(needed, 0);
        self.buf[at..at + 8].copy_from_slice(&bits.to_le_bytes());
    }

    #[inline(always)]
    pub(crate) fn write_byte(&mut self, pos: usize, val: u8) {
        let at = self.base + pos;
        if at >= self.buf.len() {
            self.buf.resize(at + 1, 0);
        }
        self.buf[at] = val;
    }

    /// Ends the stream after `len` bytes.
    #[inline(always)]
    pub(crate) fn finish(self, len: usize) {
        assert!(self.base + len <= self.buf.len());
        self.buf.truncate(self.base + len);
    }
}

#[cfg(test)]
mod tests {
    use super::count_match;

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
