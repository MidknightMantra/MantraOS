# MantraOS Specification

## 1. Vision
MantraOS is a capability-secure, high-performance microkernel operating system
designed for:

- Performance through modern SMP-aware scheduling and fast IPC
- Security through strict capability-based access control
- Reliability via service isolation and restartability
- First-class user experience
- Built-in research instrumentation and modularity

The system is designed for long-term evolution across desktop, server, and embedded targets.

---

## 2. Core Principles

1. Minimal Trusted Computing Base
2. No ambient authority (capabilities only)
3. Userland drivers and services
4. Async-first kernel design
5. Observability by default
6. Reproducible builds

---

## 3. Target Architectures

Tier 1:
- x86_64 (UEFI)

Tier 2:
- ARM64

---

## 4. Kernel: MantraCore

### Responsibilities

- Thread scheduling
- Virtual memory management
- Interprocess communication (IPC)
- Capability enforcement
- Interrupt handling (minimal)

### Non-responsibilities

- Filesystems
- Networking
- Drivers
- UI
- Policy

All of the above live in userland.

---

## 5. Process Model

Each process has:

- Address space
- Thread list
- Capability table
- IPC endpoints

Processes start with **zero privileges**.

---

## 6. Capability System (MantraCaps)

Capabilities are:

- Unforgeable
- Transferable via IPC
- Fine-grained
- Revocable (later milestone)

They control access to:

- Memory regions
- IPC endpoints
- Devices
- Files
- System services

No global root user exists.

Administrative authority is policy-driven.

---

## 7. IPC System (MantraIPC)

### Phase 1
- Copy-based message passing

### Phase 2
- Shared memory channels
- Zero-copy transfer for large payloads

### Properties

- Async
- Handle transfer
- Event-based waiting

---

## 8. Scheduling

Phase 1:
- Round-robin

Phase 2:
- Per-core run queues
- Priority classes
- Real-time class

Goals:

- Scalable to many cores
- Low latency IPC wakeups

---

## 9. Memory Management

- 4-level paging (x86_64)
- Userspace/kernel isolation
- Guard pages
- Lazy allocation (later)

Allocator:

Phase 1: simple linked-list allocator  
Phase 2: slab/region allocator

---

## 10. System Servers

### MantraInit
- First user process
- Service supervisor
- Restart crashed services

### MantraCapsd
- Security policy engine
- Capability minting

### MantraDevd
- Device discovery
- Driver lifecycle

### MantraLogd / Traced
- Logging
- System tracing pipeline

### MantraFSd
Filesystem service.

### MantraNetd
Networking stack.

---

## 11. Drivers

All drivers are:

- Userland processes
- Sandboxed
- Restartable

Driver communication happens via IPC.

---

## 12. Graphics & UX

MantraShell:

- Compositor
- Window manager
- Input routing
- Permission prompts

---

## 13. Security Model

- Capability-based access control
- Signed system components (future)
- Measured boot (future)
- Per-service sandboxing

---

## 14. Research Mode (MantraLab)

- Pluggable schedulers
- IPC strategy switching
- System-wide tracing
- Deterministic builds

---

## 15. Build System

- Reproducible
- Pinned toolchains
- Image-based output

---

## 16. Boot Flow

UEFI → Bootloader → MantraCore → MantraInit → system services → MantraShell
