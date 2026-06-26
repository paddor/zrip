#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

/// 16-byte copy via two u64 load/stores.  No overlap restriction — reads
/// complete before writes, so overlapping src/dst is well-defined.
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn copy_16(src: *const u8, dst: *mut u8) {
    unsafe {
        let a = (src as *const u64).read_unaligned();
        let b = (src.add(8) as *const u64).read_unaligned();
        (dst as *mut u64).write_unaligned(a);
        (dst.add(8) as *mut u64).write_unaligned(b);
    }
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn copy_8(src: *const u8, dst: *mut u8) {
    unsafe {
        let v = (src as *const u64).read_unaligned();
        (dst as *mut u64).write_unaligned(v);
    }
}

/// Copy `src` into the end of `vec` using 16-byte chunk copies.
///
/// All reads stay within `src` bounds (no wild over-read).
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn fast_extend_from_slice(vec: &mut Vec<u8>, src: &[u8]) {
    let len = src.len();
    if len == 0 {
        return;
    }
    debug_assert!(vec.len() + len + 16 <= vec.capacity());
    unsafe {
        let dst = vec.as_mut_ptr().add(vec.len());
        let sp = src.as_ptr();
        if len >= 16 {
            let mut off = 0usize;
            while off + 16 <= len {
                copy_16(sp.add(off), dst.add(off));
                off += 16;
            }
            if off < len {
                copy_16(sp.add(len - 16), dst.add(len - 16));
            }
        } else if len >= 8 {
            copy_8(sp, dst);
            copy_8(sp.add(len - 8), dst.add(len - 8));
        } else if len >= 4 {
            let a = (sp as *const u32).read_unaligned();
            (dst as *mut u32).write_unaligned(a);
            let b = (sp.add(len - 4) as *const u32).read_unaligned();
            (dst.add(len - 4) as *mut u32).write_unaligned(b);
        } else {
            core::ptr::copy_nonoverlapping(sp, dst, len);
        }
        vec.set_len(vec.len() + len);
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn fast_extend_from_slice(vec: &mut Vec<u8>, src: &[u8]) {
    vec.extend_from_slice(src);
}

/// Build an 8-byte repeating pattern from `offset` bytes at `src`.
/// Only reads the first `offset` bytes (no out-of-bounds access).
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn build_pattern_u64(src: *const u8, offset: usize) -> u64 {
    let mut buf = [0u8; 8];
    unsafe {
        core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), offset);
        let mut have = offset;
        while have < 8 {
            let n = have.min(8 - have);
            core::ptr::copy_nonoverlapping(buf.as_ptr(), buf.as_mut_ptr().add(have), n);
            have += n;
        }
    }
    u64::from_ne_bytes(buf)
}

/// Copy a match of `len` bytes at `offset` bytes back from the end of `vec`.
///
/// Handles all cases: non-overlapping (offset >= 16), offset 8..15,
/// RLE (offset == 1), and overlapping small-offset copies (2..7).
///
/// # Safety contract (upheld by caller):
/// - `offset > 0 && offset <= vec.len()`
/// - `vec.len() + len + 16 <= vec.capacity()`
#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn wild_copy_match(vec: &mut Vec<u8>, offset: usize, len: usize) {
    debug_assert!(offset > 0 && offset <= vec.len());
    debug_assert!(vec.len() + len + 16 <= vec.capacity());
    unsafe {
        let ptr = vec.as_mut_ptr();
        let op = ptr.add(vec.len());
        let src = op.sub(offset);

        if offset >= 16 {
            let mut off = 0usize;
            loop {
                copy_16(src.add(off), op.add(off));
                off += 16;
                if off >= len {
                    break;
                }
            }
        } else if offset == 1 {
            core::ptr::write_bytes(op, *src, len + 16);
        } else if offset >= 8 {
            // 8-byte copies tracking src; overlap at boundary is fine because
            // read_unaligned completes before write_unaligned.
            copy_8(src, op);
            copy_8(src.add(8), op.add(8));
            if len > 16 {
                let mut off = 16usize;
                loop {
                    copy_8(src.add(off), op.add(off));
                    copy_8(src.add(off + 8), op.add(off + 8));
                    off += 16;
                    if off >= len {
                        break;
                    }
                }
            }
        } else {
            // Offset 2..7: build 8-byte repeating pattern from individual source
            // bytes (avoids reading uninitialized destination memory), then stamp it
            // at offset-aligned positions.
            let pat64 = build_pattern_u64(src, offset);
            let mut off = 0usize;
            loop {
                (op.add(off) as *mut u64).write_unaligned(pat64);
                off += offset;
                if off >= len {
                    break;
                }
            }
        }
        vec.set_len(vec.len() + len);
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn wild_copy_match(vec: &mut Vec<u8>, offset: usize, len: usize) {
    let start = vec.len() - offset;
    if offset >= len {
        vec.extend_from_within(start..start + len);
    } else if offset == 1 {
        vec.resize(vec.len() + len, vec[start]);
    } else {
        vec.extend_from_within(start..start + offset);
        let mut copied = offset;
        while copied < len {
            let n = (len - copied).min(copied);
            let src = vec.len() - copied;
            vec.extend_from_within(src..src + n);
            copied += n;
        }
    }
}
