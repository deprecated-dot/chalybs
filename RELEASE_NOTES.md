# Chalybs v0.4.1 Release Notes

> **Status:** Released  
> **Primary Focus:** Isolation Level enforcement (Phase 9), stabilization of PciDeviceConfig, and alignment of VFIO pipeline.

---

## 1. Highlights

### 1.1 Phase 9: Isolation Level Enforcement (New)

`IsolationLevel` is now active:

- `dedicated` — device must be isolated from host-owned peers.
- `shared_with_host` — sharing acceptable, warnings possible.
- `forbidden` — passthrough prohibited.

Levels can be applied:

- Globally per-VM (`default_level`)
- Per-device via `PciDeviceConfig.level`

Phase 9 runs after Phase 8 (mode-based policy) and before any VFIO sysfs writes.

### 1.2 Structural Fixes

- Corrected struct-shape drift that caused missing-field test failures.
- Synchronized `PciDeviceConfig` across:
  - planner
  - verifier
  - isolation (policy + level)
  - configuration ingestion

### 1.3 Full Green State

- `cargo build` — clean  
- `cargo clippy` — clean  
- `cargo test` — **100% pass**  

This is the most stable release of the 0.4.x line.

---

## 2. Configuration Changes

### Active fields:

```toml
[vm.<name>.isolation]
mode = "audit"
default_level = "dedicated"

require_iommu_exclusive = true
require_multifunction_consistency = true
forbid_host_critical_in_group = true
```

Per-device override:

```toml
[[vm.<name>.devices.gpu]]
pci_address = "0000:03:00.0"
required = true
level = "shared_with_host"
```

Defaults remain backward-compatible.

---

## 3. Behavior & Compatibility

- No breaking changes.
- v0.4.1 preserves all v0.4.0 semantics unless `level` is explicitly used.
- Enforcement performed early: no sysfs writes happen if Phase 9 fails.

---

## 4. Looking Ahead

- Expansion of IsolationLevel semantics (NIC/storage classification).
- Multi-GPU arbitration.
- Advisor modes for NUMA, IRQ, and PCI placement.
- More granular device-level safety options.
