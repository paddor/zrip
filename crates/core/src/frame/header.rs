#![forbid(unsafe_code)]

use crate::error::DecompressError;
use crate::frame::ZSTD_MAGIC;

#[derive(Debug, Clone)]
pub struct FrameHeader {
    pub window_size: u64,
    pub frame_content_size: Option<u64>,
    pub dict_id: Option<u32>,
    pub content_checksum: bool,
    #[allow(dead_code)]
    pub single_segment: bool,
    pub header_size: usize,
}

pub fn parse_frame_header(data: &[u8]) -> Result<FrameHeader, DecompressError> {
    if data.len() < 4 {
        return Err(DecompressError::InputExhausted);
    }

    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if magic != ZSTD_MAGIC {
        return Err(DecompressError::BadMagic);
    }

    if data.len() < 5 {
        return Err(DecompressError::BadFrameHeader);
    }

    let descriptor = data[4];
    let dict_id_flag = descriptor & 0x03;
    let content_checksum = (descriptor & 0x04) != 0;
    let single_segment = (descriptor & 0x20) != 0;
    let fcs_field_size_flag = (descriptor >> 6) & 0x03;

    let reserved = (descriptor & 0x08) != 0;
    if reserved {
        return Err(DecompressError::BadFrameHeader);
    }
    let unused = (descriptor & 0x10) != 0;
    if unused {
        return Err(DecompressError::BadFrameHeader);
    }

    let mut offset = 5;

    let window_size = if single_segment {
        0
    } else {
        if data.len() <= offset {
            return Err(DecompressError::BadFrameHeader);
        }
        let window_desc = data[offset];
        offset += 1;
        let exponent = (window_desc >> 3) as u64;
        let mantissa = (window_desc & 0x07) as u64;
        let window_base = 1u64 << (10 + exponent);
        let window_add = (window_base >> 3) * mantissa;
        window_base + window_add
    };

    let dict_id_size = match dict_id_flag {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 4,
        _ => unreachable!(),
    };

    let dict_id = if dict_id_size > 0 {
        if data.len() < offset + dict_id_size {
            return Err(DecompressError::BadFrameHeader);
        }
        let id = match dict_id_size {
            1 => data[offset] as u32,
            2 => u16::from_le_bytes([data[offset], data[offset + 1]]) as u32,
            4 => u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]),
            _ => unreachable!(),
        };
        offset += dict_id_size;
        Some(id)
    } else {
        None
    };

    let fcs_field_size = match fcs_field_size_flag {
        0 => {
            if single_segment {
                1
            } else {
                0
            }
        }
        1 => 2,
        2 => 4,
        3 => 8,
        _ => unreachable!(),
    };

    let frame_content_size = if fcs_field_size > 0 {
        if data.len() < offset + fcs_field_size {
            return Err(DecompressError::BadFrameHeader);
        }
        let fcs = match fcs_field_size {
            1 => data[offset] as u64,
            2 => u16::from_le_bytes([data[offset], data[offset + 1]]) as u64 + 256,
            4 => u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as u64,
            8 => u64::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]),
            _ => unreachable!(),
        };
        offset += fcs_field_size;
        Some(fcs)
    } else {
        None
    };

    let final_window_size = if single_segment {
        frame_content_size.unwrap_or(0)
    } else {
        window_size
    };

    Ok(FrameHeader {
        window_size: final_window_size,
        frame_content_size,
        dict_id,
        content_checksum,
        single_segment,
        header_size: offset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_header() {
        let data = [0x28, 0xB5, 0x2F, 0xFD, 0x20, 0x00];
        let hdr = parse_frame_header(&data).unwrap();
        assert!(hdr.single_segment);
        assert!(!hdr.content_checksum);
        assert_eq!(hdr.dict_id, None);
        assert_eq!(hdr.frame_content_size, Some(0));
    }

    #[test]
    fn bad_magic() {
        let data = [0x00, 0x00, 0x00, 0x00, 0x00];
        assert!(matches!(
            parse_frame_header(&data),
            Err(DecompressError::BadMagic)
        ));
    }
}
