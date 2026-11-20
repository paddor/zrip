//! Branch hints for stable Rust: the unexpected path calls an empty
//! `#[cold]` function, which steers code layout.

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

#[cold]
#[inline(never)]
fn cold() {}
