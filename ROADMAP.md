
# Chalybs Roadmap

Chalybs progresses through deterministic, invariant-driven phases.  
Finished phases **never regress** — semantics lock in permanently.

---

## ✔ Completed Phases (1–13)

### ✔ Phase 1 — Core Init & Configuration
- Initial TOML config structure
- Deterministic parse/validation layer

### ✔ Phase 2 — CPU Detection + CPUID Identity
- Host CPUID enumeration
- Vendor/model/topology signature derivation
- Used to drive downstream invariants

### ✔ Phase 3 — Host CPU & NUMA Topology Model
- Deterministic CPU → NUMA mapping
- Validation of host layout
- Foundational model for CPU planning

### ✔ Phase 4 — QEMU Bootstrap
- Binary + firmware path validation
- QMP socket plan and structure

### ✔ Phase 5 — PCI / VFIO Staging
- Deterministic passthrough device ordering
- Root-port allocator + override semantics
- Capture of original drivers for clean teardown

### ✔ Phase 6 — Isolation Layer
- Deterministic cpuset partitioning
- VM vs host CPU segregation
- Isolation enforcement guarantees

### ✔ Phase 7 — QEMU Launch
- Declarative QEMU command builder (v1)
- Runtime argument assembly
- SMBIOS injection logic

### ✔ Phase 8 — IRQ Discovery & Pinning
- Deterministic MSI/MSI-X enumeration
- IRQ → NUMA affinity algorithm
- Latch-style completion flag

### ✔ Phase 9 — Peripheral Hooks (v1)
- Tasmota hook system
- Initial peripheral lifecycle event model

### ✔ Phase 10 — Hugepages Provisioning
- Deterministic NUMA node selection
- Config/topology override handling
- Explicit hugepage provisioning and tracking

### ✔ Phase 11 — Event Model / IPC
- CoreEventKind / CoreEvent
- Deterministic event logging per VM
- Foundation for daemon/TUI projections

### ✔ Phase 12 — State-Machine Coherence Pass
- Segmented step() and shutdown() phases
- Elimination of heuristics
- Deterministic error-path hardening

### ✔ Phase 13 — Deterministic Peripheral Overhaul
- **Native I²C DDC/CI engine**
  - Input switching
  - Optional verification pass
  - Conditional verification logic
- **Looking-Glass SHM subsystem**
  - Deterministic shm creation
  - NUMA-aware creation and sizing
  - Crash-safe removal on VM stop
  - Deterministic desktop-user discovery
- **SPICE subsystem**
  - virtio-serial bus
  - vdagent channel
  - `-spice` server wiring into QEMU builder
- QEMU builder consistency and structural cleanup

---

## ⧗ Upcoming Phases (14+)

### Phase 14 — Peripheral Maturity
- Complete LG peripheral hook behavior
- Export LG/DDC state through TUI
- SPICE monitoring hooks
- Additional DDC/CI capability detection

### Phase 15 — NUMA Allocation v2
- Predictive NUMA planner
- Multi-node spanning policy
- TUI/daemon NUMA visualization

### Phase 16 — IRQ Domain Planner
- IRQ domain abstraction layer
- Preferential routing guidance
- Pre-VFIO planning pass

### Phase 17 — PCI Topology Awareness
- PCIe switch/bridge graph modeling
- Optimal GPU/NVMe placement hints
- Deterministic lane grouping behavior

### Phase 18 — CPU Plan v2
- Unified model integrating:
  - Host topology
  - VM CPU layout
  - Isolation & routing invariants
- Stronger validation and feedback

### Phase 19 — TUI Coherent Visual Field
- NUMA + cpuset live visualization
- QEMU argument and phase visualizer
- Peripheral pipeline representation
- Strictly state-driven, no heuristics

### Phase 20 — Public Hardening & Release
- Prepare GitHub/GitLab mirror
- Documentation stabilization
- Public build/release pipeline

---

## Future Considerations (Unassigned)
Not assigned to a numbered phase yet, but captured intentionally:

- Deterministic hotplug/hot-unplug (within architectural limits)
- Cross-VM orchestration
- VFIO topology safety analyzer
- Network orchestration model
- Multi-GPU mirror/clone rendering strategies
- Optional remote TUI mode

---

Chalybs continues forward with strict determinism, no silent fallbacks, no heuristics, and explicit phase-bound invariants.
