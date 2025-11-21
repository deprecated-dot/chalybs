# Chalybs ROADMAP
## v0.3.x Series – PCI Safety, VFIO Refinement, State Machine Stability
- Complete GPU safety classification (Phases 1–5)
- Add PCI unbind/bind simulation and real VFIO binding primitives
- Harden cpuset NUMA behavior (C2)
- Stabilize QEMU launch path + thread/IRQ pinning race‑condition cleanup
- Unified Architecture Document integration

## v0.4.x – VFIO Orchestration & Hotplug Era
- Full VFIO device manager abstraction
- Per‑device bind/unbind orchestration with dry‑run mode
- Implement live PCI rescan helpers
- Begin work on hot‑plug/HOTREMOVE safety heuristics
- Coordinator/daemon groundwork

## v0.5.x – Daemon & IPC
- Implement chalybsd event loop
- UDS IPC protocol + request/response model
- VM lifecycle management from daemon, CLI becomes a thin client
- Real‑time telemetry: IRQ mapping, MSI vector report, thread heatmaps

## v0.6.x – NUMA Optimizer & Scheduler
- Automated NUMA placement scoring
- Memory locality advisor
- IRQ affinity watching service
- Predictive pinning scheduler for transient QEMU threads

## v0.7.x – Device Graph / Auto-Topology Detection
- IOMMU graph builder
- Auto-detection of unsafe sharing between groups
- “Show me why this device cannot be passed through” explanations
- Visualization export (mermaid + SVG)

## v0.8.x – Advanced Peripheral Hooks
- Full DDC utilization (monitor input switching via policy triggers)
- Looking Glass optimized integration
- USB/NVMe safe handoffs
- Per-VM hook bundles (pre-launch, post-launch, shutdown)

## v0.9.x – Reliability / Hardened Mode
- Full transactional VM launch (rollback-on-failure)
- Health probes + watchdog
- Daemon-on-boot startup sequencing
- Hermetic mode for production environments

## v1.0 – Production-Grade Chalybs
- Versioned configuration schema
- Stable IPC
- Complete GPU/iGPU orchestration strategy
- Verified passthrough safety engine
- End-to-end test matrix: PCI, QEMU, IRQs, NUMA, VFIO workflows
