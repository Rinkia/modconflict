//! A counting global allocator, compiled only into test builds.
//!
//! The record-scale benchmark needs a number for peak memory — the whole-plugin
//! parse holds every record id at once — and the standard library offers no
//! portable way to read it. This wraps the system allocator with two atomics so
//! the benchmark can bracket a section and read its high-water mark, with no
//! external crate and nothing in a release build.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

pub struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
        System.dealloc(ptr, layout);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let np = System.realloc(ptr, layout, new_size);
        if !np.is_null() {
            if new_size >= layout.size() {
                let grew = new_size - layout.size();
                let now = CURRENT.fetch_add(grew, Ordering::Relaxed) + grew;
                PEAK.fetch_max(now, Ordering::Relaxed);
            } else {
                CURRENT.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        np
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Pull the peak watermark down to the live bytes now, so the next section's
/// peak is measured from here rather than from the whole program's history.
pub fn reset_peak() {
    PEAK.store(CURRENT.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// The highest live-allocation watermark since the last [`reset_peak`].
pub fn peak_bytes() -> usize {
    PEAK.load(Ordering::Relaxed)
}

/// Live allocated bytes right now.
pub fn current_bytes() -> usize {
    CURRENT.load(Ordering::Relaxed)
}
