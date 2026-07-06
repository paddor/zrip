#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::boxed::Box;

use crate::BlockDecodeWorkspace;
use crate::seq_table::SeqTable;
use zrip_core::bitstream::reader::BitReader;
use zrip_core::error::DecompressError;
use zrip_core::fse::table_builder::{
    build_decode_table_from_default, build_decode_table_into, parse_fse_table_description_into,
};
use zrip_core::fse::{
    FseSeqDecodeEntry, LL_BASELINE_TABLE, LL_BITS_TABLE, LL_DEFAULT_ACCURACY, LL_DEFAULT_DIST,
    LL_MAX_ACCURACY_LOG, ML_BASELINE_TABLE, ML_BITS_TABLE, ML_DEFAULT_ACCURACY, ML_DEFAULT_DIST,
    ML_MAX_ACCURACY_LOG, OF_DEFAULT_ACCURACY, OF_DEFAULT_DIST, OF_MAX_ACCURACY_LOG,
};
use zrip_core::hint::likely;

#[derive(Clone)]
pub(crate) struct SequenceDecodeTables {
    pub(crate) ll_table: SeqTable,
    pub(crate) ll_accuracy: u8,
    pub(crate) ll_kind: SequenceTableKind,
    pub(crate) of_table: SeqTable,
    pub(crate) of_accuracy: u8,
    pub(crate) of_kind: SequenceTableKind,
    pub(crate) ml_table: SeqTable,
    pub(crate) ml_accuracy: u8,
    pub(crate) ml_kind: SequenceTableKind,
    pub(crate) ll_set: bool,
    pub(crate) of_set: bool,
    pub(crate) ml_set: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SequenceTableKind {
    Predefined,
    Other,
}

impl SequenceDecodeTables {
    pub(crate) fn new_default() -> Self {
        #[cfg(feature = "std")]
        {
            Self {
                ll_table: LL_PREDEFINED.clone(),
                ll_accuracy: LL_DEFAULT_ACCURACY,
                ll_kind: SequenceTableKind::Predefined,
                of_table: OF_PREDEFINED.clone(),
                of_accuracy: OF_DEFAULT_ACCURACY,
                of_kind: SequenceTableKind::Predefined,
                ml_table: ML_PREDEFINED.clone(),
                ml_accuracy: ML_DEFAULT_ACCURACY,
                ml_kind: SequenceTableKind::Predefined,
                ll_set: false,
                of_set: false,
                ml_set: false,
            }
        }
        #[cfg(not(feature = "std"))]
        {
            Self {
                ll_table: SeqTable::promote_ll(&build_decode_table_from_default(
                    &LL_DEFAULT_DIST,
                    LL_DEFAULT_ACCURACY,
                )),
                ll_accuracy: LL_DEFAULT_ACCURACY,
                ll_kind: SequenceTableKind::Predefined,
                of_table: SeqTable::promote_of(&build_decode_table_from_default(
                    &OF_DEFAULT_DIST,
                    OF_DEFAULT_ACCURACY,
                )),
                of_accuracy: OF_DEFAULT_ACCURACY,
                of_kind: SequenceTableKind::Predefined,
                ml_table: SeqTable::promote_ml(&build_decode_table_from_default(
                    &ML_DEFAULT_DIST,
                    ML_DEFAULT_ACCURACY,
                )),
                ml_accuracy: ML_DEFAULT_ACCURACY,
                ml_kind: SequenceTableKind::Predefined,
                ll_set: false,
                of_set: false,
                ml_set: false,
            }
        }
    }

    pub(crate) fn reset_default(&mut self) {
        if self.ll_kind != SequenceTableKind::Predefined {
            #[cfg(feature = "std")]
            {
                self.ll_table = LL_PREDEFINED.clone();
            }
            #[cfg(not(feature = "std"))]
            {
                self.ll_table = SeqTable::promote_ll(&build_decode_table_from_default(
                    &LL_DEFAULT_DIST,
                    LL_DEFAULT_ACCURACY,
                ));
            }
            self.ll_kind = SequenceTableKind::Predefined;
        }
        self.ll_accuracy = LL_DEFAULT_ACCURACY;
        self.ll_set = false;

        if self.of_kind != SequenceTableKind::Predefined {
            #[cfg(feature = "std")]
            {
                self.of_table = OF_PREDEFINED.clone();
            }
            #[cfg(not(feature = "std"))]
            {
                self.of_table = SeqTable::promote_of(&build_decode_table_from_default(
                    &OF_DEFAULT_DIST,
                    OF_DEFAULT_ACCURACY,
                ));
            }
            self.of_kind = SequenceTableKind::Predefined;
        }
        self.of_accuracy = OF_DEFAULT_ACCURACY;
        self.of_set = false;

        if self.ml_kind != SequenceTableKind::Predefined {
            #[cfg(feature = "std")]
            {
                self.ml_table = ML_PREDEFINED.clone();
            }
            #[cfg(not(feature = "std"))]
            {
                self.ml_table = SeqTable::promote_ml(&build_decode_table_from_default(
                    &ML_DEFAULT_DIST,
                    ML_DEFAULT_ACCURACY,
                ));
            }
            self.ml_kind = SequenceTableKind::Predefined;
        }
        self.ml_accuracy = ML_DEFAULT_ACCURACY;
        self.ml_set = false;
    }

    pub(crate) fn clear_repeat_flags(&mut self) {
        self.ll_set = false;
        self.of_set = false;
        self.ml_set = false;
    }
}

pub(crate) fn parse_sequence_count(data: &[u8]) -> Result<(u32, usize), DecompressError> {
    if data.is_empty() {
        return Err(DecompressError::CorruptSequences);
    }

    let byte0 = data[0] as u32;
    if byte0 < 128 {
        Ok((byte0, 1))
    } else if byte0 < 255 {
        if data.len() < 2 {
            return Err(DecompressError::CorruptSequences);
        }
        let count = ((byte0 - 128) << 8) + data[1] as u32;
        Ok((count, 2))
    } else {
        if data.len() < 3 {
            return Err(DecompressError::CorruptSequences);
        }
        let count = (data[1] as u32) + ((data[2] as u32) << 8) + 0x7F00;
        Ok((count, 3))
    }
}

#[inline(always)]
pub(crate) fn compute_offset(
    offset_value: u32,
    literal_length: u32,
    offsets: &mut [u32; 3],
) -> u32 {
    if likely(offset_value > 3) {
        let offset = offset_value - 3;
        offsets[2] = offsets[1];
        offsets[1] = offsets[0];
        offsets[0] = offset;
        offset
    } else {
        let o0 = offsets[0];
        let o1 = offsets[1];
        let o2 = offsets[2];
        let ll0 = (literal_length == 0) as u32;
        let rep_idx = offset_value - 1 + ll0;
        let offset = if rep_idx < 3 {
            offsets[rep_idx as usize]
        } else {
            o0.wrapping_sub(1)
        };
        offsets[2] = if rep_idx >= 2 { o1 } else { o2 };
        offsets[1] = if rep_idx >= 1 { o0 } else { o1 };
        offsets[0] = offset;
        offset
    }
}

#[cfg(feature = "std")]
static LL_PREDEFINED: std::sync::LazyLock<SeqTable> = std::sync::LazyLock::new(|| {
    SeqTable::promote_ll(&build_decode_table_from_default(
        &LL_DEFAULT_DIST,
        LL_DEFAULT_ACCURACY,
    ))
});

#[cfg(feature = "std")]
static ML_PREDEFINED: std::sync::LazyLock<SeqTable> = std::sync::LazyLock::new(|| {
    SeqTable::promote_ml(&build_decode_table_from_default(
        &ML_DEFAULT_DIST,
        ML_DEFAULT_ACCURACY,
    ))
});

#[cfg(feature = "std")]
static OF_PREDEFINED: std::sync::LazyLock<SeqTable> = std::sync::LazyLock::new(|| {
    SeqTable::promote_of(&build_decode_table_from_default(
        &OF_DEFAULT_DIST,
        OF_DEFAULT_ACCURACY,
    ))
});

pub(crate) fn parse_sequence_tables_ws(
    data: &[u8],
    prev: &mut SequenceDecodeTables,
    ws: &mut BlockDecodeWorkspace,
) -> Result<usize, DecompressError> {
    if data.is_empty() {
        return Err(DecompressError::CorruptSequences);
    }

    let mode_byte = data[0];
    if mode_byte & 0x03 != 0 {
        return Err(DecompressError::CorruptSequences);
    }
    let ll_mode = (mode_byte >> 6) & 0x03;
    let of_mode = (mode_byte >> 4) & 0x03;
    let ml_mode = (mode_byte >> 2) & 0x03;
    let cacheable = ll_mode != 3 && of_mode != 3 && ml_mode != 3;

    if cacheable
        && let Some(cached) = &ws.seq_table_cache
        && data.len() >= ws.seq_table_header.len()
        && data[..ws.seq_table_header.len()] == ws.seq_table_header
    {
        if ws.seq_table_cache_tables_current {
            prev.ll_set = cached.ll_set;
            prev.of_set = cached.of_set;
            prev.ml_set = cached.ml_set;
        } else {
            prev.clone_from(cached.as_ref());
            ws.seq_table_cache_tables_current = true;
        }
        return Ok(ws.seq_table_header.len());
    }

    let mut reader = BitReader::new(&data[1..]);

    match ll_mode {
        0 => {
            if prev.ll_kind != SequenceTableKind::Predefined {
                #[cfg(feature = "std")]
                {
                    prev.ll_table = LL_PREDEFINED.clone();
                }
                #[cfg(not(feature = "std"))]
                {
                    prev.ll_table = SeqTable::promote_ll(&build_decode_table_from_default(
                        &LL_DEFAULT_DIST,
                        LL_DEFAULT_ACCURACY,
                    ));
                }
                prev.ll_kind = SequenceTableKind::Predefined;
            }
            prev.ll_accuracy = LL_DEFAULT_ACCURACY;
            prev.ll_set = true;
        }
        1 => {
            let sym = reader.read_bits(8)? as usize;
            if sym >= LL_BITS_TABLE.len() {
                return Err(DecompressError::CorruptSequences);
            }
            prev.ll_table.set(
                0,
                FseSeqDecodeEntry {
                    base_line: 0,
                    num_bits: 0,
                    extra_bits: LL_BITS_TABLE[sym],
                    baseline_value: LL_BASELINE_TABLE[sym],
                },
            );
            prev.ll_accuracy = 0;
            prev.ll_kind = SequenceTableKind::Other;
            prev.ll_set = true;
        }
        2 => {
            let acc = parse_fse_table_description_into(&mut reader, 35, &mut ws.fse_dist)?;
            if acc > LL_MAX_ACCURACY_LOG {
                return Err(DecompressError::BadFseTable);
            }
            build_decode_table_into(
                &ws.fse_dist,
                acc,
                &mut ws.fse_build_buf,
                &mut ws.fse_symbol_next,
            )?;
            prev.ll_table = SeqTable::promote_ll(&ws.fse_build_buf);
            prev.ll_accuracy = acc;
            prev.ll_kind = SequenceTableKind::Other;
            prev.ll_set = true;
        }
        _ => {
            if !prev.ll_set {
                return Err(DecompressError::CorruptSequences);
            }
        }
    }

    match of_mode {
        0 => {
            if prev.of_kind != SequenceTableKind::Predefined {
                #[cfg(feature = "std")]
                {
                    prev.of_table = OF_PREDEFINED.clone();
                }
                #[cfg(not(feature = "std"))]
                {
                    prev.of_table = SeqTable::promote_of(&build_decode_table_from_default(
                        &OF_DEFAULT_DIST,
                        OF_DEFAULT_ACCURACY,
                    ));
                }
                prev.of_kind = SequenceTableKind::Predefined;
            }
            prev.of_accuracy = OF_DEFAULT_ACCURACY;
            prev.of_set = true;
        }
        1 => {
            let sym = reader.read_bits(8)? as u8;
            if sym > 31 {
                return Err(DecompressError::CorruptSequences);
            }
            prev.of_table.set(
                0,
                FseSeqDecodeEntry {
                    base_line: 0,
                    num_bits: 0,
                    extra_bits: sym,
                    baseline_value: 1u32 << sym,
                },
            );
            prev.of_accuracy = 0;
            prev.of_kind = SequenceTableKind::Other;
            prev.of_set = true;
        }
        2 => {
            let acc = parse_fse_table_description_into(&mut reader, 31, &mut ws.fse_dist)?;
            if acc > OF_MAX_ACCURACY_LOG {
                return Err(DecompressError::BadFseTable);
            }
            build_decode_table_into(
                &ws.fse_dist,
                acc,
                &mut ws.fse_build_buf,
                &mut ws.fse_symbol_next,
            )?;
            prev.of_table = SeqTable::promote_of(&ws.fse_build_buf);
            prev.of_accuracy = acc;
            prev.of_kind = SequenceTableKind::Other;
            prev.of_set = true;
        }
        _ => {
            if !prev.of_set {
                return Err(DecompressError::CorruptSequences);
            }
        }
    }

    match ml_mode {
        0 => {
            if prev.ml_kind != SequenceTableKind::Predefined {
                #[cfg(feature = "std")]
                {
                    prev.ml_table = ML_PREDEFINED.clone();
                }
                #[cfg(not(feature = "std"))]
                {
                    prev.ml_table = SeqTable::promote_ml(&build_decode_table_from_default(
                        &ML_DEFAULT_DIST,
                        ML_DEFAULT_ACCURACY,
                    ));
                }
                prev.ml_kind = SequenceTableKind::Predefined;
            }
            prev.ml_accuracy = ML_DEFAULT_ACCURACY;
            prev.ml_set = true;
        }
        1 => {
            let sym = reader.read_bits(8)? as usize;
            if sym >= ML_BITS_TABLE.len() {
                return Err(DecompressError::CorruptSequences);
            }
            prev.ml_table.set(
                0,
                FseSeqDecodeEntry {
                    base_line: 0,
                    num_bits: 0,
                    extra_bits: ML_BITS_TABLE[sym],
                    baseline_value: ML_BASELINE_TABLE[sym],
                },
            );
            prev.ml_accuracy = 0;
            prev.ml_kind = SequenceTableKind::Other;
            prev.ml_set = true;
        }
        2 => {
            let acc = parse_fse_table_description_into(&mut reader, 52, &mut ws.fse_dist)?;
            if acc > ML_MAX_ACCURACY_LOG {
                return Err(DecompressError::BadFseTable);
            }
            build_decode_table_into(
                &ws.fse_dist,
                acc,
                &mut ws.fse_build_buf,
                &mut ws.fse_symbol_next,
            )?;
            prev.ml_table = SeqTable::promote_ml(&ws.fse_build_buf);
            prev.ml_accuracy = acc;
            prev.ml_kind = SequenceTableKind::Other;
            prev.ml_set = true;
        }
        _ => {
            if !prev.ml_set {
                return Err(DecompressError::CorruptSequences);
            }
        }
    }

    let consumed = 1 + reader.bytes_consumed();
    if cacheable {
        ws.seq_table_header.clear();
        ws.seq_table_header.extend_from_slice(&data[..consumed]);
        ws.seq_table_cache = Some(Box::new(prev.clone()));
        ws.seq_table_cache_tables_current = true;
    } else {
        ws.seq_table_cache_tables_current = false;
    }

    Ok(consumed)
}
