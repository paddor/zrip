//! The 4-stream Huffman decoder: interleaved fast loops with C zstd's
//! container layout, then a checked tail per stream.

use crate::bitstream::primitives as bitstream_primitives;
use crate::bitstream::reader_reverse::ReverseBitReader;
use crate::error::DecompressError;
use crate::huffman::primitives as huffman_primitives;
use crate::huffman::{
    DECODE_TABLE_SIZE, FAST_LOOP_ROWS, FAST_TABLE_LOG, HuffmanDecodeEntry, MAX_TABLE_LOG,
    PAIR_TABLE_LOG, PAIR_TABLE_SIZE,
};

/// Decodes the 4-stream section `data` into `output[..output_size]`, one
/// `output_size.div_ceil(4)`-byte segment per stream, the last shorter.
/// Fast loops run while every stream has room; `decode_stream_tail`
/// finishes each.
#[inline(always)]
pub(super) fn decode_4_streams_core(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    pairs: Option<&[u32; PAIR_TABLE_SIZE]>,
    rows: &[u16],
    data: &[u8],
    output_size: usize,
    output: &mut [u8],
) -> Result<(), DecompressError> {
    if data.len() < 6 {
        return Err(DecompressError::BadHuffmanStream);
    }

    // Jump table: the sizes of streams 1 to 3, u16 little-endian. Stream 4
    // takes the rest.
    let s1_size = u16::from_le_bytes([data[0], data[1]]) as usize;
    let s2_size = u16::from_le_bytes([data[2], data[3]]) as usize;
    let s3_size = u16::from_le_bytes([data[4], data[5]]) as usize;

    let jump_table_size = 6;
    let s1_start = jump_table_size;
    let s2_start = s1_start + s1_size;
    let s3_start = s2_start + s2_size;
    let s4_start = s3_start + s3_size;

    if s4_start > data.len() {
        return Err(DecompressError::BadHuffmanStream);
    }

    // As C zstd: at least six symbols, so three full segments fit. At six
    // and nine the fourth segment is empty, and its stream holds only the
    // end mark.
    let seg = output_size.div_ceil(4);
    if output_size < 6 {
        return Err(DecompressError::BadHuffmanStream);
    }
    let remaining = output_size - seg * 3;
    let output = &mut output[..output_size];

    let mut r1 = ReverseBitReader::new(&data[s1_start..s2_start])
        .map_err(|_| DecompressError::BadHuffmanStream)?;
    let mut r2 = ReverseBitReader::new(&data[s2_start..s3_start])
        .map_err(|_| DecompressError::BadHuffmanStream)?;
    let mut r3 = ReverseBitReader::new(&data[s3_start..s4_start])
        .map_err(|_| DecompressError::BadHuffmanStream)?;
    let mut r4 =
        ReverseBitReader::new(&data[s4_start..]).map_err(|_| DecompressError::BadHuffmanStream)?;

    let tl = table_log as u32;
    if !(1..=u32::from(MAX_TABLE_LOG)).contains(&tl) {
        return Err(DecompressError::BadHuffmanStream);
    }
    // The fast rounds take five codes per refill, up to 11 bits each. Deeper
    // tables, which C zstd accepts and its encoder does not write, decode
    // every stream with the checked tail.
    if tl > u32::from(FAST_TABLE_LOG) {
        let (out1, rest) = output.split_at_mut(seg);
        let (out2, rest) = rest.split_at_mut(seg);
        let (out3, out4) = rest.split_at_mut(seg);
        super::decode::decode_stream_tail(table, table_log, &mut r1, out1)?;
        super::decode::decode_stream_tail(table, table_log, &mut r2, out2)?;
        super::decode::decode_stream_tail(table, table_log, &mut r3, out3)?;
        return super::decode::decode_stream_tail(table, table_log, &mut r4, out4);
    }
    // `shift >= 53` keeps every fast-round index below the 2048-entry table.
    // The bounds checks stay: they are off the lookup chain, where a mask
    // would add a cycle per symbol.
    let shift = 64 - tl;

    // The table for the fast rounds, at its fixed size, and its rows on
    // targets whose fast loop reads them ([`FAST_LOOP_ROWS`]).
    let fixed: &[HuffmanDecodeEntry; DECODE_TABLE_SIZE] = table
        .first_chunk()
        .ok_or(DecompressError::BadHuffmanStream)?;
    let rows: &[u16; DECODE_TABLE_SIZE] = if FAST_LOOP_ROWS {
        rows.first_chunk()
            .ok_or(DecompressError::BadHuffmanStream)?
    } else {
        &[0; DECODE_TABLE_SIZE]
    };

    // Output decoded so far in each segment.
    let mut done = [0usize; 4];

    // The fast rounds use C zstd's layout and bounds. Each stream is a
    // left-aligned container `b` with a sentinel bit: a load sets bit 0, and
    // each symbol shifts the container left by its length, so the next code
    // is always the top `tl` bits and `trailing_zeros` is the number of bits
    // consumed since the load. A round reads at most 62 bits after a load, so
    // the sentinel never reaches the top 11 bits.
    //
    // Positions `a` are absolute in `data`, and a reload may read bytes of
    // the stream before, as C zstd's loop does: a stream's container is then
    // partly outside it, but its unread bits are not, unless the data is
    // corrupt. A round moves a stream back at most 7 bytes. So rounds stay
    // inside `data` while stream 1, the lowest, is 7 bytes per round from
    // the start, and every stream is at or above the one before it.
    // `rebase` checks afterwards that no stream read bits below its start.
    if r1.data.len() >= 8 && r2.data.len() >= 8 && r3.data.len() >= 8 && r4.data.len() >= 8 {
        // Every stream has at least 8 bytes, so each container is a full
        // load with at most 8 bits consumed.
        let mut a1 = s1_start + r1.ptr;
        let mut a2 = s2_start + r2.ptr;
        let mut a3 = s3_start + r3.ptr;
        let mut a4 = s4_start + r4.ptr;
        let mut b1 = (r1.container | 1) << r1.bits_consumed;
        let mut b2 = (r2.container | 1) << r2.bits_consumed;
        let mut b3 = (r3.container | 1) << r3.bits_consumed;
        let mut b4 = (r4.container | 1) << r4.bits_consumed;

        macro_rules! reload {
            ($b:expr, $a:expr) => {{
                let consumed = $b.trailing_zeros();
                $a -= (consumed >> 3) as usize;
                $b = (bitstream_primitives::read_u64_le_unaligned(data, $a) | 1) << (consumed & 7);
            }};
        }

        // Rounds every stream can run: by its output room, and by stream 1's
        // distance from the start of `data`. Zero once the streams are out of
        // order, which only corrupt data does.
        let rounds_for = |done: &[usize; 4], room: usize, a: [usize; 4]| -> usize {
            if !(a[0] <= a[1] && a[1] <= a[2] && a[2] <= a[3]) {
                return 0;
            }
            let ends = [seg, seg, seg, remaining];
            let mut by_output = usize::MAX;
            for k in 0..4 {
                by_output = by_output.min(ends[k].saturating_sub(done[k]) / room);
            }
            by_output.min(a[0] / 7)
        };

        // Two-symbol rounds: four lookups per stream (at most 44 bits after a
        // load that leaves at most seven consumed), each storing both bytes of
        // its row and advancing by its count. A round may touch the
        // `PAIR_WINDOW` bytes after the stream's output; they stay inside its
        // segment, where the next round or the end overwrites them.
        const PAIR_SHIFT: u32 = 64 - PAIR_TABLE_LOG as u32;
        const PAIR_WINDOW: usize = 10;
        if let Some(pairs) = pairs {
            // One stream's round: reload, then four lookups, each storing
            // both bytes of its row at the stream's offset in its window and
            // advancing by the row's count. Stream by stream, so one window
            // and offset are live at a time: interleaved, the four streams
            // decode at half the speed in native builds.
            macro_rules! pair_round {
                ($b:expr, $a:expr, $out:expr, $d:expr) => {{
                    reload!($b, $a);
                    let w: &mut [u8; PAIR_WINDOW] =
                        (&mut $out[$d..$d + PAIR_WINDOW]).try_into().unwrap();
                    let mut o = 0usize;
                    for _ in 0..4 {
                        let row = pairs[($b >> PAIR_SHIFT) as usize];
                        $b <<= (row >> 16) & 0xFF;
                        let at = o & 7;
                        w[at..at + 2].copy_from_slice(&(row as u16).to_le_bytes());
                        o += (row >> 24) as usize;
                    }
                    $d += o;
                }};
            }
            loop {
                let rounds = rounds_for(&done, PAIR_WINDOW, [a1, a2, a3, a4]);
                if rounds == 0 {
                    break;
                }
                let (out1, rest) = output.split_at_mut(seg);
                let (out2, rest) = rest.split_at_mut(seg);
                let (out3, out4) = rest.split_at_mut(seg);
                let [mut d1, mut d2, mut d3, mut d4] = done;
                for _ in 0..rounds {
                    pair_round!(b1, a1, out1, d1);
                    pair_round!(b2, a2, out2, d2);
                    pair_round!(b3, a3, out3, d3);
                    pair_round!(b4, a4, out4, d4);
                }
                done = [d1, d2, d3, d4];
            }
        }

        // Five-symbol rounds. All four streams advance together, so each
        // takes the same number of fixed-size chunks of its segment.
        loop {
            let rounds = rounds_for(&done, 5, [a1, a2, a3, a4]);
            if rounds == 0 {
                break;
            }
            let (out1, rest) = output.split_at_mut(seg);
            let (out2, rest) = rest.split_at_mut(seg);
            let (out3, out4) = rest.split_at_mut(seg);
            let n = rounds * 5;
            let w1 = out1[done[0]..done[0] + n].as_chunks_mut::<5>().0;
            let w2 = out2[done[1]..done[1] + n].as_chunks_mut::<5>().0;
            let w3 = out3[done[2]..done[2] + n].as_chunks_mut::<5>().0;
            let w4 = out4[done[3]..done[3] + n].as_chunks_mut::<5>().0;
            for (((c1, c2), c3), c4) in w1
                .iter_mut()
                .zip(w2.iter_mut())
                .zip(w3.iter_mut())
                .zip(w4.iter_mut())
            {
                reload!(b1, a1);
                reload!(b2, a2);
                reload!(b3, a3);
                reload!(b4, a4);
                // Symbol by symbol across the streams, so the four lookup
                // chains interleave and each symbol is stored at once.
                // A row ([`super::decode::build_row_table`]) is one load:
                // the container shifts by the whole row, whose low byte is
                // the code length, and the high byte is the symbol.
                macro_rules! symbol {
                    ($b:expr, $c:expr, $i:expr) => {{
                        if FAST_LOOP_ROWS {
                            let r = rows[($b >> shift) as usize];
                            $c[$i] = (r >> 8) as u8;
                            $b <<= r & 63;
                        } else {
                            let e = fixed[($b >> shift) as usize];
                            $c[$i] = e.symbol;
                            $b <<= e.num_bits;
                        }
                    }};
                }
                for i in 0..5 {
                    symbol!(b1, c1, i);
                    symbol!(b2, c2, i);
                    symbol!(b3, c3, i);
                    symbol!(b4, c4, i);
                }
            }
            for d in &mut done {
                *d += n;
            }
        }

        rebase(&mut r1, b1, a1, s1_start)?;
        rebase(&mut r2, b2, a2, s2_start)?;
        rebase(&mut r3, b3, a3, s3_start)?;
        rebase(&mut r4, b4, a4, s4_start)?;
    }

    let (out1, rest) = output.split_at_mut(seg);
    let (out2, rest) = rest.split_at_mut(seg);
    let (out3, out4) = rest.split_at_mut(seg);

    /// Whether a refill can load eight bytes inside the stream.
    #[inline(always)]
    fn can_refill_fast(bits_consumed: u32, ptr: usize, data_len: usize) -> bool {
        let byte_shift = (bits_consumed >> 3) as usize;
        ptr.checked_sub(byte_shift)
            .and_then(|new_ptr| new_ptr.checked_add(8))
            .is_some_and(|end| end <= data_len)
    }

    // Each stream on its own with checked lookups, five symbols per refill,
    // until it is too close to its start or end; the tail does the rest.
    #[inline(always)]
    fn finish_fast(
        table: &[HuffmanDecodeEntry],
        tl: u32,
        r: &mut ReverseBitReader,
        output: &mut [u8],
        mut o_idx: usize,
    ) -> usize {
        let (mut c, mut bc, mut p_idx) = (r.container, r.bits_consumed, r.ptr);
        while o_idx + 5 <= output.len() && p_idx >= r.limit_ptr {
            if !can_refill_fast(bc, p_idx, r.data.len()) {
                break;
            }
            let byte_shift = (bc >> 3) as usize;
            p_idx -= byte_shift;
            bc -= (byte_shift as u32) * 8;
            c = bitstream_primitives::read_u64_le_unaligned(r.data, p_idx);
            for _ in 0..5 {
                let idx = ((c << (bc & 63)) >> (64 - tl)) as usize;
                let e = huffman_primitives::huf_table_lookup(table, idx);
                debug_assert!(e.num_bits > 0, "Huffman table entry with 0 bits");
                huffman_primitives::huf_output_write(output, o_idx, e.symbol);
                bc += e.num_bits as u32;
                o_idx += 1;
            }
        }
        r.container = c;
        r.bits_consumed = bc;
        r.ptr = p_idx;
        o_idx
    }

    // One loop, so the inlined tail code exists once.
    let readers = [&mut r1, &mut r2, &mut r3, &mut r4];
    let outs = [out1, out2, out3, out4];
    for ((r, out), d) in readers.into_iter().zip(outs).zip(done) {
        let o = finish_fast(table, tl, r, out, d);
        super::decode::decode_stream_tail(table, table_log, r, &mut out[o..])?;
    }

    Ok(())
}

/// Puts a stream's fast-round state back into its reader: the container
/// `b` as loaded at absolute position `a` in the section, for the stream
/// that starts at `start`. A container that reaches below the stream's
/// start moves up to it; the bits below the start must still be unread.
#[inline(always)]
fn rebase(r: &mut ReverseBitReader, b: u64, a: usize, start: usize) -> Result<(), DecompressError> {
    let consumed = b.trailing_zeros();
    if a >= start {
        r.ptr = a - start;
        r.bits_consumed = consumed;
    } else {
        let below = (start - a) * 8;
        if consumed as usize + below > 64 {
            return Err(DecompressError::BadHuffmanStream);
        }
        r.ptr = 0;
        r.bits_consumed = consumed + below as u32;
    }
    // `a + 8` stays within the stream: positions only move down from its
    // last eight bytes.
    r.container = bitstream_primitives::read_u64_le_unaligned(r.data, r.ptr);
    Ok(())
}
