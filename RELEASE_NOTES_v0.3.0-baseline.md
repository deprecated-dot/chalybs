# Release Notes – v0.3.0 Deterministic Baseline

This release marks the first fully deterministic, NUMA‑aware baseline of **Chalybs**.

## Highlights
- **Deterministic thread discovery** via QMP with process‑level fallback.
- **NUMA‑aware cpuset derivation (C2)** for universal portability across:
  - HEDT (Threadripper, EPYC)
  - Workstation/server
  - Standard consumer systems
- **Hardened cpuset orchestration** with guaranteed mem-node alignment.
- **Predictable vCPU pinning** with validated layout matching and kernel sanity enforcement.
- **Graceful degradation** when GPUs / NVMe / NIC passthrough devices are not configured.

## Why this matters
Modern QEMU builds removed stable vCPU thread names. Chalybs now uses the *only* cross-version deterministic mechanism (QMP `query-cpus-fast`) plus strict fallback logic to ensure stable behavior on all kernels and all QEMU variants.

NUMA-aware steering guarantees that:
- vCPUs
- IRQs
- Assigned PCIe devices

all land on the same NUMA node, maximizing determinism and throughput.

This release becomes the foundation for:
- scheduled hooks,
- device orchestration,
- IO threading policies,
- future multi‑VM coexistence guarantees.
