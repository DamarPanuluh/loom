use super::{
    acquire_lock, apply_schema_migrations, configure, configure_read,
    ensure_supported_persisted_schema, id_and_now, Agent, Identity, Store, LOCAL_SNAPSHOT_MARKER,
};
use crate::model::*;
use crate::registry;
use crate::{
    Result, CRATE_VERSION, GRAPH_DB, LOOM_DIR, SCHEMA_VERSION, WRITER_SCHEMA_KEY,
    WRITER_VERSION_KEY,
};
use anyhow::{anyhow, bail, Context};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::fs::File;
use std::path::{Path, PathBuf};

impl Store {
    /// Initialize a fresh graph at `root/.loom/graph.sqlite`. Idempotent: if the
    /// store already exists, opens it and backfills identity defaults.
    pub fn init(root: &Path, name: Option<&str>, observed: bool) -> Result<Store> {
        let identity = crate::identity::ExecutionIdentity::resolve_env()?;
        Self::init_with_identity(root, name, observed, identity)
    }

    /// [`Store::init`] with a caller-pinned execution identity. Tests use this
    /// to stay hermetic: identity-sensitive writes (ratification, lane gates)
    /// must not change verdict when an ambient `LOOM_AGENT` leaks into the
    /// test process from a driver's shell.
    pub fn init_with_identity(
        root: &Path,
        name: Option<&str>,
        observed: bool,
        identity: crate::identity::ExecutionIdentity,
    ) -> Result<Store> {
        let loom_dir = root.join(LOOM_DIR);
        std::fs::create_dir_all(&loom_dir)
            .with_context(|| format!("creating {}", loom_dir.display()))?;
        let db_path = loom_dir.join(GRAPH_DB);
        let lock = acquire_lock(&loom_dir, true, &identity)?;
        let fresh = !db_path.exists();
        let mut conn =
            Connection::open(&db_path).with_context(|| format!("opening {}", db_path.display()))?;
        ensure_supported_persisted_schema(&conn)?;
        configure(&conn)?;
        apply_schema_migrations(&mut conn)?;
        if fresh {
            let default_name = name
                .map(str::to_string)
                .or_else(|| {
                    root.canonicalize()
                        .ok()
                        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                })
                .unwrap_or_else(|| "loom".to_string());
            let (gid, now) = id_and_now(&conn)?;
            // One atomic seed: either the graph is born with a full identity or
            // the file stays empty. A half-written meta table (a graph_id with no
            // schema_version) would leave `open` unable to tell fresh from broken.
            let tx = conn.transaction()?;
            let set = |k: &str, v: &str| -> Result<()> {
                tx.execute("INSERT INTO meta(key,value) VALUES (?1,?2)", params![k, v])?;
                Ok(())
            };
            set("graph_id", &gid)?;
            set("name", &default_name)?;
            set("schema_version", &SCHEMA_VERSION.to_string())?;
            set("observed", if observed { "1" } else { "0" })?;
            set("created_at", &now)?;
            tx.commit()?;
        } else if name.is_some() || observed {
            // Backfill identity on an existing graph.
            if let Some(n) = name {
                conn.execute(
                    "INSERT INTO meta(key,value) VALUES ('name',?1)
                     ON CONFLICT(key) DO UPDATE SET value=?1",
                    params![n],
                )?;
            }
            if observed {
                conn.execute(
                    "INSERT INTO meta(key,value) VALUES ('observed','1')
                     ON CONFLICT(key) DO UPDATE SET value='1'",
                    [],
                )?;
            }
        }
        let store = Store {
            conn,
            root: root.to_path_buf(),
            identity: std::cell::RefCell::new(identity),
            _lock: std::cell::RefCell::new(lock),
        };
        store.stamp_writer_identity();
        Ok(store)
    }

    /// Open an existing graph at `root/.loom/graph.sqlite`.
    pub fn open(root: &Path) -> Result<Store> {
        let identity = crate::identity::ExecutionIdentity::resolve_env()?;
        Self::open_with_identity(root, identity)
    }

    /// Open using an identity already resolved by the enclosing command. Long
    /// running commands use this when they must release and reacquire the graph
    /// lock around child execution without reinterpreting process identity.
    pub fn open_with_identity(
        root: &Path,
        identity: crate::identity::ExecutionIdentity,
    ) -> Result<Store> {
        let loom_dir = root.join(LOOM_DIR);
        let db_path = loom_dir.join(GRAPH_DB);
        if !db_path.exists() {
            bail!(
                "no loom graph at {} — run `loom init` first",
                db_path.display()
            );
        }
        let lock = acquire_lock(&loom_dir, true, &identity)?;
        // Heartbeat for the advisory role lease: a lane driver touching the
        // graph is proof its session is alive. Best-effort by contract.
        crate::rolelease::refresh(root, &identity);
        let mut conn = Connection::open(&db_path)?;
        ensure_supported_persisted_schema(&conn)?;
        configure(&conn)?;
        apply_schema_migrations(&mut conn)?;
        let store = Store {
            conn,
            root: root.to_path_buf(),
            identity: std::cell::RefCell::new(identity),
            _lock: std::cell::RefCell::new(lock),
        };
        store.stamp_writer_identity();
        Ok(store)
    }

    /// Record which crate *and* schema last wrote this graph. Always overwrites
    /// the schema stamp: two builds can share a crate version and still drift.
    fn stamp_writer_identity(&self) {
        let _ = self.set_meta(WRITER_VERSION_KEY, CRATE_VERSION);
        let _ = self.set_meta(WRITER_SCHEMA_KEY, &SCHEMA_VERSION.to_string());
    }

    /// Open an existing graph read-only. Takes a SHARED advisory lock, so many
    /// readers proceed together and only wait while a writer holds the boundary,
    /// and sets `query_only` so no read command can mutate the graph. Never
    /// migrates: a schema behind `SCHEMA_VERSION` is reported, not silently
    /// upgraded, because migration is a write and must go through a write open.
    pub fn open_read(root: &Path) -> Result<Store> {
        let loom_dir = root.join(LOOM_DIR);
        let db_path = loom_dir.join(GRAPH_DB);
        if !db_path.exists() {
            bail!(
                "no loom graph at {} — run `loom init` first",
                db_path.display()
            );
        }
        let identity = crate::identity::ExecutionIdentity::resolve_env()?;
        let lock = acquire_lock(&loom_dir, false, &identity)?;
        // Heartbeat for the advisory role lease (see `open_with_identity`).
        crate::rolelease::refresh(root, &identity);
        let conn = Connection::open(&db_path)?;
        ensure_supported_persisted_schema(&conn)?;
        configure_read(&conn)?;
        // `user_version` is the migration stamp maintained by the write path
        // (`apply_schema_migrations`); a read open must not migrate, so a mismatch
        // is an explicit "run a write command first", never a silent read of an
        // older shape.
        let user_version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if user_version < SCHEMA_VERSION {
            bail!(
                "graph schema is unstamped (v{user_version}); open it once with a write-capable \
                 loom v{SCHEMA_VERSION} command before using read-only commands"
            );
        }
        Ok(Store {
            conn,
            root: root.to_path_buf(),
            identity: std::cell::RefCell::new(identity),
            _lock: std::cell::RefCell::new(lock),
        })
    }

    /// Clone this trusted local graph into an empty temporary root without
    /// applying import semantics. The interface deliberately hides SQLite/WAL
    /// consistency and journal provenance: callers get one confined graph clone
    /// that preserves the source graph's local authority while leaving every
    /// source byte untouched.
    pub fn clone_local_snapshot(&self, destination_root: &Path) -> Result<()> {
        fn copy_addressed_file(
            source_root: &Path,
            destination_root: &Path,
            relative: &Path,
        ) -> Result<()> {
            if relative.as_os_str().is_empty()
                || relative.is_absolute()
                || relative
                    .components()
                    .any(|component| !matches!(component, std::path::Component::Normal(_)))
            {
                bail!(
                    "local graph snapshot refuses non-confined registered path '{}'",
                    relative.display()
                );
            }
            let source = source_root.join(relative);
            if !source.is_file() {
                return Ok(());
            }
            let destination = destination_root.join(relative);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&source, &destination).with_context(|| {
                format!(
                    "copying registered snapshot artifact {} to {}",
                    source.display(),
                    destination.display()
                )
            })?;
            Ok(())
        }

        if destination_root == self.root {
            bail!("local graph snapshot destination must differ from its source");
        }
        let destination_loom = destination_root.join(LOOM_DIR);
        if destination_loom.exists() {
            bail!(
                "local graph snapshot destination '{}' is not empty",
                destination_loom.display()
            );
        }
        std::fs::create_dir_all(&destination_loom)
            .with_context(|| format!("creating {}", destination_loom.display()))?;
        let destination_db = destination_loom.join(GRAPH_DB);
        let result = (|| -> Result<()> {
            // VACUUM INTO is SQLite's consistent online-copy operation. Opening
            // the source read-only and holding this Store's graph lock ensures
            // the clone includes committed WAL state without checkpointing or
            // otherwise changing the source database.
            let source_db = self.root.join(LOOM_DIR).join(GRAPH_DB);
            let source = Connection::open_with_flags(&source_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .with_context(|| {
                    format!("opening local snapshot source {}", source_db.display())
                })?;
            let destination = destination_db.to_str().ok_or_else(|| {
                anyhow!(
                    "local graph snapshot destination '{}' is not valid UTF-8",
                    destination_db.display()
                )
            })?;
            source
                .execute("VACUUM INTO ?1", params![destination])
                .with_context(|| {
                    format!("cloning local graph into {}", destination_db.display())
                })?;

            // Journal provenance is local authority and therefore must not pass
            // through `restore_entries`, whose correct import behavior marks it
            // imported. Copy the immutable JSONL bytes verbatim instead.
            let source_journal = crate::journal::path(&self.root);
            if source_journal.is_file() {
                let destination_journal = crate::journal::path(destination_root);
                let parent = destination_journal.parent().ok_or_else(|| {
                    anyhow!(
                        "journal destination '{}' has no parent",
                        destination_journal.display()
                    )
                })?;
                std::fs::create_dir_all(parent)?;
                std::fs::copy(&source_journal, &destination_journal).with_context(|| {
                    format!(
                        "copying local journal {} to {}",
                        source_journal.display(),
                        destination_journal.display()
                    )
                })?;
            }

            // Graph predicates resolve these files relative to Store::root.
            // Copy only graph-addressed artifacts, not the whole repository, so
            // the clone preserves semantic state without broadening execution.
            let mut paths = std::collections::BTreeSet::new();
            for node in self.list_nodes(Some(NodeType::CodeFile), usize::MAX)? {
                paths.insert(PathBuf::from(node.name));
            }
            for node in self.list_nodes(Some(NodeType::Journey), usize::MAX)? {
                if let Some(artifact) = node
                    .body
                    .get("artifact")
                    .and_then(serde_json::Value::as_str)
                {
                    paths.insert(PathBuf::from(artifact));
                }
            }
            // Span evidence may cite files beyond the registered CodeFile set
            // (test files, journey surface manifests). `reverify_all` re-reads
            // every cited span from disk, so a clone missing one breaks the
            // anchor and stales its edge — an unchanged snapshot must sync
            // clean (INV-2). Cited files are graph-addressed artifacts too.
            let mut cited = self.conn.prepare(
                "SELECT DISTINCT json_extract(payload, '$.file') FROM evidence WHERE kind = 'span'",
            )?;
            let cited_files = cited.query_map([], |row| row.get::<_, Option<String>>(0))?;
            for file in cited_files {
                if let Some(file) = file? {
                    paths.insert(PathBuf::from(file));
                }
            }
            paths.insert(PathBuf::from(crate::GRAPH_EXPORT));
            for relative in paths {
                copy_addressed_file(&self.root, destination_root, &relative)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            if let Err(cleanup_error) = std::fs::remove_dir_all(&destination_loom) {
                return Err(error).context(format!(
                    "removing incomplete local snapshot {} also failed: {cleanup_error}",
                    destination_loom.display()
                ));
            }
            return Err(error);
        }
        if let Err(error) = std::fs::write(
            destination_loom.join(LOCAL_SNAPSHOT_MARKER),
            b"{\"schema\":\"loom.local-snapshot/v1\"}\n",
        ) {
            if let Err(cleanup_error) = std::fs::remove_dir_all(&destination_loom) {
                return Err(error).context(format!(
                    "marking the cloned graph as a local snapshot failed, and removing {} also failed: {cleanup_error}",
                    destination_loom.display()
                ));
            }
            return Err(error).context("marking the cloned graph as a local snapshot");
        }
        Ok(())
    }

    /// True only for graphs minted by [`Store::clone_local_snapshot`].
    /// Live operator graphs never carry this marker.
    pub fn is_local_snapshot(&self) -> bool {
        let path = self.root.join(LOOM_DIR).join(LOCAL_SNAPSHOT_MARKER);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata.is_file() && !metadata.file_type().is_symlink(),
            Err(_) => false,
        }
    }

    /// Walk up from `start` to find the nearest ancestor containing `.loom/`.
    pub fn find_root(start: &Path) -> Option<PathBuf> {
        let mut cur = Some(start);
        while let Some(dir) = cur {
            if dir.join(LOOM_DIR).join(GRAPH_DB).exists() {
                return Some(dir.to_path_buf());
            }
            cur = dir.parent();
        }
        None
    }

    /// The acting agent.
    pub fn agent(&self) -> Agent {
        self.identity.borrow().authority()
    }

    /// Release the advisory graph write lock while keeping the connection
    /// open. Only the Store-owned guarded Journey settlement uses this: a
    /// compiled Journey's operations may spawn child `loom` processes that
    /// open the graph, and a held exclusive lock would refuse them. The
    /// harness lock serializes proof execution; settlement re-derives every
    /// trust-relevant input and refuses on drift. Re-take with
    /// [`Store::reacquire_graph_lock`] before any write.
    pub fn release_graph_lock(&self) {
        let lock_path = self.root.join(LOOM_DIR).join("lock");
        if let Ok(placeholder) = File::open(&lock_path) {
            *self._lock.borrow_mut() = placeholder;
        }
    }

    /// Re-take the advisory graph write lock under the identity this Store
    /// opened with. Fails closed (lock contention) rather than writing
    /// without the boundary.
    pub fn reacquire_graph_lock(&self) -> Result<()> {
        let loom_dir = self.root.join(LOOM_DIR);
        let identity = self.identity.borrow().clone();
        *self._lock.borrow_mut() = acquire_lock(&loom_dir, true, &identity)?;
        Ok(())
    }

    /// Runtime provenance at the store seam. Authorization is derived from
    /// the already-validated Agent; profile remains independent attribution.
    pub fn execution_identity(&self) -> crate::identity::ExecutionIdentity {
        self.identity.borrow().clone()
    }

    /// Append provenance using the identity resolved when this Store opened.
    /// Callers never read process configuration or assemble audit identity.
    pub fn append_journal(
        &self,
        event: &str,
        target_id: &str,
        payload: serde_json::Value,
    ) -> Result<crate::journal::Entry> {
        crate::journal::append(
            self.root(),
            &self.execution_identity(),
            event,
            target_id,
            payload,
        )
    }

    pub fn append_journal_once(
        &self,
        event: &str,
        target_id: &str,
        payload: serde_json::Value,
        same_transition: impl Fn(&crate::journal::Entry) -> bool,
    ) -> Result<Option<crate::journal::Entry>> {
        crate::journal::append_once(
            self.root(),
            &self.execution_identity(),
            event,
            target_id,
            payload,
            same_transition,
        )
    }

    /// Override the acting agent (CLI sets this from `LOOM_AGENT`; tests set it
    /// explicitly to exercise lane gates without env races).
    pub fn set_agent(&self, agent: Agent) {
        let identity = self.identity.borrow().with_authority(agent);
        *self.identity.borrow_mut() = identity;
    }

    /// Lane gate: a declared lane may only write edges/verdicts it owns. Solo
    /// drives every lane. `sync` is implicit (derived paths never call this).
    ///
    /// Grounding (`implements`) writes are builder-owned, but the fixer
    /// contract explicitly allows re-grounding after a repair moved code.
    /// Verdicts on those edges stay on `check_lane` so fixer still cannot
    /// record the passing claim.
    pub(crate) fn check_grounding_write(&self) -> Result<()> {
        match self.agent() {
            Agent::Solo => Ok(()),
            Agent::Lane(role)
                if role.satisfies(registry::OwnerRole::Builder)
                    || role == registry::OwnerRole::Fixer =>
            {
                Ok(())
            }
            Agent::Lane(role) => bail!(
                "lane gate: agent '{}' may not write 'builder'-owned facts",
                role.as_str()
            ),
        }
    }

    /// Lane gate: a declared lane may only write edges/verdicts it owns. Solo
    /// drives every lane. `sync` is implicit (derived paths never call this).
    pub(crate) fn check_lane(&self, owner: registry::OwnerRole) -> Result<()> {
        match self.agent() {
            Agent::Solo => Ok(()),
            Agent::Lane(role) if role.satisfies(owner) => Ok(()),
            Agent::Lane(role) => bail!(
                "lane gate: agent '{}' may not write '{}'-owned facts",
                role.as_str(),
                owner.as_str()
            ),
        }
    }

    /// Preflight authorization for a command that will create an asserted
    /// edge after performing other writes. The edge write still enforces the
    /// same gate itself; this check lets a multi-step command fail before its
    /// first mutation, while the surrounding transaction remains the final
    /// atomicity boundary for every later error.
    pub fn require_edge_kind_owner(&self, kind: EdgeKind) -> Result<()> {
        if kind == EdgeKind::Implements {
            self.check_grounding_write()
        } else {
            self.check_lane(registry::spec(kind).owner)
        }
    }

    /// Governed research is analysis work; solo remains an unrestricted driver.
    pub fn require_research_owner(&self) -> Result<()> {
        self.check_lane(registry::OwnerRole::Analyzer)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn identity(&self) -> Result<Identity> {
        let get = |k: &str| -> Result<String> {
            self.conn
                .query_row("SELECT value FROM meta WHERE key=?1", params![k], |r| {
                    r.get::<_, String>(0)
                })
                .with_context(|| format!("reading meta '{k}'"))
        };
        // `observed` did not exist in the earliest graphs, so absence remains a
        // backward-compatible `false`. A present value is strict: swallowing a
        // database error or typo here could silently turn a monitor into an
        // owned graph and enable its build/fix lanes.
        let observed_raw = self
            .conn
            .query_row("SELECT value FROM meta WHERE key='observed'", [], |row| {
                row.get::<_, String>(0)
            })
            .optional()
            .context("reading meta 'observed'")?;
        let observed = match observed_raw.as_deref() {
            None | Some("0") => false,
            Some("1") => true,
            Some(value) => bail!("meta.observed is malformed: expected '0' or '1', got '{value}'"),
        };
        Ok(Identity {
            graph_id: get("graph_id")?,
            name: get("name")?,
            schema_version: get("schema_version")?
                .parse()
                .context("meta.schema_version is malformed")?,
            observed,
        })
    }

    /// Set the graph's mode: `observed` = maps code the driver does not own
    /// (discovery-only; build/fix/coverage/elaborate lanes disabled), `owned` =
    /// the normal build-and-prove mode. This is the post-init counterpart to
    /// `loom init --observed`; `sync` never changes it, because scanning files
    /// says nothing about who owns them. Returns the value actually set.
    pub fn set_observed(&self, observed: bool) -> Result<bool> {
        self.conn.execute(
            "INSERT INTO meta(key,value) VALUES ('observed',?1)
             ON CONFLICT(key) DO UPDATE SET value=?1",
            params![if observed { "1" } else { "0" }],
        )?;
        Ok(observed)
    }

    /// Begin an explicit transaction for a multi-mutation batch (`loom apply`).
    /// Uses `unchecked_transaction` so it composes with the store's `&self`
    /// write methods; drop it without `commit` to roll the whole batch back —
    /// the atomicity guarantee behind `loom apply`.
    pub fn begin(&self) -> Result<rusqlite::Transaction<'_>> {
        Ok(self.conn.unchecked_transaction()?)
    }

    /// Open a transaction ONLY when the connection is not already inside one.
    ///
    /// A low-level write that needs its several statements to be atomic still has
    /// to compose with an outer `begin()` batch (`loom apply`), where a nested
    /// `BEGIN` is an error. When an ambient transaction is already open this
    /// returns `None` and the outer batch owns atomicity; otherwise it returns a
    /// fresh tx the caller must `commit`. Either way the statements run on
    /// `self.conn`, so they land in whichever transaction is active.
    pub(crate) fn maybe_tx(&self) -> Result<Option<rusqlite::Transaction<'_>>> {
        if self.conn.is_autocommit() {
            Ok(Some(self.conn.unchecked_transaction()?))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::testutil::TmpRoot;
    use crate::{CRATE_VERSION, SCHEMA_VERSION, WRITER_SCHEMA_KEY, WRITER_VERSION_KEY};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn identity_defaults_only_missing_observed_and_rejects_malformed_values() {
        let tmp = TmpRoot::new("loom-store-observed-meta");
        let store = Store::init(tmp.path(), Some("observed-meta"), false).unwrap();

        store
            .conn
            .execute("DELETE FROM meta WHERE key='observed'", [])
            .unwrap();
        assert!(!store.identity().unwrap().observed);

        store
            .conn
            .execute(
                "INSERT INTO meta(key,value) VALUES ('observed','maybe')",
                [],
            )
            .unwrap();
        let error = store
            .identity()
            .expect_err("a malformed observed flag must not become owned mode");
        assert!(error.to_string().contains("meta.observed is malformed"));
    }

    #[test]
    fn write_open_stamps_writer_crate_and_schema() {
        let tmp = TmpRoot::new("loom-store-writer-stamp");
        let store = Store::init(tmp.path(), Some("stamp"), false).unwrap();
        let expected_schema = SCHEMA_VERSION.to_string();
        assert_eq!(
            store.get_meta(WRITER_VERSION_KEY).unwrap().as_deref(),
            Some(CRATE_VERSION)
        );
        assert_eq!(
            store.get_meta(WRITER_SCHEMA_KEY).unwrap().as_deref(),
            Some(expected_schema.as_str())
        );
    }
}
