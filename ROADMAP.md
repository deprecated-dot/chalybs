# Chalybs ROADMAP

> **Baseline:** v0.4.1  
> For implemented details, see `CHANGELOG` and architecture docs.

---

## 1. Current Baseline (v0.4.1)

- PCI/VFIO Phases 1–9 complete.
- IsolationMode (Phase 8) + IsolationLevel (Phase 9) both active.
- Deterministic CPU/IRQ placement (C2 policy).
- Complete internal synchronization of VFIO plan → execute → verify → restore → isolate.
- Documentation refreshed end-to-end.

---

## 2. Near-Term (v0.4.x – v0.5.x)

### 2.1 Isolation-Level Expansion
- Extend `IsolationLevel` beyond GPUs:
  - NICs
  - NVMe
  - Host-critical controllers
- Per-device overrides with richer semantics.

### 2.2 Multi-GPU Arbitration
- iGPU/dGPU host anchor selection.
- Policy-surface for “GPU priorities”.
- Validation of multi-GPU IOMMU group layouts.

### 2.3 VFIO Quality-of-Life
- Dry-run mode for VFIO.
- Better synthetic inventories and debugging commands.
- Improved error surfacing for exotic IOMMU group layouts.

---

## 3. Medium-Term (v0.5.x – v0.7.x)

### NUMA & IRQ Advisor
- Automated inspection and recommendations.
- Heatmap-style reporting for vCPU placement.

### Persistence Layer (Optional)
- Stable device identity across reboots.
- Optional persistence of VFIO configuration.

---

## 4. Long-Term (v0.7.x – v1.0)

### Deterministic VM Lifecycle
- Stronger invariants across Phase 1–9.
- Deterministic startup/teardown across identical host states.

### Hardened Mode
- Reduced nondeterminism envelope.
- Stricter syscall fences around VFIO operations.

### Daemon (chalybsd)
- Persistent control plane.
- IPC/HTTP API.
- Telemetry, logging aggregation, event bus.

---

## 5. Testing & Tooling (Cross-Cutting)

- Unit tests for policy + level interactions.
- Integration tests for VFIO plan/execute/verify/restore.
- Tools for synthetic inventories and PCI topology simulation.
