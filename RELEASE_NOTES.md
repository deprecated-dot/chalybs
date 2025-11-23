# Chalybs v0.4.0 Release Notes

> **Status:** In development / working tree  
> **Focus:** Device isolation policy (Phase 8) and architecture alignment  
> **Note:** `IsolationLevel` and `default_level` are present in configuration
> examples as **reserved / future fields**, but are **not yet implemented** in
> the v0.4.0 codebase. They will be added early in the next development cycle.

This release consolidates PCI/VFIO phases and introduces **Phase 8: Device
Isolation Policy**, while keeping existing configurations backward compatible by
default.

---

## 1. Highlights

### 1.1 Phase 8: Device Isolation Policy

Chalybs can now evaluate and (optionally) enforce device isolation constraints
before performing any VFIO sysfs operations.

Key features:

- Per-VM `isolation` block with:
  - `mode = "disabled" | "audit" | "enforce"`
  - `default_level = "dedicated" | "shared_with_host" | "forbidden"`  
    **(reserved field: not active in this release)**
  - Boolean toggles:
    - `require_iommu_exclusive`
    - `require_multifunction_consistency`
    - `forbid_host_critical_in_group`
- Detailed findings model:
  - `Info`, `Warning`, `Violation` severities
  - Codes for each violation type
  - BDF and IOMMU group tagging in logs
- Modes:
  - **disabled** — no behavior change vs prior releases.
  - **audit** — log findings, never block startup.
  - **enforce** — block VM startup when violations exist.

### 1.2 Architecture & Docs Aligned With Reality

The internal PCI/VFIO pipeline is now fully documented as **Phases 1–8**:

1. Inventory  
2. GPU driver classification  
3. Unbind safety simulation  
4. VFIO sysfs helpers  
5. VFIO action plan  
6. VFIO binding verification  
7. Deterministic VFIO restore  
8. Device isolation policy  

The regenerated `CHALYBS_EXECUTION_AND_ARCHITECTURE.md` is now the canonical
reference for:

- State machine behavior
- NUMA-aware CPU/IRQ placement (C2)
- PCI/GPU/VFIO phases
- Isolation semantics

---

## 2. Configuration Changes

### 2.1 New `isolation` Block

Each VM may now define:

```toml
[vm.win11-gpu.isolation]
mode = "audit"                    # "disabled" | "audit" | "enforce"
default_level = "dedicated"       # reserved (not implemented in v0.4.0)

require_iommu_exclusive = true
require_multifunction_consistency = true
forbid_host_critical_in_group = true
```

Defaults if the block is omitted:

- `mode = "disabled"`
- `default_level = "dedicated"` (reserved + unused)
- All booleans default to `true`.

Result: existing configurations behave exactly as they did in v0.3.x.

### 2.2 Updated Example Configuration

`chalybs.example.toml` has been updated to:

- Use the `vm.<name>.cpu` / `vm.<name>.qemu` / `vm.<name>.devices` layout
  that matches `VmConfig`.
- Show explicit `gpu` policy:

```toml
[vm.win11-gpu.gpu]
allow_single_gpu = false
force_use_igpu = false
```

- Show explicit `isolation` policy (see above).
- Retain logging configuration (`[logging]`).

---

## 3. Behavior & Compatibility

### 3.1 Backward Compatibility

- If `isolation` is omitted or `mode = "disabled"`:
  - Chalybs does not evaluate or enforce isolation rules.
  - Behavior matches previous releases.

### 3.2 Enforced Safety (Opt-In)

- If `mode = "audit"`:
  - All isolation checks run.
  - Findings appear in logs only.
  - VFIO staging and VM startup continue.

- If `mode = "enforce"`:
  - Isolation is evaluated *before* VFIO plan/execute:
    - IOMMU group exclusivity
    - Multi-function ownership consistency
    - Host-critical GPU sharing
  - Any `Violation`:
    - Aborts startup with a clear `ChalybsError::Vfio` message.
    - No VFIO sysfs writes have occurred yet.

---

## 4. Operational Notes

- **Suggested rollout pattern:**
  1. Start with `mode = "audit"` on a single VM.
  2. Inspect logs for violations and adjust hardware layout / config as needed.
  3. Once green, move that VM to `mode = "enforce"`.
  4. Repeat per-VM.

- **Logging:**
  - Isolation findings are emitted with fields:
    - `vm`, `code`, `bdf`, `iommu_group`, `msg`.

---

## 5. Relationship to v0.3.x

- v0.3.4 introduced Phase 7 (deterministic VFIO restore).
- v0.3.5 exists as a repository release but was not fully documented here.
- v0.4.0 consolidates:
  - The full Phase 1–7 pipeline.
  - Newly implemented Phase 8 isolation policy.
  - Refreshed documentation and example configuration.

---

## 6. Looking Ahead

Planned areas for the next minor releases:

- **Implementation of `IsolationLevel` and `default_level`**  
  (currently reserved / no-op in v0.4.0)
- Multi-GPU policy (iGPU/dGPU arbitration, per-GPU overrides).
- More nuanced isolation levels (per-device, per-class).
- NUMA/IRQ advisors and “what-if” analysis.
- Expanded test coverage for isolation and VFIO stages (unit + integration).
