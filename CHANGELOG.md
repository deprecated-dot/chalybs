# Chalybs CHANGELOG

## v0.3.4 — PCI Phase 7: Deterministic VFIO Restore & Inventory Rescan

**Release Date:** TBD

### Added
- **PCI Phase 7**: Deterministic VFIO shutdown restore pipeline:
  - New `restore_pci_devices_for_vm()` implementation.
  - Rebinds each passthrough device to its original driver (if any).
  - Fully idempotent + best‑effort semantics.
- Optional **PCI bus rescan** after restore.
- Full documentation updates for Phase 7 behavior.

### Changed
- Unified VFIO action pipeline (plan → execute → verify → restore).
- Improved inventory error-handling and tracing consistency.

### Removed
- No code removed.

(Older entries retained below.)

