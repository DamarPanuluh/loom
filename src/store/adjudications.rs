//! Hit-level adjudication for pattern pre-screen hits.
//!
//! A pattern pre-screen re-surfaces the same false positive on every
//! rule×intent pair, every re-measure, with shifted line numbers — so the
//! judgment "this hit is not what the rule means" belongs in the graph, not in
//! an operator's out-of-band ledger. A suppression is keyed by the CONTENT of
//! the matched text (never its position): judged once with a reason, it
//! answers that same text on every future scan, and it stops applying the
//! moment the matched text changes — invalidation is the key, not a sweep.

use crate::Result;
use anyhow::{anyhow, bail};
use rusqlite::params;
use serde::Serialize;

use super::Store;

/// One recorded hit judgment.
#[derive(Debug, Clone, Serialize)]
pub struct HitAdjudication {
    pub rule_name: String,
    /// Fingerprint of the canonical matched text — the hit's identity.
    pub content_hash: String,
    /// The matched text as scanned (trimmed, ≤160 chars).
    pub excerpt: String,
    pub reason: String,
    pub actor: String,
    pub created_at: String,
}

impl Store {
    /// Judge one hit's matched text as not-what-the-rule-means. Idempotent on
    /// the same (rule, text); a conflicting re-judgment of the same text must
    /// be an explicit unsuppress-then-suppress, not a silent overwrite.
    pub fn suppress_hit(
        &self,
        rule_name: &str,
        excerpt: &str,
        reason: &str,
    ) -> Result<HitAdjudication> {
        if reason.trim().is_empty() {
            bail!("a suppression needs a substantive --reason — it is the audit, not a formality");
        }
        let canonical = crate::prescan::canonical_excerpt(excerpt);
        if canonical.is_empty() {
            bail!("nothing to suppress — --excerpt is empty");
        }
        let hash = hit_hash(&canonical);
        if self.is_hit_suppressed(rule_name, &canonical)? {
            bail!(
                "'{rule_name}' hit already suppressed ({hash}) — `loom rule unsuppress` first to \
                 re-judge it"
            );
        }
        let actor = self.execution_identity().actor();
        let created_at: String =
            self.conn
                .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
                    r.get(0)
                })?;
        self.conn.execute(
            "INSERT INTO hit_adjudication(rule_name, content_hash, excerpt, reason, actor, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![rule_name, hash, canonical, reason, actor, created_at],
        )?;
        let row = HitAdjudication {
            rule_name: rule_name.to_string(),
            content_hash: hash,
            excerpt: canonical,
            reason: reason.to_string(),
            actor,
            created_at,
        };
        self.append_journal(
            "hit_suppressed",
            rule_name,
            serde_json::json!({
                "content_hash": row.content_hash,
                "excerpt": row.excerpt,
                "reason": row.reason,
            }),
        )?;
        Ok(row)
    }

    /// Withdraw a suppression by hash prefix or exact excerpt. Returns the row
    /// that stopped answering.
    pub fn unsuppress_hit(&self, rule_name: &str, key: &str) -> Result<HitAdjudication> {
        let row = self.resolve_hit_adjudication(rule_name, key)?;
        self.conn.execute(
            "DELETE FROM hit_adjudication WHERE rule_name=?1 AND content_hash=?2",
            params![rule_name, row.content_hash],
        )?;
        self.append_journal(
            "hit_unsuppressed",
            rule_name,
            serde_json::json!({
                "content_hash": row.content_hash,
                "excerpt": row.excerpt,
            }),
        )?;
        Ok(row)
    }

    /// Every suppression, optionally scoped to one rule — the auditable ledger
    /// behind `loom rule suppressions`.
    pub fn hit_adjudications(&self, rule: Option<&str>) -> Result<Vec<HitAdjudication>> {
        let mut stmt = match rule {
            Some(_) => self.conn.prepare(
                "SELECT rule_name, content_hash, excerpt, reason, actor, created_at
                 FROM hit_adjudication WHERE rule_name=?1
                 ORDER BY rule_name, created_at, content_hash",
            )?,
            None => self.conn.prepare(
                "SELECT rule_name, content_hash, excerpt, reason, actor, created_at
                 FROM hit_adjudication ORDER BY rule_name, created_at, content_hash",
            )?,
        };
        let rows: Result<Vec<_>, _> = match rule {
            Some(r) => stmt.query_map(params![r], row_to_adjudication)?.collect(),
            None => stmt.query_map([], row_to_adjudication)?.collect(),
        };
        Ok(rows?)
    }

    /// Does a recorded judgment answer this exact matched text for this rule?
    pub fn is_hit_suppressed(&self, rule_name: &str, excerpt: &str) -> Result<bool> {
        let hash = hit_hash(&crate::prescan::canonical_excerpt(excerpt));
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM hit_adjudication WHERE rule_name=?1 AND content_hash=?2",
            params![rule_name, hash],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    fn resolve_hit_adjudication(&self, rule_name: &str, key: &str) -> Result<HitAdjudication> {
        let rows = self.hit_adjudications(Some(rule_name))?;
        let matches: Vec<HitAdjudication> = rows
            .into_iter()
            .filter(|r| {
                r.content_hash.starts_with(key)
                    || r.excerpt == crate::prescan::canonical_excerpt(key)
            })
            .collect();
        match matches.len() {
            0 => Err(anyhow!(
                "no suppression on '{rule_name}' matches '{key}' — `loom rule suppressions {rule_name}`"
            )),
            1 => Ok(matches.into_iter().next().expect("one match")),
            n => bail!("'{key}' matches {n} suppressions on '{rule_name}' — give more of the hash"),
        }
    }
}

/// The identity of a hit: a fingerprint of its matched text. Position never
/// participates, so a shifted line or a moved file keeps the judgment.
pub(crate) fn hit_hash(canonical_excerpt: &str) -> String {
    crate::artifact::fingerprint(canonical_excerpt)
}

fn row_to_adjudication(r: &rusqlite::Row<'_>) -> rusqlite::Result<HitAdjudication> {
    Ok(HitAdjudication {
        rule_name: r.get(0)?,
        content_hash: r.get(1)?,
        excerpt: r.get(2)?,
        reason: r.get(3)?,
        actor: r.get(4)?,
        created_at: r.get(5)?,
    })
}
