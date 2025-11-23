# Chalybs CHANGELOG

> This changelog documents user-visible changes.  
> For deeper architectural details, see `CHALYBS_EXECUTION_AND_ARCHITECTURE.md`.

---

## v0.4.1 — Isolation Phase-9 (Level Enforcement) & Structural Cleanup

**Status:** Released  
**Scope:** Activation of isolation-level semantics, cleanup of config drift, full VFIO/PCI synchronization.

### Added

- **Phase 9: Isolation Level Enforcement**
  - `IsolationLevel` (`dedicated`, `shared_with_host`, `forbidden`) now active.
  - VM-level `default_level` enforced.
  - Per-device `PciDeviceConfig.level` supported and validated.
  - Enforcement occurs before any VFIO sysfs writes.

- **Config Shape Stabilization**
  - `PciDeviceConfig` unified across modules.  
    (Fixes the missing-`level` drift that caused E0063 test failures.)

### Changed

- **Isolation Pipeline**
  - Phase 8 (mode-based policy) and Phase 9 (level-based enforcement) now run sequentially.
  - VFIO planner & verifier updated to understand level semantics.

- **Documentation**
  - Architecture doc fully updated to v0.4.1.
  - Release notes reflect Phase 9.
  - Roadmap baseline changed to reference Phase 9 as implemented.

### Fixed

- E0560/E0063 struct drift errors in:
  - `vfio/plan.rs`
  - `vfio/verify.rs`
  - `vfio/isolation.rs` test blocks

### Compatibility Notes

- Existing configurations remain valid.
- Devices with no `level` inherit `default_level`.
- Default behavior matches v0.4.0 unless levels are explicitly configured.


## v0.4.0 — PCI Phase 8 & Isolation Policy Integration

**Status:** Released  
**Scope:** Completion of the Phase 8 isolation policy gate, cleanup of warning
conditions, and hardening of the isolation subsystem.

### Added

- **Phase 8: Device Isolation Policy**
  - New `IsolationMode` (Disabled / Audit / Enforce).
    - `disabled` — skip isolation checks (default).
    - `audit` — log findings but never block.
    - `enforce` — treat violations as hard errors *before* VFIO staging.
  - New `IsolationPolicyConfig` attached to `VmConfig`.
    - Includes:
      - `mode`
      - `require_iommu_exclusive`
      - `require_multifunction_consistency`
      - `forbid_host_critical_in_group`
  - New `IsolationSeverity` and `IsolationFinding` types.
  - New `IsolationReport` with structured evaluation results.
  - Core evaluation entrypoint:
    - `vfio::isolation::evaluate_isolation_for_vm(vm_name, cfg, inv)`
      - Pure read-only pass across PCI inventory + VM config.
      - Emits structured diagnostics.

- **Isolation Finding Categories**
  - IOMMU exclusivity violations.
  - Mixed multifunction ownership under same domain:bus:slot.
  - Host-critical GPU sharing with passthrough devices.
  - Host-only critical GPU groups (warning).

- **Test Suite Expansion**
  - Added tests for:
    - Cross-slot IOMMU group sharing.
    - Multiple GPUs in same group (host + passthrough).
    - Host-only GPU group warnings.
    - Enforce vs Audit vs Disabled mode behavior.
    - Multifunction consistency corner cases.

### Changed

- **PreparePci State Machine**
  - Isolation evaluation now runs before VFIO plan execution when
    `mode != disabled`.
  - In `enforce` mode, Phase 8 can abort staging prior to any sysfs writes.

- **Code Quality & Warnings**
  - Removed all Clippy warnings:
    - Replaced `.get(0)` with `.first()`.
    - Fixed doc-comment indentation issues.
  - Zero warnings under stable Rust 1.70+.

- **Documentation**
  - Regenerated documentation for:
    - Isolation policy behavior.
    - Host-critical GPU rules.
    - Examples showing audit/enforce modes.
    - Updated architecture diagrams and PCI phase sequencing.

### Removed

- No functional behavior removed.
- Deprecated comments referring to pre-Phase-8 behavior were updated.

### Compatibility Notes

- Existing configs missing `[vm.<name>.isolation]`:
  - Default to `mode = "disabled"` preserving identical historical behavior.
- Isolation policy is per-VM, allowing safe incremental rollout.

---

## v0.3.5 — Repository Release (Out-of-Band to Docs)

**Status:** Already tagged in Gogs  
**Notes:** This release exists in version control but was not fully documented in
earlier Markdown. Refer to repository history for exact changes.

---

## v0.3.4 — PCI Phase 7: Deterministic VFIO Restore & Inventory Rescan

**Release Date:** TBD (previous series)

### Added

- **PCI Phase 7**: Deterministic VFIO shutdown restore pipeline:
  - New `restore_pci_devices_for_vm()` implementation.
  - Rebinds each passthrough device to its original driver (if any).
  - Fully idempotent + best-effort semantics.
- Optional **PCI bus rescan** after restore.
- Documentation updates for Phase 7 behavior.

### Changed

- Unified VFIO action pipeline (plan → execute → verify → restore).
- Improved inventory error handling and tracing consistency.

### Removed

- No unsafe or obsolete code removed.

(Older entries remain below as needed.)
