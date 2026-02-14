use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr;

use crate::arch::x86_64::paging;
use crate::pmm;
use crate::serial;

struct Bump {
    start: u64,
    end: u64,
    next: u64,
    ready: bool,
}

struct LockedBump {
    inner: UnsafeCell<Bump>,
}

unsafe impl Sync for LockedBump {}

impl LockedBump {
    const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(Bump {
                start: 0,
                end: 0,
                next: 0,
                ready: false,
            }),
        }
    }

    unsafe fn bump(&self) -> &mut Bump {
        &mut *self.inner.get()
    }
}

#[global_allocator]
static ALLOC: KernelAlloc = KernelAlloc {};

static HEAP: LockedBump = LockedBump::new();

pub fn init() {
    // Grab a contiguous heap region early. If this fails, keep the heap disabled.
    // We try larger first then back off.
    let mut pages: u64 = 4096; // 16 MiB
    let mut base: Option<u64> = None;
    while pages >= 128 {
        if let Some(p) = pmm::alloc_pages(pages) {
            base = Some(p);
            break;
        }
        pages /= 2;
    }

    let Some(base) = base else {
        serial::write_str("heap: init failed (no pages)\n");
        return;
    };

    let size = pages * 4096;
    let base_v = paging::phys_to_virt(base);
    unsafe {
        let h = HEAP.bump();
        h.start = base_v;
        h.end = base_v + size;
        h.next = base_v;
        h.ready = true;
    }

    serial::write_str("heap: initialized base(p)=");
    serial::write_hex_u64(base);
    serial::write_str(" base(v)=");
    serial::write_hex_u64(base_v);
    serial::write_str(" size=");
    serial::write_dec_u64(size / (1024 * 1024));
    serial::write_str("MiB\n");
}

pub struct KernelAlloc;

impl KernelAlloc {
    fn align_up(x: u64, a: u64) -> u64 {
        if a == 0 {
            return x;
        }
        (x + (a - 1)) & !(a - 1)
    }
}

unsafe impl GlobalAlloc for KernelAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let h = HEAP.bump();
        if !h.ready {
            return ptr::null_mut();
        }

        let align = layout.align() as u64;
        let size = layout.size() as u64;
        let start = Self::align_up(h.next, align);
        let end = start.saturating_add(size);
        if end > h.end {
            return ptr::null_mut();
        }

        h.next = end;
        start as *mut u8
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Leak for now. We'll replace with a real allocator once VMM + locking exist.
    }
}
