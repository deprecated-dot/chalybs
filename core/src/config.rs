use serde::Deserialize;
use std::collections::HashMap;

use crate::errors::{ChalybsError, Result};

#[derive(Debug, Deserialize, Clone)]
pub struct RootConfig {
    pub vm: HashMap<String, VmConfig>,
    pub logging: Option<LoggingConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LoggingConfig {
    pub level: Option<String>,
    pub format: Option<String>, // "pretty" | "json"
}

#[derive(Debug, Deserialize, Clone)]
pub struct VmConfig {
    pub cpu: CpuConfig,
    pub qemu: QemuConfig,

    #[serde(default)]
    pub numa: Option<NumaConfig>,

    #[serde(default)]
    pub devices: DevicesConfig,

    /// GPU policy (single-GPU safety, iGPU usage, etc.)
    #[serde(default)]
    pub gpu: GpuPolicyConfig,

    #[serde(default)]
    pub peripherals: Option<PeripheralConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CpuConfig {
    /// Host CPU set for the VM’s vCPUs, e.g. "8-15,40-47"
    pub host_cpus: String,

    /// vCPU indices exposed to the guest, e.g. "0-7"
    pub vm_cpus: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct QemuConfig {
    /// QEMU binary path (e.g. /usr/bin/qemu-system-x86_64)
    pub binary: String,

    /// Extra arguments passed verbatim to QEMU
    pub args: String,

    /// Number of vCPUs to expose to the guest
    pub num_vcpus: u32,

    /// Guest memory in MiB
    pub mem_mb: u64,

    /// Whether hugepages should be used
    pub hugepages: bool,

    /// OVMF CODE image path
    pub ovmf_code: String,

    /// OVMF VARS image path
    pub ovmf_vars: String,
}

/// NUMA policy (optional)
#[derive(Debug, Deserialize, Clone, Default)]
pub struct NumaConfig {
    /// If set, prefer placing vCPUs / IRQs on this NUMA node
    pub node: Option<u16>,
}

/// Device configuration: GPU, NVMe, NIC, USB, etc.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct DevicesConfig {
    pub gpu: Option<Vec<PciDeviceConfig>>,
    pub nvme: Option<Vec<PciDeviceConfig>>,
    pub nic: Option<Vec<PciDeviceConfig>>,
    pub usb: Option<Vec<PciDeviceConfig>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PciDeviceConfig {
    /// Full PCI address, e.g. "0000:0b:00.0"
    pub pci_address: String,

    /// If true (default), VM startup fails if this device is missing
    #[serde(default = "default_required")]
    pub required: bool,
}

fn default_required() -> bool {
    true
}

/// GPU policy controls single-GPU safety and iGPU usage.
///
/// By default, **Chalybs will not** pass through a GPU on a
/// single-GPU host unless `allow_single_gpu = true` is explicitly set.
/// iGPUs are ignored unless you explicitly wire them into `devices.gpu`
/// and/or a future `force_use_igpu` toggle.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct GpuPolicyConfig {
    /// Allow passthrough when the host has **exactly one GPU**.
    ///
    /// If false (default) and the host has only one GPU,
    /// GPU passthrough preflight fails with a clear error.
    #[serde(default)]
    pub allow_single_gpu: bool,

    /// Placeholder for future behavior like “use iGPU instead of dGPU”.
    /// Currently unused; kept for forward-compatible config.
    #[serde(default)]
    pub force_use_igpu: bool,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct PeripheralConfig {
    pub tasmota: Option<TasmotaConfig>,
    pub ddc: Option<DdcConfig>,
    pub looking_glass: Option<LookingGlassConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TasmotaConfig {
    pub url: String,
    pub on_command: String,
    pub off_command: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DdcConfig {
    pub monitor_i2c_bus: u8,
    pub vm_input: u8,
    pub host_input: u8,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LookingGlassConfig {
    pub shm_name: String,
}

/// ---------------------------------------------------------------------------
/// PCI policy module (GPU safety, single-GPU protection, etc.)
///
/// This consumes the PCI inventory defined in `core/src/pci.rs`.
/// Inventory = "what exists".
/// Policy    = "what is allowed".
///
/// This module is intentionally kept here (not in pci.rs) so that
/// configuration policy remains separate from raw hardware scanning.
/// ---------------------------------------------------------------------------
pub mod pci {
    use super::{GpuPolicyConfig, VmConfig};
    use crate::errors::{ChalybsError, Result};
    use tracing::{info, warn};

    use crate::pci::PciInventory;

    /// GPU passthrough preflight policy:
    ///
    /// Rules:
    /// * If the VM has no configured GPU devices → nothing to enforce.
    /// * If host has 0 GPUs → error if VM requested one.
    /// * If host has 1 GPU → require allow_single_gpu = true.
    /// * If host has >=2 GPUs → always allowed.
    ///
    /// iGPUs count as "GPU candidates" only for *counting*, not for
    /// passthrough unless explicitly listed in devices.gpu[].
    pub fn preflight_gpu_policy(vm_name: &str, cfg: &VmConfig) -> Result<()> {
        // How many devices does the VM want to passthrough?
        let vm_gpu_count = cfg.devices.gpu.as_ref().map(|v| v.len()).unwrap_or(0);

        if vm_gpu_count == 0 {
            info!(
                vm = vm_name,
                "no GPU passthrough requested; skipping GPU policy preflight"
            );
            return Ok(());
        }

        // Build host PCI inventory.
        let inv = PciInventory::scan()?;
        let host_gpu_count = inv.count_display_controllers();

        if host_gpu_count == 0 {
            return Err(ChalybsError::Vfio(format!(
                "VM {vm_name}: GPU passthrough requested, \
                 but host has NO GPUs detected via PCI inventory."
            )));
        }

        let policy: &GpuPolicyConfig = &cfg.gpu;

        info!(
            vm = vm_name,
            vm_gpu_devices = vm_gpu_count,
            host_gpus = host_gpu_count,
            allow_single_gpu = policy.allow_single_gpu,
            "GPU policy preflight"
        );

        if host_gpu_count == 1 && !policy.allow_single_gpu {
            return Err(ChalybsError::Vfio(format!(
                "VM {vm_name}: GPU passthrough on a SINGLE-GPU host is blocked.\n\
                 To permit this, explicitly set:\n\
                 [vm.{vm_name}.gpu]\n\
                 allow_single_gpu = true\n\
                 \n\
                 WARNING: this may steal the host display.\n"
            )));
        }

        // Future: we can insert iGPU-specific handling or IOMMU grouping checks.

        info!(vm = vm_name, "GPU policy satisfied; continuing startup");
        Ok(())
    }
}

impl RootConfig {
    pub fn from_path(path: &std::path::Path) -> Result<Self> {
        let data = std::fs::read_to_string(path)?;
        let cfg: RootConfig = toml::from_str(&data)?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        if self.vm.is_empty() {
            return Err(ChalybsError::Config("no VMs defined".into()));
        }

        for (name, vm) in &self.vm {
            if vm.qemu.num_vcpus == 0 {
                return Err(ChalybsError::Config(format!(
                    "vm {name}: num_vcpus must be > 0"
                )));
            }
            if vm.cpu.vm_cpus.is_empty() {
                return Err(ChalybsError::Config(format!(
                    "vm {name}: vm_cpus must not be empty"
                )));
            }
        }

        Ok(())
    }
}
