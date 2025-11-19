use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};

use crate::errors::{ChalybsError, Result};
use crate::util::parse_cpu_list;

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

    #[serde(default)]
    pub peripherals: Option<PeripheralConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CpuConfig {
    /// CPUs reserved for the VM (e.g. "8-15,40-47")
    pub vm_cpus: String,

    /// Optional explicit host CPUs. If omitted, Chalybs derives them.
    #[serde(default)]
    pub host_cpus: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct QemuConfig {
    pub binary: String,
    pub args: String,
    pub num_vcpus: u32,
    pub mem_mb: u64,
    pub hugepages: bool,
    pub ovmf_code: String,
    pub ovmf_vars: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct NumaConfig {
    pub node: Option<u16>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct DevicesConfig {
    pub gpu: Option<Vec<PciDeviceConfig>>,
    pub nvme: Option<Vec<PciDeviceConfig>>,
    pub nic: Option<Vec<PciDeviceConfig>>,
    pub usb: Option<Vec<PciDeviceConfig>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PciDeviceConfig {
    pub pci_address: String,
    #[serde(default = "default_required")]
    pub required: bool,
}

fn default_required() -> bool {
    true
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

        let mut global_vm_cpus = BTreeSet::<u32>::new();

        for (name, vm) in &self.vm {
            if vm.qemu.num_vcpus == 0 {
                return Err(ChalybsError::Config(format!(
                    "vm {name}: num_vcpus must be > 0"
                )));
            }

            // Parse VM CPU list
            let vm_cpus = parse_cpu_list(&vm.cpu.vm_cpus).map_err(|e| {
                ChalybsError::Config(format!(
                    "vm {name}: invalid vm_cpus '{}': {e}",
                    vm.cpu.vm_cpus
                ))
            })?;

            if vm_cpus.is_empty() {
                return Err(ChalybsError::Config(format!(
                    "vm {name}: vm_cpus must not be empty"
                )));
            }

            // FIXED HERE: parentheses for comparison
            if (vm_cpus.len() as u32) < vm.qemu.num_vcpus {
                return Err(ChalybsError::Config(format!(
                    "vm {name}: vm_cpus ({} CPUs) has fewer CPUs than num_vcpus ({})",
                    vm_cpus.len(),
                    vm.qemu.num_vcpus
                )));
            }

            // Optional host CPU validation
            if let Some(ref host_str) = vm.cpu.host_cpus {
                let host_cpus = parse_cpu_list(host_str).map_err(|e| {
                    ChalybsError::Config(format!(
                        "vm {name}: invalid host_cpus '{}': {e}",
                        host_str
                    ))
                })?;

                let vm_set: BTreeSet<u32> = vm_cpus.iter().copied().collect();
                for c in &host_cpus {
                    if vm_set.contains(c) {
                        return Err(ChalybsError::Config(format!(
                            "vm {name}: host_cpus and vm_cpus overlap on CPU {}",
                            c
                        )));
                    }
                }
            }

            // Global overlap check for vm_cpus
            for c in vm_cpus {
                if !global_vm_cpus.insert(c) {
                    return Err(ChalybsError::Config(format!(
                        "CPU {c} is used by more than one VM (including vm {name})"
                    )));
                }
            }
        }

        Ok(())
    }
}
