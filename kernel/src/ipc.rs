use core::sync::atomic::{AtomicUsize, Ordering};

use crate::sched;

const MAX_ENDPOINTS: usize = 32;
const MAX_MSG: usize = 256;
const Q_LEN: usize = 32;
const MAX_WAITERS: usize = 8;

#[derive(Copy, Clone)]
struct Msg {
    len: u16,
    // Endpoint ID (1-based) transferred with this message, or 0 for none.
    xfer_ep: u32,
    data: [u8; MAX_MSG],
}

const EMPTY_MSG: Msg = Msg {
    len: 0,
    xfer_ep: 0,
    data: [0; MAX_MSG],
};

struct Endpoint {
    head: AtomicUsize,
    tail: AtomicUsize,
    buf: [Msg; Q_LEN],
    wait_head: AtomicUsize,
    wait_tail: AtomicUsize,
    waiters: [u8; MAX_WAITERS],
}

static mut ENDPOINTS: [Endpoint; MAX_ENDPOINTS] = [const {
    Endpoint {
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        buf: [EMPTY_MSG; Q_LEN],
        wait_head: AtomicUsize::new(0),
        wait_tail: AtomicUsize::new(0),
        waiters: [0; MAX_WAITERS],
    }
}; MAX_ENDPOINTS];

static NEXT_EP: AtomicUsize = AtomicUsize::new(0);

pub fn endpoint_alloc() -> Option<u32> {
    let i = NEXT_EP.fetch_add(1, Ordering::Relaxed);
    if i >= MAX_ENDPOINTS {
        return None;
    }
    // Endpoint IDs are 1-based so 0 can be used as "empty" in cap tables.
    Some((i as u32) + 1)
}

pub fn ep_create() -> u64 {
    let Some(ep) = endpoint_alloc() else {
        return u64::MAX;
    };
    let Some(cap) = sched::cap_alloc_current(ep) else {
        return u64::MAX;
    };
    cap as u64
}

pub fn waiter_push(endpoint_id: u32, pid: usize) -> bool {
    if endpoint_id == 0 || pid > u8::MAX as usize {
        return false;
    }
    let epi = (endpoint_id as usize).wrapping_sub(1);
    if epi >= MAX_ENDPOINTS {
        return false;
    }
    unsafe {
        let ep = &mut ENDPOINTS[epi];
        let head = ep.wait_head.load(Ordering::Acquire);
        let tail = ep.wait_tail.load(Ordering::Relaxed);
        if (tail.wrapping_add(1) % MAX_WAITERS) == head {
            return false; // full
        }
        let slot = tail % MAX_WAITERS;
        ep.waiters[slot] = pid as u8;
        ep.wait_tail.store(tail.wrapping_add(1), Ordering::Release);
        true
    }
}

pub fn waiter_pop(endpoint_id: u32) -> Option<usize> {
    if endpoint_id == 0 {
        return None;
    }
    let epi = (endpoint_id as usize).wrapping_sub(1);
    if epi >= MAX_ENDPOINTS {
        return None;
    }
    unsafe {
        let ep = &mut ENDPOINTS[epi];
        let head = ep.wait_head.load(Ordering::Acquire);
        let tail = ep.wait_tail.load(Ordering::Relaxed);
        if head == tail {
            return None;
        }
        let slot = head % MAX_WAITERS;
        let pid = ep.waiters[slot] as usize;
        ep.wait_head.store(head.wrapping_add(1), Ordering::Release);
        Some(pid)
    }
}

pub fn ep_send(cap: u32, msg: &[u8]) -> u64 {
    ep_send_cap(cap, msg, 0)
}

pub fn ep_send_cap(cap: u32, msg: &[u8], xfer_ep: u32) -> u64 {
    let Some(epi) = sched::cap_lookup_current(cap) else {
        return u64::MAX;
    };
    let epi = (epi as usize).wrapping_sub(1);
    if epi >= MAX_ENDPOINTS {
        return u64::MAX;
    }

    let n = core::cmp::min(msg.len(), MAX_MSG);
    unsafe {
        let ep = &mut ENDPOINTS[epi];
        let head = ep.head.load(Ordering::Relaxed);
        let tail = ep.tail.load(Ordering::Relaxed);
        if (tail.wrapping_add(1) % Q_LEN) == head {
            return u64::MAX - 1; // full
        }
        let slot = tail % Q_LEN;
        ep.buf[slot].len = n as u16;
        ep.buf[slot].xfer_ep = xfer_ep;
        ep.buf[slot].data[..n].copy_from_slice(&msg[..n]);
        ep.tail.store(tail.wrapping_add(1), Ordering::Release);
    }
    n as u64
}

pub fn ep_recv(cap: u32, out: &mut [u8]) -> u64 {
    let (n, _cap) = ep_recv_cap(cap, out);
    n
}

pub fn ep_recv_cap(cap: u32, out: &mut [u8]) -> (u64, u32) {
    let Some(epi) = sched::cap_lookup_current(cap) else {
        return (u64::MAX, 0);
    };
    let epi = (epi as usize).wrapping_sub(1);
    if epi >= MAX_ENDPOINTS {
        return (u64::MAX, 0);
    }

    unsafe {
        let ep = &mut ENDPOINTS[epi];
        let head = ep.head.load(Ordering::Acquire);
        let tail = ep.tail.load(Ordering::Relaxed);
        if head == tail {
            return (u64::MAX - 2, 0); // empty
        }
        let slot = head % Q_LEN;
        let len = ep.buf[slot].len as usize;
        let n = core::cmp::min(len, out.len());
        let xfer_ep = ep.buf[slot].xfer_ep;
        out[..n].copy_from_slice(&ep.buf[slot].data[..n]);
        ep.head.store(head.wrapping_add(1), Ordering::Release);
        (n as u64, xfer_ep)
    }
}
