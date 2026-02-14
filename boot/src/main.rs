#![no_main]
#![no_std]

use core::fmt::Write;
use core::mem;
use mantra_bootinfo::{BootInfo, MemoryRegion, PixelFormat as MantraPixelFormat, RegionKind};
use uefi::prelude::*;
use uefi::proto::console::gop::GraphicsOutput;
use uefi::proto::console::gop::PixelFormat as UefiPixelFormat;
use uefi::proto::media::file::{File, FileAttribute, FileMode, FileType};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::table::boot::{AllocateType, MemoryType};
use uefi::Identify;
use xmas_elf::program::Type;
use xmas_elf::ElfFile;

#[entry]
fn main(image: Handle, mut st: SystemTable<Boot>) -> Status {
    uefi_services::init(&mut st).unwrap();
    writeln!(st.stdout(), "MantraBoot: UEFI started").unwrap();

    // Capture framebuffer details early (before ExitBootServices).
    let fb_info = {
        let bs = st.boot_services();
        let handles = bs
            .locate_handle_buffer(uefi::table::boot::SearchType::ByProtocol(
                &GraphicsOutput::GUID,
            ))
            .unwrap();

        let mut gop = bs
            .open_protocol_exclusive::<GraphicsOutput>(handles[0])
            .unwrap();

        let mode = gop.current_mode_info();
        let (w, h) = mode.resolution();
        let stride = mode.stride();

        let format = match mode.pixel_format() {
            UefiPixelFormat::Rgb => MantraPixelFormat::Rgb,
            UefiPixelFormat::Bgr => MantraPixelFormat::Bgr,
            _ => MantraPixelFormat::Unknown,
        };

        let mut fb = gop.frame_buffer();
        (
            fb.as_mut_ptr() as u64,
            fb.size() as u64,
            w as u32,
            h as u32,
            stride as u32,
            format as u32,
        )
    };

    // -------- FILE LOAD SCOPE --------
    // Load kernel ELF file into a temporary buffer.
    let (kernel_file_addr, file_size) = {
        let bs = st.boot_services();

        let handles = bs
            .locate_handle_buffer(uefi::table::boot::SearchType::ByProtocol(
                &SimpleFileSystem::GUID,
            ))
            .unwrap();

        let mut fs = bs
            .open_protocol_exclusive::<SimpleFileSystem>(handles[0])
            .unwrap();

        let mut root = fs.open_volume().unwrap();

        let kernel = root
            .open(
                cstr16!("\\kernel.elf"),
                FileMode::Read,
                FileAttribute::empty(),
            )
            .unwrap();

        let mut file = match kernel.into_type().unwrap() {
            FileType::Regular(f) => f,
            _ => return Status::LOAD_ERROR,
        };

        let mut info_buf = [0u8; 512];
        let info = file
            .get_info::<uefi::proto::media::file::FileInfo>(&mut info_buf)
            .unwrap();

        let file_size = info.file_size() as usize;

        let pages = (file_size + 4095) / 4096;

        let kernel_file_addr = bs
            .allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, pages)
            .unwrap();

        let buffer =
            unsafe { core::slice::from_raw_parts_mut(kernel_file_addr as *mut u8, pages * 4096) };

        file.read(&mut buffer[..file_size]).unwrap();

        (kernel_file_addr, file_size)
    };
    // -------- END SCOPE (bs borrow dropped) --------

    writeln!(st.stdout(), "Kernel size: {}", file_size).unwrap();

    // Parse + load the ELF into memory at its intended addresses.
    // Your kernel linker script starts at 0x100000, so we load PT_LOAD segments
    // to their p_vaddr addresses (identity-mapped physical addresses in OVMF).
    let (entry_point, load_base, load_end) = {
        let bs = st.boot_services();

        let kernel_file_bytes =
            unsafe { core::slice::from_raw_parts(kernel_file_addr as *const u8, file_size) };

        let elf = match ElfFile::new(kernel_file_bytes) {
            Ok(e) => e,
            Err(_) => {
                writeln!(st.stdout(), "Kernel ELF parse failed").ok();
                return Status::LOAD_ERROR;
            }
        };

        let mut min_addr: u64 = u64::MAX;
        let mut max_addr: u64 = 0;
        for ph in elf.program_iter() {
            let Ok(Type::Load) = ph.get_type() else {
                continue;
            };
            let start = ph.virtual_addr();
            let end = start.saturating_add(ph.mem_size());
            min_addr = core::cmp::min(min_addr, start);
            max_addr = core::cmp::max(max_addr, end);
        }

        if min_addr == u64::MAX || max_addr <= min_addr {
            writeln!(st.stdout(), "Kernel ELF had no PT_LOAD segments").ok();
            return Status::LOAD_ERROR;
        }

        let load_base = min_addr & !0xfff;
        let load_end = (max_addr + 0xfff) & !0xfff;
        let pages = ((load_end - load_base) / 4096) as usize;

        match bs.allocate_pages(
            AllocateType::Address(load_base),
            MemoryType::LOADER_DATA,
            pages,
        ) {
            Ok(addr) if addr == load_base => {}
            Ok(_) => {
                writeln!(st.stdout(), "Kernel alloc returned unexpected address").ok();
                return Status::LOAD_ERROR;
            }
            Err(_) => {
                writeln!(st.stdout(), "Kernel alloc at {:#x} failed", load_base).ok();
                return Status::OUT_OF_RESOURCES;
            }
        }

        let load_mem =
            unsafe { core::slice::from_raw_parts_mut(load_base as *mut u8, pages * 4096) };
        load_mem.fill(0);

        for ph in elf.program_iter() {
            let Ok(Type::Load) = ph.get_type() else {
                continue;
            };
            let vaddr = ph.virtual_addr();
            let memsz = ph.mem_size() as usize;
            let filesz = ph.file_size() as usize;
            let off = ph.offset() as usize;

            if filesz > memsz {
                writeln!(st.stdout(), "Kernel segment filesz > memsz").ok();
                return Status::LOAD_ERROR;
            }
            if off.saturating_add(filesz) > kernel_file_bytes.len() {
                writeln!(st.stdout(), "Kernel segment out of file bounds").ok();
                return Status::LOAD_ERROR;
            }

            let dst_off = (vaddr - load_base) as usize;
            if dst_off.saturating_add(memsz) > load_mem.len() {
                writeln!(st.stdout(), "Kernel segment out of load bounds").ok();
                return Status::LOAD_ERROR;
            }

            load_mem[dst_off..dst_off + filesz]
                .copy_from_slice(&kernel_file_bytes[off..off + filesz]);
        }

        let entry_point = elf.header.pt2.entry_point();
        (entry_point, load_base, load_end)
    };

    writeln!(st.stdout(), "Kernel loaded at base {:#x}", load_base).unwrap();
    writeln!(st.stdout(), "Kernel entry point {:#x}", entry_point).unwrap();

    // Allocate memory for our stable boot info + translated memory regions.
    // Must be done before ExitBootServices.
    let regions_pages: usize = 8; // 32 KiB
    let (boot_info_ptr, regions_addr, regions_cap) = {
        let bs = st.boot_services();

        let boot_info_addr = bs
            .allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 1)
            .unwrap();

        let regions_addr = bs
            .allocate_pages(
                AllocateType::AnyPages,
                MemoryType::LOADER_DATA,
                regions_pages,
            )
            .unwrap();
        let regions_cap = (regions_pages * 4096) / mem::size_of::<MemoryRegion>();

        let bi = BootInfo {
            magic: BootInfo::MAGIC,
            version: BootInfo::VERSION,
            fb_base: fb_info.0,
            fb_size: fb_info.1,
            fb_width: fb_info.2,
            fb_height: fb_info.3,
            fb_stride: fb_info.4,
            fb_format: fb_info.5,
            regions_ptr: regions_addr,
            regions_len: 0,
            _reserved0: 0,
            kernel_phys_base: load_base,
            kernel_phys_end: load_end,
        };

        unsafe {
            core::ptr::write(boot_info_addr as *mut BootInfo, bi);
        }

        (boot_info_addr as *mut BootInfo, regions_addr, regions_cap)
    };

    // Exit boot services (returns the UEFI memory map).
    let (_rt, mut mmap) = st.exit_boot_services(MemoryType::LOADER_DATA);

    // Translate the UEFI memory map into a stable format for the kernel.
    let out_regions =
        unsafe { core::slice::from_raw_parts_mut(regions_addr as *mut MemoryRegion, regions_cap) };

    let mut out_len: usize = 0;
    let mut push = |base: u64, len: u64, kind: RegionKind| {
        if len == 0 || out_len >= out_regions.len() {
            return;
        }
        out_regions[out_len] = MemoryRegion {
            base,
            len,
            kind: kind as u32,
            _reserved: 0,
        };
        out_len += 1;
    };

    mmap.sort();
    for desc in mmap.entries() {
        let base = desc.phys_start as u64;
        let len = desc.page_count.saturating_mul(4096);
        let kind = match desc.ty {
            uefi::table::boot::MemoryType::CONVENTIONAL => RegionKind::Usable,
            uefi::table::boot::MemoryType::ACPI_RECLAIM => RegionKind::AcpiReclaim,
            uefi::table::boot::MemoryType::ACPI_NON_VOLATILE => RegionKind::AcpiNvs,
            uefi::table::boot::MemoryType::MMIO
            | uefi::table::boot::MemoryType::MMIO_PORT_SPACE => RegionKind::Mmio,
            _ => RegionKind::Reserved,
        };
        push(base, len, kind);
    }

    // Add explicit reserved ranges used by our OS components.
    push(
        load_base,
        load_end.saturating_sub(load_base),
        RegionKind::Kernel,
    );
    push(fb_info.0, fb_info.1, RegionKind::Framebuffer);
    push(boot_info_ptr as u64, 4096, RegionKind::Boot);
    push(
        regions_addr,
        (regions_pages as u64) * 4096,
        RegionKind::Boot,
    );

    unsafe {
        (*boot_info_ptr).regions_len = out_len as u32;
    }

    // Jump to kernel
    // Use SysV ABI explicitly so it matches the kernel target.
    let entry: extern "sysv64" fn(*const BootInfo) -> ! =
        unsafe { core::mem::transmute(entry_point as usize) };

    entry(boot_info_ptr.cast_const());
}
