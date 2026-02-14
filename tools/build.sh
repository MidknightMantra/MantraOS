#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_DIR="${ROOT_DIR}/build"

mkdir -p "${BUILD_DIR}/EFI/BOOT"

# Userland init (ELF loaded by the kernel)
RUSTFLAGS="-C link-arg=-T${ROOT_DIR}/userland/init/linker.ld" cargo \
  -Z json-target-spec \
  -Z build-std=core,compiler_builtins \
  -Z build-std-features=compiler-builtins-mem \
  build -p mantra-init --target userland/x86_64-mantra-user.json
cp -f "${ROOT_DIR}/target/x86_64-mantra-user/debug/mantra-init" "${BUILD_DIR}/init.elf"

# Bootloader (UEFI app)
cargo build -p mantra-boot --target x86_64-unknown-uefi
cp -f "${ROOT_DIR}/target/x86_64-unknown-uefi/debug/mantra-boot.efi" \
  "${BUILD_DIR}/EFI/BOOT/BOOTX64.EFI"

# Kernel (custom JSON target; build core/compiler_builtins from source)
RUSTFLAGS="-C link-arg=-T${ROOT_DIR}/kernel/linker.ld" \
MANTRA_INIT_ELF="${BUILD_DIR}/init.elf" cargo \
  -Z json-target-spec \
  -Z build-std=core,alloc,compiler_builtins \
  -Z build-std-features=compiler-builtins-mem \
  build -p mantracore --target kernel/x86_64-mantra.json
cp -f "${ROOT_DIR}/target/x86_64-mantra/debug/mantracore" "${BUILD_DIR}/kernel.elf"

# If OVMF drops into the UEFI shell, this will auto-run our bootloader.
cat >"${BUILD_DIR}/startup.nsh" <<'EOF'
\EFI\BOOT\BOOTX64.EFI
EOF

echo "Built:"
echo "  ${BUILD_DIR}/EFI/BOOT/BOOTX64.EFI"
echo "  ${BUILD_DIR}/kernel.elf"
echo "  ${BUILD_DIR}/init.elf"
