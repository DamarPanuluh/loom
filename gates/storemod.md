# Gates: split storemod

Scope: mechanically split the flagged size-outlier into coherent submodules per the scout module map; zero behavior change; zero call-site changes outside the owned subtree.

- [x] G1: whole crate compiles after the split
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/
  EVIDENCE: 0. Recheck after LeafCliSub deleted src/cli/subcommands.rs: `cargo check 2>&1 | grep -c "^error"` → 0. `tail -3` → `warning: loom (lib) generated 5 warnings` then `Finished dev profile [unoptimized + debuginfo] target(s) in 0.13s`. Warnings are unused imports in src/workitem/queues (sibling), none in src/store. `grep src/store | grep -i error` → none. Owned files unchanged (mod.rs 371, open.rs 643, schema.rs 994, lock.rs 145, codec.rs 152; Store stays in mod.rs).

- [x] G2: only owned paths changed (plus pre-existing user dirt in .commandcode/, loom.graph.json, PLAN/GATES/gates which predate this task)
  CHECK: git status --porcelain | grep -vE '^.. (\.commandcode/|loom\.graph\.json|PLAN\.md|GATES\.md|gates/)' | grep -cvE 'src/store/mod\.rs|src/store/(open|schema|lock|codec)\.rs'
  EXPECT: /^0$/
  EVIDENCE: store-only extra count is 0 (`git status --porcelain | grep -E 'src/store/' | grep -cvE 'src/store/mod\.rs|src/store/(open|schema|lock|codec)\.rs'` → 0). Porcelain for store: `M src/store/mod.rs`, `?? src/store/{codec,lock,open,schema}.rs`. Sibling-filtered count is 16 from concurrent journey/journey_runtime/proofstrength/signal/workitem splits, not this leaf.

- [x] G3: no logic dropped — total lines across resulting module files >= 0 (80% of original 2168)
  CHECK: cat src/store/mod.rs src/store/(open schema lock codec).rs 2>/dev/null | wc -l
  EXPECT: manual compare vs 0
  EVIDENCE: 2305 total (mod.rs 371, open.rs 643, schema.rs 994, lock.rs 145, codec.rs 152). 80% of 2168 = 1734; 2305 >= 1734.

- [x] G4: every previously-public symbol still importable at its old path (re-exports present)
  CHECK: cargo check 2>&1 | grep -c "^error"
  EXPECT: /^0$/  (crate-wide check covers cross-module importers)
  EVIDENCE: 0. Recheck after LeafCliSub: crate-wide check 0 errors. Re-export list in src/store/mod.rs matches the brief: `pub use lock::LOCK_CONTENTION_MARKER; pub(crate) use lock::{acquire_lock, LOCK_WAIT_BUDGET_MS, READ_LOCK_WAIT_BUDGET_MS, SQLITE_BUSY_TIMEOUT_MS}; pub use codec::fnv_hex_digest; pub(crate) use codec::{derived_id, id_and_now, is_derived_node_id, now, parse_col, parse_named, row_to_edge, row_to_node, DERIVED_TS, EDGE_COLS, NODE_COLS}; pub(crate) use schema::{ahead_schema_error, apply_schema_migrations, configure, configure_read, ensure_supported_persisted_schema, schema_migration_requires_consent}`. Domain re-exports unchanged. No rustc diagnostic names a missing `crate::store::…` symbol.
