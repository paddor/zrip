extern crate libc;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct TrackingAllocator;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let cur = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(cur, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            let diff = new_size as isize - layout.size() as isize;
            if diff > 0 {
                let cur = CURRENT.fetch_add(diff as usize, Ordering::Relaxed) + diff as usize;
                PEAK.fetch_max(cur, Ordering::Relaxed);
            } else {
                CURRENT.fetch_sub((-diff) as usize, Ordering::Relaxed);
            }
        }
        new_ptr
    }
}

#[global_allocator]
static ALLOC: TrackingAllocator = TrackingAllocator;

fn reset_peak() {
    PEAK.store(CURRENT.load(Ordering::Relaxed), Ordering::Relaxed);
}

fn peak_bytes() -> usize {
    PEAK.load(Ordering::Relaxed)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .map(|s| s.as_str())
        .unwrap_or("corpus/dickens.txt");
    let level: i32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);

    let data = std::fs::read(path).unwrap();
    let name = std::path::Path::new(path)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    let input_kb = data.len() as f64 / 1024.0;

    println!("{name} ({input_kb:.0} KB), level {level}\n");

    // Measure compress
    reset_peak();
    let compressed = zrip::compress(&data, level).unwrap();
    let compress_peak = peak_bytes();

    println!("  compress:");
    println!(
        "    peak heap:  {:.1} KB ({:.2}x input)",
        compress_peak as f64 / 1024.0,
        compress_peak as f64 / data.len() as f64
    );
    println!(
        "    output:     {:.1} KB (ratio {:.3})",
        compressed.len() as f64 / 1024.0,
        compressed.len() as f64 / data.len() as f64
    );

    // Measure decompress (from C zstd compressed data for fair comparison)
    let c_compressed = zstd::encode_all(&data[..], level).unwrap();
    drop(compressed);

    reset_peak();
    let decompressed = zrip::decompress(&c_compressed).unwrap();
    let decompress_peak = peak_bytes();

    assert_eq!(decompressed.len(), data.len());
    println!("\n  decompress (from C zstd L{level} data):");
    println!(
        "    peak heap:  {:.1} KB ({:.2}x output)",
        decompress_peak as f64 / 1024.0,
        decompress_peak as f64 / data.len() as f64
    );

    // Measure decompress with CompressContext (reused buffers)
    drop(decompressed);
    let mut ctx = zrip::CompressContext::new(level).unwrap();
    let _ = ctx.compress(&data).unwrap(); // warm up
    reset_peak();
    let _ = ctx.compress(&data).unwrap();
    let ctx_compress_peak = peak_bytes();
    println!("\n  CompressContext (reused):");
    println!(
        "    peak heap:  {:.1} KB ({:.2}x input)",
        ctx_compress_peak as f64 / 1024.0,
        ctx_compress_peak as f64 / data.len() as f64
    );
}
