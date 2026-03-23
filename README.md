# Chalybs

**A deterministic VFIO virtualization orchestrator for Linux.**

Chalybs manages the full lifecycle of GPU passthrough VMs on VFIO/KVM hosts. It replaces the heuristic-laden, timer-based patterns common in existing tooling with a strict, observable state machine where every decision is grounded in a real sysfs data point. If Chalybs cannot verify something, it fails loudly and explicitly — it does not guess, sleep and retry, or silently degrade.

It was built on a Threadripper workstation with a complex, multi-GPU VFIO setup that existing tools handled poorly. It now handles everything those tools couldn't.

---

## Why not virt-manager?

virt-manager works for simple setups. It does not solve — and in several cases actively avoids solving — problems that matter on serious VFIO hardware:

- **Hugepage provisioning is not NUMA-aware.** Allocating hugepages on the wrong node silently degrades performance, sometimes catastrophically.
- **vCPU pinning is manual and fragile.** There is no topology-aware planner; the user is expected to figure out the right host CPUs and hope they stay correct across reboots.
- **IRQ pinning is absent or heuristic.** MSI/MSI-X affinities for passthrough devices are not managed. Some wrappers add sleep timers and pray the guest has finished PCI enumeration. Chalybs does not do this.
- **IOMMU group coherence is not verified.** You can configure a passthrough device whose IOMMU group contains host-critical devices and virt-manager will not tell you.
- **NUMA locality of passthrough devices is ignored.** A GPU on the wrong die causes cross-NUMA PCIe traffic for every frame. Chalybs verifies and enforces NUMA locality across the entire device set.
- **The startup sequence is not deterministic.** Race conditions are papered over with timers. When they fail, bugs are marked WONTFIX.

Chalybs solves all of these. Not with heuristics — with observable sysfs surfaces, deterministic planners, and hard errors on violations.

---

## What it does

**NUMA-aware hugepage provisioning.** Chalybs mounts a managed `hugetlbfs`, drops pagecache, requests memory compaction, and raises `nr_hugepages` on the specific NUMA node where the VM's CPUs live. It verifies allocation before proceeding. On shutdown it returns node-local and global counts to zero and unmounts cleanly.

**Deterministic CPU planning.** Chalybs builds a `CpuPlan` from host topology discovered via sysfs. vCPUs are pinned to NUMA-local host CPUs. The plan is validated during preflight — if the requested layout cannot be satisfied, startup fails during `Validate`, not silently at runtime.

**VFIO isolation enforcement.** IOMMU group membership is fully inventoried before any sysfs writes. Chalybs distinguishes between groups that are exclusive to the VM, groups containing host-owned devices, and groups that are unsafe to pass through. Isolation policy runs in `audit` (log only) or `enforce` (hard error) mode, configurable per VM.

**Asynchronous MSI/MSI-X IRQ pinning.** A worker thread is launched after QEMU spawns and polls for IRQ appearance as the guest enumerates PCI devices. Each discovered IRQ is pinned to NUMA-local VM CPUs immediately on discovery. There are no sleep timers. Auxiliary GPU audio functions are recognized and handled separately.

**cgroup isolation.** VM vCPU threads are moved into a dedicated `vfio_vm` cgroup on launch. Host processes are confined to `vfio_host`. The split is derived from the CPU plan and applied atomically.

**Native DDC/CI monitor input switching.** Chalybs includes a purpose-built DDC/CI engine that switches monitor inputs over I²C at the correct lifecycle moment — after IRQ pinning is complete, not at an arbitrary delay. The implementation was reverse-engineered from scratch; it does not depend on ddcutil or any external tool.

**MQTT/Tasmota smart power control.** USB hubs for guest devices are gated behind Tasmota smart relays. Chalybs publishes `POWER ON` and `POWER OFF` to the configured MQTT broker at VM start and stop, preventing devices from being enumerated on the host while the guest is running.

**Looking Glass + SPICE integration.** The Looking Glass shared memory region is created deterministically with explicit ownership, NUMA-aware sizing, and crash-safe cleanup. SPICE is wired with a `virtio-serial` bus and `vdagent` channel.

**Deterministic QEMU command builder.** The full QEMU argument vector is assembled by a declarative builder from the config and the resolved `CpuPlan`. PCIe root port allocation is deterministic and stable across reboots. Given the same config and host topology, the resulting QEMU invocation is bit-for-bit identical every time.

**QEMU CPU model autodetection.** On AMD Zen-class hosts (family `>= 0x17`, including Threadripper and EPYC), Chalybs automatically selects `EPYC-v2` as the QEMU CPU model. This is deterministic and based on `/proc/cpuinfo` — no guesswork.

---

## Architecture

Chalybs is structured as three crates:

| Crate | Role |
|---|---|
| `chalybs-core` | State machine, VFIO/PCI pipeline, CPU planning, all sysfs interaction |
| `chalybsd` | Long-running daemon with IPC socket and multi-client TUI support |
| `chalybs-tui` | Terminal frontend; communicates with the daemon exclusively via IPC |

The VM lifecycle is a strict state machine. States gate on the successful completion of the previous state. There is no re-entry, no fallback, and no silent continuation on error.

```
Init → Validate → PrepareHugepages → PreparePci → ReserveCpus →
LaunchQemu → DetectThreads → PinVcpus → DetectMsi → PinIrqs →
PeripheralHooks → Steady → Shutdown → Cleanup → Idle
```

The daemon exposes a Unix domain socket at `/run/chalybsd.sock`. The TUI connects as a client. Multiple clients can connect simultaneously. The IPC contract is stable — the daemon and TUI are independently versioned against it.

---

## Configuration

Chalybs is configured via a single TOML file. Everything else — host NUMA topology, PCI device inventory, IOMMU group membership, CPU identity, IRQ layout — is discovered at runtime from sysfs. The config describes intent; Chalybs verifies that the host can satisfy it before touching anything.

See [`config/chalybs.example.toml`](config/chalybs.example.toml) for a fully annotated example covering CPU layout, QEMU arguments, passthrough devices, SMBIOS injection, isolation policy, and peripheral hooks.

---

## Current Status

Chalybs is at **v1.2.2**, in active personal use on a Threadripper 2990WX workstation with the following passthrough configuration:

- 1× RTX 3080 (Windows guest GPU)
- 1× RX 5700 XT (Linux host GPU)
- 4× NVMe drives
- 1× AQC113 10GbE NIC
- 1× USB controller (via Sonnet McFiver PCIe card)

All 13 development phases are complete. The core orchestrator, daemon, IPC, and peripheral stack are stable and considered feature-complete for this hardware profile.

The TUI is functional and includes a visual effects engine (scanlines, matrix watermark, border EMI shimmer, pulsing VM state indicators) controllable at runtime via the Chalybs shell. A TUI rewrite is planned before a formal public release.

See [`ROADMAP.md`](ROADMAP.md) for upcoming phases, and [`CHANGELOG.md`](CHANGELOG.md) for full version history.

---

## What Chalybs does not do (yet)

- Installer or onboarding wizard — configuration is currently manual
- Hotplug / hot-unplug of passthrough devices
- Cross-VM orchestration
- Multi-GPU mirror/clone strategies
- Remote TUI mode

These are tracked in the roadmap.

---

## Principles

Chalybs is built on a small set of invariants that do not bend:

- **No heuristics.** Every decision is backed by an observable data point.
- **No silent degradation.** If a precondition cannot be verified, startup fails with an explicit error.
- **No timer-based synchronization.** IRQ discovery, hugepage verification, and device binding are all confirmed, not assumed.
- **Completed phases do not regress.** Once a phase's behavior is locked, it stays locked. New behavior is additive only.
- **Zero external dependencies at runtime.** No ddcutil, no libvirt, no Python. Everything runs through the kernel's own interfaces.

---

## License

Chalybs is open source software. See [`LICENSE`](LICENSE) for details.

It was built on the shoulders of the Linux kernel, QEMU, VFIO, KVM, and every contributor who ever filed a bug or wrote documentation about how this stack actually works. Gatekeeping knowledge is antithetical to why any of this exists.
