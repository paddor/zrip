#![no_main]
//! A reused `DecompressContext` must decode every input exactly like a fresh
//! context, even after decoding other, possibly malformed, inputs.
use libfuzzer_sys::fuzz_target;

const LIMIT: usize = 1 << 20;

fn decode(
    ctx: &mut zrip::DecompressContext,
    input: &[u8],
) -> Result<Vec<u8>, zrip::DecompressError> {
    let mut output = Vec::new();
    ctx.decompress_into_with_limit(input, &mut output, LIMIT)
        .map(|_| output)
}

fuzz_target!(|data: &[u8]| {
    // The first byte picks where to split the rest into two inputs.
    let Some((&split, rest)) = data.split_first() else {
        return;
    };
    let (first, second) = rest.split_at(usize::from(split) * rest.len() / 255);

    let mut reused = zrip::DecompressContext::new();
    for input in [first, second, first, second] {
        let expected = decode(&mut zrip::DecompressContext::new(), input);
        assert_eq!(decode(&mut reused, input), expected);
    }
});
