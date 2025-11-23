# Chalybs ROADMAP

> **Current Document Baseline:** v0.4.0  
> For actual implementation status, prefer the repository and CHANGELOG.

---

## 1. Current Baseline

### v0.4.0 – PCI Phases 1–8 Online

- Deterministic PCI/VFIO lifecycle:
  - Inventory → classify → simulate → plan → execute → verify → restore → isolate
- NUMA-aware CPU/IRQ placement (C2 policy).
- Device isolation policy (Phase 8) with:
  - `disabled` / `audit` / `enforce` modes.
- Regenerated architecture and config documentation.

---

## 2. Near-Term (v0.4.x – v0.5.x)

These are “must have, timing flexible” goals.

### 2.1 PCI & Isolation Enhancements

- **Per-device isolation overrides**
  - Allow selected devices to be:
    - Stricter than default VM policy.
    - Looser (e.g. shared with host) when operator explicitly opts in.
- **Isolation-level-aware logging**
  - Make use of `IsolationLevel` (`dedicated` / `shared_with_host` / `forbidden`)
    for better reporting and future enforcement.
- **Extended host-critical classification**
  - Treat certain NICs / storage controllers as “host-critical” by policy.

### 2.2 Multi-GPU Policy & Arbitration

- **iGPU/dGPU Arbitration**
  - Policy surface to decide which GPU is the “host anchor” vs “VM candidate”.
  - Better modeling of:
    - Display-attached GPUs
    - Headless compute GPUs

- **Multi-GPU Passthrough Policy**
  - Configurable rules for multiple discrete GPUs.
  - Per-GPU “opt out” or override of isolation constraints where safe.

### 2.3 VFIO / PCI QoL

- More descriptive dry-run output:
  - “Plan only” CLI mode for VFIO staging.
- Better error surfacing for unusual IOMMU group layouts.
- Metrics-friendly logging, suitable for scraping / dashboards.

---

## 3. Medium-Term (v0.5.x – v0.7.x)

### 3.1 NUMA & IRQ Advisor

- NUMA layout inspector:
  - Report how guest vCPUs map onto host nodes.
  - Suggest more optimal arrangements based on hardware topology.

- IRQ advisor:
  - Summarize MSI/MSI-X usage.
  - Offer hints where affinity is suboptimal.

### 3.2 Persistence & Policy

- Optional persistence layer for:
  - VFIO bindings across reboots (opt-in, conservative).
  - Stable identification of devices beyond BDF (e.g. via PCI IDs + slot hints).

- Higher-level “mode” concepts:
  - `performance` vs `safety-first` presets.
  - Bundled sets of policies for operators who don’t want to tune every knob.

---

## 4. Long-Term (v0.7.x – v1.0)

### 4.1 Full Deterministic VM Lifecycle

- Strong invariants on:
  - VM bring-up / tear-down.
  - VFIO transitions (forward + restore).
  - NUMA and IRQ placement.
- “Reproducibility story”:
  - Given the same host state and config, Chalybs behavior is identical.

### 4.2 Hardened Mode

- Tightened syscall and behavior envelopes:
  - Reduce nondeterminism where possible.
  - Make unsafe configurations obviously noisy in logs.
- Defense-in-depth for:
  - Sysfs interactions.
  - Failure modes during VFIO staging.

### 4.3 Daemon (`chalybsd`) & Control Plane

- Long-lived daemon managing:
  - Multiple VMs.
  - Centralized state and telemetry.
- API surface (local IPC / HTTP) for:
  - Tooling.
  - UI layers.
  - Automated test harnesses / CI.

---

## 5. Testing & Tooling (Cross-Cutting)

Regardless of exact version number, the following are treated as “mandatory,
timing flexible” investments:

- **Unit tests**
  - Isolation evaluation across synthetic inventories and configs.
  - Planner behavior under edge cases (weird IOMMU groups, missing devices).
- **Integration tests**
  - VFIO plan/execute/restore against controlled test hosts where possible.
- **Developer tooling**
  - Helpers for generating sample inventories.
  - CLI commands to introspect and debug VFIO state.

The intent is that every new phase or policy shift arrives with a matching
testing story, not as an afterthought.
