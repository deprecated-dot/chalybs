# Chalybs v0.3.4 Release Notes

Chalybs v0.3.4 introduces deterministic and verifiable PCI passthrough
staging, completing Phases 5 and 6 of the PCI subsystem.

---

## 🆕 Major Features

### 1. VFIO Action Planning (PCI Phase 5)
Chalybs now constructs an explicit ordered plan describing:
- Which devices require unbinding
- Which devices require binding to vfio-pci
- Why each action exists (human-readable reasons)
- Strict ordering guarantees

This plan is a pure function: no sysfs writes.

---

### 2. VFIO Execution (PCI Phase 6)
- All unbinds are performed first.
- All binds to vfio-pci occur after all unbinds are complete.
- Execution is idempotent and uses only kernel sysfs APIs.
- No timers, sleeps, or userland heuristics.
- No external dependencies.

---

### 3. Post-Bind Verification
After executing the plan:
- PCI inventory is rescanned.
- Each configured passthrough device is validated to be vfio-pci bound.
- Any mismatch results in a hard failure before QEMU launch.

This closes the loop between inventory, policy, action, and verification.

---

## 🔧 Internal Improvements
- pci.rs restored to full, correct baseline.
- Clippy-clean strategy applied without altering semantics.
- Logging improved for VFIO actions and verification flow.
- State machine wiring enhanced (`PreparePci` now fully operational).

---

## 📘 Documentation
- No changes required to `CHALYBS_EXECUTION_AND_ARCHITECTURE.md` for this release.
- PCI Phase 5–6 is already represented at a conceptual level.

---

## ✅ Compatibility
- Fully backward compatible with v0.3.3.
- No VM config changes required.
