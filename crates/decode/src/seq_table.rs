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
    pub(crate) fn set(&mut self, idx: usize, val: FseSeqDecodeEntry) {
        self.data[idx] = MaybeUninit::new(val);
        self.initialized = self.initialized.max(idx + 1);
    }

    #[cfg(feature = "paranoid")]
    #[inline(always)]
    pub(crate) fn set(&mut self, idx: usize, val: FseSeqDecodeEntry) {
        self.data[idx] = val;
        self.initialized = self.initialized.max(idx + 1);
    }

    #[cfg(not(feature = "paranoid"))]
    #[inline(always)]
    pub(crate) fn get(&self, idx: usize) -> FseSeqDecodeEntry {
        assert!(idx < self.initialized);
        // SAFETY: The FSE state machine bounds idx to [0, 1 << accuracy_log).
        // All entries in that range are initialized by promote_* or set.
        unsafe { self.data[idx].assume_init() }
    }

    #[cfg(feature = "paranoid")]
    #[inline(always)]
    pub(crate) fn get(&self, idx: usize) -> FseSeqDecodeEntry {
        assert!(idx < self.initialized);
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
