#[cfg(feature = "alloc")]
use alloc::vec::Vec;

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn read_u64_le_unaligned(data: &[u8], offset: usize) -> u64 {
    let end = offset.checked_add(8).expect("read offset overflow");
    assert!(end <= data.len());
    // SAFETY: The assertion proves offset..offset+8 is inside data.
    // read_unaligned permits any byte alignment.
    unsafe { u64::from_le((data.as_ptr().add(offset) as *const u64).read_unaligned()) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn read_u64_le_unaligned(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(*data[offset..].first_chunk::<8>().unwrap())
}

#[cfg(all(feature = "alloc", not(feature = "paranoid")))]
#[inline(always)]
pub(crate) fn write_u64_le_unaligned(buf: &mut Vec<u8>, pos: usize, val: u64) {
    let needed = pos + 8;
    if buf.len() < needed {
        buf.resize(needed, 0);
    }
    buf[pos..pos + 8].copy_from_slice(&val.to_le_bytes());
}

#[cfg(all(feature = "alloc", feature = "paranoid"))]
#[inline(always)]
pub(crate) fn write_u64_le_unaligned(buf: &mut Vec<u8>, pos: usize, val: u64) {
    let needed = pos + 8;
    if buf.len() < needed {
        buf.resize(needed, 0);
    }
    buf[pos..pos + 8].copy_from_slice(&val.to_le_bytes());
}

#[cfg(all(feature = "alloc", not(feature = "paranoid")))]
#[inline(always)]
pub(crate) fn set_vec_len(vec: &mut Vec<u8>, len: usize) {
    vec.resize(len, 0);
}

#[cfg(all(feature = "alloc", feature = "paranoid"))]
#[inline(always)]
pub(crate) fn set_vec_len(vec: &mut Vec<u8>, len: usize) {
    vec.resize(len, 0);
}
