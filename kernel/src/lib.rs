#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    let framebuffer = 0xb8000 as *mut u8;

    unsafe {
        framebuffer.write_volatile(b'M');
        framebuffer.add(1).write_volatile(0x0f);
    }

    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
