//! Sequence decoding tables: FSE rows merged with each code's baseline and
//! extra-bit count, so one load yields both the next state and the value.

use zrip_core::error::DecompressError;
use zrip_core::fse::table_builder::build_decode_rows;
use zrip_core::fse::{
    FSE_SEQ_TABLE_CAPACITY, FseDecodeEntry, FseSeqDecodeEntry, LL_BASELINE_TABLE, LL_BITS_TABLE,
    LL_DEFAULT_ACCURACY, LL_DEFAULT_DIST, ML_BASELINE_TABLE, ML_BITS_TABLE, ML_DEFAULT_ACCURACY,
    ML_DEFAULT_DIST, OF_DEFAULT_ACCURACY, OF_DEFAULT_DIST,
};

/// A bit count below 32. Every sequence field and state transition reads
/// fewer bits. As an enum its range reaches the compiler with each load, so
/// shifts and masks by it need no `& 63` in front of `shrx` or `bzhi`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum BitCount {
    #[default]
    B0,
    B1,
    B2,
    B3,
    B4,
    B5,
    B6,
    B7,
    B8,
    B9,
    B10,
    B11,
    B12,
    B13,
    B14,
    B15,
    B16,
    B17,
    B18,
    B19,
    B20,
    B21,
    B22,
    B23,
    B24,
    B25,
    B26,
    B27,
    B28,
    B29,
    B30,
    B31,
}

impl BitCount {
    const ALL: [Self; 32] = [
        Self::B0,
        Self::B1,
        Self::B2,
        Self::B3,
        Self::B4,
        Self::B5,
        Self::B6,
        Self::B7,
        Self::B8,
        Self::B9,
        Self::B10,
        Self::B11,
        Self::B12,
        Self::B13,
        Self::B14,
        Self::B15,
        Self::B16,
        Self::B17,
        Self::B18,
        Self::B19,
        Self::B20,
        Self::B21,
        Self::B22,
        Self::B23,
        Self::B24,
        Self::B25,
        Self::B26,
        Self::B27,
        Self::B28,
        Self::B29,
        Self::B30,
        Self::B31,
    ];

    /// The count `count`, which is below 32 in every table the decoder builds.
    const fn new(count: u8) -> Self {
        debug_assert!(count < 32);
        Self::ALL[(count & 31) as usize]
    }

    /// The count as a number.
    #[inline(always)]
    pub(crate) const fn get(self) -> u32 {
        self as u32
    }
}

/// One sequence decoding row: the next state's base and bit count, and the
/// decoded value's baseline and extra-bit count.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SeqEntry {
    /// Next state, before adding the `num_bits` bits read for it.
    pub(crate) base_line: u16,
    /// Bits to read for the next state.
    pub(crate) num_bits: BitCount,
    /// Bits to read for the value.
    pub(crate) extra_bits: BitCount,
    /// Value before adding its extra bits: a literal length, a match
    /// length, or an offset value.
    pub(crate) baseline_value: u32,
}

impl SeqEntry {
    const fn new(base_line: u16, num_bits: u8, extra_bits: u8, baseline_value: u32) -> Self {
        Self {
            base_line,
            num_bits: BitCount::new(num_bits),
            extra_bits: BitCount::new(extra_bits),
            baseline_value,
        }
    }
}

impl From<FseSeqDecodeEntry> for SeqEntry {
    fn from(e: FseSeqDecodeEntry) -> Self {
        Self::new(e.base_line, e.num_bits, e.extra_bits, e.baseline_value)
    }
}

/// One sequence table at full capacity (`FSE_SEQ_TABLE_CAPACITY` rows), so
/// a state masked with `FSE_SEQ_TABLE_MASK` always indexes in bounds.
pub(crate) struct SeqTable {
    /// Rows in use: `1 << accuracy_log`, or 1 in RLE mode.
    initialized: usize,
    data: [SeqEntry; FSE_SEQ_TABLE_CAPACITY],
}

impl Clone for SeqTable {
    fn clone(&self) -> Self {
        Self {
            initialized: self.initialized,
            data: self.data,
        }
    }

    /// Copies only the initialized entries. Decoding reads no others, and a
    /// small table uses a fraction of the capacity.
    fn clone_from(&mut self, source: &Self) {
        let n = source.initialized;
        self.data[..n].copy_from_slice(&source.data[..n]);
        self.initialized = n;
    }
}

/// Where a predefined table's codes get their extra bits and baselines.
#[derive(Clone, Copy)]
enum Extra {
    /// From per-code tables: literal and match lengths.
    Tables(&'static [u8], &'static [u32]),
    /// Offset code `n` reads `n` extra bits onto a baseline of `1 << n`.
    Offset,
}

impl SeqTable {
    /// A table of zeroed rows, the first `initialized` of them counted as
    /// in use.
    pub(crate) const fn empty(initialized: usize) -> Self {
        Self {
            initialized,
            data: [SeqEntry::new(0, 0, 0, 0); FSE_SEQ_TABLE_CAPACITY],
        }
    }

    /// The RFC 8878 predefined literal length table.
    pub(crate) const fn predefined_ll() -> Self {
        Self::predefined(
            &LL_DEFAULT_DIST,
            LL_DEFAULT_ACCURACY,
            Extra::Tables(&LL_BITS_TABLE, &LL_BASELINE_TABLE),
        )
    }

    /// The RFC 8878 predefined match length table.
    pub(crate) const fn predefined_ml() -> Self {
        Self::predefined(
            &ML_DEFAULT_DIST,
            ML_DEFAULT_ACCURACY,
            Extra::Tables(&ML_BITS_TABLE, &ML_BASELINE_TABLE),
        )
    }

    /// The RFC 8878 predefined offset code table.
    pub(crate) const fn predefined_of() -> Self {
        Self::predefined(&OF_DEFAULT_DIST, OF_DEFAULT_ACCURACY, Extra::Offset)
    }

    /// Builds a predefined table at compile time, as
    /// [`build_decode_rows`] builds one at run time. Every predefined
    /// distribution has "less than one" (-1) symbols, so this is its
    /// general spread: those symbols take the last cells, and the walk
    /// skips them.
    const fn predefined(distribution: &[i16], accuracy_log: u8, extra: Extra) -> Self {
        let size = 1usize << accuracy_log;
        let mask = size - 1;
        // C zstd's spread step. It is odd, so the walk visits every cell once.
        let step = (size >> 1) + (size >> 3) + 3;
        let mut cells = [0u8; FSE_SEQ_TABLE_CAPACITY];
        let mut next = [0u16; 256];
        let mut high_threshold = mask;
        let mut s = 0;
        while s < distribution.len() {
            if distribution[s] == -1 {
                cells[high_threshold] = s as u8;
                high_threshold -= 1;
                next[s] = 1;
            } else {
                next[s] = distribution[s] as u16;
            }
            s += 1;
        }
        let mut position = 0;
        s = 0;
        while s < distribution.len() {
            let mut k = 0;
            while k < distribution[s] {
                cells[position] = s as u8;
                position = (position + step) & mask;
                while position > high_threshold {
                    position = (position + step) & mask;
                }
                k += 1;
            }
            s += 1;
        }
        assert!(position == 0, "a predefined distribution fills its table");

        // The k-th cell of a symbol with probability p gets state p + k; it
        // reads `nb` bits so `(p + k) << nb` lands in `size..2 * size`.
        let mut table = Self::empty(size);
        let mut cell = 0;
        while cell < size {
            let symbol = cells[cell] as usize;
            let state = next[symbol] as u32;
            next[symbol] += 1;
            let nb = accuracy_log as u32 - (31 - state.leading_zeros());
            let base_line = (state << nb) - size as u32;
            let (extra_bits, baseline_value) = match extra {
                Extra::Tables(bits, baselines) => (bits[symbol], baselines[symbol]),
                Extra::Offset => (symbol as u8, 1u32 << symbol),
            };
            table.data[cell] =
                SeqEntry::new(base_line as u16, nb as u8, extra_bits, baseline_value);
            cell += 1;
        }
        table
    }

    #[inline(always)]
    fn set(&mut self, idx: usize, entry: SeqEntry) {
        self.data[idx] = entry;
    }

    /// Makes an RLE table: one row. With accuracy log 0, the state stays 0.
    #[inline(always)]
    pub(crate) fn set_single(&mut self, val: FseSeqDecodeEntry) {
        self.set(0, val.into());
        self.initialized = 1;
    }

    /// The row for `idx`, which the caller masks below the capacity.
    #[inline(always)]
    pub(crate) fn get(&self, idx: usize) -> &SeqEntry {
        debug_assert!(idx < self.initialized);
        &self.data[idx]
    }

    /// Rewrites the first `fse.len()` entries in place from a decode table,
    /// with `extra` mapping each symbol to its extra bits and baseline. Avoids
    /// building and moving a whole new table per block.
    #[inline(always)]
    fn fill(&mut self, fse: &[FseDecodeEntry], extra: impl Fn(u8) -> (u8, u32)) {
        debug_assert!(fse.len() <= FSE_SEQ_TABLE_CAPACITY);
        let n = fse.len().min(FSE_SEQ_TABLE_CAPACITY);
        for (i, e) in fse[..n].iter().enumerate() {
            let (extra_bits, baseline_value) = extra(e.symbol);
            self.set(
                i,
                SeqEntry::new(e.base_line, e.num_bits, extra_bits, baseline_value),
            );
        }
        self.initialized = n;
    }

    /// Builds the table from a parsed distribution in one pass, with
    /// `extra` mapping each symbol to its extra bits and baseline. Leaves the
    /// table as it was on an error.
    #[inline(always)]
    fn build(
        &mut self,
        distribution: &[i16],
        accuracy_log: u8,
        extra: impl Fn(u8) -> (u8, u32),
    ) -> Result<(), DecompressError> {
        let mut cells = [0u8; FSE_SEQ_TABLE_CAPACITY];
        let data = &mut self.data;
        build_decode_rows(
            distribution,
            accuracy_log,
            &mut cells,
            |cell, symbol, base_line, num_bits| {
                let (extra_bits, baseline_value) = extra(symbol);
                data[cell & (FSE_SEQ_TABLE_CAPACITY - 1)] =
                    SeqEntry::new(base_line, num_bits, extra_bits, baseline_value);
            },
        )?;
        self.initialized = 1 << accuracy_log;
        Ok(())
    }

    /// Builds the table from a literal length distribution.
    #[cfg_attr(
        all(
            feature = "std",
            feature = "simd",
            not(feature = "paranoid"),
            target_arch = "x86_64",
            not(target_feature = "avx2")
        ),
        inline(always)
    )]
    pub(crate) fn build_ll(
        &mut self,
        distribution: &[i16],
        accuracy_log: u8,
    ) -> Result<(), DecompressError> {
        self.build(distribution, accuracy_log, ll_extra)
    }

    /// Builds the table from a match length distribution.
    #[cfg_attr(
        all(
            feature = "std",
            feature = "simd",
            not(feature = "paranoid"),
            target_arch = "x86_64",
            not(target_feature = "avx2")
        ),
        inline(always)
    )]
    pub(crate) fn build_ml(
        &mut self,
        distribution: &[i16],
        accuracy_log: u8,
    ) -> Result<(), DecompressError> {
        self.build(distribution, accuracy_log, ml_extra)
    }

    /// Builds the table from an offset code distribution.
    #[cfg_attr(
        all(
            feature = "std",
            feature = "simd",
            not(feature = "paranoid"),
            target_arch = "x86_64",
            not(target_feature = "avx2")
        ),
        inline(always)
    )]
    pub(crate) fn build_of(
        &mut self,
        distribution: &[i16],
        accuracy_log: u8,
    ) -> Result<(), DecompressError> {
        self.build(distribution, accuracy_log, of_extra)
    }

    /// Rewrites the table in place from literal length FSE rows.
    pub(crate) fn load_ll(&mut self, fse: &[FseDecodeEntry]) {
        self.fill(fse, ll_extra);
    }

    /// Rewrites the table in place from match length FSE rows.
    pub(crate) fn load_ml(&mut self, fse: &[FseDecodeEntry]) {
        self.fill(fse, ml_extra);
    }

    /// Rewrites the table in place from offset code FSE rows.
    pub(crate) fn load_of(&mut self, fse: &[FseDecodeEntry]) {
        self.fill(fse, of_extra);
    }
}

fn ll_extra(symbol: u8) -> (u8, u32) {
    (
        LL_BITS_TABLE[symbol as usize],
        LL_BASELINE_TABLE[symbol as usize],
    )
}

fn ml_extra(symbol: u8) -> (u8, u32) {
    (
        ML_BITS_TABLE[symbol as usize],
        ML_BASELINE_TABLE[symbol as usize],
    )
}

/// Offset code `n` reads `n` extra bits onto a baseline of `1 << n`.
fn of_extra(symbol: u8) -> (u8, u32) {
    (symbol, 1u32 << symbol)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq_entry(value: u32) -> FseSeqDecodeEntry {
        FseSeqDecodeEntry {
            base_line: 0,
            num_bits: 0,
            extra_bits: 0,
            baseline_value: value,
        }
    }

    #[test]
    fn single_table_replaces_entry_zero() {
        let fse = [FseDecodeEntry {
            base_line: 0,
            num_bits: 0,
            symbol: 0,
        }];
        let mut table = SeqTable::empty(0);
        table.load_ll(&fse);
        table.set_single(seq_entry(17));

        assert_eq!(table.get(0).baseline_value, 17);
    }

    #[test]
    fn promoted_table_returns_initialized_entry() {
        let fse = [
            FseDecodeEntry {
                base_line: 0,
                num_bits: 0,
                symbol: 0,
            },
            FseDecodeEntry {
                base_line: 0,
                num_bits: 0,
                symbol: 1,
            },
            FseDecodeEntry {
                base_line: 0,
                num_bits: 0,
                symbol: 2,
            },
            FseDecodeEntry {
                base_line: 0,
                num_bits: 0,
                symbol: 3,
            },
        ];
        let mut table = SeqTable::empty(0);
        table.load_of(&fse);

        assert_eq!(table.get(0).baseline_value, 1);
        assert_eq!(table.get(1).baseline_value, 2);
        assert_eq!(table.get(3).extra_bits.get(), 3);
    }

    #[test]
    fn predefined_tables_match_the_runtime_builder() {
        use zrip_core::fse::table_builder::build_decode_table_from_default;
        let cases = [
            (
                SeqTable::predefined_ll(),
                &LL_DEFAULT_DIST[..],
                LL_DEFAULT_ACCURACY,
                ll_extra as fn(u8) -> (u8, u32),
            ),
            (
                SeqTable::predefined_ml(),
                &ML_DEFAULT_DIST[..],
                ML_DEFAULT_ACCURACY,
                ml_extra,
            ),
            (
                SeqTable::predefined_of(),
                &OF_DEFAULT_DIST[..],
                OF_DEFAULT_ACCURACY,
                of_extra,
            ),
        ];
        for (table, dist, accuracy, extra) in cases {
            let mut built = SeqTable::empty(0);
            built.fill(&build_decode_table_from_default(dist, accuracy), extra);
            assert_eq!(table.initialized, 1 << accuracy);
            assert_eq!(table.initialized, built.initialized);
            for (a, b) in table.data.iter().zip(&built.data) {
                assert_eq!(
                    (a.base_line, a.num_bits, a.extra_bits, a.baseline_value),
                    (b.base_line, b.num_bits, b.extra_bits, b.baseline_value)
                );
            }
        }
    }

    #[test]
    fn bit_counts_keep_their_value() {
        for count in 0..32u8 {
            assert_eq!(BitCount::new(count).get(), u32::from(count));
        }
    }
}
