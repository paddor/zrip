//! FSE encode tables and state, laid out as C zstd's `FSE_CTable`.

#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::bitstream::writer::BitWriter;

/// FSE encode table for one normalized distribution.
pub struct FseEncodeTable {
    /// Per-symbol transforms, indexed by symbol.
    pub symbol_tt: Vec<SymbolTransform>,
    /// Next states, grouped by symbol. Values are in
    /// `1 << accuracy_log..2 << accuracy_log`.
    pub state_table: Vec<u16>,
    /// Table log: `1 << accuracy_log` states.
    pub accuracy_log: u8,
}

/// Per-symbol encode parameters (C zstd's
/// `FSE_symbolCompressionTransform`).
#[derive(Clone, Copy)]
pub struct SymbolTransform {
    /// `(state + delta_nb_bits) >> 16` is the bit count a state outputs for
    /// this symbol.
    pub delta_nb_bits: u32,
    /// Index offset into `state_table`: the next state is
    /// `state_table[(state >> nb_bits) + delta_find_state]`.
    pub delta_find_state: i32,
}

#[cfg(feature = "alloc")]
impl FseEncodeTable {
    /// Builds a table for `distribution`, normalized to `1 << accuracy_log`
    /// with -1 for "less than one".
    pub fn from_distribution(distribution: &[i16], accuracy_log: u8) -> Self {
        let table_size = 1usize << accuracy_log;
        let mut symbol_tt = vec![
            SymbolTransform {
                delta_nb_bits: 0,
                delta_find_state: 0,
            };
            distribution.len()
        ];
        let mut state_table = vec![0u16; table_size];
        let mut spread = vec![0u16; table_size];
        let mut cumul = vec![0u32; distribution.len() + 1];
        build_encode_table(
            distribution,
            accuracy_log,
            &mut symbol_tt,
            &mut state_table,
            &mut spread,
            &mut cumul,
        );
        Self {
            symbol_tt,
            state_table,
            accuracy_log,
        }
    }
}

/// Builds an encode table for `distribution` into caller storage, as C
/// zstd's `FSE_buildCTable` does: `symbol_tt` takes one transform per
/// symbol, and `state_table` and the scratch `spread` take
/// `1 << accuracy_log` entries each. `cumul` takes `distribution.len() + 1`.
pub fn build_encode_table(
    distribution: &[i16],
    accuracy_log: u8,
    symbol_tt: &mut [SymbolTransform],
    state_table: &mut [u16],
    spread: &mut [u16],
    cumul: &mut [u32],
) {
    let table_size = 1usize << accuracy_log;
    // C zstd's spread step. It is odd, so the walk visits every cell once.
    let step = (table_size >> 1) + (table_size >> 3) + 3;
    let mask = table_size - 1;

    let mut high_threshold = table_size - 1;
    let mut position = 0;
    cumul[0] = 0;
    // "Less than one" symbols take the top cells, one each, which the spread
    // skips.
    for (s, &prob) in distribution.iter().enumerate() {
        if prob == -1 {
            spread[high_threshold] = s as u16;
            high_threshold -= 1;
            cumul[s + 1] = cumul[s] + 1;
        } else {
            cumul[s + 1] = cumul[s] + prob.max(0) as u32;
        }
    }

    for (s, &prob) in distribution.iter().enumerate() {
        if prob <= 0 {
            continue;
        }
        for _ in 0..prob {
            spread[position] = s as u16;
            position = (position + step) & mask;
            while position > high_threshold {
                position = (position + step) & mask;
            }
        }
    }

    for (s, &prob) in distribution.iter().enumerate() {
        // A "less than one" probability (-1) owns one state, exactly like
        // probability 1 (C zstd's FSE_buildCTable).
        symbol_tt[s] = if prob == 0 {
            SymbolTransform {
                delta_nb_bits: ((accuracy_log as u32 + 1) << 16).wrapping_sub(1 << accuracy_log),
                delta_find_state: 0,
            }
        } else if prob == 1 || prob == -1 {
            SymbolTransform {
                delta_nb_bits: ((accuracy_log as u32) << 16).wrapping_sub(1 << accuracy_log),
                delta_find_state: cumul[s] as i32 - 1,
            }
        } else {
            let prob = prob as u32;
            let max_bits_out = accuracy_log as u32 - high_bit(prob - 1);
            let min_state_plus = prob << max_bits_out;
            SymbolTransform {
                delta_nb_bits: (max_bits_out << 16).wrapping_sub(min_state_plus),
                delta_find_state: (cumul[s] as i32) - (prob as i32),
            }
        };
    }

    // State table stores values in [table_size, 2*table_size), matching C
    // zstd's FSE_buildCTable convention. `cumul` now counts each symbol's
    // next state number.
    for (i, &st) in spread[..table_size].iter().enumerate() {
        let next = &mut cumul[st as usize];
        state_table[*next as usize] = (table_size + i) as u16;
        *next += 1;
    }
}

/// One FSE encoder state. Symbols are encoded last to first, so the
/// decoder reads them in order.
pub struct FseEncodeState<'t> {
    table: &'t FseEncodeTable,
    state: u32,
}

#[cfg(feature = "alloc")]
impl<'t> FseEncodeState<'t> {
    /// Starts on `symbol` without output bits (C zstd's `FSE_initCState2`).
    pub fn init(table: &'t FseEncodeTable, symbol: u8) -> Self {
        let tt = &table.symbol_tt[symbol as usize];
        let nb_bits_out = tt.delta_nb_bits.wrapping_add(1 << 16) >> 16;
        let value = (nb_bits_out << 16).wrapping_sub(tt.delta_nb_bits);
        let idx = (value >> nb_bits_out) as i32 + tt.delta_find_state;
        let state = table.state_table[idx as usize] as u32;
        Self { table, state }
    }

    /// Writes the low bits of the state, then moves to the state for
    /// `symbol`.
    pub fn encode_symbol(&mut self, writer: &mut BitWriter, symbol: u8) {
        let tt = &self.table.symbol_tt[symbol as usize];
        let nb_bits_out = (self.state.wrapping_add(tt.delta_nb_bits) >> 16) as u8;
        writer.write_bits(self.state & ((1u32 << nb_bits_out) - 1), nb_bits_out);
        self.state = self.table.state_table
            [((self.state >> nb_bits_out) as i32 + tt.delta_find_state) as usize]
            as u32;
    }

    /// Writes the final state, `accuracy_log` bits, for the decoder to start
    /// from.
    pub fn flush(&self, writer: &mut BitWriter, accuracy_log: u8) {
        writer.write_bits(self.state & ((1u32 << accuracy_log) - 1), accuracy_log);
    }

    /// Current state, in `1 << accuracy_log..2 << accuracy_log`.
    pub fn state(&self) -> u32 {
        self.state
    }
}

fn high_bit(val: u32) -> u32 {
    debug_assert!(val > 0);
    31 - val.leading_zeros()
}
