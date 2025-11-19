use tracing_subscriber::{fmt, EnvFilter};

pub enum LogFormat {
    Pretty,
    Json,
}

pub fn init_logging(format: LogFormat, default_level: &str) {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    let builder = fmt::Subscriber::builder()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr);

    match format {
        LogFormat::Pretty => {
            let subscriber = builder.finish();
            tracing::subscriber::set_global_default(subscriber)
                .expect("failed to set global subscriber");
        }
        LogFormat::Json => {
            let subscriber = builder.json().finish();
            tracing::subscriber::set_global_default(subscriber)
                .expect("failed to set global subscriber");
        }
    }
}
