use crate::serial;

#[repr(C, packed)]
struct Gdtr {
    limit: u16,
    base: u64,
}

#[repr(C, packed)]
struct Tss {
    _rsv0: u32,
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    _rsv1: u64,
    ist1: u64,
    ist2: u64,
    ist3: u64,
    ist4: u64,
    ist5: u64,
    ist6: u64,
    ist7: u64,
    _rsv2: u64,
    _rsv3: u16,
    iomap_base: u16,
}

impl Tss {
    const fn new() -> Self {
        Self {
            _rsv0: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            _rsv1: 0,
            ist1: 0,
            ist2: 0,
            ist3: 0,
            ist4: 0,
            ist5: 0,
            ist6: 0,
            ist7: 0,
            _rsv2: 0,
            _rsv3: 0,
            iomap_base: core::mem::size_of::<Tss>() as u16,
        }
    }
}

// Simple single-core stacks (no guard pages yet).
static mut DF_IST_STACK: [u8; 16 * 1024] = [0; 16 * 1024];
static mut KERNEL_INT_STACK0: [u8; 16 * 1024] = [0; 16 * 1024];
static mut TSS0: Tss = Tss::new();

// GDT layout:
// 0: null
// 1: kernel code (selector 0x08)
// 2: kernel data (selector 0x10)
// 3-4: TSS (selector 0x18)
// 5: user data  (selector 0x28 | RPL3)
// 6: user code  (selector 0x30 | RPL3)
static mut GDT: [u64; 7] = [0; 7];

pub const KCODE_SEL: u16 = 0x08;
pub const KDATA_SEL: u16 = 0x10;
const TSS_SEL: u16 = 0x18;
pub const UDATA_SEL: u16 = 0x28;
pub const UCODE_SEL: u16 = 0x30;

fn gdt_code64() -> u64 {
    // base=0, limit=0xFFFFF, G=1, L=1, D=0, P=1, DPL=0, S=1, type=0xA (exec/read)
    0x00AF9A000000FFFF
}

fn gdt_data() -> u64 {
    // base=0, limit=0xFFFFF, G=1, P=1, DPL=0, S=1, type=0x2 (read/write)
    0x00AF92000000FFFF
}

fn gdt_user_code64() -> u64 {
    // Same as kernel code but DPL=3.
    0x00AFFA000000FFFF
}

fn gdt_user_data() -> u64 {
    // Same as kernel data but DPL=3.
    0x00AFF2000000FFFF
}

fn gdt_tss64(base: u64, limit: u32) -> (u64, u64) {
    // 16-byte TSS descriptor (Available 64-bit TSS: type=0x9)
    let mut low: u64 = 0;
    low |= (limit as u64) & 0xFFFF;
    low |= (base & 0xFFFF) << 16;
    low |= ((base >> 16) & 0xFF) << 32;
    low |= (0x9u64) << 40; // type
    low |= 1u64 << 47; // present
    low |= ((limit as u64 >> 16) & 0xF) << 48;
    low |= ((base >> 24) & 0xFF) << 56;

    let high: u64 = (base >> 32) & 0xFFFF_FFFF;
    (low, high)
}

unsafe fn lgdt(gdt: &'static [u64]) {
    let gdtr = Gdtr {
        limit: (gdt.len() * core::mem::size_of::<u64>() - 1) as u16,
        base: gdt.as_ptr() as u64,
    };
    core::arch::asm!("lgdt [{}]", in(reg) &gdtr, options(nostack, preserves_flags));
}

unsafe fn load_segments() {
    // Reload data segments.
    core::arch::asm!(
        "mov ds, {0:x}",
        "mov es, {0:x}",
        "mov ss, {0:x}",
        in(reg) KDATA_SEL,
        options(nomem, nostack, preserves_flags)
    );

    // Reload CS using a far return to ensure CS matches our GDT.
    core::arch::asm!(
        "push {cs}",
        "lea rax, [rip + 2f]",
        "push rax",
        "retfq",
        "2:",
        cs = in(reg) (KCODE_SEL as u64),
        options(nomem, nostack, preserves_flags)
    );
}

unsafe fn ltr(sel: u16) {
    core::arch::asm!("ltr {0:x}", in(reg) sel, options(nomem, nostack, preserves_flags));
}

pub fn init() {
    unsafe {
        let df_top = (&raw const DF_IST_STACK as *const u8)
            .add(core::mem::size_of::<[u8; 16 * 1024]>()) as u64;
        TSS0.ist1 = df_top;

        let rsp0_top = (&raw const KERNEL_INT_STACK0 as *const u8)
            .add(core::mem::size_of::<[u8; 16 * 1024]>()) as u64;
        TSS0.rsp0 = rsp0_top;

        GDT[0] = 0;
        GDT[1] = gdt_code64();
        GDT[2] = gdt_data();
        let (tss_lo, tss_hi) = gdt_tss64(
            (&raw const TSS0) as u64,
            (core::mem::size_of::<Tss>() - 1) as u32,
        );
        GDT[3] = tss_lo;
        GDT[4] = tss_hi;
        GDT[5] = gdt_user_data();
        GDT[6] = gdt_user_code64();

        let gdt: &'static [u64; 7] = &*(&raw const GDT);
        lgdt(gdt);
        load_segments();
        ltr(TSS_SEL);
    }

    serial::write_str("mantracore: gdt/tss initialized\n");
}

pub fn df_ist_index() -> u8 {
    1
}

pub fn set_rsp0(rsp0_top: u64) {
    unsafe {
        TSS0.rsp0 = rsp0_top;
    }
}

pub fn current_cs() -> u16 {
    let cs: u16;
    unsafe {
        core::arch::asm!(
            "mov {0:x}, cs",
            out(reg) cs,
            options(nomem, nostack, preserves_flags)
        );
    }
    cs
}
