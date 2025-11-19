# Chalybs Mode + Capability Architecture
### Version: 0.3.1

This document provides a complete description of the Mode System and Host Capability Detection architecture introduced in Chalybs v0.3.1.  
It outlines how the VM runtime decides *how* to operate (mode) and *what the hardware allows* (capabilities).

---

## 1. Overview

Chalybs now uses a **two-layer deterministic system**:

### **Layer 1 — HostCapabilities (automatic)**
Detected automatically at runtime:
- Number of NUMA nodes  
- CPU topology  
- GPU count and PCIe function layout  
- Host IOMMU presence  
- Whether vfio-pci is bound or available  
- Whether the system has a shared or split GPU configuration

This layer is *purely descriptive* — it tells Chalybs what the hardware *is*.

### **Layer 2 — Mode (user + inference)**
Determines *how to operate*.  
Two key inputs:
1. `vm.cfg.mode` (optional)
2. Automatic inference from HostCapabilities

Modes currently defined:

| Mode | Description |
|------|-------------|
| `DualGpuPassthrough` | Host GPU + Guest GPU. Clean separation. No host fencing required. |
| `SingleGpuPassthrough` | VM needs the only GPU. Requires fencing/unfencing or full vfio takeover. |
| `DedicatedGpuPassthrough` | GPU is always reserved for the VM (boot-time vfio-pci binding). |
| `HybridFallback` | Automated fallback when ambiguity exists. |
| `UnknownUnsupported` | Detected but unsupported state. |

Chalybs fuses Mode + Capabilities to derive all CPU/IRQ/DDC safety operations.

---

## 2. Resolution Pipeline

**`Mode::resolve(vm_name, cfg, capabilities)`** produces:
- Deterministic Mode
- Derived host CPU list (C2 logic)
- Allowed or disallowed operations:
  - IRQ pinning
  - vCPU pinning
  - GPU binding/unbinding
  - DDC switching
  - Tasmota hooks

### Logic (simplified):

```
if user explicitly sets mode:
    validate against capabilities
    if valid → accept
    else → error
else:
    infer:
        if multiple GPUs → DualGpuPassthrough
        else if one GPU but vfio-pci active → DedicatedGpuPassthrough
        else if one GPU → SingleGpuPassthrough
        else → UnknownUnsupported
```

---

## 3. Effects on VM Lifecycle

| Subsystem | Dual GPU | Single GPU | Dedicated GPU |
|----------|----------|------------|----------------|
| vCPU pinning | Always on | Always on | Always on |
| IRQ pinning | Always on | Always on | Always on |
| GPU unbind/rebind | Never | Required | Never (already fenced at boot) |
| DDC switching | Optional | Required | Optional |
| Tasmota switching | Optional | Optional | Optional |
| PCI reset handling | Minimal | Heavy | None |

---

## 4. Why This Architecture?

### **Deterministic**  
No ambiguity. Every VM startup resolves into:
- a mode
- a capability set
- a safe operation subset

### **Portable**  
Threadripper/HEDT NUMA quirks are handled via capabilities.  
Users on “normal” machines get simple, safe behavior.

### **Extensible**  
New modes (e.g., “SR-IOV mixed passthrough”) plug directly into this system.

---

## 5. Future Extensions
Planned:
- Capability caching + diff detection  
- Mode override with partial behavior masks  
- “Safe Mode” automatic fallback if QEMU launch fails  

---

## 6. Summary

The Mode + Capability architecture allows Chalybs to run deterministically and safely across:
- single-GPU desktops  
- dual-GPU workstations  
- NUMA-heavy HEDT systems  
- dedicated VFIO servers  

It is now the foundation for all future features.

