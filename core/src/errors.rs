use thiserror::Error;

pub type Result<T> = std::result::Result<T, ChalybsError>;

#[derive(Debug, Error)]
pub enum ChalybsError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("cgroup/cpuset error: {0}")]
    Cgroup(String),

    #[error("QEMU error: {0}")]
    Qemu(String),

    #[error("VFIO error: {0}")]
    Vfio(String),

    #[error("IRQ error: {0}")]
    Irq(String),

    #[error("Affinity error: {0}")]
    Affinity(String),

    #[error("Peripheral error: {0}")]
    Peripheral(String),

    #[error("State transition error: {0}")]
    State(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serde error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("Toml error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("Other: {0}")]
    Other(String),
}

impl ChalybsError {
    pub fn state<S: Into<String>>(msg: S) -> Self {
        ChalybsError::State(msg.into())
    }
}
