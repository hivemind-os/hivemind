use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub version: String,
    #[serde(alias = "uptimeSecs")]
    pub uptime_secs: f64,
    pub pid: u32,
    pub platform: String,
    pub bind: String,
}
