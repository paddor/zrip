use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingAlloc;

static LARGE_ALLOCS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() >= 512 * 1024 {
            LARGE_ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

#[test]
fn compress_into_does_not_allocate_output_sized_temp_vec() {
    let input = b"hello zstd frame";
    let mut output = vec![0u8; 1024 * 1024];

    LARGE_ALLOCS.store(0, Ordering::SeqCst);
    let written = zrip::compress_into(input, &mut output, 1).unwrap();

    assert_eq!(LARGE_ALLOCS.load(Ordering::SeqCst), 0);
    assert_eq!(zrip::decompress(&output[..written]).unwrap(), input);
}
