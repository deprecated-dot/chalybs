# Chalybs v0.4.0 — Full Static Analysis (Verbatim)

## 1. Workspace layout (v0.4.0)

At the top level you’ve got a Cargo workspace:

- **core/** → `chalybs-core`  
  Deterministic engine: config, model, state machine, PCI/VFIO, cpusets, IRQ, peripherals, logging, errors, util.
- **cli/** → `chalybs`  
  User-facing launcher CLI (`chalybs up/down/status/cpuset-status`), thin wrapper around `chalybs-core`.
- **daemon/** → `chalybsd`  
  Stub daemon: logging + startup, no IPC/event loop yet.
- Docs:  
  - `CHALYBS_EXECUTION_AND_ARCHITECTURE.md`  
  - `ROADMAP.md`, `RELEASE_NOTES.md`, `CHANGELOG.md`

No tests/ directory; tests are inline or absent; build/clippy/tests all green.

## 2. Core crate: top-level surface

`core/src/lib.rs` exposes:

- `errors` → `ChalybsError`, `Result<T>`
- `logging` → tracing init
- `config` → TOML-deserializable structures
- `model` → runtime structs (`CpuSet`, `VmCpuLayout`, `CgroupPaths`, `QemuState`, `VmRuntime`)
- `state` → `VmState` + `VmStateMachine`
- `cpuset` → NUMA-aware cpusets + cgroup creation
- `qemu` → preflight, launch, shutdown
- `affinity` → vCPU thread discovery & pinning (QMP + procfs)
- `irq` → MSI IRQ discovery & pinning
- `util` → helpers (`parse_cpu_list`)
- `pci` → sysfs PCI inventory + GPU classification
- `vfio` → plan/execute/verify + Phase 8 isolation
- `peripherals` → Tasmota, DDC, Looking Glass hooks

Everything flows through **VmRuntime** + **VmStateMachine**.

## 3. Config & model

### 3.1 Config surface

- `RootConfig { vm: HashMap<String, VmConfig>, logging }`
- `VmConfig` contains:
  - CPU layout strings
  - QEMU config (binary, args, ovmf paths, mem_mb, hugepages, num_vcpus)
  - PCI passthrough devices
  - **IsolationPolicyConfig**
  - Peripherals config

### 3.2 Isolation config (Phase 8)

- `IsolationMode`:
  - `Disabled`, `Audit`, `Enforce`
- `IsolationLevel`:
  - `Dedicated`, `SharedWithHost`, `Forbidden`
- `IsolationPolicyConfig`:
  - `mode`
  - `default_level`
  - strictness flags

IsolationLevel exists but is not behavior-driving yet.

### 3.3 Runtime model

`VmRuntime` contains:

- name  
- cfg (VmConfig)  
- cpus (VmCpuLayout)  
- cgroups  
- qemu state  
- pinned_threads / pinned_irqs  
- vfio_transitions (original drivers for restore)

---

## 4. Deterministic state machine

### 4.1 States

```
Init
Validate
PreparePci
ReserveCpus
LaunchQemu
DetectThreads
PinVcpus
DetectMsi
PinIrqs
PeripheralHooks
Steady
Shutdown
Cleanup
Idle
```

### 4.2 Flow (happy path)

1. **Init → Validate**  
   Validation.

2. **Validate → PreparePci**  
   - PCI inventory  
   - Isolation evaluation (Phase 8)  
   - Enforce → abort if violations

3. **PreparePci → ReserveCpus**  
   - VFIO plan  
   - VFIO sysfs unbind/bind  
   - Verify vfio-pci

4. **ReserveCpus → LaunchQemu**  
   - cpuset setup  
   - QEMU preflight  
   - QEMU launch w/ QMP socket

5. **LaunchQemu → DetectThreads**  
   - QMP handshake  
   - procfs discovery of vCPU threads

6. **DetectThreads → PinVcpus**  
   - sched_setaffinity

7. **PinVcpus → DetectMsi**  
   - Discover MSI IRQs

8. **DetectMsi → PinIrqs**  
   - NUMA-aware IRQ pinning  
   - smp_affinity_list writes

9. **PinIrqs → PeripheralHooks**  
   - Tasmota, DDC, Looking Glass

10. **PeripheralHooks → Steady**  
   - VM running

Shutdown path undoes all bindings, restores VFIO, cpusets, peripherals.

---

## 5. PCI inventory & isolation

### 5.1 PCI inventory

- Pure sysfs read  
- Build PciInventory  
- GPU classification  
- Track: vendor/device IDs, driver, IOMMU group

### 5.2 VFIO orchestration

- `plan.rs` deterministic plan  
- `execute.rs` sysfs writes  
- `verify.rs` confirm vfio-pci  
- `restore.rs` original drivers

### 5.3 Phase-8 isolation

- Evaluate IOMMU groups  
- Produce `IsolationFinding` entries  
- `Disabled` → ignore  
- `Audit` → log  
- `Enforce` → abort on violations  
- `IsolationLevel` present but passive (Phase 9 expands)

---

## 6. CPU layout, cpusets, IRQ pinning

### 6.1 CPU layout

- parse_cpu_list expands ranges  
- sorted Vec<u32>  
- fully explicit

### 6.2 Cgroups & cpusets

- Build host/vm cpusets  
- sysfs cpuset.cpus/mems  
- No heuristics

### 6.3 Affinity (core/src/affinity.rs)

QMP-based QEMU thread control using:

- `UnixStream` to connect to QMP socket  
- `QmpCommand` to send `qmp_capabilities`  
- JSON parsing via `serde_json::Value`

Uses **procfs** to:

- enumerate QEMU process threads  
- identify vCPU threads by name or PID mapping

`wait_for_qemu_threads(&VmRuntime)`:

- Polls until expected vCPU threads appear  
- Polling uses fixed, deterministic intervals  
- No randomized backoff  
- Polling is observation, not logic

`pin_vcpus(&VmRuntime)`:

- Uses `nix::sched::sched_setaffinity` with `NixCpuSet`  
- Pins each vCPU thread to VM CPU list

The sleep/Duration calls:

- are bounded  
- deterministic  
- not used for retry/backoff logic  
- solely for waiting for QEMU to present threads

### 6.4 IRQ pinning

- NUMA-aware if device NUMA node known  
- Intersect VM CPUs with node CPUs  
- If empty, error  
- Writes smp_affinity_list per IRQ

---

## 7. Peripherals subsystem

- Trait-based: `PeripheralHook`  
- Modules: Tasmota, DDC, Looking Glass  
- `apply_vm_up` / `apply_vm_down`  
- Called inside state machine

---

## 8. CLI and daemon

### 8.1 CLI

- clap  
- Global: config, vm, log-format, log-level  
- Commands: `Up`, `Down`, `Status`, `CpusetStatus`  
- Wrapper around core

### 8.2 Daemon

- Logging init  
- Stub only

---

## 9. Phase-9 hooks

- `IsolationLevel` + defaults exist  
- Not yet influencing severity or VFIO planning  
- Natural extension points:  
  - severity mapping  
  - device-level eligibility  
  - forbidden/shared enforcement  
  - plan pruning  
