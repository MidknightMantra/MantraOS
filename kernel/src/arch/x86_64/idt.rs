use super::gdt;
use super::isr;
use crate::serial;

#[repr(C)]
#[derive(Copy, Clone)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    zero: u32,
}

impl IdtEntry {
    const fn missing() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            zero: 0,
        }
    }

    fn set_handler(&mut self, handler: u64) {
        self.offset_low = handler as u16;
        self.offset_mid = (handler >> 16) as u16;
        self.offset_high = (handler >> 32) as u32;
        self.zero = 0;

        self.selector = current_cs();
        self.ist = 0;

        // Present | DPL=0 | Type=0xE (64-bit interrupt gate)
        self.type_attr = 0x8E;
    }

    fn set_ist(&mut self, ist: u8) {
        self.ist = ist & 0x7;
    }

    fn set_dpl(&mut self, dpl: u8) {
        let dpl = (dpl & 0x3) << 5;
        self.type_attr = (self.type_attr & !(0x3 << 5)) | dpl;
    }
}

#[repr(C, packed)]
struct Idtr {
    limit: u16,
    base: u64,
}

static mut IDT: [IdtEntry; 256] = [IdtEntry::missing(); 256];
#[repr(C)]
#[derive(Copy, Clone)]
pub struct InterruptStackFrame {
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

fn current_cs() -> u16 {
    let cs: u16;
    unsafe {
        core::arch::asm!("mov {0:x}, cs", out(reg) cs, options(nomem, nostack, preserves_flags));
    }
    cs
}

fn lidt(idt: &'static [IdtEntry; 256]) {
    let idtr = Idtr {
        limit: (core::mem::size_of::<[IdtEntry; 256]>() - 1) as u16,
        base: idt.as_ptr() as u64,
    };
    unsafe {
        core::arch::asm!("lidt [{}]", in(reg) &idtr, options(nostack, preserves_flags));
    }
}

pub fn init() {
    unsafe {
        IDT[3].set_handler(breakpoint_handler as *const () as u64);
        IDT[8].set_handler(double_fault_handler as *const () as u64);
        IDT[8].set_ist(gdt::df_ist_index());
        IDT[13].set_handler(gp_fault_handler as *const () as u64);
        IDT[14].set_handler(page_fault_handler as *const () as u64);

        // PIC IRQs (0..15) are remapped to 32..47.
        // Use an assembly stub so we can context-switch by swapping RSP + iretq.
        IDT[32].set_handler(isr::mantra_timer_irq_stub as *const () as u64);

        // System call test: int 0x80 from ring3.
        IDT[0x80].set_handler(isr::mantra_syscall80_stub as *const () as u64);
        IDT[0x80].set_dpl(3);
    }

    unsafe {
        // Avoid creating a shared reference to a `static mut`.
        let idt: &'static [IdtEntry; 256] = &*(&raw const IDT);
        lidt(idt);
    }

    serial::write_str("mantracore: idt initialized\n");
}

pub fn enable_interrupts() {
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
    }
    serial::write_str("mantracore: interrupts enabled\n");
}

extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    serial::write_str("EXC: int3 rip=");
    serial::write_hex_u64(frame.rip);
    serial::write_str("\n");
}

extern "x86-interrupt" fn double_fault_handler(frame: InterruptStackFrame, _err: u64) -> ! {
    serial::write_str("EXC: double fault rip=");
    serial::write_hex_u64(frame.rip);
    serial::write_str("\n");
    loop {
        unsafe { core::arch::asm!("cli; hlt", options(nomem, nostack)) };
    }
}

extern "x86-interrupt" fn gp_fault_handler(frame: InterruptStackFrame, err: u64) -> ! {
    serial::write_str("EXC: #GP err=");
    serial::write_hex_u64(err);
    serial::write_str(" rip=");
    serial::write_hex_u64(frame.rip);
    serial::write_str(" cs=");
    serial::write_hex_u64(frame.cs);
    serial::write_str(" rsp=");
    serial::write_hex_u64(frame.rsp);
    serial::write_str(" ss=");
    serial::write_hex_u64(frame.ss);
    serial::write_str("\n");
    loop {
        unsafe { core::arch::asm!("cli; hlt", options(nomem, nostack)) };
    }
}

extern "x86-interrupt" fn page_fault_handler(frame: InterruptStackFrame, err: u64) -> ! {
    let cr2: u64;
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack, preserves_flags));
    }
    serial::write_str("EXC: #PF cr2=");
    serial::write_hex_u64(cr2);
    serial::write_str(" err=");
    serial::write_hex_u64(err);
    serial::write_str(" rip=");
    serial::write_hex_u64(frame.rip);
    serial::write_str(" cs=");
    serial::write_hex_u64(frame.cs);
    serial::write_str(" rsp=");
    serial::write_hex_u64(frame.rsp);
    serial::write_str(" ss=");
    serial::write_hex_u64(frame.ss);
    serial::write_str("\n");
    loop {
        unsafe { core::arch::asm!("cli; hlt", options(nomem, nostack)) };
    }
}

// int 0x80 is handled by an assembly stub that saves/restores GPRs and iretqs.
