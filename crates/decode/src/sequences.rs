//! Sequences section header: the sequence count and the literal length,
//! offset and match length table modes, with a cache for repeated table
//! descriptions.
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::boxed::Box;

use crate::BlockDecodeWorkspace;
use crate::seq_table::SeqTable;
use zrip_core::bitstream::reader::BitReader;
use zrip_core::error::DecompressError;
use zrip_core::fse::table_builder::{
    build_decode_table_from_default, parse_fse_table_description_into,
};
use zrip_core::fse::{
    FseSeqDecodeEntry, LL_BASELINE_TABLE, LL_BITS_TABLE, LL_DEFAULT_ACCURACY, LL_DEFAULT_DIST,
    LL_MAX_ACCURACY_LOG, ML_BASELINE_TABLE, ML_BITS_TABLE, ML_DEFAULT_ACCURACY, ML_DEFAULT_DIST,
    ML_MAX_ACCURACY_LOG, OF_DEFAULT_ACCURACY, OF_DEFAULT_DIST, OF_MAX_ACCURACY_LOG,
};
use zrip_core::hint::likely;

/// The literal length, offset and match length tables, kept across blocks
/// for repeat mode.
pub(crate) struct SequenceDecodeTables {
    /// Literal length table.
    pub(crate) ll_table: SeqTable,
    /// Accuracy log of `ll_table`: 0 in RLE mode, at most 9.
    pub(crate) ll_accuracy: u8,
    /// Whether `ll_table` is the predefined table, so predefined mode can
    /// skip the copy.
    pub(crate) ll_kind: SequenceTableKind,
    /// Offset code table.
    pub(crate) of_table: SeqTable,
    /// Accuracy log of `of_table`: 0 in RLE mode, at most 8.
    pub(crate) of_accuracy: u8,
    /// As `ll_kind`, for `of_table`.
    pub(crate) of_kind: SequenceTableKind,
    /// Match length table.
    pub(crate) ml_table: SeqTable,
    /// Accuracy log of `ml_table`: 0 in RLE mode, at most 9.
    pub(crate) ml_accuracy: u8,
    /// As `ll_kind`, for `ml_table`.
    pub(crate) ml_kind: SequenceTableKind,
    /// `ll_table` was set in this frame or by its dictionary, so repeat
    /// mode may use it.
    pub(crate) ll_set: bool,
    /// As `ll_set`, for `of_table`.
    pub(crate) of_set: bool,
    /// As `ll_set`, for `ml_table`.
    pub(crate) ml_set: bool,
}

impl Clone for SequenceDecodeTables {
    fn clone(&self) -> Self {
        Self {
            ll_table: self.ll_table.clone(),
            ll_accuracy: self.ll_accuracy,
            ll_kind: self.ll_kind,
            of_table: self.of_table.clone(),
            of_accuracy: self.of_accuracy,
            of_kind: self.of_kind,
            ml_table: self.ml_table.clone(),
            ml_accuracy: self.ml_accuracy,
            ml_kind: self.ml_kind,
            ll_set: self.ll_set,
            of_set: self.of_set,
            ml_set: self.ml_set,
        }
    }

    /// Copies the tables in place through `SeqTable::clone_from`. The
    /// derived version would build a full temporary copy first.
    fn clone_from(&mut self, source: &Self) {
        self.ll_table.clone_from(&source.ll_table);
        self.ll_accuracy = source.ll_accuracy;
        self.ll_kind = source.ll_kind;
        self.of_table.clone_from(&source.of_table);
        self.of_accuracy = source.of_accuracy;
        self.of_kind = source.of_kind;
        self.ml_table.clone_from(&source.ml_table);
        self.ml_accuracy = source.ml_accuracy;
        self.ml_kind = source.ml_kind;
        self.ll_set = source.ll_set;
        self.of_set = source.of_set;
        self.ml_set = source.ml_set;
    }
}

/// What a sequence table holds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SequenceTableKind {
    /// The RFC 8878 predefined distribution.
    Predefined,
    /// An RLE, FSE-compressed or dictionary table.
    Other,
}

impl SequenceDecodeTables {
    /// Predefined tables, none set for repeat mode.
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

    /// Restores the predefined tables, copying only those that changed, and
    /// clears the repeat flags.
    pub(crate) fn reset_default(&mut self) {
        if self.ll_kind != SequenceTableKind::Predefined {
            #[cfg(feature = "std")]
            {
                self.ll_table.clone_from(&LL_PREDEFINED);
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
                self.of_table.clone_from(&OF_PREDEFINED);
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
                self.ml_table.clone_from(&ML_PREDEFINED);
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

    /// Makes repeat mode fail until a block sets each table. Keeps the
    /// tables.
    pub(crate) fn clear_repeat_flags(&mut self) {
        self.ll_set = false;
        self.of_set = false;
        self.ml_set = false;
    }

    /// Installs the sequence tables `dict` carries. Marks them as not
    /// predefined, so Predefined_Mode reloads the predefined table.
    pub(crate) fn load_dict(&mut self, dict: &zrip_core::dict::Dictionary) {
        if let Some((t, l)) = dict.of_table() {
            self.of_table = SeqTable::promote_of(t);
            self.of_accuracy = l;
            self.of_kind = SequenceTableKind::Other;
            self.of_set = true;
        }
        if let Some((t, l)) = dict.ml_table() {
            self.ml_table = SeqTable::promote_ml(t);
            self.ml_accuracy = l;
            self.ml_kind = SequenceTableKind::Other;
            self.ml_set = true;
        }
        if let Some((t, l)) = dict.ll_table() {
            self.ll_table = SeqTable::promote_ll(t);
            self.ll_accuracy = l;
            self.ll_kind = SequenceTableKind::Other;
            self.ll_set = true;
        }
    }
}

/// Parses `Number_of_Sequences`. Returns the count and its size, 1 to 3
/// bytes.
pub(crate) fn parse_sequence_count(data: &[u8]) -> Result<(u32, usize), DecompressError> {
    if data.is_empty() {
        return Err(DecompressError::CorruptSequences);
    }

    // Below 128: the count. Below 255: a 15-bit count with the next byte.
    // 255: `0x7F00` plus the next two bytes, little-endian.
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

/// Turns an offset value into an offset and updates the repeat offsets,
/// as `SeqDecoder::decode` in `exec.rs` does for the sequence loop.
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

// Predefined tables, built once per process. `no_std` builds rebuild them
// when needed.
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

/// Parses the compression modes byte and the table descriptions after it,
/// updating `prev`. Returns the bytes consumed.
///
/// Kept out of line and compiled for x86-64-v3 when the CPU has it, as
/// [`decode_literals_ws`](crate::literals::decode_literals_ws) is.
#[cfg_attr(
    all(
        feature = "std",
        feature = "simd",
        target_arch = "x86_64",
        not(target_feature = "avx2")
    ),
    inline(never)
)]
pub(crate) fn parse_sequence_tables_ws(
    data: &[u8],
    prev: &mut SequenceDecodeTables,
    ws: &mut BlockDecodeWorkspace,
) -> Result<usize, DecompressError> {
    simd_body!(parse_sequence_tables_ws_impl(data, prev, ws))
}

#[inline(always)]
fn parse_sequence_tables_ws_impl(
    data: &[u8],
    prev: &mut SequenceDecodeTables,
    ws: &mut BlockDecodeWorkspace,
) -> Result<usize, DecompressError> {
    if data.is_empty() {
        return Err(DecompressError::CorruptSequences);
    }

    // Modes: literal lengths in bits 7-6, offsets in 5-4, match lengths in
    // 3-2. Bits 1-0 are reserved and must be 0. Each mode is 0 predefined,
    // 1 RLE, 2 FSE-compressed, or 3 repeat.
    let mode_byte = data[0];
    if mode_byte & 0x03 != 0 {
        return Err(DecompressError::CorruptSequences);
    }
    let ll_mode = (mode_byte >> 6) & 0x03;
    let of_mode = (mode_byte >> 4) & 0x03;
    let ml_mode = (mode_byte >> 2) & 0x03;
    // Without repeat mode, the tables depend only on these bytes, so the
    // bytes can key a cache.
    let cacheable = ll_mode != 3 && of_mode != 3 && ml_mode != 3;
    // Any mode but repeat replaces a table, so the tables no longer match
    // the dictionary's.
    if ll_mode != 3 || of_mode != 3 || ml_mode != 3 {
        ws.seq_is_dict = false;
    }

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

    // From here on the tables change. A description that fails partway
    // leaves them unlike the cached ones, so a later hit must copy those.
    ws.seq_table_cache_tables_current = false;

    let mut reader = BitReader::new(&data[1..]);

    match ll_mode {
        0 => {
            if prev.ll_kind != SequenceTableKind::Predefined {
                #[cfg(feature = "std")]
                {
                    prev.ll_table.clone_from(&LL_PREDEFINED);
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
            prev.ll_table.set_single(FseSeqDecodeEntry {
                base_line: 0,
                num_bits: 0,
                extra_bits: LL_BITS_TABLE[sym],
                baseline_value: LL_BASELINE_TABLE[sym],
            });
            prev.ll_accuracy = 0;
            prev.ll_kind = SequenceTableKind::Other;
            prev.ll_set = true;
        }
        2 => {
            // 35 is the largest literal length code.
            let acc = parse_fse_table_description_into(&mut reader, 35, &mut ws.fse_dist)?;
            if acc > LL_MAX_ACCURACY_LOG {
                return Err(DecompressError::BadFseTable);
            }
            prev.ll_table.build_ll(&ws.fse_dist, acc)?;
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
                    prev.of_table.clone_from(&OF_PREDEFINED);
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
            // 31 is the largest offset code, as in C zstd.
            if sym > 31 {
                return Err(DecompressError::CorruptSequences);
            }
            prev.of_table.set_single(FseSeqDecodeEntry {
                base_line: 0,
                num_bits: 0,
                extra_bits: sym,
                baseline_value: 1u32 << sym,
            });
            prev.of_accuracy = 0;
            prev.of_kind = SequenceTableKind::Other;
            prev.of_set = true;
        }
        2 => {
            // 31 is the largest offset code.
            let acc = parse_fse_table_description_into(&mut reader, 31, &mut ws.fse_dist)?;
            if acc > OF_MAX_ACCURACY_LOG {
                return Err(DecompressError::BadFseTable);
            }
            prev.of_table.build_of(&ws.fse_dist, acc)?;
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
                    prev.ml_table.clone_from(&ML_PREDEFINED);
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
            prev.ml_table.set_single(FseSeqDecodeEntry {
                base_line: 0,
                num_bits: 0,
                extra_bits: ML_BITS_TABLE[sym],
                baseline_value: ML_BASELINE_TABLE[sym],
            });
            prev.ml_accuracy = 0;
            prev.ml_kind = SequenceTableKind::Other;
            prev.ml_set = true;
        }
        2 => {
            // 52 is the largest match length code.
            let acc = parse_fse_table_description_into(&mut reader, 52, &mut ws.fse_dist)?;
            if acc > ML_MAX_ACCURACY_LOG {
                return Err(DecompressError::BadFseTable);
            }
            prev.ml_table.build_ml(&ws.fse_dist, acc)?;
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
        match &mut ws.seq_table_cache {
            Some(cache) => cache.as_mut().clone_from(prev),
            None => ws.seq_table_cache = Some(Box::new(prev.clone())),
        }
        ws.seq_table_cache_tables_current = true;
    }

    Ok(consumed)
}
