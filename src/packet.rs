//! Packet identity — the seam that makes "did loom's context actually help?"
//! a measurable question instead of a claim.
//!
//! Plane: journal-backed bookkeeping. A packet id is minted where a packet
//! LEAVES the process (the CLI renderer or the MCP tool call), never where it is
//! assembled — assembling a packet for a test or an internal caller is not a
//! serving, and stamping it there would inflate the denominator.
//!
//! Contract: every served packet appends one `packet_served` journal entry
//! naming its id, kind, target, and actor. A later verified write records the
//! packet ids live in that actor's recent window, so the efficacy ratio
//! (packets cited in work that subsequently passed a proof loom ran) is derived
//! from the append-only record rather than self-reported. That ratio is
//! STATISTICAL: it is reported, never gated (INV-3).

use crate::store::Store;
use crate::Result;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};

/// Process-local monotonic suffix, so two packets minted inside the same
/// millisecond still differ.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One packet handed to a consumer.
#[derive(Debug, Clone, Serialize)]
pub struct Served {
    pub id: String,
    /// `context` | `next` | `<lane>` — what kind of packet this was.
    pub kind: String,
    /// The entity the packet was about, so efficacy can be attributed.
    pub target: String,
}

fn mint(kind: &str, target: &str) -> Served {
    let timestamp = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_millis().to_string(),
        // Batch minting is infallible, so retain the clock state in the id instead of inventing an epoch.
        Err(error) => format!("pre-epoch-{}", error.duration().as_millis()),
    };
    let seq = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Served {
        id: format!("pkt-{timestamp}-{seq}"),
        kind: kind.to_string(),
        target: target.to_string(),
    }
}

/// Record a batch of packets served by one invocation as a single journal
/// entry. One entry per invocation, not per packet: `loom next --all` serves a
/// packet per lane and should read as one act of serving.
pub fn serve(store: &Store, packets: &[Served]) -> Result<()> {
    if packets.is_empty() {
        return Ok(());
    }
    store.append_journal(
        "packet_served",
        "packets",
        serde_json::json!({ "packets": packets }),
    )?;
    Ok(())
}

/// Mint and journal a single packet, returning its id.
pub fn serve_one(store: &Store, kind: &str, target: &str) -> Result<String> {
    let served = mint(kind, target);
    let id = served.id.clone();
    serve(store, std::slice::from_ref(&served))?;
    Ok(id)
}

/// Mint ids for a batch without journaling; pair with [`serve`].
pub fn mint_batch(entries: &[(&str, &str)]) -> Vec<Served> {
    entries.iter().map(|(k, t)| mint(k, t)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_within_a_millisecond() {
        let a = mint("next", "x");
        let b = mint("next", "x");
        assert_ne!(a.id, b.id, "the sequence suffix must disambiguate");
        assert!(a.id.starts_with("pkt-"));
    }
}
