This repo uses `loom` — an intent-graph CLI where the graph is the SINGLE source of
truth about what the code is and why; docs are projections of it.

1. Confirm `loom` is on PATH and v0.3.3+ (`loom --version`). If there's no graph yet,
   `loom init .` and add `.loom/` to .gitignore (local cache; only loom.graph.json travels).
2. Run `loom guide` and follow it — it is the source of truth on HOW to drive loom. Drive
   loom DIRECTLY (every read takes `--json`); never write wrapper scripts/loops around it,
   bulk is built in (`loom next --take N` → edit → `loom batch -`).
3. If the repo already has docs (README architecture, docs/, ADRs, design wikis) they are a
   SECOND source of truth that drifts. Run `loom guide --mode brownfield` and reconcile them
   through the inbox: capture each durable claim (`loom inbox add "<claim>" --source import
   --link file:<doc>`), then `loom inbox normalize` it — VERIFY against the code and route it
   (`intent` / `note` / `quality_rule`) or `ignore` it when the code disagrees (code wins) or
   it duplicates the graph. Then regenerate the human doc with `loom wiki` and retire the
   hand-maintained copy — one source of truth. Leave docs loom doesn't own (install/license).
4. Keep it fresh: after any code change, `loom sync` → `loom next --mode fix` → `loom export`
   + `loom wiki` before committing (`loom export --check` / `loom wiki --check` guard drift).
5. Record HONEST confidence (<0.7 auto-routes to review); never fabricate evidence — the graph
   is only worth as much as it's true. Use `loom explain <intent|file>` to understand a node,
   `loom explain <file> --impact` before you edit.
