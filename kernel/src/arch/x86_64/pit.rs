use super::port;

pub fn init(hz: u32) {
    let hz = hz.clamp(18, 2000);
    let divisor: u16 = (1193182u32 / hz) as u16;

    unsafe {
        // Channel 0, lobyte/hibyte, mode 3 (square wave), binary.
        port::outb(0x43, 0x36);
        port::outb(0x40, (divisor & 0xff) as u8);
        port::outb(0x40, (divisor >> 8) as u8);
    }
}
