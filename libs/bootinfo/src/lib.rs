#![no_std]

#[repr(C)]
#[derive(Copy, Clone)]
pub struct BootInfo {
    pub magic: u32,
    pub version: u32,

    // Framebuffer (UEFI GOP)
    pub fb_base: u64,
    pub fb_size: u64,
    pub fb_width: u32,
    pub fb_height: u32,
    pub fb_stride: u32, // pixels per scanline
    pub fb_format: u32, // PixelFormat as u32

    // Physical memory map (translated by the bootloader; stable layout).
    pub regions_ptr: u64, // *const MemoryRegion
    pub regions_len: u32,
    pub _reserved0: u32,

    // Loaded kernel physical range [kernel_phys_base, kernel_phys_end).
    pub kernel_phys_base: u64,
    pub kernel_phys_end: u64,
}

impl BootInfo {
    pub const MAGIC: u32 = 0x4D_41_4E_54; // "MANT"
    pub const VERSION: u32 = 2;
}

#[repr(u32)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum PixelFormat {
    Unknown = 0,
    Rgb = 1, // 0x00RRGGBB in memory as little-endian u32
    Bgr = 2, // 0x00BBGGRR in memory as little-endian u32
}

#[repr(u32)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum RegionKind {
    Unknown = 0,
    Usable = 1,
    Reserved = 2,
    AcpiReclaim = 3,
    AcpiNvs = 4,
    Mmio = 5,
    Kernel = 6,
    Boot = 7,
    Framebuffer = 8,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct MemoryRegion {
    pub base: u64,
    pub len: u64,
    pub kind: u32, // RegionKind as u32
    pub _reserved: u32,
}
