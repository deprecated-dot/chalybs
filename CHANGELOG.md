# Chalybs CHANGELOG

## v0.3.3 — GPU Driver Detection, Unbind Safety Simulation, Unified Architecture Doc

**Release Date:** TBD

### Added
- **PCI Phase 2:** GPU driver detection and safety classification.
- **PCI Phase 3:** Read‑only unbind feasibility simulation (IOMMU‑group aware).
- **PCI Phase 4 foundation:** VFIO bind/unbind helper functions (no automation yet).
- **Unified Super‑Doc:** `CHALYBS_EXECUTION_AND_ARCHITECTURE.md`
  - Merged architecture, execution pipeline, NUMA model, PCI/GPU pipeline, and mode/capability structure.
  - Replaces several scattered documents:
    - `docs_pipeline.md`
    - `PIPELINE_OVERVIEW.md`
    - `docs_architecture.md`
    - `MODE_CAPABILITY_ARCHITECTURE.md`
    - `C2_NUMA_DERIVATION.md`

### Changed
- Minor cpuset documentation fixes for clippy compliance.
- Topology/scanning and PCI policy paths clarified in state machine docs.

### Removed
- No code removed, but several old documents are now superseded by the unified doc.

---

## v0.3.2 — PCI Inventory & Policy Foundations
- Full PCI inventory rebuilt from sysfs with robust error-handling.
- PCI policy preflight wired into VM state machine.
- Single-GPU safety enforcement implemented.

## v0.3.1 — Initial QEMU Launch Pipeline
- VM state machine implemented.
- cpuset creation + teardown.
- IRQ/MSI detection and pinning.

## v0.3.0 — Initial Project Resurrection & Refactor into Rust
- Full crate reorganization.
- Core modules established.
