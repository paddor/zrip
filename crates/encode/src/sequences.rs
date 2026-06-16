#![forbid(unsafe_code)]
#![allow(dead_code)]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use zrip_core::Sequence;
use zrip_core::bitstream::writer::BitWriter;
use zrip_core::fse::{LL_BASELINE_TABLE, LL_BITS_TABLE, ML_BASELINE_TABLE, ML_BITS_TABLE};

pub fn encode_sequences_section(
    sequences: &[Sequence],
    rep_offsets: &[u32; 3],
    output: &mut Vec<u8>,
) {
    if sequences.is_empty() {
        output.push(0);
        return;
    }

    let real_seq_count = sequences
        .iter()
        .filter(|s| s.match_length > 0 || s.offset > 0)
        .count();

    encode_sequence_count(real_seq_count as u32, output);

    if real_seq_count == 0 {
        return;
    }

    output.push(0x00);

    let mut writer = BitWriter::new();

    let encoded: Vec<(u8, u8, u8, u32, u32, u32)> = sequences
        .iter()
        .filter(|s| s.match_length > 0 || s.offset > 0)
        .map(|seq| {
            let ll_code = ll_code(seq.literal_length);
            let ml_code = ml_code(seq.match_length);
            let (of_code, of_value) = of_code_value(seq.offset, seq.literal_length, rep_offsets);

            let ll_extra_bits = LL_BITS_TABLE[ll_code as usize];
            let ll_extra = seq.literal_length - LL_BASELINE_TABLE[ll_code as usize];
            let ml_extra_bits = ML_BITS_TABLE[ml_code as usize];
            let ml_extra = seq.match_length - ML_BASELINE_TABLE[ml_code as usize];

            (
                ll_code,
                ml_code,
                of_code,
                of_value,
                ll_extra | ((ll_extra_bits as u32) << 24),
                ml_extra | ((ml_extra_bits as u32) << 24),
            )
        })
        .collect();

    for (_i, &(ll_code, ml_code, of_code, of_value, ll_packed, ml_packed)) in
        encoded.iter().enumerate()
    {
        let of_bits = of_code;
        if of_bits > 0 {
            let of_extra = of_value & ((1u32 << of_bits) - 1);
            writer.write_bits(of_extra, of_bits);
        }

        let ml_extra_bits = (ml_packed >> 24) as u8;
        let ml_extra = ml_packed & 0xFFFFFF;
        if ml_extra_bits > 0 {
            writer.write_bits(ml_extra, ml_extra_bits);
        }

        let ll_extra_bits = (ll_packed >> 24) as u8;
        let ll_extra = ll_packed & 0xFFFFFF;
        if ll_extra_bits > 0 {
            writer.write_bits(ll_extra, ll_extra_bits);
        }

        writer.write_bits(ll_code as u32, 8);
        writer.write_bits(of_code as u32, 8);
        writer.write_bits(ml_code as u32, 8);
    }

    writer.close_reverse_stream();
    output.extend_from_slice(&writer.into_bytes());
}

fn encode_sequence_count(count: u32, output: &mut Vec<u8>) {
    if count < 128 {
        output.push(count as u8);
    } else if count < 0x7F00 {
        output.push(((count >> 8) + 128) as u8);
        output.push(count as u8);
    } else {
        output.push(0xFF);
        let adjusted = count - 0x7F00;
        output.push(adjusted as u8);
        output.push((adjusted >> 8) as u8);
    }
}

fn ll_code(ll: u32) -> u8 {
    if ll < 16 {
        return ll as u8;
    }
    let high = 31 - ll.leading_zeros();
    let code = (high - 1) as u8 + 16;
    code.min(35)
}

fn ml_code(ml: u32) -> u8 {
    if ml < 3 {
        return 0;
    }
    let ml = ml - 3;
    if ml < 32 {
        return ml as u8;
    }
    let high = 31 - ml.leading_zeros();
    let code = (high - 1) as u8 + 32;
    code.min(52)
}

fn of_code_value(offset: u32, _ll: u32, _rep: &[u32; 3]) -> (u8, u32) {
    if offset == 0 {
        return (0, 0);
    }
    let real_offset = offset + 3;
    let code = 31 - real_offset.leading_zeros();
    (code as u8, real_offset)
}
