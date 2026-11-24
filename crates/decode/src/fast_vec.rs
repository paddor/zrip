//! Sequence output and copy primitives: [`SeqOut`] with chunked wild
//! copies for the sequence loop, and the appending [`BlockOutput`] for
//! blocks with one sequence.

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use zrip_core::error::DecompressError;
use zrip_core::hint::{likely, unlikely};

/// Slack past a block's limit that wild copies may write into. They write
/// at most 32 bytes past a sequence's end.
pub(crate) const WILDCOPY_OVERLENGTH: usize = 64;

/// A block's output for the single-sequence path, appended to `vec`.
pub(crate) struct BlockOutput<'a> {
    vec: &'a mut Vec<u8>,
    /// `vec.len()` the block may not pass.
    block_limit: usize,
}

/// One sequence's writes into a [`BlockOutput`]. `begin_sequence` checked
/// that its lengths fit the block.
pub(crate) struct SequenceOutput<'block, 'vec> {
    output: &'block mut BlockOutput<'vec>,
    literal_len: usize,
    match_len: usize,
}

impl<'a> BlockOutput<'a> {
    /// Starts a block of at most `max_block_size` bytes at the end of `vec`,
    /// reserving the slack for fixed-size literal copies.
    #[inline(always)]
    pub(crate) fn new(vec: &'a mut Vec<u8>, max_block_size: usize) -> Self {
        vec.reserve(max_block_size + WILDCOPY_OVERLENGTH);
        let block_limit = vec.len() + max_block_size;
        Self { vec, block_limit }
    }

    /// Output length, bytes before the block included.
    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        self.vec.len()
    }

    /// Starts a sequence of `literal_len` literals and a `match_len` match.
    /// Fails with `BlockTooLarge` if they pass the block's limit.
    #[inline(always)]
    pub(crate) fn begin_sequence(
        &mut self,
        literal_len: usize,
        match_len: usize,
    ) -> Result<SequenceOutput<'_, 'a>, DecompressError> {
        debug_assert!(self.vec.len() <= self.block_limit);
        let remaining = self.block_limit - self.vec.len();
        if unlikely(literal_len > remaining || match_len > remaining - literal_len) {
            return Err(DecompressError::BlockTooLarge);
        }
        Ok(SequenceOutput {
            output: self,
            literal_len,
            match_len,
        })
    }

    #[inline(always)]
    fn ensure_block_space(&self, len: usize) -> Result<(), DecompressError> {
        debug_assert!(self.vec.len() <= self.block_limit);
        if unlikely(len > self.block_limit - self.vec.len()) {
            return Err(DecompressError::BlockTooLarge);
        }
        Ok(())
    }

    /// Appends `src[start..start + len]` within the block's limit.
    #[inline(always)]
    pub(crate) fn extend_literals_range(
        &mut self,
        src: &[u8],
        start: usize,
        len: usize,
    ) -> Result<(), DecompressError> {
        self.extend_slice_range(src, start, len, DecompressError::CorruptLiterals)
    }

    #[inline(always)]
    fn extend_slice_range(
        &mut self,
        src: &[u8],
        start: usize,
        len: usize,
        bounds_error: DecompressError,
    ) -> Result<(), DecompressError> {
        if unlikely(start > src.len() || len > src.len() - start) {
            return Err(bounds_error);
        }
        self.ensure_block_space(len)?;
        self.extend_slice_range_in_block(src, start, len);
        Ok(())
    }

    #[inline(always)]
    fn extend_slice_range_in_block(&mut self, src: &[u8], start: usize, len: usize) {
        // Short runs append a fixed 16 bytes and truncate: a constant-size
        // copy has no length-dependent branches. `new` reserved the slack,
        // so neither step reallocates.
        if len <= 16 && src.len() - start >= 16 {
            let end = self.vec.len() + len;
            self.vec.extend_from_slice(&src[start..start + 16]);
            self.vec.truncate(end);
        } else {
            self.vec.extend_from_slice(&src[start..start + len]);
        }
    }
}

impl SequenceOutput<'_, '_> {
    /// Output length, bytes before the block included.
    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        self.output.len()
    }

    /// Appends the sequence's literals from `src[start..]`.
    #[inline(always)]
    pub(crate) fn extend_literals_range(
        &mut self,
        src: &[u8],
        start: usize,
    ) -> Result<(), DecompressError> {
        self.extend_slice_range(
            src,
            start,
            self.literal_len,
            DecompressError::CorruptLiterals,
        )
    }

    #[inline(always)]
    fn extend_slice_range(
        &mut self,
        src: &[u8],
        start: usize,
        len: usize,
        bounds_error: DecompressError,
    ) -> Result<(), DecompressError> {
        if unlikely(start > src.len() || len > src.len() - start) {
            return Err(bounds_error);
        }
        self.output.extend_slice_range_in_block(src, start, len);
        Ok(())
    }

    /// Appends the sequence's match from `offset` bytes back in the output.
    #[inline(always)]
    pub(crate) fn copy_match_single(&mut self, offset: usize) -> Result<(), DecompressError> {
        if unlikely(offset == 0 || offset > self.output.vec.len()) {
            return Err(DecompressError::InvalidOffset);
        }
        let len = self.match_len;
        if len == 0 {
            return Ok(());
        }

        wild_copy_match(self.output.vec, offset, len);
        Ok(())
    }

    /// Appends the sequence's match when it starts in `history`, before the
    /// frame, and may continue into the output. `out_pos` is the output
    /// position from the frame's first byte; `offset` must exceed it.
    #[inline(always)]
    pub(crate) fn copy_match_from_history(
        &mut self,
        history: &[u8],
        offset: usize,
        out_pos: usize,
    ) -> Result<(), DecompressError> {
        if unlikely(offset <= out_pos || offset > out_pos + history.len()) {
            return Err(DecompressError::InvalidOffset);
        }

        let history_reach = offset - out_pos;
        let history_start = history.len() - history_reach;
        let from_history = history_reach.min(self.match_len);
        self.extend_slice_range(
            history,
            history_start,
            from_history,
            DecompressError::InvalidOffset,
        )?;

        let remaining = self.match_len - from_history;
        if remaining > 0 {
            self.copy_match_len(offset, remaining)?;
        }
        Ok(())
    }

    #[inline(always)]
    fn copy_match_len(&mut self, offset: usize, len: usize) -> Result<(), DecompressError> {
        if unlikely(offset == 0 || offset > self.output.vec.len()) {
            return Err(DecompressError::InvalidOffset);
        }
        if len == 0 {
            return Ok(());
        }

        wild_copy_match(self.output.vec, offset, len);
        Ok(())
    }
}

/// Bytes the fast sequence path reads past the end of a literal run.
pub(crate) const LITERAL_OVERREAD: usize = 32;

/// A block's literals and the slack after them.
///
/// The fast sequence path copies literals in whole chunks and reads up to
/// [`LITERAL_OVERREAD`] bytes past a run, so it serves runs whose end plus
/// that overread lies inside `fast`: the literals and as much slack as the
/// bytes hold, at most `LITERAL_OVERREAD`. The exact path serves the rest.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Literals<'a> {
    fast: &'a [u8],
    len: usize,
}

impl<'a> Literals<'a> {
    /// The first `len` bytes of `bytes` are the literals; any bytes after
    /// them are slack the copies may read.
    pub(crate) fn new(bytes: &'a [u8], len: usize) -> Self {
        debug_assert!(len <= bytes.len());
        let len = len.min(bytes.len());
        Self {
            fast: &bytes[..(len + LITERAL_OVERREAD).min(bytes.len())],
            len,
        }
    }

    /// Number of literals.
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// The literals without the slack.
    pub(crate) fn as_slice(&self) -> &'a [u8] {
        &self.fast[..self.len]
    }
}

/// Bytes added to an overlapping match's source after its first four bytes,
/// per offset below eight (C zstd's `dec32table`).
const SPREAD_ADD: [usize; 8] = [0, 1, 2, 1, 4, 4, 4, 4];
/// Bytes then subtracted from the source (C zstd's `dec64table`).
const SPREAD_SUB: [usize; 8] = [8, 8, 8, 7, 8, 9, 10, 11];

/// One block's sequence output: the output from the lowest byte a match
/// may copy from, zeroed through the block's limit plus
/// [`WILDCOPY_OVERLENGTH`], so the copies index an initialized slice.
///
/// Positions count from that byte: the frame's first byte, or the window's
/// start when streaming. The block's limit is the slice's end less the
/// overlength. The loop then keeps only the slice and the output position
/// in registers. With `NARROW`, the slice ends below 4 GiB, and each
/// sequence masks its positions to 32 bits ([`ix`]).
pub(crate) struct SeqOut<'a, const NARROW: bool> {
    buf: &'a mut [u8],
    /// Bytes of output; every byte below it is initialized.
    op: usize,
}

/// Zeroes the room of a block of at most `max_block` bytes appended to `vec`,
/// whose matches reach back to `frame_start`. Returns where the block's
/// [`SeqOut`] starts in `vec` and its output position there.
#[inline(always)]
pub(crate) fn open_block(
    vec: &mut Vec<u8>,
    max_block: usize,
    frame_start: usize,
) -> (usize, usize) {
    let start = vec.len();
    let frame_start = frame_start.min(start);
    vec.resize(start + max_block + WILDCOPY_OVERLENGTH, 0);
    (frame_start, start - frame_start)
}

/// [`open_block`] for a block that starts at `start` in `vec`, before any
/// room earlier blocks left. Also returns where the block's room ends.
///
/// Only bytes past `vec.len()` are zeroed. The block writes each byte of
/// the room before it reads it, so the room needs no zeroing again: a
/// streaming decoder that keeps it zeroes each byte of its history once,
/// not a whole block's room for every block.
#[inline(always)]
pub(crate) fn open_block_at(
    vec: &mut Vec<u8>,
    start: usize,
    max_block: usize,
    frame_start: usize,
) -> (usize, usize, usize) {
    debug_assert!(start <= vec.len());
    let frame_start = frame_start.min(start);
    let end = start + max_block + WILDCOPY_OVERLENGTH;
    if vec.len() < end {
        vec.resize(end, 0);
    }
    (frame_start, start - frame_start, end)
}

impl<'a, const NARROW: bool> SeqOut<'a, NARROW> {
    /// The output in `buf`, which [`open_block`] returned, at position `op`.
    #[inline(always)]
    pub(crate) fn new(buf: &'a mut [u8], op: usize) -> Self {
        debug_assert!(op + WILDCOPY_OVERLENGTH <= buf.len());
        Self { buf, op }
    }

    /// The output position.
    #[inline(always)]
    pub(crate) fn op(&self) -> usize {
        self.op
    }
}

impl<const NARROW: bool> SeqOut<'_, NARROW> {
    /// Largest `op` the block may reach.
    #[inline(always)]
    fn limit(&self) -> usize {
        self.buf.len() - WILDCOPY_OVERLENGTH
    }

    /// Executes one sequence: `ll` literals from `lits[lp..]`, then `ml`
    /// bytes copied from `offset` bytes back. A match may reach before the
    /// frame into `history`.
    ///
    /// The fast path makes three tests: the block's limit, the literals'
    /// end against their slack, and the offset. Everything else, including
    /// the error checks in their original order, is in the cold
    /// [`execute_exact`]. With `NARROW`, the tests cover the bounds of each
    /// copy's first chunk, and the compiler drops those checks.
    #[inline(always)]
    pub(crate) fn execute(
        &mut self,
        lits: Literals<'_>,
        lp: usize,
        ll: usize,
        ml: usize,
        offset: usize,
        history: &[u8],
    ) -> Result<(), DecompressError> {
        let (op, lp) = (ix::<NARROW>(self.op), ix::<NARROW>(lp));
        let lit_end = op + ll;
        let match_end = lit_end + ml;
        if unlikely(match_end + WILDCOPY_OVERLENGTH > self.buf.len()) {
            return Err(DecompressError::BlockTooLarge);
        }
        // `checked_sub` tells the compiler that the source is at most
        // `lit_end`, which bounds its first chunk.
        match lit_end.checked_sub(offset) {
            Some(src) if likely(lp + ll + LITERAL_OVERREAD <= lits.fast.len() && offset != 0) => {
                let mut dst = Dst(self.buf);
                dst.wild_literals(op, lits.fast, lp, ll);
                dst.wild_match(lit_end, src, offset, ml);
                self.op = match_end;
                Ok(())
            }
            _ => {
                let span = Span { op, ll, ml };
                self.op = execute_exact(Dst(self.buf), span, lits.as_slice(), lp, offset, history)?;
                Ok(())
            }
        }
    }

    /// Appends `lits[lp..lp + len]` exactly.
    #[inline(always)]
    pub(crate) fn extend_literals(
        &mut self,
        lits: Literals<'_>,
        lp: usize,
        len: usize,
    ) -> Result<(), DecompressError> {
        let lits = lits.as_slice();
        if unlikely(lp > lits.len() || len > lits.len() - lp) {
            return Err(DecompressError::CorruptLiterals);
        }
        if unlikely(len > self.limit() - self.op) {
            return Err(DecompressError::BlockTooLarge);
        }
        let op = self.op;
        Dst(self.buf).put(op, &lits[lp..lp + len]);
        self.op += len;
        Ok(())
    }
}

/// A sequence's place in the output: where it starts and its literal and
/// match lengths.
#[derive(Clone, Copy)]
struct Span {
    op: usize,
    ll: usize,
    ml: usize,
}

/// A sequence near the end of the literals, or whose offset reaches before
/// the frame or is invalid; returns the new output length. The caller
/// checked that `span` ends within the block's limit.
#[cold]
#[inline(never)]
fn execute_exact(
    mut dst: Dst<'_>,
    span: Span,
    lits: &[u8],
    lp: usize,
    offset: usize,
    history: &[u8],
) -> Result<usize, DecompressError> {
    let Span { op, ll, ml } = span;
    if lp > lits.len() || ll > lits.len() - lp {
        return Err(DecompressError::CorruptLiterals);
    }
    dst.put(op, &lits[lp..lp + ll]);
    let lit_end = op + ll;
    if offset == 0 {
        return Err(DecompressError::InvalidOffset);
    }
    let mut done = 0;
    if offset > lit_end {
        // The match starts in the history before the frame and continues
        // from the frame's first byte.
        let before = offset - lit_end;
        if before > history.len() {
            return Err(DecompressError::InvalidOffset);
        }
        let start = history.len() - before;
        done = before.min(ml);
        dst.put(lit_end, &history[start..start + done]);
    }
    if ml > done {
        let at = lit_end + done;
        dst.wild_match(at, at - offset, offset, ml - done);
    }
    Ok(lit_end + ml)
}

/// A block's output as the copies see it ([`SeqOut`]'s slice). Valid for
/// writes up to the block's limit plus [`WILDCOPY_OVERLENGTH`]; every byte
/// below the output position is initialized.
struct Dst<'a>(&'a mut [u8]);

/// `i` masked to 32 bits under `NARROW`.
///
/// Every position is then below 4 GiB anyway, so the mask changes nothing,
/// but it shows the compiler that sums of positions and lengths cannot
/// overflow. Range checks then take one compare instead of two, and checks
/// implied by earlier ones go away.
#[inline(always)]
const fn ix<const NARROW: bool>(i: usize) -> usize {
    if NARROW { i & u32::MAX as usize } else { i }
}

impl Dst<'_> {
    /// Writes `bytes` at `at`; `at + bytes.len()` is within the limit.
    #[inline(always)]
    fn put(&mut self, at: usize, bytes: &[u8]) {
        self.0[at..at + bytes.len()].copy_from_slice(bytes);
    }

    /// Copies `len` literals from `lits[lp..]` to `at` in whole 32-byte
    /// chunks. Requires `lp + len + LITERAL_OVERREAD <= lits.len()` and
    /// `at + len` within the limit.
    #[inline(always)]
    fn wild_literals(&mut self, at: usize, lits: &[u8], lp: usize, len: usize) {
        debug_assert!(lp + len + LITERAL_OVERREAD <= lits.len());
        self.0[at..at + 32].copy_from_slice(&lits[lp..lp + 32]);
        let mut i = 32;
        while i < len {
            let (d, s) = (at + i, lp + i);
            self.0[d..d + 32].copy_from_slice(&lits[s..s + 32]);
            i += 32;
        }
    }

    /// Copies a match of `len` bytes from `src`, `offset` bytes before `at`,
    /// in whole chunks. Requires `1 <= offset <= at` and `at + len` within
    /// the limit.
    #[inline(always)]
    fn wild_match(&mut self, at: usize, src: usize, offset: usize, len: usize) {
        debug_assert!(offset >= 1 && offset <= at && src == at - offset);
        let out = &mut *self.0;
        if offset >= 32 {
            chunk::<32>(out, src, at);
            let mut i = 32;
            while i < len {
                chunk::<32>(out, src + i, at + i);
                i += 32;
            }
        } else if offset >= 16 {
            chunk::<16>(out, src, at);
            let mut i = 16;
            while i < len {
                chunk::<16>(out, src + i, at + i);
                i += 16;
            }
        } else {
            // A chunk reads all its bytes before writing, so its source must
            // be at least its size back. Offsets below 8 spread the first 8
            // bytes, after which the source sits a multiple of `offset` back,
            // and at least 8.
            let mut src = src;
            if offset < 8 {
                for k in 0..4 {
                    out[at + k] = out[src + k];
                }
                src += SPREAD_ADD[offset];
                chunk::<4>(out, src, at + 4);
                src = src + 8 - SPREAD_SUB[offset];
            } else {
                chunk::<8>(out, src, at);
                src += 8;
            }
            let mut i = 8;
            while i < len {
                chunk::<8>(out, src, at + i);
                src += 8;
                i += 8;
            }
        }
    }
}

/// Copies `N` bytes within `out`, reading them all before writing.
#[inline(always)]
fn chunk<const N: usize>(out: &mut [u8], src: usize, dst: usize) {
    let mut value = [0u8; N];
    value.copy_from_slice(&out[src..src + N]);
    out[dst..dst + N].copy_from_slice(&value);
}

/// Appends `len` bytes copied from `offset` bytes before the end of `vec`.
/// Requires `1 <= offset <= vec.len()`.
#[inline(always)]
fn wild_copy_match(vec: &mut Vec<u8>, offset: usize, len: usize) {
    let start = vec.len() - offset;
    if offset >= len {
        vec.extend_from_within(start..start + len);
    } else if offset == 1 {
        vec.resize(vec.len() + len, vec[start]);
    } else {
        // Overlap: copy one period, then keep doubling the copied run. It
        // stays a multiple of `offset`, so the pattern repeats.
        vec.extend_from_within(start..start + offset);
        let mut copied = offset;
        while copied < len {
            let n = (len - copied).min(copied);
            let src = vec.len() - copied;
            vec.extend_from_within(src..src + n);
            copied += n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_lengths(max_len: usize) -> Vec<usize> {
        (1..=max_len).collect()
    }

    #[test]
    fn open_block_at_zeroes_only_new_room() {
        let mut v = vec![7u8; 100];
        let (base, op, end) = open_block_at(&mut v, 40, 80, 10);
        assert_eq!((base, op, end), (10, 30, 120 + WILDCOPY_OVERLENGTH));
        assert_eq!(v.len(), end);
        assert!(v[..100].iter().all(|&b| b == 7));
        assert!(v[100..].iter().all(|&b| b == 0));

        // Room that already reaches past the block is kept as it is.
        v.fill(9);
        let (base, op, end) = open_block_at(&mut v, 4, 16, 8);
        assert_eq!((base, op, end), (4, 0, 20 + WILDCOPY_OVERLENGTH));
        assert_eq!(v.len(), 120 + WILDCOPY_OVERLENGTH);
        assert!(v.iter().all(|&b| b == 9));
    }

    fn expected_match(seed: &[u8], offset: usize, len: usize) -> Vec<u8> {
        let mut expected = seed.to_vec();
        let start = expected.len() - offset;
        for i in 0..len {
            expected.push(expected[start + i % offset]);
        }
        expected
    }

    /// Runs `f` on the output of a block of at most `max_block` bytes appended
    /// to `vec`, then cuts `vec` to the output, as `decode_execute_sequences`
    /// does.
    fn with_seq_out<const NARROW: bool, R>(
        vec: &mut Vec<u8>,
        max_block: usize,
        frame_start: usize,
        f: impl FnOnce(&mut SeqOut<'_, NARROW>) -> R,
    ) -> R {
        let (base, op) = open_block(vec, max_block, frame_start);
        let mut out = SeqOut::<NARROW>::new(&mut vec[base..], op);
        let result = f(&mut out);
        let end = base + out.op();
        vec.truncate(end);
        result
    }

    /// Runs one sequence through `SeqOut::execute` after `prefix`, with
    /// `slack` readable bytes after the literals, and returns the output.
    fn seq_out_run(
        prefix: &[u8],
        lits: &[u8],
        slack: usize,
        ml: usize,
        offset: usize,
        history: &[u8],
    ) -> Result<Vec<u8>, DecompressError> {
        let narrow = seq_out_run_as::<true>(prefix, lits, slack, ml, offset, history);
        let wide = seq_out_run_as::<false>(prefix, lits, slack, ml, offset, history);
        assert_eq!(narrow, wide);
        narrow
    }

    fn seq_out_run_as<const NARROW: bool>(
        prefix: &[u8],
        lits: &[u8],
        slack: usize,
        ml: usize,
        offset: usize,
        history: &[u8],
    ) -> Result<Vec<u8>, DecompressError> {
        let mut buf = lits.to_vec();
        buf.resize(lits.len() + slack, 0xAA);
        let mut v = prefix.to_vec();
        let result = with_seq_out::<NARROW, _>(&mut v, lits.len() + ml, 0, |out| {
            out.execute(
                Literals::new(&buf, lits.len()),
                0,
                lits.len(),
                ml,
                offset,
                history,
            )
        });
        result.map(|()| v)
    }

    fn check_wild_copy_match_offsets(start_offset: usize, end_offset: usize) {
        for offset in start_offset..=end_offset {
            let seed: Vec<u8> = (0..offset).map(|i| (i as u8).wrapping_mul(37)).collect();
            for len in test_lengths(128) {
                let expected = expected_match(&seed, offset, len);
                // With slack the fast path copies whole chunks; without it
                // the exact path runs.
                for slack in [LITERAL_OVERREAD, 0] {
                    let v = seq_out_run(&seed, &[], slack, len, offset, &[]).unwrap();
                    assert_eq!(v, expected, "offset={offset} len={len} slack={slack}");
                }
            }
        }
    }

    fn check_wild_copy_match_single_offsets(start_offset: usize, end_offset: usize) {
        for offset in start_offset..=end_offset {
            for len in test_lengths(256) {
                let mut v = Vec::new();
                let seed: Vec<u8> = (0..offset).map(|i| (i as u8).wrapping_mul(37)).collect();
                v.extend_from_slice(&seed);
                let expected = expected_match(&v, offset, len);
                let mut output = BlockOutput::new(&mut v, len);
                let mut sequence = output.begin_sequence(0, len).unwrap();
                sequence.copy_match_single(offset).unwrap();
                assert_eq!(
                    &v[..offset + len],
                    &expected[..offset + len],
                    "offset={offset} len={len}"
                );
            }
        }
    }

    #[test]
    fn fast_extend_from_slice_all_sizes() {
        // `tail` bytes of source follow the copied run: with 16 or more the
        // copy reads a fixed 16 bytes, with fewer it copies exactly.
        for len in 0..=64 {
            for tail in [0, 1, 15, 16, 17, 40] {
                for start in [0, 3] {
                    let src: Vec<u8> = (0..(start + len + tail) as u8).collect();
                    let mut dst = vec![0xEEu8; 5];
                    {
                        let mut output = BlockOutput::new(&mut dst, len);
                        output.extend_literals_range(&src, start, len).unwrap();
                    }
                    assert_eq!(dst[..5], [0xEE; 5]);
                    assert_eq!(dst[5..], src[start..start + len], "len={len} tail={tail}");
                }
            }
        }
    }

    #[test]
    fn block_output_extends_literal_range() {
        let mut dst = Vec::new();
        {
            let mut output = BlockOutput::new(&mut dst, 16);
            output
                .extend_literals_range(b"abcdefgh", 2, 4)
                .expect("literal range should fit");
        }
        assert_eq!(dst, b"cdef");
    }

    #[test]
    fn block_output_rejects_literal_range_oob() {
        let mut dst = Vec::new();
        let mut output = BlockOutput::new(&mut dst, 16);
        let err = output
            .extend_literals_range(b"abc", 2, 4)
            .expect_err("literal range should be out of bounds");
        assert_eq!(err, DecompressError::CorruptLiterals);
    }

    #[test]
    fn block_output_rejects_block_overflow() {
        let mut dst = Vec::new();
        let mut output = BlockOutput::new(&mut dst, 4);
        let err = output
            .extend_literals_range(b"abcde", 0, 5)
            .expect_err("literal range should exceed the block limit");
        assert_eq!(err, DecompressError::BlockTooLarge);
    }

    #[test]
    fn seq_out_copies_literals_then_matches() {
        let prefix: Vec<u8> = (0..40u8).map(|i| i.wrapping_mul(29) ^ 0x5A).collect();
        let lits_all: Vec<u8> = (0..100u8).map(|i| i ^ 0xC3).collect();
        for ll in 0..=70 {
            for ml in [3, 17, 40, 100] {
                for offset in [1, 2, 3, 7, 8, 15, 16, 31, 32, 40 + ll] {
                    let lits = &lits_all[..ll];
                    let mut expected = prefix.clone();
                    expected.extend_from_slice(lits);
                    let expected = expected_match(&expected, offset, ml);
                    for slack in [LITERAL_OVERREAD, 5, 0] {
                        let v = seq_out_run(&prefix, lits, slack, ml, offset, &[]).unwrap();
                        assert_eq!(v, expected, "ll={ll} ml={ml} offset={offset} slack={slack}");
                    }
                }
            }
        }
    }

    #[test]
    fn seq_out_rejects_bad_offsets() {
        for slack in [LITERAL_OVERREAD, 0] {
            for offset in [0, 5, 100] {
                let err = seq_out_run(b"abcd", &[], slack, 4, offset, &[]).unwrap_err();
                assert_eq!(err, DecompressError::InvalidOffset, "offset={offset}");
            }
        }
        // A match may not reach before the frame's first byte.
        let mut v = b"abcdef".to_vec();
        let lits = [0u8; LITERAL_OVERREAD];
        with_seq_out::<true, _>(&mut v, 8, 4, |out| {
            assert_eq!(
                out.execute(Literals::new(&lits, 0), 0, 0, 4, 3, &[]),
                Err(DecompressError::InvalidOffset)
            );
            assert_eq!(
                out.execute(Literals::new(&lits, 0), 0, 0, 4, 2, &[]),
                Ok(())
            );
        });
        assert_eq!(v, b"abcdefefef");
    }

    #[test]
    fn seq_out_rejects_overruns() {
        let lits = [7u8; 10];
        // More literals than the section holds.
        let mut v = b"ab".to_vec();
        with_seq_out::<true, _>(&mut v, 32, 0, |out| {
            assert_eq!(
                out.execute(Literals::new(&lits, 10), 4, 8, 3, 1, &[]),
                Err(DecompressError::CorruptLiterals)
            );
        });
        // A sequence past the block's limit.
        let mut v = b"ab".to_vec();
        with_seq_out::<true, _>(&mut v, 8, 0, |out| {
            assert_eq!(
                out.execute(Literals::new(&lits, 10), 0, 4, 5, 1, &[]),
                Err(DecompressError::BlockTooLarge)
            );
            assert_eq!(
                out.extend_literals(Literals::new(&lits, 10), 0, 9),
                Err(DecompressError::BlockTooLarge)
            );
            assert_eq!(
                out.extend_literals(Literals::new(&lits, 10), 4, 7),
                Err(DecompressError::CorruptLiterals)
            );
            assert_eq!(out.extend_literals(Literals::new(&lits, 10), 4, 6), Ok(()));
        });
        assert_eq!(v, b"ab\x07\x07\x07\x07\x07\x07");
    }

    #[test]
    fn seq_out_copies_matches_from_history() {
        let history = b"abcdefghijklmnop";
        let prefix = b"qrst";
        let lits = b"uv";
        for offset in [7, 8, 20, 22] {
            for ml in [1, 2, 10, 30] {
                let mut combined = history.to_vec();
                combined.extend_from_slice(prefix);
                combined.extend_from_slice(lits);
                let expected = expected_match(&combined, offset, ml);
                for slack in [LITERAL_OVERREAD, 0] {
                    let v = seq_out_run(prefix, lits, slack, ml, offset, history).unwrap();
                    assert_eq!(v, expected[history.len()..], "offset={offset} ml={ml}");
                }
            }
        }
        let err = seq_out_run(prefix, lits, 0, 3, 23, history).unwrap_err();
        assert_eq!(err, DecompressError::InvalidOffset);
    }

    #[test]
    fn block_output_copies_match_from_history() {
        let history = b"abcdefghijklmnop";
        let mut dst = b"qrst".to_vec();
        let out_pos = dst.len();
        let offset = 8;
        let match_len = 20;

        let mut expected = dst.clone();
        for i in 0..match_len {
            let src = history.len() + out_pos - offset + i;
            let byte = if src < history.len() {
                history[src]
            } else {
                expected[src - history.len()]
            };
            expected.push(byte);
        }

        {
            let mut output = BlockOutput::new(&mut dst, 32);
            let mut sequence = output
                .begin_sequence(0, match_len)
                .expect("sequence should fit block");
            sequence
                .copy_match_from_history(history, offset, out_pos)
                .expect("history match should fit");
        }

        assert_eq!(dst, expected);
    }

    #[test]
    fn wild_copy_match_offsets_1_8() {
        check_wild_copy_match_offsets(1, 8);
    }

    #[test]
    fn wild_copy_match_offsets_9_16() {
        check_wild_copy_match_offsets(9, 16);
    }

    #[test]
    fn wild_copy_match_offsets_17_32() {
        check_wild_copy_match_offsets(17, 32);
    }

    #[test]
    fn wild_copy_match_offsets_33_64() {
        check_wild_copy_match_offsets(33, 64);
    }

    #[test]
    fn wild_copy_match_single_offsets_1_8() {
        check_wild_copy_match_single_offsets(1, 8);
    }

    #[test]
    fn wild_copy_match_single_offsets_9_16() {
        check_wild_copy_match_single_offsets(9, 16);
    }

    #[test]
    fn wild_copy_match_single_offsets_17_32() {
        check_wild_copy_match_single_offsets(17, 32);
    }

    #[test]
    fn wild_copy_match_single_offsets_33_64() {
        check_wild_copy_match_single_offsets(33, 64);
    }

    #[test]
    fn wild_copy_match_single_offsets_65_96() {
        check_wild_copy_match_single_offsets(65, 96);
    }

    #[test]
    fn wild_copy_match_single_offsets_97_128() {
        check_wild_copy_match_single_offsets(97, 128);
    }
}
