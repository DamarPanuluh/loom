//! Judgment-proposal persistence — the LLM-proposal inbox for human-only
//! judgments.
//!
//! Plane: engine (persistence). A proposal is a RECOMMENDATION, never a
//! decision: staging one writes nothing gated (INV-8 is untouched — the
//! human's typed challenge still happens at confirm time, through the same
//! `ratify_intent_from_human` / `reject_intent_from_human` chokepoints the
//! direct commands use). What the inbox removes is the pile: candidates the
//! LLM discovered over days arrive as one auditable digest instead of an
//! undifferentiated terminal session, and junk intents stop squatting in
//! work queues while their rejection waits to be remembered.

use super::*;
use std::fmt;
use std::str::FromStr;

/// The decision a proposal recommends. These spellings are also the durable
/// SQLite values and the CLI/JSON wire values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum JudgmentKind {
    #[serde(rename = "ratify")]
    Ratify,
    #[serde(rename = "reject")]
    Reject,
    #[serde(rename = "redefine")]
    Redefine,
}

impl JudgmentKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ratify => "ratify",
            Self::Reject => "reject",
            Self::Redefine => "redefine",
        }
    }
}

impl fmt::Display for JudgmentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for JudgmentKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "ratify" => Ok(Self::Ratify),
            "reject" => Ok(Self::Reject),
            "redefine" => Ok(Self::Redefine),
            other => bail!("judgment kind must be ratify|reject|redefine, got '{other}'"),
        }
    }
}

/// Lifecycle state of a judgment proposal. These spellings are also the
/// durable SQLite values and the JSON wire values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum JudgmentState {
    #[serde(rename = "staged")]
    Staged,
    #[serde(rename = "confirmed")]
    Confirmed,
    #[serde(rename = "withdrawn")]
    Withdrawn,
}

impl JudgmentState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Confirmed => "confirmed",
            Self::Withdrawn => "withdrawn",
        }
    }
}

impl fmt::Display for JudgmentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for JudgmentState {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "staged" => Ok(Self::Staged),
            "confirmed" => Ok(Self::Confirmed),
            "withdrawn" => Ok(Self::Withdrawn),
            other => bail!("judgment state must be staged|confirmed|withdrawn, got '{other}'"),
        }
    }
}

/// One staged judgment proposal.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JudgmentProposal {
    pub id: String,
    pub kind: JudgmentKind,
    pub intent_id: String,
    /// Why the proposer believes the judgment holds (reject reason / ratify
    /// evidence / redefine rationale).
    pub evidence: String,
    /// The replacement statement — only meaningful for `redefine`.
    pub detail: String,
    pub staged_by: String,
    pub staged_at: String,
    pub state: JudgmentState,
    pub decided_at: String,
}

impl Store {
    /// Stage a proposal. Not gated: recommending is not deciding. The
    /// caller validates the target intent and the substantive evidence.
    pub fn stage_judgment(
        &self,
        kind: JudgmentKind,
        intent_id: &str,
        evidence: &str,
        detail: &str,
        staged_by: &str,
    ) -> Result<JudgmentProposal> {
        // A live staged proposal for the same judgment on the same intent is
        // one inbox entry, not two — the pile is what this exists to end.
        if let Some(existing) = self
            .conn
            .query_row(
                "SELECT id FROM judgment_proposal
                 WHERE kind=?1 AND intent_id=?2 AND state='staged'",
                params![kind.as_str(), intent_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            bail!(
                "a {kind} proposal for this intent is already staged [{}] — \
                 confirm or withdraw it first (loom judgment digest)",
                crate::model::short(&existing)
            );
        }
        let (id, now) = id_and_now(&self.conn)?;
        self.conn.execute(
            "INSERT INTO judgment_proposal(id,kind,intent_id,evidence,detail,staged_by,staged_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                id,
                kind.as_str(),
                intent_id,
                evidence,
                detail,
                staged_by,
                now
            ],
        )?;
        self.get_judgment(&id)?
            .ok_or_else(|| anyhow!("judgment proposal vanished after insert"))
    }

    pub fn get_judgment(&self, id: &str) -> Result<Option<JudgmentProposal>> {
        self.conn
            .query_row(
                "SELECT id,kind,intent_id,evidence,detail,staged_by,staged_at,state,decided_at
                 FROM judgment_proposal WHERE id=?1",
                params![id],
                row_to_judgment,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Resolve by exact id or unique id-prefix — the 8-char ids `digest`
    /// prints are enough to act on; ambiguity errors with the count.
    pub fn resolve_judgment(&self, key: &str) -> Result<JudgmentProposal> {
        if let Some(p) = self.get_judgment(key)? {
            return Ok(p);
        }
        let mut stmt = self.conn.prepare(
            "SELECT id,kind,intent_id,evidence,detail,staged_by,staged_at,state,decided_at
             FROM judgment_proposal WHERE id LIKE ?1 ORDER BY id",
        )?;
        let mut matches = stmt
            .query_map(params![format!("{key}%")], row_to_judgment)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        match matches.len() {
            0 => bail!("no judgment proposal matches '{key}'"),
            1 => matches
                .pop()
                .ok_or_else(|| anyhow!("len == 1 but proposal vector empty")),
            n => bail!("ambiguous judgment proposal prefix '{key}': {n} match"),
        }
    }

    /// Proposals in one state (or all, for audit), oldest first — the digest
    /// reads staged; a reviewer auditing history reads the rest.
    pub fn list_judgments(&self, state: Option<JudgmentState>) -> Result<Vec<JudgmentProposal>> {
        let (sql, args): (&str, Vec<String>) = match state {
            Some(s) => (
                "SELECT id,kind,intent_id,evidence,detail,staged_by,staged_at,state,decided_at
                 FROM judgment_proposal WHERE state=?1 ORDER BY staged_at, id",
                vec![s.as_str().to_string()],
            ),
            None => (
                "SELECT id,kind,intent_id,evidence,detail,staged_by,staged_at,state,decided_at
                 FROM judgment_proposal ORDER BY staged_at, id",
                Vec::new(),
            ),
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args), row_to_judgment)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn count_judgments(&self, state: JudgmentState) -> Result<usize> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM judgment_proposal WHERE state=?1",
            params![state.as_str()],
            |r| r.get::<_, i64>(0),
        )? as usize)
    }

    /// Mark a proposal decided. Only a `staged` proposal can transition —
    /// confirm and withdraw both fail closed on an already-decided row, so a
    /// double-confirm cannot replay the gated write under a second decision.
    pub fn decide_judgment(&self, id: &str, state: JudgmentState) -> Result<()> {
        if state == JudgmentState::Staged {
            bail!("judgment state must be confirmed|withdrawn, got '{state}'");
        }
        let now = crate::journal::now_iso();
        let n = self.conn.execute(
            "UPDATE judgment_proposal SET state=?1, decided_at=?2 WHERE id=?3 AND state='staged'",
            params![state.as_str(), now, id],
        )?;
        if n == 0 {
            bail!("judgment proposal '{id}' is not staged (already decided?)");
        }
        Ok(())
    }
}

fn row_to_judgment(r: &rusqlite::Row) -> rusqlite::Result<JudgmentProposal> {
    let kind_raw: String = r.get(1)?;
    let kind = kind_raw.parse().map_err(|error: anyhow::Error| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error.to_string(),
            )),
        )
    })?;
    let state_raw: String = r.get(7)?;
    let state = state_raw.parse().map_err(|error: anyhow::Error| {
        rusqlite::Error::FromSqlConversionFailure(
            7,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error.to_string(),
            )),
        )
    })?;
    Ok(JudgmentProposal {
        id: r.get(0)?,
        kind,
        intent_id: r.get(2)?,
        evidence: r.get(3)?,
        detail: r.get(4)?,
        staged_by: r.get(5)?,
        staged_at: r.get(6)?,
        state,
        decided_at: r.get(8)?,
    })
}
