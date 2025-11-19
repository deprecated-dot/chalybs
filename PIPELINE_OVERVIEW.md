# Chalybs Execution Pipeline
### Version: 0.3.1

This document fully explains the Chalybs VM lifecycle pipeline in deterministic order.

---

# 1. Overview

Chalybs uses a strict, ordered state machine to ensure reproducible, NUMA-aware, capability-aware VM launches.

Pipeline:

```
Init
Validate
ResolveModeAndCapabilities
ReserveCpus (cpuset creation)
LaunchQemu
DetectThreads
PinVcpus
DetectMsi
PinIrqs
PeripheralHooks
Steady
```

---

# 2. State Breakdown

## **Init**
- Prepare logger
- Load config file
- Resolve VM name
- Construct VmRuntime

---

## **Validate**
- Validate config fields
- Check QEMU binary/firmware
- Ensure cpusets are consistent

---

## **ResolveModeAndCapabilities**
- Detect host NUMA topology  
- Detect GPU count + vfio bindings  
- Detect PCIe IOMMU mapping  
- Determine VM mode  
- Run C2 host CPU derivation logic  

Output:
- Final host cpuset  
- Final VM cpuset  
- Mode object  
- Capability flags  

---

## **ReserveCpus**
- Create `/sys/fs/cgroup/vfio_vm`  
- Create `/sys/fs/cgroup/vfio_host`  
- Write cpuset.cpus / cpuset.mems  

---

## **LaunchQemu**
- Start QEMU
- Move PID → vm cpuset  
- Create QMP socket

---

## **DetectThreads**
- Wait for all vCPU threads via:
  1. QMP `query-cpus-fast`, else
  2. Procfs fallback

---

## **PinVcpus**
- For each vCPU index:
  - derive tid
  - map vcpu → host CPU
  - sched_setaffinity() with deterministic handling

---

## **DetectMsi**
- Discover MSI vectors for each passed-through PCIe device
- Determine IRQ → NUMA mapping

---

## **PinIrqs**
- Move all VM-related IRQs to correct NUMA node  
- Avoid CPU overlap with VM vCPUs  
- Avoid IRQ steering to isolated nodes  

---

## **PeripheralHooks**
Optional:
- Tasmota (smart power switching)
- DDC monitor input switching
- Looking-glass shared memory setup

---

## **Steady**
VM is live and fully pinned.

---

# 3. Shutdown Pipeline

```
Shutdown
Cleanup (destroy cpusets)
Idle
```

---

# 4. Summary

This deterministic pipeline ensures:
- reproducible launches  
- correct NUMA behavior  
- safe GPU binding  
- ideal IRQ/vCPU placement  
- portable behavior on all hardware  

Chalybs v0.3.1 is now fully deterministic, NUMA-aware, and capability-driven.
