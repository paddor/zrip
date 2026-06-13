#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::{vec, vec::Vec};

use crate::bitstream::reader::BitReader;
use crate::bitstream::reader_reverse::ReverseBitReader;
use crate::decode::BlockDecodeWorkspace;
use crate::error::DecompressError;
use crate::fse::table_builder::{
    build_decode_table, build_decode_table_from_default, build_decode_table_into,
    parse_fse_table_description_into,
};
use crate::fse::{
    FseSeqDecodeEntry, LL_BASELINE_TABLE, LL_BITS_TABLE, LL_DEFAULT_ACCURACY, LL_DEFAULT_DIST,
    ML_BASELINE_TABLE, ML_BITS_TABLE, ML_DEFAULT_ACCURACY, ML_DEFAULT_DIST, OF_DEFAULT_ACCURACY,
    OF_DEFAULT_DIST, promote_ll_table, promote_ml_table, promote_of_table,
};

#[derive(Debug, Clone, Copy)]
pub struct Sequence {
    pub literal_length: u32,
    pub offset: u32,
    pub match_length: u32,
}

pub struct SequenceDecodeTables {
    pub ll_table: Vec<FseSeqDecodeEntry>,
    pub ll_accuracy: u8,
    pub of_table: Vec<FseSeqDecodeEntry>,
    pub of_accuracy: u8,
    pub ml_table: Vec<FseSeqDecodeEntry>,
    pub ml_accuracy: u8,
}

impl SequenceDecodeTables {
    pub fn new_default() -> Self {
        Self {
            ll_table: promote_ll_table(&build_decode_table_from_default(
                &LL_DEFAULT_DIST,
                LL_DEFAULT_ACCURACY,
            )),
            ll_accuracy: LL_DEFAULT_ACCURACY,
            of_table: promote_of_table(&build_decode_table_from_default(
                &OF_DEFAULT_DIST,
                OF_DEFAULT_ACCURACY,
            )),
            of_accuracy: OF_DEFAULT_ACCURACY,
            ml_table: promote_ml_table(&build_decode_table_from_default(
                &ML_DEFAULT_DIST,
                ML_DEFAULT_ACCURACY,
            )),
            ml_accuracy: ML_DEFAULT_ACCURACY,
        }
    }
}

pub fn parse_sequence_count(data: &[u8]) -> Result<(u32, usize), DecompressError> {
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

pub fn parse_sequence_tables(
    data: &[u8],
    prev: &mut SequenceDecodeTables,
) -> Result<usize, DecompressError> {
    if data.is_empty() {
        return Err(DecompressError::CorruptSequences);
    }

    let mode_byte = data[0];
    let ll_mode = (mode_byte >> 6) & 0x03;
    let of_mode = (mode_byte >> 4) & 0x03;
    let ml_mode = (mode_byte >> 2) & 0x03;

    let mut reader = BitReader::new(&data[1..]);

    if ll_mode == 0 {
        prev.ll_table = promote_ll_table(&build_decode_table_from_default(
            &LL_DEFAULT_DIST,
            LL_DEFAULT_ACCURACY,
        ));
        prev.ll_accuracy = LL_DEFAULT_ACCURACY;
    } else if ll_mode == 1 {
        let sym = reader.read_bits(8)? as usize;
        if sym >= LL_BITS_TABLE.len() {
            return Err(DecompressError::CorruptSequences);
        }
        prev.ll_table = vec![FseSeqDecodeEntry {
            base_line: 0,
            num_bits: 0,
            extra_bits: LL_BITS_TABLE[sym],
            baseline_value: LL_BASELINE_TABLE[sym],
        }];
        prev.ll_accuracy = 0;
    } else if ll_mode == 2 {
        let (dist, acc) = crate::fse::table_builder::parse_fse_table_description(&mut reader, 35)?;
        prev.ll_table = promote_ll_table(&build_decode_table(&dist, acc)?);
        prev.ll_accuracy = acc;
    }

    if of_mode == 0 {
        prev.of_table = promote_of_table(&build_decode_table_from_default(
            &OF_DEFAULT_DIST,
            OF_DEFAULT_ACCURACY,
        ));
        prev.of_accuracy = OF_DEFAULT_ACCURACY;
    } else if of_mode == 1 {
        let sym = reader.read_bits(8)? as u8;
        if sym > 31 {
            return Err(DecompressError::CorruptSequences);
        }
        prev.of_table = vec![FseSeqDecodeEntry {
            base_line: 0,
            num_bits: 0,
            extra_bits: sym,
            baseline_value: 1u32 << sym,
        }];
        prev.of_accuracy = 0;
    } else if of_mode == 2 {
        let (dist, acc) = crate::fse::table_builder::parse_fse_table_description(&mut reader, 31)?;
        prev.of_table = promote_of_table(&build_decode_table(&dist, acc)?);
        prev.of_accuracy = acc;
    }

    if ml_mode == 0 {
        prev.ml_table = promote_ml_table(&build_decode_table_from_default(
            &ML_DEFAULT_DIST,
            ML_DEFAULT_ACCURACY,
        ));
        prev.ml_accuracy = ML_DEFAULT_ACCURACY;
    } else if ml_mode == 1 {
        let sym = reader.read_bits(8)? as usize;
        if sym >= ML_BITS_TABLE.len() {
            return Err(DecompressError::CorruptSequences);
        }
        prev.ml_table = vec![FseSeqDecodeEntry {
            base_line: 0,
            num_bits: 0,
            extra_bits: ML_BITS_TABLE[sym],
            baseline_value: ML_BASELINE_TABLE[sym],
        }];
        prev.ml_accuracy = 0;
    } else if ml_mode == 2 {
        let (dist, acc) = crate::fse::table_builder::parse_fse_table_description(&mut reader, 52)?;
        prev.ml_table = promote_ml_table(&build_decode_table(&dist, acc)?);
        prev.ml_accuracy = acc;
    }

    Ok(1 + reader.bytes_consumed())
}

pub fn decode_sequences(
    data: &[u8],
    num_sequences: u32,
    tables: &SequenceDecodeTables,
    offsets: &mut [u32; 3],
) -> Result<Vec<Sequence>, DecompressError> {
    if data.is_empty() {
        return Err(DecompressError::CorruptSequences);
    }

    let mut rev_reader =
        ReverseBitReader::new(data).map_err(|_| DecompressError::CorruptSequences)?;

    let mut ll_state = rev_reader.read_bits(tables.ll_accuracy)?;
    let mut of_state = rev_reader.read_bits(tables.of_accuracy)?;
    let mut ml_state = rev_reader.read_bits(tables.ml_accuracy)?;

    let mut sequences = Vec::with_capacity(num_sequences as usize);

    for i in 0..num_sequences {
        rev_reader.refill();

        let of_e = tables.of_table[of_state as usize];
        let ml_e = tables.ml_table[ml_state as usize];
        let ll_e = tables.ll_table[ll_state as usize];

        let of_extra = rev_reader.read_bits_fast(of_e.extra_bits);
        let offset_value = of_e.baseline_value + of_extra;

        let ml_extra = rev_reader.read_bits_fast(ml_e.extra_bits);
        let match_length = ml_e.baseline_value + ml_extra;

        let ll_extra = rev_reader.read_bits_fast(ll_e.extra_bits);
        let literal_length = ll_e.baseline_value + ll_extra;

        let offset = compute_offset(offset_value, literal_length, offsets);

        sequences.push(Sequence {
            literal_length,
            offset,
            match_length,
        });

        if i < num_sequences - 1 {
            rev_reader.refill();

            let ll_entry = tables.ll_table[ll_state as usize];
            ll_state = ll_entry.base_line as u32 + rev_reader.read_bits_fast(ll_entry.num_bits);

            let ml_entry = tables.ml_table[ml_state as usize];
            ml_state = ml_entry.base_line as u32 + rev_reader.read_bits_fast(ml_entry.num_bits);

            let of_entry = tables.of_table[of_state as usize];
            of_state = of_entry.base_line as u32 + rev_reader.read_bits_fast(of_entry.num_bits);
        }
    }

    Ok(sequences)
}

#[inline(always)]
pub(crate) fn compute_offset(
    offset_value: u32,
    literal_length: u32,
    offsets: &mut [u32; 3],
) -> u32 {
    if offset_value > 3 {
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
static LL_PREDEFINED: std::sync::LazyLock<Vec<FseSeqDecodeEntry>> =
    std::sync::LazyLock::new(|| {
        promote_ll_table(&build_decode_table_from_default(
            &LL_DEFAULT_DIST,
            LL_DEFAULT_ACCURACY,
        ))
    });

#[cfg(feature = "std")]
static ML_PREDEFINED: std::sync::LazyLock<Vec<FseSeqDecodeEntry>> =
    std::sync::LazyLock::new(|| {
        promote_ml_table(&build_decode_table_from_default(
            &ML_DEFAULT_DIST,
            ML_DEFAULT_ACCURACY,
        ))
    });

#[cfg(feature = "std")]
static OF_PREDEFINED: std::sync::LazyLock<Vec<FseSeqDecodeEntry>> =
    std::sync::LazyLock::new(|| {
        promote_of_table(&build_decode_table_from_default(
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
    let ll_mode = (mode_byte >> 6) & 0x03;
    let of_mode = (mode_byte >> 4) & 0x03;
    let ml_mode = (mode_byte >> 2) & 0x03;

    let mut reader = BitReader::new(&data[1..]);

    match ll_mode {
        0 => {
            #[cfg(feature = "std")]
            {
                prev.ll_table.clear();
                prev.ll_table.extend_from_slice(&LL_PREDEFINED);
            }
            #[cfg(not(feature = "std"))]
            {
                prev.ll_table = promote_ll_table(&build_decode_table_from_default(
                    &LL_DEFAULT_DIST,
                    LL_DEFAULT_ACCURACY,
                ));
            }
            prev.ll_accuracy = LL_DEFAULT_ACCURACY;
        }
        1 => {
            let sym = reader.read_bits(8)? as usize;
            if sym >= LL_BITS_TABLE.len() {
                return Err(DecompressError::CorruptSequences);
            }
            prev.ll_table.clear();
            prev.ll_table.push(FseSeqDecodeEntry {
                base_line: 0,
                num_bits: 0,
                extra_bits: LL_BITS_TABLE[sym],
                baseline_value: LL_BASELINE_TABLE[sym],
            });
            prev.ll_accuracy = 0;
        }
        2 => {
            let acc = parse_fse_table_description_into(&mut reader, 35, &mut ws.fse_dist)?;
            build_decode_table_into(
                &ws.fse_dist,
                acc,
                &mut ws.fse_build_buf,
                &mut ws.fse_symbol_next,
            )?;
            prev.ll_table = promote_ll_table(&ws.fse_build_buf);
            prev.ll_accuracy = acc;
        }
        _ => {}
    }

    match of_mode {
        0 => {
            #[cfg(feature = "std")]
            {
                prev.of_table.clear();
                prev.of_table.extend_from_slice(&OF_PREDEFINED);
            }
            #[cfg(not(feature = "std"))]
            {
                prev.of_table = promote_of_table(&build_decode_table_from_default(
                    &OF_DEFAULT_DIST,
                    OF_DEFAULT_ACCURACY,
                ));
            }
            prev.of_accuracy = OF_DEFAULT_ACCURACY;
        }
        1 => {
            let sym = reader.read_bits(8)? as u8;
            if sym > 31 {
                return Err(DecompressError::CorruptSequences);
            }
            prev.of_table.clear();
            prev.of_table.push(FseSeqDecodeEntry {
                base_line: 0,
                num_bits: 0,
                extra_bits: sym,
                baseline_value: 1u32 << sym,
            });
            prev.of_accuracy = 0;
        }
        2 => {
            let acc = parse_fse_table_description_into(&mut reader, 31, &mut ws.fse_dist)?;
            build_decode_table_into(
                &ws.fse_dist,
                acc,
                &mut ws.fse_build_buf,
                &mut ws.fse_symbol_next,
            )?;
            prev.of_table = promote_of_table(&ws.fse_build_buf);
            prev.of_accuracy = acc;
        }
        _ => {}
    }

    match ml_mode {
        0 => {
            #[cfg(feature = "std")]
            {
                prev.ml_table.clear();
                prev.ml_table.extend_from_slice(&ML_PREDEFINED);
            }
            #[cfg(not(feature = "std"))]
            {
                prev.ml_table = promote_ml_table(&build_decode_table_from_default(
                    &ML_DEFAULT_DIST,
                    ML_DEFAULT_ACCURACY,
                ));
            }
            prev.ml_accuracy = ML_DEFAULT_ACCURACY;
        }
        1 => {
            let sym = reader.read_bits(8)? as usize;
            if sym >= ML_BITS_TABLE.len() {
                return Err(DecompressError::CorruptSequences);
            }
            prev.ml_table.clear();
            prev.ml_table.push(FseSeqDecodeEntry {
                base_line: 0,
                num_bits: 0,
                extra_bits: ML_BITS_TABLE[sym],
                baseline_value: ML_BASELINE_TABLE[sym],
            });
            prev.ml_accuracy = 0;
        }
        2 => {
            let acc = parse_fse_table_description_into(&mut reader, 52, &mut ws.fse_dist)?;
            build_decode_table_into(
                &ws.fse_dist,
                acc,
                &mut ws.fse_build_buf,
                &mut ws.fse_symbol_next,
            )?;
            prev.ml_table = promote_ml_table(&ws.fse_build_buf);
            prev.ml_accuracy = acc;
        }
        _ => {}
    }

    Ok(1 + reader.bytes_consumed())
}
