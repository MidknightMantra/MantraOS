use core::arch::global_asm;

use super::pic;
use crate::arch::x86_64::paging;
use crate::ipc;
use crate::serial;
use crate::user;
use mantra_sys::syscall;

// Trap frame layout produced by `mantra_timer_irq_stub`.
// This is the pointer value passed to `mantra_timer_irq_rust`.
#[repr(C)]
pub struct TrapFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,

    // CPU-pushed frame (ring3 -> ring0): RIP, CS, RFLAGS, RSP, SS.
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

extern "C" {
    pub fn mantra_timer_irq_stub();
    pub fn mantra_syscall80_stub();
    pub fn mantra_trap_return() -> !;
}

#[no_mangle]
pub extern "C" fn mantra_timer_irq_rust(tf: *mut TrapFrame) -> u64 {
    // Acknowledge the interrupt early so we don't lose timer events if we run long.
    pic::eoi(0);
    crate::sched::on_timer_irq(tf)
}

// Trap frame layout produced by `mantra_syscall80_stub` (ring3 -> ring0): GPRs + RIP/CS/RFLAGS/RSP/SS.
#[repr(C)]
pub struct SyscallFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,

    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

#[no_mangle]
pub extern "C" fn mantra_syscall80_rust(tf: *mut SyscallFrame) -> u64 {
    let tf = unsafe { &mut *tf };
    let n = tf.rax;
    let mut switch_to: u64 = 0;

    match n {
        syscall::PUTC => {
            serial::write_byte(tf.rdi as u8);
            tf.rax = 0;
        }
        syscall::YIELD_ => {
            // Cooperative yield.
            tf.rax = 0;
            switch_to = crate::sched::yield_from_syscall(tf as *mut _ as u64);
        }
        syscall::WRITE => {
            // (ptr,len) -> bytes_written
            let user_ptr = tf.rdi;
            let user_len = tf.rsi as usize;
            let max = 1024usize;
            let n = core::cmp::min(user_len, max);

            let mut written = 0usize;
            while written < n {
                let v = user_ptr.wrapping_add(written as u64);
                if let Some(p) = user_virt_to_phys(v) {
                    let b = unsafe {
                        core::ptr::read_volatile(paging::phys_to_virt_ptr::<u8>(p))
                    };
                    serial::write_byte(b);
                    written += 1;
                } else {
                    break;
                }
            }
            tf.rax = written as u64;
        }
        syscall::IPC_EP_CREATE => {
            tf.rax = ipc::ep_create();
        }
        syscall::IPC_SEND => {
            // (cap, ptr, len) -> bytes_sent or err
            let cap = tf.rdi as u32;
            let user_ptr = tf.rsi;
            let user_len = core::cmp::min(tf.rdx as usize, 1024usize);
            let mut tmp = [0u8; 256];
            let n = core::cmp::min(user_len, tmp.len());
            if user_copy_in(&mut tmp[..n], user_ptr).is_none() {
                tf.rax = u64::MAX;
            } else {
                // If a receiver is blocked waiting on this endpoint, deliver directly.
                if let Some(ep_id) = crate::sched::cap_lookup_current(cap) {
                    if let Some(pid) = ipc::waiter_pop(ep_id) {
                        tf.rax = deliver_ipc(pid, &tmp[..n], 0);
                    } else {
                        tf.rax = ipc::ep_send_cap(cap, &tmp[..n], 0);
                    }
                } else {
                    tf.rax = u64::MAX;
                }
            }
        }
        syscall::IPC_RECV => {
            // (cap, ptr, max_len) -> bytes_recv or err
            let cap = tf.rdi as u32;
            let user_ptr = tf.rsi;
            let max_len = core::cmp::min(tf.rdx as usize, 1024usize);
            let mut tmp = [0u8; 256];
            let n = core::cmp::min(max_len, tmp.len());
            let got = ipc::ep_recv(cap, &mut tmp[..n]);
            if got == u64::MAX || got == u64::MAX - 2 {
                // Empty: block (if possible) instead of spinning in userspace.
                if got == u64::MAX - 2 && crate::sched::has_other_runnable() {
                    if let Some(ep_id) = crate::sched::cap_lookup_current(cap) {
                        if ipc::waiter_push(ep_id, crate::sched::current_pid()) {
                            crate::sched::block_current_on_ep(ep_id);
                            switch_to = crate::sched::yield_from_syscall(tf as *mut _ as u64);
                            // Do not update tf.rax here; it will be filled in by the sender's delivery path.
                        } else {
                            tf.rax = got;
                        }
                    } else {
                        tf.rax = u64::MAX;
                    }
                } else {
                    tf.rax = got;
                }
            } else {
                let got = got as usize;
                if user_copy_out(user_ptr, &tmp[..got]).is_some() {
                    tf.rax = got as u64;
                } else {
                    tf.rax = u64::MAX;
                }
            }
        }
        syscall::IPC_SEND_CAP => {
            // (cap, ptr, len, xfer_cap) -> bytes_sent or err
            let cap = tf.rdi as u32;
            let user_ptr = tf.rsi;
            let user_len = core::cmp::min(tf.rdx as usize, 1024usize);
            let xfer_cap = tf.rcx as u32;

            let xfer_ep = if xfer_cap == 0 {
                0
            } else if let Some(ep) = crate::sched::cap_lookup_current(xfer_cap) {
                ep
            } else {
                tf.rax = u64::MAX;
                return 0;
            };

            let mut tmp = [0u8; 256];
            let n = core::cmp::min(user_len, tmp.len());
            if user_copy_in(&mut tmp[..n], user_ptr).is_none() {
                tf.rax = u64::MAX;
            } else {
                if let Some(ep_id) = crate::sched::cap_lookup_current(cap) {
                    if let Some(pid) = ipc::waiter_pop(ep_id) {
                        tf.rax = deliver_ipc(pid, &tmp[..n], xfer_ep);
                    } else {
                        tf.rax = ipc::ep_send_cap(cap, &tmp[..n], xfer_ep);
                    }
                } else {
                    tf.rax = u64::MAX;
                }
            }
        }
        syscall::IPC_RECV_CAP => {
            // (cap, ptr, max_len) -> bytes_recv or err; out: rdx=received_cap (0 if none)
            let cap = tf.rdi as u32;
            let user_ptr = tf.rsi;
            let max_len = core::cmp::min(tf.rdx as usize, 1024usize);
            let mut tmp = [0u8; 256];
            let n = core::cmp::min(max_len, tmp.len());

            let (got, xfer_ep) = ipc::ep_recv_cap(cap, &mut tmp[..n]);
            if got == u64::MAX || got == u64::MAX - 2 {
                if got == u64::MAX - 2 && crate::sched::has_other_runnable() {
                    if let Some(ep_id) = crate::sched::cap_lookup_current(cap) {
                        if ipc::waiter_push(ep_id, crate::sched::current_pid()) {
                            crate::sched::block_current_on_ep(ep_id);
                            switch_to = crate::sched::yield_from_syscall(tf as *mut _ as u64);
                            // Sender will fill rax/rdx and user buffer.
                        } else {
                            tf.rax = got;
                            tf.rdx = 0;
                        }
                    } else {
                        tf.rax = u64::MAX;
                        tf.rdx = 0;
                    }
                } else {
                    tf.rax = got;
                    tf.rdx = 0;
                }
            } else {
                let got_usz = got as usize;
                if user_copy_out(user_ptr, &tmp[..got_usz]).is_some() {
                    // Install a local cap to the transferred endpoint, if any.
                    tf.rdx = 0;
                    if xfer_ep != 0 {
                        if let Some(new_cap) = crate::sched::cap_alloc_current(xfer_ep) {
                            tf.rdx = new_cap as u64;
                        } else {
                            // No cap slots available: drop the transfer but keep the message.
                            tf.rdx = 0;
                        }
                    }
                    tf.rax = got;
                } else {
                    tf.rax = u64::MAX;
                    tf.rdx = 0;
                }
            }
        }
        syscall::PROC_SPAWN => {
            // (prog_id, role, share_cap) -> pid or err
            let prog_id = tf.rdi;
            let role = tf.rsi;
            let share_cap = tf.rdx as u32;
            tf.rax = user::spawn_init_from_syscall(prog_id, role, share_cap);
        }
        _ => {
            serial::write_str("SYS: unknown int80 n=");
            serial::write_hex_u64(n);
            serial::write_str("\n");
            tf.rax = u64::MAX;
        }
    }

    switch_to
}

fn virt_to_phys_in(pml4_phys: u64, virt: u64) -> Option<u64> {
    // Walk 4-level tables. Require U=1 at every level and leaf present.
    const MASK: u64 = 0x000f_ffff_ffff_f000;
    const PTE_P: u64 = 1 << 0;
    const PTE_U: u64 = 1 << 2;

    let pml4 = pml4_phys & MASK;
    let pml4_i = ((virt >> 39) & 0x1ff) as usize;
    let pdpt_i = ((virt >> 30) & 0x1ff) as usize;
    let pd_i = ((virt >> 21) & 0x1ff) as usize;
    let pt_i = ((virt >> 12) & 0x1ff) as usize;
    let off = virt & 0xfff;

    unsafe fn rd(table: u64, idx: usize) -> u64 {
        core::ptr::read_volatile(paging::phys_to_virt_ptr::<u64>(table).add(idx))
    }

    let pml4e = unsafe { rd(pml4, pml4_i) };
    if (pml4e & (PTE_P | PTE_U)) != (PTE_P | PTE_U) {
        return None;
    }
    let pdpt = pml4e & MASK;

    let pdpte = unsafe { rd(pdpt, pdpt_i) };
    if (pdpte & (PTE_P | PTE_U)) != (PTE_P | PTE_U) {
        return None;
    }
    let pd = pdpte & MASK;

    let pde = unsafe { rd(pd, pd_i) };
    if (pde & (PTE_P | PTE_U)) != (PTE_P | PTE_U) {
        return None;
    }
    let pt = pde & MASK;

    let pte = unsafe { rd(pt, pt_i) };
    if (pte & (PTE_P | PTE_U)) != (PTE_P | PTE_U) {
        return None;
    }

    Some((pte & MASK) + off)
}

fn user_copy_out_in(pml4_phys: u64, user_ptr: u64, src: &[u8]) -> Option<()> {
    for (i, b) in src.iter().enumerate() {
        let v = user_ptr.wrapping_add(i as u64);
        let p = virt_to_phys_in(pml4_phys, v)?;
        unsafe { core::ptr::write_volatile(paging::phys_to_virt_ptr::<u8>(p), *b) };
    }
    Some(())
}

fn deliver_ipc(pid: usize, msg: &[u8], xfer_ep: u32) -> u64 {
    let Some(cr3) = crate::sched::proc_cr3(pid) else {
        return u64::MAX;
    };
    let Some(tf_rsp) = crate::sched::proc_tf_rsp(pid) else {
        return u64::MAX;
    };
    let tf = unsafe { &mut *(tf_rsp as *mut SyscallFrame) };
    let user_ptr = tf.rsi;
    let max_len = core::cmp::min(tf.rdx as usize, 1024usize);
    let n = core::cmp::min(core::cmp::min(max_len, 256usize), msg.len());

    if user_copy_out_in(cr3, user_ptr, &msg[..n]).is_none() {
        return u64::MAX;
    }

    tf.rax = n as u64;
    tf.rdx = 0;
    if xfer_ep != 0 {
        if let Some(new_cap) = crate::sched::cap_alloc_for(pid, xfer_ep) {
            tf.rdx = new_cap as u64;
        }
    }
    crate::sched::wake(pid);
    n as u64
}

fn current_user_pml4() -> u64 {
    let cr3: u64;
    unsafe {
        core::arch::asm!(
            "mov {}, cr3",
            out(reg) cr3,
            options(nomem, nostack, preserves_flags)
        )
    };
    cr3 & 0x000f_ffff_ffff_f000
}

fn user_virt_to_phys(virt: u64) -> Option<u64> {
    // Walk 4-level tables. Require U=1 at every level and leaf present.
    const MASK: u64 = 0x000f_ffff_ffff_f000;
    const PTE_P: u64 = 1 << 0;
    const PTE_U: u64 = 1 << 2;

    let pml4 = current_user_pml4();
    let pml4_i = ((virt >> 39) & 0x1ff) as usize;
    let pdpt_i = ((virt >> 30) & 0x1ff) as usize;
    let pd_i = ((virt >> 21) & 0x1ff) as usize;
    let pt_i = ((virt >> 12) & 0x1ff) as usize;
    let off = virt & 0xfff;

    unsafe fn rd(table: u64, idx: usize) -> u64 {
        core::ptr::read_volatile(paging::phys_to_virt_ptr::<u64>(table).add(idx))
    }

    let pml4e = unsafe { rd(pml4, pml4_i) };
    if (pml4e & (PTE_P | PTE_U)) != (PTE_P | PTE_U) {
        return None;
    }
    let pdpt = pml4e & MASK;

    let pdpte = unsafe { rd(pdpt, pdpt_i) };
    if (pdpte & (PTE_P | PTE_U)) != (PTE_P | PTE_U) {
        return None;
    }
    let pd = pdpte & MASK;

    let pde = unsafe { rd(pd, pd_i) };
    if (pde & (PTE_P | PTE_U)) != (PTE_P | PTE_U) {
        return None;
    }
    let pt = pde & MASK;

    let pte = unsafe { rd(pt, pt_i) };
    if (pte & (PTE_P | PTE_U)) != (PTE_P | PTE_U) {
        return None;
    }

    Some((pte & MASK) + off)
}

fn user_copy_in(dst: &mut [u8], user_ptr: u64) -> Option<()> {
    for (i, b) in dst.iter_mut().enumerate() {
        let v = user_ptr.wrapping_add(i as u64);
        let p = user_virt_to_phys(v)?;
        *b = unsafe { core::ptr::read_volatile(paging::phys_to_virt_ptr::<u8>(p)) };
    }
    Some(())
}

fn user_copy_out(user_ptr: u64, src: &[u8]) -> Option<()> {
    for (i, b) in src.iter().enumerate() {
        let v = user_ptr.wrapping_add(i as u64);
        let p = user_virt_to_phys(v)?;
        unsafe { core::ptr::write_volatile(paging::phys_to_virt_ptr::<u8>(p), *b) };
    }
    Some(())
}

global_asm!(
    r#"
.intel_syntax noprefix
.global mantra_trap_return
.type mantra_trap_return, @function
mantra_trap_return:
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rsi
    pop rdi
    pop rbp
    pop rdx
    pop rcx
    pop rbx
    pop rax
    iretq

.global mantra_timer_irq_stub
.type mantra_timer_irq_stub, @function
mantra_timer_irq_stub:
    // Save GPRs. Order matches `TrapFrame`.
    push rax
    push rbx
    push rcx
    push rdx
    push rbp
    push rdi
    push rsi
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15

    // Arg0 = &mut TrapFrame (current RSP)
    mov rdi, rsp

    // Call Rust handler on an aligned temporary stack, but keep the TF pointer.
    // Use RBX (callee-saved) to preserve the original RSP across the call.
    mov rbx, rsp
    and rsp, -16
    call mantra_timer_irq_rust
    mov rsp, rbx

    // If rax != 0, it is the new task's saved RSP (TrapFrame pointer).
    test rax, rax
    jz 1f
    mov rsp, rax
    // Switch address space for the selected process before returning to user.
    mov rcx, qword ptr [rip + MANTRA_NEXT_CR3]
    mov cr3, rcx
1:
    jmp mantra_trap_return
.att_syntax
"#
);

global_asm!(
    r#"
.intel_syntax noprefix
.global mantra_syscall80_stub
.type mantra_syscall80_stub, @function
mantra_syscall80_stub:
    // Save GPRs. Order matches `SyscallFrame`.
    push rax
    push rbx
    push rcx
    push rdx
    push rbp
    push rdi
    push rsi
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15

    // Arg0 = &mut SyscallFrame (current RSP)
    mov rdi, rsp

    // Call Rust handler on aligned stack, but keep the frame pointer.
    // Use RBX (callee-saved) to preserve the original RSP across the call.
    mov rbx, rsp
    and rsp, -16
    call mantra_syscall80_rust
    mov rsp, rbx

    // If rax != 0, it is the next task's saved RSP (SyscallFrame/TrapFrame pointer).
    test rax, rax
    jz 1f
    mov rsp, rax
    // Switch address space for the selected process before returning to user.
    mov rcx, qword ptr [rip + MANTRA_NEXT_CR3]
    mov cr3, rcx
1:
    jmp mantra_trap_return
.att_syntax
"#
);
