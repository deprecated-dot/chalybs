use anyhow::Result;
use chalybs_core::logging::{init_logging, LogFormat};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "chalybsd",
    about = "Chalybs launch daemon (multi-client IPC server)"
)]
struct DaemonCli {
    /// Log format: pretty | json
    #[arg(short = 'f', long = "log-format", default_value = "pretty")]
    log_format: String,

    /// Log level: trace | debug | info | warn | error
    #[arg(short = 'l', long = "log-level", default_value = "info")]
    log_level: String,
}

mod ipc;
mod server;
mod state;

fn main() -> Result<()> {
    let cli = DaemonCli::parse();

    let format = match cli.log_format.as_str() {
        "json" => LogFormat::Json,
        _ => LogFormat::Pretty,
    };

    init_logging(format, &cli.log_level);

    tracing::info!("chalybsd daemon starting (IPC server)");

    // Full deterministic multi-client server
    server::run_server()
}
