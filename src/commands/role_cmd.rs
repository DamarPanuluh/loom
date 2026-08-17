//! `loom role` — advisory driver-role leases (claim / release / list).
//!
//! Plane: orchestration over `crate::rolelease`. Claim and release never open
//! the store: a lease is operational state beside the graph, so contending
//! for the graph lock to coordinate would defeat the point. `list` opens
//! read-only for the queue depths behind each role.

use crate::cli::{ClaimRoleArg, RoleCmd};
use crate::Result;
use std::path::Path;

pub(crate) fn dispatch(graph: Option<&Path>, cmd: RoleCmd, json: bool) -> Result<()> {
    match cmd {
        RoleCmd::Claim { role, take_stale } => claim(graph, role, take_stale, json),
        RoleCmd::Release { role } => release(graph, role, json),
        RoleCmd::List => list(graph, json),
    }
}

fn claim(graph: Option<&Path>, role: ClaimRoleArg, take_stale: bool, json: bool) -> Result<()> {
    let root = super::resolve_root(graph)?;
    let identity = crate::identity::ExecutionIdentity::resolve_env()?;
    let (lease, displaced) =
        crate::rolelease::claim(&root, &identity, role.owner_role(), take_stale)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "claimed": lease,
                "took_over_stale": displaced,
            }))?
        );
        return Ok(());
    }
    match displaced {
        Some(d) => println!(
            "claimed role '{}' as '{}' — took over the stale lease from '{}'",
            lease.role, lease.profile, d.profile
        ),
        None => println!(
            "claimed role '{}' as '{}' — every loom command you run under this identity \
             refreshes the lease; it reads as stale after {}s of silence",
            lease.role,
            lease.profile,
            crate::rolelease::ROLE_LEASE_TTL_MS / 1000
        ),
    }
    Ok(())
}

fn release(graph: Option<&Path>, role: ClaimRoleArg, json: bool) -> Result<()> {
    let root = super::resolve_root(graph)?;
    let identity = crate::identity::ExecutionIdentity::resolve_env()?;
    let lease = crate::rolelease::release(&root, &identity, role.owner_role())?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "released": lease }))?
        );
        return Ok(());
    }
    println!("released role '{}' (was '{}')", lease.role, lease.profile);
    Ok(())
}

fn list(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = super::open_read(graph)?;
    let (_, queues) = crate::maturity::ladder_and_depths(&store)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "roles": crate::rolelease::roster_value(store.root(), &queues),
            }))?
        );
        return Ok(());
    }
    println!("roles (advisory leases; a lease grants no write authority):");
    for line in crate::rolelease::describe(store.root(), &queues) {
        println!("  {line}");
    }
    println!("  claim a free one: LOOM_AGENT=llm:<role> LOOM_AGENT_PROFILE=<you> loom role claim <role>");
    Ok(())
}
