use crate::pmm;
use crate::serial;
use core::sync::atomic::{AtomicU64, Ordering};

const PAGE_SIZE: u64 = 4096;
const HUGE_2M: u64 = 2 * 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;

// Higher-half direct map: virt = HHDM_BASE + phys.
// PML4 index 256 corresponds to 0xffff_8000_0000_0000..0xffff_ffff_ffff_ffff.
pub const HHDM_BASE: u64 = 0xffff_8000_0000_0000;
pub const KMAP_BASE: u64 = 0xffff_ff00_0000_0000;
const KMAP_PML4_INDEX: usize = 510;

const PTE_P: u64 = 1 << 0;
const PTE_RW: u64 = 1 << 1;
const PTE_PS: u64 = 1 << 7;

#[repr(C, align(4096))]
struct PageTable {
    e: [u64; 512],
}

static PML4_PHYS: AtomicU64 = AtomicU64::new(0);
static KMAP_NEXT: AtomicU64 = AtomicU64::new(KMAP_BASE);

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

unsafe fn zero_page(p: u64) {
    core::ptr::write_bytes(p as *mut u8, 0, PAGE_SIZE as usize);
}

unsafe fn alloc_table() -> u64 {
    let p = pmm::alloc_pages(1).expect("paging: alloc_pages failed");
    zero_page(p);
    p
}

unsafe fn load_cr3(pml4_phys: u64) {
    core::arch::asm!(
        "mov cr3, {}",
        in(reg) pml4_phys,
        options(nomem, nostack, preserves_flags)
    );
}

#[inline]
pub fn phys_to_virt(phys: u64) -> u64 {
    HHDM_BASE.wrapping_add(phys)
}

#[inline]
pub fn phys_to_virt_ptr<T>(phys: u64) -> *mut T {
    phys_to_virt(phys) as *mut T
}

pub fn pml4_phys() -> u64 {
    PML4_PHYS.load(Ordering::Acquire)
}

unsafe fn invlpg(addr: u64) {
    core::arch::asm!("invlpg [{}]", in(reg) addr, options(nomem, nostack, preserves_flags));
}

unsafe fn table_entry_mut(table_phys: u64, idx: usize) -> *mut u64 {
    phys_to_virt_ptr::<u64>(table_phys).add(idx)
}

unsafe fn get_or_alloc_table(entry: *mut u64) -> u64 {
    let v = core::ptr::read_volatile(entry);
    if (v & PTE_P) != 0 {
        return v & 0x000f_ffff_ffff_f000;
    }
    let t = alloc_table();
    core::ptr::write_volatile(entry, t | (PTE_P | PTE_RW));
    t
}

// Create a 4 KiB mapping in the dedicated KMAP region.
pub fn kmap_map_4k(virt: u64, phys: u64, flags: u64) {
    let virt = align_down(virt, PAGE_SIZE);
    let phys = align_down(phys, PAGE_SIZE);

    unsafe {
        let pml4 = pml4_phys();
        if pml4 == 0 {
            serial::write_str("kmap: paging not initialized\n");
            return;
        }

        let pml4e = table_entry_mut(pml4, KMAP_PML4_INDEX);
        let pdpt = get_or_alloc_table(pml4e);

        let pdpt_i = ((virt >> 30) & 0x1ff) as usize;
        let pde_i = ((virt >> 21) & 0x1ff) as usize;
        let pte_i = ((virt >> 12) & 0x1ff) as usize;

        let pdpte = table_entry_mut(pdpt, pdpt_i);
        let pd = get_or_alloc_table(pdpte);

        let pde = table_entry_mut(pd, pde_i);
        let pt = get_or_alloc_table(pde);

        let pte = table_entry_mut(pt, pte_i);
        core::ptr::write_volatile(pte, phys | (PTE_P | PTE_RW) | flags);

        invlpg(virt);
    }
}

pub fn kmap_alloc_4k(phys: u64) -> u64 {
    let virt = KMAP_NEXT.fetch_add(PAGE_SIZE, Ordering::Relaxed);
    kmap_map_4k(virt, phys, 0);
    virt
}

pub fn init(max_phys_addr_inclusive: u64) {
    // Identity map [0, max_phys_end) with 2 MiB huge pages.
    let max_end = align_up(max_phys_addr_inclusive.saturating_add(1), GIB);
    let pdpt_entries = ((max_end + (GIB - 1)) / GIB).min(512) as usize;

    if pdpt_entries == 0 {
        serial::write_str("paging: max_end too small\n");
        return;
    }

    unsafe {
        let pml4 = alloc_table();
        let pdpt = alloc_table();

        // PML4[0] -> PDPT
        *(pml4 as *mut u64).add(0) = pdpt | (PTE_P | PTE_RW);
        // PML4[256] -> same PDPT (HHDM)
        *(pml4 as *mut u64).add(256) = pdpt | (PTE_P | PTE_RW);

        for i in 0..pdpt_entries {
            let pd = alloc_table();
            *(pdpt as *mut u64).add(i) = pd | (PTE_P | PTE_RW);

            // Fill PD with 2MiB entries mapping this 1GiB chunk.
            let chunk_base = (i as u64) * GIB;
            for j in 0..512usize {
                let phys = chunk_base + (j as u64) * HUGE_2M;
                *(pd as *mut u64).add(j) = phys | (PTE_P | PTE_RW | PTE_PS);
            }
        }

        serial::write_str("paging: loading new cr3, identity map up to ");
        serial::write_dec_u64(max_end / GIB);
        serial::write_str("GiB (HHDM enabled)\n");

        load_cr3(pml4);
        PML4_PHYS.store(pml4, Ordering::Release);
        serial::write_str("paging: enabled\n");
    }
}

pub fn kmap_smoke_test() {
    let Some(p) = pmm::alloc_frame() else {
        serial::write_str("kmap: alloc_frame failed\n");
        return;
    };

    let v = kmap_alloc_4k(p);
    serial::write_str("kmap: mapped p=");
    serial::write_hex_u64(p);
    serial::write_str(" v=");
    serial::write_hex_u64(v);
    serial::write_str("\n");

    unsafe {
        let ptr = v as *mut u64;
        core::ptr::write_volatile(ptr, 0x1122_3344_5566_7788);
        let r = core::ptr::read_volatile(ptr);
        serial::write_str("kmap: readback=");
        serial::write_hex_u64(r);
        serial::write_str("\n");
    }
}
