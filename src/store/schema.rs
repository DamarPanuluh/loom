use crate::{
    Result, CRATE_VERSION, JOURNEY_SCHEMA_CUT, SCHEMA_VERSION, WRITER_SCHEMA_KEY,
    WRITER_VERSION_KEY,
};
use anyhow::{bail, Context};
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};

use super::SQLITE_BUSY_TIMEOUT_MS;

const SCHEMA: &str = r#"
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE node (
    id          TEXT PRIMARY KEY,
    node_type   TEXT NOT NULL,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    status      TEXT NOT NULL DEFAULT '',
    truth_class TEXT NOT NULL DEFAULT 'asserted' CHECK (truth_class IN ('derived','asserted')),
    body        TEXT NOT NULL DEFAULT '{}',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX idx_node_type ON node(node_type);
CREATE INDEX idx_node_name ON node(name);

CREATE TABLE edge (
    id           TEXT PRIMARY KEY,
    from_id      TEXT NOT NULL REFERENCES node(id) ON DELETE CASCADE,
    to_id        TEXT NOT NULL REFERENCES node(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL,
    truth_class  TEXT NOT NULL CHECK (truth_class IN ('derived','asserted')),
    status       TEXT NOT NULL DEFAULT 'uninspected',
    criterion    TEXT NOT NULL DEFAULT '',
    evidence     TEXT NOT NULL DEFAULT '',
    confidence   REAL NOT NULL DEFAULT 0,
    depends_on   TEXT NOT NULL DEFAULT '[]',
    inspected_by TEXT NOT NULL DEFAULT '',
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    UNIQUE (from_id, to_id, kind)
);
CREATE INDEX idx_edge_queue ON edge(truth_class, status);
CREATE INDEX idx_edge_kind  ON edge(kind, status);
CREATE INDEX idx_edge_from  ON edge(from_id, kind);
CREATE INDEX idx_edge_to    ON edge(to_id, kind);

CREATE TABLE facet (
    target_id   TEXT NOT NULL,
    target_kind TEXT NOT NULL CHECK (target_kind IN ('node','edge')),
    key         TEXT NOT NULL,
    value       TEXT NOT NULL,
    truth_class TEXT NOT NULL CHECK (truth_class IN ('derived','asserted')),
    PRIMARY KEY (target_id, target_kind, key)
);

CREATE TABLE tag (
    target_id   TEXT NOT NULL,
    target_kind TEXT NOT NULL CHECK (target_kind IN ('node','edge')),
    term        TEXT NOT NULL,
    PRIMARY KEY (target_id, target_kind, term)
);

CREATE TABLE tag_vocabulary (
    term        TEXT PRIMARY KEY,
    description TEXT NOT NULL DEFAULT '',
    created_at  TEXT NOT NULL
);
"#;

// ---- helpers -------------------------------------------------------------

/// Migration 3 — the evidence spine.
///
/// `fact` becomes the home of every asserted claim. Without it `assert_fact`
/// cannot be a chokepoint: adjudication and ratification lived in `facet`, and
/// `set_facet` is a public primitive every command reaches for directly.
///
/// The verdict columns leave `edge` and come back as a VIEW projected from the
/// fact. Readers keep working unchanged; writers lose the ability to set them at
/// all, because there is no longer a column to write.
///
/// This is a hardcut, not a data migration: every asserted verdict is reset,
/// because not one of them was anchored to anything loom could re-check. The
/// ladder going red afterwards is the point.
const MIGRATION_3_EVIDENCE_SPINE: &str = r#"
CREATE TABLE fact (
    id           TEXT PRIMARY KEY,
    subject_kind TEXT NOT NULL CHECK (subject_kind IN ('node','edge')),
    subject_id   TEXT NOT NULL,
    claim        TEXT NOT NULL CHECK (claim IN ('verdict','observation','adjudication','ratification')),
    state        TEXT NOT NULL,
    criterion    TEXT NOT NULL DEFAULT '',
    verification TEXT NOT NULL CHECK (verification IN ('verified','cited','claimed','expired')),
    confidence   REAL NOT NULL DEFAULT 0,
    asserted_by  TEXT NOT NULL DEFAULT '',
    asserted_at  TEXT NOT NULL,
    stale        TEXT NOT NULL DEFAULT '',
    UNIQUE (subject_kind, subject_id, claim)
);
CREATE INDEX idx_fact_subject      ON fact(subject_kind, subject_id);
CREATE INDEX idx_fact_verification ON fact(verification, claim);
CREATE INDEX idx_fact_claim_state  ON fact(claim, state);

CREATE TABLE evidence (
    id            TEXT PRIMARY KEY,
    fact_id       TEXT NOT NULL REFERENCES fact(id) ON DELETE CASCADE,
    payload       TEXT NOT NULL DEFAULT '{}',
    kind          TEXT NOT NULL CHECK (kind IN ('run','span','journal','claim')),
    recorded_at   TEXT NOT NULL,
    holds         INTEGER NOT NULL DEFAULT 1,
    expiry_reason TEXT NOT NULL DEFAULT ''
);
CREATE INDEX idx_evidence_fact  ON evidence(fact_id, kind);
CREATE INDEX idx_evidence_holds ON evidence(holds);

ALTER TABLE edge DROP COLUMN criterion;
ALTER TABLE edge DROP COLUMN evidence;
ALTER TABLE edge DROP COLUMN confidence;
ALTER TABLE edge DROP COLUMN inspected_by;

CREATE VIEW edge_view AS
SELECT e.id, e.from_id, e.to_id, e.kind, e.truth_class, e.status,
       e.depends_on, e.created_at, e.updated_at,
       COALESCE(f.criterion, '')   AS criterion,
       COALESCE(f.confidence, 0.0) AS confidence,
       COALESCE(f.asserted_by, '') AS inspected_by
FROM edge e
LEFT JOIN fact f
  ON f.subject_kind = 'edge' AND f.subject_id = e.id AND f.claim = 'verdict';

UPDATE edge SET status = 'uninspected' WHERE truth_class = 'asserted';
DELETE FROM facet WHERE key IN (
    'evidence_spans', 'stale_cause', 'adjudication',
    'ratification', 'ratified_by', 'ratified_at', 'ratified_presence'
);
"#;

/// Strip `proof_level` from validation bodies.
///
/// v3 made proof strength DERIVED — computed from what a proof actually does —
/// and deleted the flag that let a caller claim it. The stored values stayed
/// behind, so `loom validation show` printed `"proof_level":"L5"` in the body
/// directly above a derived `strength: S1`. Nothing reads it; it is inert data
/// that contradicts the number beside it, and it travels in every export.
const MIGRATION_4_DROP_CLAIMED_PROOF_LEVEL: &str = r#"
UPDATE node
   SET body = json_remove(body, '$.proof_level')
 WHERE node_type = 'validation'
   AND json_extract(body, '$.proof_level') IS NOT NULL;
"#;

/// Remove delegated ratification. Policy-authored facts cease to establish
/// wantedness, and their evidence is removed by the fact foreign key cascade.
/// Journal entries are intentionally append-only and live outside SQLite, so
/// the historical acts remain available to audit.
const MIGRATION_5_DROP_RATIFY_POLICIES: &str = r#"
DELETE FROM meta WHERE key = 'ratify_policies';
DELETE FROM fact
 WHERE claim = 'ratification'
   AND asserted_by LIKE 'policy:%';
"#;

const MIGRATION_7_BATCH_AUTH: &str = r#"
ALTER TABLE fact ADD COLUMN decision_mode TEXT NOT NULL DEFAULT 'individual';
ALTER TABLE fact ADD COLUMN batch_id TEXT NOT NULL DEFAULT '';
"#;

/// Migration 8 — hit-level adjudication. A suppression's key is the content
/// hash of the matched text, so it answers the same text wherever it moves and
/// expires by construction when the text changes.
const MIGRATION_8_HIT_ADJUDICATION: &str = r#"
CREATE TABLE hit_adjudication (
    rule_name    TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    excerpt      TEXT NOT NULL,
    reason       TEXT NOT NULL,
    actor        TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    PRIMARY KEY (rule_name, content_hash)
);
"#;

/// Migration 9 — the judgment inbox. Human-only judgments (ratify, reject)
/// and redefinitions get a staged proposal object: an LLM discovers the
/// candidate and files it with evidence; the human reviews a digest and
/// confirms each through the SAME typed challenge the direct command would
/// demand. Authority is unchanged — the inbox holds recommendations, never
/// decisions.
const MIGRATION_9_JUDGMENT_INBOX: &str = r#"
CREATE TABLE judgment_proposal (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL CHECK (kind IN ('ratify','reject','redefine')),
    intent_id  TEXT NOT NULL,
    evidence   TEXT NOT NULL,
    detail     TEXT NOT NULL DEFAULT '',
    staged_by  TEXT NOT NULL,
    staged_at  TEXT NOT NULL,
    state      TEXT NOT NULL DEFAULT 'staged' CHECK (state IN ('staged','confirmed','withdrawn')),
    decided_at TEXT NOT NULL DEFAULT ''
);
CREATE INDEX idx_judgment_state ON judgment_proposal(state, kind);
"#;

/// Preserve the executor profile independently from write authority. Existing
/// facts predate profile capture and correctly remain empty rather than being
/// backfilled with an invented worker identity.
const MIGRATION_10_EXECUTOR_PROFILE: &str = r#"
ALTER TABLE fact ADD COLUMN asserted_profile TEXT NOT NULL DEFAULT '';
"#;

/// Normalize absent executor attribution to SQL NULL. V10 used an empty-string
/// sentinel; preserve real values while removing that representational split.
const MIGRATION_11_NULLABLE_EXECUTOR_PROFILE: &str = r#"
ALTER TABLE fact ADD COLUMN asserted_profile_nullable TEXT;
UPDATE "fact" SET asserted_profile_nullable = NULLIF(asserted_profile, '');
ALTER TABLE fact DROP COLUMN asserted_profile;
ALTER TABLE fact RENAME COLUMN asserted_profile_nullable TO asserted_profile;
"#;

/// Migration 13 — durable adversarial challenge facts and fact-snapshot
/// evidence. SQLite cannot extend a CHECK constraint in place, so both tables
/// are rebuilt while preserving every existing row and the edge projection.
const MIGRATION_13_ADVERSARIAL_REVIEW: &str = r#"
DROP VIEW edge_view;

CREATE TABLE fact_v13 (
    id               TEXT PRIMARY KEY,
    subject_kind     TEXT NOT NULL CHECK (subject_kind IN ('node','edge')),
    subject_id       TEXT NOT NULL,
    claim            TEXT NOT NULL CHECK (claim IN ('verdict','observation','adjudication','ratification','challenge')),
    state            TEXT NOT NULL,
    criterion        TEXT NOT NULL DEFAULT '',
    verification     TEXT NOT NULL CHECK (verification IN ('verified','cited','claimed','expired')),
    confidence       REAL NOT NULL DEFAULT 0,
    asserted_by      TEXT NOT NULL DEFAULT '',
    asserted_at      TEXT NOT NULL,
    stale            TEXT NOT NULL DEFAULT '',
    decision_mode    TEXT NOT NULL DEFAULT 'individual',
    batch_id         TEXT NOT NULL DEFAULT '',
    asserted_profile TEXT,
    UNIQUE (subject_kind, subject_id, claim)
);

CREATE TABLE evidence_v13 (
    id            TEXT PRIMARY KEY,
    fact_id       TEXT NOT NULL REFERENCES fact_v13(id) ON DELETE CASCADE,
    payload       TEXT NOT NULL DEFAULT '{}',
    kind          TEXT NOT NULL CHECK (kind IN ('run','span','journal','claim','fact_snapshot')),
    recorded_at   TEXT NOT NULL,
    holds         INTEGER NOT NULL DEFAULT 1,
    expiry_reason TEXT NOT NULL DEFAULT ''
);

INSERT INTO "fact_v13"
SELECT id,subject_kind,subject_id,claim,state,criterion,verification,confidence,
       asserted_by,asserted_at,stale,decision_mode,batch_id,asserted_profile
  FROM fact;
INSERT INTO "evidence_v13"
SELECT id,fact_id,payload,kind,recorded_at,holds,expiry_reason FROM evidence;

DROP TABLE evidence;
DROP TABLE fact;
ALTER TABLE fact_v13 RENAME TO fact;
ALTER TABLE evidence_v13 RENAME TO evidence;

CREATE INDEX idx_fact_subject      ON fact(subject_kind, subject_id);
CREATE INDEX idx_fact_verification ON fact(verification, claim);
CREATE INDEX idx_fact_claim_state  ON fact(claim, state);
CREATE INDEX idx_evidence_fact     ON evidence(fact_id, kind);
CREATE INDEX idx_evidence_holds    ON evidence(holds);

CREATE VIEW edge_view AS
SELECT e.id, e.from_id, e.to_id, e.kind, e.truth_class, e.status,
       e.depends_on, e.created_at, e.updated_at,
       COALESCE(f.criterion, '')   AS criterion,
       COALESCE(f.confidence, 0.0) AS confidence,
       COALESCE(f.asserted_by, '') AS inspected_by
FROM edge e
LEFT JOIN fact f
  ON f.subject_kind = 'edge' AND f.subject_id = e.id AND f.claim = 'verdict';
"#;

fn schema_migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(SCHEMA),
        M::up(
            "CREATE INDEX IF NOT EXISTS idx_tag_term ON tag(term);
             CREATE INDEX IF NOT EXISTS idx_facet_key_value ON facet(key, value);",
        ),
        M::up(MIGRATION_3_EVIDENCE_SPINE),
        M::up(MIGRATION_4_DROP_CLAIMED_PROOF_LEVEL),
        M::up(MIGRATION_5_DROP_RATIFY_POLICIES),
        M::up("SELECT 1;"),
        M::up(MIGRATION_7_BATCH_AUTH),
        M::up(MIGRATION_8_HIT_ADJUDICATION),
        M::up(MIGRATION_9_JUDGMENT_INBOX),
        M::up(MIGRATION_10_EXECUTOR_PROFILE),
        M::up(MIGRATION_11_NULLABLE_EXECUTOR_PROFILE),
        // V12 is a semantic hard cut: the SQLite shape is unchanged, but old
        // graph vocabularies cannot be interpreted under the journey-root model.
        M::up("SELECT 1;"),
        M::up(MIGRATION_13_ADVERSARIAL_REVIEW),
    ])
}

fn persisted_meta_schema_version(conn: &Connection) -> Result<Option<u32>> {
    let has_meta = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta'",
            [],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !has_meta {
        return Ok(None);
    }
    conn.query_row(
        "SELECT value FROM meta WHERE key='schema_version'",
        [],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|raw| {
        raw.parse::<u32>()
            .with_context(|| format!("invalid persisted schema_version '{raw}'"))
    })
    .transpose()
}

/// Refuse incompatible persisted graphs before configuration or migration can
/// mutate their database. Version zero with no meta stamp is a fresh database;
/// version zero with a v12 meta stamp is the old-style current stamp adopted
/// below. Every genuine v1-v11 graph must be rebuilt under the journey-root
/// vocabulary rather than translated into a graph that claims new semantics.
///
/// Behind-schema graphs at or above [`JOURNEY_SCHEMA_CUT`] are allowed through
/// so [`apply_schema_migrations`] can raise them — possibly with consent.
fn meta_opt(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |row| {
        row.get::<_, String>(0)
    })
    .optional()
    .ok()
    .flatten()
}

/// Parse `major.minor.patch` into a comparable tuple; `None` if it is not that
/// shape. Owned here because the schema plane is what GATES on the comparison
/// (migration consent below); `commands::warn_on_writer_drift` reads the same
/// stamps to warn, and used to carry a byte-identical private copy — two
/// answers to "is this binary older than the graph's last writer" that could
/// disagree about what parses.
pub(crate) fn parse_crate_version(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value.split('.').map(|part| part.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// Auto-migrate only when this binary is a newer *crate* than the last writer.
/// Same crate version + higher schema is an unreleased-branch leak. Missing or
/// unparsable writer stamps also require consent: we cannot prove this is a
/// release upgrade. Fresh DBs (`persisted_schema == 0`) and already-current
/// graphs do not migrate.
pub(crate) fn schema_migration_requires_consent(
    persisted_schema: u32,
    binary_schema: u32,
    writer_crate: Option<&str>,
    binary_crate: &str,
) -> bool {
    if persisted_schema == 0 || persisted_schema >= binary_schema {
        return false;
    }
    !matches!(
        (
            parse_crate_version(writer_crate.unwrap_or("")),
            parse_crate_version(binary_crate),
        ),
        (Some(writer), Some(binary)) if binary > writer
    )
}

pub(crate) fn ahead_schema_error(
    graph_schema: u32,
    binary_schema: u32,
    binary_crate: &str,
    writer_crate: Option<&str>,
    writer_schema: Option<&str>,
) -> String {
    let writer_bits = match (writer_crate, writer_schema) {
        (Some(c), Some(s)) => format!("Last writer: loom {c} (schema v{s})."),
        (Some(c), None) => format!("Last writer: loom {c} (writer schema unstamped)."),
        (None, Some(s)) => format!("Last writer crate unstamped (schema v{s})."),
        (None, None) => "Last writer: unknown.".to_string(),
    };
    let same_crate = writer_crate == Some(binary_crate);
    if same_crate {
        format!(
            "this graph is v{graph_schema}; this loom understands v{binary_schema}. \
             {writer_bits} This binary is also loom {binary_crate} — a same-version \
             schema fork, not a newer release. There is no downgrade. \
             Reinstalling {binary_crate} will not help. Restore a pre-migration \
             backup, or use the build that understands v{graph_schema}. \
             The graph is untouched."
        )
    } else {
        format!(
            "this graph is v{graph_schema}; this loom understands v{binary_schema}. \
             {writer_bits} It was written by a newer loom — upgrade this binary \
             (see README) to a build that understands v{graph_schema}. \
             There is no downgrade. The graph is untouched."
        )
    }
}

pub(crate) fn ensure_supported_persisted_schema(conn: &Connection) -> Result<()> {
    let user_version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let meta_version = persisted_meta_schema_version(conn)?;
    for version in [(user_version != 0).then_some(user_version), meta_version]
        .into_iter()
        .flatten()
    {
        if version < JOURNEY_SCHEMA_CUT {
            bail!(
                "this graph is v{version}; loom v12 introduced the journey paradigm — re-init \
                 and rebuild (loom bootstrap suggest, author journeys, loom journey derive). \
                 The graph is untouched."
            );
        }
        if version > SCHEMA_VERSION {
            let writer_crate = meta_opt(conn, WRITER_VERSION_KEY);
            let writer_schema = meta_opt(conn, WRITER_SCHEMA_KEY);
            bail!(
                "{}",
                ahead_schema_error(
                    version,
                    SCHEMA_VERSION,
                    CRATE_VERSION,
                    writer_crate.as_deref(),
                    writer_schema.as_deref(),
                )
            );
        }
    }
    Ok(())
}

pub(crate) fn apply_schema_migrations(conn: &mut Connection) -> Result<()> {
    ensure_supported_persisted_schema(conn)?;
    adopt_legacy_schema_version(conn)?;
    let from_version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let writer_crate = meta_opt(conn, WRITER_VERSION_KEY);
    let requires_consent = schema_migration_requires_consent(
        from_version,
        SCHEMA_VERSION,
        writer_crate.as_deref(),
        CRATE_VERSION,
    );
    let consented = std::env::var("LOOM_SCHEMA_MIGRATE").ok().as_deref() == Some("1");
    if from_version < SCHEMA_VERSION && from_version != 0 && requires_consent && !consented {
        bail!(
            "refusing to migrate graph schema v{from_version} → v{SCHEMA_VERSION}: this binary \
             is still loom {CRATE_VERSION} (same crate as the last writer, or writer unknown). \
             An unreleased branch with a higher schema would rewrite the graph in place. \
             Set LOOM_SCHEMA_MIGRATE=1 to consent, or use a release that bumped the crate \
             version. The graph is untouched."
        );
    }
    if from_version < SCHEMA_VERSION && from_version != 0 {
        let writer = writer_crate.as_deref().unwrap_or("unknown");
        if consented && requires_consent {
            eprintln!(
                "loom: migrating graph schema v{from_version} → v{SCHEMA_VERSION} \
                 (consented via LOOM_SCHEMA_MIGRATE=1; last writer {writer})"
            );
        } else {
            eprintln!(
                "loom: migrating graph schema v{from_version} → v{SCHEMA_VERSION} \
                 (last writer loom {writer} → {CRATE_VERSION})"
            );
        }
    }
    schema_migrations()
        .to_latest(conn)
        .context("migrating graph schema")?;
    let foreign_key_issue: Option<(String, i64, String)> = conn
        .query_row(
            "SELECT \"table\", rowid, parent FROM pragma_foreign_key_check LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if let Some((table, rowid, parent)) = foreign_key_issue {
        bail!(
            "graph migration left a foreign-key violation: table={table} rowid={rowid} parent={parent}"
        );
    }
    // Keep the portable identity stamp aligned with the migration counter when
    // meta already exists (re-open / upgrade). Fresh init inserts schema_version
    // itself after this returns.
    let user_version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let has_schema_meta = conn
        .query_row("SELECT 1 FROM meta WHERE key='schema_version'", [], |_| {
            Ok(true)
        })
        .optional()?
        .unwrap_or(false);
    if has_schema_meta {
        conn.execute(
            "UPDATE meta SET value=?1 WHERE key='schema_version'",
            params![user_version.to_string()],
        )?;
    }
    Ok(())
}

fn adopt_legacy_schema_version(conn: &Connection) -> Result<()> {
    let user_version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if user_version != 0 {
        return Ok(());
    }

    let has_meta = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta'",
            [],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !has_meta {
        return Ok(());
    }

    let legacy_schema_version = conn
        .query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?;

    let legacy_schema_version = legacy_schema_version
        .map(|raw| {
            raw.parse::<u32>()
                .with_context(|| format!("invalid persisted schema_version '{raw}'"))
        })
        .transpose()?;

    if let Some(legacy) = legacy_schema_version {
        if (JOURNEY_SCHEMA_CUT..=SCHEMA_VERSION).contains(&legacy) {
            conn.pragma_update(None, "user_version", legacy)?;
        }
    }
    Ok(())
}

pub(crate) fn configure(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", SQLITE_BUSY_TIMEOUT_MS as i64)?;
    Ok(())
}

/// Connection setup for a read-only open. Sets the busy timeout and enforces
/// `query_only`, so a mis-routed read command fails loudly instead of writing.
/// Deliberately does NOT set `journal_mode` (a write) or run migrations — a read
/// open never mutates the file.
pub(crate) fn configure_read(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "busy_timeout", SQLITE_BUSY_TIMEOUT_MS as i64)?;
    conn.pragma_update(None, "query_only", true)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use crate::store::{Assertion, Store, Subject};
    use crate::testutil::TmpRoot;
    use crate::{
        CRATE_VERSION, GRAPH_DB, LOOM_DIR, SCHEMA_VERSION, WRITER_SCHEMA_KEY, WRITER_VERSION_KEY,
    };
    use rusqlite::{params, Connection};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn sqlite_user_version(conn: &Connection) -> u32 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn graph_schema_migrations_are_valid() {
        schema_migrations().validate().unwrap();
    }

    #[test]
    fn fresh_init_sets_sqlite_user_version() {
        let tmp = TmpRoot::new("loom-store-fresh-migration");
        let store = Store::init(tmp.path(), Some("fresh"), false).unwrap();
        assert_eq!(sqlite_user_version(&store.conn), SCHEMA_VERSION);
        assert_eq!(store.identity().unwrap().schema_version, SCHEMA_VERSION);
        let asserted_profile_not_null: i64 = store
            .conn
            .query_row(
                "SELECT \"notnull\" FROM pragma_table_info('fact') WHERE name = 'asserted_profile'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            asserted_profile_not_null, 0,
            "absent executor attribution must be represented as SQL NULL"
        );
    }

    #[test]
    fn v12_store_migrates_in_place_to_v13_without_losing_fact_evidence() {
        let tmp = TmpRoot::new("loom-store-v12-to-v13");
        let store = Store::init(tmp.path(), Some("v12-upgrade"), false).unwrap();
        let finding = store
            .add_node(
                NodeType::Finding,
                "preserve this observation",
                "migration must keep it",
                "code_audit",
                serde_json::json!({
                    "kind": "code_audit",
                    "source": "code_audit",
                    "evidence": "migration fixture",
                    "impact": "loss would corrupt history",
                    "confidence": 0.8,
                    "link": "migration-fixture"
                }),
            )
            .unwrap();
        let fact = store
            .assert_fact(
                crate::store::Assertion::new(
                    crate::store::Subject::Node(finding.id.clone()),
                    Claim::Observation,
                    "observed",
                    "solo",
                )
                .confidence(0.8)
                .cited(vec![crate::evidence::CitedEvidence::Claim(
                    "migration fixture".into(),
                )]),
            )
            .unwrap();
        let evidence_id = fact.evidence[0].id.clone();
        store
            .conn
            .pragma_update(None, "user_version", 12u32)
            .unwrap();
        store
            .conn
            .execute("UPDATE meta SET value='12' WHERE key='schema_version'", [])
            .unwrap();
        // Simulate a real release upgrade (crate 0.34.0 → 0.34.1), not a
        // same-crate feature-branch leak. Same-crate 12→13 is refused below.
        store.set_meta(WRITER_VERSION_KEY, "0.34.0").unwrap();
        store.set_meta(WRITER_SCHEMA_KEY, "12").unwrap();
        drop(store);

        let read_error = match Store::open_read(tmp.path()) {
            Ok(_) => panic!("read-only v12 open must require migration"),
            Err(error) => error.to_string(),
        };
        assert!(read_error.contains("write-capable"), "{read_error}");

        let store = Store::open(tmp.path()).unwrap();
        assert_eq!(sqlite_user_version(&store.conn), 13);
        assert_eq!(store.identity().unwrap().schema_version, 13);
        assert!(store.fact_by_id(&fact.fact.id).unwrap().is_some());
        let evidence_count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM evidence WHERE id=?1",
                [&evidence_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(evidence_count, 1);
        let fact_sql: String = store
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='fact'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let evidence_sql: String = store
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='evidence'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(fact_sql.contains("'challenge'"));
        assert!(evidence_sql.contains("'fact_snapshot'"));
        let fk_issues: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(fk_issues, 0);
    }

    #[test]
    fn v12_store_refuses_same_crate_migrate_without_consent() {
        let tmp = TmpRoot::new("loom-store-v12-same-crate-consent");
        let store = Store::init(tmp.path(), Some("v12-consent"), false).unwrap();
        store
            .conn
            .pragma_update(None, "user_version", 12u32)
            .unwrap();
        store
            .conn
            .execute("UPDATE meta SET value='12' WHERE key='schema_version'", [])
            .unwrap();
        drop(store);

        let error = match Store::open(tmp.path()) {
            Ok(_) => panic!("same-crate 12→13 must not migrate in place"),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains("LOOM_SCHEMA_MIGRATE"),
            "refusal must name the consent flag: {error}"
        );
        let conn = Connection::open(tmp.path().join(LOOM_DIR).join(GRAPH_DB)).unwrap();
        assert_eq!(
            sqlite_user_version(&conn),
            12,
            "refused migration must leave the graph untouched"
        );
    }

    #[test]
    fn pre_v12_graph_is_refused_without_changing_its_stamps() {
        let tmp = TmpRoot::new("loom-store-v11-hard-cut");
        {
            let store = Store::init(tmp.path(), Some("old"), false).unwrap();
            drop(store);
        }
        let db = tmp.path().join(crate::LOOM_DIR).join(crate::GRAPH_DB);
        let conn = Connection::open(&db).unwrap();
        conn.pragma_update(None, "user_version", 11u32).unwrap();
        conn.execute("UPDATE meta SET value='11' WHERE key='schema_version'", [])
            .unwrap();
        drop(conn);

        for error in [
            Store::open_read(tmp.path())
                .err()
                .expect("read open refuses"),
            Store::open(tmp.path()).err().expect("write open refuses"),
            Store::init(tmp.path(), None, false)
                .err()
                .expect("idempotent init refuses"),
        ] {
            let message = error.to_string();
            assert!(message.contains("journey paradigm"), "{message}");
            assert!(message.contains("re-init and rebuild"), "{message}");
        }

        let conn = Connection::open(&db).unwrap();
        assert_eq!(sqlite_user_version(&conn), 11);
        let meta: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(meta, "11");
    }

    #[test]
    fn migration_removes_policy_ratification_and_evidence_but_keeps_journal() {
        let tmp = TmpRoot::new("loom-store-drop-ratify-policy");
        let store = Store::init(tmp.path(), Some("legacy-policy"), false).unwrap();
        let intent = store
            .add_node(
                NodeType::Intent,
                "delegated behavior",
                "a policy had approved this behavior",
                "planned",
                serde_json::json!({}),
            )
            .unwrap();
        let journal = store
            .append_journal(
                "ratification",
                &intent.id,
                serde_json::json!({ "ratified_by": "policy:legacy" }),
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO meta(key,value) VALUES ('ratify_policies','[]')",
                [],
            )
            .unwrap();
        let insert_fact = concat!(
            "INSERT INTO ",
            "fact(id,subject_kind,subject_id,claim,state,criterion,verification,confidence,asserted_by,asserted_at,stale) ",
            "VALUES ('policy-fact','node',?1,'ratification','ratified','policy approval','cited',1.0,'policy:legacy','2026-01-01T00:00:00Z','')"
        );
        store.conn.execute(insert_fact, [&intent.id]).unwrap();
        store
            .conn
            .execute(
                concat!(
                    "INSERT INTO ",
                    "evidence(id,fact_id,payload,kind,recorded_at,holds,expiry_reason) ",
                    "VALUES ('policy-evidence','policy-fact','{}','claim','2026-01-01T00:00:00Z',1,'')"
                ),
                [],
            )
            .unwrap();
        // Rewind to a faithful v4 graph: the version stamp AND the columns
        // later migrations added, or migration 7 re-adds what is already
        // there and the simulation fails on its own bookkeeping.
        store
            .conn
            .execute_batch(
                "ALTER TABLE fact DROP COLUMN asserted_profile;
                 ALTER TABLE fact DROP COLUMN decision_mode;
                 ALTER TABLE fact DROP COLUMN batch_id;
                 DROP TABLE hit_adjudication;
                 DROP TABLE judgment_proposal;",
            )
            .unwrap();
        store
            .conn
            .pragma_update(None, "user_version", 4u32)
            .unwrap();
        drop(store);

        // Public Store opens must refuse v4 under the v12 hard cut. Exercise
        // the preserved historical migration directly: those steps exist to
        // build fresh databases and remain independently valid, not to upgrade
        // persisted pre-journey graphs.
        let db = tmp.path().join(LOOM_DIR).join(GRAPH_DB);
        let mut conn = Connection::open(db).unwrap();
        schema_migrations().to_latest(&mut conn).unwrap();
        drop(conn);

        let store = Store::open(tmp.path()).unwrap();
        assert_eq!(store.ratification(&intent.id).unwrap(), "unratified");
        assert_eq!(store.get_meta("ratify_policies").unwrap(), None);
        let evidence_count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM evidence WHERE id='policy-evidence'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(evidence_count, 0, "fact evidence must cascade away");
        assert!(
            crate::journal::exists(tmp.path(), &journal.id).unwrap(),
            "migration must preserve append-only history"
        );
    }

    #[test]
    fn old_style_schema_is_adopted_without_rerunning_create_table() {
        let tmp = TmpRoot::new("loom-store-legacy-migration");
        let loom_dir = tmp.path().join(LOOM_DIR);
        std::fs::create_dir_all(&loom_dir).unwrap();
        let db_path = loom_dir.join(GRAPH_DB);
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO meta(key,value) VALUES
                 ('graph_id','legacy'),
                 ('name','legacy'),
                 ('schema_version',?1),
                 ('observed','0'),
                 ('created_at','legacy')",
                params![SCHEMA_VERSION.to_string()],
            )
            .unwrap();
            assert_eq!(sqlite_user_version(&conn), 0);
        }

        let store = Store::open(tmp.path()).unwrap();
        assert_eq!(sqlite_user_version(&store.conn), SCHEMA_VERSION);
        assert_eq!(store.identity().unwrap().name, "legacy");
    }

    #[test]
    fn malformed_legacy_schema_version_is_reported() {
        let tmp = TmpRoot::new("loom-store-invalid-legacy-version");
        let loom_dir = tmp.path().join(LOOM_DIR);
        std::fs::create_dir_all(&loom_dir).unwrap();
        let db_path = loom_dir.join(GRAPH_DB);
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO meta(key,value) VALUES ('schema_version','not-a-number')",
            [],
        )
        .unwrap();
        drop(conn);

        let error = Store::open(tmp.path())
            .err()
            .expect("malformed legacy schema version must fail open");
        assert!(
            error
                .to_string()
                .contains("invalid persisted schema_version"),
            "corrupt schema metadata must be surfaced: {error}"
        );
    }

    #[test]
    fn same_crate_schema_bump_requires_consent() {
        assert!(
            schema_migration_requires_consent(12, 13, Some("0.34.1"), "0.34.1"),
            "unreleased branch at the same crate version must not silently migrate"
        );
        assert!(
            schema_migration_requires_consent(12, 13, None, "0.34.1"),
            "missing writer stamp cannot be assumed to be a release upgrade"
        );
        assert!(
            !schema_migration_requires_consent(12, 13, Some("0.34.1"), "0.35.0"),
            "a crate bump is a real release upgrade"
        );
        assert!(
            !schema_migration_requires_consent(0, 13, None, "0.34.1"),
            "fresh databases must still run to_latest"
        );
        assert!(
            !schema_migration_requires_consent(12, 12, Some("0.34.1"), "0.34.1"),
            "already-current graphs do not migrate"
        );
    }

    #[test]
    fn ahead_schema_error_names_the_fork_when_crate_matches() {
        let fork = ahead_schema_error(13, 12, "0.34.1", Some("0.34.1"), Some("13"));
        assert!(fork.contains("same-version"), "{fork}");
        assert!(fork.contains("will not help"), "{fork}");
        assert!(fork.contains("no downgrade"), "{fork}");
        assert!(
            !fork.contains("upgrade this binary"),
            "same-crate fork must not instruct an impossible upgrade: {fork}"
        );

        let upgrade = ahead_schema_error(13, 12, "0.34.1", Some("0.35.0"), Some("13"));
        assert!(
            upgrade.contains("newer loom") && upgrade.contains("upgrade"),
            "{upgrade}"
        );
        assert!(upgrade.contains("no downgrade"), "{upgrade}");
    }
}
