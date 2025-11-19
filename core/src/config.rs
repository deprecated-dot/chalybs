use serde::Deserialize;
use crate::errors::{Result, ChalybsError};
use std::collections::HashMap;

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
    pub host_cpus: String, // "0-7,32-39"
    pub vm_cpus: String,   // "8-15,40-47"
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

fn default_required() -> bool { true }

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
