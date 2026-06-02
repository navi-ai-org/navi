use serde::{Deserialize, Serialize};

/// Risk level for a tool or capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RiskLevel {
    Low = 1,
    Medium = 2,
    High = 4,
    Critical = 8,
    Forbidden = 16,
}

impl RiskLevel {
    pub fn score(self) -> u8 {
        self as u8
    }

    pub fn is_at_least(self, other: RiskLevel) -> bool {
        self >= other
    }
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskLevel::Low => write!(f, "LOW"),
            RiskLevel::Medium => write!(f, "MEDIUM"),
            RiskLevel::High => write!(f, "HIGH"),
            RiskLevel::Critical => write!(f, "CRITICAL"),
            RiskLevel::Forbidden => write!(f, "FORBIDDEN"),
        }
    }
}

/// The result of a risk classification.
#[derive(Debug, Clone)]
pub struct RiskAssessment {
    pub level: RiskLevel,
    pub warning: Option<String>,
    pub single_risks: Vec<(String, RiskLevel)>,
    pub compound_risks: Vec<(String, RiskLevel)>,
}
