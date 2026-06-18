#[cfg(feature = "alloc")]
use alloc::vec::Vec;

#[inline(always)]
pub(crate) fn read_u64_le_unaligned(data: &[u8], offset: usize) -> u64 {
    debug_assert!(offset + 8 <= data.len());
    unsafe { u64::from_le((data.as_ptr().add(offset) as *const u64).read_unaligned()) }
}

#[inline(always)]
pub(crate) fn get_byte_unchecked(data: &[u8], idx: usize) -> u8 {
    debug_assert!(idx < data.len());
    unsafe { *data.get_unchecked(idx) }
}

#[inline(always)]
pub(crate) fn write_u64_le_unaligned(buf: &mut Vec<u8>, pos: usize, val: u64) {
    debug_assert!(pos + 8 <= buf.capacity());
    unsafe {
        (buf.as_mut_ptr().add(pos) as *mut u64).write_unaligned(val.to_le());
    }
}

#[inline(always)]
pub(crate) fn set_vec_len(vec: &mut Vec<u8>, len: usize) {
    debug_assert!(len <= vec.capacity());
    unsafe { vec.set_len(len) }
}
