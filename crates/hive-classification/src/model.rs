use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum DataClass {
    #[serde(alias = "PUBLIC")]
    Public,
    #[serde(alias = "INTERNAL")]
    #[default]
    Internal,
    #[serde(alias = "CONFIDENTIAL")]
    Confidential,
    #[serde(alias = "RESTRICTED")]
    Restricted,
}

impl DataClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "PUBLIC",
            Self::Internal => "INTERNAL",
            Self::Confidential => "CONFIDENTIAL",
            Self::Restricted => "RESTRICTED",
        }
    }

    pub fn to_i64(self) -> i64 {
        match self {
            Self::Public => 0,
            Self::Internal => 1,
            Self::Confidential => 2,
            Self::Restricted => 3,
        }
    }

    pub fn from_i64(value: i64) -> Option<Self> {
        match value {
            0 => Some(Self::Public),
            1 => Some(Self::Internal),
            2 => Some(Self::Confidential),
            3 => Some(Self::Restricted),
            _ => None,
        }
    }
}

impl fmt::Display for DataClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelClass {
    Public,
    Internal,
    Private,
    LocalOnly,
}

impl ChannelClass {
    pub fn max_allowed(self) -> DataClass {
        match self {
            Self::Public => DataClass::Public,
            Self::Internal => DataClass::Internal,
            Self::Private => DataClass::Confidential,
            Self::LocalOnly => DataClass::Restricted,
        }
    }

    pub fn allows(self, data_class: DataClass) -> bool {
        data_class <= self.max_allowed()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
            Self::Private => "private",
            Self::LocalOnly => "local-only",
        }
    }
}

impl fmt::Display for ChannelClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelSource {
    Pattern,
    Source,
    User,
    GraphInheritance,
    ModelSuggestion,
    Override,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassificationLabel {
    pub level: DataClass,
    pub source: LabelSource,
    pub reason: Option<String>,
    pub timestamp_ms: u128,
}

impl ClassificationLabel {
    pub fn new(level: DataClass, source: LabelSource, reason: Option<String>) -> Self {
        Self {
            level,
            source,
            reason,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensitiveSpan {
    pub start: usize,
    pub end: usize,
    pub reason: String,
    pub level: DataClass,
}
