# Chalybs Architecture

Chalybs is a deterministic VFIO/KVM orchestration system designed to eliminate nondeterminism in:
- vCPU assignment
- IRQ steering
- NUMA placement
- Device bring‑up
- Launch sequencing

## Goals
1. **Determinism first.**
2. **Full NUMA alignment.**
3. **Portable structure** across all Linux/QEMU combinations.
4. **Strict validation** at every state boundary.

## Core Components
- `VmRuntime` – authoritative runtime object.
- `VmStateMachine` – strict state transition engine.
- `cpuset` – NUMA-aware host/vm cpuset derivation.
- `affinity` – deterministic vCPU discovery + pinning.
- `irq` – device IRQ discovery + steering.
- `qemu` – QEMU lifecycle (launch, shutdown, state reflection).
- `peripherals` – device hooks and orchestration.

## Why C2 (NUMA-host derivation)?
C2 ensures:
- maximum portability,
- no assumptions about CPU topology,
- no manual host CPU declarations required,
- correct behavior on both multi-node HEDT hardware and single-NUMA consumer systems.
