//! Role leases — advisory coordination between several LLM drivers.
//!
//! Plane: operational state under `.loom/leases/`, like the lock files. Never
//! graph truth: a lease grants no write authority (the lane gate in
//! `Store::check_lane` does that) and `LOOM_AGENT_PROFILE` remains
//! self-declared attribution (`identity.rs`), so a lease is a signpost honest
//! drivers honor, not a security fence.
//!
//! Mechanism: a heartbeat file per role, not a held flock. Drivers are many
//! short-lived `loom` processes, so nothing lives long enough to hold a lock
//! for the session; instead `claim` writes the lease and every later command
//! run under the same role+profile refreshes `last_seen_ms` at store open. A
//! lease not refreshed within [`ROLE_LEASE_TTL_MS`] reads as stale, and a
//! stale role is offered as free (with its debt) by `loom role list`,
//! `loom session`, and `loom status`. A crashed driver therefore frees its
//! role by silence — no cleanup required.
//!
//! Claim and release serialize under a brief flock on `.loom/leases/lock`
//! (the journal-lock pattern) and land as journal entries; the refresh
//! heartbeat is deliberately journal-silent so it cannot spam the record.

use crate::identity::{Agent, ExecutionIdentity};
use crate::lane::Lane;
use crate::registry::OwnerRole;
use crate::{Result, GRAPH_DB, LOOM_DIR};
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

/// Marker inside the contention error so `main` maps it to the contention
/// exit code — the same convention as the graph and harness locks.
pub const ROLE_CONTENTION_MARKER: &str = "loom-role-contention";

/// Freshness window: a lease whose `last_seen_ms` is older than this reads as
/// stale. Registered in `loom limits` as `role_lease_ttl_ms`.
pub const ROLE_LEASE_TTL_MS: u64 = 900_000;

/// The six lane authorities a driver can hold (`LOOM_AGENT=llm:<role>`).
/// `sync` is loom's own derived writer and is never claimable.
pub const CLAIMABLE: &[OwnerRole] = &[
    OwnerRole::Builder,
    OwnerRole::Analyzer,
    OwnerRole::Fixer,
    OwnerRole::Validator,
    OwnerRole::Quality,
    OwnerRole::Rectify,
];

/// One role's heartbeat lease.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleLease {
    pub role: String,
    /// The self-declared worker profile that claimed the role.
    pub profile: String,
    /// The claiming authority (`llm:<role>`), kept for the announce.
    pub actor: String,
    /// Pid of the last process that touched the lease — informative only;
    /// the lease outlives any single process by design.
    pub pid: u32,
    pub claimed_at_ms: u64,
    pub last_seen_ms: u64,
}

impl RoleLease {
    pub fn is_fresh(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.last_seen_ms) <= ROLE_LEASE_TTL_MS
    }
}

/// The lanes each role drains, for the announce's per-role debt. The `review`
/// lane is shared: its packet runs AS the low-confidence edge's owning lane
/// (see `review_item` in `workitem/queues.rs`), so it appears under analyzer,
/// validator, AND quality — per-role debt sums overlap there by design.
pub fn role_lanes(role: OwnerRole) -> &'static [Lane] {
    match role {
        OwnerRole::Builder => &[
            Lane::Derive,
            Lane::Build,
            Lane::Surface,
            Lane::Coverage,
            Lane::Elaborate,
        ],
        OwnerRole::Fixer => &[Lane::Fix],
        OwnerRole::Analyzer => &[
            Lane::Analyze,
            Lane::Prove,
            Lane::Triage,
            Lane::Audit,
            Lane::Review,
        ],
        OwnerRole::Validator => &[Lane::Validate, Lane::Deepen, Lane::Review],
        OwnerRole::Quality => &[Lane::Quality, Lane::Review],
        OwnerRole::Rectify => &[Lane::Rectify],
        OwnerRole::Sync => &[],
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn leases_dir(root: &Path) -> PathBuf {
    root.join(LOOM_DIR).join("leases")
}

fn lease_path(root: &Path, role: OwnerRole) -> PathBuf {
    leases_dir(root).join(format!("{}.json", role.as_str()))
}

/// Brief exclusive lock serializing claim/release/refresh — the journal-lock
/// pattern: held only across one read-decide-write of a lease file.
fn lease_lock(root: &Path) -> Result<File> {
    let dir = leases_dir(root);
    fs::create_dir_all(&dir)?;
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(dir.join("lock"))?;
    file.lock()?;
    Ok(file)
}

/// Read one role's lease. Best-effort: a missing or unparsable file is `None` —
/// a corrupt lease is operational residue that must never brick commands, and
/// the next `claim` simply overwrites it.
pub fn read(root: &Path, role: OwnerRole) -> Option<RoleLease> {
    let raw = fs::read_to_string(lease_path(root, role)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_lease(root: &Path, lease: &RoleLease) -> Result<()> {
    let dir = leases_dir(root);
    fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!(".{}.tmp-{}", lease.role, std::process::id()));
    fs::write(&tmp, serde_json::to_vec_pretty(lease)?)?;
    fs::rename(&tmp, dir.join(format!("{}.json", lease.role)))
        .with_context(|| format!("publishing lease for role '{}'", lease.role))?;
    Ok(())
}

/// Claim and release never open the store, so they must repeat its
/// graph-exists guard: without it a mistyped `--graph` path would scaffold
/// `.loom/leases/` and a journal in a directory that has no graph.
fn require_graph(root: &Path) -> Result<()> {
    let db_path = root.join(LOOM_DIR).join(GRAPH_DB);
    if !db_path.exists() {
        bail!(
            "no loom graph at {} — run `loom init` first",
            db_path.display()
        );
    }
    Ok(())
}

/// The role+profile a lease action runs as, or why it cannot run.
fn lease_identity(identity: &ExecutionIdentity, role: OwnerRole) -> Result<String> {
    match identity.authority() {
        Agent::Solo => bail!(
            "role leases coordinate llm drivers; a solo operator drives every lane and \
             claims nothing — set LOOM_AGENT=llm:{} to drive this role",
            role.as_str()
        ),
        Agent::Lane(lane) if lane != role => bail!(
            "claiming '{}' under authority 'llm:{}' — a lease belongs to the role you \
             drive; set LOOM_AGENT=llm:{}",
            role.as_str(),
            lane.as_str(),
            role.as_str()
        ),
        Agent::Lane(_) => {}
    }
    match identity.profile() {
        Some(profile) => Ok(profile.to_string()),
        None => bail!(
            "a lease needs a worker identity: set LOOM_AGENT_PROFILE (self-declared, \
             for example 'agent-7') so the announce can name who holds '{}'",
            role.as_str()
        ),
    }
}

/// Claim a role for the current `LOOM_AGENT_PROFILE`. Re-claim by the same
/// profile is an idempotent refresh. A fresh foreign lease refuses with
/// [`ROLE_CONTENTION_MARKER`]; a stale foreign lease requires the deliberate
/// `take_stale` acknowledgement. Returns the new lease and any displaced one.
pub fn claim(
    root: &Path,
    identity: &ExecutionIdentity,
    role: OwnerRole,
    take_stale: bool,
) -> Result<(RoleLease, Option<RoleLease>)> {
    require_graph(root)?;
    let profile = lease_identity(identity, role)?;
    let _guard = lease_lock(root)?;
    let now = now_ms();
    let existing = read(root, role);
    let mut displaced = None;
    if let Some(current) = existing {
        if current.profile != profile {
            if current.is_fresh(now) {
                bail!(
                    "{ROLE_CONTENTION_MARKER}: role '{}' is held by profile '{}' \
                     (pid {}, last seen {}s ago) — pick a free role from `loom role list`, \
                     or retry after the lease goes stale (role_lease_ttl_ms={})",
                    role.as_str(),
                    current.profile,
                    current.pid,
                    now.saturating_sub(current.last_seen_ms) / 1000,
                    ROLE_LEASE_TTL_MS
                );
            }
            if !take_stale {
                bail!(
                    "role '{}' has a STALE lease from profile '{}' (last seen {}s ago, \
                     ttl {}s) — pass --take-stale to take the role over deliberately",
                    role.as_str(),
                    current.profile,
                    now.saturating_sub(current.last_seen_ms) / 1000,
                    ROLE_LEASE_TTL_MS / 1000
                );
            }
            displaced = Some(current);
        }
    }
    let lease = RoleLease {
        role: role.as_str().to_string(),
        profile,
        actor: identity.actor(),
        pid: std::process::id(),
        claimed_at_ms: now,
        last_seen_ms: now,
    };
    write_lease(root, &lease)?;
    crate::journal::append(
        root,
        identity,
        "role_claimed",
        &format!("role:{}", role.as_str()),
        serde_json::json!({
            "role": lease.role,
            "profile": lease.profile,
            "pid": lease.pid,
            "took_over_stale": displaced.as_ref().map(|d| d.profile.clone()),
        }),
    )?;
    Ok((lease, displaced))
}

/// Release the current profile's lease on a role. Strict: only the holding
/// profile releases; a stale foreign lease is taken over via
/// `claim --take-stale`, never silently removed.
pub fn release(root: &Path, identity: &ExecutionIdentity, role: OwnerRole) -> Result<RoleLease> {
    require_graph(root)?;
    let profile = lease_identity(identity, role)?;
    let _guard = lease_lock(root)?;
    let Some(current) = read(root, role) else {
        bail!("role '{}' has no lease to release", role.as_str());
    };
    if current.profile != profile {
        bail!(
            "role '{}' is leased by profile '{}', not '{}' — only the holder releases; \
             a stale lease is taken over with `loom role claim {} --take-stale`",
            role.as_str(),
            current.profile,
            profile,
            role.as_str()
        );
    }
    fs::remove_file(lease_path(root, role))?;
    crate::journal::append(
        root,
        identity,
        "role_released",
        &format!("role:{}", role.as_str()),
        serde_json::json!({ "role": current.role, "profile": current.profile }),
    )?;
    Ok(current)
}

/// The heartbeat: called at store open. If the process runs as `llm:<role>`
/// with a profile matching that role's lease, stamp `last_seen_ms`. Never
/// creates a lease (claim stays explicit), never journals, and never fails
/// the command — coordination residue must not block graph work.
pub fn refresh(root: &Path, identity: &ExecutionIdentity) {
    let Agent::Lane(role) = identity.authority() else {
        return;
    };
    let Some(profile) = identity.profile() else {
        return;
    };
    // Cheap miss before taking the lock: no lease file, nothing to refresh.
    if !lease_path(root, role).exists() {
        return;
    }
    let Ok(_guard) = lease_lock(root) else { return };
    let Some(mut lease) = read(root, role) else {
        return;
    };
    if lease.profile != profile {
        return;
    }
    lease.last_seen_ms = now_ms();
    lease.pid = std::process::id();
    let _ = write_lease(root, &lease);
}

/// The announce block: every claimable role with its lease (if any),
/// freshness, per-lane queue depths, and their sum as `debt`.
pub fn roster_value(root: &Path, queues: &crate::lane::QueueDepths) -> serde_json::Value {
    let now = now_ms();
    let mut roles = serde_json::Map::new();
    for &role in CLAIMABLE {
        let lanes: serde_json::Map<String, serde_json::Value> = role_lanes(role)
            .iter()
            .map(|&lane| (lane.as_str().to_string(), queues.get(lane).into()))
            .collect();
        let debt: u64 = role_lanes(role).iter().map(|&l| queues.get(l) as u64).sum();
        let entry = match read(root, role) {
            Some(lease) => serde_json::json!({
                "claimed_by": lease.profile,
                "pid": lease.pid,
                "claimed_at_ms": lease.claimed_at_ms,
                "last_seen_ms": lease.last_seen_ms,
                "fresh": lease.is_fresh(now),
                "lanes": lanes,
                "debt": debt,
            }),
            None => serde_json::json!({
                "claimed_by": serde_json::Value::Null,
                "lanes": lanes,
                "debt": debt,
            }),
        };
        roles.insert(role.as_str().to_string(), entry);
    }
    serde_json::Value::Object(roles)
}

/// One human-readable line per role for `loom role list` and the session offer.
pub fn describe(root: &Path, queues: &crate::lane::QueueDepths) -> Vec<String> {
    let now = now_ms();
    CLAIMABLE
        .iter()
        .map(|&role| {
            let debt: u64 = role_lanes(role).iter().map(|&l| queues.get(l) as u64).sum();
            match read(root, role) {
                Some(l) if l.is_fresh(now) => format!(
                    "{:<10} held by {} (seen {}s ago)   debt={}",
                    role.as_str(),
                    l.profile,
                    now.saturating_sub(l.last_seen_ms) / 1000,
                    debt
                ),
                Some(l) => format!(
                    "{:<10} STALE lease from {} (seen {}s ago; claim --take-stale)   debt={}",
                    role.as_str(),
                    l.profile,
                    now.saturating_sub(l.last_seen_ms) / 1000,
                    debt
                ),
                None => format!("{:<10} free   debt={}", role.as_str(), debt),
            }
        })
        .collect()
}

/// A one-line collision warning when a served packet's owning role holds a
/// FRESH lease under a different profile: the packet is that driver's work.
/// Advisory like the lease itself — `next` still serves the packet; the
/// warning exists so the collision is chosen, never accidental. Stale leases
/// and unclaimable owners (`human`, `sync`) warn about nothing.
pub fn conflict_warning(
    root: &Path,
    identity: &ExecutionIdentity,
    owner_role: &str,
) -> Option<String> {
    let role = CLAIMABLE.iter().copied().find(|r| r.as_str() == owner_role)?;
    let lease = read(root, role)?;
    if !lease.is_fresh(now_ms()) {
        return None;
    }
    if identity.profile() == Some(lease.profile.as_str()) {
        return None;
    }
    Some(format!(
        "role '{}' is freshly leased to '{}' — this packet is that driver's work; \
         pick a free role from `loom role list` or coordinate before writing",
        role.as_str(),
        lease.profile
    ))
}

/// Compact `role=profile(fresh|stale)` summary of held roles, or `None` when
/// no lease exists — so solo graphs never print coordination noise.
pub fn holders_line(root: &Path) -> Option<String> {
    let now = now_ms();
    let held: Vec<String> = CLAIMABLE
        .iter()
        .filter_map(|&role| {
            read(root, role).map(|l| {
                format!(
                    "{}={}({})",
                    role.as_str(),
                    l.profile,
                    if l.is_fresh(now) { "fresh" } else { "stale" }
                )
            })
        })
        .collect();
    if held.is_empty() {
        None
    } else {
        Some(held.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every lane a driver can be served work from belongs to exactly the
    /// role whose packets it emits (see the `*_item` constructors in
    /// `workitem/queues.rs`); the union covers all item-serving, non-human
    /// lanes so a fully-leased graph leaves no orphan queue.
    #[test]
    fn role_lanes_cover_every_llm_served_lane() {
        let mut covered = std::collections::BTreeSet::new();
        for &role in CLAIMABLE {
            for lane in role_lanes(role) {
                covered.insert(lane.as_str());
            }
        }
        let expected: std::collections::BTreeSet<&str> = Lane::LADDER
            .iter()
            .filter(|l| l.serves_items() && !l.requires_human_decision())
            .map(|l| l.as_str())
            .collect();
        assert_eq!(covered, expected);
        assert!(role_lanes(OwnerRole::Sync).is_empty());
    }

    #[test]
    fn freshness_is_a_ttl_over_last_seen() {
        let lease = RoleLease {
            role: "analyzer".into(),
            profile: "agent-a".into(),
            actor: "llm:analyzer".into(),
            pid: 1,
            claimed_at_ms: 0,
            last_seen_ms: 1_000,
        };
        assert!(lease.is_fresh(1_000 + ROLE_LEASE_TTL_MS));
        assert!(!lease.is_fresh(1_001 + ROLE_LEASE_TTL_MS));
    }
}
