#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::bitstream::writer::BitWriter;

pub struct FseEncodeTable {
    pub symbol_tt: Vec<SymbolTransform>,
    pub state_table: Vec<u16>,
    pub accuracy_log: u8,
}

#[derive(Clone, Copy)]
pub struct SymbolTransform {
    pub delta_nb_bits: u32,
    pub delta_find_state: i32,
}

#[cfg(feature = "alloc")]
impl FseEncodeTable {
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

        let step = (table_size >> 1) + (table_size >> 3) + 3;
        let mask = table_size - 1;

        let mut high_threshold = table_size - 1;
        let mut position = 0;
        let mut cumul = vec![0u32; distribution.len() + 1];

        for (s, &prob) in distribution.iter().enumerate() {
            if prob == -1 {
                state_table[high_threshold] = s as u16;
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
                state_table[position] = s as u16;
                position = (position + step) & mask;
                while position > high_threshold {
                    position = (position + step) & mask;
                }
            }
        }

        let mut next_state_number = vec![0u32; distribution.len()];
        for s in 0..distribution.len() {
            let prob = distribution[s];
            if prob <= 0 {
                let max_nb_bits = accuracy_log;
                let min_state_plus = if prob == -1 { 1u32 } else { 0 };
                symbol_tt[s].delta_nb_bits = ((max_nb_bits as u32 + 1) << 16) - min_state_plus;
                symbol_tt[s].delta_find_state = 0;
                next_state_number[s] = cumul[s];
            } else if prob == 1 {
                let max_bits_out = accuracy_log as u32;
                let min_state_plus = 1u32 << accuracy_log;
                symbol_tt[s].delta_nb_bits = (max_bits_out << 16).wrapping_sub(min_state_plus);
                symbol_tt[s].delta_find_state = cumul[s] as i32 - 1;
                next_state_number[s] = cumul[s];
            } else {
                let prob = prob as u32;
                let max_bits_out = accuracy_log as u32 - high_bit(prob - 1);
                let min_state_plus = prob << max_bits_out;
                symbol_tt[s].delta_nb_bits = (max_bits_out << 16).wrapping_sub(min_state_plus);
                symbol_tt[s].delta_find_state = (cumul[s] as i32) - (prob as i32);
                next_state_number[s] = cumul[s];
            }
        }

        // State table stores values in [table_size, 2*table_size),
        // matching C zstd's FSE_buildCTable convention.
        let mut table_symbol_sorted = vec![0u16; table_size];
        for (i, &st) in state_table.iter().enumerate().take(table_size) {
            let s = st as usize;
            let ns = next_state_number[s];
            table_symbol_sorted[ns as usize] = (table_size + i) as u16;
            next_state_number[s] += 1;
        }

        Self {
            symbol_tt,
            state_table: table_symbol_sorted,
            accuracy_log,
        }
    }
}

pub struct FseEncodeState<'t> {
    table: &'t FseEncodeTable,
    state: u32,
}

#[cfg(feature = "alloc")]
impl<'t> FseEncodeState<'t> {
    pub fn init(table: &'t FseEncodeTable, symbol: u8) -> Self {
        let tt = &table.symbol_tt[symbol as usize];
        let nb_bits_out = tt.delta_nb_bits.wrapping_add(1 << 16) >> 16;
        let value = (nb_bits_out << 16).wrapping_sub(tt.delta_nb_bits);
        let idx = (value >> nb_bits_out) as i32 + tt.delta_find_state;
        let state = table.state_table[idx as usize] as u32;
        Self { table, state }
    }

    pub fn encode_symbol(&mut self, writer: &mut BitWriter, symbol: u8) {
        let tt = &self.table.symbol_tt[symbol as usize];
        let nb_bits_out = (self.state.wrapping_add(tt.delta_nb_bits) >> 16) as u8;
        writer.write_bits(self.state & ((1u32 << nb_bits_out) - 1), nb_bits_out);
        self.state = self.table.state_table
            [((self.state >> nb_bits_out) as i32 + tt.delta_find_state) as usize]
            as u32;
    }

    pub fn flush(&self, writer: &mut BitWriter, accuracy_log: u8) {
        writer.write_bits(self.state & ((1u32 << accuracy_log) - 1), accuracy_log);
    }

    pub fn state(&self) -> u32 {
        self.state
    }
}

fn high_bit(val: u32) -> u32 {
    debug_assert!(val > 0);
    31 - val.leading_zeros()
}
