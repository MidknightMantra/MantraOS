#![no_std]

pub mod syscall {
    pub const PUTC: u64 = 1;
    pub const YIELD_: u64 = 2;
    pub const WRITE: u64 = 3; // (ptr,len) -> bytes_written

    // IPC (capability-based, bring-up API).
    pub const IPC_EP_CREATE: u64 = 0x10;
    pub const IPC_SEND: u64 = 0x11; // (cap, ptr, len) -> bytes_sent or err
    pub const IPC_RECV: u64 = 0x12; // (cap, ptr, max_len) -> bytes_recv or err
    pub const IPC_SEND_CAP: u64 = 0x13; // (cap, ptr, len, xfer_cap) -> bytes_sent or err
    pub const IPC_RECV_CAP: u64 = 0x14; // (cap, ptr, max_len) -> bytes_recv or err; out: rdx=received_cap (0 if none)

    // Process management (bring-up).
    pub const PROC_SPAWN: u64 = 0x20; // (prog_id, role, share_cap) -> pid or err
}
