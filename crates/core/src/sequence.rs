#[derive(Debug, Clone, Copy)]
pub struct Sequence {
    pub literal_length: u32,
    pub offset: u32,
    pub match_length: u32,
}
