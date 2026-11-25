//! Huffman stream decoding: single-stream, and 4-stream with an optional
//! two-symbol table.

#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use super::primitives;
use crate::bitstream::reader_reverse::ReverseBitReader;
use crate::error::DecompressError;
use crate::huffman::{
    DECODE_TABLE_SIZE, FAST_LOOP_ROWS, FAST_TABLE_LOG, HuffmanDecodeEntry, MAX_TABLE_LOG,
    PAIR_TABLE_LOG, PAIR_TABLE_SIZE,
};

/// Decodes `output_size` symbols from one reverse stream into a new
/// `Vec`. The stream must end exactly after the last symbol.
pub fn decode_single_stream(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
) -> Result<Vec<u8>, DecompressError> {
    validate_decode_table(table, table_log)?;
    let mut reader = ReverseBitReader::new(data).map_err(|_| DecompressError::BadHuffmanStream)?;

    let mut output = Vec::with_capacity(output_size);

    let tl = table_log as usize;
    while output.len() + 4 <= output_size && reader.bits_remaining() >= tl + tl + tl + tl {
        for _ in 0..4 {
            let bits = reader.peek_bits(table_log);
            let entry = primitives::huf_table_lookup(table, bits as usize);
            output.push(entry.symbol);
            reader.consume_bits(entry.num_bits);
        }
    }

    while output.len() < output_size {
        reader.refill();
        let remaining = reader.bits_remaining();
        if remaining == 0 {
            return Err(DecompressError::BadHuffmanStream);
        }
        // Near the stream start, missing low bits read as zeros.
        let bits = if remaining >= tl {
            reader.peek_bits(table_log)
        } else {
            reader.peek_bits(remaining as u8) << (tl - remaining)
        };
        let entry = primitives::huf_table_lookup(table, bits as usize);
        if entry.num_bits as usize > remaining {
            return Err(DecompressError::BadHuffmanStream);
        }
        output.push(entry.symbol);
        reader.bits_consumed += entry.num_bits as u32;
    }

    if reader.bits_remaining() != 0 {
        return Err(DecompressError::BadHuffmanStream);
    }

    Ok(output)
}

/// Decodes one reverse stream into all of `output`. The stream must end
/// exactly after the last symbol.
#[inline(always)]
pub fn decode_single_stream_into(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output: &mut [u8],
) -> Result<(), DecompressError> {
    validate_decode_table(table, table_log)?;
    let mut reader = ReverseBitReader::new(data).map_err(|_| DecompressError::BadHuffmanStream)?;
    decode_stream_tail(table, table_log, &mut reader, output)
}

/// [`decode_single_stream_into`] with `output` resized to `output_size`.
/// Clears `output` on error.
#[inline(always)]
pub fn decode_single_stream_vec(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
    output: &mut Vec<u8>,
) -> Result<(), DecompressError> {
    validate_decode_table(table, table_log)?;
    prepare_output(output, output_size);
    let result = decode_single_stream_into(table, table_log, data, output);
    if result.is_err() {
        output.clear();
    }
    result
}

/// Decodes a 4-stream section, a 6-byte jump table and then the streams,
/// into a new `Vec` of `output_size` bytes.
pub fn decode_4_streams(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
) -> Result<Vec<u8>, DecompressError> {
    validate_decode_table(table, table_log)?;
    let mut output = vec![0u8; output_size];
    let rows = row_table_for(table, table_log);
    decode_4_streams_core_safe(
        table,
        table_log,
        None,
        rows_of(rows.as_ref()),
        data,
        output_size,
        &mut output,
    )?;
    Ok(output)
}

/// [`decode_4_streams`] with `output` resized to `output_size`. Clears
/// `output` on error.
#[inline(always)]
pub fn decode_4_streams_into(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
    output: &mut Vec<u8>,
) -> Result<(), DecompressError> {
    let rows = row_table_for(table, table_log);
    decode_4_streams_with(
        table,
        table_log,
        None,
        rows_of(rows.as_ref()),
        data,
        output_size,
        output,
    )
}

/// [`decode_4_streams_into`] with the rows from [`build_row_table`] for the
/// same tree, which callers that decode many sections keep with the table.
/// Where the fast loop reads entries ([`FAST_LOOP_ROWS`] is false), `rows`
/// is not read and may be empty.
#[inline(always)]
pub fn decode_4_streams_rows_into(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    rows: &[u16],
    data: &[u8],
    output_size: usize,
    output: &mut Vec<u8>,
) -> Result<(), DecompressError> {
    decode_4_streams_with(table, table_log, None, rows, data, output_size, output)
}

/// [`decode_4_streams_into`] with a two-symbol table from
/// [`build_pair_table`] for the same tree: the fast loop decodes up to two
/// symbols per lookup. Streams still end on the single-symbol table.
// Out of line in builds that do not dispatch: pairs take sections of at
// least `PAIR_MIN_LITERALS`, where a call costs nothing, and their fast loop
// runs 7% slower inlined into the literals decoder.
#[cfg_attr(
    all(target_arch = "x86_64", not(target_feature = "avx2")),
    inline(always)
)]
#[cfg_attr(
    not(all(target_arch = "x86_64", not(target_feature = "avx2"))),
    inline(never)
)]
pub fn decode_4_streams_pairs_into(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    pairs: &[u32],
    data: &[u8],
    output_size: usize,
    output: &mut Vec<u8>,
) -> Result<(), DecompressError> {
    let pairs: &[u32; PAIR_TABLE_SIZE] = pairs
        .first_chunk()
        .ok_or(DecompressError::BadHuffmanStream)?;
    let rows = row_table_for(table, table_log);
    decode_4_streams_with(
        table,
        table_log,
        Some(pairs),
        rows_of(rows.as_ref()),
        data,
        output_size,
        output,
    )
}

/// [`decode_4_streams_pairs_into`] with the rows from [`build_row_table`],
/// as [`decode_4_streams_rows_into`].
// Out of line in builds that do not dispatch: pairs take sections of at
// least `PAIR_MIN_LITERALS`, where a call costs nothing, and their fast loop
// runs 7% slower inlined into the literals decoder.
#[cfg_attr(
    all(target_arch = "x86_64", not(target_feature = "avx2")),
    inline(always)
)]
#[cfg_attr(
    not(all(target_arch = "x86_64", not(target_feature = "avx2"))),
    inline(never)
)]
pub fn decode_4_streams_pairs_rows_into(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    pairs: &[u32],
    rows: &[u16],
    data: &[u8],
    output_size: usize,
    output: &mut Vec<u8>,
) -> Result<(), DecompressError> {
    let pairs: &[u32; PAIR_TABLE_SIZE] = pairs
        .first_chunk()
        .ok_or(DecompressError::BadHuffmanStream)?;
    decode_4_streams_with(
        table,
        table_log,
        Some(pairs),
        rows,
        data,
        output_size,
        output,
    )
}

/// The reference decoder's timing model (`algoTime`): per compression-ratio
/// bucket, the table build and per-256-symbol decode times of the
/// single-symbol and the two-symbol decoder.
const ALGO_TIME: [[(u32, u32); 2]; 16] = [
    [(0, 0), (1, 1)],
    [(0, 0), (1, 1)],
    [(150, 216), (381, 119)],
    [(170, 205), (514, 112)],
    [(177, 199), (539, 110)],
    [(197, 194), (644, 107)],
    [(221, 192), (735, 107)],
    [(256, 189), (881, 106)],
    [(359, 188), (1167, 109)],
    [(582, 187), (1570, 114)],
    [(688, 187), (1712, 122)],
    [(825, 186), (1965, 136)],
    [(976, 185), (2131, 150)],
    [(1180, 186), (2070, 175)],
    [(1377, 185), (1731, 202)],
    [(1412, 185), (1695, 202)],
];

/// Fewest literals the two-symbol decoder takes. Its 2048-entry table and
/// the single-symbol stream ends cost more than pairs save on shorter
/// sections: on Silesia text slices, pairs decode 10% slower at 14 KiB of
/// literals, 2.5% slower at 20 KiB, and 2% faster at 26 KiB.
const PAIR_MIN_LITERALS: usize = 24 * 1024;

/// Whether the two-symbol decoder is expected to be faster for `literals`
/// symbols in `compressed` bytes.
///
/// C zstd's timing model (`HUF_selectDecoder`) decides, but only for sections
/// of at most five bits per symbol: with longer codes few pairs fit in an
/// 11-bit lookup, and the single-symbol loop is faster (measured on the
/// Silesia corpus at level 1, where text gains up to 6% and high-entropy
/// literals lose up to 20% without this limit), and only for sections of at
/// least `PAIR_MIN_LITERALS`.
pub fn prefers_pairs(literals: usize, compressed: usize) -> bool {
    if literals < PAIR_MIN_LITERALS || compressed * 8 > literals * 5 {
        return false;
    }
    let bucket = if compressed >= literals {
        15
    } else {
        compressed * 16 / literals
    };
    let blocks = (literals >> 8) as u32;
    let [single, pair] = ALGO_TIME[bucket];
    let single = single.0 + single.1 * blocks;
    let pair = pair.0 + pair.1 * blocks;
    pair + (pair >> 5) < single
}

/// Builds the single-symbol rows of the 4-stream fast loop from `table`, as
/// C zstd's `HUF_DEltX1`: the code length in the low byte, so the loop
/// shifts its container by the whole row, and the symbol in the high byte.
/// A lookup is then one 16-bit load with its index scaled in the address,
/// not two byte loads: 13-18% faster on high-entropy literals on Apple M5.
/// For a table built from weights,
/// [`build_huffman_decode_tables_into`](super::weights::build_huffman_decode_tables_into)
/// fills the rows without a second pass.
pub fn build_row_table(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    rows: &mut Vec<u16>,
) -> Result<(), DecompressError> {
    validate_decode_table(table, table_log)?;
    rows.resize(DECODE_TABLE_SIZE, 0);
    let n = 1usize << table_log;
    for (row, entry) in rows[..n].iter_mut().zip(&table[..n]) {
        *row = row_of(*entry);
    }
    Ok(())
}

/// The fast-loop row of `entry` ([`build_row_table`]).
#[inline(always)]
const fn row_of(entry: HuffmanDecodeEntry) -> u16 {
    super::decode_row(entry.symbol, entry.num_bits)
}

/// The rows of `table` for a caller without cached ones, on targets whose
/// fast loop reads them ([`FAST_LOOP_ROWS`]): built only for tables the
/// fast loop reads, at most [`FAST_TABLE_LOG`] bits.
fn row_table_for(table: &[HuffmanDecodeEntry], table_log: u8) -> Option<[u16; DECODE_TABLE_SIZE]> {
    if !FAST_LOOP_ROWS {
        return None;
    }
    let mut rows = [0u16; DECODE_TABLE_SIZE];
    if table_log <= FAST_TABLE_LOG {
        let n = (1usize << table_log).min(table.len());
        for (row, entry) in rows[..n].iter_mut().zip(&table[..n]) {
            *row = row_of(*entry);
        }
    }
    Some(rows)
}

/// The rows from [`row_table_for`], empty where the fast loop reads
/// entries.
#[inline(always)]
fn rows_of(rows: Option<&[u16; DECODE_TABLE_SIZE]>) -> &[u16] {
    rows.map_or(&[], |rows| rows)
}

/// Packs a two-symbol row: the symbols in the low 16 bits, first one in the
/// low byte, the bits they take in the next byte, and their count in the top
/// byte.
#[inline(always)]
const fn pair_row(symbols: u32, nb_bits: u32, count: u32) -> u32 {
    symbols | (nb_bits << 16) | (count << 24)
}

/// Builds the two-symbol table of an 11-bit lookup (C zstd's `HUF_DEltX2`)
/// from a single-symbol table: each row decodes the code at the top of its
/// index and, when both fit in 11 bits, the code after it. Decoding with it
/// reads exactly the bits the single-symbol table reads for those symbols.
pub fn build_pair_table(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    pairs: &mut Vec<u32>,
) -> Result<(), DecompressError> {
    validate_decode_table(table, table_log)?;
    if table_log > PAIR_TABLE_LOG {
        return Err(DecompressError::BadHuffmanStream);
    }
    let table: &[HuffmanDecodeEntry; DECODE_TABLE_SIZE] = table
        .first_chunk()
        .ok_or(DecompressError::BadHuffmanStream)?;
    pairs.resize(PAIR_TABLE_SIZE, 0);
    let down = u32::from(PAIR_TABLE_LOG - table_log);
    let mask = PAIR_TABLE_SIZE - 1;
    for (index, row) in pairs[..PAIR_TABLE_SIZE].iter_mut().enumerate() {
        let first = table[(index >> down) & (DECODE_TABLE_SIZE - 1)];
        let first_bits = u32::from(first.num_bits) & 15;
        let second = table[((index << first_bits) & mask) >> down];
        let second_bits = u32::from(second.num_bits);
        *row = if first_bits + second_bits <= u32::from(PAIR_TABLE_LOG) {
            pair_row(
                u32::from(first.symbol) | (u32::from(second.symbol) << 8),
                first_bits + second_bits,
                2,
            )
        } else {
            pair_row(u32::from(first.symbol), first_bits, 1)
        };
    }
    Ok(())
}

#[inline(always)]
fn decode_4_streams_with(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    pairs: Option<&[u32; PAIR_TABLE_SIZE]>,
    rows: &[u16],
    data: &[u8],
    output_size: usize,
    output: &mut Vec<u8>,
) -> Result<(), DecompressError> {
    validate_decode_table(table, table_log)?;
    prepare_output(output, output_size);
    let result =
        decode_4_streams_core_safe(table, table_log, pairs, rows, data, output_size, output);
    if result.is_err() {
        output.clear();
    }
    result
}

/// Rejects table logs outside 1 to 12 and tables shorter than the log.
fn validate_decode_table(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
) -> Result<(), DecompressError> {
    if table_log == 0 || table_log > MAX_TABLE_LOG {
        return Err(DecompressError::BadHuffmanStream);
    }
    if table.len() < (1usize << table_log) {
        return Err(DecompressError::BadHuffmanStream);
    }
    Ok(())
}

#[inline(always)]
fn decode_4_streams_core_safe(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    pairs: Option<&[u32; PAIR_TABLE_SIZE]>,
    rows: &[u16],
    data: &[u8],
    output_size: usize,
    output: &mut [u8],
) -> Result<(), DecompressError> {
    super::decode_4stream::decode_4_streams_core(
        table,
        table_log,
        pairs,
        rows,
        data,
        output_size,
        output,
    )
}

#[inline(always)]
fn prepare_output(output: &mut Vec<u8>, output_size: usize) {
    output.resize(output_size, 0);
}

/// Decodes the rest of `reader`'s stream into `output` and checks that
/// the stream ends exactly there.
#[inline(always)]
pub(super) fn decode_stream_tail(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    reader: &mut ReverseBitReader,
    output: &mut [u8],
) -> Result<(), DecompressError> {
    // A refill leaves at least 57 bits: five codes of up to 11 bits, or four
    // of 12.
    if table_log <= FAST_TABLE_LOG {
        decode_stream_tail_n::<5>(table, table_log, reader, output)
    } else {
        decode_stream_tail_n::<4>(table, table_log, reader, output)
    }
}

/// [`decode_stream_tail`], decoding `N` symbols per refill.
#[inline(always)]
fn decode_stream_tail_n<const N: usize>(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    reader: &mut ReverseBitReader,
    output: &mut [u8],
) -> Result<(), DecompressError> {
    let output_size = output.len();
    let tl = table_log as usize;
    let mut pos = 0;

    let round_bits = (N * tl) as u32;
    while pos + N <= output_size {
        reader.refill();
        if reader.bits_remaining() < round_bits as usize
            || 64u32.saturating_sub(reader.bits_consumed) < round_bits
        {
            break;
        }

        for _ in 0..N {
            let bits =
                ((reader.container << reader.bits_consumed) >> (64 - table_log as u32)) as usize;
            let entry = primitives::huf_table_lookup(table, bits);
            primitives::huf_output_write(output, pos, entry.symbol);
            pos += 1;
            reader.bits_consumed += entry.num_bits as u32;
        }
    }

    // Either fewer than `N` symbols are left and the refill leaves at least
    // 57 bits in the container, or fewer than `N * table_log` bits remain and
    // the refill reaches the start of the stream (`ptr == 0`), so the
    // container holds every remaining bit. Past the start, the shift reads
    // zeros. A symbol longer than the bits left pushes the count past 64,
    // which the checks below reject.
    reader.refill();
    let container = reader.container;
    let mut consumed = reader.bits_consumed;
    while pos < output_size {
        if consumed >= 64 {
            return Err(DecompressError::BadHuffmanStream);
        }
        let bits = ((container << consumed) >> (64 - table_log as u32)) as usize;
        let entry = primitives::huf_table_lookup(table, bits);
        primitives::huf_output_write(output, pos, entry.symbol);
        pos += 1;
        consumed += entry.num_bits as u32;
    }
    reader.bits_consumed = consumed;

    if reader.ptr != 0 || consumed != 64 {
        return Err(DecompressError::BadHuffmanStream);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitstream::writer::BitWriter;

    #[test]
    fn huffman_decode_rejects_zero_table_log_before_fast_tail_lookup() {
        // Regression: a one-entry table satisfies `table.len() >= 1 <<
        // table_log` when `table_log == 0`, but zero is not a valid Huffman
        // table log. Reject it before `decode_stream_tail` can compute an
        // out-of-range table index from the fast path.
        let table = [HuffmanDecodeEntry {
            symbol: 0,
            num_bits: 1,
        }];
        let data = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x80];
        let mut output = [0u8; 5];

        assert_eq!(
            decode_single_stream_into(&table, 0, &data, &mut output),
            Err(DecompressError::BadHuffmanStream)
        );
    }

    #[test]
    fn huffman_4stream_rejects_zero_table_log_before_tail_lookup() {
        // Regression: the 4-stream decoder reaches the same tail helper after
        // splitting the output. Reject a zero table log at the public boundary
        // so it cannot reach `huf_table_lookup`'s table read.
        let table = [HuffmanDecodeEntry {
            symbol: 0,
            num_bits: 1,
        }];
        let stream = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x80];
        let mut data = Vec::new();
        data.extend_from_slice(&8u16.to_le_bytes());
        data.extend_from_slice(&8u16.to_le_bytes());
        data.extend_from_slice(&8u16.to_le_bytes());
        data.extend_from_slice(&stream);
        data.extend_from_slice(&stream);
        data.extend_from_slice(&stream);
        data.extend_from_slice(&stream);
        let mut output = Vec::new();

        assert_eq!(
            decode_4_streams_into(&table, 0, &data, 24, &mut output),
            Err(DecompressError::BadHuffmanStream)
        );
    }

    fn xorshift(seed: u64) -> impl FnMut() -> u64 {
        let mut seed = seed;
        move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        }
    }

    /// Decode table, table log, used symbols, and `(code, length)` per symbol.
    type RandomCode = (Vec<HuffmanDecodeEntry>, u8, Vec<u8>, Vec<(u32, u8)>);

    /// A random complete prefix code over up to 60 of the 256 symbols, at
    /// most 11 bits deep.
    fn random_code(rand: &mut impl FnMut() -> u64) -> RandomCode {
        use crate::huffman::weights::build_huffman_decode_table;

        // Split random leaves.
        let leaves = 2 + (rand() as usize) % 60;
        let mut depths = vec![1u8, 1];
        while depths.len() < leaves {
            let i = (rand() as usize) % depths.len();
            if depths[i] < 11 {
                depths[i] += 1;
                depths.push(depths[i]);
            }
        }
        let table_log = *depths.iter().max().unwrap();
        // Leaf `k` is symbol `symbols[k]`. The last leaf takes the highest
        // symbol, whose weight is implied.
        let last_symbol = leaves - 1 + (rand() as usize) % (256 - leaves + 1);
        let mut symbols: Vec<usize> = (0..last_symbol).collect();
        for i in (1..symbols.len()).rev() {
            symbols.swap(i, (rand() as usize) % (i + 1));
        }
        symbols.truncate(leaves - 1);
        symbols.push(last_symbol);
        let mut weights = vec![0u8; last_symbol];
        for (&symbol, &depth) in symbols[..leaves - 1].iter().zip(&depths) {
            weights[symbol] = table_log + 1 - depth;
        }
        let (table, log) = build_huffman_decode_table(&weights).unwrap();
        assert_eq!(log, table_log);

        let mut codes = vec![(0u32, 0u8); 256];
        for (i, e) in table[..1 << table_log].iter().enumerate() {
            codes[e.symbol as usize] = ((i >> (table_log - e.num_bits)) as u32, e.num_bits);
        }
        let symbols = symbols.iter().map(|&s| s as u8).collect();
        (table, log, symbols, codes)
    }

    /// A reverse Huffman stream that decodes to `message`.
    fn encode_stream(message: &[u8], codes: &[(u32, u8)]) -> Vec<u8> {
        let mut writer = BitWriter::new();
        for &symbol in message.iter().rev() {
            let (code, bits) = codes[symbol as usize];
            writer.write_bits(code, bits);
        }
        writer.close_reverse_stream();
        writer.into_bytes()
    }

    #[test]
    fn stream_tail_matches_reference() {
        let mut rand = xorshift(0x9e6c_63d0_676a_9a99);
        let (mut ok, mut err) = (0, 0);
        for _ in 0..2000 {
            let (table, log, symbols, codes) = random_code(&mut rand);
            let message: Vec<u8> = (0..=(rand() as usize) % 300)
                .map(|_| symbols[(rand() as usize) % symbols.len()])
                .collect();
            let stream = encode_stream(&message, &codes);

            let mut variants = vec![stream.clone()];
            let mut flipped = stream.clone();
            let bit = (rand() as usize) % (flipped.len() * 8);
            flipped[bit / 8] ^= 1 << (bit % 8);
            variants.push(flipped);
            variants.push(stream[..(rand() as usize) % stream.len()].to_vec());
            let mut longer = stream.clone();
            longer.push(1 + rand() as u8 % 255);
            variants.push(longer);

            for (v, data) in variants.iter().enumerate() {
                let expected = decode_single_stream(&table, log, data, message.len());
                let mut out = vec![0u8; message.len()];
                let result = decode_single_stream_into(&table, log, data, &mut out);
                match (expected, result) {
                    (Ok(expected), Ok(())) => {
                        assert_eq!(out, expected);
                        if v == 0 {
                            assert_eq!(out, message);
                        }
                        ok += 1;
                    }
                    (Err(e), Err(result_e)) => {
                        assert_eq!(result_e, e);
                        err += 1;
                    }
                    (expected, result) => panic!("{expected:?} vs {result:?}"),
                }
            }
        }
        assert!(ok > 2000 && err > 2000, "ok {ok}, err {err}");
    }

    #[test]
    fn four_streams_match_single_stream_decode() {
        let mut rand = xorshift(0x2f69_3a8e_1bd5_c047);
        let (mut ok, mut err) = (0, 0);
        for _ in 0..1500 {
            let (table, log, symbols, codes) = random_code(&mut rand);
            // Long enough messages that the fast rounds run.
            let size = 16 + (rand() as usize) % 2000;
            let message: Vec<u8> = (0..size)
                .map(|_| symbols[(rand() as usize) % symbols.len()])
                .collect();
            let seg = size.div_ceil(4);
            let parts = [
                &message[..seg],
                &message[seg..2 * seg],
                &message[2 * seg..3 * seg],
                &message[3 * seg..],
            ];
            let streams: Vec<Vec<u8>> = parts.iter().map(|m| encode_stream(m, &codes)).collect();

            let mut variants = Vec::new();
            for corrupt in 0..3 {
                let mut streams = streams.clone();
                let k = (rand() as usize) % 4;
                match corrupt {
                    1 => {
                        let bit = (rand() as usize) % (streams[k].len() * 8);
                        streams[k][bit / 8] ^= 1 << (bit % 8);
                    }
                    2 => {
                        let len = streams[k].len();
                        streams[k].truncate(1 + (rand() as usize) % len);
                        if streams[k].last() == Some(&0) {
                            streams[k].pop();
                            streams[k].push(1);
                        }
                    }
                    _ => {}
                }
                if streams[..3].iter().any(|s| s.len() > usize::from(u16::MAX)) {
                    continue;
                }
                let mut data = Vec::new();
                for stream in &streams[..3] {
                    data.extend_from_slice(&(stream.len() as u16).to_le_bytes());
                }
                for stream in &streams {
                    data.extend_from_slice(stream);
                }
                variants.push((corrupt, data, streams));
            }

            for (corrupt, data, streams) in variants {
                // Reference: each stream on its own.
                let expected: Result<Vec<u8>, DecompressError> = streams
                    .iter()
                    .zip(parts)
                    .map(|(s, part)| decode_single_stream(&table, log, s, part.len()))
                    .collect::<Result<Vec<_>, _>>()
                    .map(|p| p.concat());
                let mut out = Vec::new();
                let result = decode_4_streams_into(&table, log, &data, size, &mut out);
                // The two-symbol table decodes the same symbols and rejects
                // the same streams.
                let mut pairs = Vec::new();
                build_pair_table(&table, log, &mut pairs).unwrap();
                let mut out_pairs = Vec::new();
                let result_pairs =
                    decode_4_streams_pairs_into(&table, log, &pairs, &data, size, &mut out_pairs);
                assert_eq!(result.is_ok(), result_pairs.is_ok(), "corrupt {corrupt}");
                if result.is_ok() {
                    assert_eq!(out_pairs, out, "corrupt {corrupt}");
                }
                match (expected, result) {
                    (Ok(expected), Ok(())) => {
                        assert_eq!(out, expected);
                        if corrupt == 0 {
                            assert_eq!(out, message);
                        }
                        ok += 1;
                    }
                    (Err(_), Err(_)) => err += 1,
                    (expected, result) => panic!("{expected:?} vs {result:?} (corrupt {corrupt})"),
                }
            }
        }
        assert!(ok > 1500 && err > 500, "ok {ok}, err {err}");
    }

    #[test]
    fn the_timing_model_prefers_pairs_for_well_compressed_large_sections() {
        assert!(prefers_pairs(100_000, 30_000));
        assert!(!prefers_pairs(100, 99));
        assert!(!prefers_pairs(1_000, 2_000));
        // Well compressed, but too short to repay the pair table.
        assert!(!prefers_pairs(8_000, 2_400));
    }

    #[test]
    fn decode_single_stream_rejects_short_huffman_table() {
        let table = [HuffmanDecodeEntry {
            symbol: 0,
            num_bits: 1,
        }];
        let mut data = BitWriter::new();
        data.write_bits(1, 1);
        data.close_reverse_stream();

        assert_eq!(
            decode_single_stream(&table, 1, &data.into_bytes(), 1),
            Err(DecompressError::BadHuffmanStream)
        );
    }
}
