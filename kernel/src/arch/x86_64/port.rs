pub unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
        options(nomem, nostack, preserves_flags)
    );
}

pub unsafe fn inb(port: u16) -> u8 {
    let mut val: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port,
        out("al") val,
        options(nomem, nostack, preserves_flags)
    );
    val
}

pub unsafe fn io_wait() {
    // Port 0x80 is used for 'checkpoints' on some systems; writing is a common delay.
    outb(0x80, 0);
}
