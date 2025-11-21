# Chalybs v0.3.3 Release Notes

This release introduces major advancements in PCI/GPU introspection and safety, along with a fully unified architectural reference document.

---

## 🆕 Major Features

### 1. GPU Driver Detection (PCI Phase 2)
Chalybs can now classify host GPUs by:
- Bound kernel driver (vfio‑pci, amdgpu, radeon, nvidia, nouveau, or other)
- Host safety class (HostOwned, VfioReady, Unknown)

### 2. GPU Unbind Safety Simulation (PCI Phase 3)
Chalybs simulates whether unbinding a GPU appears:
- **Safe**
- **Risky** (requires operator review)
- **Unsafe** (cannot isolate—e.g., no IOMMU group)

This simulation is strictly read‑only and does *not* write to sysfs.

### 3. VFIO Bind/Unbind Helpers (PCI Phase 4 foundation)
Low‑level helpers were added:
- `unbind_current_driver`
- `bind_to_vfio_pci`

No automatic unbinding is performed yet—Phase 5 work.

---

## 📘 New Unified Architecture Document

A major improvement for maintainability:

### **`CHALYBS_EXECUTION_AND_ARCHITECTURE.md`**
This is now the authoritative reference for:
- Execution pipeline + QEMU/IRQ lifecycle
- NUMA + cpuset strategy (C2)
- PCI/GPU architecture, Phases 1–4
- Mode/capability architecture
- Peripheral hooks

This document replaces fragmented prior docs:
- `docs_pipeline.md`
- `PIPELINE_OVERVIEW.md`
- `docs_architecture.md`
- `MODE_CAPABILITY_ARCHITECTURE.md`
- `C2_NUMA_DERIVATION.md`

---

## 🧹 Additional Improvements
- cpuset module doc cleanup (clippy compliance).
- State machine comments expanded for clarity.
- Future PCI phases planned and documented.

---

## ✅ Compatibility
- Fully backward compatible with v0.3.2.
- No changes required to VM configs.

