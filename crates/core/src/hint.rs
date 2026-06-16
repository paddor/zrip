#[inline(always)]
pub fn likely(b: bool) -> bool {
    if !b {
        cold();
    }
    b
}

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
