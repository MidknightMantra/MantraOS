#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
BUILD_DIR="${ROOT_DIR}/build"

QEMU=(qemu-system-x86_64 -m 512M)

if [[ -e /dev/kvm && -r /dev/kvm && -w /dev/kvm ]]; then
  QEMU+=(-accel kvm)
else
  QEMU+=(-accel tcg)
fi

# Prefer split OVMF code/vars if present, otherwise fall back to a monolithic OVMF firmware.
if [[ -r /usr/share/OVMF/OVMF_CODE.fd && -r /usr/share/OVMF/OVMF_VARS.fd ]]; then
  OVMF_VARS_RW="${BUILD_DIR}/OVMF_VARS.fd"
  if [[ ! -f "${OVMF_VARS_RW}" ]]; then
    cp /usr/share/OVMF/OVMF_VARS.fd "${OVMF_VARS_RW}"
  fi

  QEMU+=(
    -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE.fd
    -drive if=pflash,format=raw,file="${OVMF_VARS_RW}"
  )
elif [[ -r /usr/share/ovmf/OVMF.fd ]]; then
  QEMU+=(-bios /usr/share/ovmf/OVMF.fd)
else
  echo "Could not find OVMF firmware. Install an OVMF package (e.g. ovmf)." >&2
  exit 1
fi

QEMU+=(
  -drive format=raw,file="fat:rw:${BUILD_DIR}"
)

if (( $# )); then
  QEMU+=("$@")
fi

exec "${QEMU[@]}"
