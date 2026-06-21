use super::SqliteGraphStore;
use super::*;

fn create_table_batch() -> &'static str {
    r#"
CREATE TABLE IF NOT EXISTS meta(
  id INTEGER PRIMARY KEY CHECK(id = 1),
  schema_version TEXT NOT NULL,
  graph_id TEXT NOT NULL,
  graph_name TEXT NOT NULL,
  custody TEXT NOT NULL CHECK(custody IN ('owned','observed','')),
  created_at TEXT NOT NULL DEFAULT '',
  last_synced TEXT NOT NULL DEFAULT '',
  transition_cap TEXT NOT NULL DEFAULT '',
  layer_order TEXT NOT NULL CHECK(json_valid(layer_order))
);

CREATE TABLE IF NOT EXISTS intent(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL,
  criterion TEXT NOT NULL DEFAULT '',
  abstraction_level TEXT NOT NULL,
  domain TEXT NOT NULL DEFAULT '',
  layer TEXT NOT NULL DEFAULT '',
  source_refs TEXT NOT NULL CHECK(json_valid(source_refs)),
  status TEXT NOT NULL,
  aspect TEXT NOT NULL DEFAULT '',
  lifecycle TEXT NOT NULL CHECK(lifecycle IN ('planned','implemented','needs_change','deferred','to_be_removed')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  tags TEXT NOT NULL CHECK(json_valid(tags)),
  visibility TEXT NOT NULL DEFAULT '' CHECK(visibility IN ('user_visible','internal','')),
  boundary TEXT NOT NULL DEFAULT '' CHECK(boundary IN ('inbound','outbound',''))
);

CREATE TABLE IF NOT EXISTS codefile(
  id TEXT PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  language TEXT NOT NULL,
  last_modified TEXT NOT NULL,
  imports TEXT NOT NULL CHECK(json_valid(imports)),
  symbols TEXT NOT NULL CHECK(json_valid(symbols)),
  symbol_facts TEXT NOT NULL CHECK(json_valid(symbol_facts)),
  content_hash TEXT NOT NULL DEFAULT '',
  extractor_grade TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS quality_rule(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL,
  detection_logic TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT '',
  severity TEXT NOT NULL CHECK(severity IN ('warning','error')),
  inspection_effort TEXT NOT NULL DEFAULT '' CHECK(inspection_effort IN ('low','mid','high',''))
);

CREATE TABLE IF NOT EXISTS validation(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL DEFAULT '',
  validation_type TEXT NOT NULL CHECK(validation_type IN ('test','assertion','benchmark','manual_check','saga')),
  command TEXT NOT NULL DEFAULT '',
  last_run TEXT NOT NULL DEFAULT '',
  last_result TEXT NOT NULL CHECK(last_result IN ('passed','failed','not_run','blocked','')),
  last_executed_run TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS note(
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  text TEXT NOT NULL,
  author TEXT NOT NULL,
  target_kind TEXT NOT NULL,
  target_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  audience TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS ignore_rule(
  id TEXT PRIMARY KEY,
  pattern TEXT NOT NULL UNIQUE,
  reason TEXT NOT NULL,
  author TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS delegation(
  id TEXT PRIMARY KEY,
  pattern TEXT NOT NULL UNIQUE,
  target TEXT NOT NULL,
  export_hash TEXT NOT NULL DEFAULT '',
  seam_intents TEXT NOT NULL DEFAULT '[]',
  author TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS hypothesis(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  claim TEXT NOT NULL,
  proposal TEXT NOT NULL,
  predicted_outcome TEXT NOT NULL,
  status TEXT NOT NULL,
  author TEXT NOT NULL,
  evidence TEXT NOT NULL DEFAULT '',
  last_inspected TEXT NOT NULL DEFAULT '',
  inspected_by TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS vocab_term(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL,
  author TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS persona(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL,
  author TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS interface_surface(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  surface_kind TEXT NOT NULL,
  method TEXT NOT NULL DEFAULT '',
  target TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(surface_kind, method, target)
);

CREATE TABLE IF NOT EXISTS inbox_item(
  id TEXT PRIMARY KEY,
  raw_text TEXT NOT NULL,
  normalized_claim TEXT NOT NULL DEFAULT '',
  kind TEXT NOT NULL CHECK(kind IN (__INBOX_KIND_SQL_VALUES__)),
  status TEXT NOT NULL CHECK(status IN ('new','triaged','routed','rejected','deferred','duplicate')),
  source TEXT NOT NULL CHECK(source IN ('chat','user','llm','code_audit','validation','import','unknown')),
  author TEXT NOT NULL,
  tags TEXT NOT NULL CHECK(json_valid(tags)),
  links TEXT NOT NULL CHECK(json_valid(links)),
  route_kind TEXT NOT NULL DEFAULT '' CHECK(route_kind IN (
    'intent','hypothesis','validation','quality_rule','vocab','note','ignore','answer','none',''
  )),
  route_command TEXT NOT NULL DEFAULT '',
  route_target_kind TEXT NOT NULL DEFAULT '',
  route_target_id TEXT NOT NULL DEFAULT '',
  resolution TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS relates_to(
  from_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  to_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  inspection_status TEXT NOT NULL CHECK(inspection_status IN ('uninspected','passing','failing','independent','needs_reverification')),
  criterion TEXT NOT NULL DEFAULT '',
  confidence REAL NOT NULL DEFAULT 0,
  evidence TEXT NOT NULL DEFAULT '',
  last_inspected TEXT NOT NULL DEFAULT '',
  inspected_by TEXT NOT NULL DEFAULT '',
  priority_score REAL NOT NULL DEFAULT 0,
  notes TEXT NOT NULL DEFAULT '',
  kinds TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(kinds)),
  stable TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(from_id, to_id),
  CHECK(from_id <> to_id)
);

CREATE TABLE IF NOT EXISTS hierarchy(
  parent_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  child_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(parent_id, child_id),
  CHECK(parent_id <> child_id)
);

CREATE TABLE IF NOT EXISTS implements(
  intent_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  codefile_id TEXT NOT NULL REFERENCES codefile(id) ON DELETE CASCADE,
  inspection_status TEXT NOT NULL CHECK(inspection_status IN ('uninspected','passing','failing','independent','needs_reverification')),
  criterion TEXT NOT NULL DEFAULT '',
  confidence REAL NOT NULL DEFAULT 0,
  evidence TEXT NOT NULL DEFAULT '',
  last_inspected TEXT NOT NULL DEFAULT '',
  inspected_by TEXT NOT NULL DEFAULT '',
  locator TEXT NOT NULL DEFAULT '',
  notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(intent_id, codefile_id)
);

CREATE TABLE IF NOT EXISTS governs(
  rule_id TEXT NOT NULL REFERENCES quality_rule(id) ON DELETE CASCADE,
  intent_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  inspection_status TEXT NOT NULL CHECK(inspection_status IN ('uninspected','passing','failing','independent','needs_reverification')),
  criterion TEXT NOT NULL DEFAULT '',
  confidence REAL NOT NULL DEFAULT 0,
  evidence TEXT NOT NULL DEFAULT '',
  last_inspected TEXT NOT NULL DEFAULT '',
  inspected_by TEXT NOT NULL DEFAULT '',
  notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(rule_id, intent_id)
);

CREATE TABLE IF NOT EXISTS validates(
  validation_id TEXT NOT NULL REFERENCES validation(id) ON DELETE CASCADE,
  intent_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  inspection_status TEXT NOT NULL CHECK(inspection_status IN ('uninspected','passing','failing','needs_reverification')),
  notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(validation_id, intent_id)
);

CREATE TABLE IF NOT EXISTS targets(
  hypothesis_id TEXT NOT NULL REFERENCES hypothesis(id) ON DELETE CASCADE,
  intent_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  inspection_status TEXT NOT NULL CHECK(inspection_status IN ('uninspected','passing','failing','independent','needs_reverification')),
  criterion TEXT NOT NULL DEFAULT '',
  confidence REAL NOT NULL DEFAULT 0,
  evidence TEXT NOT NULL DEFAULT '',
  last_inspected TEXT NOT NULL DEFAULT '',
  inspected_by TEXT NOT NULL DEFAULT '',
  notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(hypothesis_id, intent_id)
);

CREATE TABLE IF NOT EXISTS serves(
  persona_id TEXT NOT NULL REFERENCES persona(id) ON DELETE CASCADE,
  intent_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  inspection_status TEXT NOT NULL CHECK(inspection_status IN ('uninspected','passing','failing','independent','needs_reverification')),
  criterion TEXT NOT NULL DEFAULT '',
  confidence REAL NOT NULL DEFAULT 0,
  evidence TEXT NOT NULL DEFAULT '',
  last_inspected TEXT NOT NULL DEFAULT '',
  inspected_by TEXT NOT NULL DEFAULT '',
  notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(persona_id, intent_id)
);

CREATE TABLE IF NOT EXISTS journeys(
  persona_id TEXT NOT NULL REFERENCES persona(id) ON DELETE CASCADE,
  validation_id TEXT NOT NULL REFERENCES validation(id) ON DELETE CASCADE,
  notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(persona_id, validation_id)
);

CREATE TABLE IF NOT EXISTS calls(
  validation_id TEXT NOT NULL REFERENCES validation(id) ON DELETE CASCADE,
  interface_id TEXT NOT NULL REFERENCES interface_surface(id) ON DELETE CASCADE,
  step_index TEXT NOT NULL,
  step_name TEXT NOT NULL DEFAULT '',
  intent_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(validation_id, step_index)
);

CREATE INDEX IF NOT EXISTS idx_intent_lifecycle_status ON intent(lifecycle, status);
CREATE INDEX IF NOT EXISTS idx_intent_name ON intent(name);
CREATE INDEX IF NOT EXISTS idx_codefile_path ON codefile(path);
CREATE INDEX IF NOT EXISTS idx_relates_status_priority ON relates_to(inspection_status, priority_score DESC);
CREATE INDEX IF NOT EXISTS idx_relates_to ON relates_to(to_id);
CREATE INDEX IF NOT EXISTS idx_implements_file ON implements(codefile_id);
CREATE INDEX IF NOT EXISTS idx_implements_status ON implements(inspection_status);
CREATE INDEX IF NOT EXISTS idx_governs_status ON governs(inspection_status);
CREATE INDEX IF NOT EXISTS idx_validates_status ON validates(inspection_status);
CREATE INDEX IF NOT EXISTS idx_targets_status ON targets(inspection_status);
CREATE INDEX IF NOT EXISTS idx_serves_status ON serves(inspection_status);
CREATE INDEX IF NOT EXISTS idx_interface_surface_identity ON interface_surface(surface_kind, method, target);
CREATE INDEX IF NOT EXISTS idx_calls_interface ON calls(interface_id);
CREATE INDEX IF NOT EXISTS idx_inbox_status_kind ON inbox_item(status, kind, created_at);
CREATE INDEX IF NOT EXISTS idx_note_target ON note(target_kind, target_id, kind, created_at);
CREATE INDEX IF NOT EXISTS idx_note_target_only ON note(target_id, created_at);
CREATE INDEX IF NOT EXISTS idx_note_kind ON note(kind, created_at);

CREATE VIRTUAL TABLE IF NOT EXISTS intent_fts USING fts5(
  intent_id UNINDEXED,
  name,
  description,
  domain,
  layer,
  content='',
  tokenize='unicode61'
);
"#
}

impl SqliteGraphStore {
    pub(super) fn create_schema(&self) -> Result<()> {
        let inbox_kind_values = inbox_kind_sql_values();
        self.conn.execute_batch(
            &create_table_batch().replace("__INBOX_KIND_SQL_VALUES__", &inbox_kind_values),
        )?;
        self.ensure_meta_columns()?;
        self.ensure_taxonomy_columns()?;
        self.ensure_inbox_kind_vocabulary()?;
        self.ensure_intent_lifecycle_vocabulary()?;
        // The migrations above bring an older graph fully to the current shape;
        // stamp the version so `doctor`/`export` agree with this binary. Opening
        // with a newer loom IS the migration — there is no separate migrate
        // step. The stamp MUST come after every migration so it never claims a
        // version the on-disk schema can't honour. No-op when the meta row
        // doesn't exist yet (a fresh `init` inserts it immediately after).
        self.conn.execute(
            "UPDATE meta SET schema_version = ?1 WHERE id = 1",
            params![schema::SCHEMA_VERSION],
        )?;
        Ok(())
    }

    /// Additive taxonomy columns (the edge-kind program): RELATES_TO.kinds (the
    /// relationship multiset, JSON list) and QualityRule.kind (the norm
    /// category). Additive — existing graphs gain them on open with their
    /// defaults, no version bump (loom's convention for additive changes).
    fn ensure_taxonomy_columns(&self) -> Result<()> {
        for (table, column, definition) in [
            ("relates_to", "kinds", "TEXT NOT NULL DEFAULT '[]'"),
            ("relates_to", "stable", "TEXT NOT NULL DEFAULT ''"),
            ("quality_rule", "kind", "TEXT NOT NULL DEFAULT ''"),
            ("intent", "criterion", "TEXT NOT NULL DEFAULT ''"),
            ("delegation", "export_hash", "TEXT NOT NULL DEFAULT ''"),
            ("delegation", "seam_intents", "TEXT NOT NULL DEFAULT '[]'"),
            (
                "validation",
                "last_executed_run",
                "TEXT NOT NULL DEFAULT ''",
            ),
            ("codefile", "extractor_grade", "TEXT NOT NULL DEFAULT ''"),
        ] {
            if !table_has_column(&self.conn, table, column)? {
                let table = checked_sql_ident(table)?;
                let column = checked_sql_ident(column)?;
                self.conn.execute(
                    &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                    [],
                )?;
            }
        }
        Ok(())
    }

    fn ensure_meta_columns(&self) -> Result<()> {
        for (column, definition) in [
            ("created_at", "TEXT NOT NULL DEFAULT ''"),
            ("last_synced", "TEXT NOT NULL DEFAULT ''"),
            ("transition_cap", "TEXT NOT NULL DEFAULT ''"),
        ] {
            if !table_has_column(&self.conn, "meta", column)? {
                let column = checked_sql_ident(column)?;
                self.conn.execute(
                    &format!("ALTER TABLE meta ADD COLUMN {column} {definition}"),
                    [],
                )?;
            }
        }
        Ok(())
    }

    /// v10 widened the intent.lifecycle CHECK to admit `to_be_removed`, but a
    /// CHECK constraint cannot be ALTERed in place. `CREATE TABLE IF NOT EXISTS`
    /// is a no-op on an existing table, so a pre-v10 graph would keep the old
    /// 4-value CHECK and reject `to_be_removed` — while the version stamp claimed
    /// v10. This rebuilds the intent table when the CHECK is stale, using the
    /// SQLite-recommended procedure for a table with INBOUND foreign keys:
    /// foreign_keys OFF (so the DROP doesn't cascade-delete every edge), create a
    /// NEW-named table, copy by explicit column name, DROP the old, RENAME the new
    /// into place (other tables reference "intent", which the rename restores).
    fn ensure_intent_lifecycle_vocabulary(&self) -> Result<()> {
        let create_sql: Option<String> = self
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'intent'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        match create_sql.as_deref() {
            // No intent table yet (fresh init creates it with the v10 CHECK), or
            // the CHECK already admits to_be_removed → nothing to migrate.
            None => return Ok(()),
            Some(sql) if sql.contains("to_be_removed") => return Ok(()),
            Some(_) => {}
        }
        // PRAGMA foreign_keys must be toggled OUTSIDE a transaction; the BEGIN…
        // COMMIT inside the batch makes the rebuild itself atomic.
        self.conn.execute_batch(
            r#"
PRAGMA foreign_keys=OFF;
BEGIN;
CREATE TABLE intent_v10(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL,
  criterion TEXT NOT NULL DEFAULT '',
  abstraction_level TEXT NOT NULL,
  domain TEXT NOT NULL DEFAULT '',
  layer TEXT NOT NULL DEFAULT '',
  source_refs TEXT NOT NULL CHECK(json_valid(source_refs)),
  status TEXT NOT NULL,
  aspect TEXT NOT NULL DEFAULT '',
  lifecycle TEXT NOT NULL CHECK(lifecycle IN ('planned','implemented','needs_change','deferred','to_be_removed')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  tags TEXT NOT NULL CHECK(json_valid(tags)),
  visibility TEXT NOT NULL DEFAULT '' CHECK(visibility IN ('user_visible','internal','')),
  boundary TEXT NOT NULL DEFAULT '' CHECK(boundary IN ('inbound','outbound',''))
);
INSERT INTO intent_v10(
  id, name, description, criterion, abstraction_level, domain, layer, source_refs,
  status, aspect, lifecycle, created_at, updated_at, tags, visibility, boundary
)
SELECT
  id, name, description, criterion, abstraction_level, domain, layer, source_refs,
  status, aspect, lifecycle, created_at, updated_at, tags, visibility, boundary
FROM intent;
DROP TABLE intent;
ALTER TABLE intent_v10 RENAME TO intent;
CREATE INDEX IF NOT EXISTS idx_intent_lifecycle_status ON intent(lifecycle, status);
CREATE INDEX IF NOT EXISTS idx_intent_name ON intent(name);
COMMIT;
PRAGMA foreign_keys=ON;
"#,
        )?;
        Ok(())
    }

    fn ensure_inbox_kind_vocabulary(&self) -> Result<()> {
        let create_sql: Option<String> = self
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'inbox_item'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if match create_sql.as_deref() {
            None => true,
            Some(sql) => schema::INBOX_KINDS
                .iter()
                .all(|kind| sql.contains(&sql_string_literal(kind))),
        } {
            return Ok(());
        }

        let rebuild_sql = format!(
            r#"
ALTER TABLE inbox_item RENAME TO inbox_item_old;
CREATE TABLE inbox_item(
  id TEXT PRIMARY KEY,
  raw_text TEXT NOT NULL,
  normalized_claim TEXT NOT NULL DEFAULT '',
  kind TEXT NOT NULL CHECK(kind IN ({inbox_kind_values})),
  status TEXT NOT NULL CHECK(status IN ('new','triaged','routed','rejected','deferred','duplicate')),
  source TEXT NOT NULL CHECK(source IN ('chat','user','llm','code_audit','validation','import','unknown')),
  author TEXT NOT NULL,
  tags TEXT NOT NULL CHECK(json_valid(tags)),
  links TEXT NOT NULL CHECK(json_valid(links)),
  route_kind TEXT NOT NULL DEFAULT '' CHECK(route_kind IN (
    'intent','hypothesis','validation','quality_rule','vocab','note','ignore','answer','none',''
  )),
  route_command TEXT NOT NULL DEFAULT '',
  route_target_kind TEXT NOT NULL DEFAULT '',
  route_target_id TEXT NOT NULL DEFAULT '',
  resolution TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
INSERT INTO inbox_item(
  id, raw_text, normalized_claim, kind, status, source, author, tags, links,
  route_kind, route_command, route_target_kind, route_target_id, resolution,
  created_at, updated_at
)
SELECT
  id, raw_text, normalized_claim, kind, status, source, author, tags, links,
  route_kind, route_command, route_target_kind, route_target_id, resolution,
  created_at, updated_at
FROM inbox_item_old;
DROP TABLE inbox_item_old;
CREATE INDEX IF NOT EXISTS idx_inbox_status_kind ON inbox_item(status, kind, created_at);
"#,
            inbox_kind_values = inbox_kind_sql_values()
        );
        self.conn.execute_batch(&rebuild_sql)?;
        Ok(())
    }
}

fn inbox_kind_sql_values() -> String {
    schema::INBOX_KINDS
        .iter()
        .map(|kind| sql_string_literal(kind))
        .collect::<Vec<_>>()
        .join(",")
}
