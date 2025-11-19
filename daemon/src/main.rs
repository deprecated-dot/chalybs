use anyhow::Result;
use clap::Parser;
use chalybs_core::logging::{init_logging, LogFormat};

#[derive(Parser, Debug)]
#[command(name = "chalybsd", about = "Chalybs launch daemon (stub)")]
struct DaemonCli {
    /// Log format: pretty | json
    #[arg(short, long, default_value = "pretty")]
    log_format: String,

    /// Log level: trace | debug | info | warn | error
    #[arg(short, long, default_value = "info")]
    log_level: String,
}

fn main() -> Result<()> {
    let cli = DaemonCli::parse();

    let format = match cli.log_format.as_str() {
        "json" => LogFormat::Json,
        _ => LogFormat::Pretty,
    };

    init_logging(format, &cli.log_level);

    tracing::info!("chalybsd daemon stub starting (no IPC implemented yet)");

    // Placeholder: later add Unix socket, event loop, etc.
    Ok(())
}
