use super::port;

const PIC1: u16 = 0x20;
const PIC2: u16 = 0xA0;
const PIC1_CMD: u16 = PIC1;
const PIC1_DATA: u16 = PIC1 + 1;
const PIC2_CMD: u16 = PIC2;
const PIC2_DATA: u16 = PIC2 + 1;

const ICW1_INIT: u8 = 0x10;
const ICW1_ICW4: u8 = 0x01;
const ICW4_8086: u8 = 0x01;

pub fn init() {
    unsafe {
        // Start init sequence.
        port::outb(PIC1_CMD, ICW1_INIT | ICW1_ICW4);
        port::io_wait();
        port::outb(PIC2_CMD, ICW1_INIT | ICW1_ICW4);
        port::io_wait();

        // Remap offsets: master 0x20, slave 0x28.
        port::outb(PIC1_DATA, 0x20);
        port::io_wait();
        port::outb(PIC2_DATA, 0x28);
        port::io_wait();

        // Tell Master PIC about Slave at IRQ2, and tell Slave its cascade identity.
        port::outb(PIC1_DATA, 0x04);
        port::io_wait();
        port::outb(PIC2_DATA, 0x02);
        port::io_wait();

        // 8086 mode.
        port::outb(PIC1_DATA, ICW4_8086);
        port::io_wait();
        port::outb(PIC2_DATA, ICW4_8086);
        port::io_wait();

        // Mask everything except IRQ0 (timer) and IRQ2 (cascade).
        port::outb(PIC1_DATA, 0b1111_1010);
        port::outb(PIC2_DATA, 0b1111_1111);
    }
}

pub fn eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            port::outb(PIC2_CMD, 0x20);
        }
        port::outb(PIC1_CMD, 0x20);
    }
}
