# Chalybs Roadmap (Rust Rewrite Edition)

This roadmap tracks the current, intermediate, and long-term goals of the
Chalybs Rust rewrite. The final scope represents the intended steady-state
feature set: deterministic VFIO virtualization on Linux with predictable
behavior, clarity of configuration, and safety by default.

---

# Current Milestone (v0.3.x series)
## PCI Foundations & Safety Layer
✔ Deterministic PCI inventory from sysfs  
✔ GPU passthrough safety (single-GPU protection, zero-GPU guardrails)  
⬜ PCI Phase 2: GPU driver detection (amdgpu/nvidia/vfio-pci)  
⬜ PCI Phase 3: Safe unbind/bind orchestration  
⬜ PCI Phase 4: IOMMU group validation + strict mode  
⬜ PCI Phase 5: Capability graph (host capabilities → allowed VM modes)

## CPU / IRQ Affinity
✔ cpuset creation  
✔ NUMA-aware vCPU scheduling  
✔ IRQ detection + pinning  
⬜ NUMA-policy-based IRQ placement refinement  
⬜ Dynamic IRQ migration on VM restart

## Configuration
✔ Clean TOML schema  
✔ Device list resolution  
⬜ Mode definitions (Dedicated GPU, Hybrid, Single-GPU Safe, IGPU-Primary)  
⬜ Validation of unsupported combinations  
⬜ Host capabilities gating (e.g., "no IOMMU → no passthrough")

---

# Intermediate Milestones (v0.4.x series)
## PCI Driver Lifecycle Engine (Core Feature)
A deterministic engine that:
- Detects current GPU driver (amdgpu/nvidia/vfio-pci)
- Evaluates safe handoff
- Quiesces userspace consumers (DRM, console, fbcon)
- Performs orderly unbind → bind to vfio-pci
- Starts VM
- On shutdown: unbind vfio-pci → rebind host driver
- Optional hotplug support for secondary GPUs

## Device Mode System
Introduce structured VM operational modes:
- `dedicated_gpu`
- `single_gpu_mode`
- `dual_gpu_switch`
- `igpu_primary`
- `auto`
These will be validated against host capabilities and PCI topology.

## Improved Peripheral Hooks
- Tasmota: async control + retries  
- DDC: safe probe logic + error recovery  
- LookingGlass: shared memory lifecycle verification  

---

# Long-Term Milestones (v0.5.x – v0.9.x)
## Full Deterministic Virtualization Stack
- Machine-readable VM status API (daemon)
- CLI for lifecycle: `chalybs start`, `stop`, `attach`, `events`  
- Persistent VM registry  
- Advanced logging pipeline (journald optional)

## Storage / Networking Expansion
- PCIe NVMe passthrough lifecycle (similar to GPU)  
- PCIe NIC passthrough safety (VFIO group scanning + checks)  
- vhost-user integration

## Monitoring & Introspection
- Per-thread stats  
- IRQ distribution visualizer  
- PCI device health tracking

## Testing & Validation Framework
- Synthetic PCI trees  
- Regression suite for PCI and NUMA policies  
- Sandbox VM mode for dry-run validation  

---

# Final Scope (v1.0.0)
A fully deterministic VFIO + QEMU orchestration layer that:

- Bootstraps VMs with strict CPU, IRQ, PCI, and NUMA policy  
- Ensures zero nondeterministic behavior  
- Protects host from unsafe passthrough  
- Unifies driver lifecycle, policy, and configuration  
- Has predictable, reproducible operation on any Linux host with IOMMU  
- Has no external dependencies (no lspci, no shell calls, no systemd magic)  
- Provides a stable CLI + daemon API  
- Supports multi-VM scenarios with clear resource isolation  

Chalybs reaches 1.0 when the system is deterministic, reproducible, safe, and
fully driver-lifecycle aware.

---
