#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::fse::FseDecodeEntry;
use crate::fse::table_builder::{
    build_decode_table, normalize_counts, serialize_fse_table_description,
};
use crate::huffman::{MAX_BITS, MAX_SYMBOL_VALUE};

pub struct HuffmanEncodeTable {
    codes: [u16; MAX_SYMBOL_VALUE + 1],
    num_bits: [u8; MAX_SYMBOL_VALUE + 1],
    weights: Vec<u8>,
    max_symbol: u8,
    table_log: u8,
}

#[cfg(feature = "alloc")]
impl HuffmanEncodeTable {
    pub fn from_data(data: &[u8]) -> Option<Self> {
        if data.is_empty() {
            return None;
        }

        let mut freqs = [0u32; MAX_SYMBOL_VALUE + 1];
        let mut max_sym = 0u8;
        for &b in data {
            freqs[b as usize] += 1;
            if b > max_sym {
                max_sym = b;
            }
        }

        let num_symbols = max_sym as usize + 1;
        let active_count = freqs[..num_symbols].iter().filter(|&&f| f > 0).count();
        if active_count < 2 {
            return None;
        }

        let (weights, table_log) = compute_huffman_weights(&freqs, num_symbols)?;
        let (codes, num_bits) = build_encode_codes(&weights, table_log);

        if max_sym as usize > 128 {
            let explicit = &weights[..max_sym as usize];
            if serialize_weights_fse(explicit).is_none() {
                return None;
            }
        }

        Some(Self {
            codes,
            num_bits,
            weights,
            max_symbol: max_sym,
            table_log,
        })
    }

    pub fn table_log(&self) -> u8 {
        self.table_log
    }

    pub fn can_encode(&self, data: &[u8]) -> bool {
        for &b in data {
            if self.num_bits[b as usize] == 0 {
                return false;
            }
        }
        true
    }

    pub fn serialize_weights(&self) -> Vec<u8> {
        let explicit = &self.weights[..self.max_symbol as usize];
        let num_symbols = explicit.len();

        if num_symbols <= 128 {
            let mut out = Vec::with_capacity(1 + (num_symbols + 1) / 2);
            out.push((num_symbols + 127) as u8);
            let num_bytes = (num_symbols + 1) / 2;
            for i in 0..num_bytes {
                let hi = explicit.get(i * 2).copied().unwrap_or(0);
                let lo = explicit.get(i * 2 + 1).copied().unwrap_or(0);
                out.push((hi << 4) | lo);
            }
            out
        } else {
            serialize_weights_fse(explicit).expect("FSE weight encoding verified in from_data")
        }
    }

    pub fn encode_single_stream(&self, data: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(data.len() + 8);
        self.encode_single_stream_into(data, &mut buf);
        buf
    }

    pub fn encode_single_stream_into(&self, data: &[u8], buf: &mut Vec<u8>) {
        buf.clear();
        let tl = self.table_log as usize;
        let unroll: usize = if tl > 0 { 32 / tl } else { 1 }.max(2);

        buf.reserve(data.len() + 16);
        let mut bits: u64 = 0;
        let mut bits_used: u8 = 0;
        let mut wpos: usize = 0;
        let codes = self.codes.as_ptr();
        let nbits = self.num_bits.as_ptr();

        macro_rules! flush_bits {
            () => {
                if wpos + 8 > buf.capacity() {
                    unsafe {
                        buf.set_len(wpos);
                    }
                    buf.reserve(64);
                }
                unsafe {
                    let ptr = buf.as_mut_ptr().add(wpos);
                    (ptr as *mut u64).write_unaligned(bits.to_le());
                    let nb = (bits_used >> 3) as usize;
                    wpos += nb;
                    bits >>= nb << 3;
                    bits_used &= 7;
                }
            };
        }

        let mut pos = data.len();
        while pos >= unroll {
            pos -= unroll;
            for j in 0..unroll {
                let b = unsafe { *data.get_unchecked(pos + (unroll - 1 - j)) };
                let c = unsafe { *codes.add(b as usize) } as u64;
                let n = unsafe { *nbits.add(b as usize) };
                bits |= c << bits_used;
                bits_used += n;
            }
            if bits_used >= 32 {
                flush_bits!();
            }
        }
        while pos > 0 {
            pos -= 1;
            let b = unsafe { *data.get_unchecked(pos) };
            let c = unsafe { *codes.add(b as usize) } as u64;
            let n = unsafe { *nbits.add(b as usize) };
            bits |= c << bits_used;
            bits_used += n;
            if bits_used >= 32 {
                flush_bits!();
            }
        }

        unsafe {
            buf.set_len(wpos);
        }
        bits |= 1u64 << bits_used;
        bits_used += 1;
        while bits_used > 0 {
            buf.push(bits as u8);
            bits >>= 8;
            bits_used = bits_used.saturating_sub(8);
        }
    }

    pub fn encode_4_streams(&self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_4_streams_into(data, &mut out, &mut Vec::new());
        out
    }

    pub fn encode_4_streams_into(&self, data: &[u8], out: &mut Vec<u8>, stream_buf: &mut Vec<u8>) {
        let seg = (data.len() + 3) / 4;
        let s1 = &data[..seg.min(data.len())];
        let s2 = &data[seg.min(data.len())..(seg * 2).min(data.len())];
        let s3 = &data[(seg * 2).min(data.len())..(seg * 3).min(data.len())];
        let s4 = &data[(seg * 3).min(data.len())..];

        out.clear();
        out.extend_from_slice(&[0u8; 6]);

        self.encode_single_stream_into(s1, stream_buf);
        let e1_len = stream_buf.len();
        out.extend_from_slice(stream_buf);

        self.encode_single_stream_into(s2, stream_buf);
        let e2_len = stream_buf.len();
        out.extend_from_slice(stream_buf);

        self.encode_single_stream_into(s3, stream_buf);
        let e3_len = stream_buf.len();
        out.extend_from_slice(stream_buf);

        self.encode_single_stream_into(s4, stream_buf);
        out.extend_from_slice(stream_buf);

        out[0..2].copy_from_slice(&(e1_len as u16).to_le_bytes());
        out[2..4].copy_from_slice(&(e2_len as u16).to_le_bytes());
        out[4..6].copy_from_slice(&(e3_len as u16).to_le_bytes());
    }

    pub fn compressed_size_single(&self, data: &[u8]) -> usize {
        let total_bits: usize = data
            .iter()
            .map(|&b| self.num_bits[b as usize] as usize)
            .sum();
        (total_bits + 8) / 8
    }
}

fn compute_huffman_weights(freqs: &[u32], num_symbols: usize) -> Option<(Vec<u8>, u8)> {
    use alloc::collections::BinaryHeap;
    use core::cmp::Reverse;

    let active: Vec<(u64, usize)> = freqs[..num_symbols]
        .iter()
        .enumerate()
        .filter(|(_, f)| **f > 0)
        .map(|(s, &f)| (f as u64, s))
        .collect();

    if active.len() < 2 {
        return None;
    }

    let n = active.len();

    let max_nodes = 2 * n;
    let mut parent = vec![usize::MAX; max_nodes];

    let mut heap: BinaryHeap<Reverse<(u64, usize)>> = BinaryHeap::with_capacity(n);
    for (i, &(f, _)) in active.iter().enumerate() {
        heap.push(Reverse((f, i)));
    }

    let mut next_id = n;
    for _ in 0..n - 1 {
        let Reverse((f1, n1)) = heap.pop().unwrap();
        let Reverse((f2, n2)) = heap.pop().unwrap();
        parent[n1] = next_id;
        parent[n2] = next_id;
        heap.push(Reverse((f1 + f2, next_id)));
        next_id += 1;
    }

    let mut bit_lengths = vec![0u8; num_symbols];
    for i in 0..n {
        let sym = active[i].1;
        let mut depth = 0u8;
        let mut node = i;
        while parent[node] != usize::MAX {
            depth += 1;
            node = parent[node];
        }
        bit_lengths[sym] = depth;
    }

    let max_bl = *bit_lengths.iter().max().unwrap();
    if max_bl == 0 || max_bl > MAX_BITS {
        return None;
    }

    let table_log = max_bl;
    let mut weights = vec![0u8; num_symbols];
    for (s, &bl) in bit_lengths.iter().enumerate() {
        if bl > 0 {
            weights[s] = table_log + 1 - bl;
        }
    }

    Some((weights, table_log))
}

fn build_encode_codes(
    weights: &[u8],
    table_log: u8,
) -> ([u16; MAX_SYMBOL_VALUE + 1], [u8; MAX_SYMBOL_VALUE + 1]) {
    let mut codes = [0u16; MAX_SYMBOL_VALUE + 1];
    let mut num_bits = [0u8; MAX_SYMBOL_VALUE + 1];

    let max_w = table_log + 1;
    let mut rank_count = [0u32; MAX_BITS as usize + 2];

    for (s, &w) in weights.iter().enumerate() {
        if w > 0 && w <= max_w {
            num_bits[s] = table_log + 1 - w;
            rank_count[w as usize] += 1;
        }
    }

    let mut rank_start = [0u32; MAX_BITS as usize + 2];
    let mut cumul = 0u32;
    for w in 1..=max_w {
        rank_start[w as usize] = cumul;
        cumul += rank_count[w as usize] * (1u32 << (w - 1));
    }

    for (s, &w) in weights.iter().enumerate() {
        if w == 0 {
            continue;
        }
        let start = rank_start[w as usize];
        codes[s] = (start >> (w - 1)) as u16;
        rank_start[w as usize] += 1u32 << (w - 1);
    }

    (codes, num_bits)
}

#[derive(Clone, Copy)]
struct WeightSymTT {
    delta_nb_bits: u32,
    delta_find_state: i32,
}

struct WeightFseEnc {
    symbol_tt: Vec<WeightSymTT>,
    state_table: Vec<u16>,
    table_log: u8,
}

impl WeightFseEnc {
    fn build(
        decode_table: &[FseDecodeEntry],
        table_size: usize,
        max_sym: usize,
        table_log: u8,
    ) -> Self {
        let num_symbols = max_sym + 1;
        let mut count = vec![0u32; num_symbols];
        for i in 0..table_size {
            count[decode_table[i].symbol as usize] += 1;
        }

        let mut symbol_tt = vec![
            WeightSymTT {
                delta_nb_bits: (table_log as u32 + 1) << 16,
                delta_find_state: 0,
            };
            num_symbols
        ];
        let mut total = 0i32;
        for s in 0..num_symbols {
            let c = count[s];
            if c == 0 {
                symbol_tt[s].delta_nb_bits = ((table_log as u32 + 1) << 16) | (1u32 << table_log);
                continue;
            }
            let max_bits_out = if c == 1 {
                table_log as u32
            } else {
                table_log as u32 - (31 - (c - 1).leading_zeros())
            };
            let min_state_plus = c << max_bits_out;
            symbol_tt[s].delta_nb_bits = (max_bits_out << 16).wrapping_sub(min_state_plus);
            symbol_tt[s].delta_find_state = total - c as i32;
            total += c as i32;
        }

        let mut cumul = vec![0u32; num_symbols + 1];
        for s in 0..num_symbols {
            cumul[s + 1] = cumul[s] + count[s];
        }

        let mut state_table = vec![0u16; table_size];
        let mut cumul_copy = cumul.clone();
        for i in 0..table_size {
            let s = decode_table[i].symbol as usize;
            let idx = cumul_copy[s] as usize;
            state_table[idx] = (table_size + i) as u16;
            cumul_copy[s] += 1;
        }

        Self {
            symbol_tt,
            state_table,
            table_log,
        }
    }

    fn init_state(&self, symbol: u8) -> u32 {
        let tt = &self.symbol_tt[symbol as usize];
        let nb_bits_out = tt.delta_nb_bits.wrapping_add(1 << 15) >> 16;
        let base_state = (nb_bits_out << 16).wrapping_sub(tt.delta_nb_bits);
        let idx = (base_state >> nb_bits_out) as i32 + tt.delta_find_state;
        self.state_table[idx as usize] as u32
    }
}

fn serialize_weights_fse(weights: &[u8]) -> Option<Vec<u8>> {
    let n = weights.len();
    if n < 2 {
        return None;
    }

    let max_w = *weights.iter().max().unwrap() as usize;
    let mut freqs = vec![0u32; max_w + 1];
    for &w in weights {
        freqs[w as usize] += 1;
    }

    let distinct = freqs.iter().filter(|&&f| f > 0).count();
    if distinct <= 1 {
        return None;
    }

    let min_log = if max_w > 0 {
        (32 - (max_w as u32).leading_zeros()) as u8 + 2
    } else {
        5
    };
    let acc_log = min_log.max(5).min(7);

    let dist = normalize_counts(&freqs, acc_log);
    for (i, &f) in freqs.iter().enumerate() {
        if f > 0 && dist[i] == 0 {
            return None;
        }
    }

    let table_desc = serialize_fse_table_description(&dist, acc_log);
    let decode_table = build_decode_table(&dist, acc_log).ok()?;
    let table_size = 1usize << acc_log;
    let enc = WeightFseEnc::build(&decode_table, table_size, max_w, acc_log);

    let fse_stream = encode_weights_interleaved(weights, &enc)?;

    let compressed_size = table_desc.len() + fse_stream.len();
    if compressed_size > 127 {
        return None;
    }

    let mut out = Vec::with_capacity(1 + compressed_size);
    out.push(compressed_size as u8);
    out.extend_from_slice(&table_desc);
    out.extend_from_slice(&fse_stream);

    Some(out)
}

fn encode_weights_interleaved(weights: &[u8], enc: &WeightFseEnc) -> Option<Vec<u8>> {
    use crate::bitstream::writer::BitWriter;

    let n = weights.len();
    if n < 2 {
        return None;
    }

    let acc_log = enc.table_log;
    let table_size = 1u32 << acc_log;

    let last_s1 = if n % 2 != 0 { n - 1 } else { n - 2 };
    let last_s2 = if n % 2 != 0 { n - 2 } else { n - 1 };

    let mut state1 = enc.init_state(weights[last_s1]);
    let mut state2 = enc.init_state(weights[last_s2]);

    let mut writer = BitWriter::with_capacity(n * 2 + 16);

    if n > 2 {
        for k in (0..=(n - 3)).rev() {
            let w = weights[k];
            let state = if k % 2 == 0 { &mut state1 } else { &mut state2 };

            let tt = &enc.symbol_tt[w as usize];
            let nb = ((tt.delta_nb_bits.wrapping_add(*state)) >> 16) as u8;
            writer.write_bits(*state & ((1u32 << nb) - 1), nb);
            let idx = (*state >> nb as u32) as i32 + tt.delta_find_state;
            *state = enc.state_table[idx as usize] as u32;
        }
    }

    writer.write_bits(state2 - table_size, acc_log);
    writer.write_bits(state1 - table_size, acc_log);
    writer.close_reverse_stream();

    Some(writer.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_simple() {
        let data = b"hello world hello world hello world!";
        let table = HuffmanEncodeTable::from_data(data).unwrap();
        let weights_raw = table.serialize_weights();
        let encoded = table.encode_single_stream(data);

        let (parsed_weights, _) =
            crate::huffman::weights::parse_huffman_weights(&weights_raw).unwrap();
        let (decode_table, decode_log) =
            crate::huffman::weights::build_huffman_decode_table(&parsed_weights).unwrap();
        let decoded = crate::huffman::decode::decode_single_stream(
            &decode_table,
            decode_log,
            &encoded,
            data.len(),
        )
        .unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn roundtrip_4_streams() {
        let data: Vec<u8> = b"ABCDEFGH".iter().cycle().take(1024).copied().collect();
        let table = HuffmanEncodeTable::from_data(&data).unwrap();
        let weights_raw = table.serialize_weights();
        let encoded = table.encode_4_streams(&data);

        let (parsed_weights, _) =
            crate::huffman::weights::parse_huffman_weights(&weights_raw).unwrap();
        let (decode_table, decode_log) =
            crate::huffman::weights::build_huffman_decode_table(&parsed_weights).unwrap();
        let decoded = crate::huffman::decode::decode_4_streams(
            &decode_table,
            decode_log,
            &encoded,
            data.len(),
        )
        .unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn roundtrip_all_bytes() {
        let data: Vec<u8> = (0u8..=127).cycle().take(4096).collect();
        let table = HuffmanEncodeTable::from_data(&data).unwrap();
        let weights_raw = table.serialize_weights();
        let encoded = table.encode_single_stream(&data);

        let (parsed_weights, _) =
            crate::huffman::weights::parse_huffman_weights(&weights_raw).unwrap();
        let (decode_table, decode_log) =
            crate::huffman::weights::build_huffman_decode_table(&parsed_weights).unwrap();
        let decoded = crate::huffman::decode::decode_single_stream(
            &decode_table,
            decode_log,
            &encoded,
            data.len(),
        )
        .unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn skewed_distribution() {
        let mut data = vec![0u8; 900];
        data.extend(vec![1u8; 80]);
        data.extend(vec![2u8; 15]);
        data.extend(vec![3u8; 5]);
        let table = HuffmanEncodeTable::from_data(&data).unwrap();
        assert!(table.num_bits[0] < table.num_bits[3]);
        let weights_raw = table.serialize_weights();
        let encoded = table.encode_single_stream(&data);

        let (parsed_weights, _) =
            crate::huffman::weights::parse_huffman_weights(&weights_raw).unwrap();
        let (decode_table, decode_log) =
            crate::huffman::weights::build_huffman_decode_table(&parsed_weights).unwrap();
        let decoded = crate::huffman::decode::decode_single_stream(
            &decode_table,
            decode_log,
            &encoded,
            data.len(),
        )
        .unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn roundtrip_high_symbol_fse_weights() {
        let mut data: Vec<u8> = (0u8..=200).cycle().take(8192).collect();
        data.extend(vec![0u8; 4096]);
        let table = HuffmanEncodeTable::from_data(&data).unwrap();
        assert!(table.max_symbol > 128);
        let weights_raw = table.serialize_weights();
        assert!(weights_raw[0] < 128, "header byte must indicate FSE path");
        let encoded = table.encode_4_streams(&data);

        let (parsed_weights, _) =
            crate::huffman::weights::parse_huffman_weights(&weights_raw).unwrap();
        let (decode_table, decode_log) =
            crate::huffman::weights::build_huffman_decode_table(&parsed_weights).unwrap();
        let decoded = crate::huffman::decode::decode_4_streams(
            &decode_table,
            decode_log,
            &encoded,
            data.len(),
        )
        .unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn roundtrip_255_symbols_fse_weights() {
        let mut data: Vec<u8> = (0u8..=255).cycle().take(8192).collect();
        data.extend(vec![0u8; 2048]);
        let table = HuffmanEncodeTable::from_data(&data).unwrap();
        assert_eq!(table.max_symbol, 255);
        let weights_raw = table.serialize_weights();
        assert!(weights_raw[0] < 128);
        let encoded = table.encode_4_streams(&data);

        let (parsed_weights, _) =
            crate::huffman::weights::parse_huffman_weights(&weights_raw).unwrap();
        let (decode_table, decode_log) =
            crate::huffman::weights::build_huffman_decode_table(&parsed_weights).unwrap();
        let decoded = crate::huffman::decode::decode_4_streams(
            &decode_table,
            decode_log,
            &encoded,
            data.len(),
        )
        .unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn two_symbols() {
        let mut data = vec![0u8; 500];
        data.extend(vec![1u8; 500]);
        let table = HuffmanEncodeTable::from_data(&data).unwrap();
        assert_eq!(table.num_bits[0], 1);
        assert_eq!(table.num_bits[1], 1);
        let encoded = table.encode_single_stream(&data);

        let weights_raw = table.serialize_weights();
        let (parsed_weights, _) =
            crate::huffman::weights::parse_huffman_weights(&weights_raw).unwrap();
        let (decode_table, decode_log) =
            crate::huffman::weights::build_huffman_decode_table(&parsed_weights).unwrap();
        let decoded = crate::huffman::decode::decode_single_stream(
            &decode_table,
            decode_log,
            &encoded,
            data.len(),
        )
        .unwrap();
        assert_eq!(decoded, data);
    }
}
