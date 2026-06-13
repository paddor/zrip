use std::io::{Read, Write};

fn main() {
    let data = b"Hello, zrip! Fast pure-Rust zstd compression. ".repeat(100);

    // One-shot compression and decompression
    let compressed = zrip::compress(&data, 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);
    println!(
        "one-shot: {} -> {} bytes ({:.1}x)",
        data.len(),
        compressed.len(),
        data.len() as f64 / compressed.len() as f64
    );

    // Streaming compression
    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    encoder.write_all(&data).unwrap();
    let stream_compressed = encoder.finish().unwrap();

    // Streaming decompression
    let mut decoder = zrip::FrameDecoder::new(&stream_compressed[..]);
    let mut stream_decompressed = Vec::new();
    decoder.read_to_end(&mut stream_decompressed).unwrap();
    assert_eq!(stream_decompressed, data);
    println!("streaming: round-trip OK");

    // Buffer-reusing context for hot loops
    let mut enc_ctx = zrip::CompressContext::new(1).unwrap();
    let mut dec_ctx = zrip::DecompressContext::new();
    for i in 0..5 {
        let input = format!("message {i}: {}", "x".repeat(100));
        let c = enc_ctx.compress(input.as_bytes()).unwrap();
        let d = dec_ctx.decompress(&c).unwrap();
        assert_eq!(&*d, input.as_bytes());
    }
    println!("context reuse: 5 round-trips OK");
}
