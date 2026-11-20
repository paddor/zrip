//! The LZ77 sequence the match finders emit for the block encoder.

/// One LZ77 sequence: copy literals, then a match.
#[derive(Debug, Clone, Copy)]
pub struct Sequence {
    /// Literal bytes before the match.
    pub literal_length: u32,
    /// Match distance in bytes. The block encoder turns it into a repeat
    /// code or an offset value.
    pub offset: u32,
    /// Bytes the match copies.
    pub match_length: u32,
}
