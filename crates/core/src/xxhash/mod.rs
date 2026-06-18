mod primitives;

const PRIME64_1: u64 = 0x9E37_79B1_85EB_CA87;
const PRIME64_2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const PRIME64_3: u64 = 0x1656_67B1_9E37_79F9;
const PRIME64_4: u64 = 0x85EB_CA77_C2B2_AE63;
const PRIME64_5: u64 = 0x27D4_EB2F_1656_67C5;

#[inline]
fn xxh64_round(mut acc: u64, input: u64) -> u64 {
    acc = acc.wrapping_add(input.wrapping_mul(PRIME64_2));
    acc = acc.rotate_left(31);
    acc.wrapping_mul(PRIME64_1)
}

#[inline]
fn xxh64_merge_round(mut acc: u64, val: u64) -> u64 {
    let val = xxh64_round(0, val);
    acc ^= val;
    acc = acc.wrapping_mul(PRIME64_1).wrapping_add(PRIME64_4);
    acc
}

pub fn xxh64(data: &[u8], seed: u64) -> u64 {
    let len = data.len();
    let mut h64: u64;

    if len >= 32 {
        let mut v1 = seed.wrapping_add(PRIME64_1).wrapping_add(PRIME64_2);
        let mut v2 = seed.wrapping_add(PRIME64_2);
        let mut v3 = seed;
        let mut v4 = seed.wrapping_sub(PRIME64_1);

        primitives::bulk_rounds(data, &mut v1, &mut v2, &mut v3, &mut v4);

        h64 = v1
            .rotate_left(1)
            .wrapping_add(v2.rotate_left(7))
            .wrapping_add(v3.rotate_left(12))
            .wrapping_add(v4.rotate_left(18));

        h64 = xxh64_merge_round(h64, v1);
        h64 = xxh64_merge_round(h64, v2);
        h64 = xxh64_merge_round(h64, v3);
        h64 = xxh64_merge_round(h64, v4);
    } else {
        h64 = seed.wrapping_add(PRIME64_5);
    }

    h64 = h64.wrapping_add(len as u64);

    let tail = &data[len & !31..];
    let mut remaining = tail;
    while remaining.len() >= 8 {
        let k1 = xxh64_round(0, primitives::read_u64_le(remaining, 0));
        h64 ^= k1;
        h64 = h64
            .rotate_left(27)
            .wrapping_mul(PRIME64_1)
            .wrapping_add(PRIME64_4);
        remaining = &remaining[8..];
    }

    while remaining.len() >= 4 {
        h64 ^= (primitives::read_u32_le(remaining, 0) as u64).wrapping_mul(PRIME64_1);
        h64 = h64
            .rotate_left(23)
            .wrapping_mul(PRIME64_2)
            .wrapping_add(PRIME64_3);
        remaining = &remaining[4..];
    }

    for &b in remaining {
        h64 ^= (b as u64).wrapping_mul(PRIME64_5);
        h64 = h64.rotate_left(11).wrapping_mul(PRIME64_1);
    }

    h64 ^= h64 >> 33;
    h64 = h64.wrapping_mul(PRIME64_2);
    h64 ^= h64 >> 29;
    h64 = h64.wrapping_mul(PRIME64_3);
    h64 ^= h64 >> 32;

    h64
}

pub struct Xxh64State {
    v1: u64,
    v2: u64,
    v3: u64,
    v4: u64,
    total_len: u64,
    buf: [u8; 32],
    buf_used: usize,
    large: bool,
    seed: u64,
}

impl Xxh64State {
    pub fn new(seed: u64) -> Self {
        Self {
            v1: seed.wrapping_add(PRIME64_1).wrapping_add(PRIME64_2),
            v2: seed.wrapping_add(PRIME64_2),
            v3: seed,
            v4: seed.wrapping_sub(PRIME64_1),
            total_len: 0,
            buf: [0u8; 32],
            buf_used: 0,
            large: false,
            seed,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total_len += data.len() as u64;

        if self.buf_used + data.len() < 32 {
            self.buf[self.buf_used..self.buf_used + data.len()].copy_from_slice(data);
            self.buf_used += data.len();
            return;
        }

        if self.buf_used > 0 {
            let fill = 32 - self.buf_used;
            self.buf[self.buf_used..32].copy_from_slice(&data[..fill]);
            data = &data[fill..];

            self.v1 = xxh64_round(self.v1, primitives::read_u64_le(&self.buf, 0));
            self.v2 = xxh64_round(self.v2, primitives::read_u64_le(&self.buf, 8));
            self.v3 = xxh64_round(self.v3, primitives::read_u64_le(&self.buf, 16));
            self.v4 = xxh64_round(self.v4, primitives::read_u64_le(&self.buf, 24));
            self.buf_used = 0;
            self.large = true;
        }

        if data.len() >= 32 {
            self.large = true;
            primitives::bulk_rounds(data, &mut self.v1, &mut self.v2, &mut self.v3, &mut self.v4);
            let consumed = data.len() & !31;
            data = &data[consumed..];
        }

        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_used = data.len();
        }
    }

    pub fn finish(&self) -> u64 {
        let mut h64: u64;

        if self.large {
            h64 = self
                .v1
                .rotate_left(1)
                .wrapping_add(self.v2.rotate_left(7))
                .wrapping_add(self.v3.rotate_left(12))
                .wrapping_add(self.v4.rotate_left(18));

            h64 = xxh64_merge_round(h64, self.v1);
            h64 = xxh64_merge_round(h64, self.v2);
            h64 = xxh64_merge_round(h64, self.v3);
            h64 = xxh64_merge_round(h64, self.v4);
        } else {
            h64 = self.seed.wrapping_add(PRIME64_5);
        }

        h64 = h64.wrapping_add(self.total_len);

        let data = &self.buf[..self.buf_used];
        let len = data.len();
        let mut offset = 0;

        while offset + 8 <= len {
            let k1 = xxh64_round(0, primitives::read_u64_le(data, offset));
            h64 ^= k1;
            h64 = h64
                .rotate_left(27)
                .wrapping_mul(PRIME64_1)
                .wrapping_add(PRIME64_4);
            offset += 8;
        }

        while offset + 4 <= len {
            h64 ^= (primitives::read_u32_le(data, offset) as u64).wrapping_mul(PRIME64_1);
            h64 = h64
                .rotate_left(23)
                .wrapping_mul(PRIME64_2)
                .wrapping_add(PRIME64_3);
            offset += 4;
        }

        while offset < len {
            h64 ^= (data[offset] as u64).wrapping_mul(PRIME64_5);
            h64 = h64.rotate_left(11).wrapping_mul(PRIME64_1);
            offset += 1;
        }

        h64 ^= h64 >> 33;
        h64 = h64.wrapping_mul(PRIME64_2);
        h64 ^= h64 >> 29;
        h64 = h64.wrapping_mul(PRIME64_3);
        h64 ^= h64 >> 32;

        h64
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn known_vectors() {
        assert_eq!(xxh64(b"", 0), 0xEF46_DB37_51D8_E999);
        assert_eq!(xxh64(b"a", 0), 0xD24E_C4F1_A98C_6E5B);
        assert_eq!(xxh64(b"abc", 0), 0x44BC_2CF5_AD77_0999);
    }

    #[test]
    fn large_input() {
        let data: Vec<u8> = (0u8..=255).cycle().take(1000).collect();
        let h = xxh64(&data, 0);
        let mut state = Xxh64State::new(0);
        state.update(&data);
        assert_eq!(state.finish(), h);
    }

    #[test]
    fn streaming_chunked() {
        let data: Vec<u8> = (0u8..100).collect();
        let expected = xxh64(&data, 0);

        let mut state = Xxh64State::new(0);
        for chunk in data.chunks(7) {
            state.update(chunk);
        }
        assert_eq!(state.finish(), expected);
    }

    #[test]
    fn streaming_single_bytes() {
        let data = b"Hello, World!";
        let expected = xxh64(data, 0);

        let mut state = Xxh64State::new(0);
        for &b in data.iter() {
            state.update(&[b]);
        }
        assert_eq!(state.finish(), expected);
    }

    #[test]
    fn content_checksum() {
        let hash = xxh64(b"test data for zstd", 0);
        let checksum = (hash & 0xFFFF_FFFF) as u32;
        assert_eq!(checksum, hash as u32);
    }
}
