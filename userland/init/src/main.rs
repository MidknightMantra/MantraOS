#![no_std]
#![no_main]

use core::arch::asm;
use mantra_sys::syscall;

#[inline(always)]
unsafe fn syscall1(n: u64, a1: u64) -> u64 {
    let mut rax = n;
    asm!(
        "int 0x80",
        inout("rax") rax,
        in("rdi") a1,
        options(nostack)
    );
    rax
}

#[inline(always)]
unsafe fn syscall2(n: u64, a1: u64, a2: u64) -> u64 {
    let mut rax = n;
    asm!(
        "int 0x80",
        inout("rax") rax,
        in("rdi") a1,
        in("rsi") a2,
        options(nostack)
    );
    rax
}

#[inline(always)]
unsafe fn syscall3(n: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let mut rax = n;
    asm!(
        "int 0x80",
        inout("rax") rax,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        options(nostack)
    );
    rax
}

#[inline(always)]
unsafe fn syscall4(n: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    let mut rax = n;
    asm!(
        "int 0x80",
        inout("rax") rax,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        in("rcx") a4,
        options(nostack)
    );
    rax
}

#[inline(always)]
unsafe fn syscall3_ret_rdx(n: u64, a1: u64, a2: u64, a3: u64) -> (u64, u64) {
    let mut rax = n;
    let mut rdx = a3;
    asm!(
        "int 0x80",
        inout("rax") rax,
        in("rdi") a1,
        in("rsi") a2,
        inlateout("rdx") rdx,
        options(nostack)
    );
    (rax, rdx)
}

fn putc(b: u8) {
    unsafe {
        let _ = syscall1(syscall::PUTC, b as u64);
    }
}

fn puts(s: &str) {
    unsafe {
        let _ = syscall2(syscall::WRITE, s.as_ptr() as u64, s.len() as u64);
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let role: u64;
    unsafe { asm!("mov {}, rdi", out(reg) role, options(nomem, nostack, preserves_flags)) };
    let ep: u64;
    unsafe { asm!("mov {}, rsi", out(reg) ep, options(nomem, nostack, preserves_flags)) };

    if role == 0 {
        puts("init[0]: server start\n");
        // Create an endpoint, then spawn the client and pass it a derived cap to the same endpoint.
        let ep = unsafe { syscall1(syscall::IPC_EP_CREATE, 0) };
        puts("init[0]: ep=");
        put_hex(ep);
        puts("\n");

        let pid = unsafe { syscall3(syscall::PROC_SPAWN, 1, 1, ep) };
        puts("init[0]: spawned pid=");
        put_hex(pid);
        puts("\n");

        // Create a second endpoint and transfer its capability over `ep`.
        let ep2 = unsafe { syscall1(syscall::IPC_EP_CREATE, 0) };
        puts("init[0]: ep2=");
        put_hex(ep2);
        puts("\n");

        let note = b"cap transfer: ep2\n";
        let sent = unsafe {
            syscall4(
                syscall::IPC_SEND_CAP,
                ep,
                note.as_ptr() as u64,
                note.len() as u64,
                ep2,
            )
        };
        puts("init[0]: sent cap note=");
        put_hex(sent);
        puts("\n");

        let mut buf = [0u8; 64];
        loop {
            let got = unsafe { syscall3(syscall::IPC_RECV, ep2, buf.as_mut_ptr() as u64, buf.len() as u64) };
            if got < 0x8000_0000_0000_0000 {
                puts("init[0]: recv msg=");
                let n = core::cmp::min(got as usize, buf.len());
                unsafe {
                    let _ = syscall2(syscall::WRITE, buf.as_ptr() as u64, n as u64);
                }
                puts("\n");
            }
            unsafe {
                let _ = syscall1(syscall::YIELD_, 0);
            }
        }
    } else {
        puts("init[1]: client start\n");
        puts("init[1]: ep=");
        put_hex(ep);
        puts("\n");

        let mut buf = [0u8; 64];
        let (got, new_cap) = loop {
            let (got, new_cap) = unsafe {
                syscall3_ret_rdx(
                    syscall::IPC_RECV_CAP,
                    ep,
                    buf.as_mut_ptr() as u64,
                    buf.len() as u64,
                )
            };
            if got == u64::MAX - 2 {
                unsafe { let _ = syscall1(syscall::YIELD_, 0); }
                continue;
            }
            break (got, new_cap);
        };
        puts("init[1]: recv note bytes=");
        put_hex(got);
        puts(" cap=");
        put_hex(new_cap);
        puts("\n");

        if got < 0x8000_0000_0000_0000 {
            puts("init[1]: note=");
            let n = core::cmp::min(got as usize, buf.len());
            unsafe { let _ = syscall2(syscall::WRITE, buf.as_ptr() as u64, n as u64); }
            puts("\n");
        }

        let msg = b"ping over transferred cap\n";
        let sent = unsafe { syscall3(syscall::IPC_SEND, new_cap, msg.as_ptr() as u64, msg.len() as u64) };
        puts("init[1]: sent on new cap=");
        put_hex(sent);
        puts("\n");
        loop {
            unsafe {
                let _ = syscall1(syscall::YIELD_, 0);
            }
        }
    }

}

fn put_hex(v: u64) {
    // Minimal hex printer via syscalls.
    let hex = *b"0123456789abcdef";
    putc(b'0');
    putc(b'x');
    for i in (0..16).rev() {
        let d = ((v >> (i * 4)) & 0xf) as usize;
        putc(hex[d]);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        unsafe { asm!("int 0x80", in("rax") syscall::YIELD_, in("rdi") 0u64, options(nostack)) };
    }
}
