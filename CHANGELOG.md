# Chalybs CHANGELOG

## v0.3.4 — Deterministic VFIO Staging & Verification (PCI Phase 5–6)
**Release Date:** TBD

### Added
- **PCI Phase 5:** Deterministic VFIO action planning for all passthrough devices.
- **PCI Phase 6:** Full VFIO execution + post-bind verification pipeline.
- `vfio::stage_pci_devices_for_vm` integrated into VM state machine.
- Strict ordering: all unbinds occur before all binds.
- Deterministic and idempotent sysfs interaction (`unbind`, `bind`).

### Changed
- Improved internal documentation for VFIO execution behavior.
- pci.rs corrected and restored to full baseline with clippy-clean strategy.
- Safety model wiring aligned with earlier PCI phases.

### Removed
- None. No regressions or structural reductions.

---

## v0.3.3 — GPU Driver Detection, Unbind Safety Simulation, Unified Architecture Doc
**Release Date:** TBD

### Added
- **PCI Phase 2:** GPU driver detection and safety classification.
- **PCI Phase 3:** Read-only unbind feasibility simulation (IOMMU-group aware).
- **PCI Phase 4 foundation:** VFIO bind/unbind helper functions.
- **Unified Super-Doc:** `CHALYBS_EXECUTION_AND_ARCHITECTURE.md`

### Changed
- Minor cpuset documentation fixes for clippy.
- Clarified PCI preflight flow in state machine docs.

### Removed
- Several superseded docs merged into unified architecture document.

---

## v0.3.2 — PCI Inventory & Policy Foundations
- Full PCI inventory from sysfs.
- Single-GPU safety rules.
- PCI policy preflight integration.

## v0.3.1 — Initial QEMU Launch Pipeline
- VM state machine.
- cpuset, IRQ/MSI management.

## v0.3.0 — Project Resurrection & Rust Refactor
- Reorganization.
- Core crate structure established.
