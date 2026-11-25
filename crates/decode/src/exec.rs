//! Sequence execution: decodes each sequence from the FSE bitstream and
//! copies its literals and match in the same pass.

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::fast_vec::{BlockOutput, Literals, SeqOut, open_block, open_block_at};
use crate::sequences::{SequenceDecodeTables, compute_offset};
use zrip_core::bitstream::reader_reverse::ReverseBitReader;
use zrip_core::error::DecompressError;
use zrip_core::fse::FSE_SEQ_TABLE_MASK;
use zrip_core::hint::{likely, unlikely};

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
use aarch64::run_sequences;

/// Where a block's sequences may write and copy from.
#[derive(Clone, Copy)]
pub(crate) struct SequenceOutputScope<'a> {
    /// Index in the output of the lowest byte a match may copy from: the
    /// frame's first byte, or the window's start when streaming.
    pub(crate) output_base: usize,
    /// Most bytes the block may write.
    pub(crate) max_block_output: usize,
    /// Dictionary content before `output_base`, for matches that reach
    /// back beyond it. Empty when streaming, which keeps it in the output.
    pub(crate) history: &'a [u8],
}

/// Most extra bits a sequence reads before its literal length without a
/// second word, given the state bits that follow: C zstd's
/// `64 - 7 - (LLFSELog + MLFSELog + OffFSELog)`.
const MID_RELOAD_THRESHOLD: u32 = 64 - 7 - (9 + 9 + 8);

/// Fewest unread bits a [`Word`] holds while the stream has that many.
const WORD_BITS: usize = 57;

/// Backward reader for the sequences bitstream, C zstd's `BIT_DStream_t`
/// reduced to the count of unread bits. Each sequence loads the word that
/// holds its bits ([`SeqBits::word`]), so the loop carries one number
/// instead of a container, its fill, and a position. Reads past the
/// stream's start make the count wrap to a huge value, which never comes
/// back to zero, so [`SeqBits::is_exhausted`] rejects the stream.
#[derive(Clone, Copy)]
struct SeqBits {
    left: usize,
}

/// The stream bytes `pos..pos + 8` as a little-endian word, its low `avail`
/// bits unread.
#[derive(Clone, Copy)]
struct Word {
    container: u64,
    avail: usize,
    pos: usize,
}

impl SeqBits {
    /// Opens `src`, or `None` when it is empty or ends without a marker bit.
    #[inline]
    fn new(src: &[u8]) -> Option<Self> {
        let &last = src.last()?;
        if last == 0 {
            return None;
        }
        Some(Self {
            left: src.len() * 8 - (last.leading_zeros() as usize + 1),
        })
    }

    /// The word holding the next [`WORD_BITS`] unread bits, or all of them
    /// near the start. Requires `src.len() >= 8`.
    #[inline(always)]
    fn word(self, src: &[u8]) -> Word {
        // No `unlikely` here or on the repeat offsets: its cold path is a
        // call, and the sequence loop is faster without one.
        let pos = if self.left < WORD_BITS {
            0
        } else {
            (self.left - WORD_BITS) >> 3
        };
        // A valid count keeps `pos` below the clamp; a wrapped one reads
        // any word.
        let pos = pos.min(src.len() - 8);
        let word = src[pos..pos + 8].try_into().unwrap_or([0; 8]);
        Word {
            container: u64::from_le_bytes(word),
            avail: self.left.wrapping_sub(pos * 8),
            pos,
        }
    }

    /// Whether every bit was read, and no more.
    #[inline]
    fn is_exhausted(self) -> bool {
        self.left == 0
    }
}

impl Word {
    /// Reads `count <= 31` bits.
    #[inline(always)]
    fn read(&mut self, count: u32) -> u32 {
        let start = self.avail.wrapping_sub(count as usize);
        self.avail = start;
        // `& 63` keeps a wrapped `start` from overflowing the shift; the
        // final `is_exhausted` check rejects that stream.
        ((self.container >> (start & 63)) & ((1u64 << count) - 1)) as u32
    }

    /// The reader after this word's reads.
    #[inline(always)]
    fn bits(&self) -> SeqBits {
        SeqBits {
            left: (self.pos * 8).wrapping_add(self.avail),
        }
    }
}

/// Decodes `num_sequences` sequences from `data`, executes them into
/// `output`, then appends the block's last literals. Updates `offsets` on
/// success. With `HAS_HISTORY`, matches may reach into `scope.history`.
#[inline(always)]
pub(crate) fn decode_execute_sequences<const HAS_HISTORY: bool>(
    data: &[u8],
    num_sequences: u32,
    tables: &SequenceDecodeTables,
    offsets: &mut [u32; 3],
    literals: Literals<'_>,
    output: &mut Vec<u8>,
    scope: SequenceOutputScope<'_>,
) -> Result<(), DecompressError> {
    if num_sequences == 0 {
        return Ok(());
    }
    let bits = SeqBits::new(data).ok_or(DecompressError::CorruptSequences)?;
    let state_bits = u32::from(tables.ll_accuracy)
        + u32::from(tables.of_accuracy)
        + u32::from(tables.ml_accuracy);
    if bits.left < state_bits as usize {
        return Err(DecompressError::InputExhausted);
    }
    // A short stream is read from a zero-padded copy.
    let mut padded = [0u8; 8];
    let data = if data.len() < 8 {
        padded[..data.len()].copy_from_slice(data);
        &padded[..]
    } else {
        data
    };
    let history = if HAS_HISTORY { scope.history } else { &[] };
    let mut vec = core::mem::take(output);
    let (base, op) = open_block(&mut vec, scope.max_block_output, scope.output_base);
    // No closure here: the loop must inline into the `fearless_simd` body.
    // Output below 4 GiB, the usual case, gets 32-bit positions (`NARROW`).
    let (result, op) = if u32::try_from(vec.len()).is_ok() {
        let mut out = SeqOut::<true>::new(&mut vec[base..], op);
        let result = run_sequences(
            bits,
            data,
            num_sequences,
            tables,
            offsets,
            literals,
            &mut out,
            history,
        );
        (result, out.op())
    } else {
        let mut out = SeqOut::<false>::new(&mut vec[base..], op);
        let result = run_sequences(
            bits,
            data,
            num_sequences,
            tables,
            offsets,
            literals,
            &mut out,
            history,
        );
        (result, out.op())
    };
    // Drop the zeroed room past the output, also after an error.
    vec.truncate(base + op);
    *output = vec;
    result
}

/// [`decode_execute_sequences`] into a streaming history: the block starts
/// at `*len` in `output`, before the room earlier blocks left
/// ([`open_block_at`]). The room past the block's output stays for the next
/// block, and `*len` is set to where the output ends, also after an error.
///
/// A copy, not a flag on [`decode_execute_sequences`], so the one-shot
/// path's code stays as it was: the SIMD kernels' code shifts with small
/// changes around them (see `decode_sequences_dispatch`).
#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_execute_sequences_room<const HAS_HISTORY: bool>(
    data: &[u8],
    num_sequences: u32,
    tables: &SequenceDecodeTables,
    offsets: &mut [u32; 3],
    literals: Literals<'_>,
    output: &mut Vec<u8>,
    len: &mut usize,
    scope: SequenceOutputScope<'_>,
) -> Result<(), DecompressError> {
    if num_sequences == 0 {
        return Ok(());
    }
    let bits = SeqBits::new(data).ok_or(DecompressError::CorruptSequences)?;
    let state_bits = u32::from(tables.ll_accuracy)
        + u32::from(tables.of_accuracy)
        + u32::from(tables.ml_accuracy);
    if bits.left < state_bits as usize {
        return Err(DecompressError::InputExhausted);
    }
    // A short stream is read from a zero-padded copy.
    let mut padded = [0u8; 8];
    let data = if data.len() < 8 {
        padded[..data.len()].copy_from_slice(data);
        &padded[..]
    } else {
        data
    };
    let history = if HAS_HISTORY { scope.history } else { &[] };
    let mut vec = core::mem::take(output);
    let (base, op, end) = open_block_at(&mut vec, *len, scope.max_block_output, scope.output_base);
    // As in `decode_execute_sequences`.
    let (result, op) = if u32::try_from(end).is_ok() {
        let mut out = SeqOut::<true>::new(&mut vec[base..end], op);
        let result = run_sequences(
            bits,
            data,
            num_sequences,
            tables,
            offsets,
            literals,
            &mut out,
            history,
        );
        (result, out.op())
    } else {
        let mut out = SeqOut::<false>::new(&mut vec[base..end], op);
        let result = run_sequences(
            bits,
            data,
            num_sequences,
            tables,
            offsets,
            literals,
            &mut out,
            history,
        );
        (result, out.op())
    };
    *len = base + op;
    *output = vec;
    result
}

/// The decoding half of the sequence loop: the three FSE states and the
/// repeat offsets.
#[cfg_attr(target_arch = "aarch64", allow(dead_code))]
struct SeqDecoder {
    ll_state: u32,
    of_state: u32,
    ml_state: u32,
    rep: [u32; 3],
}

#[cfg_attr(target_arch = "aarch64", allow(dead_code))]
impl SeqDecoder {
    /// Decodes one sequence as `(literal length, match length, offset)`,
    /// C zstd's `ZSTD_decodeSequence`. With `UPDATE` the states advance;
    /// the block's last sequence keeps them.
    #[inline(always)]
    fn decode<const UPDATE: bool>(
        &mut self,
        b: &mut SeqBits,
        data: &[u8],
        tables: &SequenceDecodeTables,
    ) -> (usize, usize, usize) {
        let mut w = b.word(data);
        macro_rules! table_entry {
            ($table:expr, $state:expr) => {{
                let idx = ($state & FSE_SEQ_TABLE_MASK) as usize;
                *$table.get(idx)
            }};
        }
        let of_e = table_entry!(tables.of_table, self.of_state);
        let ml_e = table_entry!(tables.ml_table, self.ml_state);
        let ll_e = table_entry!(tables.ll_table, self.ll_state);
        let of_bits = of_e.extra_bits.get();
        let offset_value = of_e.baseline_value + w.read(of_bits);
        let ml_bits = ml_e.extra_bits.get();
        let match_length = ml_e.baseline_value + w.read(ml_bits);
        let ll_bits = ll_e.extra_bits.get();
        if ll_bits + ml_bits + of_bits > MID_RELOAD_THRESHOLD {
            w = w.bits().word(data);
        }
        let literal_length = ll_e.baseline_value + w.read(ll_bits);

        let [rep0, rep1, rep2] = self.rep;
        // Offset values above 3 carry `offset + 3`; 1 to 3 pick a repeat
        // offset. Without literals the pick shifts by one, and index 3 means
        // `rep0 - 1`, which is 0 for `rep0 == 1` and fails on execution.
        let offset = if offset_value > 3 {
            let offset = offset_value - 3;
            self.rep = [offset, rep0, rep1];
            offset
        } else {
            let ll0 = u32::from(literal_length == 0);
            let rep_idx = offset_value - 1 + ll0;
            let offset = match rep_idx {
                0 => rep0,
                1 => rep1,
                2 => rep2,
                _ => rep0.wrapping_sub(1),
            };
            self.rep = [
                offset,
                if rep_idx >= 1 { rep0 } else { rep1 },
                if rep_idx >= 2 { rep1 } else { rep2 },
            ];
            offset
        };

        if UPDATE {
            // The format's order: literal length, match length, offset.
            self.ll_state = u32::from(ll_e.base_line) + w.read(ll_e.num_bits.get());
            self.ml_state = u32::from(ml_e.base_line) + w.read(ml_e.num_bits.get());
            self.of_state = u32::from(of_e.base_line) + w.read(of_e.num_bits.get());
        }
        *b = w.bits();
        (
            literal_length as usize,
            match_length as usize,
            offset as usize,
        )
    }
}

/// The sequence loop of [`decode_execute_sequences`], C zstd's
/// `ZSTD_decompressSequences_body`: one word per sequence, a second one
/// only when its extra bits could exhaust the first, and the last
/// sequence peeled off so the loop never tests for it. AArch64 has its own
/// ([`aarch64`]).
#[cfg(not(target_arch = "aarch64"))]
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn run_sequences<const NARROW: bool>(
    bits: SeqBits,
    data: &[u8],
    num_sequences: u32,
    tables: &SequenceDecodeTables,
    offsets: &mut [u32; 3],
    literals: Literals<'_>,
    out: &mut SeqOut<'_, NARROW>,
    history: &[u8],
) -> Result<(), DecompressError> {
    if data.len() < 8 {
        // The caller pads short streams; this tells the compiler.
        return Err(DecompressError::CorruptSequences);
    }
    let mut w = bits.word(data);
    let mut dec = SeqDecoder {
        ll_state: w.read(u32::from(tables.ll_accuracy)),
        of_state: w.read(u32::from(tables.of_accuracy)),
        ml_state: w.read(u32::from(tables.ml_accuracy)),
        rep: *offsets,
    };
    let mut b = w.bits();

    let mut lp: usize = 0;
    let mut left = num_sequences - 1;
    while left != 0 {
        let (ll, ml, offset) = dec.decode::<true>(&mut b, data, tables);
        out.execute(literals, lp, ll, ml, offset, history)?;
        lp += ll;
        left -= 1;
    }
    let (ll, ml, offset) = dec.decode::<false>(&mut b, data, tables);
    out.execute(literals, lp, ll, ml, offset, history)?;
    lp += ll;

    if !b.is_exhausted() {
        return Err(DecompressError::CorruptSequences);
    }
    if lp < literals.len() {
        out.extend_literals(literals, lp, literals.len() - lp)?;
    }
    *offsets = dec.rep;
    Ok(())
}

/// [`decode_execute_sequences`] for a block with exactly one sequence:
/// a plain bit reader and an appending [`BlockOutput`], without the loop's
/// zeroed block room. The last sequence needs no state update.
#[inline(always)]
pub(crate) fn decode_execute_single_sequence<const HAS_HISTORY: bool>(
    data: &[u8],
    tables: &SequenceDecodeTables,
    offsets: &mut [u32; 3],
    literals: &[u8],
    output: &mut Vec<u8>,
    scope: SequenceOutputScope<'_>,
) -> Result<(), DecompressError> {
    if data.is_empty() {
        return Err(DecompressError::CorruptSequences);
    }

    let mut rev_reader =
        ReverseBitReader::new(data).map_err(|_| DecompressError::CorruptSequences)?;
    let ll_state = rev_reader.read_bits(tables.ll_accuracy)?;
    let of_state = rev_reader.read_bits(tables.of_accuracy)?;
    let ml_state = rev_reader.read_bits(tables.ml_accuracy)?;

    let mut output = BlockOutput::new(output, scope.max_block_output);
    let output_base = scope.output_base;
    let history = scope.history;

    macro_rules! table_entry {
        ($table:expr, $state:expr) => {{
            let idx = ($state & FSE_SEQ_TABLE_MASK) as usize;
            $table.get(idx)
        }};
    }

    rev_reader.refill();
    let of_e = table_entry!(tables.of_table, of_state);
    let ml_e = table_entry!(tables.ml_table, ml_state);
    let ll_e = table_entry!(tables.ll_table, ll_state);

    let of_extra = rev_reader.read_bits_branchless(of_e.extra_bits.get() as u8);
    let offset_value = of_e.baseline_value + of_extra;
    let ml_extra = rev_reader.read_bits_branchless(ml_e.extra_bits.get() as u8);
    let match_length = ml_e.baseline_value + ml_extra;
    let ll_extra = rev_reader.read_bits_branchless(ll_e.extra_bits.get() as u8);
    let literal_length = ll_e.baseline_value + ll_extra;
    let offset = compute_offset(offset_value, literal_length, offsets);

    let ll = literal_length as usize;
    let ml = match_length as usize;
    {
        let mut seq_output = output.begin_sequence(ll, ml)?;
        seq_output.extend_literals_range(literals, 0)?;

        let off = offset as usize;
        let out_pos = seq_output.len() - output_base;
        if HAS_HISTORY {
            if likely(off <= out_pos) {
                seq_output.copy_match_single(off)?;
            } else {
                seq_output.copy_match_from_history(history, off, out_pos)?;
            }
        } else {
            if unlikely(off == 0 || off > out_pos) {
                return Err(DecompressError::InvalidOffset);
            }
            seq_output.copy_match_single(off)?;
        }
    }

    if !rev_reader.is_exhausted() {
        return Err(DecompressError::CorruptSequences);
    }

    if ll < literals.len() {
        let remaining = literals.len() - ll;
        output.extend_literals_range(literals, ll, remaining)?;
    }

    Ok(())
}
