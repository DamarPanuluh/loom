//! Runtime execution identity.
//!
//! This module is the single seam between process configuration and Loom's
//! authorization/provenance model. `LOOM_AGENT` grants lane authority;
//! `LOOM_AGENT_PROFILE` is self-declared executor attribution and grants
//! nothing. Both are resolved and validated once, then the resulting value is
//! passed to every persistence and lock surface.

use crate::registry::OwnerRole;
use crate::Result;
use anyhow::bail;

pub const AGENT_ENV: &str = "LOOM_AGENT";
pub const PROFILE_ENV: &str = "LOOM_AGENT_PROFILE";

/// Authorization identity. Solo may drive every lane; a declared lane is
/// enforced at the Store write seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Solo,
    Lane(OwnerRole),
}

impl Agent {
    /// Resolve authority from the process environment. Unset means an
    /// intentional local solo operator; every explicit value must be canonical.
    pub fn from_env() -> Result<Self> {
        match std::env::var(AGENT_ENV) {
            Ok(value) => Self::parse(&value),
            Err(std::env::VarError::NotPresent) => Ok(Self::Solo),
            Err(error) => Err(error.into()),
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        let value = value.trim();
        if value == "solo" {
            return Ok(Self::Solo);
        }
        // Both the accepted set and the message come from `OwnerRole::ALL`, so a
        // new lane cannot be accepted by the parser while the error still lists
        // the old set — this used to be a hand-written match plus the same
        // sentence typed out twice.
        let role = value
            .strip_prefix("llm:")
            .and_then(|lane| lane.parse::<OwnerRole>().ok())
            .filter(|role| *role != OwnerRole::Sync);
        let Some(role) = role else {
            bail!(
                "unrecognized LOOM_AGENT '{value}' — use canonical llm:<{}>, explicit solo, or leave it unset for solo",
                Agent::lane_names().join("|")
            );
        };
        Ok(Self::Lane(role))
    }

    /// The lanes `LOOM_AGENT` accepts: every owner role except `sync`, which is
    /// loom's own derived-edge writer and never an agent identity.
    fn lane_names() -> Vec<&'static str> {
        OwnerRole::ALL
            .iter()
            .filter(|role| **role != OwnerRole::Sync)
            .map(|role| role.as_str())
            .collect()
    }

    pub fn actor(self) -> String {
        match self {
            Self::Solo => "solo".into(),
            Self::Lane(role) => format!("llm:{}", role.as_str()),
        }
    }
}

/// A validated, self-declared worker profile such as `loom-auditor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerProfile(String);

impl WorkerProfile {
    pub fn parse(value: &str) -> Result<Self> {
        let value = value.trim();
        let valid = !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:/".contains(&byte));
        if !valid {
            bail!(
                "invalid LOOM_AGENT_PROFILE '{value}' — use 1-128 ASCII letters, digits, '-', '_', '.', ':', or '/'"
            );
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where executor attribution came from. Environment profiles are observable
/// but self-declared, so they are never presented as verified identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSource {
    Environment,
}

impl ProfileSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "environment",
        }
    }

    pub fn verified(self) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutorIdentity {
    profile: WorkerProfile,
    source: ProfileSource,
}

impl ExecutorIdentity {
    pub fn environment(profile: WorkerProfile) -> Self {
        Self {
            profile,
            source: ProfileSource::Environment,
        }
    }

    pub fn profile(&self) -> &str {
        self.profile.as_str()
    }

    pub fn source(&self) -> ProfileSource {
        self.source
    }

    pub fn verified(&self) -> bool {
        self.source.verified()
    }
}

/// Canonical runtime identity passed to facts, journal entries, and lock
/// records. Authority and executor attribution cannot be confused by type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionIdentity {
    authority: Agent,
    executor: Option<ExecutorIdentity>,
}

impl ExecutionIdentity {
    pub fn solo() -> Self {
        Self::new(Agent::Solo, None)
    }

    pub fn resolve_env() -> Result<Self> {
        let authority = Agent::from_env()?;
        let executor = match std::env::var(PROFILE_ENV) {
            Ok(value) => Some(ExecutorIdentity::environment(WorkerProfile::parse(&value)?)),
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => return Err(error.into()),
        };
        Ok(Self::new(authority, executor))
    }

    pub fn new(authority: Agent, executor: Option<ExecutorIdentity>) -> Self {
        Self {
            authority,
            executor,
        }
    }

    pub fn authority(&self) -> Agent {
        self.authority
    }

    pub fn actor(&self) -> String {
        self.authority.actor()
    }

    pub fn executor(&self) -> Option<&ExecutorIdentity> {
        self.executor.as_ref()
    }

    pub fn profile(&self) -> Option<&str> {
        self.executor.as_ref().map(ExecutorIdentity::profile)
    }

    pub fn with_authority(&self, authority: Agent) -> Self {
        Self::new(authority, self.executor.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authority_accepts_only_canonical_spellings() {
        assert_eq!(Agent::parse("solo").unwrap(), Agent::Solo);
        assert_eq!(
            Agent::parse("llm:analyzer").unwrap(),
            Agent::Lane(OwnerRole::Analyzer)
        );
        assert!(Agent::parse("analyzer").is_err());
        assert!(Agent::parse("llm").is_err());
        assert!(Agent::parse("").is_err());
    }

    #[test]
    fn worker_profile_is_validated_but_never_grants_authority() {
        let profile = WorkerProfile::parse("loom-auditor").unwrap();
        let identity = ExecutionIdentity::new(
            Agent::Lane(OwnerRole::Analyzer),
            Some(ExecutorIdentity::environment(profile)),
        );
        assert_eq!(identity.authority(), Agent::Lane(OwnerRole::Analyzer));
        assert_eq!(identity.actor(), "llm:analyzer");
        assert_eq!(identity.profile(), Some("loom-auditor"));
        assert!(!identity.executor().unwrap().verified());
        assert!(WorkerProfile::parse("loom auditor").is_err());
    }
}
