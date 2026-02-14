use core::cmp;
use mantra_bootinfo::{MemoryRegion, RegionKind};

const PAGE_SIZE: u64 = 4096;
const MAX_RANGES: usize = 128;

#[derive(Copy, Clone, Default)]
struct Range {
    base: u64,
    end: u64, // exclusive
}

#[derive(Copy, Clone)]
pub struct PmmStats {
    pub usable_bytes: u64,
    pub free_bytes: u64,
    pub range_count: usize,
}

struct StaticCell<T> {
    inner: core::cell::UnsafeCell<T>,
}

impl<T> StaticCell<T> {
    const fn new(value: T) -> Self {
        Self {
            inner: core::cell::UnsafeCell::new(value),
        }
    }

    unsafe fn get(&self) -> *mut T {
        self.inner.get()
    }
}

unsafe impl<T> Sync for StaticCell<T> {}

struct Pmm {
    ranges: [Range; MAX_RANGES],
    len: usize,
    cursor: usize,
}

static PMM: StaticCell<Option<Pmm>> = StaticCell::new(None);

fn align_up(x: u64, a: u64) -> u64 {
    if a == 0 {
        return x;
    }
    (x + (a - 1)) & !(a - 1)
}

fn align_down(x: u64, a: u64) -> u64 {
    if a == 0 {
        return x;
    }
    x & !(a - 1)
}

fn overlaps(a0: u64, a1: u64, b0: u64, b1: u64) -> bool {
    a0 < b1 && b0 < a1
}

fn sort_by_base(ranges: &mut [Range], len: usize) {
    // Insertion sort: small N, no alloc.
    let mut i = 1;
    while i < len {
        let key = ranges[i];
        let mut j = i;
        while j > 0 && ranges[j - 1].base > key.base {
            ranges[j] = ranges[j - 1];
            j -= 1;
        }
        ranges[j] = key;
        i += 1;
    }
}

fn merge_adjacent(ranges: &mut [Range], len: &mut usize) {
    if *len == 0 {
        return;
    }
    let mut out = 0usize;
    let mut i = 0usize;
    while i < *len {
        let mut cur = ranges[i];
        i += 1;
        while i < *len && ranges[i].base <= cur.end {
            cur.end = cmp::max(cur.end, ranges[i].end);
            i += 1;
        }
        ranges[out] = cur;
        out += 1;
    }
    *len = out;
}

fn subtract_reserved(ranges: &mut [Range], len: &mut usize, res_base: u64, res_end: u64) -> bool {
    let mut i = 0usize;
    while i < *len {
        let r = ranges[i];
        if !overlaps(r.base, r.end, res_base, res_end) {
            i += 1;
            continue;
        }

        // Fully covered: remove.
        if res_base <= r.base && res_end >= r.end {
            // shift left
            let mut j = i + 1;
            while j < *len {
                ranges[j - 1] = ranges[j];
                j += 1;
            }
            *len -= 1;
            continue;
        }

        // Overlap at start.
        if res_base <= r.base && res_end < r.end {
            ranges[i].base = res_end;
            i += 1;
            continue;
        }

        // Overlap at end.
        if res_base > r.base && res_end >= r.end {
            ranges[i].end = res_base;
            i += 1;
            continue;
        }

        // Overlap in middle: split into two ranges.
        // Left: [r.base, res_base), Right: [res_end, r.end)
        if *len >= ranges.len() {
            return false;
        }
        let left = Range {
            base: r.base,
            end: res_base,
        };
        let right = Range {
            base: res_end,
            end: r.end,
        };
        ranges[i] = left;
        // insert right after i
        let mut j = *len;
        while j > i + 1 {
            ranges[j] = ranges[j - 1];
            j -= 1;
        }
        ranges[i + 1] = right;
        *len += 1;
        i += 2;
    }
    true
}

pub fn init(regions: &[MemoryRegion]) -> Result<PmmStats, ()> {
    let mut ranges = [Range::default(); MAX_RANGES];
    let mut len: usize = 0;
    let mut usable_bytes: u64 = 0;

    // Collect usable ranges.
    for r in regions {
        if r.kind != RegionKind::Usable as u32 {
            continue;
        }
        let base = align_up(r.base, PAGE_SIZE);
        let end = align_down(r.base.saturating_add(r.len), PAGE_SIZE);
        if end <= base {
            continue;
        }
        usable_bytes = usable_bytes.saturating_add(end - base);
        if len >= ranges.len() {
            return Err(());
        }
        ranges[len] = Range { base, end };
        len += 1;
    }

    if len == 0 {
        return Err(());
    }

    sort_by_base(&mut ranges, len);
    merge_adjacent(&mut ranges, &mut len);

    // Subtract all non-usable ranges (including kernel/boot/framebuffer).
    for r in regions {
        if r.kind == RegionKind::Usable as u32 {
            continue;
        }
        if r.len == 0 {
            continue;
        }
        let res_base = align_down(r.base, PAGE_SIZE);
        let res_end = align_up(r.base.saturating_add(r.len), PAGE_SIZE);
        if res_end <= res_base {
            continue;
        }
        if !subtract_reserved(&mut ranges, &mut len, res_base, res_end) {
            return Err(());
        }
    }

    // Hard-reserve the first 1 MiB. Even if firmware marks parts as usable,
    // this avoids allocating over low-memory real-mode/firmware structures.
    if !subtract_reserved(&mut ranges, &mut len, 0, 0x10_0000) {
        return Err(());
    }

    // Drop empty ranges.
    let mut out = 0usize;
    for i in 0..len {
        if ranges[i].end > ranges[i].base {
            ranges[out] = ranges[i];
            out += 1;
        }
    }
    len = out;
    if len == 0 {
        return Err(());
    }

    let mut free_bytes: u64 = 0;
    for i in 0..len {
        free_bytes = free_bytes.saturating_add(ranges[i].end - ranges[i].base);
    }

    unsafe {
        *PMM.get() = Some(Pmm {
            ranges,
            len,
            cursor: 0,
        });
    }

    Ok(PmmStats {
        usable_bytes,
        free_bytes,
        range_count: len,
    })
}

pub fn alloc_frame() -> Option<u64> {
    alloc_pages(1)
}

pub fn alloc_pages(pages: u64) -> Option<u64> {
    if pages == 0 {
        return None;
    }
    unsafe {
        let slot = &mut *PMM.get();
        let pmm = slot.as_mut()?;

        while pmm.cursor < pmm.len {
            let r = &mut pmm.ranges[pmm.cursor];
            if r.base >= r.end {
                pmm.cursor += 1;
                continue;
            }

            let need = pages.saturating_mul(PAGE_SIZE);
            let avail = r.end.saturating_sub(r.base);
            if avail < need {
                pmm.cursor += 1;
                continue;
            }

            let p = r.base;
            r.base = r.base.saturating_add(need);
            if r.base >= r.end {
                pmm.cursor += 1;
            }
            return Some(p);
        }
        None
    }
}
