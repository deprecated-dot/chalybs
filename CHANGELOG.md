# Changelog

## v0.3.0 – Deterministic Baseline (NUMA‑Aware C2)
### Added
- Fully deterministic vCPU discovery using QMP `query-cpus-fast` with `/proc/<pid>/task` fallback.
- NUMA‑aware cpuset derivation (C2 strategy): derive host CPUs from online CPUs minus VM CPUs unless explicitly configured.
- Strong cpuset creation + strict cpuset.mems validation.
- Robust QMP handshake with deterministic retries and full protocol sanity checks.
- vCPU pinning improvements with explicit EINVAL handling and kernel sanity enforcement.
- IRQ discovery and pinning framework with safe device‑empty fallbacks.
- End-to-end deterministic VM bring‑up pipeline with structured logging.

### Fixed
- Eliminated nondeterministic vCPU detection behavior on recent QEMU versions.
- Corrected EINVAL decode paths for nix 0.29 semantics.
- Fixed stale borrow conflicts in QMP reader handling.
- Removed legacy procfs-only detection path.

## [0.3.1] – 2025-11-19
### Added
- Introduced **Mode + Capability Architecture**, enabling deterministic behavior across varying host hardware.
- Added full `HostCapabilities` detection: GPU count, NUMA layout, VFIO status, PCI topology.
- Added new runtime Mode resolver supporting:
  - `single_gpu`
  - `dual_gpu_preferred`
  - `dual_gpu_fallback`
  - `dedicated_gpu`
- Added fully deterministic NUMA-aware host CPU derivation (C2 logic) with override support.

### Changed
- Updated vCPU/IRQ discovery pipeline to QMP-first with procfs fallback.
- Tightened cpuset logic and made all cpuset transitions atomic.
- Improved state engine logs (Init → Validate → ReserveCpus → Launch → DetectThreads → PinVcpus → DetectMsi → PinIrqs → PeripheralHooks → SteadyState).
- Cleaned up affinity code path and reduced nondeterministic cases.

### Fixed
- Eliminated borrow-checker errors from QMP handling.
- Fixed CPU-range parsing to properly bubble ChalybsError.
- Resolved struct/field mismatches generated during large refactors.

### Documentation
- Added `MODE_CAPABILITY_ARCHITECTURE.md`.
- Added `C2_NUMA_DERIVATION.md`.
- Added `PIPELINE_OVERVIEW.md`.
- Updated architecture diagrams and state-flow documentation.

