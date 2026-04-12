use hive_classification::DataClass;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RiskVerdict {
    Clean,
    Suspicious,
    Detected,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScanRecommendation {
    Pass,
    Redact,
    Block,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScanActionTaken {
    Passed,
    Blocked,
    Redacted,
    UserAllowed,
    UserBlocked,
    Flagged,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScanDecision {
    Allow,
    Block,
    Redact,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FlaggedSpan {
    pub start: usize,
    pub end: usize,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScanSummary {
    pub verdict: RiskVerdict,
    pub confidence: f32,
    #[serde(alias = "actionTaken")]
    pub action_taken: ScanActionTaken,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromptInjectionReview {
    pub source: String,
    pub verdict: RiskVerdict,
    pub confidence: f32,
    pub threat_type: Option<String>,
    pub flagged_spans: Vec<FlaggedSpan>,
    pub recommendation: ScanRecommendation,
    pub preview: String,
    pub proposed_redaction: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RiskScanRecord {
    pub id: String,
    #[serde(alias = "scanType")]
    pub scan_type: String,
    #[serde(alias = "payloadHash")]
    pub payload_hash: String,
    #[serde(alias = "payloadPreview")]
    pub payload_preview: String,
    pub source: String,
    #[serde(alias = "sourceSession")]
    pub source_session: Option<String>,
    pub verdict: RiskVerdict,
    pub confidence: f32,
    #[serde(alias = "threatType")]
    pub threat_type: Option<String>,
    #[serde(alias = "flaggedSpans")]
    pub flagged_spans: Vec<FlaggedSpan>,
    #[serde(alias = "actionTaken")]
    pub action_taken: ScanActionTaken,
    #[serde(alias = "userDecision")]
    pub user_decision: Option<ScanDecision>,
    #[serde(alias = "modelUsed")]
    pub model_used: String,
    #[serde(alias = "scanDurationMs")]
    pub scan_duration_ms: u64,
    #[serde(alias = "dataClass")]
    pub data_class: DataClass,
    #[serde(alias = "scannedAtMs")]
    pub scanned_at_ms: u64,
}

/// Status of a file's security audit
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAuditStatus {
    /// File has never been audited
    Unaudited,
    /// File was audited and found safe
    Safe,
    /// File was audited and has identified risks
    Risky,
    /// File content changed since last audit — needs re-audit
    Stale,
}

/// A single identified risk in a file audit
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileAuditRisk {
    pub id: String,
    pub description: String,
    pub probability: f64,
    pub severity: RiskSeverity,
    pub evidence: Option<String>,
}

/// Severity level for file audit risks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskSeverity {
    Low,
    Medium,
    High,
    Critical,
}

/// Complete audit record for a file, keyed by content hash
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileAuditRecord {
    pub path: String,
    #[serde(alias = "contentHash")]
    pub content_hash: String,
    pub risks: Vec<FileAuditRisk>,
    pub verdict: RiskVerdict,
    pub summary: String,
    #[serde(alias = "modelUsed")]
    pub model_used: String,
    #[serde(alias = "auditedAtMs")]
    pub audited_at_ms: u64,
}
