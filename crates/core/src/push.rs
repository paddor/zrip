//! Types shared by the push encoder and decoder.

/// What a push encoder or decoder needs next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// All input was consumed and all output delivered; pass more input.
    NeedsInput,
    /// The output buffer is full and more output may follow; call again
    /// with more room.
    NeedsOutput,
    /// A frame ended and all of its output was delivered.
    Finished,
}

/// Result of one push call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Input bytes consumed.
    pub consumed: usize,
    /// Output bytes written.
    pub produced: usize,
    /// What the caller should do next.
    pub status: Status,
}
