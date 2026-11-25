//! Branch hints for stable Rust: the unexpected path calls an empty
//! `#[cold]` function, which steers code layout.
//!
//! The call is real. The unexpected path pays a call and a return, and the
//! values of a loop around it must sit in callee-saved registers or on the
//! stack. Hint only paths that are rare, such as errors: the sequence
//! decoder got 3% faster without a hint on its repeat offset branch.

/// Hints that `b` is usually true, and returns it.
#[inline(always)]
pub fn likely(b: bool) -> bool {
    if !b {
        cold();
    }
    b
}

/// Hints that `b` is usually false, and returns it.
#[inline(always)]
pub fn unlikely(b: bool) -> bool {
    if b {
        cold();
    }
    b
}

// OPTIMIZE: with an MSRV of 1.95, try `core::hint::cold_path()` here. It
// emits no call. Swapped in on 2026-11-25, native decoding gained 3%, all
// of it in the sequence loop, which no longer uses these hints. Small level 1
// inputs encoded 2% slower and portable builds moved by 3% both ways, so
// check each user.
#[cold]
#[inline(never)]
fn cold() {}
