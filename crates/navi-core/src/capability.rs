//! Capability ledger for auditable zero-trust tool execution.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Capability {
    RepoRead,
    RepoWriteSrc,
    RepoWriteTests,
    RepoWriteDocs,
    RepoWriteCi,
    RepoWriteLockfile,
    SecretsRead,
    NetworkGithub,
    NetworkNpm,
    ShellSafe,
    ShellPrivileged,
    Mcp { server: String, capability: String },
    Custom(String),
}

impl Capability {
    pub fn parse(value: &str) -> Self {
        match value {
            "repo.read" => Self::RepoRead,
            "repo.write.src" | "repo.write" => Self::RepoWriteSrc,
            "repo.write.tests" => Self::RepoWriteTests,
            "repo.write.docs" => Self::RepoWriteDocs,
            "repo.write.ci" => Self::RepoWriteCi,
            "repo.write.lockfile" => Self::RepoWriteLockfile,
            "secrets.read" => Self::SecretsRead,
            "network.github" => Self::NetworkGithub,
            "network.npm" | "network.package" => Self::NetworkNpm,
            "shell.safe" | "shell.exec" => Self::ShellSafe,
            "shell.privileged" | "shell.bash" => Self::ShellPrivileged,
            other if other.starts_with("mcp.") => {
                let mut parts = other.splitn(3, '.');
                let _ = parts.next();
                let server = parts.next().unwrap_or_default().to_string();
                let capability = parts.next().unwrap_or("*").to_string();
                Self::Mcp { server, capability }
            }
            other => Self::Custom(other.to_string()),
        }
    }

    pub fn as_key(&self) -> String {
        match self {
            Self::RepoRead => "repo.read".to_string(),
            Self::RepoWriteSrc => "repo.write.src".to_string(),
            Self::RepoWriteTests => "repo.write.tests".to_string(),
            Self::RepoWriteDocs => "repo.write.docs".to_string(),
            Self::RepoWriteCi => "repo.write.ci".to_string(),
            Self::RepoWriteLockfile => "repo.write.lockfile".to_string(),
            Self::SecretsRead => "secrets.read".to_string(),
            Self::NetworkGithub => "network.github".to_string(),
            Self::NetworkNpm => "network.npm".to_string(),
            Self::ShellSafe => "shell.safe".to_string(),
            Self::ShellPrivileged => "shell.privileged".to_string(),
            Self::Mcp { server, capability } => format!("mcp.{server}.{capability}"),
            Self::Custom(value) => value.clone(),
        }
    }

    pub fn is_guarded(&self) -> bool {
        matches!(
            self,
            Self::SecretsRead
                | Self::ShellPrivileged
                | Self::RepoWriteCi
                | Self::RepoWriteLockfile
                | Self::NetworkGithub
                | Self::NetworkNpm
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilityScope {
    Session,
    Turn(String),
    Branch(String),
    Subagent(String),
    SingleCall(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilityDecision {
    Requested,
    Granted,
    Denied,
    Consumed,
    Expired,
    Violated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityGrant {
    pub capability: Capability,
    pub scope: CapabilityScope,
    pub justification: String,
    pub expires_at_ms: Option<u64>,
    pub guarded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityLedgerEntry {
    pub capability: Capability,
    pub scope: CapabilityScope,
    pub decision: CapabilityDecision,
    pub at_ms: u64,
    pub justification: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilityLedger {
    grants: BTreeMap<String, CapabilityGrant>,
    consumed: BTreeSet<String>,
    entries: Vec<CapabilityLedgerEntry>,
}

impl CapabilityLedger {
    pub fn entries(&self) -> &[CapabilityLedgerEntry] {
        &self.entries
    }

    pub fn request(&mut self, capability: Capability, scope: CapabilityScope, at_ms: u64) {
        self.entries.push(CapabilityLedgerEntry {
            capability,
            scope,
            decision: CapabilityDecision::Requested,
            at_ms,
            justification: String::new(),
        });
    }

    pub fn grant(
        &mut self,
        capability: Capability,
        scope: CapabilityScope,
        justification: impl Into<String>,
        expires_at_ms: Option<u64>,
        at_ms: u64,
        explicit_user_approval: bool,
    ) -> bool {
        if capability.is_guarded() && !explicit_user_approval {
            self.entries.push(CapabilityLedgerEntry {
                capability,
                scope,
                decision: CapabilityDecision::Denied,
                at_ms,
                justification: "guarded capability requires explicit approval".to_string(),
            });
            return false;
        }

        let grant = CapabilityGrant {
            guarded: capability.is_guarded(),
            capability: capability.clone(),
            scope: scope.clone(),
            justification: justification.into(),
            expires_at_ms,
        };
        self.grants
            .insert(grant_key(&capability, &scope), grant.clone());
        self.entries.push(CapabilityLedgerEntry {
            capability,
            scope,
            decision: CapabilityDecision::Granted,
            at_ms,
            justification: grant.justification,
        });
        true
    }

    pub fn consume(
        &mut self,
        capability: &Capability,
        scope: &CapabilityScope,
        call_id: &str,
        at_ms: u64,
    ) -> bool {
        let key = grant_key(capability, scope);
        let Some(grant) = self.grants.get(&key).cloned() else {
            self.entries.push(CapabilityLedgerEntry {
                capability: capability.clone(),
                scope: scope.clone(),
                decision: CapabilityDecision::Violated,
                at_ms,
                justification: "capability was not granted".to_string(),
            });
            return false;
        };
        if grant
            .expires_at_ms
            .map(|expires| at_ms > expires)
            .unwrap_or(false)
        {
            self.entries.push(CapabilityLedgerEntry {
                capability: capability.clone(),
                scope: scope.clone(),
                decision: CapabilityDecision::Expired,
                at_ms,
                justification: "capability grant expired".to_string(),
            });
            return false;
        }
        self.consumed.insert(format!("{key}:{call_id}"));
        self.entries.push(CapabilityLedgerEntry {
            capability: capability.clone(),
            scope: scope.clone(),
            decision: CapabilityDecision::Consumed,
            at_ms,
            justification: call_id.to_string(),
        });
        true
    }
}

pub fn capabilities_from_tool_metadata(values: &[String]) -> Vec<Capability> {
    values
        .iter()
        .map(|value| Capability::parse(value))
        .collect()
}

fn grant_key(capability: &Capability, scope: &CapabilityScope) -> String {
    format!("{}::{scope:?}", capability.as_key())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarded_capability_requires_explicit_approval() {
        let mut ledger = CapabilityLedger::default();

        let granted = ledger.grant(
            Capability::ShellPrivileged,
            CapabilityScope::Session,
            "run shell",
            None,
            1,
            false,
        );

        assert!(!granted);
        assert_eq!(ledger.entries()[0].decision, CapabilityDecision::Denied);
    }

    #[test]
    fn granted_capability_can_be_consumed_before_expiry() {
        let mut ledger = CapabilityLedger::default();
        assert!(ledger.grant(
            Capability::RepoRead,
            CapabilityScope::Turn("t1".to_string()),
            "read repo",
            Some(10),
            1,
            false,
        ));

        assert!(ledger.consume(
            &Capability::RepoRead,
            &CapabilityScope::Turn("t1".to_string()),
            "call-1",
            5,
        ));
    }

    #[test]
    fn expired_capability_is_not_consumed() {
        let mut ledger = CapabilityLedger::default();
        assert!(ledger.grant(
            Capability::RepoRead,
            CapabilityScope::Session,
            "read repo",
            Some(10),
            1,
            false,
        ));

        assert!(!ledger.consume(&Capability::RepoRead, &CapabilityScope::Session, "call", 11));
        assert_eq!(
            ledger.entries().last().unwrap().decision,
            CapabilityDecision::Expired
        );
    }
}
