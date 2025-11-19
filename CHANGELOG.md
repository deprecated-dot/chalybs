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
