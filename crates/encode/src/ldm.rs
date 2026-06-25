#![forbid(unsafe_code)]

use crate::dfast;
use crate::fast;
use crate::strategy::{LdmParams, LevelParams, Strategy};
use zrip_core::Sequence;
use zrip_core::xxhash::xxh64;

const LDM_BATCH_SIZE: usize = 64;

// Gear hash table from C zstd (BSD/GPLv2 licensed).
// 256 random u64 values for content-defined chunking.
#[allow(clippy::unreadable_literal)]
static GEAR_TABLE: [u64; 256] = [
    0xf5b8f72c5f77775c,
    0x84935f266b7ac412,
    0xb647ada9ca730ccc,
    0xb065bb4b114fb1de,
    0x34584e7e8c3a9fd0,
    0x4e97e17c6ae26b05,
    0x3a03d743bc99a604,
    0xcecd042422c4044f,
    0x76de76c58524259e,
    0x9c8528f65badeaca,
    0x86563706e2097529,
    0x2902475fa375d889,
    0xafb32a9739a5ebe6,
    0xce2714da3883e639,
    0x21eaf821722e69e,
    0x37b628620b628,
    0x49a8d455d88caf5,
    0x8556d711e6958140,
    0x4f7ae74fc605c1f,
    0x829f0c3468bd3a20,
    0x4ffdc885c625179e,
    0x8473de048a3daf1b,
    0x51008822b05646b2,
    0x69d75d12b2d1cc5f,
    0x8c9d4a19159154bc,
    0xc3cc10f4abbd4003,
    0xd06ddc1cecb97391,
    0xbe48e6e7ed80302e,
    0x3481db31cee03547,
    0xacc3f67cdaa1d210,
    0x65cb771d8c7f96cc,
    0x8eb27177055723dd,
    0xc789950d44cd94be,
    0x934feadc3700b12b,
    0x5e485f11edbdf182,
    0x1e2e2a46fd64767a,
    0x2969ca71d82efa7c,
    0x9d46e9935ebbba2e,
    0xe056b67e05e6822b,
    0x94d73f55739d03a0,
    0xcd7010bdb69b5a03,
    0x455ef9fcd79b82f4,
    0x869cb54a8749c161,
    0x38d1a4fa6185d225,
    0xb475166f94bbe9bb,
    0xa4143548720959f1,
    0x7aed4780ba6b26ba,
    0xd0ce264439e02312,
    0x84366d746078d508,
    0xa8ce973c72ed17be,
    0x21c323a29a430b01,
    0x9962d617e3af80ee,
    0xab0ce91d9c8cf75b,
    0x530e8ee6d19a4dbc,
    0x2ef68c0cf53f5d72,
    0xc03a681640a85506,
    0x496e4e9f9c310967,
    0x78580472b59b14a0,
    0x273824c23b388577,
    0x66bf923ad45cb553,
    0x47ae1a5a2492ba86,
    0x35e304569e229659,
    0x4765182a46870b6f,
    0x6cbab625e9099412,
    0xddac9a2e598522c1,
    0x7172086e666624f2,
    0xdf5003ca503b7837,
    0x88c0c1db78563d09,
    0x58d51865acfc289d,
    0x177671aec65224f1,
    0xfb79d8a241e967d7,
    0x2be1e101cad9a49a,
    0x6625682f6e29186b,
    0x399553457ac06e50,
    0x35dffb4c23abb74,
    0x429db2591f54aade,
    0xc52802a8037d1009,
    0x6acb27381f0b25f3,
    0xf45e2551ee4f823b,
    0x8b0ea2d99580c2f7,
    0x3bed519cbcb4e1e1,
    0xff452823dbb010a,
    0x9d42ed614f3dd267,
    0x5b9313c06257c57b,
    0xa114b8008b5e1442,
    0xc1fe311c11c13d4b,
    0x66e8763ea34c5568,
    0x8b982af1c262f05d,
    0xee8876faaa75fbb7,
    0x8a62a4d0d172bb2a,
    0xc13d94a3b7449a97,
    0x6dbbba9dc15d037c,
    0xc786101f1d92e0f1,
    0xd78681a907a0b79b,
    0xf61aaf2962c9abb9,
    0x2cfd16fcd3cb7ad9,
    0x868c5b6744624d21,
    0x25e650899c74ddd7,
    0xba042af4a7c37463,
    0x4eb1a539465a3eca,
    0xbe09dbf03b05d5ca,
    0x774e5a362b5472ba,
    0x47a1221229d183cd,
    0x504b0ca18ef5a2df,
    0xdffbdfbde2456eb9,
    0x46cd2b2fbee34634,
    0xf2aef8fe819d98c3,
    0x357f5276d4599d61,
    0x24a5483879c453e3,
    0x88026889192b4b9,
    0x28da96671782dbec,
    0x4ef37c40588e9aaa,
    0x8837b90651bc9fb3,
    0xc164f741d3f0e5d6,
    0xbc135a0a704b70ba,
    0x69cd868f7622ada,
    0xbc37ba89e0b9c0ab,
    0x47c14a01323552f6,
    0x4f00794bacee98bb,
    0x7107de7d637a69d5,
    0x88af793bb6f2255e,
    0xf3c6466b8799b598,
    0xc288c616aa7f3b59,
    0x81ca63cf42fca3fd,
    0x88d85ace36a2674b,
    0xd056bd3792389e7,
    0xe55c396c4e9dd32d,
    0xbefb504571e6c0a6,
    0x96ab32115e91e8cc,
    0xbf8acb18de8f38d1,
    0x66dae58801672606,
    0x833b6017872317fb,
    0xb87c16f2d1c92864,
    0xdb766a74e58b669c,
    0x89659f85c61417be,
    0xc8daad856011ea0c,
    0x76a4b565b6fe7eae,
    0xa469d085f6237312,
    0xaaf0365683a3e96c,
    0x4dbb746f8424f7b8,
    0x638755af4e4acc1,
    0x3d7807f5bde64486,
    0x17be6d8f5bbb7639,
    0x903f0cd44dc35dc,
    0x67b672eafdf1196c,
    0xa676ff93ed4c82f1,
    0x521d1004c5053d9d,
    0x37ba9ad09ccc9202,
    0x84e54d297aacfb51,
    0xa0b4b776a143445,
    0x820d471e20b348e,
    0x1874383cb83d46dc,
    0x97edeec7a1efe11c,
    0xb330e50b1bdc42aa,
    0x1dd91955ce70e032,
    0xa514cdb88f2939d5,
    0x2791233fd90db9d3,
    0x7b670a4cc50f7a9b,
    0x77c07d2a05c6dfa5,
    0xe3778b6646d0a6fa,
    0xb39c8eda47b56749,
    0x933ed448addbef28,
    0xaf846af6ab7d0bf4,
    0xe5af208eb666e49,
    0x5e6622f73534cd6a,
    0x297daeca42ef5b6e,
    0x862daef3d35539a6,
    0xe68722498f8e1ea9,
    0x981c53093dc0d572,
    0xfa09b0bfbf86fbf5,
    0x30b1e96166219f15,
    0x70e7d466bdc4fb83,
    0x5a66736e35f2a8e9,
    0xcddb59d2b7c1baef,
    0xd6c7d247d26d8996,
    0xea4e39eac8de1ba3,
    0x539c8bb19fa3aff2,
    0x9f90e4c5fd508d8,
    0xa34e5956fbaf3385,
    0x2e2f8e151d3ef375,
    0x173691e9b83faec1,
    0xb85a8d56bf016379,
    0x8382381267408ae3,
    0xb90f901bbdc0096d,
    0x7c6ad32933bcec65,
    0x76bb5e2f2c8ad595,
    0x390f851a6cf46d28,
    0xc3e6064da1c2da72,
    0xc52a0c101cfa5389,
    0xd78eaf84a3fbc530,
    0x3781b9e2288b997e,
    0x73c2f6dea83d05c4,
    0x4228e364c5b5ed7,
    0x9d7a3edf0da43911,
    0x8edcfeda24686756,
    0x5e7667a7b7a9b3a1,
    0x4c4f389fa143791d,
    0xb08bc1023da7cddc,
    0x7ab4be3ae529b1cc,
    0x754e6132dbe74ff9,
    0x71635442a839df45,
    0x2f6fb1643fbe52de,
    0x961e0a42cf7a8177,
    0xf3b45d83d89ef2ea,
    0xee3de4cf4a6e3e9b,
    0xcd6848542c3295e7,
    0xe4cee1664c78662f,
    0x9947548b474c68c4,
    0x25d73777a5ed8b0b,
    0xc915b1d636b7fc,
    0x21c2ba75d9b0d2da,
    0x5f6b5dcf608a64a1,
    0xdcf333255ff9570c,
    0x633b922418ced4ee,
    0xc136dde0b004b34a,
    0x58cc83b05d4b2f5a,
    0x5eb424dda28e42d2,
    0x62df47369739cd98,
    0xb4e0b42485e4ce17,
    0x16e1f0c1f9a8d1e7,
    0x8ec3916707560ebf,
    0x62ba6e2df2cc9db3,
    0xcbf9f4ff77d83a16,
    0x78d9d7d07d2bbcc4,
    0xef554ce1e02c41f4,
    0x8d7581127eccf94d,
    0xa9b53336cb3c8a05,
    0x38c42c0bf45c4f91,
    0x640893cdf4488863,
    0x80ec34bc575ea568,
    0x39f324f5b48eaa40,
    0xe9d9ed1f8eff527f,
    0x9224fc058cc5a214,
    0xbaba00b04cfe7741,
    0x309a9f120fcf52af,
    0xa558f3ec65626212,
    0x424bec8b7adabe2f,
    0x41622513a6aea433,
    0xb88da2d5324ca798,
    0xd287733b245528a4,
    0x9a44697e6d68aec3,
    0x7b1093be2f49bb28,
    0x50bbec632e3d8aad,
    0x6cd90723e1ea8283,
    0x897b9e7431b02bf3,
    0x219efdcb338a7047,
    0x3b0311f0a27c0656,
    0xdb17bf91c0db96e7,
    0x8cd4fd6b4e85a5b2,
    0xfab071054ba6409d,
    0x40d6fe831fa9dfd9,
    0xaf358debad7d791e,
    0xeb8d0e25a65e3e58,
    0xbbcbd3df14e08580,
    0xcf751f27ecdab2b,
    0x2b4da14f2613d8f4,
];

#[derive(Clone, Copy, Default)]
struct LdmEntry {
    offset: u32,
    checksum: u32,
}

#[derive(Clone, Copy)]
struct RawLdmSeq {
    lit_length: u32,
    match_length: u32,
    offset: u32,
}

struct GearHash {
    rolling: u64,
    stop_mask: u64,
}

impl GearHash {
    fn new(params: &LdmParams) -> Self {
        let max_bits = (params.min_match_length as u64).min(64);
        let hrl = params.hash_rate_log;
        let stop_mask = if hrl > 0 && hrl <= max_bits as u32 {
            ((1u64 << hrl) - 1) << (max_bits as u32 - hrl)
        } else {
            (1u64 << hrl) - 1
        };
        Self {
            rolling: !0u32 as u64,
            stop_mask,
        }
    }

    fn reset(&mut self, data: &[u8]) {
        for &b in data {
            self.rolling = (self.rolling << 1).wrapping_add(GEAR_TABLE[b as usize]);
        }
    }

    fn feed(
        &mut self,
        data: &[u8],
        splits: &mut [usize; LDM_BATCH_SIZE],
        num_splits: &mut usize,
    ) -> usize {
        let mut n = 0;
        while n + 3 < data.len() {
            self.rolling = (self.rolling << 1).wrapping_add(GEAR_TABLE[data[n] as usize]);
            n += 1;
            if (self.rolling & self.stop_mask) == 0 {
                splits[*num_splits] = n;
                *num_splits += 1;
                if *num_splits == LDM_BATCH_SIZE {
                    return n;
                }
            }
            self.rolling = (self.rolling << 1).wrapping_add(GEAR_TABLE[data[n] as usize]);
            n += 1;
            if (self.rolling & self.stop_mask) == 0 {
                splits[*num_splits] = n;
                *num_splits += 1;
                if *num_splits == LDM_BATCH_SIZE {
                    return n;
                }
            }
            self.rolling = (self.rolling << 1).wrapping_add(GEAR_TABLE[data[n] as usize]);
            n += 1;
            if (self.rolling & self.stop_mask) == 0 {
                splits[*num_splits] = n;
                *num_splits += 1;
                if *num_splits == LDM_BATCH_SIZE {
                    return n;
                }
            }
            self.rolling = (self.rolling << 1).wrapping_add(GEAR_TABLE[data[n] as usize]);
            n += 1;
            if (self.rolling & self.stop_mask) == 0 {
                splits[*num_splits] = n;
                *num_splits += 1;
                if *num_splits == LDM_BATCH_SIZE {
                    return n;
                }
            }
        }
        while n < data.len() {
            self.rolling = (self.rolling << 1).wrapping_add(GEAR_TABLE[data[n] as usize]);
            n += 1;
            if (self.rolling & self.stop_mask) == 0 {
                splits[*num_splits] = n;
                *num_splits += 1;
                if *num_splits == LDM_BATCH_SIZE {
                    return n;
                }
            }
        }
        n
    }
}

pub struct LdmState {
    table: Vec<LdmEntry>,
    bucket_offsets: Vec<u8>,
    params: LdmParams,
    splits: [usize; LDM_BATCH_SIZE],
    raw_sequences: Vec<RawLdmSeq>,
    segment_sequences: Vec<Sequence>,
    entries_inserted: u32,
}

impl LdmState {
    pub fn new(params: &LdmParams) -> Self {
        let num_entries = 1usize << params.hash_log;
        let num_buckets = num_entries >> params.bucket_size_log;
        Self {
            table: vec![LdmEntry::default(); num_entries],
            bucket_offsets: vec![0u8; num_buckets],
            params: *params,
            splits: [0; LDM_BATCH_SIZE],
            raw_sequences: Vec::new(),
            segment_sequences: Vec::new(),
            entries_inserted: 0,
        }
    }

    pub fn reset(&mut self) {
        self.table.fill(LdmEntry::default());
        self.bucket_offsets.fill(0);
        self.raw_sequences.clear();
        self.entries_inserted = 0;
    }

    pub fn reduce_positions(&mut self, shift: u32) {
        if shift == 0 {
            return;
        }
        for entry in &mut self.table {
            if entry.offset == 0 || entry.offset < shift {
                *entry = LdmEntry::default();
            } else {
                entry.offset -= shift;
            }
        }
    }

    fn h_bits(&self) -> u32 {
        self.params.hash_log - self.params.bucket_size_log
    }

    fn insert_entry(&mut self, hash: u32, entry: LdmEntry) {
        let bucket_start = (hash as usize) << self.params.bucket_size_log;
        let offset = self.bucket_offsets[hash as usize];
        self.table[bucket_start + offset as usize] = entry;
        let bucket_mask = (1u8 << self.params.bucket_size_log) - 1;
        self.bucket_offsets[hash as usize] = (offset + 1) & bucket_mask;
        self.entries_inserted += 1;
    }

    fn fill_only(&mut self, combined: &[u8], scan_start: usize, scan_end: usize) {
        let min_match = self.params.min_match_length as usize;
        let h_bits = self.h_bits();

        if scan_end.saturating_sub(scan_start) < min_match {
            return;
        }

        let mut gear = GearHash::new(&self.params);
        let reset_end = (scan_start + min_match).min(scan_end);
        gear.reset(&combined[scan_start..reset_end]);
        let mut ip = reset_end;
        let ilimit = scan_end.saturating_sub(8);

        while ip < ilimit {
            let mut num_splits = 0;
            let hashed = gear.feed(&combined[ip..ilimit], &mut self.splits, &mut num_splits);

            for n in 0..num_splits {
                let trigger_pos = ip + self.splits[n];
                if trigger_pos < scan_start + min_match {
                    continue;
                }
                let split = trigger_pos - min_match;
                if split + min_match > combined.len() {
                    continue;
                }
                let xxhash = xxh64(&combined[split..split + min_match], 0);
                let hash = (xxhash as u32) & ((1u32 << h_bits) - 1);
                let checksum = (xxhash >> 32) as u32;
                self.insert_entry(
                    hash,
                    LdmEntry {
                        offset: split as u32,
                        checksum,
                    },
                );
            }

            ip += hashed;
        }
    }

    fn generate_sequences(&mut self, combined: &[u8], scan_start: usize, scan_end: usize) {
        self.raw_sequences.clear();
        let min_match = self.params.min_match_length as usize;
        let h_bits = self.h_bits();
        let ents_per_bucket = 1usize << self.params.bucket_size_log;

        if scan_end.saturating_sub(scan_start) < min_match {
            return;
        }

        let mut gear = GearHash::new(&self.params);
        let reset_end = (scan_start + min_match).min(scan_end);
        gear.reset(&combined[scan_start..reset_end]);
        let mut ip = reset_end;
        let ilimit = scan_end.saturating_sub(8);
        let mut anchor = scan_start;

        while ip < ilimit {
            let mut num_splits = 0;
            let hashed = gear.feed(&combined[ip..ilimit], &mut self.splits, &mut num_splits);

            let mut reset_anchor = false;

            for n in 0..num_splits {
                let trigger_pos = ip + self.splits[n];
                if trigger_pos < scan_start + min_match {
                    continue;
                }
                let split = trigger_pos - min_match;
                if split + min_match > combined.len() {
                    continue;
                }

                let xxhash = xxh64(&combined[split..split + min_match], 0);
                let hash = (xxhash as u32) & ((1u32 << h_bits) - 1);
                let checksum = (xxhash >> 32) as u32;

                let new_entry = LdmEntry {
                    offset: split as u32,
                    checksum,
                };

                if split < anchor {
                    self.insert_entry(hash, new_entry);
                    continue;
                }

                let bucket_start_idx = (hash as usize) << self.params.bucket_size_log;
                let mut best_fwd = 0usize;
                let mut best_back = 0usize;
                let mut best_match_pos = 0usize;

                for i in 0..ents_per_bucket {
                    let cur = self.table[bucket_start_idx + i];
                    if cur.checksum != checksum || cur.offset == 0 {
                        continue;
                    }
                    let match_pos = cur.offset as usize;
                    if match_pos >= split {
                        continue;
                    }

                    let max_fwd = (scan_end - split).min(combined.len() - match_pos);
                    let mut fwd = 0;
                    while fwd < max_fwd && combined[split + fwd] == combined[match_pos + fwd] {
                        fwd += 1;
                    }
                    if fwd < min_match {
                        continue;
                    }

                    let mut back = 0;
                    while split > anchor + back
                        && match_pos > back
                        && combined[split - back - 1] == combined[match_pos - back - 1]
                    {
                        back += 1;
                    }

                    let total = fwd + back;
                    if total > best_fwd + best_back {
                        best_fwd = fwd;
                        best_back = back;
                        best_match_pos = match_pos;
                    }
                }

                self.insert_entry(hash, new_entry);

                if best_fwd == 0 {
                    continue;
                }

                let match_start = split - best_back;
                let match_ref = best_match_pos - best_back;
                let match_length = best_fwd + best_back;
                let offset = (match_start - match_ref) as u32;

                self.raw_sequences.push(RawLdmSeq {
                    lit_length: (match_start - anchor) as u32,
                    match_length: match_length as u32,
                    offset,
                });

                anchor = match_start + match_length;

                if anchor > ip + hashed {
                    let reset_start = anchor.saturating_sub(min_match);
                    if reset_start + min_match <= scan_end {
                        gear = GearHash::new(&self.params);
                        gear.reset(&combined[reset_start..reset_start + min_match]);
                    }
                    ip = anchor;
                    reset_anchor = true;
                    break;
                }
            }

            if reset_anchor {
                continue;
            }
            ip += hashed;
        }
    }

    /// Compress a block using LDM + regular match finder.
    ///
    /// Generates LDM sequences, then for each gap between LDM matches runs
    /// the regular Fast/DFast compressor. Output sequences go into `sequences`.
    #[allow(clippy::too_many_arguments)]
    pub fn compress_block(
        &mut self,
        src: &[u8],
        block_start: usize,
        block_end: usize,
        params: &LevelParams,
        rep_offsets: &[u32; 3],
        hash_table: &mut [u32],
        hash_long: &mut [u32],
        sequences: &mut Vec<Sequence>,
    ) {
        if self.entries_inserted == 0 {
            self.fill_only(src, block_start, block_end);
            match params.strategy {
                Strategy::Fast => {
                    fast::compress_fast_block(
                        src,
                        block_start,
                        block_end,
                        params,
                        rep_offsets,
                        hash_table,
                        sequences,
                    );
                }
                Strategy::DFast => {
                    dfast::compress_dfast_block(
                        src,
                        block_start,
                        block_end,
                        params,
                        rep_offsets,
                        hash_table,
                        hash_long,
                        sequences,
                    );
                }
            }
            return;
        }
        self.generate_sequences(src, block_start, block_end);

        if self.raw_sequences.is_empty() {
            match params.strategy {
                Strategy::Fast => {
                    fast::compress_fast_block(
                        src,
                        block_start,
                        block_end,
                        params,
                        rep_offsets,
                        hash_table,
                        sequences,
                    );
                }
                Strategy::DFast => {
                    dfast::compress_dfast_block(
                        src,
                        block_start,
                        block_end,
                        params,
                        rep_offsets,
                        hash_table,
                        hash_long,
                        sequences,
                    );
                }
            }
            return;
        }

        sequences.clear();
        let mut cursor = block_start;
        let mut reps = *rep_offsets;
        let num_ldm = self.raw_sequences.len();

        for i in 0..num_ldm {
            let ldm_seq = self.raw_sequences[i];
            let gap_end = cursor + ldm_seq.lit_length as usize;

            let last_lits;
            if gap_end > cursor {
                match params.strategy {
                    Strategy::Fast => {
                        fast::compress_fast_block(
                            src,
                            cursor,
                            gap_end,
                            params,
                            &reps,
                            hash_table,
                            &mut self.segment_sequences,
                        );
                    }
                    Strategy::DFast => {
                        dfast::compress_dfast_block(
                            src,
                            cursor,
                            gap_end,
                            params,
                            &reps,
                            hash_table,
                            hash_long,
                            &mut self.segment_sequences,
                        );
                    }
                }

                let consumed: usize = self
                    .segment_sequences
                    .iter()
                    .map(|s| (s.literal_length + s.match_length) as usize)
                    .sum();
                last_lits = (gap_end - cursor) - consumed;

                if let Some(last) = self.segment_sequences.last() {
                    reps = [last.offset, reps[0], reps[1]];
                }
                sequences.extend_from_slice(&self.segment_sequences);
            } else {
                last_lits = 0;
            }

            sequences.push(Sequence {
                literal_length: last_lits as u32,
                offset: ldm_seq.offset,
                match_length: ldm_seq.match_length,
            });
            reps = [ldm_seq.offset, reps[0], reps[1]];

            cursor = gap_end + ldm_seq.match_length as usize;
        }

        if cursor < block_end {
            match params.strategy {
                Strategy::Fast => {
                    fast::compress_fast_block(
                        src,
                        cursor,
                        block_end,
                        params,
                        &reps,
                        hash_table,
                        &mut self.segment_sequences,
                    );
                }
                Strategy::DFast => {
                    dfast::compress_dfast_block(
                        src,
                        cursor,
                        block_end,
                        params,
                        &reps,
                        hash_table,
                        hash_long,
                        &mut self.segment_sequences,
                    );
                }
            }
            sequences.extend_from_slice(&self.segment_sequences);
        }
    }
}
