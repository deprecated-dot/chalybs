
# Chalybs ROADMAP

> **Baseline:** v1.1.0  
> For implemented details, see `CHANGELOG` and architecture docs.

---

## 1.1 TUI Baseline (v0.5.0)

- Fully operational text UI with modal overlays
- Event stream with scrollback + locking
- Shell panel with history
- Brand-consistent theming engine
- Stable UI architecture (ui.rs, app.rs, theme.rs, logo.rs)
- Mock backend integration online

This forms the foundation for all future `chalybsd` interaction.

## 1.2 Visual Effects & PNG/Halo (v1.0.0 – v1.1.0)

- v1.0.0:
  - Deterministic TUI visual effects engine (pulse, scanlines, matrix, border
    noise, badges, logo_reactive, load_index).
  - Persistent effects configuration via `tui.conf`.
  - Width-aware VM list layouts and load sparklines.
- v1.1.0:
  - Experimental PNG logo renderer for capable terminals.
  - Set C halo profiles (`c3`, `c3narrow`, `c3wide`, `c3extrawide`) integrated
    into the TUI via `effects halo`.
  - Copilot contract + halo bug reference docs added as first-class project
    artifacts.

The long-term goal is a **fully brand-consistent** TUI where ASCII and PNG
paths feel like two skins over the same architectural core.

---

## 2. Current Core Baseline (v0.4.1)

- PCI/VFIO Phases 1–9 complete.
- IsolationMode (Phase 8) + IsolationLevel (Phase 9) both active.
- Deterministic CPU/IRQ placement (C2 policy).
- Complete internal synchronization of VFIO plan → execute → verify → restore → isolate.
- Documentation refreshed end-to-end.

The daemon and core remain the reference for deterministic virtualization;
the TUI rides on top as a strictly cosmetic/control surface.

---

## 3. Near-Term (v0.4.x – v1.2.x)

### 3.1 Isolation-Level Expansion

- Extend `IsolationLevel` beyond GPUs:
  - NICs
  - NVMe
  - Host-critical controllers
- Per-device overrides with richer semantics.
- Additional tests and docs for non-GPU passthrough devices.

### 3.2 Multi-GPU Arbitration

- iGPU/dGPU host anchor selection.
- Policy surface for “GPU priorities”.
- Validation of multi-GPU IOMMU group layouts.

### 3.3 VFIO Quality-of-Life

- Dry-run mode for VFIO.
- Better synthetic inventories and debugging commands.
- Improved error surfacing for exotic IOMMU group layouts.

### 3.4 TUI Expansion (v1.1.x – v1.3.x)

- Live VM control actions from TUI:
  - Power on/off
  - Restart
  - Rebind devices
  - Inspect VFIO plan / inventory
- “Event detail” modal (stacked modals).
- Sidebar expander for CPU/IRQ heatmaps.
- Better terminal-agnostic layout (80x24 compatibility mode).
- Optional kitty-graphics renderer for PCI diagrams and heatmaps.
- Theme customizer + accessibility mode (high contrast).

### 3.5 PNG Logo & Halo Rework (Post v1.1.0)

- Treat halo as a **tracked, explicit work item**:
  - Align halo rect strictly with PNG logo rect (origin + height).
  - Eliminate vertical cropping of wings.
  - Center dual-wing profiles around the logo centerline with a clear gap.
- Re-tune aesthetics toward a **Gibson-ish console glow**:
  - Subtle, low-amplitude breathing tied to `logo_breath_factor`.
  - Palette-respecting halo (teal/pink/purple with careful blending).
- Add debug/fixture mode:
  - Optionally draw halo rect outline in a single color to verify geometry.
  - Capture reference screenshots for all halo profiles.
- Only once the above is stable:
  - Consider enabling a minimal halo as a default profile.

See `HALO_RENDERING_BUG_REFERENCE.md` for the detailed bug capsule and
acceptance criteria for “fixed”.

---

## 4. Medium-Term (v0.5.x – v0.7.x)

### NUMA & IRQ Advisor

- Automated inspection and recommendations.
- Heatmap-style reporting for vCPU placement.
- Integration with TUI visualization panels.

### Persistence Layer (Optional)

- Stable device identity across reboots.
- Optional persistence of VFIO configuration.
- Configuration diffing and rollback tooling.

---

## 5. Long-Term (v0.7.x – v1.0)

### Deterministic VM Lifecycle

- Stronger invariants across Phase 1–9.
- Deterministic startup/teardown across identical host states.
- Better replayability for regression testing.

### Hardened Mode

- Reduced nondeterminism envelope.
- Stricter syscall fences around VFIO operations.
- Optional “paranoid mode” for high-assurance deployments.

### Daemon (chalybsd)

- Persistent control plane.
- IPC/HTTP API.
- Telemetry, logging aggregation, event bus.
- TUI and CLI clients are first-class consumers, not special snowflakes.

---

## 6. Testing & Tooling (Cross-Cutting)

- Unit tests for policy + level interactions.
- Integration tests for VFIO plan/execute/verify/restore.
- Tools for synthetic PCI inventories and topology simulation.
- Golden fixtures for TUI visuals (including halo profiles once fixed).

The roadmap is a living document; each tagged release must update this file to
reflect what actually shipped and what moved to the next horizon.

## Phase 10A: Synthetic Removal — Completed
- All synthetic VM and event paths removed.
- Daemon now reflects real config + PCI + state machines.

## Phase 11: Real Daemon/Core Integration — Completed
- Real VmStateMachine tied into daemon tick.
- IPC snapshot now reflects live VM states and events.

---

## Added for v1.2.0 — NUMA/RTC/QEMU Determinism Milestone

### ✔ Completed in v1.2.0
- Introduced TOML-configurable QEMU RTC policy
- Deterministic NUMA node + hugepage allocation pipeline
- Config.rs fully reconstructed (no truncation, Deserialize-correct)
- QEMU argument layers finalized (pre/core/mid/post)
- NUMA correctness hooks for future hugepage allocator + LG SHM

### 🚧 In Progress (v1.3.0 target)
- Deterministic VmStateMachine (step + step_shutdown)
- Hugepage allocator (deterministic node placement)
- Looking Glass SHM NUMA placement
- IRQ locality enforcement
- TUI numactl/pinning inspector

### v1.2.1 checkpoint (2025-12-02)

- **Segmented VmStateMachine** is now in-tree (`step`, `run_until_steady`, `step_shutdown`, `run_shutdown`), covering validation, hugepages, PCI/VFIO staging, CPU reservation, QEMU launch, IRQ worker spawn, and peripheral hooks.
- **VM-scoped lifecycle events** (`CoreEvent*` on `VmRuntime`) are plumbed through to the daemon/TUI for the events rail and F2 VM detail modal.
- **IRQ locality workstream** has its first concrete piece: an asynchronous MSI/MSI-X worker with NUMA-aware CPU selection and config-only aux-GPU detection. Further refinements remain under the existing IRQ locality + TUI inspector workstreams.


v1.2.2 checkpoint (2025-12-03)

- **VFIO dedicated passthrough restore semantics** is now wired through the VFIO staging and `RestorePci` phases. Devices that are already bound to `vfio-pci` at staging time are treated as dedicated passthrough and are excluded from driver transition bookkeeping; shutdown leaves them alone instead of trying to guess a host driver.
- **RestorePci summary reporting** now produces explicit per-VM summaries in both dry-run and live modes, including the “no recorded transitions” case, so you can see at a glance whether PCI restore actually touched anything for a VM.
- **Host-critical GPU host-only warnings** extend the VFIO isolation checker: host-only IOMMU groups that contain GPUs considered host-critical are now logged as warnings, keeping host GPU safety visible without blocking VM startup when no passthrough devices live in that group.

