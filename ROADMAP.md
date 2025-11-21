# Chalybs ROADMAP

## Current Version: v0.3.4 — PCI Phase 7 Complete

---

## Near-Term (v0.3.5 – v0.3.7)

### PCI Phase 8 — Automatic iGPU/dGPU arbitration
- Intelligent selection of passthrough GPU.
- Early determination of GPU handoff safety.

### PCI Phase 9 — Multi-GPU passthrough policy
- Configurable policy for multiple discrete GPUs.
- Per-GPU safety overrides.

### QEMU Pipeline Expansion
- Scream audio integration
- Looking Glass refinements

---

## Mid-Term (v0.4.x)

### NUMA/IRQ Automatic Layout Advisor
- Automatically derive optimal host NUMA node.
- Predictable IRQ balancing.
- Pre-flight NUMA mismatch warnings.

### VFIO Persistence Across Reboots
- Optional stable passthrough persistence layer.
- Probe-persistence to avoid misbinding on boot.

---

## Long-Term (v1.0)

### Full Deterministic VM Lifecycle
- End-to-end reproducibility.
- Strict invariants and operator overrides.

### Hardened Mode
- forbid nondeterministic syscalls
- integrity enforcement for kernel-facing operations

