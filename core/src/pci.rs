//! PCI/PCIe inventory and helpers.
//!
//! This module is responsible for *reading* host PCI topology from sysfs
//! and presenting it in a structured way. Policy decisions (what to pass
//! through, single-GPU safety, etc.) live in `config::pci` and consume
//! these types.
//!
//! The intent is:
//!   * deterministic: only sysfs, no `lspci` or external tools
//!   * conservative: failure to read one device does not abort inventory
//!   * portable: works on any reasonably modern Linux kernel

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use tracing::{debug, info, warn};

use crate::config::PciDeviceConfig;
use crate::errors::{ChalybsError, Result};

/// Representation of a single PCI function on the host, based on sysfs.
///
/// Example sysfs layout:
///   /sys/bus/pci/devices/0000:0b:00.0/
///     ├── vendor        -> "0x10de"
///     ├── device        -> "0x1b80"
///     ├── class         -> "0x030000"
///     ├── driver        -> symlink to ".../drivers/nvidia"
///     ├── iommu_group   -> symlink to ".../iommu_groups/19"
///     └── numa_node     -> "2" or "-1"
#[derive(Debug, Clone)]
pub struct PciFunction {
    /// Full BDF string, e.g. "0000:0b:00.0".
    pub bdf: String,
    pub vendor_id: u16,
    pub device_id: u16,
    /// Raw PCI class code, e.g. 0x030000 for VGA controller.
    pub class: u32,
    /// Bound kernel driver name (e.g. "vfio-pci", "amdgpu"), if any.
    pub driver: Option<String>,
    /// IOMMU group id, if discoverable.
    pub iommu_group: Option<u32>,
    /// NUMA node id, or -1 if unknown / not associated.
    pub numa_node: Option<i32>,
}

/// GPU-specific driver binding kinds discovered from sysfs for display
/// controllers. This is a pure classification of the bound kernel
/// driver name; it does not itself apply any policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuDriverKind {
    /// Bound to vfio-pci, typically ready for passthrough.
    Vfio,
    /// Bound to an AMD GPU driver such as amdgpu or radeon.
    AmdGpu,
    /// Bound to the proprietary NVIDIA driver.
    Nvidia,
    /// Bound to the open-source nouveau driver.
    Nouveau,
    /// Bound to some other kernel driver; we preserve the raw name.
    OtherKernel(String),
    /// No bound kernel driver for this GPU.
    Unbound,
}

/// Safety classification for host GPUs from the perspective of
/// passthrough. This is purely descriptive in Phase 2 and does not
/// change policy decisions yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuSafetyClass {
    /// Bound to vfio-pci and expected to be safe for passthrough.
    VfioReady,
    /// Bound to a host GPU driver and likely backing host display or
    /// otherwise owned by the host.
    HostOwned,
    /// No driver or an unknown driver; requires operator judgement.
    Unknown,
}

/// Phase 3/4: unbind safety simulation result for a GPU.
///
/// This does *not* itself unbind anything; it only describes how safe
/// it appears to be to detach this device based on current topology.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuUnbindFeasibility {
    /// Nothing suspicious detected from inventory’s point of view.
    Safe,
    /// Potentially disruptive but not clearly fatal; operator judgement
    /// required before attempting unbind.
    Risky(String),
    /// Strong indicators that unbinding would be unsafe.
    Unsafe(String),
}

/// Logical classification of a PCI function for policy purposes.
///
/// This is *purely* derived from the PCI class code and existing
/// helpers; there are no external tools or additional heuristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    Gpu,
    Nvme,
    Nic,
    UsbController,
    StorageOther,
    Other,
}

impl PciFunction {
    /// Return the (base, sub, prog_if) triple derived from the raw class.
    ///
    /// See PCI spec:
    ///   class = (base << 16) | (sub << 8) | prog_if
    pub fn class_triple(&self) -> (u8, u8, u8) {
        let base = ((self.class >> 16) & 0xff) as u8;
        let sub = ((self.class >> 8) & 0xff) as u8;
        let prog_if = (self.class & 0xff) as u8;
        (base, sub, prog_if)
    }

    /// Base class code (e.g. 0x03 for display controller).
    pub fn class_base(&self) -> u8 {
        (self.class >> 16) as u8
    }

    /// Subclass code.
    pub fn class_sub(&self) -> u8 {
        ((self.class >> 8) & 0xff) as u8
    }

    /// Programming interface byte.
    pub fn class_prog_if(&self) -> u8 {
        (self.class & 0xff) as u8
    }

    /// For display controllers, classify the bound kernel driver into a
    /// GPU driver kind. Returns None for non-GPU functions.
    pub fn gpu_driver_kind(&self) -> Option<GpuDriverKind> {
        if !self.is_display_controller() {
            return None;
        }

        let driver = match &self.driver {
            Some(d) => d.as_str(),
            None => return Some(GpuDriverKind::Unbound),
        };

        let kind = match driver {
            "vfio-pci" => GpuDriverKind::Vfio,
            "amdgpu" | "radeon" => GpuDriverKind::AmdGpu,
            "nvidia" => GpuDriverKind::Nvidia,
            "nouveau" => GpuDriverKind::Nouveau,
            other => GpuDriverKind::OtherKernel(other.to_string()),
        };

        Some(kind)
    }

    /// For display controllers, classify the safety of this GPU from the
    /// host perspective. Returns None for non-GPU functions.
    pub fn gpu_safety_class(&self) -> Option<GpuSafetyClass> {
        let kind = self.gpu_driver_kind()?;

        let safety = match kind {
            GpuDriverKind::Vfio => GpuSafetyClass::VfioReady,
            GpuDriverKind::AmdGpu | GpuDriverKind::Nvidia | GpuDriverKind::Nouveau => {
                GpuSafetyClass::HostOwned
            }
            GpuDriverKind::OtherKernel(_) | GpuDriverKind::Unbound => GpuSafetyClass::Unknown,
        };

        Some(safety)
    }

    /// Return true if this function is some kind of display controller (GPU).
    pub fn is_display_controller(&self) -> bool {
        self.class_base() == 0x03
    }

    /// Return true if this looks like a network controller (NIC).
    pub fn is_network_controller(&self) -> bool {
        self.class_base() == 0x02
    }

    /// Return true if this looks like a mass storage controller.
    pub fn is_storage_controller(&self) -> bool {
        self.class_base() == 0x01
    }

    /// Heuristic: NVMe typically reports class 0x010802.
    pub fn is_nvme(&self) -> bool {
        let (base, sub, _pi) = self.class_triple();
        base == 0x01 && sub == 0x08
    }

    /// USB host controllers are 0x0c03xx.
    pub fn is_usb_controller(&self) -> bool {
        let (base, sub, _pi) = self.class_triple();
        base == 0x0c && sub == 0x03
    }

    /// Deterministic device classification based solely on the PCI
    /// class code and the helpers above.
    pub fn device_class(&self) -> DeviceClass {
        if self.is_display_controller() {
            DeviceClass::Gpu
        } else if self.is_nvme() {
            DeviceClass::Nvme
        } else if self.is_network_controller() {
            DeviceClass::Nic
        } else if self.is_usb_controller() {
            DeviceClass::UsbController
        } else if self.is_storage_controller() {
            DeviceClass::StorageOther
        } else {
            DeviceClass::Other
        }
    }

    /// Return the (domain,bus,slot) triple parsed from the BDF.
    ///
    /// This ignores the function number and returns None on malformed BDFs.
    pub fn slot_key(&self) -> Option<(u16, u8, u8)> {
        parse_bdf_slot(&self.bdf)
    }

    /// Phase 4: unbind this device from its current kernel driver, if any.
    ///
    /// This uses the classic sysfs interface:
    ///   /sys/bus/pci/drivers/<driver>/unbind  ← echo BDF
    pub fn unbind_current_driver(&self) -> Result<()> {
        let driver_name = match &self.driver {
            Some(d) => d,
            None => {
                debug!(
                    bdf = self.bdf.as_str(),
                    "no bound driver for PCI function; skipping unbind"
                );
                return Ok(());
            }
        };

        // Option A semantics:
        // If this function is already bound to vfio-pci, we treat it as
        // operator-assigned for passthrough and never attempt to unbind it.
        if driver_name == "vfio-pci" {
            debug!(
                bdf = self.bdf.as_str(),
                "PCI function bound to vfio-pci; skipping unbind per Option A semantics"
            );
            return Ok(());
        }

        let driver_unbind_path = Path::new("/sys/bus/pci/drivers")
            .join(driver_name)
            .join("unbind");

        if !driver_unbind_path.exists() {
            return Err(ChalybsError::Vfio(format!(
                "cannot unbind {bdf}: driver unbind path {} does not exist",
                driver_unbind_path.display(),
                bdf = self.bdf
            )));
        }

        debug!(
            bdf = self.bdf.as_str(),
            driver = driver_name.as_str(),
            path = %driver_unbind_path.display(),
            "unbinding PCI function from current driver"
        );

        // NOTE: we deliberately pass a reference here to avoid moving the
        // PathBuf, since we still use it in the error path.
        fs::write(&driver_unbind_path, format!("{}\n", self.bdf)).map_err(|e| {
            ChalybsError::Vfio(format!(
                "failed to write BDF {} to {} for unbind: {e}",
                self.bdf,
                driver_unbind_path.display()
            ))
        })
    }

    /// Phase 4: bind this device to vfio-pci, if possible.
    ///
    /// If it is already bound to vfio-pci, this is a no-op.
    ///
    /// This uses:
    ///   /sys/bus/pci/drivers/vfio-pci/bind  ← echo BDF
    ///
    /// We assume the operator has already loaded the vfio-pci module and
    /// configured any vendor/device id matching as needed.
    pub fn bind_to_vfio_pci(&self) -> Result<()> {
        if matches!(self.driver.as_deref(), Some("vfio-pci")) {
            debug!(
                bdf = self.bdf.as_str(),
                "PCI function already bound to vfio-pci; skipping bind"
            );
            return Ok(());
        }

        let bind_path = Path::new("/sys/bus/pci/drivers/vfio-pci/bind");

        if !bind_path.exists() {
            return Err(ChalybsError::Vfio(format!(
                "vfio-pci bind path {} does not exist; \
                 ensure vfio-pci is loaded and configured",
                bind_path.display()
            )));
        }

        debug!(
            bdf = self.bdf.as_str(),
            path = %bind_path.display(),
            "binding PCI function to vfio-pci"
        );

        // bind_path is a &Path; it already implements AsRef<Path>, so we can
        // pass it directly without extra borrowing or hacks.
        fs::write(bind_path, format!("{}\n", self.bdf)).map_err(|e| {
            ChalybsError::Vfio(format!(
                "failed to bind BDF {} to vfio-pci via {}: {e}",
                self.bdf,
                bind_path.display()
            ))
        })
    }

    /// Generic helper: bind this device to an arbitrary kernel driver
    /// under `/sys/bus/pci/drivers/<driver>/bind`.
    ///
    /// If it is already bound to this driver (based on the inventory
    /// snapshot), this is treated as a no-op.
    pub fn bind_to_driver(&self, driver: &str) -> Result<()> {
        if matches!(self.driver.as_deref(), Some(d) if d == driver) {
            debug!(
                bdf = self.bdf.as_str(),
                driver, "PCI function already bound to requested driver; skipping bind"
            );
            return Ok(());
        }

        let bind_path = Path::new("/sys/bus/pci/drivers").join(driver).join("bind");

        if !bind_path.exists() {
            return Err(ChalybsError::Vfio(format!(
                "driver bind path {} does not exist for `{driver}`; \
                 ensure the module is loaded",
                bind_path.display()
            )));
        }

        debug!(
            bdf = self.bdf.as_str(),
            driver,
            path = %bind_path.display(),
            "binding PCI function to driver"
        );

        // Same rationale as unbind_current_driver(): keep ownership of the
        // PathBuf so we can still use bind_path.display() in the error path.
        fs::write(&bind_path, format!("{}\n", self.bdf)).map_err(|e| {
            ChalybsError::Vfio(format!(
                "failed to bind BDF {} to driver `{}` via {}: {e}",
                self.bdf,
                driver,
                bind_path.display()
            ))
        })
    }
}

/// Collection of all discovered PCI functions on the host.
#[derive(Debug, Clone)]
pub struct PciInventory {
    pub functions: Vec<PciFunction>,
}

/// Summary information for GPU-like PCI functions, including their
/// driver binding and safety classification.
#[derive(Debug, Clone)]
pub struct GpuFunctionSummary {
    pub bdf: String,
    pub vendor_id: u16,
    pub device_id: u16,
    pub driver: Option<String>,
    pub driver_kind: Option<GpuDriverKind>,
    pub safety: Option<GpuSafetyClass>,
}

/// Phase 3/4: simulated unbind safety for a given GPU.
#[derive(Debug, Clone)]
pub struct GpuUnbindAssessment {
    pub bdf: String,
    pub current_driver: Option<String>,
    pub safety_class: GpuSafetyClass,
    pub iommu_group: Option<u32>,

    /// Other members in the same IOMMU group (BDFs), excluding this GPU.
    pub group_members: Vec<String>,

    pub feasibility: GpuUnbindFeasibility,
}

impl PciInventory {
    /// Scan `/sys/bus/pci/devices` and build an inventory.
    pub fn scan() -> Result<Self> {
        let root = Path::new("/sys/bus/pci/devices");
        if !root.exists() {
            return Err(ChalybsError::Vfio(format!(
                "PCI devices directory {} does not exist",
                root.display()
            )));
        }

        let mut functions = Vec::new();

        let entries = fs::read_dir(root).map_err(|e| {
            ChalybsError::Vfio(format!(
                "failed to list PCI devices in {}: {e}",
                root.display()
            ))
        })?;

        for entry_res in entries {
            let entry = match entry_res {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = ?e, "skipping unreadable PCI entry");
                    continue;
                }
            };

            let path = entry.path();
            let bdf = match entry.file_name().into_string() {
                Ok(s) => s,
                Err(_) => {
                    debug!(path = %path.display(), "non-UTF8 PCI BDF; skipping");
                    continue;
                }
            };

            match read_function(&path, &bdf) {
                Ok(func) => {
                    debug!(bdf = func.bdf.as_str(), "discovered PCI function");
                    functions.push(func);
                }
                Err(e) => {
                    warn!(
                        bdf = bdf.as_str(),
                        error = ?e,
                        "failed to read PCI function"
                    );
                }
            }
        }

        info!(count = functions.len(), "PCI inventory built");
        Ok(PciInventory { functions })
    }

    /// Helper: count display controllers (VGA/3D) by class code.
    ///
    /// We treat any function with base class 0x03 as a "GPU candidate".
    pub fn count_display_controllers(&self) -> usize {
        self.functions
            .iter()
            .filter(|f| f.is_display_controller())
            .count()
    }

    /// Return all functions that look like GPUs.
    pub fn gpus(&self) -> Vec<&PciFunction> {
        self.functions
            .iter()
            .filter(|f| f.is_display_controller())
            .collect()
    }

    /// Return a summary of all GPU-like functions, including driver
    /// binding and safety classification.
    pub fn gpu_summaries(&self) -> Vec<GpuFunctionSummary> {
        self.gpus()
            .into_iter()
            .map(|f| GpuFunctionSummary {
                bdf: f.bdf.clone(),
                vendor_id: f.vendor_id,
                device_id: f.device_id,
                driver: f.driver.clone(),
                driver_kind: f.gpu_driver_kind(),
                safety: f.gpu_safety_class(),
            })
            .collect()
    }

    /// Return all functions that look like network connectors.
    pub fn nics(&self) -> Vec<&PciFunction> {
        self.functions
            .iter()
            .filter(|f| f.is_network_controller())
            .collect()
    }

    /// Return all functions that look like NVMe storage.
    pub fn nvmes(&self) -> Vec<&PciFunction> {
        self.functions.iter().filter(|f| f.is_nvme()).collect()
    }

    /// Return all functions that look like USB host controllers.
    pub fn usb_controllers(&self) -> Vec<&PciFunction> {
        self.functions
            .iter()
            .filter(|f| f.is_usb_controller())
            .collect()
    }

    /// Group all discovered functions by IOMMU group.
    ///
    /// Devices without a discoverable group are omitted.
    pub fn by_iommu_group(&self) -> HashMap<u32, Vec<&PciFunction>> {
        let mut map: HashMap<u32, Vec<&PciFunction>> = HashMap::new();

        for func in &self.functions {
            if let Some(gid) = func.iommu_group {
                map.entry(gid).or_default().push(func);
            }
        }

        map
    }

    /// Find a function by its BDF (e.g. "0000:0b:00.0").
    pub fn find_by_bdf(&self, bdf: &str) -> Option<&PciFunction> {
        self.functions.iter().find(|f| f.bdf == bdf)
    }

    /// Resolve a list of configured PCI devices into concrete functions.
    ///
    /// For each configured device:
    ///   * If present in inventory → returned.
    ///   * If missing and `required = true` → error.
    ///   * If missing and `required = false` → silently skipped.
    pub fn resolve_configured(&self, cfgs: &[PciDeviceConfig]) -> Result<Vec<&PciFunction>> {
        let mut out = Vec::new();

        for dev in cfgs {
            match self.find_by_bdf(&dev.pci_address) {
                Some(func) => {
                    debug!(
                        bdf = %dev.pci_address,
                        vendor = format_args!("0x{:04x}", func.vendor_id),
                        device = format_args!("0x{:04x}", func.device_id),
                        class = format_args!("0x{:06x}", func.class),
                        "resolved configured PCI device"
                    );
                    out.push(func);
                }
                None if dev.required => {
                    return Err(ChalybsError::Vfio(format!(
                        "required PCI device {} not present in host inventory",
                        dev.pci_address
                    )));
                }
                None => {
                    info!(
                        bdf = %dev.pci_address,
                        "optional PCI device not present; skipping"
                    );
                }
            }
        }

        Ok(out)
    }

    /// Return true if the given BDF appears to be part of a "GPU complex"
    /// at the slot level: at least one function in the same
    /// (domain,bus,slot) triple is a display controller.
    ///
    /// **Important**: this is defined only for devices that actually
    /// exist in this inventory snapshot. A synthetic or unknown BDF
    /// with a matching slot does *not* count.
    pub fn is_bdf_in_gpu_complex(&self, bdf: &str) -> bool {
        let key = match parse_bdf_slot(bdf) {
            Some(k) => k,
            None => return false,
        };

        // First, require that this BDF is present in the inventory at all.
        if !self.functions.iter().any(|f| f.bdf == bdf) {
            return false;
        }

        // Then, check whether any function in the same slot is a display
        // controller (GPU). This includes the BDF itself when it is
        // the GPU function.
        for func in &self.functions {
            if let Some(fkey) = func.slot_key() {
                if fkey == key && func.is_display_controller() {
                    return true;
                }
            }
        }

        false
    }

    /// Phase 3: simulate unbind safety for each GPU discovered in the
    /// inventory (read-only, no sysfs writes).
    pub fn assess_gpu_unbind_safety(&self) -> Vec<GpuUnbindAssessment> {
        let groups = self.by_iommu_group();
        let mut out = Vec::new();

        for gpu in self.gpus() {
            let safety = gpu.gpu_safety_class().unwrap_or(GpuSafetyClass::Unknown);
            let group_id = gpu.iommu_group;

            // Collect group member BDFs (excluding this GPU) for logging.
            let mut member_bdfs: Vec<String> = Vec::new();
            if let Some(gid) = group_id {
                if let Some(members) = groups.get(&gid) {
                    for m in members {
                        if m.bdf != gpu.bdf {
                            member_bdfs.push(m.bdf.clone());
                        }
                    }
                }
            }

            // Option A semantics:
            // If this GPU is already bound to vfio-pci, we treat it as
            // operator-assigned for passthrough and do *not* attempt any
            // unbind safety heuristics. From Chalybs' point of view this
            // is categorically safe.
            let feasibility = if matches!(gpu.driver.as_deref(), Some("vfio-pci")) {
                GpuUnbindFeasibility::Safe
            } else {
                match group_id {
                    None => GpuUnbindFeasibility::Unsafe(
                        "GPU has no IOMMU group; cannot safely isolate for passthrough".to_string(),
                    ),
                    Some(gid) => match groups.get(&gid) {
                        None => GpuUnbindFeasibility::Risky(
                            "IOMMU group not found in inventory; treating as risky".to_string(),
                        ),
                        Some(members) => evaluate_gpu_unbind_feasibility(gpu, &safety, members),
                    },
                }
            };

            out.push(GpuUnbindAssessment {
                bdf: gpu.bdf.clone(),
                current_driver: gpu.driver.clone(),
                safety_class: safety,
                iommu_group: group_id,
                group_members: member_bdfs,
                feasibility,
            });
        }

        out
    }
}

/// Decide how safe it looks to unbind `gpu`, given its safety class and
/// all devices in its IOMMU group.
///
/// NOTE: vfio-pci–bound GPUs are handled earlier in
/// `assess_gpu_unbind_safety()` and never reach this function under
/// Option A semantics.
fn evaluate_gpu_unbind_feasibility(
    gpu: &PciFunction,
    safety: &GpuSafetyClass,
    group_members: &[&PciFunction],
) -> GpuUnbindFeasibility {
    match safety {
        GpuSafetyClass::VfioReady => {
            // If the group contains only this GPU and other vfio-bound or
            // unbound GPUs, we lean "Safe". Any non-GPU or host-owned device
            // in the group downgrades the assessment to Risky.
            let mut problematic: Vec<String> = Vec::new();

            for member in group_members {
                if member.bdf == gpu.bdf {
                    continue;
                }

                if !member.is_display_controller() {
                    problematic.push(member.bdf.clone());
                    continue;
                }

                match member.gpu_driver_kind() {
                    Some(GpuDriverKind::Vfio) | Some(GpuDriverKind::Unbound) | None => {
                        // acceptable
                    }
                    Some(_) => {
                        problematic.push(member.bdf.clone());
                    }
                }
            }

            if problematic.is_empty() {
                GpuUnbindFeasibility::Safe
            } else {
                GpuUnbindFeasibility::Risky(format!(
                    "GPU shares IOMMU group with other non-vfio or non-GPU devices: {}",
                    problematic.join(", ")
                ))
            }
        }

        GpuSafetyClass::HostOwned => GpuUnbindFeasibility::Risky(
            "GPU appears host-owned (bound to graphics driver); unbinding may disrupt host display"
                .to_string(),
        ),

        GpuSafetyClass::Unknown => GpuUnbindFeasibility::Risky(
            "GPU driver state is unknown or unbound; operator review required before unbinding"
                .to_string(),
        ),
    }
}

/// Internal helper: read a single PCI function directory.
fn read_function(dir: &Path, bdf: &str) -> Result<PciFunction> {
    let vendor_id = read_hex_u16(&dir.join("vendor"))?;
    let device_id = read_hex_u16(&dir.join("device"))?;
    let class = read_hex_u32(&dir.join("class"))?;

    let driver = read_driver_name(dir);
    let iommu_group = read_iommu_group(dir);
    let numa_node = read_numa_node(dir);

    Ok(PciFunction {
        bdf: bdf.to_string(),
        vendor_id,
        device_id,
        class,
        driver,
        iommu_group,
        numa_node,
    })
}

fn read_hex_u16(path: &Path) -> Result<u16> {
    let raw = fs::read_to_string(path)?;
    let s = raw.trim();
    let without_prefix = s.strip_prefix("0x").unwrap_or(s);
    u16::from_str_radix(without_prefix, 16).map_err(|e| {
        ChalybsError::Vfio(format!(
            "failed to parse {} as hex u16 from {}: {e}",
            s,
            path.display()
        ))
    })
}

fn read_hex_u32(path: &Path) -> Result<u32> {
    let raw = fs::read_to_string(path)?;
    let s = raw.trim();
    let without_prefix = s.strip_prefix("0x").unwrap_or(s);
    u32::from_str_radix(without_prefix, 16).map_err(|e| {
        ChalybsError::Vfio(format!(
            "failed to parse {} as hex u32 from {}: {e}",
            s,
            path.display()
        ))
    })
}

fn read_driver_name(dir: &Path) -> Option<String> {
    let driver_path = dir.join("driver");
    let link = fs::read_link(&driver_path).ok()?;

    link.file_name()
        .and_then(|os| os.to_str())
        .map(|s| s.to_string())
}

fn read_iommu_group(dir: &Path) -> Option<u32> {
    let grp_path = dir.join("iommu_group");
    let link = fs::read_link(&grp_path).ok()?;
    let name = link.file_name()?.to_str()?;
    name.parse::<u32>().ok()
}

fn read_numa_node(dir: &Path) -> Option<i32> {
    let path = dir.join("numa_node");
    let raw = fs::read_to_string(&path).ok()?;
    let s = raw.trim();
    s.parse::<i32>().ok()
}

/// Parse a BDF string "0000:bb:dd.f" into a (domain,bus,slot) key.
/// Returns None if the BDF is malformed.
fn parse_bdf_slot(bdf: &str) -> Option<(u16, u8, u8)> {
    // Expected form: "dddd:bb:dd.f"
    let mut parts = bdf.split(':');
    let domain_str = parts.next()?;
    let bus_str = parts.next()?;
    let devfunc_str = parts.next()?;
    if parts.next().is_some() {
        return None;
    }

    let mut devfunc_parts = devfunc_str.split('.');
    let dev_str = devfunc_parts.next()?;
    let func_str = devfunc_parts.next()?;
    if devfunc_parts.next().is_some() {
        return None;
    }

    // Enforce strict widths to avoid accepting malformed BDFs like "00:01:23.1".
    if domain_str.len() != 4 || bus_str.len() != 2 || dev_str.len() != 2 || func_str.is_empty() {
        return None;
    }

    let domain = u16::from_str_radix(domain_str, 16).ok()?;
    let bus = u8::from_str_radix(bus_str, 16).ok()?;
    let dev = u8::from_str_radix(dev_str, 16).ok()?;

    Some((domain, bus, dev))
}

/// Request Linux to rescan the PCI bus.
///
/// This is typically only needed after hot-plug operations or when we
/// expect new functions to appear. For Chalybs we keep it as a small
/// utility and call it explicitly when necessary.
pub fn rescan_pci_bus() -> Result<()> {
    fs::write("/sys/bus/pci/rescan", "1\n").map_err(ChalybsError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_gpu(driver: Option<&str>, iommu_group: Option<u32>) -> PciFunction {
        PciFunction {
            bdf: "0000:01:00.0".to_string(),
            vendor_id: 0x1234,
            device_id: 0x5678,
            class: 0x030000, // display controller
            driver: driver.map(|d| d.to_string()),
            iommu_group,
            numa_node: Some(0),
        }
    }

    fn make_non_gpu() -> PciFunction {
        PciFunction {
            bdf: "0000:02:00.0".to_string(),
            vendor_id: 0x1234,
            device_id: 0x5678,
            class: 0x020000, // network controller
            driver: Some("e1000e".to_string()),
            iommu_group: Some(2),
            numa_node: Some(0),
        }
    }

    #[test]
    fn gpu_driver_kind_is_none_for_non_gpu() {
        let nic = make_non_gpu();
        assert!(nic.gpu_driver_kind().is_none());
        assert!(nic.gpu_safety_class().is_none());
    }

    #[test]
    fn gpu_driver_kind_classification_basic() {
        let vfio = make_gpu(Some("vfio-pci"), Some(1));
        assert!(matches!(vfio.gpu_driver_kind(), Some(GpuDriverKind::Vfio)));

        let amd = make_gpu(Some("amdgpu"), Some(1));
        assert!(matches!(amd.gpu_driver_kind(), Some(GpuDriverKind::AmdGpu)));

        let radeon = make_gpu(Some("radeon"), Some(1));
        assert!(matches!(
            radeon.gpu_driver_kind(),
            Some(GpuDriverKind::AmdGpu)
        ));

        let nvidia = make_gpu(Some("nvidia"), Some(1));
        assert!(matches!(
            nvidia.gpu_driver_kind(),
            Some(GpuDriverKind::Nvidia)
        ));

        let nouveau = make_gpu(Some("nouveau"), Some(1));
        assert!(matches!(
            nouveau.gpu_driver_kind(),
            Some(GpuDriverKind::Nouveau)
        ));

        let other = make_gpu(Some("weirdgpu"), Some(1));
        match other.gpu_driver_kind() {
            Some(GpuDriverKind::OtherKernel(name)) => assert_eq!(name, "weirdgpu"),
            _ => panic!("expected OtherKernel"),
        }

        let unbound = make_gpu(None, Some(1));
        assert!(matches!(
            unbound.gpu_driver_kind(),
            Some(GpuDriverKind::Unbound)
        ));
    }

    #[test]
    fn gpu_safety_classification_basic() {
        let vfio = make_gpu(Some("vfio-pci"), Some(1));
        assert!(matches!(
            vfio.gpu_safety_class(),
            Some(GpuSafetyClass::VfioReady)
        ));

        let host_owned = make_gpu(Some("amdgpu"), Some(1));
        assert!(matches!(
            host_owned.gpu_safety_class(),
            Some(GpuSafetyClass::HostOwned)
        ));

        let unknown = make_gpu(Some("weirdgpu"), Some(1));
        assert!(matches!(
            unknown.gpu_safety_class(),
            Some(GpuSafetyClass::Unknown)
        ));

        let unbound = make_gpu(None, Some(1));
        assert!(matches!(
            unbound.gpu_safety_class(),
            Some(GpuSafetyClass::Unknown)
        ));
    }

    #[test]
    fn gpu_unbind_assessment_vfio_without_iommu_group_is_safe() {
        // vfio-pci bound GPU with no IOMMU group is treated as Safe,
        // because we never intend to unbind it (Option A semantics).
        let gpu = make_gpu(Some("vfio-pci"), None);

        let inv = PciInventory {
            functions: vec![gpu],
        };

        let assessments = inv.assess_gpu_unbind_safety();
        assert_eq!(assessments.len(), 1);
        let a = &assessments[0];

        assert!(matches!(a.feasibility, GpuUnbindFeasibility::Safe));
    }

    #[test]
    fn gpu_unbind_assessment_non_vfio_no_iommu_group_is_unsafe() {
        // Non-vfio GPU with no IOMMU group remains Unsafe.
        let gpu = make_gpu(Some("nvidia"), None);

        let inv = PciInventory {
            functions: vec![gpu],
        };

        let assessments = inv.assess_gpu_unbind_safety();
        assert_eq!(assessments.len(), 1);
        let a = &assessments[0];

        assert!(matches!(a.feasibility, GpuUnbindFeasibility::Unsafe(_)));
    }

    #[test]
    fn bind_to_vfio_is_noop_if_already_bound() {
        let gpu = make_gpu(Some("vfio-pci"), Some(1));
        // This must not touch /sys because we early-return.
        gpu.bind_to_vfio_pci().unwrap();
    }

    #[test]
    fn is_bdf_in_gpu_complex_identifies_slot_mates() {
        let gpu = PciFunction {
            bdf: "0000:4a:00.0".to_string(),
            vendor_id: 0x1234,
            device_id: 0x5678,
            class: 0x030000, // display controller
            driver: Some("nvidia".to_string()),
            iommu_group: Some(1),
            numa_node: Some(0),
        };

        let audio = PciFunction {
            bdf: "0000:4a:00.1".to_string(),
            vendor_id: 0x1234,
            device_id: 0x8765,
            class: 0x040300, // typical HDA audio
            driver: Some("snd_hda_intel".to_string()),
            iommu_group: Some(1),
            numa_node: Some(0),
        };

        let inv = PciInventory {
            functions: vec![gpu, audio],
        };

        assert!(inv.is_bdf_in_gpu_complex("0000:4a:00.0"));
        assert!(inv.is_bdf_in_gpu_complex("0000:4a:00.1"));
        assert!(!inv.is_bdf_in_gpu_complex("0000:4a:00.2"));
    }
}
