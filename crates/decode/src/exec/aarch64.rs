//! The sequence loop for AArch64: each sequence executes after the next one
//! is decoded.
//!
//! [`super::run_sequences`] decodes a sequence, then executes it. On
//! AArch64 its compiled loop keeps a dozen values in stack slots, among
//! them the bit position, so every sequence waits on store-to-load
//! forwarding. Here the decoder holds the table rows of its states, loaded
//! as soon as the states are read, and each sequence's copies overlap the
//! next sequence's bit reads: 15-30% faster on Apple M5. On x86-64, with
//! half the registers, the same loop spills more and runs 5-10% slower, so
//! it stays AArch64's.

use super::{MID_RELOAD_THRESHOLD, SeqBits};
use crate::fast_vec::{Literals, SeqOut};
use crate::seq_table::{SeqEntry, SeqTable};
use crate::sequences::SequenceDecodeTables;
use zrip_core::error::DecompressError;
use zrip_core::fse::FSE_SEQ_TABLE_MASK;

/// The row of `table` for `state`.
#[inline(always)]
fn row(table: &SeqTable, state: u32) -> SeqEntry {
    *table.get((state & FSE_SEQ_TABLE_MASK) as usize)
}

/// [`super::SeqDecoder`] holding the rows of the three FSE states rather
/// than the states.
struct RowDecoder {
    ll: SeqEntry,
    of: SeqEntry,
    ml: SeqEntry,
    rep: [u32; 3],
}

impl RowDecoder {
    /// [`super::SeqDecoder::decode`].
    #[inline(always)]
    fn decode<const UPDATE: bool>(
        &mut self,
        b: &mut SeqBits,
        data: &[u8],
        tables: &SequenceDecodeTables,
    ) -> (usize, usize, usize) {
        let mut w = b.word(data);
        let (of_e, ml_e, ll_e) = (self.of, self.ml, self.ll);
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
            let ll_state = u32::from(ll_e.base_line) + w.read(ll_e.num_bits.get());
            let ml_state = u32::from(ml_e.base_line) + w.read(ml_e.num_bits.get());
            let of_state = u32::from(of_e.base_line) + w.read(of_e.num_bits.get());
            self.ll = row(&tables.ll_table, ll_state);
            self.ml = row(&tables.ml_table, ml_state);
            self.of = row(&tables.of_table, of_state);
        }
        *b = w.bits();
        (
            literal_length as usize,
            match_length as usize,
            offset as usize,
        )
    }
}

/// [`super::run_sequences`], executing each sequence after decoding the
/// next.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
pub(super) fn run_sequences<const NARROW: bool>(
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
    let ll_state = w.read(u32::from(tables.ll_accuracy));
    let of_state = w.read(u32::from(tables.of_accuracy));
    let ml_state = w.read(u32::from(tables.ml_accuracy));
    let mut dec = RowDecoder {
        ll: row(&tables.ll_table, ll_state),
        of: row(&tables.of_table, of_state),
        ml: row(&tables.ml_table, ml_state),
        rep: *offsets,
    };
    let mut b = w.bits();

    let mut lp: usize = 0;
    let mut left = num_sequences - 1;
    if left != 0 {
        let mut cur = dec.decode::<true>(&mut b, data, tables);
        left -= 1;
        while left != 0 {
            let next = dec.decode::<true>(&mut b, data, tables);
            let (ll, ml, offset) = cur;
            out.execute(literals, lp, ll, ml, offset, history)?;
            lp += ll;
            cur = next;
            left -= 1;
        }
        let last = dec.decode::<false>(&mut b, data, tables);
        let (ll, ml, offset) = cur;
        out.execute(literals, lp, ll, ml, offset, history)?;
        lp += ll;
        let (ll, ml, offset) = last;
        out.execute(literals, lp, ll, ml, offset, history)?;
        lp += ll;
    } else {
        let (ll, ml, offset) = dec.decode::<false>(&mut b, data, tables);
        out.execute(literals, lp, ll, ml, offset, history)?;
        lp += ll;
    }

    if !b.is_exhausted() {
        return Err(DecompressError::CorruptSequences);
    }
    if lp < literals.len() {
        out.extend_literals(literals, lp, literals.len() - lp)?;
    }
    *offsets = dec.rep;
    Ok(())
}
