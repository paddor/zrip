//! FSE decoder state over a reverse bitstream.

#![forbid(unsafe_code)]

use crate::bitstream::reader_reverse::ReverseBitReader;
use crate::error::DecompressError;
use crate::fse::FseDecodeEntry;

/// One FSE decoder state over a decode table.
pub struct FseState<'t> {
    table: &'t [FseDecodeEntry],
    state: u32,
}

impl<'t> FseState<'t> {
    /// Reads the initial state, `accuracy_log` bits.
    pub fn new(
        table: &'t [FseDecodeEntry],
        accuracy_log: u8,
        reader: &mut ReverseBitReader,
    ) -> Result<Self, DecompressError> {
        let state = reader.read_bits(accuracy_log)?;
        Ok(Self { table, state })
    }

    /// Symbol of the current state.
    #[inline]
    pub fn symbol(&self) -> u8 {
        self.table[self.state as usize].symbol
    }

    /// Bits the next [`update_state`](Self::update_state) reads.
    #[inline]
    pub fn num_bits(&self) -> u8 {
        self.table[self.state as usize].num_bits
    }

    /// Moves to the next state: the cell's `base_line` plus `num_bits` read
    /// bits.
    #[inline]
    pub fn update_state(&mut self, reader: &mut ReverseBitReader) -> Result<(), DecompressError> {
        let entry = &self.table[self.state as usize];
        let bits = reader.read_bits(entry.num_bits)?;
        self.state = entry.base_line as u32 + bits;
        Ok(())
    }

    /// Current state, an index into the table.
    pub fn state(&self) -> u32 {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitstream::writer::BitWriter;
    use crate::fse::table_builder::build_decode_table_from_default;
    use crate::fse::{LL_DEFAULT_ACCURACY, LL_DEFAULT_DIST};

    #[test]
    fn init_and_read_symbol() {
        let table = build_decode_table_from_default(&LL_DEFAULT_DIST, LL_DEFAULT_ACCURACY);

        let mut w = BitWriter::new();
        w.write_bits(0, LL_DEFAULT_ACCURACY);
        w.close_reverse_stream();
        let data = w.into_bytes();

        let mut reader = ReverseBitReader::new(&data).unwrap();
        let state = FseState::new(&table, LL_DEFAULT_ACCURACY, &mut reader).unwrap();
        let sym = state.symbol();
        assert!(sym <= 35);
    }
}
