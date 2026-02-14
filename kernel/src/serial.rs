pub fn init() {
    unsafe {
        // Disable interrupts
        outb(COM1 + 1, 0x00);
        // Enable DLAB
        outb(COM1 + 3, 0x80);
        // Divisor (lo/hi) for 115200 baud on 1.8432 MHz clock => 1
        outb(COM1 + 0, 0x01);
        outb(COM1 + 1, 0x00);
        // 8 bits, no parity, one stop bit
        outb(COM1 + 3, 0x03);
        // Enable FIFO, clear, 14-byte threshold
        outb(COM1 + 2, 0xC7);
        // IRQs enabled, RTS/DSR set
        outb(COM1 + 4, 0x0B);
    }
}

pub fn write_str(s: &str) {
    for b in s.bytes() {
        write_byte(b);
    }
}

pub fn write_dec_u64(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut i = 0;
    if v == 0 {
        write_byte(b'0');
        return;
    }
    while v != 0 && i < buf.len() {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        write_byte(buf[i]);
    }
}

pub fn write_hex_u64(v: u64) {
    write_str("0x");
    for i in (0..16).rev() {
        let shift = i * 4;
        let d = ((v >> shift) & 0xf) as u8;
        let c = match d {
            0..=9 => b'0' + d,
            _ => b'a' + (d - 10),
        };
        write_byte(c);
    }
}

const COM1: u16 = 0x3F8;

pub fn write_byte(b: u8) {
    unsafe {
        while (inb(COM1 + 5) & 0x20) == 0 {}
        outb(COM1, b);
    }
}

unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags));
}

unsafe fn inb(port: u16) -> u8 {
    let mut val: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") val, options(nomem, nostack, preserves_flags));
    val
}
