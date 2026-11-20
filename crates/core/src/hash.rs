//! Multipliers the match finders use for their hash tables.

#![forbid(unsafe_code)]

/// Multiplier of 4-byte hashes: XXH32's first prime, also C zstd's
/// `prime4bytes`.
pub const PRIME32_1: u32 = 0x9E37_79B1;
/// Multiplier of 5- to 8-byte hashes: XXH64's first prime. C zstd uses a
/// separate prime per length, so these hash values differ.
pub const PRIME64_1: u64 = 0x9E37_79B1_85EB_CA87;
