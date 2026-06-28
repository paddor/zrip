#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::primitives;
use zrip_core::Sequence;
use zrip_core::fse::table_builder::{
    build_decode_table, normalize_counts, serialize_fse_table_description,
};
use zrip_core::fse::{
    FseDecodeEntry, LL_BASELINE_TABLE, LL_BITS_TABLE, LL_DEFAULT_ACCURACY, LL_DEFAULT_DIST,
    ML_BASELINE_TABLE, ML_BITS_TABLE, ML_DEFAULT_ACCURACY, ML_DEFAULT_DIST, OF_DEFAULT_ACCURACY,
    OF_DEFAULT_DIST,
};
use zrip_core::huffman::encode::HuffmanEncodeTable;

#[inline]
fn write_seq_count(output: &mut Vec<u8>, num_seq: u32) {
    if num_seq < 128 {
        output.push(num_seq as u8);
    } else if num_seq < 0x7F00 {
        output.push(((num_seq >> 8) + 128) as u8);
        output.push(num_seq as u8);
    } else {
        output.push(0xFF);
        let adj = num_seq - 0x7F00;
        output.push(adj as u8);
        output.push((adj >> 8) as u8);
    }
}

#[derive(Clone, Copy, Default)]
struct PackedSeq {
    extra_bits: u64,
    ll_c: u8,
    ml_c: u8,
    of_c: u8,
    extra_nbits: u8,
}

pub(crate) struct BlockEncodeWorkspace {
    pub lit_buf: Vec<u8>,
    pub lit_section: Vec<u8>,
    pub pred_seq: Vec<u8>,
    pub cust_seq: Vec<u8>,
    pub repeat_seq: Vec<u8>,
    pub huf_concat: Vec<u8>,
    pub huf_stream: Vec<u8>,
    pub pred_writer_buf: Vec<u8>,
    pub cust_writer_buf: Vec<u8>,
    pub repeat_writer_buf: Vec<u8>,
    packed_seqs: Vec<PackedSeq>,
    pub prev_huffman: Option<HuffmanEncodeTable>,
    pub prev_ll: Option<FseEncodeTable>,
    pub prev_of: Option<FseEncodeTable>,
    pub prev_ml: Option<FseEncodeTable>,
}

impl BlockEncodeWorkspace {
    pub fn new() -> Self {
        Self {
            lit_buf: Vec::new(),
            lit_section: Vec::new(),
            pred_seq: Vec::new(),
            cust_seq: Vec::new(),
            repeat_seq: Vec::new(),
            huf_concat: Vec::new(),
            huf_stream: Vec::new(),
            pred_writer_buf: Vec::new(),
            cust_writer_buf: Vec::new(),
            repeat_writer_buf: Vec::new(),
            packed_seqs: Vec::new(),
            prev_huffman: None,
            prev_ll: None,
            prev_of: None,
            prev_ml: None,
        }
    }
}

/// C zstd-style symbolTT entry. Two fields per symbol encode the
/// variable-width bit emission and state transition in one lookup.
/// Total predefined table footprint: ~1.3 KB (was ~20 KB).
#[derive(Clone, Copy)]
#[repr(C)]
struct SymbolTT {
    delta_nb_bits: u32,
    delta_find_state: i32,
}

#[derive(Clone)]
pub(crate) struct FseEncodeTable {
    symbol_tt: [SymbolTT; MAX_SYMBOLS],
    state_table: [u16; MAX_TABLE_SIZE],
    table_size: u32,
    table_log: u8,
}

const MAX_SYMBOLS: usize = 53;
const MAX_TABLE_SIZE: usize = 512;

impl FseEncodeTable {
    pub(crate) fn from_decode_table(
        decode_table: &[FseDecodeEntry],
        accuracy_log: u8,
        max_symbol: usize,
    ) -> Self {
        Self::build(decode_table, 1 << accuracy_log, max_symbol, accuracy_log)
    }

    fn build(
        decode_table: &[FseDecodeEntry],
        table_size: usize,
        max_symbol: usize,
        table_log: u8,
    ) -> Self {
        let num_symbols = max_symbol + 1;
        debug_assert!(num_symbols <= MAX_SYMBOLS);
        debug_assert!(table_size <= MAX_TABLE_SIZE);

        let mut count = [0u32; MAX_SYMBOLS];
        for i in 0..table_size {
            count[decode_table[i].symbol as usize] += 1;
        }

        let mut cumul = [0u32; MAX_SYMBOLS + 1];
        for s in 0..num_symbols {
            cumul[s + 1] = cumul[s] + count[s];
        }

        let default_delta = (table_log as u32 + 1) << 16;
        let mut symbol_tt = [SymbolTT {
            delta_nb_bits: default_delta,
            delta_find_state: 0,
        }; MAX_SYMBOLS];
        let mut total = 0i32;
        for s in 0..num_symbols {
            let c = count[s];
            if c == 0 {
                symbol_tt[s].delta_nb_bits = default_delta | (1u32 << table_log);
                continue;
            }
            let max_bits_out = if c == 1 {
                table_log as u32
            } else {
                table_log as u32 - (31 - (c - 1).leading_zeros())
            };
            let min_state_plus = c << max_bits_out;
            symbol_tt[s].delta_nb_bits = (max_bits_out << 16).wrapping_sub(min_state_plus);
            symbol_tt[s].delta_find_state = total - c as i32;
            total += c as i32;
        }

        let mut state_table = [0u16; MAX_TABLE_SIZE];
        let mut cumul_copy = cumul;
        for (i, entry) in decode_table.iter().enumerate().take(table_size) {
            let s = entry.symbol as usize;
            let idx = cumul_copy[s] as usize;
            state_table[idx] = (table_size + i) as u16;
            cumul_copy[s] += 1;
        }

        Self {
            symbol_tt,
            state_table,
            table_size: table_size as u32,
            table_log,
        }
    }

    #[inline]
    fn init_state(&self, symbol: u8) -> u32 {
        let tt = primitives::slice_get_ref(&self.symbol_tt, symbol as usize);
        let nb_bits_out = tt.delta_nb_bits.wrapping_add(1 << 15) >> 16;
        let base_state = (nb_bits_out << 16).wrapping_sub(tt.delta_nb_bits);
        let idx = (base_state >> nb_bits_out) as i32 + tt.delta_find_state;
        debug_assert!(idx >= 0);
        primitives::slice_get(&self.state_table, idx as usize) as u32
    }
}

#[cfg(feature = "std")]
static PREDEFINED_TABLES: std::sync::LazyLock<PredefinedEncodeTables> =
    std::sync::LazyLock::new(PredefinedEncodeTables::build);

#[cfg(feature = "std")]
fn predefined_tables() -> &'static PredefinedEncodeTables {
    &PREDEFINED_TABLES
}

#[cfg(not(feature = "std"))]
fn predefined_tables() -> PredefinedEncodeTables {
    PredefinedEncodeTables::build()
}

struct PredefinedEncodeTables {
    ll: FseEncodeTable,
    ml: FseEncodeTable,
    of: FseEncodeTable,
}

impl PredefinedEncodeTables {
    fn build() -> Self {
        use zrip_core::fse::table_builder::build_decode_table_from_default;

        let ll_decode = build_decode_table_from_default(&LL_DEFAULT_DIST, LL_DEFAULT_ACCURACY);
        let ml_decode = build_decode_table_from_default(&ML_DEFAULT_DIST, ML_DEFAULT_ACCURACY);
        let of_decode = build_decode_table_from_default(&OF_DEFAULT_DIST, OF_DEFAULT_ACCURACY);

        Self {
            ll: FseEncodeTable::build(
                &ll_decode,
                1 << LL_DEFAULT_ACCURACY,
                35,
                LL_DEFAULT_ACCURACY,
            ),
            ml: FseEncodeTable::build(
                &ml_decode,
                1 << ML_DEFAULT_ACCURACY,
                52,
                ML_DEFAULT_ACCURACY,
            ),
            of: FseEncodeTable::build(
                &of_decode,
                1 << OF_DEFAULT_ACCURACY,
                31,
                OF_DEFAULT_ACCURACY,
            ),
        }
    }
}

pub fn encode_raw_block(data: &[u8], last: bool, output: &mut Vec<u8>) {
    let block_size = data.len() as u32;
    let header = (block_size << 3) | if last { 1 } else { 0 };
    output.push(header as u8);
    output.push((header >> 8) as u8);
    output.push((header >> 16) as u8);
    output.extend_from_slice(data);
}

pub(crate) fn encode_compressed_block(
    src: &[u8],
    sequences: &[Sequence],
    rep_offsets: &mut [u32; 3],
    last: bool,
    output: &mut Vec<u8>,
    workspace: &mut BlockEncodeWorkspace,
) {
    if sequences.is_empty() {
        encode_raw_block(src, last, output);
        return;
    }

    let n = sequences.len();
    let total_match: usize = sequences.iter().map(|s| s.match_length as usize).sum();
    if total_match <= n * 3 {
        encode_raw_block(src, last, output);
        return;
    }

    let saved_rep = *rep_offsets;

    pack_sequences_and_literals(src, sequences, rep_offsets, workspace);

    encode_literals_section(
        &workspace.lit_buf,
        &mut workspace.lit_section,
        &mut workspace.huf_concat,
        &mut workspace.huf_stream,
        &mut workspace.prev_huffman,
    );

    let has_repeat =
        workspace.prev_ll.is_some() && workspace.prev_of.is_some() && workspace.prev_ml.is_some();

    if has_repeat {
        encode_seq_repeat(
            &workspace.packed_seqs,
            workspace.prev_ll.as_ref().unwrap(),
            workspace.prev_of.as_ref().unwrap(),
            workspace.prev_ml.as_ref().unwrap(),
            &mut workspace.repeat_seq,
            &mut workspace.repeat_writer_buf,
        );
    }

    let do_codes = n >= 64;
    let seq_data = if do_codes {
        let (ll_freq, ml_freq, of_freq, total_extra_bits) =
            compute_frequencies(&workspace.packed_seqs);
        let pred_est = estimate_predefined_cost(&ll_freq, &ml_freq, &of_freq, total_extra_bits, n);
        let use_custom = encode_seq_custom(
            &workspace.packed_seqs,
            &ll_freq,
            &ml_freq,
            &of_freq,
            n,
            pred_est,
            &mut workspace.cust_seq,
            &mut workspace.cust_writer_buf,
        );
        if use_custom && workspace.cust_seq.len() < pred_est {
            &workspace.cust_seq
        } else {
            encode_seq_predefined(
                &workspace.packed_seqs,
                &mut workspace.pred_seq,
                &mut workspace.pred_writer_buf,
            );
            if use_custom && workspace.cust_seq.len() < workspace.pred_seq.len() {
                &workspace.cust_seq
            } else {
                &workspace.pred_seq
            }
        }
    } else {
        encode_seq_predefined(
            &workspace.packed_seqs,
            &mut workspace.pred_seq,
            &mut workspace.pred_writer_buf,
        );
        &workspace.pred_seq
    };

    let seq_data = if has_repeat && workspace.repeat_seq.len() < seq_data.len() {
        &workspace.repeat_seq
    } else {
        seq_data
    };

    // Clear dict FSE tables after first block
    workspace.prev_ll = None;
    workspace.prev_of = None;
    workspace.prev_ml = None;

    let block_len = workspace.lit_section.len() + seq_data.len();
    if block_len >= src.len() {
        *rep_offsets = saved_rep;
        encode_raw_block(src, last, output);
        return;
    }

    let block_size = block_len as u32;
    let header = (block_size << 3) | 0x04 | if last { 1 } else { 0 };
    output.push(header as u8);
    output.push((header >> 8) as u8);
    output.push((header >> 16) as u8);
    output.extend_from_slice(&workspace.lit_section);
    output.extend_from_slice(seq_data);
}

pub(crate) fn encode_compressed_block_raw(
    src: &[u8],
    sequences: &[Sequence],
    rep_offsets: &mut [u32; 3],
    last: bool,
    output: &mut Vec<u8>,
    workspace: &mut BlockEncodeWorkspace,
) {
    if sequences.is_empty() {
        encode_raw_block(src, last, output);
        return;
    }

    let n = sequences.len();
    let total_match: usize = sequences.iter().map(|s| s.match_length as usize).sum();
    if total_match <= n * 3 {
        encode_raw_block(src, last, output);
        return;
    }

    let saved_rep = *rep_offsets;

    pack_sequences_and_literals(src, sequences, rep_offsets, workspace);

    workspace.lit_section.clear();
    encode_raw_literals_section(&workspace.lit_buf, &mut workspace.lit_section);

    encode_seq_predefined(
        &workspace.packed_seqs,
        &mut workspace.pred_seq,
        &mut workspace.pred_writer_buf,
    );

    let block_len = workspace.lit_section.len() + workspace.pred_seq.len();
    if block_len >= src.len() {
        *rep_offsets = saved_rep;
        encode_raw_block(src, last, output);
        return;
    }

    let block_size = block_len as u32;
    let header = (block_size << 3) | 0x04 | if last { 1 } else { 0 };
    output.push(header as u8);
    output.push((header >> 8) as u8);
    output.push((header >> 16) as u8);
    output.extend_from_slice(&workspace.lit_section);
    output.extend_from_slice(&workspace.pred_seq);
}

fn pack_sequences_and_literals(
    src: &[u8],
    sequences: &[Sequence],
    rep_offsets: &mut [u32; 3],
    workspace: &mut BlockEncodeWorkspace,
) {
    let n = sequences.len();
    let src_len = src.len();

    workspace.lit_buf.clear();
    workspace.lit_buf.reserve(src_len + 16);
    workspace.packed_seqs.clear();
    workspace.packed_seqs.reserve(n);

    let mut lit_pos = 0usize;
    let mut lit_offset = 0usize;

    let mut rep0 = rep_offsets[0];
    let mut rep1 = rep_offsets[1];
    let mut rep2 = rep_offsets[2];

    for (i, seq) in sequences[..n].iter().enumerate() {
        let ll = seq.literal_length as usize;
        if lit_offset + ll <= src_len {
            primitives::copy_literals_fast(src, lit_offset, &mut workspace.lit_buf, lit_pos, ll);
            lit_pos += ll;
        }
        lit_offset += ll + seq.match_length as usize;

        let actual_offset = seq.offset;
        let ov = if seq.literal_length > 0 {
            if actual_offset == rep0 {
                1
            } else if actual_offset == rep1 {
                rep1 = rep0;
                rep0 = actual_offset;
                2
            } else if actual_offset == rep2 {
                rep2 = rep1;
                rep1 = rep0;
                rep0 = actual_offset;
                3
            } else {
                rep2 = rep1;
                rep1 = rep0;
                rep0 = actual_offset;
                actual_offset + 3
            }
        } else if actual_offset == rep1 {
            rep1 = rep0;
            rep0 = actual_offset;
            1
        } else if actual_offset == rep2 {
            rep2 = rep1;
            rep1 = rep0;
            rep0 = actual_offset;
            2
        } else if rep0 > 1 && actual_offset == rep0 - 1 {
            rep2 = rep1;
            rep1 = rep0;
            rep0 = actual_offset;
            3
        } else {
            rep2 = rep1;
            rep1 = rep0;
            rep0 = actual_offset;
            actual_offset + 3
        };

        let ll_c = ll_code(seq.literal_length);
        let ml_c = ml_code(seq.match_length);
        let of_c = of_code(ov);

        let ll_nb = LL_BITS_TABLE[ll_c as usize];
        let ml_nb = ML_BITS_TABLE[ml_c as usize];
        let ll_extra = seq.literal_length - LL_BASELINE_TABLE[ll_c as usize];
        let ml_extra = seq.match_length - ML_BASELINE_TABLE[ml_c as usize];
        let of_extra = ov - (1u32 << of_c);
        let extra_bits = (ll_extra as u64)
            | ((ml_extra as u64) << ll_nb)
            | ((of_extra as u64) << (ll_nb + ml_nb));
        let extra_nbits = ll_nb + ml_nb + of_c;

        primitives::vec_write_at(
            &mut workspace.packed_seqs,
            i,
            PackedSeq {
                extra_bits,
                ll_c,
                ml_c,
                of_c,
                extra_nbits,
            },
        );
    }
    primitives::set_vec_len(&mut workspace.packed_seqs, n);
    rep_offsets[0] = rep0;
    rep_offsets[1] = rep1;
    rep_offsets[2] = rep2;

    if lit_offset < src_len {
        let tail = src_len - lit_offset;
        primitives::copy_literals_fast(src, lit_offset, &mut workspace.lit_buf, lit_pos, tail);
        lit_pos += tail;
    }
    primitives::set_vec_len(&mut workspace.lit_buf, lit_pos);
}

fn compute_frequencies(packed: &[PackedSeq]) -> ([u32; 36], [u32; 53], [u32; 32], u64) {
    let mut ll_freq = [0u32; 36];
    let mut ml_freq = [0u32; 53];
    let mut of_freq = [0u32; 32];
    let mut total_extra_bits: u64 = 0;
    for p in packed {
        ll_freq[p.ll_c as usize] += 1;
        ml_freq[p.ml_c as usize] += 1;
        of_freq[p.of_c as usize] += 1;
        total_extra_bits += p.extra_nbits as u64;
    }
    (ll_freq, ml_freq, of_freq, total_extra_bits)
}

/// Approximate log2(x) in 8.8 fixed point (result = log2(x) * 256).
fn log2_fp8(x: u32) -> u32 {
    if x <= 1 {
        return 0;
    }
    let msb = 31 - x.leading_zeros();
    let shifted = if msb > 8 {
        x >> (msb - 8)
    } else {
        x << (8 - msb)
    };
    msb * 256 + (shifted - 256)
}

/// Cheap entropy estimate: returns true if Huffman is likely to produce savings.
/// Avoids the expensive build-table-encode-discard cycle for blocks where
/// Huffman overhead exceeds the compression benefit.
fn huf_worth_trying(data: &[u8]) -> bool {
    if data.len() > 32768 {
        return true;
    }
    if data.len() < 64 {
        return false;
    }

    let n = data.len() as u32;
    let mut freqs = [0u32; 256];
    let mut max_sym = 0u8;
    for &b in data {
        freqs[b as usize] += 1;
        if b > max_sym {
            max_sym = b;
        }
    }

    if max_sym > 128 {
        return false;
    }

    let active = freqs[..=max_sym as usize]
        .iter()
        .filter(|&&f| f > 0)
        .count();
    if active < 2 {
        return false;
    }

    let log2_n = log2_fp8(n);
    let mut entropy_bits_fp8: u64 = 0;
    for &f in &freqs[..=max_sym as usize] {
        if f > 0 {
            entropy_bits_fp8 += f as u64 * (log2_n - log2_fp8(f)) as u64;
        }
    }

    let estimated_bytes = entropy_bits_fp8.div_ceil(2048);
    let tree_overhead = 1 + (max_sym as u64).div_ceil(2);
    let min_gain = n as u64 / 32;

    estimated_bytes + tree_overhead + min_gain < n as u64
}

fn encode_literals_section(
    lits: &[u8],
    output: &mut Vec<u8>,
    huf_concat: &mut Vec<u8>,
    huf_stream: &mut Vec<u8>,
    prev_huffman: &mut Option<HuffmanEncodeTable>,
) {
    output.clear();

    let use_4_streams = lits.len() >= 1024;

    if let Some(prev) = prev_huffman.as_ref()
        && prev.can_encode(lits)
    {
        if use_4_streams {
            prev.encode_4_streams_into(lits, huf_concat, huf_stream);
        } else {
            prev.encode_single_stream_into(lits, huf_concat);
        }
        let compressed_size = huf_concat.len();
        if compressed_size < lits.len() {
            encode_treeless_literals_header(lits.len(), compressed_size, use_4_streams, output);
            output.extend_from_slice(huf_concat);
            return;
        }
    }

    if !huf_worth_trying(lits) {
        *prev_huffman = None;
        encode_raw_literals_section(lits, output);
        return;
    }

    if let Some(table) = HuffmanEncodeTable::from_data(lits) {
        let tree_desc = table.serialize_weights();
        if use_4_streams {
            table.encode_4_streams_into(lits, huf_concat, huf_stream);
        } else {
            table.encode_single_stream_into(lits, huf_concat);
        }

        let compressed_size = tree_desc.len() + huf_concat.len();

        if compressed_size < lits.len() {
            encode_compressed_literals_header(lits.len(), compressed_size, use_4_streams, output);
            output.extend_from_slice(&tree_desc);
            output.extend_from_slice(huf_concat);
            *prev_huffman = Some(table);
            return;
        }
    }

    *prev_huffman = None;
    encode_raw_literals_section(lits, output);
}

fn encode_treeless_literals_header(
    regen_size: usize,
    comp_size: usize,
    four_streams: bool,
    output: &mut Vec<u8>,
) {
    encode_huf_literals_header(0x03, regen_size, comp_size, four_streams, output);
}

fn encode_compressed_literals_header(
    regen_size: usize,
    comp_size: usize,
    four_streams: bool,
    output: &mut Vec<u8>,
) {
    encode_huf_literals_header(0x02, regen_size, comp_size, four_streams, output);
}

fn encode_huf_literals_header(
    block_type: u8,
    regen_size: usize,
    comp_size: usize,
    four_streams: bool,
    output: &mut Vec<u8>,
) {
    if regen_size <= 1023 && comp_size <= 1023 {
        let size_format: u8 = if four_streams { 1 } else { 0 };
        let both = (regen_size as u32) | ((comp_size as u32) << 10);
        output.push(block_type | (size_format << 2) | (((both & 0x0F) as u8) << 4));
        output.push((both >> 4) as u8);
        output.push((both >> 12) as u8);
    } else if regen_size <= 16383 && comp_size <= 16383 {
        let both = (regen_size as u32) | ((comp_size as u32) << 14);
        output.push(block_type | (2 << 2) | (((both & 0x0F) as u8) << 4));
        output.push((both >> 4) as u8);
        output.push((both >> 12) as u8);
        output.push((both >> 20) as u8);
    } else {
        let both = (regen_size as u64) | ((comp_size as u64) << 18);
        output.push(block_type | (3 << 2) | (((both & 0x0F) as u8) << 4));
        output.push((both >> 4) as u8);
        output.push((both >> 12) as u8);
        output.push((both >> 20) as u8);
        output.push((both >> 28) as u8);
    }
}

fn encode_raw_literals_section(lits: &[u8], output: &mut Vec<u8>) {
    let size = lits.len();

    if size <= 31 {
        output.push((size as u8) << 3);
    } else if size <= 4095 {
        let b0 = 0x04 | ((size as u8 & 0x0F) << 4);
        let b1 = (size >> 4) as u8;
        output.push(b0);
        output.push(b1);
    } else {
        let b0 = 0x0C | ((size as u8 & 0x0F) << 4);
        let b1 = (size >> 4) as u8;
        let b2 = (size >> 12) as u8;
        output.push(b0);
        output.push(b1);
        output.push(b2);
    }

    output.extend_from_slice(lits);
}

fn encode_seq_predefined(packed: &[PackedSeq], output: &mut Vec<u8>, writer_buf: &mut Vec<u8>) {
    let n = packed.len();
    let num_seq = n as u32;
    output.clear();
    output.reserve(n * 4 + 4);

    write_seq_count(output, num_seq);

    output.push(0x00);

    let tables = predefined_tables();
    let last = primitives::slice_get_ref(packed, n - 1);

    let max_out = n * 18 + 16;
    writer_buf.clear();
    writer_buf.reserve(max_out);

    let mut ll_state = tables.ll.init_state(last.ll_c);
    let mut of_state = tables.of.init_state(last.of_c);
    let mut ml_state = tables.ml.init_state(last.ml_c);

    let mut pos: usize = 0;
    let mut bits: u64 = 0;
    let mut bits_used: u32 = 0;

    macro_rules! add_bits {
        ($val:expr, $n:expr) => {
            bits |= ($val as u64) << bits_used;
            bits_used += $n as u32;
        };
    }
    macro_rules! flush_fast {
        () => {
            primitives::bitstream_flush(writer_buf, pos, bits);
            let nb = (bits_used >> 3) as usize;
            pos += nb;
            bits >>= (nb << 3) as u64;
            bits_used &= 7;
        };
    }
    macro_rules! encode_transition {
        ($table:expr, $symbol:expr, $state:expr) => {{
            let tt = primitives::slice_get_ref(&$table.symbol_tt, $symbol as usize);
            let nb = (tt.delta_nb_bits.wrapping_add($state)) >> 16;
            add_bits!($state & ((1u32 << nb) - 1), nb);
            let idx = ($state >> nb) as i32 + tt.delta_find_state;
            debug_assert!(idx >= 0);
            $state = primitives::slice_get(&$table.state_table, idx as usize) as u32;
        }};
    }

    add_bits!(last.extra_bits, last.extra_nbits);

    for k in (0..n - 1).rev() {
        let p = primitives::slice_get_ref(packed, k);

        flush_fast!();
        encode_transition!(tables.of, p.of_c, of_state);
        encode_transition!(tables.ml, p.ml_c, ml_state);
        encode_transition!(tables.ll, p.ll_c, ll_state);
        flush_fast!();

        add_bits!(p.extra_bits, p.extra_nbits);
    }

    flush_fast!();
    let ll_ts = tables.ll.table_size;
    let of_ts = tables.of.table_size;
    let ml_ts = tables.ml.table_size;
    add_bits!(ml_state - ml_ts, ML_DEFAULT_ACCURACY);
    add_bits!(of_state - of_ts, OF_DEFAULT_ACCURACY);
    add_bits!(ll_state - ll_ts, LL_DEFAULT_ACCURACY);

    add_bits!(1u32, 1u8);
    flush_fast!();
    while bits_used > 0 {
        primitives::bitstream_write_byte(writer_buf, pos, bits as u8);
        bits >>= 8;
        bits_used = bits_used.saturating_sub(8);
        pos += 1;
    }
    primitives::set_vec_len(writer_buf, pos);
    output.extend_from_slice(writer_buf);
}

fn encode_seq_repeat(
    packed: &[PackedSeq],
    ll_t: &FseEncodeTable,
    of_t: &FseEncodeTable,
    ml_t: &FseEncodeTable,
    output: &mut Vec<u8>,
    writer_buf: &mut Vec<u8>,
) {
    let n = packed.len();
    let num_seq = n as u32;
    output.clear();
    output.reserve(n * 4 + 4);

    write_seq_count(output, num_seq);

    // Mode byte: repeat (3) for all three streams
    output.push((3 << 6) | (3 << 4) | (3 << 2));

    let last = primitives::slice_get_ref(packed, n - 1);

    let max_out = n * 18 + 16;
    writer_buf.clear();
    writer_buf.reserve(max_out);

    let mut ll_s = ll_t.init_state(last.ll_c);
    let mut of_s = of_t.init_state(last.of_c);
    let mut ml_s = ml_t.init_state(last.ml_c);

    let mut pos: usize = 0;
    let mut bits: u64 = 0;
    let mut bits_used: u32 = 0;

    macro_rules! add_bits {
        ($val:expr, $n:expr) => {
            bits |= ($val as u64) << bits_used;
            bits_used += $n as u32;
        };
    }
    macro_rules! flush_fast {
        () => {
            primitives::bitstream_flush(writer_buf, pos, bits);
            let nb = (bits_used >> 3) as usize;
            pos += nb;
            bits >>= (nb << 3) as u64;
            bits_used &= 7;
        };
    }

    add_bits!(last.extra_bits, last.extra_nbits);

    for k in (0..n - 1).rev() {
        let p = primitives::slice_get_ref(packed, k);

        flush_fast!();
        {
            let tt = primitives::slice_get_ref(&of_t.symbol_tt, p.of_c as usize);
            let nb = (tt.delta_nb_bits.wrapping_add(of_s)) >> 16;
            add_bits!(of_s & ((1u32 << nb) - 1), nb);
            let idx = (of_s >> nb) as i32 + tt.delta_find_state;
            of_s = primitives::slice_get(&of_t.state_table, idx as usize) as u32;
        }
        {
            let tt = primitives::slice_get_ref(&ml_t.symbol_tt, p.ml_c as usize);
            let nb = (tt.delta_nb_bits.wrapping_add(ml_s)) >> 16;
            add_bits!(ml_s & ((1u32 << nb) - 1), nb);
            let idx = (ml_s >> nb) as i32 + tt.delta_find_state;
            ml_s = primitives::slice_get(&ml_t.state_table, idx as usize) as u32;
        }
        {
            let tt = primitives::slice_get_ref(&ll_t.symbol_tt, p.ll_c as usize);
            let nb = (tt.delta_nb_bits.wrapping_add(ll_s)) >> 16;
            add_bits!(ll_s & ((1u32 << nb) - 1), nb);
            let idx = (ll_s >> nb) as i32 + tt.delta_find_state;
            ll_s = primitives::slice_get(&ll_t.state_table, idx as usize) as u32;
        }
        flush_fast!();

        add_bits!(p.extra_bits, p.extra_nbits);
    }

    flush_fast!();
    add_bits!(ml_s - ml_t.table_size, ml_t.table_log);
    add_bits!(of_s - of_t.table_size, of_t.table_log);
    add_bits!(ll_s - ll_t.table_size, ll_t.table_log);

    add_bits!(1u32, 1u8);
    flush_fast!();
    while bits_used > 0 {
        primitives::bitstream_write_byte(writer_buf, pos, bits as u8);
        bits >>= 8;
        bits_used = bits_used.saturating_sub(8);
        pos += 1;
    }
    primitives::set_vec_len(writer_buf, pos);
    output.extend_from_slice(writer_buf);
}

#[allow(clippy::large_enum_variant)]
enum TableEnc {
    Compressed {
        enc_table: FseEncodeTable,
        header: Vec<u8>,
    },
    Rle(u8),
}

fn build_table_enc(freqs: &[u32], max_sym: usize, max_log: u8, n: usize) -> Option<TableEnc> {
    let distinct: Vec<usize> = (0..=max_sym).filter(|&s| freqs[s] > 0).collect();
    if distinct.len() <= 1 {
        return Some(TableEnc::Rle(distinct.first().copied().unwrap_or(0) as u8));
    }
    let num_distinct = distinct.len();
    let min_log_for_sym = (32 - ((num_distinct as u32 + 1).leading_zeros())) as u8;
    let acc = optimal_table_log(max_log, n, max_sym).max(min_log_for_sym);
    if acc > max_log {
        return None;
    }
    let dist = normalize_counts(&freqs[..=max_sym], acc);
    for s in 0..=max_sym {
        if freqs[s] > 0 && dist[s] == 0 {
            return None;
        }
    }
    let header = serialize_fse_table_description(&dist, acc);
    let decode_table = build_decode_table(&dist, acc).ok()?;
    let enc_table = FseEncodeTable::build(&decode_table, 1 << acc, max_sym, acc);
    Some(TableEnc::Compressed { enc_table, header })
}

#[allow(clippy::too_many_arguments)]
fn encode_seq_custom(
    packed: &[PackedSeq],
    ll_freq: &[u32; 36],
    ml_freq: &[u32; 53],
    of_freq: &[u32; 32],
    n: usize,
    pred_size: usize,
    output: &mut Vec<u8>,
    writer_buf: &mut Vec<u8>,
) -> bool {
    output.clear();

    let ll_max = ll_freq.iter().rposition(|&f| f > 0).unwrap_or(0);
    let ml_max = ml_freq.iter().rposition(|&f| f > 0).unwrap_or(0);
    let of_max = of_freq.iter().rposition(|&f| f > 0).unwrap_or(0);

    let estimated_bits = estimate_fse_bits(&ll_freq[..=ll_max], n)
        + estimate_fse_bits(&ml_freq[..=ml_max], n)
        + estimate_fse_bits(&of_freq[..=of_max], n);
    let estimated_header = 20;
    let estimated_bytes = estimated_bits.div_ceil(8) + estimated_header + 4;
    if estimated_bytes >= pred_size {
        return false;
    }

    let Some(ll_enc) = build_table_enc(ll_freq, ll_max, 9, n) else {
        return false;
    };
    let Some(of_enc) = build_table_enc(of_freq, of_max, 8, n) else {
        return false;
    };
    let Some(ml_enc) = build_table_enc(ml_freq, ml_max, 9, n) else {
        return false;
    };

    let ll_mode: u8 = match &ll_enc {
        TableEnc::Compressed { .. } => 2,
        TableEnc::Rle(_) => 1,
    };
    let of_mode: u8 = match &of_enc {
        TableEnc::Compressed { .. } => 2,
        TableEnc::Rle(_) => 1,
    };
    let ml_mode: u8 = match &ml_enc {
        TableEnc::Compressed { .. } => 2,
        TableEnc::Rle(_) => 1,
    };

    let num_seq = n as u32;
    output.reserve(n * 4 + 64);

    write_seq_count(output, num_seq);

    output.push((ll_mode << 6) | (of_mode << 4) | (ml_mode << 2));

    match &ll_enc {
        TableEnc::Rle(sym) => output.push(*sym),
        TableEnc::Compressed { header, .. } => output.extend_from_slice(header),
    }
    match &of_enc {
        TableEnc::Rle(sym) => output.push(*sym),
        TableEnc::Compressed { header, .. } => output.extend_from_slice(header),
    }
    match &ml_enc {
        TableEnc::Rle(sym) => output.push(*sym),
        TableEnc::Compressed { header, .. } => output.extend_from_slice(header),
    }

    let last = &packed[n - 1];

    let max_out = n * 18 + 16;
    writer_buf.clear();
    writer_buf.reserve(max_out);
    let mut pos: usize = 0;
    let mut bits: u64 = 0;
    let mut bits_used: u32 = 0;

    macro_rules! add_bits {
        ($val:expr, $n:expr) => {
            bits |= ($val as u64) << bits_used;
            bits_used += $n as u32;
        };
    }
    macro_rules! flush_fast {
        () => {
            primitives::bitstream_flush(writer_buf, pos, bits);
            let nb = (bits_used >> 3) as usize;
            pos += nb;
            bits >>= (nb << 3) as u64;
            bits_used &= 7;
        };
    }

    add_bits!(last.extra_bits, last.extra_nbits);

    match (&ll_enc, &of_enc, &ml_enc) {
        (
            TableEnc::Compressed {
                enc_table: ll_t, ..
            },
            TableEnc::Compressed {
                enc_table: of_t, ..
            },
            TableEnc::Compressed {
                enc_table: ml_t, ..
            },
        ) => {
            let mut ll_s = ll_t.init_state(last.ll_c);
            let mut of_s = of_t.init_state(last.of_c);
            let mut ml_s = ml_t.init_state(last.ml_c);

            for k in (0..n - 1).rev() {
                let p = primitives::slice_get_ref(packed, k);

                flush_fast!();
                {
                    let tt = primitives::slice_get_ref(&of_t.symbol_tt, p.of_c as usize);
                    let nb = (tt.delta_nb_bits.wrapping_add(of_s)) >> 16;
                    add_bits!(of_s & ((1u32 << nb) - 1), nb);
                    let idx = (of_s >> nb) as i32 + tt.delta_find_state;
                    of_s = primitives::slice_get(&of_t.state_table, idx as usize) as u32;
                }
                {
                    let tt = primitives::slice_get_ref(&ml_t.symbol_tt, p.ml_c as usize);
                    let nb = (tt.delta_nb_bits.wrapping_add(ml_s)) >> 16;
                    add_bits!(ml_s & ((1u32 << nb) - 1), nb);
                    let idx = (ml_s >> nb) as i32 + tt.delta_find_state;
                    ml_s = primitives::slice_get(&ml_t.state_table, idx as usize) as u32;
                }
                {
                    let tt = primitives::slice_get_ref(&ll_t.symbol_tt, p.ll_c as usize);
                    let nb = (tt.delta_nb_bits.wrapping_add(ll_s)) >> 16;
                    add_bits!(ll_s & ((1u32 << nb) - 1), nb);
                    let idx = (ll_s >> nb) as i32 + tt.delta_find_state;
                    ll_s = primitives::slice_get(&ll_t.state_table, idx as usize) as u32;
                }
                flush_fast!();

                add_bits!(p.extra_bits, p.extra_nbits);
            }

            flush_fast!();
            add_bits!(ml_s - ml_t.table_size, ml_t.table_log);
            add_bits!(of_s - of_t.table_size, of_t.table_log);
            add_bits!(ll_s - ll_t.table_size, ll_t.table_log);
        }
        _ => {
            let ll_table = match &ll_enc {
                TableEnc::Compressed { enc_table, .. } => Some(enc_table),
                TableEnc::Rle(_) => None,
            };
            let of_table = match &of_enc {
                TableEnc::Compressed { enc_table, .. } => Some(enc_table),
                TableEnc::Rle(_) => None,
            };
            let ml_table = match &ml_enc {
                TableEnc::Compressed { enc_table, .. } => Some(enc_table),
                TableEnc::Rle(_) => None,
            };

            let mut ll_state = ll_table.as_ref().map(|t| t.init_state(last.ll_c));
            let mut of_state = of_table.as_ref().map(|t| t.init_state(last.of_c));
            let mut ml_state = ml_table.as_ref().map(|t| t.init_state(last.ml_c));

            macro_rules! encode_transition_opt {
                ($table:expr, $state:expr, $symbol:expr) => {
                    if let (Some(t), Some(s)) = (&$table, &mut $state) {
                        let tt = primitives::slice_get_ref(&t.symbol_tt, $symbol as usize);
                        let nb = (tt.delta_nb_bits.wrapping_add(*s)) >> 16;
                        add_bits!(*s & ((1u32 << nb) - 1), nb);
                        let idx = (*s >> nb) as i32 + tt.delta_find_state;
                        *s = primitives::slice_get(&t.state_table, idx as usize) as u32;
                    }
                };
            }

            for k in (0..n - 1).rev() {
                let p = &packed[k];

                flush_fast!();
                encode_transition_opt!(of_table, of_state, p.of_c);
                encode_transition_opt!(ml_table, ml_state, p.ml_c);
                encode_transition_opt!(ll_table, ll_state, p.ll_c);
                flush_fast!();

                add_bits!(p.extra_bits, p.extra_nbits);
            }

            flush_fast!();
            if let (Some(t), Some(s)) = (&ml_table, ml_state) {
                add_bits!(s - t.table_size, t.table_log);
            }
            if let (Some(t), Some(s)) = (&of_table, of_state) {
                add_bits!(s - t.table_size, t.table_log);
            }
            if let (Some(t), Some(s)) = (&ll_table, ll_state) {
                add_bits!(s - t.table_size, t.table_log);
            }
        }
    }

    add_bits!(1u32, 1u8);
    flush_fast!();
    while bits_used > 0 {
        primitives::bitstream_write_byte(writer_buf, pos, bits as u8);
        bits >>= 8;
        bits_used = bits_used.saturating_sub(8);
        pos += 1;
    }
    primitives::set_vec_len(writer_buf, pos);
    output.extend_from_slice(writer_buf);

    true
}

// Predefined FSE bit costs per symbol, in 256ths of a bit.
// cost_256 = 256 * (accuracy_log - log2(max(1, |dist[s]|)))
#[rustfmt::skip]
static LL_PRED_COST_256: [u16; 36] = [
    1024, 1130, 1280, 1280, 1280, 1280, 1280, 1280, 1280, 1280, 1280, 1280, 1280,
    1536, 1536, 1536,
    1280, 1280, 1280, 1280, 1280, 1280, 1280, 1280, 1280,
    1130, 1280,
    1536, 1536, 1536, 1536, 1536,
    1536, 1536, 1536, 1536,
];

#[rustfmt::skip]
static ML_PRED_COST_256: [u16; 53] = [
    1536,
    1024, 1130, 1280, 1280, 1280, 1280, 1280, 1280,
    1536, 1536, 1536, 1536, 1536, 1536, 1536, 1536, 1536, 1536, 1536,
    1536, 1536, 1536, 1536, 1536, 1536, 1536, 1536, 1536, 1536, 1536, 1536,
    1536, 1536, 1536, 1536, 1536, 1536, 1536, 1536, 1536, 1536, 1536, 1536, 1536, 1536,
    1536, 1536, 1536, 1536, 1536, 1536, 1536,
];

#[rustfmt::skip]
static OF_PRED_COST_256: [u16; 32] = [
    1280, 1280, 1280, 1280, 1280, 1280,
    1024, 1024, 1024,
    1280, 1280, 1280, 1280, 1280, 1280, 1280, 1280, 1280, 1280, 1280,
    1280, 1280, 1280, 1280,
    1280, 1280, 1280, 1280, 1280, 1280, 1280, 1280,
];

fn estimate_predefined_cost(
    ll_freq: &[u32; 36],
    ml_freq: &[u32; 53],
    of_freq: &[u32; 32],
    extra_bits: u64,
    n: usize,
) -> usize {
    let mut fse_cost_256: u64 = 0;
    for (c, &f) in ll_freq.iter().enumerate() {
        fse_cost_256 += f as u64 * LL_PRED_COST_256[c] as u64;
    }
    for (c, &f) in ml_freq.iter().enumerate() {
        fse_cost_256 += f as u64 * ML_PRED_COST_256[c] as u64;
    }
    for (c, &f) in of_freq.iter().enumerate() {
        fse_cost_256 += f as u64 * OF_PRED_COST_256[c] as u64;
    }

    let total_bits = fse_cost_256.div_ceil(256) + extra_bits;
    let header_bytes = if n < 128 {
        2
    } else if n < 0x7F00 {
        3
    } else {
        4
    };
    (total_bits as usize + 17 + 8) / 8 + header_bytes
}

fn estimate_fse_bits(freqs: &[u32], total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    let log2_total = (usize::BITS).saturating_sub(total.leading_zeros());
    let mut bits: u64 = 0;
    for &f in freqs {
        if f > 0 {
            let log2_f = (u32::BITS).saturating_sub(f.leading_zeros());
            let cost = log2_total.saturating_sub(log2_f);
            bits += f as u64 * cost as u64;
        }
    }
    bits as usize
}

fn optimal_table_log(max_log: u8, num_seq: usize, max_symbol: usize) -> u8 {
    let min_log = 5u8;
    if num_seq <= 1 {
        return min_log;
    }
    let seq_log = 32 - ((num_seq as u32 - 1).leading_zeros());
    let min_bits_sym = if max_symbol > 0 {
        (32 - (max_symbol as u32).leading_zeros()) as u8 + 2
    } else {
        0
    };
    let target = seq_log.saturating_sub(2) as u8;
    target.max(min_log).max(min_bits_sym).min(max_log)
}

#[rustfmt::skip]
static LL_CODE_TABLE: [u8; 64] = [
     0,  1,  2,  3,  4,  5,  6,  7,  8,  9, 10, 11, 12, 13, 14, 15,
    16, 16, 17, 17, 18, 18, 19, 19, 20, 20, 20, 20, 21, 21, 21, 21,
    22, 22, 22, 22, 22, 22, 22, 22, 23, 23, 23, 23, 23, 23, 23, 23,
    24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
];

#[inline]
fn ll_code(ll: u32) -> u8 {
    if ll < 64 {
        return LL_CODE_TABLE[ll as usize];
    }
    let high_bit = 31 - ll.leading_zeros();
    high_bit as u8 + 19
}

#[rustfmt::skip]
static ML_CODE_TABLE: [u8; 128] = [
    // mlBase = matchLength - 3, indexed 0..127
     0,  1,  2,  3,  4,  5,  6,  7,  8,  9, 10, 11, 12, 13, 14, 15,
    16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
    32, 32, 33, 33, 34, 34, 35, 35, 36, 36, 36, 36, 37, 37, 37, 37,
    38, 38, 38, 38, 38, 38, 38, 38, 39, 39, 39, 39, 39, 39, 39, 39,
    40, 40, 40, 40, 40, 40, 40, 40, 40, 40, 40, 40, 40, 40, 40, 40,
    41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41,
    42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42,
    42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42,
];

#[inline]
fn ml_code(ml: u32) -> u8 {
    let ml_base = ml - 3;
    if ml_base < 128 {
        return ML_CODE_TABLE[ml_base as usize];
    }
    let high_bit = 31 - ml_base.leading_zeros();
    high_bit as u8 + 36
}

#[inline]
fn of_code(offset_value: u32) -> u8 {
    (31 - offset_value.leading_zeros()) as u8
}
