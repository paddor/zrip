//! Bit-level I/O. Forward streams (FSE table descriptions) are read LSB
//! first from the start. Reverse streams (FSE and Huffman payloads) are
//! written forward and read backward from an end mark.

pub(crate) mod primitives;
pub mod reader;
pub mod reader_reverse;
pub mod writer;
