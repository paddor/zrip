use binggan::{INSTRUMENTED_SYSTEM, InputGroup, PeakMemAlloc, black_box, plugins::*};

#[global_allocator]
pub static GLOBAL: &PeakMemAlloc<std::alloc::System> = &INSTRUMENTED_SYSTEM;

fn gen_json_1k() -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..20 {
        out.extend_from_slice(
            format!(
                r#"{{"id":{},"name":"user_{}","email":"user{}@example.com","active":true,"score":{}}}"#,
                i,
                i,
                i,
                i * 17 % 100
            )
            .as_bytes(),
        );
        out.push(b'\n');
    }
    out.truncate(1024);
    out
}

fn gen_repetitive_64k() -> Vec<u8> {
    let pattern = b"The quick brown fox jumps over the lazy dog. ";
    pattern.iter().cycle().take(65536).copied().collect()
}

fn gen_mixed_128k() -> Vec<u8> {
    let mut data = Vec::with_capacity(128 * 1024);
    for i in 0..128 * 1024 {
        data.push(match i % 256 {
            0..=63 => b'A' + (i % 26) as u8,
            64..=127 => ((i * 7 + 13) % 256) as u8,
            128..=191 => b'0' + (i % 10) as u8,
            _ => b' ',
        });
    }
    data
}

fn main() {
    let json = gen_json_1k();
    let repetitive = gen_repetitive_64k();
    let mixed = gen_mixed_128k();

    bench_compress(
        "compress",
        vec![
            ("json_1k", json.clone()),
            ("repetitive_64k", repetitive.clone()),
            ("mixed_128k", mixed.clone()),
        ],
    );

    bench_decompress(
        "decompress",
        vec![
            ("json_1k", json),
            ("repetitive_64k", repetitive),
            ("mixed_128k", mixed),
        ],
    );
}

fn bench_compress(_name: &str, inputs: Vec<(&str, Vec<u8>)>) {
    let mut group = InputGroup::new_with_inputs(
        inputs
            .into_iter()
            .map(|(n, d)| (n.to_string(), d))
            .collect(),
    );
    group
        .add_plugin(CacheTrasher::default())
        .add_plugin(PeakMemAllocPlugin::new(GLOBAL));
    group.throughput(|input| input.len());

    group.register("zrip level 1", |input| {
        black_box(zrip::compress(input, 1).unwrap());
    });
    group.register("zstd level 1", |input| {
        black_box(zstd::bulk::compress(input, 1).unwrap());
    });
    group.register("zrip level 3", |input| {
        black_box(zrip::compress(input, 3).unwrap());
    });
    group.register("zstd level 3", |input| {
        black_box(zstd::bulk::compress(input, 3).unwrap());
    });

    group.run();
}

fn bench_decompress(_name: &str, inputs: Vec<(&str, Vec<u8>)>) {
    let prepared: Vec<(String, Vec<u8>)> = inputs
        .iter()
        .map(|(n, d)| {
            let compressed = zrip::compress(d, 1).unwrap();
            (format!("{n} (zrip L1)"), compressed)
        })
        .chain(inputs.iter().map(|(n, d)| {
            let compressed = zstd::bulk::compress(d, 1).unwrap();
            (format!("{n} (zstd L1)"), compressed)
        }))
        .collect();

    let mut group = InputGroup::new_with_inputs(prepared);
    group
        .add_plugin(CacheTrasher::default())
        .add_plugin(PeakMemAllocPlugin::new(GLOBAL));

    group.register("zrip decompress", |input| {
        black_box(zrip::decompress(input).unwrap());
    });
    group.register("zstd decompress", |input| {
        black_box(zstd::bulk::decompress(input, 64 * 1024 * 1024).unwrap());
    });

    group.run();
}
