# Chalybs ROADMAP

> Deterministic virtualization for serious systems.

This roadmap is *historical plus forward-looking*:
- **Past / completed work** is kept for context and marked clearly.
- **In‑progress** items capture the current development focus.
- **Planned / future** items sketch out where Chalybs is headed next.

Status legend:
- ✅ = Complete
- 🚧 = In progress / partially implemented
- 🔜 = Planned / near‑term
- 🧪 = Experimental / research
- 💤 = Deferred / “someday/maybe”

---

## 0. Foundations & Baseline

### 0.1 Core Principles (anchored, non‑negotiable)

✅ **Determinism over heuristics**
- No polling loops, timers, or “poke‑and‑pray” retry logic.
- Every external interaction is explicit, ordered, and bounded.
- Behavior must be reproducible given the same host + config.

✅ **Sysfs‑native behavior**
- Prefer talking directly to `/sys`, `/proc`, and cgroups over shelling out.
- No hidden dependencies on userland tools where we can reasonably avoid them.

✅ **No config‑breaking surprises**
- TOML layout is treated as a contract.
- New behavior is opt‑in, or defaults to “no change” where possible.
- All behavior changes that can affect existing VMs must be documented and version‑gated.

✅ **CPU / NUMA / IRQ awareness**
- The entire design assumes real multi‑socket, multi‑NUMA Threadripper‑class hosts.
- Scheduling, pinning, and hugepage placement must respect actual topology.

---

## 1. State Machine & Execution Model

### 1.1 VM State Machine (Phase 0–N)

✅ **Phase‑oriented VM lifecycle**
- `Init → Validate → PrepareHugepages → PreparePci → ReserveCpus → LaunchQemu → DetectThreads → PinVcpus → DetectMsi → PinIrqs → PeripheralHooks → Shutdown/Cleanup`.
- Runtime is carried through as a single `VmRuntime` object with deterministic mutation.

✅ **Core event model**
- `CoreEventKind` / `CoreEvent` exist and are accumulated on `VmRuntime`.
- Events are VM‑scoped and order‑preserving with respect to the state machine.

🔜 **Daemon/TUI projection of events**
- Project per‑VM `CoreEvent`s into daemon IPC snapshots for TUI consumption.
- Provide phase‑aware views (e.g. “show me everything PCI did for this VM”).

---

## 2. CPU & NUMA Planning

### 2.1 CPU Layout & Planning

✅ **VmCpuLayout & CpuPlan**
- `VmCpuLayout` holds host/vm CPU sets.
- `CpuPlan` encapsulates validated mappings between host topology and VM layout.

✅ **Validation on startup**
- Rejects invalid or nonsensical CPU layouts early (config‑time errors, not runtime flakiness).

🔜 **Advanced NUMA biasing**
- Prefer “local” NUMA nodes for vCPU and IRQ placement where possible.
- Express NUMA intent more richly in config (beyond a single preferred node).

### 2.2 IRQ Pinning

🚧 **IRQ pinning with completion latch**
- IRQ pinning runs asynchronously after QEMU launch.
- `irq_pinning_complete: AtomicBool` on `VmRuntime` acts as a latch once IRQs are pinned.
- No timers, no polling loops; callers simply check or re‑invoke deterministic helpers.

🔜 **More granular IRQ summaries**
- Attach “IRQ mapping summaries” to `VmRuntime` for TUI representation.
- Provide deterministic ordering (by device, by vector, etc.).

---

## 3. PCI, VFIO & Isolation

### 3.1 GPU Safety & Policy

✅ **GPU policy preflight (Phase 1–3)**
- PCI inventory + GPU detection via `PciInventory::scan()`.
- Safety classification into `GpuSafetyClass` plus unbind feasibility assessment.
- Single‑GPU host protection via `[vm.<name>.gpu] allow_single_gpu = true` gating.

✅ **Prepare GPU passthrough (Phase 4)**
- Enforces “Unsafe” unbind assessment as a hard failure.
- Logs “Risky” assessments but proceeds per operator policy.
- Performs deterministic unbind + bind to `vfio-pci`.

### 3.2 PCI Root‑Port Placement

✅ **Deterministic pci_rootport mapping**
- `build_pci_rootport_map` assigns stable PCIe root‑port slots to all passthrough devices.
- Priority ordering: GPU → NVMe → NIC → USB, each sorted by BDF.
- Supports explicit overrides via `qemu.pci_rootport` with validation.
- Emits `bus=pcie.0,addr=0xNN` for each `vfio-pci` device.

### 3.3 Isolation Policy (Phase 8/9)

✅ **Configurable isolation modes**
- `IsolationMode = Disabled | Audit | Enforce`.
- Defaults to `Disabled` to preserve legacy behavior.

✅ **IsolationLevel per device**
- `Dedicated`, `SharedWithHost`, `Forbidden` semantics modeled, ready for Phase 9 enforcement.

🔜 **Phase 9 enforcement & findings**
- Implement full IOMMU group / multifunction / host‑critical device checks.
- Emit structured isolation findings into `VmRuntime.events`.
- In `Enforce` mode, abort startup on violations before touching sysfs.

---

## 4. QEMU Builder & CPU/SMBIOS Extras

### 4.1 QEMU Command Builder

✅ **Declarative QEMU command construction**
- `QemuCommandBuilder` builds a “dumb data” `QemuCommand`:
  - Core args: `-enable-kvm`, `-cpu`, `-smp`, `-m`, optional hugepages.
  - Machine/firmware: `-machine q35,accel=kvm`, OVMF code/vars drives.
  - QMP: per‑VM Unix socket under `/run/chalybs`.
  - VFIO devices: deterministic root‑port mapping, optional `rombar=0`.
  - SMBIOS: `-smbios type=0/1/2` driven by config.

✅ **CPU model & extras plumbing**
- `qemu.cpu_model` and `qemu.cpu_extras` merged into a single `-cpu` string.
- Supports `"auto"` detection path via `cpu::detect::autodetect_qemu_cpu_model()`.
- Preserves legacy “ABI / TOPO / HV_CONTEXTS / VENDOR_ID” semantics.

✅ **RTC policy**
- Configurable via `qemu.rtc` with the following behavior:
  - `Some(non-empty)` → `-rtc <value>`.
  - `Some("")` → skip `-rtc` entirely (QEMU default).
  - `None` → default `-rtc base=localtime,driftfix=slew` (Bash parity).

---

## 5. Peripherals

### 5.1 Tasmota (MQTT Relay Power)

✅ **Tasmota peripheral hooks**
- `[vm.<name>.peripherals.tasmota]` with deterministic topic and payloads.
- `vm_up` publishes `"ON"`, `vm_down` publishes `"OFF"`.
- `VmRuntime.tasmota_powered: Cell<bool>` tracks best‑known state.

🔜 **Daemon/TUI power view**
- Surface Tasmota state in daemon snapshots for TUI display (per‑VM “powered” indicator).

### 5.2 DDC / Monitor Input Switching

✅ **In‑tree DDC client**
- Replaces external `ddcutil` dependency with direct I²C/EDID/DDC handling in Rust.
- Deterministic bus selection and input codes driven by `DdcConfig`.

✅ **IRQ‑gated VM‑up behavior**
- `apply_vm_up` defers DDC `vm_up` until `irq_pinning_complete == true`.
- No polling, no timers: daemon/TUI re‑invokes `apply_vm_up` when the latch flips.
- Clear `CoreEvent` emitted when DDC is deferred.

🚧 **DDC send test coverage**
- Most DDC tests are green; `send_ddc_set_input` integration behavior still under active validation.
- Next step: finish the last DDC test, document any hardware‑specific quirks.

### 5.3 Looking Glass (ivshmem)

✅ **Deterministic SHM preparation**
- `[vm.<name>.peripherals.looking_glass]` drives SHM creation:
  - `shm_name` (typically `/dev/shm/looking-glass`)
  - `mem_mb` (size in MiB)
- `prepare_looking_glass_shm`:
  - Creates/truncates the file, sizes it to `mem_mb` MiB, sets mode `0660`.
  - Resolves desktop user via environment (SUDO_USER/DOAS_USER/USER/LOGNAME).
  - Resolves `uid` from `/etc/passwd` and `gid` from `kvm` or the user’s primary group.
  - `chown(path, uid, gid)` with clear logging.
  - Treats creation/sizing failures as **hard** QEMU errors; ownership issues are warnings only.

✅ **Crash‑safe / reboot‑safe behavior**
- Uses create+truncate semantics so stale SHM from a prior crash is harmless.
- VM startup is responsible for (re)creating the SHM backing file each time.

✅ **QEMU ivshmem wiring**
- Adds:
  - `-object memory-backend-file,id=ivshmem,share=on,mem-path=<shm>,size=<mem_mb>M`
  - `-device ivshmem-plain,memdev=ivshmem,bus=pcie.0`

✅ **Teardown cleanup**
- SHM is removed deterministically on VM shutdown.
- Ensures no stale `/dev/shm/looking-glass` file remains between runs.

🔜 **NUMA‑aware SHM placement (Phase 12B)**
- Optionally integrate hugepage/NUMA placement for LG SHM where useful.
- Make this explicit and opt‑in to avoid surprising behavior.

### 5.4 SPICE (Clipboard / Input / Convenience Channel)

✅ **SPICE peripheral config**
- `[vm.<name>.peripherals.spice]`:
  - `enabled = true|false`
  - `port = <u16>` (e.g. 5900)
  - `addr = "<ip or hostname>"` (e.g. `"127.0.0.1"`)
- Works as a pure peripheral: no effect unless configured.

✅ **SPICE QEMU wiring (the SPICE must flow)**
- When enabled and valid:
  - `-device virtio-serial-pci,id=virtio-serial0,max_ports=16,bus=pcie.0,addr=0x10`
  - `-chardev spicevmc,name=vdagent,id=vdagent`
  - `-device virtserialport,nr=1,bus=virtio-serial0.0,chardev=vdagent,name=com.redhat.spice.0`
  - `-spice port=<port>,addr=<addr>,disable-ticketing=on`

✅ **SPICE lifecycle logging**
- `SpiceHook` emits clear `vm_up` / `vm_down` logs when enabled.
- When disabled but present in config, logs that it is “present but disabled”.

🔜 **Tighter daemon/TUI integration**
- Surface SPICE endpoints (addr/port) in daemon snapshots for TUI to show “connect” hints.
- Optional integration hints for Looking Glass + SPICE combo workflows.

---

## 6. Config & Validation

### 6.1 TOML Structure

✅ **Stable config layout**
- `RootConfig` with `vm`, `logging`, plus per‑VM:
  - `cpu`, `qemu`, `numa`, `devices`, `gpu`, `isolation`, `peripherals`.
- `PeripheralConfig` holds `tasmota`, `ddc`, `looking_glass`, `spice`.

✅ **Basic validation**
- Ensures at least one VM exists.
- Validates `num_vcpus > 0` and non‑empty CPU syntax.

🔜 **Deeper config validation**
- Cross‑check PCI BDFs against inventory at startup (with controlled behavior in headless/offline modes).
- Validate DDC buses / inputs in a read‑only preflight before exposing them to runtime.

---

## 7. Daemon & TUI Integration

### 7.1 Daemon

🔜 **Event‑rich IPC snapshots**
- Include `VmRuntime.events`, IRQ latch state, Tasmota power, LG SPICE flags, etc.
- Stabilize IPC schema so the TUI can evolve without reshaping the core often.

🔜 **Multi‑VM orchestration**
- Clean handling of multiple concurrent VMs, including ordering guarantees when bringing up or tearing down groups.

### 7.2 TUI

🧪 **“Coherent visual field” design**
- Represent VM lifecycle, PCI/VFIO state, IRQ status, and peripherals in a single coherent view.
- Avoid “UI flapping” or noisy refresh cycles; prefer stateful, phase‑aware transitions.

🔜 **Per‑VM detail panels**
- Show CPU plan, NUMA layout, IRQ pinning, PCI root‑port assignments, and peripheral status (DDC/LG/SPICE/Tasmota).

---

## 8. Testing, Tooling & Quality

### 8.1 Test Coverage

🚧 **Unit & integration tests**
- Broad coverage across CPU planning, PCI policy, and QEMU builder behavior.
- DDC coverage is mostly green; last remaining test focuses on `send_ddc_set_input` semantics.

🔜 **Hardware‑aware test harness**
- Structured “test matrix” for known hardware configs (e.g. WX/Threadripper layouts, specific GPUs).
- Targeted regression tests for specific quirks (e.g. IOMMU group oddities, multi‑function devices).

### 8.2 Tooling & Release Discipline

✅ **Semantic versioning**
- v1.3.5 → v1.4.0 for the current cycle (DDC client, LG SHM, SPICE peripheral).
- Behavior‑changing features are captured in CHANGELOG / RELEASE_NOTES.

🔜 **Continuous integration**
- Wire up a conservative CI pipeline:
  - `cargo build`, `cargo test`, `cargo clippy` with agreed‑upon lint configuration.
  - Fast, deterministic, no flaky hardware dependencies.

---

## 9. Future & “Someday / Maybe”

🧪 **Advanced scheduling / isolation modes**
- Explore alternate `cgroups` v2 profiles and NUMA policies for ultra‑low‑latency VMs.
- Optional “strict isolation” preset that pushes more checks into Enforce‑mode defaults.

🧪 **More sophisticated PCI/ACS awareness**
- Per‑platform ACS quirks database (purely declarative, clearly versioned).
- Ability to surface hardware restrictions and recommended BIOS settings through the TUI.

🧪 **Alternative front‑ends**
- TUI‑only “headless controller” modes.
- Possible future REST/gRPC control plane (still strictly deterministic).

💤 **Non‑x86 architectures**
- No near‑term plan, but the core model should keep ARM/other architectures in mind where straightforward.

---

## 10. Current Cycle Summary (v1.4.0)

This release cycle (from v1.3.5 → v1.4.0) captures:

- ✅ In‑tree DDC client for monitor input switching (no external `ddcutil` dependency).
- ✅ IRQ‑gated DDC vm_up behavior with deterministic latch semantics.
- ✅ Looking Glass SHM bring‑up and teardown:
  - Deterministic user/ownership resolution.
  - Crash‑safe/reboot‑safe SHM recreation.
- ✅ SPICE peripheral wiring in QEMU builder:
  - Virtio‑serial, vdagent channel, and SPICE server flags.
  - Clean on/off semantics via `[vm.<name>.peripherals.spice] enabled`.
- 🚧 Remaining: tighten DDC tests (`send_ddc_set_input`) and finish documentation updates for this cycle.

Everything else above remains either historical, in progress, or planned work for upcoming minor/patch releases.
