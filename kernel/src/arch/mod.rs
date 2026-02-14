pub mod x86_64;

pub fn init() {
    // Single-arch for now.
    x86_64::init();
}

pub fn enable_interrupts() {
    x86_64::enable_interrupts();
}

pub fn init_paging(max_phys_addr_inclusive: u64) {
    x86_64::init_paging(max_phys_addr_inclusive);
}
