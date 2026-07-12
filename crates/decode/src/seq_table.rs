#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

use zrip_core::fse::{
    FSE_SEQ_TABLE_CAPACITY, FseDecodeEntry, FseSeqDecodeEntry, LL_BASELINE_TABLE, LL_BITS_TABLE,
    ML_BASELINE_TABLE, ML_BITS_TABLE,
};

#[cfg(not(feature = "paranoid"))]
use core::mem::MaybeUninit;

#[cfg(feature = "paranoid")]
use zrip_core::fse::FSE_SEQ_DECODE_ENTRY_ZERO;

pub(crate) struct SeqTable {
    initialized: usize,
    #[cfg(not(feature = "paranoid"))]
    data: [MaybeUninit<FseSeqDecodeEntry>; FSE_SEQ_TABLE_CAPACITY],
    #[cfg(feature = "paranoid")]
    data: [FseSeqDecodeEntry; FSE_SEQ_TABLE_CAPACITY],
}

impl Clone for SeqTable {
    fn clone(&self) -> Self {
        Self {
            initialized: self.initialized,
            data: self.data,
        }
    }
}

impl SeqTable {
    #[cfg(not(feature = "paranoid"))]
    #[inline(always)]
    pub(crate) fn set_single(&mut self, val: FseSeqDecodeEntry) {
        self.data[0] = MaybeUninit::new(val);
        self.initialized = 1;
    }

    #[cfg(feature = "paranoid")]
    #[inline(always)]
    pub(crate) fn set_single(&mut self, val: FseSeqDecodeEntry) {
        self.data[0] = val;
        self.initialized = 1;
    }

    #[cfg(not(feature = "paranoid"))]
    #[inline(always)]
    pub(crate) unsafe fn get(&self, idx: usize) -> FseSeqDecodeEntry {
        debug_assert!(idx < self.initialized);
        // SAFETY: The FSE state machine bounds idx to [0, 1 << accuracy_log).
        // promote_* initializes all entries in that range. set_single is used
        // only for RLE tables, whose state is always zero.
        unsafe { self.data[idx].assume_init() }
    }

    #[cfg(feature = "paranoid")]
    #[inline(always)]
    pub(crate) fn get(&self, idx: usize) -> FseSeqDecodeEntry {
        debug_assert!(idx < self.initialized);
        self.data[idx]
    }

    #[cfg(not(feature = "paranoid"))]
    pub(crate) fn promote_ll(fse: &[FseDecodeEntry]) -> Self {
        debug_assert!(fse.len() <= FSE_SEQ_TABLE_CAPACITY);
        let mut table = Self {
            initialized: fse.len(),
            data: [const { MaybeUninit::uninit() }; FSE_SEQ_TABLE_CAPACITY],
        };
        for (i, e) in fse.iter().enumerate() {
            table.data[i] = MaybeUninit::new(FseSeqDecodeEntry {
                base_line: e.base_line,
                num_bits: e.num_bits,
                extra_bits: LL_BITS_TABLE[e.symbol as usize],
                baseline_value: LL_BASELINE_TABLE[e.symbol as usize],
            });
        }
        table
    }

    #[cfg(feature = "paranoid")]
    pub(crate) fn promote_ll(fse: &[FseDecodeEntry]) -> Self {
        let mut table = Self {
            initialized: fse.len(),
            data: [FSE_SEQ_DECODE_ENTRY_ZERO; FSE_SEQ_TABLE_CAPACITY],
        };
        for (i, e) in fse.iter().enumerate() {
            table.data[i] = FseSeqDecodeEntry {
                base_line: e.base_line,
                num_bits: e.num_bits,
                extra_bits: LL_BITS_TABLE[e.symbol as usize],
                baseline_value: LL_BASELINE_TABLE[e.symbol as usize],
            };
        }
        table
    }

    #[cfg(not(feature = "paranoid"))]
    pub(crate) fn promote_ml(fse: &[FseDecodeEntry]) -> Self {
        debug_assert!(fse.len() <= FSE_SEQ_TABLE_CAPACITY);
        let mut table = Self {
            initialized: fse.len(),
            data: [const { MaybeUninit::uninit() }; FSE_SEQ_TABLE_CAPACITY],
        };
        for (i, e) in fse.iter().enumerate() {
            table.data[i] = MaybeUninit::new(FseSeqDecodeEntry {
                base_line: e.base_line,
                num_bits: e.num_bits,
                extra_bits: ML_BITS_TABLE[e.symbol as usize],
                baseline_value: ML_BASELINE_TABLE[e.symbol as usize],
            });
        }
        table
    }

    #[cfg(feature = "paranoid")]
    pub(crate) fn promote_ml(fse: &[FseDecodeEntry]) -> Self {
        let mut table = Self {
            initialized: fse.len(),
            data: [FSE_SEQ_DECODE_ENTRY_ZERO; FSE_SEQ_TABLE_CAPACITY],
        };
        for (i, e) in fse.iter().enumerate() {
            table.data[i] = FseSeqDecodeEntry {
                base_line: e.base_line,
                num_bits: e.num_bits,
                extra_bits: ML_BITS_TABLE[e.symbol as usize],
                baseline_value: ML_BASELINE_TABLE[e.symbol as usize],
            };
        }
        table
    }

    #[cfg(not(feature = "paranoid"))]
    pub(crate) fn promote_of(fse: &[FseDecodeEntry]) -> Self {
        debug_assert!(fse.len() <= FSE_SEQ_TABLE_CAPACITY);
        let mut table = Self {
            initialized: fse.len(),
            data: [const { MaybeUninit::uninit() }; FSE_SEQ_TABLE_CAPACITY],
        };
        for (i, e) in fse.iter().enumerate() {
            table.data[i] = MaybeUninit::new(FseSeqDecodeEntry {
                base_line: e.base_line,
                num_bits: e.num_bits,
                extra_bits: e.symbol,
                baseline_value: 1u32 << e.symbol,
            });
        }
        table
    }

    #[cfg(feature = "paranoid")]
    pub(crate) fn promote_of(fse: &[FseDecodeEntry]) -> Self {
        let mut table = Self {
            initialized: fse.len(),
            data: [FSE_SEQ_DECODE_ENTRY_ZERO; FSE_SEQ_TABLE_CAPACITY],
        };
        for (i, e) in fse.iter().enumerate() {
            table.data[i] = FseSeqDecodeEntry {
                base_line: e.base_line,
                num_bits: e.num_bits,
                extra_bits: e.symbol,
                baseline_value: 1u32 << e.symbol,
            };
        }
        table
    }
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
        let mut table = SeqTable::promote_ll(&fse);
        table.set_single(seq_entry(17));

        assert_eq!(paranoid_unsafe_call!(table.get(0)).baseline_value, 17);
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
        let table = SeqTable::promote_of(&fse);

        assert_eq!(paranoid_unsafe_call!(table.get(0)).baseline_value, 1);
        assert_eq!(paranoid_unsafe_call!(table.get(1)).baseline_value, 2);
    }
}
