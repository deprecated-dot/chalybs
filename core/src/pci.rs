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
}

/// Collection of all discovered PCI functions on the host.
#[derive(Debug, Clone)]
pub struct PciInventory {
    pub functions: Vec<PciFunction>,
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

    /// Return all functions that look like network controllers.
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
    pub fn resolve_configured(
        &self,
        cfgs: &[PciDeviceConfig],
    ) -> Result<Vec<&PciFunction>> {
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

/// Request Linux to rescan the PCI bus.
///
/// This is typically only needed after hot-plug operations or when we
/// expect new functions to appear. For Chalybs we keep it as a small
/// utility and call it explicitly when necessary.
pub fn rescan_pci_bus() -> Result<()> {
    fs::write("/sys/bus/pci/rescan", "1\n").map_err(ChalybsError::Io)
}
