# MantraOS Development Roadmap

## M0 — Boot to screen
Goal: kernel runs under QEMU/UEFI

- UEFI bootloader
- Enter long mode
- Framebuffer text output
- Serial logging

SUCCESS: "MantraCore alive" printed on screen.

---

## M1 — Core kernel
- IDT
- Timer interrupt
- Physical memory manager
- Paging
- Kernel heap
- Thread struct
- Context switch
- Round-robin scheduler

SUCCESS: two kernel threads switching.

---

## M2 — IPC + Capabilities
- Process abstraction
- Userspace entry
- Handle table
- Basic capability type
- Message passing IPC
- Spawn MantraInit

SUCCESS: kernel ↔ init message exchange.

THIS IS THE BIRTH OF MANTRAOS.

---

## M3 — First system
- MantraInit service supervisor
- Console server
- Keyboard driver (userland)
- Panic isolation

SUCCESS: userland service restart without kernel crash.

---

## M4 — Storage seed
- Block device driver
- Read-only filesystem
- Load userspace binaries from disk

SUCCESS: init launched from filesystem.

---

## M5 — Security policy
- MantraCapsd
- Capability minting rules
- Permission prompts (text mode)

SUCCESS: service denied access without capability.

---

## M6 — UX seed
- Basic compositor
- Single client window
- Input routing

SUCCESS: graphical userspace program.

---

## M7 — Performance phase
- Shared memory IPC
- Per-core scheduler
- Async syscalls

---

## M8 — Self-hosting tools
- Package format
- System updater
- SDK preview

---

## LONG TERM

- ARM64 port
- Signed system components
- Network stack
- Audio stack
- Multi-user model
