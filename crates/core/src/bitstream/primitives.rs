#[cfg(feature = "alloc")]
use alloc::vec::Vec;

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn read_u64_le_unaligned(data: &[u8], offset: usize) -> u64 {
    debug_assert!(offset + 8 <= data.len());
    // SAFETY: The debug assertion mirrors the caller contract: offset..offset+8
    // is inside data. read_unaligned permits any byte alignment.
    unsafe { u64::from_le((data.as_ptr().add(offset) as *const u64).read_unaligned()) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn read_u64_le_unaligned(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn byte_at(data: &[u8], idx: usize) -> u8 {
    debug_assert!(idx < data.len());
    // SAFETY: The caller keeps idx inside data; debug builds verify it.
    unsafe { *data.get_unchecked(idx) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn byte_at(data: &[u8], idx: usize) -> u8 {
    data[idx]
}

#[cfg(all(feature = "alloc", not(feature = "paranoid")))]
#[inline(always)]
pub(crate) fn write_u64_le_unaligned(buf: &mut Vec<u8>, pos: usize, val: u64) {
    debug_assert!(pos + 8 <= buf.capacity());
    // SAFETY: The caller reserves at least pos+8 capacity. The write initializes
    // bytes that a later set_len makes visible.
    unsafe {
        (buf.as_mut_ptr().add(pos) as *mut u64).write_unaligned(val.to_le());
    }
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
    debug_assert!(len <= vec.capacity());
    // SAFETY: Callers only expose bytes that have already been initialized.
    unsafe { vec.set_len(len) }
}

#[cfg(all(feature = "alloc", feature = "paranoid"))]
#[inline(always)]
pub(crate) fn set_vec_len(vec: &mut Vec<u8>, len: usize) {
    vec.resize(len, 0);
}
