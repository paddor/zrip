//! Block encoder: turns a block's sequences into a literals section and an
//! FSE-coded sequences section, or writes the block raw when that is not
//! smaller.
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::output::OutputSink;
use crate::primitives;
use crate::strategy::BlockPolicy;
use zrip_core::Sequence;
use zrip_core::bitstream::writer::BitWriter;
use zrip_core::error::CompressError;
use zrip_core::fse::{
    FseDecodeEntry, LL_DEFAULT_ACCURACY, LL_DEFAULT_DIST, ML_DEFAULT_ACCURACY, ML_DEFAULT_DIST,
    OF_DEFAULT_ACCURACY, OF_DEFAULT_DIST,
};
use zrip_core::huffman::encode::{HuffmanEncodeTable, byte_histogram};

/// Defines `$name!(p)`, which appends the extra bits of packed sequence `p`
/// after its three state transitions.
///
/// At most 7 + 26 bits are pending after the transitions, so up to 30 extra
/// bits fit without a flush, as in C zstd. Longer extras flush first, and
/// more than 56 (a long literal run, a long match, and a far offset in one
/// sequence) go in two parts, so the container never holds 64 bits.
macro_rules! add_extra_bits_macro {
    ($name:ident, $add_bits:ident, $flush:ident) => {
        macro_rules! $name {
            ($p:expr) => {
                if $p.extra_nbits <= 30 {
                    $add_bits!($p.extra_bits, $p.extra_nbits);
                } else {
                    $flush!();
                    if $p.extra_nbits <= 56 {
                        $add_bits!($p.extra_bits, $p.extra_nbits);
                    } else {
                        $add_bits!($p.extra_bits & 0xFFFF_FFFF, 32);
                        $flush!();
                        $add_bits!($p.extra_bits >> 32, $p.extra_nbits - 32);
                    }
                }
            };
        }
    };
}

/// Writes the Number_of_Sequences field, 1 to 3 bytes.
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

/// A sequence's three codes and their extra bits, packed from the low bit:
/// literal length, then match length, then offset.
#[derive(Clone, Copy, Default)]
struct PackedSeq {
    extra_bits: u64,
    ll_c: u8,
    ml_c: u8,
    of_c: u8,
    extra_nbits: u8,
}

/// Code counts of the last packed block, counted while packing. The tables
/// have a slot for every `u8` code, so the counting needs neither masks nor
/// bounds checks.
#[derive(Clone)]
struct SeqFreqs {
    ll: [u32; 256],
    ml: [u32; 256],
    of: [u32; 256],
    extra_bits: u64,
}

impl Default for SeqFreqs {
    fn default() -> Self {
        Self {
            ll: [0; 256],
            ml: [0; 256],
            of: [0; 256],
            extra_bits: 0,
        }
    }
}

/// Buffers and entropy tables kept across the blocks of a frame.
pub(crate) struct BlockEncodeWorkspace {
    /// Literals of the last packed block in `lit_buf[..lit_len]`. The buffer
    /// only grows, so packing a block does not clear it.
    pub lit_buf: Vec<u8>,
    lit_len: usize,
    /// Literals section header, then the Huffman streams when the literals
    /// are coded. Raw literals stay in `lit_buf`.
    pub lit_section: Vec<u8>,
    /// Sequences section with the predefined tables.
    pub pred_seq: Vec<u8>,
    /// Sequences section with tables built for the block.
    pub cust_seq: Vec<u8>,
    /// Sequences section in repeat mode, with `prev_ll`, `prev_of` and
    /// `prev_ml`.
    pub repeat_seq: Vec<u8>,
    /// Huffman literal streams; bytes past the encoded length are scratch.
    pub huf_concat: Vec<u8>,
    /// Sequences of the last packed block in `packed_seqs[..num_packed]`.
    packed_seqs: Vec<PackedSeq>,
    num_packed: usize,
    seq_freqs: SeqFreqs,
    custom_seq_tables: CustomSeqTables,
    /// Huffman tables. Slot `huffman_prev` holds the one the decoder has:
    /// the last one a compressed block sent, which raw literals and raw
    /// blocks leave in place, as in C zstd. The other slot holds a table
    /// built for the block being encoded, which takes over by index once
    /// that block is written compressed, so no table is copied.
    huffman: [Option<HuffmanEncodeTable>; 2],
    huffman_prev: usize,
    /// The dictionary's literal length table, for repeat mode.
    /// `encode_compressed_block` clears all three once it codes a block's
    /// sequences.
    pub prev_ll: Option<FseEncodeTable>,
    /// The dictionary's offset table; see `prev_ll`.
    pub prev_of: Option<FseEncodeTable>,
    /// The dictionary's match length table; see `prev_ll`.
    pub prev_ml: Option<FseEncodeTable>,
}

impl BlockEncodeWorkspace {
    /// Empty buffers, no previous tables.
    pub fn new() -> Self {
        Self {
            lit_buf: Vec::new(),
            lit_len: 0,
            lit_section: Vec::new(),
            pred_seq: Vec::new(),
            cust_seq: Vec::new(),
            repeat_seq: Vec::new(),
            huf_concat: Vec::new(),
            packed_seqs: Vec::new(),
            num_packed: 0,
            seq_freqs: SeqFreqs::default(),
            custom_seq_tables: CustomSeqTables::new(),
            huffman: [None, None],
            huffman_prev: 0,
            prev_ll: None,
            prev_of: None,
            prev_ml: None,
        }
    }

    /// The Huffman table the decoder holds.
    #[cfg(test)]
    fn prev_huffman(&self) -> Option<&HuffmanEncodeTable> {
        self.huffman[self.huffman_prev].as_ref()
    }

    /// Sets the Huffman table the decoder holds, as at a frame start.
    pub fn set_prev_huffman(&mut self, table: Option<HuffmanEncodeTable>) {
        self.huffman[self.huffman_prev] = table;
        self.huffman[1 - self.huffman_prev] = None;
    }
}

/// The decoder's Huffman table and the slot for a fresh one.
fn huffman_slots(
    huffman: &mut [Option<HuffmanEncodeTable>; 2],
    prev: usize,
) -> (Option<&HuffmanEncodeTable>, &mut Option<HuffmanEncodeTable>) {
    let [a, b] = huffman;
    if prev == 0 {
        (a.as_ref(), b)
    } else {
        (b.as_ref(), a)
    }
}

/// C zstd-style symbolTT entry. Two fields per symbol encode the
/// variable-width bit emission and state transition in one lookup.
/// An [`FseEncodeTable`] holds 64 of them and 512 states, about 1.5 KB.
#[derive(Clone, Copy)]
#[repr(C)]
struct SymbolTT {
    delta_nb_bits: u32,
    delta_find_state: i32,
}

/// FSE encoding table for literal length, offset or match length codes.
#[derive(Clone)]
pub(crate) struct FseEncodeTable {
    symbol_tt: [SymbolTT; MAX_SYMBOLS],
    /// `table_size + slot` for each decode table slot, grouped by symbol,
    /// as C zstd's `stateTable`.
    state_table: [u16; MAX_TABLE_SIZE],
    table_size: u32,
    table_log: u8,
    /// Bit `s` is set when symbol `s` has a state. Encoding any other symbol
    /// writes garbage, so repeat mode checks the block's codes first.
    present: u64,
}

impl Default for FseEncodeTable {
    fn default() -> Self {
        Self {
            symbol_tt: [SymbolTT {
                delta_nb_bits: 0,
                delta_find_state: 0,
            }; MAX_SYMBOLS],
            state_table: [0; MAX_TABLE_SIZE],
            table_size: 0,
            table_log: 0,
            present: 0,
        }
    }
}

/// Symbols with a nonzero count, as a bit set.
fn present_mask(count: &[u32; MAX_SYMBOLS]) -> u64 {
    count
        .iter()
        .enumerate()
        .fold(0, |mask, (s, &c)| mask | u64::from(c > 0) << s)
}

/// Symbol slots per table: 53 codes rounded up to a power of two, so the
/// encoders mask symbol indices instead of bounds-checking them.
const MAX_SYMBOLS: usize = 64;
const MAX_TABLE_SIZE: usize = 512;

impl FseEncodeTable {
    /// The encode table for a dictionary's decode table of
    /// 2^`accuracy_log` entries and codes up to `max_symbol`.
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

        // As C zstd's `FSE_buildCTable`: a symbol emits
        // `(state + delta_nb_bits) >> 16` bits, and its next state is
        // `state_table[(state >> bits) + delta_find_state]`.
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
            present: present_mask(&count),
        }
    }

    /// Whether every symbol with a nonzero count in `freqs` has a state.
    fn covers(&self, freqs: &[u32]) -> bool {
        freqs
            .iter()
            .enumerate()
            .all(|(s, &f)| f == 0 || (s < MAX_SYMBOLS && self.present >> s & 1 == 1))
    }

    /// Rebuilds the table from a normalized `distribution`, where -1 marks
    /// a low-probability symbol. Returns false, leaving the table as it
    /// was, if the distribution does not fill a table.
    #[allow(
        clippy::disallowed_methods,
        reason = "rebuilds a small FSE table in place"
    )]
    fn build_from_distribution_into(
        &mut self,
        distribution: &[i16],
        table_log: u8,
        max_symbol: usize,
    ) -> bool {
        let table_size = 1usize << table_log;
        let num_symbols = max_symbol + 1;
        debug_assert!(num_symbols <= MAX_SYMBOLS);
        debug_assert!(table_size <= MAX_TABLE_SIZE);

        let mut count = [0u32; MAX_SYMBOLS];
        for s in 0..num_symbols {
            let probability = distribution[s];
            count[s] = if probability == -1 {
                1
            } else if probability > 0 {
                probability as u32
            } else {
                0
            };
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

        // The format's symbol spread: -1 symbols take the last slots, the
        // rest step through the others by `step`.
        let step = (table_size >> 1) + (table_size >> 3) + 3;
        let mask = table_size - 1;
        let mut high_threshold = table_size - 1;
        let mut table_symbols = [0u8; MAX_TABLE_SIZE];

        for (s, &probability) in distribution.iter().enumerate().take(num_symbols) {
            if probability == -1 {
                if high_threshold == 0 {
                    return false;
                }
                table_symbols[high_threshold] = s as u8;
                high_threshold -= 1;
            }
        }

        let mut position = 0usize;
        // Without -1 symbols no slot is skipped, so four go at a time.
        if high_threshold == table_size - 1 {
            for (s, &probability) in distribution.iter().enumerate().take(num_symbols) {
                if probability <= 0 {
                    continue;
                }
                let symbol = s as u8;
                let mut remaining = probability as usize;
                while remaining >= 4 {
                    table_symbols[position] = symbol;
                    position = (position + step) & mask;
                    table_symbols[position] = symbol;
                    position = (position + step) & mask;
                    table_symbols[position] = symbol;
                    position = (position + step) & mask;
                    table_symbols[position] = symbol;
                    position = (position + step) & mask;
                    remaining -= 4;
                }
                for _ in 0..remaining {
                    table_symbols[position] = symbol;
                    position = (position + step) & mask;
                }
            }
        } else {
            for (s, &probability) in distribution.iter().enumerate().take(num_symbols) {
                if probability <= 0 {
                    continue;
                }
                for _ in 0..probability {
                    table_symbols[position] = s as u8;
                    position = (position + step) & mask;
                    while position > high_threshold {
                        position = (position + step) & mask;
                    }
                }
            }
        }

        if position != 0 {
            return false;
        }

        self.symbol_tt.fill(SymbolTT {
            delta_nb_bits: default_delta,
            delta_find_state: 0,
        });
        self.state_table[..table_size].fill(0);
        self.table_size = table_size as u32;
        self.table_log = table_log;
        self.present = present_mask(&count);

        self.symbol_tt[..num_symbols].copy_from_slice(&symbol_tt[..num_symbols]);

        let mut cumul_copy = cumul;
        for (i, &symbol) in table_symbols.iter().enumerate().take(table_size) {
            let s = symbol as usize;
            let idx = cumul_copy[s] as usize;
            self.state_table[idx] = (table_size + i) as u16;
            cumul_copy[s] += 1;
        }

        true
    }

    /// Encodes `symbol` from `state`. Returns the low state bits to emit,
    /// their count, and the next state. The masks keep indices in bounds
    /// without a check; valid tables never need them.
    #[inline(always)]
    fn transition(&self, symbol: u8, state: u32) -> (u32, u32, u32) {
        let tt = &self.symbol_tt[usize::from(symbol) & (MAX_SYMBOLS - 1)];
        let nb = tt.delta_nb_bits.wrapping_add(state) >> 16;
        let idx = ((state >> nb) as i32 + tt.delta_find_state) as usize;
        let next = u32::from(self.state_table[idx & (MAX_TABLE_SIZE - 1)]);
        (state & ((1u32 << nb) - 1), nb, next)
    }

    /// The starting state for `symbol`, the block's last code, as C zstd's
    /// `FSE_initCState2`.
    #[inline]
    fn init_state(&self, symbol: u8) -> u32 {
        let tt = &self.symbol_tt[symbol as usize];
        let nb_bits_out = tt.delta_nb_bits.wrapping_add(1 << 15) >> 16;
        let base_state = (nb_bits_out << 16).wrapping_sub(tt.delta_nb_bits);
        let idx = (base_state >> nb_bits_out) as i32 + tt.delta_find_state;
        debug_assert!(idx >= 0);
        self.state_table[idx as usize] as u32
    }
}

#[cfg(feature = "std")]
static PREDEFINED_TABLES: std::sync::LazyLock<PredefinedEncodeTables> =
    std::sync::LazyLock::new(PredefinedEncodeTables::build);

#[cfg(feature = "std")]
fn predefined_tables() -> &'static PredefinedEncodeTables {
    &PREDEFINED_TABLES
}

/// Without `std` there is no `LazyLock`, so each call builds the tables.
#[cfg(not(feature = "std"))]
fn predefined_tables() -> PredefinedEncodeTables {
    PredefinedEncodeTables::build()
}

/// Encode tables for the format's predefined distributions.
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

/// Writes `data` as a raw block. `last` marks the frame's last block.
pub fn encode_raw_block(
    data: &[u8],
    last: bool,
    output: &mut impl OutputSink,
) -> Result<(), CompressError> {
    let block_size = data.len() as u32;
    // Block_Header, 3 bytes: Last_Block in bit 0, Block_Type in bits 1-2
    // (0 raw, 2 compressed), Block_Size above.
    let header = (block_size << 3) | if last { 1 } else { 0 };
    output.push(header as u8)?;
    output.push((header >> 8) as u8)?;
    output.push((header >> 16) as u8)?;
    output.extend_from_slice(data)
}

/// Writes `src` as one block coded from `sequences`, or raw if that is not
/// smaller. The sequences cover `src` from its start; bytes past the last
/// one are literals. `rep_offsets` advances unless the block goes out raw.
pub(crate) fn encode_compressed_block(
    src: &[u8],
    sequences: &[Sequence],
    rep_offsets: &mut [u32; 3],
    last: bool,
    output: &mut impl OutputSink,
    workspace: &mut BlockEncodeWorkspace,
    policy: BlockPolicy,
) -> Result<(), CompressError> {
    simd_body!(encode_compressed_block_impl(
        src,
        sequences,
        rep_offsets,
        last,
        output,
        workspace,
        policy
    ))
}

#[inline(always)]
fn encode_compressed_block_impl(
    src: &[u8],
    sequences: &[Sequence],
    rep_offsets: &mut [u32; 3],
    last: bool,
    output: &mut impl OutputSink,
    workspace: &mut BlockEncodeWorkspace,
    policy: BlockPolicy,
) -> Result<(), CompressError> {
    let n = sequences.len();
    let total_match: usize = sequences.iter().map(|s| s.match_length as usize).sum();
    // Matches of 3 bytes on average do not pay for their sequences.
    if n == 0 || total_match <= n * 3 {
        return encode_literals_only_block(src, last, output, workspace, policy);
    }

    // Restored if the block goes out raw.
    let saved_rep = *rep_offsets;

    pack_sequences_and_literals::<true>(
        src,
        sequences,
        src.len() - total_match,
        rep_offsets,
        workspace,
    );

    let (prev_huffman, fresh_huffman) =
        huffman_slots(&mut workspace.huffman, workspace.huffman_prev);
    let raw_literals = encode_literals_section(
        &workspace.lit_buf[..workspace.lit_len],
        n,
        &mut workspace.lit_section,
        &mut workspace.huf_concat,
        prev_huffman,
        fresh_huffman,
        policy,
    );

    // Repeat mode reuses the dictionary's tables, which a literals-only block
    // leaves in place, so they must hold every code this block uses: a far
    // offset can need a code the dictionary never saw.
    let f = &workspace.seq_freqs;
    let has_repeat = match (&workspace.prev_ll, &workspace.prev_of, &workspace.prev_ml) {
        (Some(ll), Some(of), Some(ml)) => ll.covers(&f.ll) && of.covers(&f.of) && ml.covers(&f.ml),
        _ => false,
    };

    if has_repeat {
        encode_seq_repeat(
            &workspace.packed_seqs[..workspace.num_packed],
            workspace.prev_ll.as_ref().unwrap(),
            workspace.prev_of.as_ref().unwrap(),
            workspace.prev_ml.as_ref().unwrap(),
            &mut workspace.repeat_seq,
            workspace.seq_freqs.extra_bits,
        );
    }

    // As C zstd at its fast strategies: with valid repeat tables, up to 1000
    // sequences skip building custom ones, unless the repeat tables fit them
    // badly. Custom tables cost about the codes' order-0 entropy plus their
    // descriptions, so within 20% and 32 bytes of it they rarely pay off.
    let do_codes = policy.custom_tables
        && n >= 64
        && !(has_repeat
            && n < 1000
            && workspace.repeat_seq.len() * 5
                <= sequence_entropy_bytes(&workspace.seq_freqs, n) * 6 + 160);
    let seq_data = if do_codes {
        let f = &workspace.seq_freqs;
        let ll_freq = f.ll.first_chunk::<36>().unwrap();
        let ml_freq = f.ml.first_chunk::<53>().unwrap();
        let of_freq = f.of.first_chunk::<32>().unwrap();
        let pred_est = estimate_predefined_cost(ll_freq, ml_freq, of_freq, f.extra_bits, n);
        let use_custom = encode_seq_custom(
            &workspace.packed_seqs[..workspace.num_packed],
            ll_freq,
            ml_freq,
            of_freq,
            n,
            pred_est,
            &mut workspace.cust_seq,
            workspace.seq_freqs.extra_bits,
            &mut workspace.custom_seq_tables,
        );
        if use_custom && workspace.cust_seq.len() < pred_est {
            &workspace.cust_seq
        } else {
            encode_seq_predefined(
                &workspace.packed_seqs[..workspace.num_packed],
                &mut workspace.pred_seq,
                workspace.seq_freqs.extra_bits,
            );
            if use_custom && workspace.cust_seq.len() < workspace.pred_seq.len() {
                &workspace.cust_seq
            } else {
                &workspace.pred_seq
            }
        }
    } else {
        encode_seq_predefined(
            &workspace.packed_seqs[..workspace.num_packed],
            &mut workspace.pred_seq,
            workspace.seq_freqs.extra_bits,
        );
        &workspace.pred_seq
    };

    let seq_data = if has_repeat && workspace.repeat_seq.len() < seq_data.len() {
        &workspace.repeat_seq
    } else {
        seq_data
    };

    // Clear dict FSE tables after first block.
    workspace.prev_ll = None;
    workspace.prev_of = None;
    workspace.prev_ml = None;

    let literal_section_len =
        workspace.lit_section.len() + if raw_literals { workspace.lit_len } else { 0 };
    let block_len = literal_section_len + seq_data.len();
    if block_len >= src.len() {
        *rep_offsets = saved_rep;
        // The decoder never sees this block's table and keeps the previous one.
        workspace.huffman[1 - workspace.huffman_prev] = None;
        return encode_raw_block(src, last, output);
    }
    commit_fresh_huffman(&mut workspace.huffman, &mut workspace.huffman_prev);

    let block_size = block_len as u32;
    let header = (block_size << 3) | 0x04 | if last { 1 } else { 0 };
    output.push(header as u8)?;
    output.push((header >> 8) as u8)?;
    output.push((header >> 16) as u8)?;
    output.extend_from_slice(&workspace.lit_section)?;
    if raw_literals {
        output.extend_from_slice(&workspace.lit_buf[..workspace.lit_len])?;
    }
    output.extend_from_slice(seq_data)
}

/// Makes the table the block just written sent the decoder's previous one.
fn commit_fresh_huffman(huffman: &mut [Option<HuffmanEncodeTable>; 2], prev: &mut usize) {
    let fresh = 1 - *prev;
    if huffman[fresh].is_some() {
        huffman[*prev] = None;
        *prev = fresh;
    }
}

/// Encodes a block without sequences: Huffman-coded literals and an empty
/// sequence section. Falls back to a raw block when that is not smaller.
fn encode_literals_only_block(
    src: &[u8],
    last: bool,
    output: &mut impl OutputSink,
    workspace: &mut BlockEncodeWorkspace,
    policy: BlockPolicy,
) -> Result<(), CompressError> {
    let (prev_huffman, fresh_huffman) =
        huffman_slots(&mut workspace.huffman, workspace.huffman_prev);
    let raw_literals = encode_literals_section(
        src,
        0,
        &mut workspace.lit_section,
        &mut workspace.huf_concat,
        prev_huffman,
        fresh_huffman,
        policy,
    );
    // One byte for the empty sequence section (Number_of_Sequences = 0).
    let block_len = workspace.lit_section.len() + 1;
    if raw_literals || block_len >= src.len() {
        // The decoder never sees this block's table and keeps the previous one.
        workspace.huffman[1 - workspace.huffman_prev] = None;
        return encode_raw_block(src, last, output);
    }
    commit_fresh_huffman(&mut workspace.huffman, &mut workspace.huffman_prev);
    let header = ((block_len as u32) << 3) | 0x04 | u32::from(last);
    output.push(header as u8)?;
    output.push((header >> 8) as u8)?;
    output.push((header >> 16) as u8)?;
    output.extend_from_slice(&workspace.lit_section)?;
    output.push(0)
}

/// Encoding choices for a raw-literal block that is almost all literals:
/// Huffman literals for runs over 32 KiB below 6.09 bits per byte, with the
/// predefined sequence tables. The gate sits below the other negative
/// levels' 6.25, so L-7 compresses such data a little, less than L-6.
const LITERAL_HEAVY_POLICY: BlockPolicy = BlockPolicy {
    custom_tables: false,
    max_literal_entropy_fp8: 6 * 256 + 24,
};

/// [`encode_compressed_block`] for parameters that keep literals raw: raw
/// literals and the predefined sequence tables. Blocks over 32 KiB that
/// are almost all literals may take Huffman literals.
pub(crate) fn encode_compressed_block_raw(
    src: &[u8],
    sequences: &[Sequence],
    rep_offsets: &mut [u32; 3],
    last: bool,
    output: &mut impl OutputSink,
    workspace: &mut BlockEncodeWorkspace,
) -> Result<(), CompressError> {
    simd_body!(encode_compressed_block_raw_impl(
        src,
        sequences,
        rep_offsets,
        last,
        output,
        workspace
    ))
}

#[inline(always)]
fn encode_compressed_block_raw_impl(
    src: &[u8],
    sequences: &[Sequence],
    rep_offsets: &mut [u32; 3],
    last: bool,
    output: &mut impl OutputSink,
    workspace: &mut BlockEncodeWorkspace,
) -> Result<(), CompressError> {
    let n = sequences.len();
    let total_match: usize = sequences.iter().map(|s| s.match_length as usize).sum();
    // Matches covering under 1/8 of the block leave raw literals almost no
    // compression, so such blocks try Huffman literals. Only runs over
    // 32 KiB can take them, and a sample of the block, which is nearly all
    // literals, rules out dense ones before the literals are gathered.
    if total_match < src.len() / 8
        && src.len() > 32 * 1024
        && !histogram_too_dense(
            &byte_histogram(&src[..DENSITY_SAMPLE]),
            LITERAL_HEAVY_POLICY.max_literal_entropy_fp8,
        )
    {
        return encode_compressed_block_impl(
            src,
            sequences,
            rep_offsets,
            last,
            output,
            workspace,
            LITERAL_HEAVY_POLICY,
        );
    }
    if total_match <= n * 3 {
        return encode_raw_block(src, last, output);
    }

    let saved_rep = *rep_offsets;

    pack_sequences_and_literals::<false>(
        src,
        sequences,
        src.len() - total_match,
        rep_offsets,
        workspace,
    );

    workspace.lit_section.clear();
    encode_raw_literals_header(workspace.lit_len, &mut workspace.lit_section);

    encode_seq_predefined(
        &workspace.packed_seqs[..workspace.num_packed],
        &mut workspace.pred_seq,
        workspace.seq_freqs.extra_bits,
    );

    let block_len = workspace.lit_section.len() + workspace.lit_len + workspace.pred_seq.len();
    if block_len >= src.len() {
        *rep_offsets = saved_rep;
        return encode_raw_block(src, last, output);
    }

    // The decoder now holds the predefined tables, so later blocks must not
    // repeat the dictionary's.
    workspace.prev_ll = None;
    workspace.prev_of = None;
    workspace.prev_ml = None;

    let block_size = block_len as u32;
    let header = (block_size << 3) | 0x04 | if last { 1 } else { 0 };
    output.push(header as u8)?;
    output.push((header >> 8) as u8)?;
    output.push((header >> 16) as u8)?;
    output.extend_from_slice(&workspace.lit_section)?;
    output.extend_from_slice(&workspace.lit_buf[..workspace.lit_len])?;
    output.extend_from_slice(&workspace.pred_seq)
}

/// Packs `sequences` for the entropy coders and gathers their `lit_total`
/// literals. With `COUNT`, also counts the codes for custom tables.
fn pack_sequences_and_literals<const COUNT: bool>(
    src: &[u8],
    sequences: &[Sequence],
    lit_total: usize,
    rep_offsets: &mut [u32; 3],
    workspace: &mut BlockEncodeWorkspace,
) {
    simd_body!(pack_sequences_and_literals_impl::<COUNT>(
        src,
        sequences,
        lit_total,
        rep_offsets,
        workspace
    ));
}

#[inline(always)]
fn pack_sequences_and_literals_impl<const COUNT: bool>(
    src: &[u8],
    sequences: &[Sequence],
    lit_total: usize,
    rep_offsets: &mut [u32; 3],
    workspace: &mut BlockEncodeWorkspace,
) {
    let n = sequences.len();
    let src_len = src.len();

    // Sized to the literals, not the block, so a fresh buffer only zeroes
    // what the literals use.
    if workspace.lit_buf.len() < lit_total + 32 {
        workspace.lit_buf.resize(lit_total + 32, 0);
    }
    if workspace.packed_seqs.len() < n {
        workspace.packed_seqs.resize(n, PackedSeq::default());
    }
    // The exact length lets the compiler check literal stores at positions
    // clamped to `lit_total` against nothing.
    let lit_buf = &mut workspace.lit_buf[..lit_total + 32];
    let freqs = &mut workspace.seq_freqs;
    if COUNT {
        freqs.ll = [0; 256];
        freqs.ml = [0; 256];
        freqs.of = [0; 256];
    }
    let mut extra_bits_total = 0u64;
    let mut lit_pos = 0usize;
    let mut lit_offset = 0usize;

    let mut rep0 = rep_offsets[0];
    let mut rep1 = rep_offsets[1];
    let mut rep2 = rep_offsets[2];

    for (seq, packed) in sequences.iter().zip(&mut workspace.packed_seqs[..n]) {
        let ll = seq.literal_length as usize;
        debug_assert!(lit_offset + ll <= src_len);
        // Short runs copy a fixed 32 bytes; `lit_buf` has 32 bytes of slack.
        if ll <= 32 && lit_offset + 32 <= src_len {
            let s32: &[u8; 32] = src[lit_offset..].first_chunk().unwrap();
            let d32: &mut [u8; 32] = lit_buf[lit_pos.min(lit_total)..].first_chunk_mut().unwrap();
            *d32 = *s32;
        } else {
            lit_buf[lit_pos..lit_pos + ll].copy_from_slice(&src[lit_offset..lit_offset + ll]);
        }
        lit_pos += ll;
        lit_offset += ll + seq.match_length as usize;

        // Repeat offsets for codes 1 to 3. Without literals, code 1 means the
        // second offset and code 3 the first offset minus one. Selects instead
        // of branches: rep and new offsets interleave unpredictably.
        let off = seq.offset;
        let no_lits = seq.literal_length == 0;
        let (c1, c2, c3) = if no_lits {
            (rep1, rep2, rep0 - 1)
        } else {
            (rep0, rep1, rep2)
        };
        let mut ov = if off == c3 { 3 } else { off + 3 };
        if off == c2 {
            ov = 2;
        }
        if off == c1 {
            ov = 1;
        }
        // Code 1 with literals keeps the history; the code naming the second
        // offset swaps the first two; every other code shifts in `off`.
        let keep = !no_lits && ov == 1;
        let swap = ov == 2 - u32::from(no_lits);
        if !(keep || swap) {
            rep2 = rep1;
        }
        if !keep {
            rep1 = rep0;
        }
        rep0 = off;

        let (ll_c, ll_nb, ll_extra) = ll_split(seq.literal_length);
        let (ml_c, ml_nb, ml_extra) = ml_split(seq.match_length);
        let of_c = of_code(ov);
        let of_extra = ov - (1u32 << of_c);
        let extra_bits = (ll_extra as u64)
            | ((ml_extra as u64) << ll_nb)
            | ((of_extra as u64) << (ll_nb + ml_nb));
        let extra_nbits = (ll_nb + ml_nb) as u8 + of_c;

        if COUNT {
            freqs.ll[usize::from(ll_c)] += 1;
            freqs.ml[usize::from(ml_c)] += 1;
            freqs.of[usize::from(of_c)] += 1;
        }
        extra_bits_total += u64::from(extra_nbits);

        *packed = PackedSeq {
            extra_bits,
            ll_c,
            ml_c,
            of_c,
            extra_nbits,
        };
    }
    rep_offsets[0] = rep0;
    rep_offsets[1] = rep1;
    rep_offsets[2] = rep2;

    if lit_offset < src_len {
        let rest = src_len - lit_offset;
        lit_buf[lit_pos..lit_pos + rest].copy_from_slice(&src[lit_offset..src_len]);
        lit_pos += rest;
    }
    debug_assert_eq!(lit_pos, lit_total);
    freqs.extra_bits = extra_bits_total;
    workspace.lit_len = lit_pos;
    workspace.num_packed = n;
}

/// Approximate log2(x) in 8.8 fixed point (result = log2(x) * 256).
pub(crate) fn log2_fp8(x: u32) -> u32 {
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

/// Whether a fresh Huffman table is worth trying for literals whose policy
/// does not ask for one: above 32 KiB.
fn huf_worth_trying(data: &[u8]) -> bool {
    data.len() > 32 * 1024
}

/// Literals the entropy gate samples. Shorter literal runs are measured
/// from their full histogram.
const DENSITY_SAMPLE: usize = 4096;

/// Whether bytes with these counts have an order-0 entropy above
/// `limit_fp8` bits per byte (8.8 fixed point).
fn histogram_too_dense(counts: &[u32; 256], limit_fp8: u32) -> bool {
    let n: u32 = counts.iter().sum();
    if n == 0 {
        return false;
    }
    entropy_bits_fp8(counts, n) > u64::from(limit_fp8) * u64::from(n)
}

/// Order-0 entropy of symbols with these counts, summing to `n`, in bits
/// (8.8 fixed point).
fn entropy_bits_fp8(counts: &[u32], n: u32) -> u64 {
    let log2_n = log2_fp8(n);
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| u64::from(c) * u64::from(log2_n - log2_fp8(c)))
        .sum()
}

/// The order-0 entropy of the `n` sequences' codes plus their extra bits, in
/// bytes: about the least custom tables can code them in.
fn sequence_entropy_bytes(f: &SeqFreqs, n: usize) -> usize {
    let n = n as u32;
    let code_bits = (entropy_bits_fp8(&f.ll[..36], n)
        + entropy_bits_fp8(&f.ml[..53], n)
        + entropy_bits_fp8(&f.of[..32], n))
        / 256;
    (code_bits + f.extra_bits).div_ceil(8) as usize
}

/// Literals sampled from each end before the full histogram when they look
/// incompressible.
const SUSPECT_SAMPLE: usize = 4096;

/// Literals above this order-0 entropy (bits per byte, 8.8 fixed point) stay
/// raw: Huffman would save under 0.4%, less than its table costs. Random bytes
/// measure about 8.02 with the `log2_fp8` estimate.
const FLAT_LITERALS_FP8: u32 = 8 * 256 - 8;

/// Writes the literals section for `lits` to `output`: its header, and the
/// Huffman streams when they win. Returns true if the literals stay raw;
/// the caller then appends them. A table built here goes to
/// `fresh_huffman`.
fn encode_literals_section(
    lits: &[u8],
    num_seq: usize,
    output: &mut Vec<u8>,
    huf_concat: &mut Vec<u8>,
    prev_huffman: Option<&HuffmanEncodeTable>,
    fresh_huffman: &mut Option<HuffmanEncodeTable>,
    policy: BlockPolicy,
) -> bool {
    output.clear();

    // As C zstd's `ZSTD_minLiteralsToCompress` at its fast strategies, a
    // fresh table never repays its description below 64 literals, so only
    // the previous table is tried there, from 6 literals. Unlike C, it is
    // tried even when it may lack codes; the estimate skips it then. Keeping
    // it over a table built for a few literals also serves later blocks.
    let min_literals = if prev_huffman.is_some() { 6 } else { 64 };
    if lits.len() < min_literals {
        encode_raw_literals_header(lits.len(), output);
        return true;
    }

    let try_huffman_literals = policy.custom_tables;

    // Like C zstd: from 256 literals, four streams decode in parallel and
    // repay their 6-byte jump table.
    let use_4_streams = lits.len() >= 256;
    let encode = |table: &HuffmanEncodeTable, concat: &mut Vec<u8>| {
        if use_4_streams {
            table.encode_4_streams_into(lits, concat)
        } else {
            table.encode_single_stream_into(lits, concat)
        }
    };

    // Pick between reusing the previous table, a fresh table with its
    // description, and raw bytes by estimated size, as C zstd does. Reuse is
    // only a win when the old table still fits these literals.
    let want_fresh = lits.len() >= 64 && (try_huffman_literals || huf_worth_trying(lits));
    if prev_huffman.is_none() && !want_fresh {
        encode_raw_literals_header(lits.len(), output);
        return true;
    }
    // Dense literals stay raw. Long runs are judged by a sample, so they
    // skip the full histogram when they fail.
    let limit = policy.max_literal_entropy_fp8;
    let sample = (limit != u32::MAX && lits.len() > DENSITY_SAMPLE)
        .then(|| byte_histogram(&lits[..DENSITY_SAMPLE]));
    if sample.is_some_and(|sample| histogram_too_dense(&sample, limit)) {
        encode_raw_literals_header(lits.len(), output);
        return true;
    }
    // Few sequences per literal suggest incompressible bytes. As in C zstd,
    // two small samples then rule out Huffman before the full histogram.
    let suspect = num_seq == 0 || lits.len() / num_seq >= 20;
    if suspect && lits.len() >= 2 * SUSPECT_SAMPLE {
        let head = byte_histogram(&lits[..SUSPECT_SAMPLE]);
        let tail = byte_histogram(&lits[lits.len() - SUSPECT_SAMPLE..]);
        let sample: [u32; 256] = core::array::from_fn(|i| head[i] + tail[i]);
        if histogram_too_dense(&sample, FLAT_LITERALS_FP8) {
            encode_raw_literals_header(lits.len(), output);
            return true;
        }
    }
    // The density sample already counted the head.
    let freqs = match sample {
        Some(head) => {
            let rest = byte_histogram(&lits[DENSITY_SAMPLE..]);
            core::array::from_fn(|i| head[i] + rest[i])
        }
        None => byte_histogram(lits),
    };
    if histogram_too_dense(&freqs, FLAT_LITERALS_FP8) {
        encode_raw_literals_header(lits.len(), output);
        return true;
    }
    if limit != u32::MAX && lits.len() <= DENSITY_SAMPLE && histogram_too_dense(&freqs, limit) {
        encode_raw_literals_header(lits.len(), output);
        return true;
    }
    let reuse_bits = prev_huffman.and_then(|p| p.estimate_bits(&freqs));
    // As C zstd at its fast strategies: short literals keep a table that can
    // encode them. A fresh one rarely repays its description there, and
    // building it costs more than encoding them.
    let prefer_reuse = reuse_bits.is_some() && lits.len() <= 1024;
    let fresh = if want_fresh && !prefer_reuse {
        HuffmanEncodeTable::from_histogram(&freqs).map(|t| {
            let bits = t.estimate_bits(&freqs).unwrap_or(u64::MAX)
                + t.weights_description().len() as u64 * 8;
            (t, bits)
        })
    } else {
        None
    };
    let fresh_bits = fresh.as_ref().map_or(u64::MAX, |f| f.1);
    // The estimates are lower bounds: when neither beats raw bytes, skip the
    // encode.
    if reuse_bits.unwrap_or(u64::MAX).min(fresh_bits) / 8 >= lits.len() as u64 {
        encode_raw_literals_header(lits.len(), output);
        return true;
    }

    // Huffman pays only when it beats the raw section, headers included:
    // tiny sections need the longer Huffman header.
    let raw_size = raw_literals_header_len(lits.len()) + lits.len();
    if let Some(bits) = reuse_bits
        && bits <= fresh_bits
    {
        let len = encode(prev_huffman.unwrap(), huf_concat);
        if huf_literals_header_len(lits.len(), len) + len < raw_size {
            encode_treeless_literals_header(lits.len(), len, use_4_streams, output);
            output.extend_from_slice(&huf_concat[..len]);
            return false;
        }
    } else if let Some((table, _)) = fresh {
        let len = encode(&table, huf_concat);
        let tree_desc = table.weights_description();
        let compressed_size = tree_desc.len() + len;
        if huf_literals_header_len(lits.len(), compressed_size) + compressed_size < raw_size {
            encode_compressed_literals_header(lits.len(), compressed_size, use_4_streams, output);
            output.extend_from_slice(tree_desc);
            output.extend_from_slice(&huf_concat[..len]);
            *fresh_huffman = Some(table);
            return false;
        }
    }

    encode_raw_literals_header(lits.len(), output);
    true
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

/// Writes a Huffman literals header of 3 to 5 bytes. Size_Format 0 means
/// one stream; 1 to 3 mean four streams with 10-, 14- or 18-bit sizes.
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

/// Length of the header [`encode_raw_literals_header`] writes.
fn raw_literals_header_len(size: usize) -> usize {
    match size {
        0..=31 => 1,
        32..=4095 => 2,
        _ => 3,
    }
}

/// Length of the header [`encode_huf_literals_header`] writes.
fn huf_literals_header_len(regen_size: usize, comp_size: usize) -> usize {
    match regen_size.max(comp_size) {
        0..=1023 => 3,
        1024..=16383 => 4,
        _ => 5,
    }
}

/// Writes a raw literals header of 1 to 3 bytes.
fn encode_raw_literals_header(size: usize, output: &mut Vec<u8>) {
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
}

/// Writes the sequences section of `packed` with the predefined tables.
fn encode_seq_predefined(packed: &[PackedSeq], output: &mut Vec<u8>, extra_bits: u64) {
    simd_body!(encode_seq_predefined_impl(packed, output, extra_bits));
}

/// Bytes a sequence bitstream can take, so its buffer is sized once: each
/// sequence adds its extra bits and at most one table log of state bits per
/// code, the final states one log each, and the end mark one bit. The last
/// 8-byte store reaches 8 bytes past the end.
fn seq_stream_bound(n: usize, extra_bits: u64, state_bits: u32) -> usize {
    let bits = extra_bits + (n as u64 + 1) * u64::from(state_bits) + 1;
    bits.div_ceil(8) as usize + 8
}

#[inline(always)]
fn encode_seq_predefined_impl(packed: &[PackedSeq], output: &mut Vec<u8>, extra_bits: u64) {
    let n = packed.len();
    let num_seq = n as u32;
    output.clear();

    write_seq_count(output, num_seq);

    // Symbol compression modes: predefined for all three.
    output.push(0x00);

    let tables = predefined_tables();
    let last = &packed[n - 1];

    let state_bits = u32::from(LL_DEFAULT_ACCURACY + ML_DEFAULT_ACCURACY + OF_DEFAULT_ACCURACY);
    let max_out = seq_stream_bound(n, extra_bits, state_bits);
    let mut bitstream = primitives::BitstreamScratch::new(output, max_out);

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
            bitstream.flush(pos, bits);
            let nb = (bits_used >> 3) as usize;
            pos += nb;
            bits >>= (nb << 3) as u64;
            bits_used &= 7;
        };
    }
    add_extra_bits_macro!(add_extra_bits, add_bits, flush_fast);
    macro_rules! encode_transition {
        ($table:expr, $symbol:expr, $state:expr) => {{
            let (out, nb, next) = $table.transition($symbol, $state);
            add_bits!(out, nb);
            $state = next;
        }};
    }

    // The decoder reads the stream from its end, so sequences go in
    // backward. The last one only seeds the states and adds its extra bits.
    add_bits!(last.extra_bits, last.extra_nbits);

    for k in (0..n - 1).rev() {
        let p = &packed[k];

        flush_fast!();
        encode_transition!(tables.of, p.of_c, of_state);
        encode_transition!(tables.ml, p.ml_c, ml_state);
        encode_transition!(tables.ll, p.ll_c, ll_state);
        add_extra_bits!(p);
    }

    flush_fast!();
    // Final states, then the end mark.
    let ll_ts = tables.ll.table_size;
    let of_ts = tables.of.table_size;
    let ml_ts = tables.ml.table_size;
    add_bits!(ml_state - ml_ts, ML_DEFAULT_ACCURACY);
    add_bits!(of_state - of_ts, OF_DEFAULT_ACCURACY);
    add_bits!(ll_state - ll_ts, LL_DEFAULT_ACCURACY);

    add_bits!(1u32, 1u8);
    flush_fast!();
    while bits_used > 0 {
        bitstream.write_byte(pos, bits as u8);
        bits >>= 8;
        bits_used = bits_used.saturating_sub(8);
        pos += 1;
    }
    bitstream.finish(pos);
}

/// Writes the sequences section of `packed` in repeat mode with the given
/// tables.
fn encode_seq_repeat(
    packed: &[PackedSeq],
    ll_t: &FseEncodeTable,
    of_t: &FseEncodeTable,
    ml_t: &FseEncodeTable,
    output: &mut Vec<u8>,
    extra_bits: u64,
) {
    simd_body!(encode_seq_repeat_impl(
        packed, ll_t, of_t, ml_t, output, extra_bits
    ));
}

#[inline(always)]
fn encode_seq_repeat_impl(
    packed: &[PackedSeq],
    ll_t: &FseEncodeTable,
    of_t: &FseEncodeTable,
    ml_t: &FseEncodeTable,
    output: &mut Vec<u8>,
    extra_bits: u64,
) {
    let n = packed.len();
    let num_seq = n as u32;
    output.clear();

    write_seq_count(output, num_seq);

    // Mode byte: repeat (3) for all three streams
    output.push((3 << 6) | (3 << 4) | (3 << 2));

    let last = &packed[n - 1];

    let state_bits = u32::from(ll_t.table_log + of_t.table_log + ml_t.table_log);
    let max_out = seq_stream_bound(n, extra_bits, state_bits);
    let mut bitstream = primitives::BitstreamScratch::new(output, max_out);

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
            bitstream.flush(pos, bits);
            let nb = (bits_used >> 3) as usize;
            pos += nb;
            bits >>= (nb << 3) as u64;
            bits_used &= 7;
        };
    }
    add_extra_bits_macro!(add_extra_bits, add_bits, flush_fast);

    add_bits!(last.extra_bits, last.extra_nbits);

    for k in (0..n - 1).rev() {
        let p = &packed[k];

        flush_fast!();
        {
            let (out, nb, next) = of_t.transition(p.of_c, of_s);
            add_bits!(out, nb);
            of_s = next;
        }
        {
            let (out, nb, next) = ml_t.transition(p.ml_c, ml_s);
            add_bits!(out, nb);
            ml_s = next;
        }
        {
            let (out, nb, next) = ll_t.transition(p.ll_c, ll_s);
            add_bits!(out, nb);
            ll_s = next;
        }
        add_extra_bits!(p);
    }

    flush_fast!();
    add_bits!(ml_s - ml_t.table_size, ml_t.table_log);
    add_bits!(of_s - of_t.table_size, of_t.table_log);
    add_bits!(ll_s - ll_t.table_size, ll_t.table_log);

    add_bits!(1u32, 1u8);
    flush_fast!();
    while bits_used > 0 {
        bitstream.write_byte(pos, bits as u8);
        bits >>= 8;
        bits_used = bits_used.saturating_sub(8);
        pos += 1;
    }
    bitstream.finish(pos);
}

/// How a custom table codes one code type: FSE, or RLE of one code.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TableMode {
    Compressed,
    Rle(u8),
}

/// A custom table for one code type, rebuilt per block.
struct TableEnc {
    mode: TableMode,
    enc_table: FseEncodeTable,
    /// The serialized table description.
    header: Vec<u8>,
    /// The normalized distribution.
    dist: Vec<i16>,
}

impl TableEnc {
    fn new() -> Self {
        Self {
            mode: TableMode::Rle(0),
            enc_table: FseEncodeTable::default(),
            header: Vec::new(),
            dist: Vec::new(),
        }
    }
}

struct CustomSeqTables {
    ll: TableEnc,
    of: TableEnc,
    ml: TableEnc,
}

impl CustomSeqTables {
    fn new() -> Self {
        Self {
            ll: TableEnc::new(),
            of: TableEnc::new(),
            ml: TableEnc::new(),
        }
    }
}

/// Scales `freqs` to a distribution that sums to 2^`accuracy_log`. Codes
/// that would round to 0 get -1, and the most frequent code takes the
/// remainder. Simpler than C zstd's `FSE_normalizeCount`.
fn normalize_counts_into(freqs: &[u32], accuracy_log: u8, dist: &mut Vec<i16>) {
    let table_size = 1i32 << accuracy_log;
    let total: u64 = freqs.iter().map(|&f| f as u64).sum();

    dist.resize(freqs.len(), 0);
    // Measured faster on tiny inputs than clearing, then resizing.
    #[allow(clippy::disallowed_methods, reason = "a few dozen entries per table")]
    dist.fill(0);

    if total == 0 {
        return;
    }

    let mut largest_idx = 0;
    let mut largest_freq = 0u32;
    for (i, &freq) in freqs.iter().enumerate() {
        if freq > largest_freq {
            largest_freq = freq;
            largest_idx = i;
        }
    }

    let budget = table_size - 1;
    let mut distributed = 0i32;

    for (i, &freq) in freqs.iter().enumerate() {
        if freq == 0 || i == largest_idx {
            continue;
        }
        if distributed >= budget {
            break;
        }
        let prob = ((freq as u64) * (table_size as u64) / total) as i32;
        if prob < 1 {
            dist[i] = -1;
            distributed += 1;
        } else {
            let capped = prob.min(budget - distributed);
            dist[i] = capped as i16;
            distributed += capped;
        }
    }

    dist[largest_idx] = (table_size - distributed) as i16;
}

/// Writes the format's FSE table description of `distribution` to `out`.
fn serialize_fse_table_description_into(distribution: &[i16], accuracy_log: u8, out: &mut Vec<u8>) {
    let buf = core::mem::take(out);
    let mut writer = BitWriter::from_vec(buf);
    writer.write_bits((accuracy_log - 5) as u32, 4);

    let table_size = 1i32 << accuracy_log;
    let mut remaining = table_size + 1;
    let mut threshold = table_size;
    let mut nb_bits = accuracy_log + 1;

    let mut i = 0;
    while i < distribution.len() && remaining > 1 {
        let prob = distribution[i];
        let count = (prob + 1) as i32;
        let max_val = (2 * threshold - 1) - remaining;

        if count < max_val {
            writer.write_bits(count as u32, nb_bits - 1);
        } else if count < threshold {
            writer.write_bits(count as u32, nb_bits);
        } else {
            writer.write_bits((count + max_val) as u32, nb_bits);
        }

        if prob == -1 {
            remaining -= 1;
        } else if prob > 0 {
            remaining -= prob as i32;
        }

        if prob == 0 {
            let mut zeros = 0;
            let start = i + 1;
            while start + zeros < distribution.len() && distribution[start + zeros] == 0 {
                zeros += 1;
            }
            let mut z = zeros;
            loop {
                if z >= 3 {
                    writer.write_bits(3, 2);
                    z -= 3;
                } else {
                    writer.write_bits(z as u32, 2);
                    break;
                }
            }
            i += 1 + zeros;
        } else {
            i += 1;
        }

        while remaining < threshold {
            nb_bits -= 1;
            threshold >>= 1;
        }
    }

    *out = writer.into_bytes();
}

/// Builds `out` for `n` codes up to `max_sym`: RLE for a single distinct
/// code, else an FSE table of at most `max_log`. Returns false if no valid
/// table fits.
fn build_table_enc(
    out: &mut TableEnc,
    freqs: &[u32],
    max_sym: usize,
    max_log: u8,
    n: usize,
) -> bool {
    let mut num_distinct = 0usize;
    let mut rle_sym = 0usize;
    for (s, &freq) in freqs.iter().enumerate().take(max_sym + 1) {
        if freq > 0 {
            num_distinct += 1;
            rle_sym = s;
        }
    }

    out.header.clear();
    if num_distinct <= 1 {
        out.mode = TableMode::Rle(rle_sym as u8);
        return true;
    }
    let min_log_for_sym = (32 - ((num_distinct as u32 + 1).leading_zeros())) as u8;
    let acc = optimal_table_log(max_log, n, max_sym).max(min_log_for_sym);
    if acc > max_log {
        return false;
    }
    normalize_counts_into(&freqs[..=max_sym], acc, &mut out.dist);
    let dist = &out.dist;
    for s in 0..=max_sym {
        if freqs[s] > 0 && dist[s] == 0 {
            return false;
        }
    }
    serialize_fse_table_description_into(dist, acc, &mut out.header);
    if !out
        .enc_table
        .build_from_distribution_into(dist, acc, max_sym)
    {
        return false;
    }
    out.mode = TableMode::Compressed;
    true
}

/// Writes the sequences section of `packed` with tables built for it.
/// Returns false, with nothing usable in `output`, if the estimate does not
/// beat `pred_size` or a table cannot be built.
#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn encode_seq_custom(
    packed: &[PackedSeq],
    ll_freq: &[u32; 36],
    ml_freq: &[u32; 53],
    of_freq: &[u32; 32],
    n: usize,
    pred_size: usize,
    output: &mut Vec<u8>,
    extra_bits: u64,
    tables: &mut CustomSeqTables,
) -> bool {
    simd_body!(encode_seq_custom_impl(
        packed, ll_freq, ml_freq, of_freq, n, pred_size, output, extra_bits, tables
    ))
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn encode_seq_custom_impl(
    packed: &[PackedSeq],
    ll_freq: &[u32; 36],
    ml_freq: &[u32; 53],
    of_freq: &[u32; 32],
    n: usize,
    pred_size: usize,
    output: &mut Vec<u8>,
    extra_bits: u64,
    tables: &mut CustomSeqTables,
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

    // The format's largest accuracy logs: 9 for literal and match
    // lengths, 8 for offsets.
    if !build_table_enc(&mut tables.ll, ll_freq, ll_max, 9, n) {
        return false;
    }
    if !build_table_enc(&mut tables.of, of_freq, of_max, 8, n) {
        return false;
    }
    if !build_table_enc(&mut tables.ml, ml_freq, ml_max, 9, n) {
        return false;
    }

    // Symbol compression modes: 1 RLE, 2 FSE compressed.
    let ll_mode: u8 = match tables.ll.mode {
        TableMode::Compressed => 2,
        TableMode::Rle(_) => 1,
    };
    let of_mode: u8 = match tables.of.mode {
        TableMode::Compressed => 2,
        TableMode::Rle(_) => 1,
    };
    let ml_mode: u8 = match tables.ml.mode {
        TableMode::Compressed => 2,
        TableMode::Rle(_) => 1,
    };

    let num_seq = n as u32;
    write_seq_count(output, num_seq);

    output.push((ll_mode << 6) | (of_mode << 4) | (ml_mode << 2));

    match tables.ll.mode {
        TableMode::Rle(sym) => output.push(sym),
        TableMode::Compressed => output.extend_from_slice(&tables.ll.header),
    }
    match tables.of.mode {
        TableMode::Rle(sym) => output.push(sym),
        TableMode::Compressed => output.extend_from_slice(&tables.of.header),
    }
    match tables.ml.mode {
        TableMode::Rle(sym) => output.push(sym),
        TableMode::Compressed => output.extend_from_slice(&tables.ml.header),
    }

    let last = &packed[n - 1];

    let log = |t: &TableEnc| match t.mode {
        TableMode::Compressed => u32::from(t.enc_table.table_log),
        TableMode::Rle(_) => 0,
    };
    let state_bits = log(&tables.ll) + log(&tables.of) + log(&tables.ml);
    let max_out = seq_stream_bound(n, extra_bits, state_bits);
    let mut bitstream = primitives::BitstreamScratch::new(output, max_out);
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
            bitstream.flush(pos, bits);
            let nb = (bits_used >> 3) as usize;
            pos += nb;
            bits >>= (nb << 3) as u64;
            bits_used &= 7;
        };
    }
    add_extra_bits_macro!(add_extra_bits, add_bits, flush_fast);

    add_bits!(last.extra_bits, last.extra_nbits);

    match (tables.ll.mode, tables.of.mode, tables.ml.mode) {
        (TableMode::Compressed, TableMode::Compressed, TableMode::Compressed) => {
            let ll_t = &tables.ll.enc_table;
            let of_t = &tables.of.enc_table;
            let ml_t = &tables.ml.enc_table;
            let mut ll_s = ll_t.init_state(last.ll_c);
            let mut of_s = of_t.init_state(last.of_c);
            let mut ml_s = ml_t.init_state(last.ml_c);

            for k in (0..n - 1).rev() {
                let p = &packed[k];

                flush_fast!();
                {
                    let (out, nb, next) = of_t.transition(p.of_c, of_s);
                    add_bits!(out, nb);
                    of_s = next;
                }
                {
                    let (out, nb, next) = ml_t.transition(p.ml_c, ml_s);
                    add_bits!(out, nb);
                    ml_s = next;
                }
                {
                    let (out, nb, next) = ll_t.transition(p.ll_c, ll_s);
                    add_bits!(out, nb);
                    ll_s = next;
                }
                add_extra_bits!(p);
            }

            flush_fast!();
            add_bits!(ml_s - ml_t.table_size, ml_t.table_log);
            add_bits!(of_s - of_t.table_size, of_t.table_log);
            add_bits!(ll_s - ll_t.table_size, ll_t.table_log);
        }
        _ => {
            let ll_table = match tables.ll.mode {
                TableMode::Compressed => Some(&tables.ll.enc_table),
                TableMode::Rle(_) => None,
            };
            let of_table = match tables.of.mode {
                TableMode::Compressed => Some(&tables.of.enc_table),
                TableMode::Rle(_) => None,
            };
            let ml_table = match tables.ml.mode {
                TableMode::Compressed => Some(&tables.ml.enc_table),
                TableMode::Rle(_) => None,
            };

            let mut ll_state = ll_table.as_ref().map(|t| t.init_state(last.ll_c));
            let mut of_state = of_table.as_ref().map(|t| t.init_state(last.of_c));
            let mut ml_state = ml_table.as_ref().map(|t| t.init_state(last.ml_c));

            macro_rules! encode_transition_opt {
                ($table:expr, $state:expr, $symbol:expr) => {
                    if let (Some(t), Some(s)) = (&$table, &mut $state) {
                        let (out, nb, next) = t.transition($symbol, *s);
                        add_bits!(out, nb);
                        *s = next;
                    }
                };
            }

            for k in (0..n - 1).rev() {
                let p = &packed[k];

                flush_fast!();
                encode_transition_opt!(of_table, of_state, p.of_c);
                encode_transition_opt!(ml_table, ml_state, p.ml_c);
                encode_transition_opt!(ll_table, ll_state, p.ll_c);
                add_extra_bits!(p);
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
        bitstream.write_byte(pos, bits as u8);
        bits >>= 8;
        bits_used = bits_used.saturating_sub(8);
        pos += 1;
    }
    bitstream.finish(pos);

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

/// Bytes of the sequences section with the predefined tables: code costs,
/// extra bits, 17 bits of final states, the end mark, and the header.
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

/// Rough FSE cost of codes with counts `freqs`, in bits: about
/// log2(`total` / f) per code, with integer logs.
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

/// Accuracy log for a custom table of `num_seq` codes: about
/// log2(`num_seq`) - 2, at least 5 and wide enough for `max_symbol`, at
/// most `max_log`.
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

/// Literal length code of each length below 64.
#[rustfmt::skip]
const LL_CODE_TABLE: [u8; 64] = [
     0,  1,  2,  3,  4,  5,  6,  7,  8,  9, 10, 11, 12, 13, 14, 15,
    16, 16, 17, 17, 18, 18, 19, 19, 20, 20, 20, 20, 21, 21, 21, 21,
    22, 22, 22, 22, 22, 22, 22, 22, 23, 23, 23, 23, 23, 23, 23, 23,
    24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
];

#[rustfmt::skip]
const ML_CODE_TABLE: [u8; 128] = [
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

/// Offset code: the high bit of the Offset_Value.
#[inline]
fn of_code(offset_value: u32) -> u8 {
    (31 - offset_value.leading_zeros()) as u8
}

/// Rows `code | extra_bits << 8 | baseline << 16` for each index of a code
/// table. Each code covers a run of indices whose length is a power of two,
/// so the run gives the baseline and the extra-bit count.
const fn code_rows<const N: usize>(codes: &[u8; N]) -> [u32; N] {
    let mut rows = [0u32; N];
    let mut i = 0;
    while i < N {
        let code = codes[i];
        let mut first = i;
        while first > 0 && codes[first - 1] == code {
            first -= 1;
        }
        let mut end = i;
        while end < N && codes[end] == code {
            end += 1;
        }
        let nb = (end - first).trailing_zeros();
        rows[i] = code as u32 | nb << 8 | (first as u32) << 16;
        i += 1;
    }
    rows
}

/// [`code_rows`] of literal lengths below 64.
const LL_ROWS: [u32; 64] = code_rows(&LL_CODE_TABLE);
/// [`code_rows`] of match lengths less 3, below 128.
const ML_ROWS: [u32; 128] = code_rows(&ML_CODE_TABLE);

/// A literal length's code, extra-bit count and extra bits. One row lookup
/// replaces the code, bit-count and baseline tables.
#[inline(always)]
fn ll_split(ll: u32) -> (u8, u32, u32) {
    if ll < 64 {
        let row = LL_ROWS[ll as usize];
        (row as u8, (row >> 8) & 0xFF, ll - (row >> 16))
    } else {
        // Past the table, the code is the high bit plus 19 (36 for match
        // lengths), as in C zstd.
        let high_bit = 31 - ll.leading_zeros();
        (high_bit as u8 + 19, high_bit, ll - (1 << high_bit))
    }
}

/// A match length's code, extra-bit count and extra bits, as [`ll_split`].
#[inline(always)]
fn ml_split(ml: u32) -> (u8, u32, u32) {
    let ml_base = ml - 3;
    if ml_base < 128 {
        let row = ML_ROWS[ml_base as usize];
        (row as u8, (row >> 8) & 0xFF, ml_base - (row >> 16))
    } else {
        let high_bit = 31 - ml_base.leading_zeros();
        (high_bit as u8 + 36, high_bit, ml_base - (1 << high_bit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNUTH: u32 = 0x9E37_79B1;
    const POLICY: BlockPolicy = BlockPolicy {
        custom_tables: true,
        max_literal_entropy_fp8: u32::MAX,
    };

    /// Bytes skewed toward low symbols (about 5 bits per byte) with almost no
    /// repeated strings.
    fn skewed_literals(len: usize) -> Vec<u8> {
        (0..len as u32)
            .map(|i| {
                let r = i.wrapping_mul(KNUTH).rotate_left(13).wrapping_mul(KNUTH);
                ((r >> 26) & (r >> 20) & 63) as u8 + b' '
            })
            .collect()
    }

    fn block_header(out: &[u8]) -> (bool, u32, usize) {
        let h = u32::from(out[0]) | (u32::from(out[1]) << 8) | (u32::from(out[2]) << 16);
        (h & 1 == 1, (h >> 1) & 3, (h >> 3) as usize)
    }

    #[test]
    fn block_without_sequences_is_huffman_coded() {
        let src = skewed_literals(64 * 1024);
        let mut out = Vec::new();
        let mut ws = BlockEncodeWorkspace::new();
        let mut rep = [1, 4, 8];
        encode_compressed_block(&src, &[], &mut rep, true, &mut out, &mut ws, POLICY).unwrap();

        let (last, block_type, size) = block_header(&out);
        assert!(last);
        assert_eq!(block_type, 2, "compressed block");
        assert_eq!(size, out.len() - 3);
        assert!(out.len() * 10 < src.len() * 9, "{} bytes", out.len());
        assert_eq!(out[3] & 3, 2, "Huffman literals with a tree");
        assert_eq!(out.last(), Some(&0), "empty sequence section");
        assert_eq!(rep, [1, 4, 8]);
        assert!(ws.prev_huffman().is_some());
    }

    #[test]
    fn block_without_sequences_falls_back_to_raw_for_dense_bytes() {
        // Uniform bytes: Huffman cannot beat raw storage.
        let src: Vec<u8> = (0..4096u32)
            .map(|i| (i.wrapping_mul(KNUTH) >> 24) as u8)
            .collect();
        let mut out = Vec::new();
        let mut ws = BlockEncodeWorkspace::new();
        encode_compressed_block(&src, &[], &mut [1, 4, 8], false, &mut out, &mut ws, POLICY)
            .unwrap();
        let (last, block_type, size) = block_header(&out);
        assert!(!last);
        assert_eq!(block_type, 0, "raw block");
        assert_eq!(size, src.len());
        assert!(ws.prev_huffman().is_none());
    }

    #[test]
    fn raw_block_keeps_the_previous_table() {
        let mut ws = BlockEncodeWorkspace::new();
        let first = skewed_literals(16 * 1024);
        let mut out = Vec::new();
        encode_compressed_block(
            &first,
            &[],
            &mut [1, 4, 8],
            false,
            &mut out,
            &mut ws,
            POLICY,
        )
        .unwrap();
        assert!(ws.prev_huffman().is_some());

        // Dense bytes fall back to a raw block, which leaves the decoder's
        // table alone.
        let dense: Vec<u8> = (0..4096u32)
            .map(|i| (i.wrapping_mul(KNUTH) >> 24) as u8)
            .collect();
        let mut out = Vec::new();
        encode_compressed_block(
            &dense,
            &[],
            &mut [1, 4, 8],
            false,
            &mut out,
            &mut ws,
            POLICY,
        )
        .unwrap();
        assert_eq!(block_header(&out).1, 0, "raw block");
        assert!(ws.prev_huffman().is_some());

        // Literals like the first block's reuse its table without a tree.
        let mut out = Vec::new();
        encode_compressed_block(
            &first[..1024],
            &[],
            &mut [1, 4, 8],
            true,
            &mut out,
            &mut ws,
            POLICY,
        )
        .unwrap();
        assert_eq!(block_header(&out).1, 2, "compressed block");
        assert_eq!(out[3] & 3, 3, "treeless literals");
    }

    #[test]
    fn fresh_table_replaces_one_that_cannot_code_new_symbols() {
        let mut ws = BlockEncodeWorkspace::new();
        let first = skewed_literals(16 * 1024);
        let mut out = Vec::new();
        encode_compressed_block(
            &first,
            &[],
            &mut [1, 4, 8],
            false,
            &mut out,
            &mut ws,
            POLICY,
        )
        .unwrap();
        assert!(ws.prev_huffman().is_some());

        // Same shape, shifted to byte values the first table has no code for.
        let second: Vec<u8> = first.iter().map(|&b| b + 128).collect();
        let mut out = Vec::new();
        encode_compressed_block(
            &second,
            &[],
            &mut [1, 4, 8],
            true,
            &mut out,
            &mut ws,
            POLICY,
        )
        .unwrap();
        let (_, block_type, _) = block_header(&out);
        assert_eq!(block_type, 2);
        assert_eq!(out[3] & 3, 2, "new tree, not treeless reuse");
    }

    #[test]
    fn previous_table_is_reused_for_the_same_distribution() {
        let mut ws = BlockEncodeWorkspace::new();
        let data = skewed_literals(32 * 1024);
        let mut out = Vec::new();
        encode_compressed_block(
            &data[..16 * 1024],
            &[],
            &mut [1, 4, 8],
            false,
            &mut out,
            &mut ws,
            POLICY,
        )
        .unwrap();
        let mut out = Vec::new();
        encode_compressed_block(
            &data[16 * 1024..],
            &[],
            &mut [1, 4, 8],
            true,
            &mut out,
            &mut ws,
            POLICY,
        )
        .unwrap();
        assert_eq!(out[3] & 3, 3, "treeless literals");
    }

    #[test]
    fn dense_literals_stay_raw_under_an_entropy_limit() {
        // About 7 bits per byte: Huffman saves roughly 12%.
        let src: Vec<u8> = (0..16_384u32)
            .map(|i| (i.wrapping_mul(KNUTH) >> 25) as u8)
            .collect();
        let limited = BlockPolicy {
            custom_tables: true,
            max_literal_entropy_fp8: 6 * 256,
        };
        let mut out = Vec::new();
        let mut ws = BlockEncodeWorkspace::new();
        encode_compressed_block(&src, &[], &mut [1, 4, 8], true, &mut out, &mut ws, limited)
            .unwrap();
        assert_eq!(block_header(&out).1, 0, "raw block under the limit");

        let mut out = Vec::new();
        let mut ws = BlockEncodeWorkspace::new();
        encode_compressed_block(&src, &[], &mut [1, 4, 8], true, &mut out, &mut ws, POLICY)
            .unwrap();
        assert_eq!(block_header(&out).1, 2, "Huffman-coded without a limit");
    }

    #[test]
    fn sequence_codes_follow_the_zstd_tables() {
        use zrip_core::fse::{LL_BASELINE_TABLE, LL_BITS_TABLE, ML_BASELINE_TABLE, ML_BITS_TABLE};
        for ll in 0..(1 << 17) {
            let (code, nb, extra) = ll_split(ll);
            let c = usize::from(code);
            assert_eq!(nb, u32::from(LL_BITS_TABLE[c]), "ll {ll}");
            assert_eq!(extra, ll - LL_BASELINE_TABLE[c], "ll {ll}");
            assert!(extra < 1 << nb, "ll {ll}");
        }
        for ml in 3..(1 << 17) {
            let (code, nb, extra) = ml_split(ml);
            let c = usize::from(code);
            assert_eq!(nb, u32::from(ML_BITS_TABLE[c]), "ml {ml}");
            assert_eq!(extra, ml - ML_BASELINE_TABLE[c], "ml {ml}");
            assert!(extra < 1 << nb, "ml {ml}");
        }
        assert_eq!(ll_split(63).0, 24);
        assert_eq!(ll_split(64).0, 25);
        assert_eq!(ml_split(131).0, 43);
        assert_eq!(of_code(1), 0);
        assert_eq!(of_code(4), 2);
        assert_eq!(of_code(1 << 20), 20);
    }
}
