use crate::errors::{Result, ChalybsError};

/// Parse CPU list strings like "0-3,8,9,10-12"
pub fn parse_cpu_list(s: &str) -> Result<Vec<u32>> {
    let mut cpus = Vec::new();
    for part in s.split(',').map(|p| p.trim()).filter(|p| !p.is_empty()) {
        if let Some((start, end)) = part.split_once('-') {
            let start: u32 = start.parse().map_err(|e| {
                ChalybsError::Config(format!("invalid cpu range start '{start}': {e}"))
            })?;
            let end: u32 = end.parse().map_err(|e| {
                ChalybsError::Config(format!("invalid cpu range end '{end}': {e}"))
            })?;
            for c in start..=end {
                cpus.push(c);
            }
        } else {
            let c: u32 = part.parse().map_err(|e| {
                ChalybsError::Config(format!("invalid cpu '{part}': {e}"))
            })?;
            cpus.push(c);
        }
    }
    Ok(cpus)
}
