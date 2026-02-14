#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]

extern crate alloc;

use core::fmt::Write;
use core::panic::PanicInfo;
use mantra_bootinfo::{BootInfo, MemoryRegion, PixelFormat, RegionKind};

mod arch;
mod fb;
mod heap;
mod init_elf;
mod ipc;
mod pmm;
mod sched;
mod serial;
mod user;

#[no_mangle]
pub extern "sysv64" fn _start(boot_info: *const BootInfo) -> ! {
    serial::init();
    serial::write_str("mantracore: entered kernel\n");

    // Firmware may leave IF=1. Keep interrupts off until IDT/PIC/PIT/scheduler are ready.
    unsafe { core::arch::asm!("cli", options(nomem, nostack, preserves_flags)) };

    arch::init();

    let bi = unsafe { boot_info.as_ref() };
    if bi.is_none() {
        serial::write_str("mantracore: boot_info null\n");
        loop {
            unsafe {
                core::arch::asm!("hlt");
            }
        }
    }
    let bi = bi.unwrap();

    if bi.magic != BootInfo::MAGIC || bi.version != BootInfo::VERSION {
        serial::write_str("mantracore: boot_info magic/version mismatch\n");
        loop {
            unsafe {
                core::arch::asm!("hlt");
            }
        }
    }

    let regions: &[MemoryRegion] = if bi.regions_ptr != 0 && bi.regions_len != 0 {
        unsafe {
            core::slice::from_raw_parts(
                bi.regions_ptr as *const MemoryRegion,
                bi.regions_len as usize,
            )
        }
    } else {
        &[]
    };
    serial::write_str("mantracore: regions=");
    serial::write_dec_u64(regions.len() as u64);
    serial::write_str(" usable=");
    let mut usable_cnt: u64 = 0;
    for r in regions {
        if r.kind == RegionKind::Usable as u32 {
            usable_cnt += 1;
        }
    }
    serial::write_dec_u64(usable_cnt);
    serial::write_str("\n");

    let format = match bi.fb_format {
        x if x == PixelFormat::Rgb as u32 => PixelFormat::Rgb,
        x if x == PixelFormat::Bgr as u32 => PixelFormat::Bgr,
        _ => PixelFormat::Unknown,
    };

    let mut con = fb::Console::new(fb::FrameBuffer {
        base: bi.fb_base as *mut u8,
        size: bi.fb_size as usize,
        width: bi.fb_width as usize,
        height: bi.fb_height as usize,
        stride: bi.fb_stride as usize,
        format,
    });

    con.clear(fb::Rgb {
        r: 0x08,
        g: 0x0b,
        b: 0x10,
    });
    con.set_colors(
        fb::Rgb {
            r: 0xe8,
            g: 0xef,
            b: 0xff,
        },
        fb::Rgb {
            r: 0x08,
            g: 0x0b,
            b: 0x10,
        },
    );

    writeln!(&mut con, "MantraOS").ok();
    writeln!(&mut con, "BootInfo v{} OK", bi.version).ok();
    writeln!(&mut con, "Regions: {}", regions.len()).ok();
    writeln!(
        &mut con,
        "FB {}x{} stride={} fmt={:?}",
        bi.fb_width, bi.fb_height, bi.fb_stride, format
    )
    .ok();
    writeln!(&mut con, "FB base={:#x} size={:#x}", bi.fb_base, bi.fb_size).ok();
    writeln!(
        &mut con,
        "Kernel {:#x}-{:#x}",
        bi.kernel_phys_base, bi.kernel_phys_end
    )
    .ok();

    serial::write_str("mantracore: framebuffer initialized\n");

    match pmm::init(regions) {
        Ok(stats) => {
            serial::write_str("mantracore: pmm initialized\n");
            let _ = writeln!(
                &mut con,
                "PMM usable={}MiB free={}MiB ranges={}",
                stats.usable_bytes / (1024 * 1024),
                stats.free_bytes / (1024 * 1024),
                stats.range_count
            );

            for n in 0..3 {
                if let Some(p) = pmm::alloc_frame() {
                    serial::write_str("mantracore: alloc_frame ok ");
                    serial::write_hex_u64(p);
                    serial::write_str("\n");
                    let _ = writeln!(&mut con, "Frame{} {:#x}", n, p);
                } else {
                    serial::write_str("mantracore: alloc_frame failed\n");
                    let _ = writeln!(&mut con, "Frame{} FAIL", n);
                }
            }

            // Take ownership of paging (identity map enough RAM for kernel+fb).
            let mut max_phys = bi.kernel_phys_end;
            let fb_end = bi.fb_base.saturating_add(bi.fb_size);
            if fb_end > max_phys {
                max_phys = fb_end;
            }
            // Keep some headroom for page tables and early allocations.
            max_phys = max_phys.saturating_add(512 * 1024 * 1024);
            arch::init_paging(max_phys);

            // Switch framebuffer pointer to the higher-half direct map.
            con.fb.base = crate::arch::x86_64::paging::phys_to_virt_ptr(bi.fb_base);

            heap::init();
            crate::arch::x86_64::paging::kmap_smoke_test();

            // Heap smoke test (forces `alloc` to work).
            {
                use alloc::boxed::Box;
                use alloc::vec::Vec;

                let mut v: Vec<u64> = Vec::new();
                for i in 0..16u64 {
                    v.push(i * 3);
                }
                let b = Box::new(0xdead_beef_u64);

                serial::write_str("heap: vec_len=");
                serial::write_dec_u64(v.len() as u64);
                serial::write_str(" box=");
                serial::write_hex_u64(*b);
                serial::write_str("\n");
            }

            // First ring3 smoke test (int 0x80 back into kernel).
            user::enter_first_user(bi.kernel_phys_base, bi.kernel_phys_end, max_phys);
        }
        Err(_) => {
            serial::write_str("mantracore: pmm init failed\n");
            let _ = writeln!(&mut con, "PMM init failed");
        }
    }

    // Enable IRQ delivery once the console + PMM are up (so we can debug easily).
    arch::enable_interrupts();

    // Prove the IDT works (exception path) before we move to scheduler/VM work.
    unsafe {
        core::arch::asm!("int3");
    }

    // Visible "alive" marker (diagonal line).
    for i in 0..core::cmp::min(con.fb.width, con.fb.height) {
        con.fb.put_pixel(
            i,
            i,
            fb::Rgb {
                r: 0x5a,
                g: 0xff,
                b: 0x86,
            },
        );
    }

    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

#[alloc_error_handler]
fn oom(_layout: core::alloc::Layout) -> ! {
    serial::write_str("OOM\n");
    loop {
        unsafe {
            core::arch::asm!("cli; hlt", options(nomem, nostack));
        }
    }
}
