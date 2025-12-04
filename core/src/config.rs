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

    /// Device isolation policy (Phase 8 / Phase 9).
    ///
    /// This controls whether Chalybs enforces IOMMU-group based
    /// isolation for all passthrough devices (GPU, NVMe, NIC, USB),
    /// and how per-device IsolationLevel values are interpreted.
    ///
    /// By default, isolation is **disabled** so existing configs
    /// continue to behave as before. Operators may opt into `Audit`
    /// or `Enforce` modes on a per-VM basis.
    #[serde(default)]
    pub isolation: IsolationPolicyConfig,

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

/// QEMU configuration and CPU/SMBIOS extras.
#[derive(Debug, Deserialize, Clone)]
pub struct QemuConfig {
    /// QEMU binary path (e.g. /usr/bin/qemu-system-x86_64)
    pub binary: String,

    /// Extra arguments injected **before** Chalybs-managed core args.
    ///
    /// Example:
    ///   pre_args = "-name win11-gpu,debug-threads=on -nodefaults"
    ///
    /// These are split on whitespace and appended verbatim.
    #[serde(default)]
    pub pre_args: Option<String>,

    /// Extra arguments passed verbatim to QEMU (mid-section).
    ///
    /// Historically this was the only "args" field; it is preserved
    /// for backwards compatibility. These are added *after* the core
    /// Chalybs-managed args, but before `post_args`.
    pub args: String,

    /// Extra arguments injected **after** everything else.
    ///
    /// Example:
    ///   post_args = "-boot menu=on -net none -global kvm-pit.lost_tick_policy=discard"
    ///
    /// These are split on whitespace and appended verbatim at the end.
    #[serde(default)]
    pub post_args: Option<String>,

    /// Optional RTC policy for QEMU.
    ///
    /// If:
    ///   - `rtc` is **Some(non-empty)**  → emit `-rtc <value>` verbatim.
    ///   - `rtc` is **Some(empty string)** → emit **no** `-rtc` flag at all
    ///       (caller accepts QEMU’s default, which is UTC in most builds).
    ///   - `rtc` is **None** → Chalybs emits:
    ///       `-rtc base=localtime,driftfix=slew`
    ///
    /// This mirrors your legacy Bash suite’s default unless explicitly
    /// overridden in TOML.
    #[serde(default)]
    pub rtc: Option<String>,

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

    /// Optional SMBIOS configuration (type 0/1/2).
    #[serde(default)]
    pub smbios: Option<SmbiosConfig>,

    /// Optional base CPU model string for QEMU -cpu.
    ///
    /// When set:
    ///   - "auto" (case-insensitive) → request autodetection based on
    ///     /proc/cpuinfo (currently AMD Zen+ mapping to "EPYC-v2").
    ///   - any other non-empty string → used verbatim as the base model
    ///     (e.g. "EPYC-v2", "Skylake-Server", etc.).
    ///
    /// When unset, the base model falls back to `cpu_extras.abi` if present
    /// or "host" as a final default.
    #[serde(default)]
    pub cpu_model: Option<String>,

    /// Optional CPU model / feature / hypervisor extras.
    ///
    /// This is where "ABI / TOPO / HV_CONTEXTS / VENDOR_ID" from the
    /// legacy Bash suite live.
    #[serde(default)]
    pub cpu_extras: Option<CpuExtrasConfig>,

    /// Optional list of PCI devices whose option ROM BAR should be disabled.
    ///
    /// Entries must be full PCI BDFs, e.g. "0000:49:00.0". When a device
    /// in this list is attached via vfio-pci, Chalybs adds `rombar=0` to the
    /// corresponding `-device` argument.
    #[serde(default)]
    pub rombar_off: Option<Vec<String>>,

    /// Optional mapping from passthrough PCI devices to deterministic
    /// root-port addresses on the Q35 root complex.
    ///
    /// Keys are full PCI BDFs, e.g. "0000:49:00.0".
    /// Values are hex slot addresses of the form "0xNN" where NN ∈ [00,1f].
    ///
    /// When present, these entries *override* the automatic allocator for the
    /// corresponding devices. All other passthrough devices are assigned
    /// stable, deterministic slots by Chalybs according to a fixed priority
    /// and BDF ordering.
    #[serde(default)]
    pub pci_rootport: Option<HashMap<String, String>>,

    /// If true, the first configured GPU for this VM is treated as a legacy
    /// VGA device and Chalybs will add `x-vga=on,multifunction=on` to its
    /// vfio-pci `-device` argument.
    ///
    /// This is intended only for truly legacy, pre-UEFI/GOP GPUs. For modern
    /// GPUs (Ampere, RDNA2, etc.) this should remain false so they behave as
    /// pure PCIe devices without legacy VGA routing or extra enumeration.
    #[serde(default)]
    pub legacy_primary_gpu: bool,
}

/// NUMA policy (optional)
#[derive(Debug, Deserialize, Clone, Default)]
pub struct NumaConfig {
    /// Preferred NUMA node for vCPUs / IRQs.
    ///
    /// This is the legacy "NODE=2" semantic from your Bash suite.
    /// If this is set, Chalybs will:
    ///   - Bias IRQ pinning to this node
    ///   - Treat it as the default hugepage node if `hugepage_node`
    ///     is not explicitly set.
    #[serde(default)]
    pub node: Option<u16>,

    /// Optional override for hugepage-backed RAM placement.
    ///
    /// If set, hugepages for guest RAM are provisioned on this node
    /// even if CPU/IRQ affinity uses a different node.
    ///
    /// If None, we fall back to:
    ///   - `node` above if set
    ///   - otherwise, detect the node from the host CPU set.
    #[serde(default)]
    pub hugepage_node: Option<u16>,
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

    /// Optional per-device isolation level override.
    ///
    /// If omitted, the VM’s `isolation.default_level` applies. When
    /// present, this value participates in the Phase 9 isolation-level
    /// evaluation and can upgrade or relax the default policy for this
    /// specific device.
    #[serde(default)]
    pub level: Option<IsolationLevel>,
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

/// Isolation mode for device safety (Phase 8 / Phase 9).
///
/// - Disabled → no additional checks; behavior is identical to
///   earlier versions of Chalybs.
/// - Audit    → evaluate isolation and isolation levels, log findings,
///   but do not block startup.
/// - Enforce  → treat any isolation violation as a hard error and
///   abort VFIO staging before touching sysfs.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IsolationMode {
    Disabled,
    Audit,
    Enforce,
}

/// Desired isolation level for passthrough devices.
///
/// In Phase 9 this becomes behavior-driving:
///
/// - Dedicated      → device expects exclusive IOMMU grouping with
///   other Dedicated devices for this VM only.
/// - SharedWithHost → device may share an IOMMU group with host-owned
///   devices, subject to other policy checks.
/// - Forbidden      → device must not be passed through; any attempt
///   to do so is a policy violation.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum IsolationLevel {
    Dedicated,
    SharedWithHost,
    Forbidden,
}

/// Per-VM device isolation policy (Phase 8 / Phase 9).
///
/// This is intentionally conservative by default (Disabled mode) so
/// existing VM configurations behave exactly as before. Operators can
/// opt into Audit or Enforce for stricter guarantees around IOMMU
/// isolation and host-critical device sharing.
///
/// Phase 9 uses `default_level` as the baseline for each device,
/// allowing per-device overrides via `PciDeviceConfig.level`.
#[derive(Debug, Deserialize, Clone, Copy)]
pub struct IsolationPolicyConfig {
    /// Overall isolation mode for this VM.
    ///
    /// Default: Disabled (no behavior change vs pre-Phase-8).
    #[serde(default = "default_isolation_mode")]
    pub mode: IsolationMode,

    /// Default isolation level for passthrough devices that do not
    /// have an explicit override in `PciDeviceConfig.level`.
    #[serde(default = "default_isolation_level")]
    pub default_level: IsolationLevel,

    /// If true (default), any IOMMU group that contains at least one
    /// passthrough device must not contain non-passthrough members.
    ///
    /// Violations are either logged (Audit) or treated as hard errors
    /// (Enforce).
    #[serde(default = "default_true")]
    pub require_iommu_exclusive: bool,

    /// If true (default), multi-function PCI devices (same domain/bus/
    /// slot, different function) are expected to be treated as a unit.
    /// If some functions are passed through while others remain on the
    /// host, an isolation finding is emitted.
    #[serde(default = "default_true")]
    pub require_multifunction_consistency: bool,

    /// If true (default), passthrough from an IOMMU group that also
    /// contains a host-owned GPU (e.g. amdgpu/nvidia/nouveau) is
    /// treated as a violation.
    #[serde(default = "default_true")]
    pub forbid_host_critical_in_group: bool,
}

fn default_isolation_mode() -> IsolationMode {
    IsolationMode::Disabled
}

fn default_isolation_level() -> IsolationLevel {
    IsolationLevel::Dedicated
}

fn default_true() -> bool {
    true
}

impl Default for IsolationPolicyConfig {
    fn default() -> Self {
        Self {
            mode: default_isolation_mode(),
            default_level: default_isolation_level(),
            require_iommu_exclusive: default_true(),
            require_multifunction_consistency: default_true(),
            forbid_host_critical_in_group: default_true(),
        }
    }
}

/// SMBIOS configuration mapped to QEMU -smbios type 0/1/2.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct SmbiosConfig {
    /// type=0
    #[serde(default)]
    pub bios_vendor: Option<String>,
    #[serde(default)]
    pub bios_version: Option<String>,
    #[serde(default)]
    pub bios_date: Option<String>,

    /// type=1
    #[serde(default)]
    pub system_manufacturer: Option<String>,
    #[serde(default)]
    pub system_product_name: Option<String>,
    #[serde(default)]
    pub system_uuid: Option<String>,

    /// type=2
    #[serde(default)]
    pub baseboard_manufacturer: Option<String>,
    #[serde(default)]
    pub baseboard_product: Option<String>,
}

/// CPU model / feature / hypervisor extras for QEMU -cpu.
///
/// This is the Rust-side home of the old:
///   ABI / TOPO / HV_CONTEXTS / VENDOR_ID
#[derive(Debug, Deserialize, Clone, Default)]
pub struct CpuExtrasConfig {
    /// CPU model (e.g. "EPYC-v2"). Defaults to "host" if omitted.
    #[serde(default)]
    pub abi: Option<String>,

    /// Feature flags appended as-is, e.g.:
    ///   "pdpe1gb,+hypervisor,+invtsc,+topoext,+kvm"
    #[serde(default)]
    pub topo: Option<String>,

    /// Hypervisor contexts, e.g.:
    ///   "hv-stimer,hv-time,hv-synic,hv-vpindex,hv-avic"
    #[serde(default)]
    pub hv_contexts: Option<String>,

    /// Hypervisor vendor id fragment, e.g.:
    ///   "hv-vendor-id=ASUSTeK"
    #[serde(default)]
    pub vendor_id: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct PeripheralConfig {
    pub tasmota: Option<TasmotaConfig>,
    pub ddc: Option<DdcConfig>,
    pub looking_glass: Option<LookingGlassConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TasmotaConfig {
    /// MQTT broker, e.g. "mqtt://homeassistant.local.arpa:1883"
    pub mqtt_host: String,

    /// Optional username for broker authentication.
    #[serde(default)]
    pub username: Option<String>,

    /// Optional password for broker authentication.
    #[serde(default)]
    pub password: Option<String>,

    /// Tasmota device id used in FullTopic.
    ///
    /// Topic is derived deterministically as:
    ///   "cmnd/<device_id>/POWER"
    ///
    /// Payload is:
    ///   "ON"  on vm_up
    ///   "OFF" on vm_down
    pub device_id: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DdcConfig {
    pub monitor_i2c_bus: u8,
    pub vm_input: u8,
    pub host_input: u8,

    /// If true, DDC errors are treated as fatal VM errors. If false,
    /// they are logged as warnings and ignored.
    #[serde(default)]
    pub fatal_on_error: bool,
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
    use std::collections::HashMap;

    use super::{GpuPolicyConfig, VmConfig};
    use crate::errors::{ChalybsError, Result};
    use tracing::{info, warn};

    use crate::pci::{GpuSafetyClass, GpuUnbindAssessment, GpuUnbindFeasibility, PciInventory};

    /// GPU passthrough preflight policy (Phase 1/2/3):
    ///
    /// Rules:
    /// * If the VM has no configured GPU devices → nothing to enforce.
    /// * If host has 0 GPUs → error if VM requested one.
    /// * If host has 1 GPU → require allow_single_gpu = true.
    /// * If host has >=2 GPUs → always allowed.
    ///
    /// Phase 2/3 also logs driver classification and unbind feasibility,
    /// but does not yet block beyond the single-GPU rules and "no GPUs"
    /// condition.
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

        // Phase 2: GPU driver detection + safety classification (read-only).
        let gpu_summaries = inv.gpu_summaries();
        for s in &gpu_summaries {
            info!(
                bdf = s.bdf.as_str(),
                vendor = format_args!("0x{:04x}", s.vendor_id),
                device = format_args!("0x{:04x}", s.device_id),
                driver = s.driver.as_deref().unwrap_or("<none>"),
                driver_kind = ?s.driver_kind,
                safety = ?s.safety,
                "host GPU classification"
            );
        }

        let host_owned = gpu_summaries
            .iter()
            .filter(|s| matches!(s.safety, Some(GpuSafetyClass::HostOwned)))
            .count();

        if host_owned > 0 {
            warn!(
                vm = vm_name,
                host_owned_gpus = host_owned,
                "GPU passthrough requested while some GPUs are classified as HostOwned \
(bound to host GPU drivers); Phase 2 is detection-only, no policy change yet"
            );
        }

        // Phase 3: unbind safety simulation (still read-only, no sysfs writes).
        let unbind_assessments = inv.assess_gpu_unbind_safety();
        let mut by_bdf: HashMap<&str, &GpuUnbindAssessment> = HashMap::new();
        for a in &unbind_assessments {
            by_bdf.insert(a.bdf.as_str(), a);
        }

        for a in &unbind_assessments {
            let group_members_str = if a.group_members.is_empty() {
                "<none>".to_string()
            } else {
                a.group_members.join(",")
            };

            match &a.feasibility {
                GpuUnbindFeasibility::Safe => {
                    info!(
                        vm = vm_name,
                        bdf = a.bdf.as_str(),
                        driver = a.current_driver.as_deref().unwrap_or("<none>"),
                        safety_class = ?a.safety_class,
                        iommu_group = ?a.iommu_group,
                        group_members = group_members_str.as_str(),
                        "GPU unbind simulation: SAFE"
                    );
                }
                GpuUnbindFeasibility::Risky(reason) => {
                    warn!(
                        vm = vm_name,
                        bdf = a.bdf.as_str(),
                        driver = a.current_driver.as_deref().unwrap_or("<none>"),
                        safety_class = ?a.safety_class,
                        iommu_group = ?a.iommu_group,
                        group_members = group_members_str.as_str(),
                        reason = reason.as_str(),
                        "GPU unbind simulation: RISKY"
                    );
                }
                GpuUnbindFeasibility::Unsafe(reason) => {
                    warn!(
                        vm = vm_name,
                        bdf = a.bdf.as_str(),
                        driver = a.current_driver.as_deref().unwrap_or("<none>"),
                        safety_class = ?a.safety_class,
                        iommu_group = ?a.iommu_group,
                        group_members = group_members_str.as_str(),
                        reason = reason.as_str(),
                        "GPU unbind simulation: UNSAFE"
                    );
                }
            }
        }

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

        // Future: we can insert iGPU-specific handling or more detailed
        // gating from unbind feasibility here. For now, this remains
        // advisory only.

        info!(vm = vm_name, "GPU policy satisfied; continuing startup");
        Ok(())
    }

    /// Phase 4: Prepare PCI for GPU passthrough.
    ///
    /// Called from VmState::PreparePci, *after* preflight_gpu_policy()
    /// has passed. This function:
    ///
    ///   * Resolves configured GPU devices against PCI inventory.
    ///   * Reuses unbind safety assessments for those GPUs.
    ///   * Blocks on "Unsafe" feasibility for configured GPUs.
    ///   * Logs and proceeds on "Risky" per operator judgement.
    ///   * Unbinds current drivers and binds GPUs to vfio-pci.
    ///
    /// This is the first phase that performs sysfs writes.
    pub fn prepare_gpu_passthrough(vm_name: &str, cfg: &VmConfig) -> Result<()> {
        let gpu_cfgs = match cfg.devices.gpu.as_ref() {
            Some(v) if !v.is_empty() => v,
            _ => {
                info!(
                    vm = vm_name,
                    "no GPU devices configured; skipping PCI prepare"
                );
                return Ok(());
            }
        };

        // Build host PCI inventory and resolve configured GPUs.
        let inv = PciInventory::scan()?;
        let resolved = inv.resolve_configured(gpu_cfgs)?;

        // Build a BDF → assessment map from unbind safety simulation.
        let assessments = inv.assess_gpu_unbind_safety();
        let mut by_bdf: HashMap<&str, &GpuUnbindAssessment> = HashMap::new();
        for a in &assessments {
            by_bdf.insert(a.bdf.as_str(), a);
        }

        // Gate based on feasibility for configured GPUs.
        for func in &resolved {
            if !func.is_display_controller() {
                return Err(ChalybsError::Vfio(format!(
                    "VM {vm_name}: device {} in devices.gpu is not a display controller",
                    func.bdf
                )));
            }

            if let Some(a) = by_bdf.get(func.bdf.as_str()) {
                match &a.feasibility {
                    GpuUnbindFeasibility::Unsafe(reason) => {
                        return Err(ChalybsError::Vfio(format!(
                            "VM {vm_name}: GPU {} is classified as UNSAFE to unbind: {reason}",
                            func.bdf
                        )));
                    }
                    GpuUnbindFeasibility::Risky(reason) => {
                        warn!(
                            vm = vm_name,
                            bdf = func.bdf.as_str(),
                            iommu_group = ?a.iommu_group,
                            reason = reason.as_str(),
                            "GPU unbind classified as RISKY for configured GPU; \
                             proceeding per operator configuration"
                        );
                    }
                    GpuUnbindFeasibility::Safe => {
                        info!(
                            vm = vm_name,
                            bdf = func.bdf.as_str(),
                            "GPU unbind classified as SAFE for configured GPU; proceeding"
                        );
                    }
                }
            } else {
                warn!(
                    vm = vm_name,
                    bdf = func.bdf.as_str(),
                    "no unbind assessment found for configured GPU; treating as RISKY and proceeding"
                );
            }
        }

        // Perform unbind + vfio-pci bind for all configured GPUs.
        for func in &resolved {
            info!(
                vm = vm_name,
                bdf = func.bdf.as_str(),
                "unbinding current driver for GPU prior to passthrough"
            );
            func.unbind_current_driver()?;

            info!(
                vm = vm_name,
                bdf = func.bdf.as_str(),
                "binding GPU to vfio-pci for passthrough"
            );
            func.bind_to_vfio_pci()?;
        }

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
