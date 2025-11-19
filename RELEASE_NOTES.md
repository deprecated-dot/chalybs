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

# Chalybs v0.3.1 — Mode/Capability Architecture + NUMA Derivation

This release builds on the v0.3.0 *Deterministic Baseline* by introducing the next major chunk of infrastructure necessary for full feature parity with the legacy bash suite.

v0.3.1 brings:

## 🚀 Major Features

### 1. Mode System
Chalybs now supports explicit runtime modes, with automatic fallback:
- **single_gpu**
- **dual_gpu_preferred**
- **dual_gpu_fallback**
- **dedicated_gpu**

Modes are resolved deterministically based on host capabilities and user config.

### 2. HostCapabilities Engine
Chalybs now auto-discovers:
- NUMA node count
- CPU→NUMA mapping
- GPU count & PCI topology
- VFIO availability
- Single-GPU vs Dual-GPU safety constraints

This dramatically reduces configuration requirements.

### 3. Deterministic NUMA Derivation (C2)
Host CPU selection now uses:
- explicit config (if provided)
- otherwise: derived host CPU = all CPUs not in vm_cpus

Works perfectly across:
- Threadripper 2990WX/3990X
- Ryzen/Intel monolithic dies
- Multi-socket systems

### 4. Improved vCPU & IRQ affinity
- QMP-first discovery
- Procfs fallback
- Deterministic pinning ordering
- Proper state-machine integration

## 📚 Documentation
Three new architecture documents shipped:
- MODE_CAPABILITY_ARCHITECTURE.md
- C2_NUMA_DERIVATION.md
- PIPELINE_OVERVIEW.md

## 🎯 Stability
v0.3.1 is the first release to include both deterministic affinity *and* deterministic mode/capability resolution.

