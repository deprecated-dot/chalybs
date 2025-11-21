# Chalybs v0.3.4 Release Notes

This release completes **PCI Passthrough Phase 7**, bringing Chalybs to a full deterministic VFIO lifecycle: plan → execute → verify → restore.

## 🆕 Major Features

### 1. Deterministic VFIO Shutdown Restore
All PCI devices staged for passthrough now:
- Restore to their **original driver** on VM shutdown.
- Skip restore when originally unbound.
- Skip restore when originally vfio‑bound.

### 2. Optional PCI Rescan
When enabled, Chalybs can request a PCI bus rescan after restore.

### 3. Updated Unified Architecture Document
Phase 7 is now fully described in the authoritative `CHALYBS_EXECUTION_AND_ARCHITECTURE.md`.

## Compatibility
- Fully backward compatible with v0.3.3.
- No config changes required.

