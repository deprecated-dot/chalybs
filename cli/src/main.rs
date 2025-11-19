use std::path::PathBuf;
use clap::{Parser, Subcommand};
use anyhow::Result;

use chalybs_core::{
    config::RootConfig,
    logging::{init_logging, LogFormat},
    model::{VmRuntime, VmCpuLayout, CpuSet},
    state::VmStateMachine,
    util::parse_cpu_list,
    cpuset::cpuset_status,
};

#[derive(Parser, Debug)]
#[command(name = "chalybs", about = "Deterministic VFIO/KVM launcher")]
struct Cli {
    /// Path to configuration file
    #[arg(short = 'c', long, value_name = "FILE", default_value = "/etc/chalybs.toml")]
    config: PathBuf,

    /// Log format: pretty | json
    #[arg(short = 'f', long, default_value = "pretty")]
    log_format: String,

    /// Log level: trace | debug | info | warn | error
    #[arg(short = 'L', long, default_value = "info")]
    log_level: String,

    /// VM name inside the config [vm.<name>]
    #[arg(short = 'n', long)]
    vm: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Bring up a VM
    Up,

    /// Shut down a VM
    Down,

    /// Show VM status
    Status,

    /// Show cpuset bindings for VM + host
    CpusetStatus,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let format = match cli.log_format.as_str() {
        "json" => LogFormat::Json,
        _ => LogFormat::Pretty,
    };

    init_logging(format, &cli.log_level);

    let cfg = RootConfig::from_path(&cli.config)?;
    let vm_name = cli.vm.unwrap_or_else(|| "default".to_string());

    let vm_cfg = cfg.vm.get(&vm_name)
        .ok_or_else(|| anyhow::anyhow!(format!("VM {vm_name} not found in config")))?;
    let vm_cfg = vm_cfg.clone();

    // CPU parsing is now SIMPLE — always Strings.
    let host_cpus = parse_cpu_list(&vm_cfg.cpu.host_cpus)?;
    let vm_cpus   = parse_cpu_list(&vm_cfg.cpu.vm_cpus)?;

    let cpus = VmCpuLayout {
        host: CpuSet { cpus: host_cpus },
        vm:   CpuSet { cpus: vm_cpus },
    };

    // VmRuntime::new automatically resolves:
    // - modes (ModeConfig + HostCapabilities)
    // - caps (NUMA detection)
    let rt = VmRuntime::new(vm_name.clone(), vm_cfg, cpus);
    let mut sm = VmStateMachine::new(rt);

    match cli.command {
        Commands::Up => {
            sm.run_until_steady()?;
        }
        Commands::Down => {
            sm.run_shutdown()?;
        }
        Commands::Status => {
            println!("VM: {}", vm_name);
            println!("State reporting coming via daemon (not yet implemented)");
        }
        Commands::CpusetStatus => {
            let text = cpuset_status(&vm_name)?;
            println!("{text}");
        }
    }

    Ok(())
}
