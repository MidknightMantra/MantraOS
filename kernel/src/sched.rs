use crate::arch::x86_64::gdt;
use crate::arch::x86_64::isr::TrapFrame;
use crate::serial;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

const MAX_PROCS: usize = 8;

#[derive(Copy, Clone)]
struct Proc {
    tf_rsp: u64,      // saved TrapFrame pointer (kernel RSP)
    kstack_top: u64,  // TSS.rsp0 to use for this task
    cr3: u64,         // address space root
    caps: [u32; 32],  // cap -> endpoint id (0 = empty)
    alive: bool,
    runnable: bool,
    // Bring-up blocking model: a proc can block on an endpoint receive.
    blocked_ep: u32, // endpoint id (1-based) or 0
}

static INITED: AtomicBool = AtomicBool::new(false);
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static TICKS: AtomicU64 = AtomicU64::new(0);

#[no_mangle]
pub static mut MANTRA_NEXT_CR3: u64 = 0;

static mut PROCS: [Proc; MAX_PROCS] = [const {
    Proc {
        tf_rsp: 0,
        kstack_top: 0,
        cr3: 0,
        caps: [0; 32],
        alive: false,
        runnable: false,
        blocked_ep: 0,
    }
}; MAX_PROCS];

pub fn install_first(tf_rsp: u64, kstack_top: u64, cr3: u64) {
    unsafe {
        PROCS[0] = Proc {
            tf_rsp,
            kstack_top,
            cr3,
            caps: [0; 32],
            alive: true,
            runnable: true,
            blocked_ep: 0,
        };
        for p in PROCS.iter_mut().skip(1) {
            *p = Proc {
                tf_rsp: 0,
                kstack_top: 0,
                cr3: 0,
                caps: [0; 32],
                alive: false,
                runnable: false,
                blocked_ep: 0,
            };
        }
        MANTRA_NEXT_CR3 = cr3;
    }
    CURRENT.store(0, Ordering::Release);
    INITED.store(true, Ordering::Release);
    serial::write_str("sched: installed proc0\n");
}

pub fn current_pid() -> usize {
    CURRENT.load(Ordering::Relaxed)
}

pub fn spawn_proc(tf_rsp: u64, kstack_top: u64, cr3: u64) -> Option<usize> {
    unsafe {
        for (pid, p) in PROCS.iter_mut().enumerate() {
            if !p.alive {
                *p = Proc {
                    tf_rsp,
                    kstack_top,
                    cr3,
                    caps: [0; 32],
                    alive: true,
                    runnable: true,
                    blocked_ep: 0,
                };
                return Some(pid);
            }
        }
    }
    None
}

pub fn proc_cr3(pid: usize) -> Option<u64> {
    if pid >= MAX_PROCS {
        return None;
    }
    unsafe { Some(PROCS[pid].cr3) }
}

pub fn proc_tf_rsp(pid: usize) -> Option<u64> {
    if pid >= MAX_PROCS {
        return None;
    }
    unsafe { Some(PROCS[pid].tf_rsp) }
}

pub fn wake(pid: usize) {
    if pid >= MAX_PROCS {
        return;
    }
    unsafe {
        if PROCS[pid].alive {
            PROCS[pid].runnable = true;
            PROCS[pid].blocked_ep = 0;
        }
    }
}

pub fn block_current_on_ep(ep_id: u32) {
    let pid = current_pid();
    unsafe {
        PROCS[pid].runnable = false;
        PROCS[pid].blocked_ep = ep_id;
    }
}

pub fn has_other_runnable() -> bool {
    let cur = current_pid();
    unsafe {
        for (pid, p) in PROCS.iter().enumerate() {
            if pid != cur && p.alive && p.runnable {
                return true;
            }
        }
    }
    false
}

fn pick_next_runnable(cur: usize) -> usize {
    let mut next = cur;
    for _ in 0..MAX_PROCS {
        next = (next + 1) % MAX_PROCS;
        unsafe {
            if PROCS[next].alive && PROCS[next].runnable {
                return next;
            }
        }
    }
    cur
}

fn switch_from(cur_tf: u64) -> u64 {
    let cur = CURRENT.load(Ordering::Relaxed);
    unsafe {
        PROCS[cur].tf_rsp = cur_tf;
    }

    let next = pick_next_runnable(cur);
    if next == cur {
        return 0;
    }

    unsafe {
        gdt::set_rsp0(PROCS[next].kstack_top);
        MANTRA_NEXT_CR3 = PROCS[next].cr3;
    }
    CURRENT.store(next, Ordering::Relaxed);
    unsafe { PROCS[next].tf_rsp }
}

pub fn yield_from_syscall(current_tf: u64) -> u64 {
    if !INITED.load(Ordering::Acquire) {
        return 0;
    }
    switch_from(current_tf)
}

pub fn cap_alloc_for(pid: usize, endpoint_id: u32) -> Option<u32> {
    if pid >= MAX_PROCS || endpoint_id == 0 {
        return None;
    }
    unsafe {
        for (i, slot) in PROCS[pid].caps.iter_mut().enumerate() {
            if *slot == 0 {
                *slot = endpoint_id;
                return Some((i as u32) + 1);
            }
        }
    }
    None
}

pub fn cap_alloc_current(endpoint_id: u32) -> Option<u32> {
    cap_alloc_for(current_pid(), endpoint_id)
}

pub fn cap_lookup_current(cap: u32) -> Option<u32> {
    if cap == 0 {
        return None;
    }
    let idx = (cap as usize).wrapping_sub(1);
    let pid = current_pid();
    if pid >= MAX_PROCS || idx >= 32 {
        return None;
    }
    unsafe {
        let ep = PROCS[pid].caps[idx];
        if ep == 0 { None } else { Some(ep) }
    }
}

pub fn on_timer_irq(current_tf: *mut TrapFrame) -> u64 {
    if !INITED.load(Ordering::Acquire) {
        return 0;
    }

    let t = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    let cur = CURRENT.load(Ordering::Relaxed);
    // Save and potentially switch. If all other tasks are blocked, this returns 0 and we keep running cur.
    let next_tf = switch_from(current_tf as u64);
    if next_tf == 0 {
        return 0;
    }
    let next = CURRENT.load(Ordering::Relaxed);

    if (t % 100) == 0 {
        serial::write_str("sched: tick=");
        serial::write_dec_u64(t);
        serial::write_str(" switch ");
        serial::write_dec_u64(cur as u64);
        serial::write_str("->");
        serial::write_dec_u64(next as u64);
        serial::write_str("\n");
    }
    next_tf
}
