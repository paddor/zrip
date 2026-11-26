//! A stream as IronRDP's QOIZ server writes it: C zstd at level 3 with long distance matching,
//! one frame per session, flushed after each update. Long distance matching declares a 128 MiB
//! window, which `PushDecoder::new()` accepts, and its matches reach megabytes back. Not gated
//! on `std`, so the `alloc`-only test build (without the encoder's `ldm` feature) runs it too.
use zrip::push::PushDecoder;
use zstd_safe::zstd_sys::ZSTD_EndDirective;
use zstd_safe::{CCtx, CParameter, InBuffer, OutBuffer};

/// Incompressible bytes, so that only long distance matching finds their repeats.
fn noise(state: &mut u64, n: usize) -> Vec<u8> {
    (0..n)
        .map(|_| {
            *state ^= *state << 13;
            *state ^= *state >> 7;
            *state ^= *state << 17;
            *state as u8
        })
        .collect()
}

/// What `compress_stream2` writes for `input` with `end`.
fn compress(c: &mut CCtx<'static>, input: &[u8], end: ZSTD_EndDirective) -> Vec<u8> {
    let mut out = vec![0; zstd_safe::compress_bound(input.len()) + 64];
    let mut inb = InBuffer::around(input);
    let mut pos = 0;
    loop {
        let mut outb = OutBuffer::around_pos(out.as_mut_slice(), pos);
        let left = c.compress_stream2(&mut outb, &mut inb, end).unwrap();
        pos = outb.pos();
        if left == 0 && inb.pos() == input.len() {
            break;
        }
        out.resize(out.len() * 2, 0);
    }
    out.truncate(pos);
    out
}

#[test]
fn push_decoder_reads_c_long_distance_updates_from_megabytes_back() {
    let mut state = 0x9E37_79B9_7F4A_7C15;
    let base = noise(&mut state, 4 << 20);
    // 64 KiB repeats from 1 to 4 MiB before the end of the first update, each followed by new
    // bytes.
    let mut updates = vec![base.clone()];
    for back in 1..=4 {
        let start = base.len() - (back << 20);
        updates.push(base[start..start + (64 << 10)].to_vec());
        updates.push(noise(&mut state, 1000));
    }

    let mut c = CCtx::create();
    c.set_parameter(CParameter::CompressionLevel(3)).unwrap();
    c.set_parameter(CParameter::EnableLongDistanceMatching(true))
        .unwrap();
    let parts: Vec<Vec<u8>> = updates
        .iter()
        .map(|u| compress(&mut c, u, ZSTD_EndDirective::ZSTD_e_flush))
        .collect();
    let end = compress(&mut c, &[], ZSTD_EndDirective::ZSTD_e_end);

    // The frame header states no content size and a window of 2^27 bytes.
    assert_eq!(parts[0][4] & 0xe0, 0, "frame header descriptor");
    assert_eq!(parts[0][5], (27 - 10) << 3, "window descriptor");
    // Each repeat is a long match: a few bytes, where its 64 KiB of noise would not compress.
    for (i, part) in parts.iter().enumerate().skip(1).step_by(2) {
        assert!(part.len() < 1024, "update {i}: {} bytes", part.len());
    }

    let mut dec = PushDecoder::new();
    for (i, (update, part)) in updates.iter().zip(&parts).enumerate() {
        let mut out = Vec::new();
        dec.decode_to_vec(part, &mut out).unwrap();
        assert!(out == *update, "update {i}: {} bytes", out.len());
    }
    let mut out = Vec::new();
    dec.decode_to_vec(&end, &mut out).unwrap();
    assert!(out.is_empty());
    dec.finish().unwrap();
}
