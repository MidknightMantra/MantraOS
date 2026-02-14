pub mod gdt;
mod idt;
pub mod isr;
pub mod paging;
mod pic;
mod pit;
mod port;

pub fn init() {
    gdt::init();
    idt::init();
    pic::init();
    pit::init(100); // 100 Hz
}

pub fn enable_interrupts() {
    idt::enable_interrupts();
}

pub fn init_paging(max_phys_addr_inclusive: u64) {
    paging::init(max_phys_addr_inclusive);
}
