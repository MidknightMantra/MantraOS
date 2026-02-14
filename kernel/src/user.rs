use crate::arch::x86_64::gdt;
use crate::arch::x86_64::isr;
use crate::arch::x86_64::paging;
use crate::init_elf;
use crate::ipc;
use crate::pmm;
use crate::sched;
use crate::serial;
use alloc::boxed::Box;
use core::arch::asm;

const PAGE_SIZE: u64 = 4096;

const PTE_P: u64 = 1 << 0;
const PTE_RW: u64 = 1 << 1;
const PTE_U: u64 = 1 << 2;

// Transition stack used while switching CR3 and building the iretq frame.
// The kernel's current stack may still be in boot/firmware memory, which won't be
// mapped in the user CR3 (we only map the kernel image + HHDM + user pages).
static mut USER_SWITCH_STACK: [u8; 16 * 1024] = [0; 16 * 1024];

static BOOT_KB: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static BOOT_KE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static BOOT_MAX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn align_down(x: u64, a: u64) -> u64 {
    if a == 0 {
        return x;
    }
    x & !(a - 1)
}

fn align_up(x: u64, a: u64) -> u64 {
    if a == 0 {
        return x;
    }
    (x + (a - 1)) & !(a - 1)
}

unsafe fn zero_page(p: u64) {
    core::ptr::write_bytes(paging::phys_to_virt_ptr::<u8>(p), 0, PAGE_SIZE as usize);
}

unsafe fn alloc_table() -> u64 {
    let p = pmm::alloc_pages(1).expect("user: alloc_pages failed");
    zero_page(p);
    p
}

unsafe fn invlpg(addr: u64) {
    asm!("invlpg [{}]", in(reg) addr, options(nomem, nostack, preserves_flags));
}

unsafe fn table_entry_mut(table_phys: u64, idx: usize) -> *mut u64 {
    paging::phys_to_virt_ptr::<u64>(table_phys).add(idx)
}

unsafe fn get_or_alloc_table(entry: *mut u64, flags: u64) -> u64 {
    let mut v = core::ptr::read_volatile(entry);
    if (v & PTE_P) != 0 {
        // For user mappings, every level must have the U bit set.
        if (flags & PTE_U) != 0 && (v & PTE_U) == 0 {
            v |= PTE_U;
            core::ptr::write_volatile(entry, v);
        }
        return v & 0x000f_ffff_ffff_f000;
    }
    let t = alloc_table();
    let mut e = t | (PTE_P | PTE_RW);
    if (flags & PTE_U) != 0 {
        e |= PTE_U;
    }
    core::ptr::write_volatile(entry, e);
    t
}

unsafe fn map_4k(pml4: u64, virt: u64, phys: u64, flags: u64) {
    let virt = align_down(virt, PAGE_SIZE);
    let phys = align_down(phys, PAGE_SIZE);

    let pml4_i = ((virt >> 39) & 0x1ff) as usize;
    let pdpt_i = ((virt >> 30) & 0x1ff) as usize;
    let pd_i = ((virt >> 21) & 0x1ff) as usize;
    let pt_i = ((virt >> 12) & 0x1ff) as usize;

    let pml4e = table_entry_mut(pml4, pml4_i);
    let pdpt = get_or_alloc_table(pml4e, flags);

    let pdpte = table_entry_mut(pdpt, pdpt_i);
    let pd = get_or_alloc_table(pdpte, flags);

    let pde = table_entry_mut(pd, pd_i);
    let pt = get_or_alloc_table(pde, flags);

    let pte = table_entry_mut(pt, pt_i);
    core::ptr::write_volatile(pte, phys | (PTE_P | flags));
    invlpg(virt);
}

unsafe fn map_hhdm_huge(pml4: u64, max_phys_inclusive: u64) {
    // Map HHDM using 2 MiB huge pages (supervisor-only).
    let max_end = align_up(max_phys_inclusive.saturating_add(1), 1024 * 1024 * 1024);
    let pdpt_entries =
        ((max_end + (1024 * 1024 * 1024 - 1)) / (1024 * 1024 * 1024)).min(512) as usize;

    let pdpt = alloc_table();
    *table_entry_mut(pml4, 256) = pdpt | (PTE_P | PTE_RW);

    for i in 0..pdpt_entries {
        let pd = alloc_table();
        *table_entry_mut(pdpt, i) = pd | (PTE_P | PTE_RW);
        let chunk_base = (i as u64) * (1024 * 1024 * 1024);
        for j in 0..512usize {
            let phys = chunk_base + (j as u64) * (2 * 1024 * 1024);
            *table_entry_mut(pd, j) = phys | (PTE_P | PTE_RW | (1 << 7));
        }
    }
}

#[repr(C)]
struct Elf64Ehdr {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[repr(C)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

#[repr(C)]
struct TaskTrapFrame {
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rsi: u64,
    rdi: u64,
    rbp: u64,
    rdx: u64,
    rcx: u64,
    rbx: u64,
    rax: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

unsafe fn build_initial_tf(
    kstack_top: u64,
    entry: u64,
    user_rsp: u64,
    role: u64,
    init_ep_cap: u64,
) -> u64 {
    let tf_ptr = (kstack_top - core::mem::size_of::<TaskTrapFrame>() as u64) as *mut TaskTrapFrame;
    core::ptr::write_bytes(tf_ptr as *mut u8, 0, core::mem::size_of::<TaskTrapFrame>());
    (*tf_ptr).rdi = role;
    (*tf_ptr).rsi = init_ep_cap;
    (*tf_ptr).rip = entry;
    (*tf_ptr).cs = (gdt::UCODE_SEL as u64) | 3;
    (*tf_ptr).rflags = 0x202;
    (*tf_ptr).rsp = user_rsp;
    (*tf_ptr).ss = (gdt::UDATA_SEL as u64) | 3;
    tf_ptr as u64
}

fn kstack_alloc_top() -> u64 {
    // Leak a kernel stack; it's mapped via HHDM in every user CR3.
    let b: Box<[u8; 16 * 1024]> = Box::new([0; 16 * 1024]);
    let base = Box::into_raw(b) as *mut u8 as u64;
    base + (16 * 1024) as u64
}

unsafe fn translate_4k(pml4: u64, virt: u64) -> Option<u64> {
    let virt = virt as u64;
    let pml4_i = ((virt >> 39) & 0x1ff) as usize;
    let pdpt_i = ((virt >> 30) & 0x1ff) as usize;
    let pd_i = ((virt >> 21) & 0x1ff) as usize;
    let pt_i = ((virt >> 12) & 0x1ff) as usize;
    let off = virt & 0xfff;

    let pml4e = core::ptr::read_volatile(table_entry_mut(pml4, pml4_i));
    if (pml4e & PTE_P) == 0 {
        return None;
    }
    let pdpt = pml4e & 0x000f_ffff_ffff_f000;

    let pdpte = core::ptr::read_volatile(table_entry_mut(pdpt, pdpt_i));
    if (pdpte & PTE_P) == 0 {
        return None;
    }
    let pd = pdpte & 0x000f_ffff_ffff_f000;

    let pde = core::ptr::read_volatile(table_entry_mut(pd, pd_i));
    if (pde & PTE_P) == 0 {
        return None;
    }
    let pt = pde & 0x000f_ffff_ffff_f000;

    let pte = core::ptr::read_volatile(table_entry_mut(pt, pt_i));
    if (pte & PTE_P) == 0 {
        return None;
    }
    let phys = (pte & 0x000f_ffff_ffff_f000) + off;
    Some(phys)
}

unsafe fn load_elf_into_user(pml4: u64, elf: &[u8]) -> Option<u64> {
    if elf.len() < core::mem::size_of::<Elf64Ehdr>() {
        return None;
    }
    let eh = &*(elf.as_ptr() as *const Elf64Ehdr);

    // "\x7fELF", class=64-bit, little endian.
    if eh.e_ident[0] != 0x7f
        || eh.e_ident[1] != b'E'
        || eh.e_ident[2] != b'L'
        || eh.e_ident[3] != b'F'
        || eh.e_ident[4] != 2
        || eh.e_ident[5] != 1
    {
        return None;
    }
    if eh.e_machine != 0x3e {
        return None;
    }
    if eh.e_phentsize as usize != core::mem::size_of::<Elf64Phdr>() {
        return None;
    }

    let phoff = eh.e_phoff as usize;
    let phnum = eh.e_phnum as usize;
    let phsz = core::mem::size_of::<Elf64Phdr>();
    if phoff.checked_add(phnum * phsz).unwrap_or(usize::MAX) > elf.len() {
        return None;
    }

    for i in 0..phnum {
        let ph = &*(elf.as_ptr().add(phoff + i * phsz) as *const Elf64Phdr);
        if ph.p_type != PT_LOAD || ph.p_memsz == 0 {
            continue;
        }

        // Map segment pages.
        let seg_start = align_down(ph.p_vaddr, PAGE_SIZE);
        let seg_end = align_up(ph.p_vaddr.saturating_add(ph.p_memsz), PAGE_SIZE);

        let mut flags = PTE_U;
        if (ph.p_flags & PF_W) != 0 {
            flags |= PTE_RW;
        }
        // NX is not enabled yet; ignore PF_X/PF_R.
        let _ = ph.p_flags & (PF_X | PF_R);

        let mut v = seg_start;
        while v < seg_end {
            let p = pmm::alloc_frame().expect("user: alloc_frame segment");
            map_4k(pml4, v, p, flags);
            v += PAGE_SIZE;
        }

        // Copy file bytes -> mapped pages using the built page tables to translate.
        if ph.p_filesz != 0 {
            let foff = ph.p_offset as usize;
            let fsz = ph.p_filesz as usize;
            if foff.checked_add(fsz).unwrap_or(usize::MAX) > elf.len() {
                return None;
            }
            for off in 0..fsz {
                let va = ph.p_vaddr + off as u64;
                let Some(pa) = translate_4k(pml4, va) else {
                    return None;
                };
                let src = elf[foff + off];
                *paging::phys_to_virt_ptr::<u8>(pa) = src;
            }
        }

        // Zero BSS.
        if ph.p_memsz > ph.p_filesz {
            let z = (ph.p_memsz - ph.p_filesz) as usize;
            for off in 0..z {
                let va = ph.p_vaddr + ph.p_filesz + off as u64;
                let Some(pa) = translate_4k(pml4, va) else {
                    return None;
                };
                *paging::phys_to_virt_ptr::<u8>(pa) = 0;
            }
        }
    }

    Some(eh.e_entry)
}

unsafe fn build_proc_from_init(role: u64, init_ep_cap: u64) -> (u64, u64, u64, u64) {
    let kb = BOOT_KB.load(core::sync::atomic::Ordering::Relaxed);
    let ke = BOOT_KE.load(core::sync::atomic::Ordering::Relaxed);
    let maxp = BOOT_MAX.load(core::sync::atomic::Ordering::Relaxed);
    if kb == 0 || ke == 0 || maxp == 0 {
        panic!("user: boot params not set");
    }

    let pml4 = alloc_table();

    // Map kernel identity (supervisor).
    let kb = align_down(kb, PAGE_SIZE);
    let ke = align_up(ke, PAGE_SIZE);
    let mut p = kb;
    while p < ke {
        map_4k(pml4, p, p, PTE_RW);
        p += PAGE_SIZE;
    }
    map_hhdm_huge(pml4, maxp);

    // User stack (fixed VA).
    let user_stack_top: u64 = 0x0000_0000_2000_0000;
    let stack_pages = 4u64;
    let stack_base = user_stack_top - stack_pages * PAGE_SIZE;
    for i in 0..stack_pages {
        let sp = pmm::alloc_frame().expect("user: alloc_frame stack");
        map_4k(pml4, stack_base + i * PAGE_SIZE, sp, PTE_U | PTE_RW);
    }
    // SysV ABI: at function entry, compilers generally assume RSP % 16 == 8.
    // Since we enter userspace via `iretq` (not a `call`), we emulate the post-call alignment.
    let user_rsp = user_stack_top - 8;

    // Code.
    let entry = if !init_elf::INIT_ELF.is_empty() {
        load_elf_into_user(pml4, init_elf::INIT_ELF).expect("user: init ELF load failed")
    } else {
        let user_code_v: u64 = 0x0000_0000_1000_0000;
        let code_p = pmm::alloc_frame().expect("user: alloc_frame code");
        map_4k(pml4, user_code_v, code_p, PTE_U);
        let code = [0xCDu8, 0x80, 0xEBu8, 0xFE]; // int 0x80; jmp $
        let code_ptr = paging::phys_to_virt_ptr::<u8>(code_p);
        core::ptr::copy_nonoverlapping(code.as_ptr(), code_ptr, code.len());
        user_code_v
    };

    let kstack_top = kstack_alloc_top();
    let tf_rsp = build_initial_tf(kstack_top, entry, user_rsp, role, init_ep_cap);
    (tf_rsp, kstack_top, pml4, entry)
}

pub fn spawn_init_from_syscall(prog_id: u64, role: u64, share_cap: u32) -> u64 {
    // Only one program exists right now.
    if prog_id != 1 {
        return u64::MAX;
    }

    let ep_id = if share_cap != 0 {
        sched::cap_lookup_current(share_cap).unwrap_or(0)
    } else {
        0
    };

    unsafe {
        // Build the process with placeholder cap.
        let (tf_rsp, kstack_top, cr3, _entry) = build_proc_from_init(role, 0);
        let Some(pid) = sched::spawn_proc(tf_rsp, kstack_top, cr3) else {
            return u64::MAX;
        };

        // Derive a child-local cap to the shared endpoint and patch the trap frame.
        let mut child_cap: u64 = 0;
        if ep_id != 0 {
            let c = sched::cap_alloc_for(pid, ep_id).unwrap_or(0);
            child_cap = c as u64;
        }
        let tf_ptr = tf_rsp as *mut TaskTrapFrame;
        (*tf_ptr).rsi = child_cap;

        pid as u64
    }
}

pub fn enter_first_user(kernel_phys_base: u64, kernel_phys_end: u64, max_phys_hint: u64) -> ! {
    serial::write_str("user: setting up address space\n");

    unsafe {
        BOOT_KB.store(kernel_phys_base, core::sync::atomic::Ordering::Relaxed);
        BOOT_KE.store(kernel_phys_end, core::sync::atomic::Ordering::Relaxed);
        BOOT_MAX.store(max_phys_hint, core::sync::atomic::Ordering::Relaxed);

        // Build and enter the first userspace process (init role 0).
        let (tf_rsp, kstack_top, cr3, entry) = build_proc_from_init(0, 0);
        serial::write_str("user: cr3=");
        serial::write_hex_u64(cr3);
        serial::write_str(" entry=");
        serial::write_hex_u64(entry);
        serial::write_str("\n");

        sched::install_first(tf_rsp, kstack_top, cr3);
        gdt::set_rsp0(kstack_top);

        let udata = ((gdt::UDATA_SEL as u64) | 3) as u16;
        let kstack_top = (&raw const USER_SWITCH_STACK as *const u8)
            .add(core::mem::size_of::<[u8; 16 * 1024]>()) as u64;

        // Switch to a known kernel stack, load CR3, load user DS/ES, then jump into the common
        // trap-return path (pops regs and iretqs) to start task0.
        asm!(
            "cli",
            "mov rsp, {kstack}",
            "mov cr3, {cr3}",
            "mov ds, ax",
            "mov es, ax",
            "mov rsp, {task_tf}",
            "jmp {ret}",
            in("ax") udata,
            kstack = in(reg) kstack_top,
            cr3 = in(reg) cr3,
            task_tf = in(reg) tf_rsp,
            ret = in(reg) (isr::mantra_trap_return as *const () as usize),
            options(noreturn)
        );
    }
}
