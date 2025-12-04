
# Chalybs CHANGELOG

> This changelog documents user-visible changes.  
> For deeper architectural details, see `CHALYBS_EXECUTION_AND_ARCHITECTURE.md`.

---

## v1.2.2 – VFIO isolation polish, hugepages manager, Tasmota peripherals

**Status:** working-tree snapshot

### Added

- Stateful node-local hugepages manager that:
  - mounts a Chalybs-managed `hugetlbfs` at `/dev/hugepages` during `PrepareHugepages`,
  - raises `nr_hugepages` on the configured NUMA node just-in-time for the VM, and
  - wraps both provision and cleanup in an explicit pagecache-drop + memory-compaction request, with detailed before/after logging.
- Peripheral hooks for Tasmota smart power control, driven directly from the `PeripheralHooks` phase:
  - VM up → publish `POWER ON` to the configured MQTT topic,
  - VM down → publish `POWER OFF`,
  - with logs that show broker, topic, and payload so failures can be correlated with lifecycle events.

### Changed

- PCI / VFIO lifecycle:
  - Treat passthrough devices that are already bound to `vfio-pci` at staging time as **dedicated passthrough**: no restore transition is recorded, and the logs explicitly acknowledge the device as "already vfio-bound".
  - Extend isolation logging to distinguish between:
    - host-owned GPUs that remain bound to host drivers (`HostOwned`, detection-only in Phase 2, warnings only in Phase 8/9), and
    - IOMMU groups that are fully exclusive to the VM (`IOMMU_GROUP_EXCLUSIVE_PASSTHROUGH`).
  - Make the PCI restore path during shutdown a no-op when there are no recorded transitions, with a summary log covering total/restored/failed devices.
- Hugepages lifecycle:
  - Move from a mostly static `nr_hugepages` view to an explicit **stateful hugepages manager** that raises node-local pages for the VM, then returns both node-local and global counts to zero on shutdown and unmounts `/dev/hugepages`.
- IRQ worker:
  - Remove the old "wait timer" heuristic for IRQ discovery and rely on the worker thread launched at `DetectMsi` to deterministically discover and pin MSI/MSI-X lines.
  - Continue treating auxiliary GPU audio functions as IRQ-pinning no-ops (they still live on the VM cpuset by virtue of the device group, but we do not try to micro-optimise them).

## v1.2.1 – VM state machine & IRQ worker

**Status:** local-only / not yet packaged

**Added**

- Segmented `VmStateMachine` bring-up and shutdown pipeline:
  - `step` / `run_until_steady` for forward progress towards Steady.
  - `step_shutdown` / `run_shutdown` for deterministic teardown to Idle.
  - Explicit states for hugepage provisioning, PCI/VFIO staging, CPU reservation, QEMU launch, IRQ detection, and peripheral hooks.
- VM-scoped lifecycle event rail:
  - `CoreEventKind`, `CoreEvent`, and `VmRuntime::push_*` helpers.
  - Used to emit ordered lifecycle milestones (cpuset preflight, PCI staging, QEMU launch, IRQ worker spawn, shutdown cleanup, etc.).
- Asynchronous IRQ pinning worker in `core::irq::pin`:
  - `spawn_irq_pin_worker` and an internal `irq_worker` loop that polls for MSI/MSI-X IRQs while QEMU is alive.
  - NUMA-aware CPU selection shared with the synchronous `pin_irqs` path.
  - Config-only heuristic to treat GPU HDMI/audio “aux” functions as best-effort for missing IRQs.

**Changed**

- IRQ pinning semantics:
  - Synchronous `pin_irqs` keeps strict behavior for required devices (missing IRQs still fail bring-up).
  - Background worker pins IRQs without blocking bring-up and logs per-device completion plus a final summary line once all devices are handled.
- TUI integration:
  - VM list badges and the F2 VM detail modal now reflect CPU pinning, IRQ pinning, hugepage enablement, and isolation mode (`disabled`/`audit`/`enforce`) derived from runtime state.

## v1.1.0 – PNG Logo & Halo Pipeline (Experimental)

**Status:** local-only / not yet packaged

### Added

- Experimental **PNG logo renderer** and **Set C halo pipeline** in the TUI:
  - `tui/src/logo_png.rs` now handles Kitty/iTerm backends via `viuer` and
    renders a transparent PNG logo where supported.
  - `VisualEffects` gains a canonical `logo_halo: LogoHaloProfile` field with
    profiles: `off`, `c3`, `c3narrow`, `c3wide`, `c3extrawide`.
  - New `effects halo <off|c3|c3narrow|c3wide|c3extrawide>` shell command
    controls halo profile at runtime.
  - Set C halo masks (C3, C3Narrow, C3Wide, C3ExtraWide) defined as
    deterministic 7×7 intensity fields per side.

- **Logo breathing exports** for TUI coherence:
  - `logo_breath_factor(tick: u64) -> f32` – primary brightness factor for
    logo + sparkline coherence.
  - `logo_breath_coherence(tick: u64, salt: u64) -> f32` – secondary 0..1
    signal used by header and per-VM sparklines for a shared “heartbeat”.

- **Copilot contract & halo bug reference docs**:
  - `CHALYBS_COPILOT_CONTRACT.md` – formal rules for ChatGPT-as-copilot,
    including full-drop-in guarantees, immutability after tagging, and
    TUI/halo-specific constraints.
  - `HALO_RENDERING_BUG_REFERENCE.md` – a dedicated capsule explaining the
    misaligned halo issue, geometry expectations, and acceptance criteria for
    future fixes.

### Changed

- `tui/src/logo.rs`:
  - Now delegates the upper portion of the status-panel logo slot to
    `logo_png::draw_png_logo()` when an image backend and PNG are available.
  - Renders a compact breathing caption (“CHALYBS ⟐”) in reserved bottom rows
    when the PNG path is active; ASCII logo behavior is preserved as the
    fallback when PNG is unavailable.
  - Exposes the public breathing helpers (`logo_breath_factor`,
    `logo_breath_coherence`) for other TUI effects.

- `tui/src/app.rs`:
  - `VisualEffects` updated to include the canonical `logo_halo` profile,
    seeded from `TuiConfig` if present.
  - `effects halo ...` subcommand now routes through `LogoHaloProfile` and
    reports the active profile by name in `effects status`.

- `tui/src/ui.rs`:
  - `draw_status_panel` now uses `logo::draw_logo(...)` for hybrid ASCII/PNG
    rendering, keeping all logo logic centralized in `logo.rs`.

### Known Issues

- The Set C halo is still **visually incorrect**:
  - Wings can be vertically cropped at the bottom.
  - Horizontal placement is not yet consistently centered around the PNG; in
    some configurations the halo appears “too high” in the status panel.
  - Overall aesthetic is not yet aligned with the desired Gibson-ish,
    low-key console glow and can read as “neon club” in some profiles.

- As a result, the halo is considered **experimental** and should remain
  **off by default** (`halo = off`) for production use in v1.1.0.

For details, see `HALO_RENDERING_BUG_REFERENCE.md`.

### Notes

- Core daemon (chalybsd) execution model, PCI pipeline, and VM orchestration
  remain unchanged from v0.5.0 / v0.4.1.
- The TUI visual effects baseline from v1.0.0 is preserved; the PNG/halo layer
  is additive and cosmetic only.
- v1.1.0 is primarily a **TUI checkpoint** plus documentation and process
  hardening (copilot contract, bug capsule).

---

## v1.0.0 – TUI visual effects baseline

**Status:** local-only / not yet packaged

### Added
- First-class TUI visual effects engine with deterministic, aesthetic-only behavior.
- `VisualEffects` configuration struct with on-disk persistence via `tui.conf`.
- `effects` local shell command:
  - `effects status` to print current flag values.
  - `effects on` / `effects off` to toggle all effects at once.
  - `effects <pulse|scanlines|matrix|border|badges|logo|load> <on|off>` to control individual flags.
  - `effects save` to persist the current configuration.
- VM list layout refinements:
  - Width-aware layouts (full / medium / compact) with micro-badges for ISO/TAS/CPU/IRQ.
  - Per-VM synthetic load sparkline under the VM row when `load_index` is enabled.
  - Pulsing state glyphs for non-stopped VMs when `pulse` is enabled.
- Events panel enhancements:
  - Subtle scanline-style row banding driven by `tick_count`.
  - Matrix-style watermark prefix (drifting dots + occasional glitch glyphs) that never obscures real log text.
- Panel border EMI shimmer:
  - Per-panel salted pseudo-random shimmer with very low amplitude "hiss".
  - Rare, short-lived brightness bursts and dual-frequency wobble for a lab-bench RF noise feel.
- Header visual load indicator: optional single-row sparkline when `load_index` is enabled.

### Changed
- TUI shell command handling now supports local-only commands (`effects ...`) without forwarding them to the backend.
- Shell command submission path cleaned up to avoid borrow-checker hazards while keeping history and events in sync.
- Minor tweaks to `theme.rs` and `logo.rs` to keep all visual effects strictly cosmetic and opt-out via config.

### Notes
- Core daemon (chalybsd) execution model, PCI pipeline, and VM orchestration remain unchanged from v0.5.0.
- All visual effects are driven from deterministic inputs (tick counters, salts, and config) and do **not** affect control-plane behavior.
- This release is considered the first "1.x" visual baseline for the TUI; future changes should treat these behaviors as stable UX.


## v0.5.0 — First Functional TUI (Chalybs Terminal UI)

**Status:** Released  
**Scope:** Introduction of the complete TUI subsystem, including panels,
interaction model, modal system, brand-aligned color theme, and foundation for
daemon-driven live VM management.

### Added

- **Chalybs TUI (new subsystem)**
  - Three-panel layout:
    - VM Status panel (left)
    - Events stream with scroll lock (middle)
    - Chalybs shell with input + history (right)
  - Brand-derived theme system:
    - ACCENT_TEAL, ACCENT_PINK, ACCENT_PURPLE
    - SUCCESS/WARNING/ERROR semantic surfaces
    - Normal/Dim text layers
  - Initial **Chalybs logo** (ASCII rune + header)

- **Event Stream with Scroll Control**
  - PgUp/PgDn scrollback
  - `Ctrl-S` lock view (prevent auto-follow)
  - `Ctrl-Q` unlock and snap to latest
  - `LOCKED` state indicator

- **Shell Subsystem**
  - Input prompt with styled prefix
  - Command history
  - Integration with mock backend

- **VM Status Panel**
  - Color-coded state glyphs:
    - Running → teal
    - Starting/ShuttingDown → amber
    - Stopped → red
  - Highlighted selected VM
  - Upcoming: richer indicators (CPU pinning, IRQs, etc.)

- **Modal Overlay System ("Scrim D")**
  - F2 opens VM detail modal
  - Esc closes
  - Fully opaque modal with shaded border + filled background
  - Scrim layer prevents bleed-through on transparent terminals
  - Modal presents:
    - State
    - CPU pinned
    - IRQ pinned
    - Tasmota status
    - Isolation mode
    - Hugepages

- **Theme Extensions**
  - `modal_bg()` and `scrim_bg()` surfaces
  - Unified style entrypoints for blocks, headers, glyphs, events, and modal text

### Changed

- UI architecture restructured into:
  - `ui.rs` (layout & rendering)
  - `logo.rs` (logo rendering backends)
  - `theme.rs` (palette + style)
  - `app.rs` (pure state/logic, backend-agnostic)

- Moved modal toggling to **F2**  
  (`d` deprecated for future shell command surface)

- Overhauled event scrolling logic:
  - Window follows latest unless locked
  - Offset-safe bounds checking

### Fixed

- Bleed-through between events panel and modal overlay (transparent terminals)
- Drift between modal keybind docs and implementation
- Type inference issues in VM list rendering (`collect::<Vec<_>>()`)

### Compatibility Notes

- No CLI or config surfaces changed.
- TUI currently talks only to MockBackend; daemon integration will begin in v0.5.x.


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

## v1.1.1 — Phase 10A/11 Integration
- Removed synthetic daemon backend.
- Introduced real VmStateMachine-driven lifecycle in daemon.
- Added deterministic per-tick VM bring-up/shutdown.
- Integrated core events into daemon snapshots.

---

## v1.2.0 – 2025-12-01

### Added
- RTC configuration support in TOML
- Deterministic NUMA node resolution
- Full config.rs reconstruction
- QEMU argument layer architecture

### Changed
- Improved determinism in RAM placement

### Fixed
- Deserialize errors
- Removed stray semicolon

## [1.2.3] - 2025-12-03
### Fixed
- Tasmota MQTT publish correctness and ConnAck sequencing.
- Peripheral hook borrow discipline (&mut VmRuntime).
- Daemon state-machine truth reporting for CPU/IRQ/Tasmota.
- TUI badge mismatch between config and runtime state.
- Stale tasmota_configured removed.

### Changed
- All peripheral badges now fully runtime-driven.

