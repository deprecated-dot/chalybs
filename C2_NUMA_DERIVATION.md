# NUMA-Aware Host CPU Derivation (C2 Logic)
### Version: 0.3.1

This document fully details the C2 host CPU derivation logic added in v0.3.1.

---

# 1. Purpose

C2 Logic exists to:

1. Detect the NUMA node of all VM vCPUs  
2. Detect the NUMA node(s) of all passed-through PCIe devices  
3. Derive **correct host cpusets** that avoid:
   - cross-node RAM traffic
   - stolen bandwidth from the VM
   - IRQ latency spikes

This ensures **deterministic performance**, especially on Threadripper, EPYC, Xeon-W, and Ampere.

---

# 2. Input Sources

### From VM Config:
- `cpu.vm_cpus` (required)
- `cpu.host_cpus` (optional override)
- `numa.node` (optional specificity)

### From HostCapabilities:
- host NUMA topology  
- mapping: CPU → NUMA node  
- mapping: PCI device → NUMA node  

---

# 3. Derivation Algorithm

If user specifies `host_cpus`, we respect it *as-is*.

Else Chalybs derives:

1. Identify NUMA node(s) used by VM CPUs:
```
vm_nodes = set(map(cpu → numa_node))
```

2. Compute host nodes:
```
host_nodes = all_nodes - vm_nodes
```

3. Collect CPUs belonging to host nodes:
```
host_cpus = { cpu ∈ online_cpus | numa(cpu) ∈ host_nodes }
```

4. Validate:
- Ensure host_cpus is not empty  
- Ensure it contains at least `num_vcpus` spare CPUs  
- Validate no overlap with vm_cpus

5. Final cpuset:
- vm → exact configured VM cores  
- host → derived C2 host cores

---

# 4. Example (Threadripper 2990WX)

VM wants:
```
vm_cpus = 8-15,40-47
→ node 2
```

Host nodes:
```
{0,1,3}
```

Derived:
```
host_cpus = CPUs from nodes 0,1,3
host_mems = nodes 0,1,3
```

---

# 5. Determinism and Safety

### Deterministic
- identical input produces identical cpusets  
- zero kernel heuristics  
- zero QEMU heuristics  

### Safe
- VM node becomes “exclusive domain”  
- Host never steals cycles from VM  
- IRQ steering remains on correct nodes  

---

# 6. Summary

The C2 logic guarantees:
- deterministic performance
- NUMA-correct binding
- perfect isolation of VM and host workloads

It resolves Threadripper quirks cleanly while remaining portable for “normal” systems.

