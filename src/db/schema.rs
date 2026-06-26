//! The loom graph schema, declared in ONE place.
//!
//! SQLite owns the physical schema, while this module owns the graph vocabulary
//! surfaced to commands, exports, doctor checks, and the LLM-facing schema view.
//! loom is the sole author of the graph — the LLM never writes SQL, it only
//! calls structured subcommands — so schema stability is enforced in three
//! layers:
//!   1. This module declares the entire vocabulary (labels, edge types,
//!      properties, version) once. New code and `loom doctor` reference it.
//!   2. `loom doctor` (see `commands::doctor`) verifies the *live* graph against
//!      these declarations and catches storage drift.
//!   3. Round-trip tests write→read every field of every entity.

// ---------------------------------------------------------------------------
// Schema version
// ---------------------------------------------------------------------------

/// Bumped whenever the node/edge/property vocabulary below changes in a way that
/// would make an older `.loom/` graph inconsistent. Stored on the `LoomMeta`
/// node at `init` and checked by `loom doctor`.
///
/// v3: every field now declares its owning agent role (see `role`); HIERARCHY
/// dropped its vestigial `inspection_status` (the tree is enforced at insert, not
/// inspected); GOVERNS gained `confidence` to match the other inspectable edges.
///
/// Still v3 after the federation additions (graph identity + custody on the
/// meta sentinel, the Delegation label, CodeFile.content_hash): all additive —
/// older graphs stay consistent (identity backfills on `loom init`, missing
/// labels/props simply read empty) and older exports still import.
///
/// Still v3 after the hypothesis plane (the Hypothesis label + TARGETS edge):
/// additive again — older graphs simply have no hypotheses, and older exports
/// without the sections import as empty (see `portability::is_additive_*`).
///
/// Still v3 after the bounded tag vocabulary (the VocabTerm label +
/// Intent.tags): additive once more — older graphs have no terms and untagged
/// intents, older exports import with `tags` read as "" (`is_optional_prop`).
///
/// v4: edge identity is DERIVED, never stored. An edge's id is
/// `edge_key(type, from, to)` (e.g. `rt:<intent-a>:<intent-b>`) computed at
/// read time — edges are unique per endpoint pair, and derived keys are stable
/// across export/import. Stored edge uuids from pre-v4 exports are normalized
/// during import.
///
/// v5: `Intent.source_refs`, `Intent.tags`, and `CodeFile.imports` are NATIVE
/// LISTS instead of JSON-encoded strings — no more double encoding (the
/// committed export now diffs as real arrays), and the malformed-JSON failure
/// class is impossible by construction. Readers tolerate legacy string values
/// (parsed as JSON) so legacy exports import into current storage correctly.
///
/// Still v5 after the Persona plane (the Persona label + SERVES edge + JOURNEYS
/// edge): additive — older graphs simply have no personas; older exports import
/// with empty persona sections; `loom doctor` only checks declared required
/// properties, so a graph with no Persona nodes passes unchanged.
///
/// v6: product `Intent.domain` and architecture `Intent.layer` are separate.
/// `layering_violation` now reads `Intent.layer` against LoomMeta.layer_order;
/// the old LoomMeta.domain_order is accepted only as legacy migration/import
/// input. Product domain remains discovery/search/scoring metadata.
///
/// v7: `CodeFile.symbols` records tree-sitter-derived top-level physical
/// symbols as an additive diagnostic fact. It is not required for doctor green.
///
/// v8: `CodeFile.symbol_facts` records rich symbol metadata for accountability
/// diagnostics. It is additive and not required for doctor green.
///
/// v9: `Intent.lifecycle` gains a fourth state, `deferred` — work consciously
/// PARKED (the design is valid and still wanted, just not being built now),
/// distinct from `retire`'s `status=deprecated` (superseded/out of scope). The
/// `lifecycle` CHECK widens to admit it; additive, and old graphs upgrade by
/// the usual export → re-init → import path.
///
/// Still v9 after the InterfaceSurface plane (the InterfaceSurface label +
/// CALLS edge): additive — older graphs simply have no interface surfaces until
/// sagas are re-registered or new journeys land.
///
/// Still v9 after the InboxItem plane: additive — free-form language can be
/// captured as durable intake cards before it becomes graph truth. Older
/// graphs simply open with an empty inbox table.
/// v10 (data-model expansion): RELATES_TO gains a `stable` low-churn flag
/// (sync stops re-opening it on every endpoint code change); Intent gains a
/// first-class `criterion` and a `to_be_removed` lifecycle (cleanup is now a
/// tracked, falsifiable-by-absence verb); Persona/InterfaceSurface gain a
/// `lifecycle` + removal; a new EXPOSES edge links a provider Intent to the
/// InterfaceSurface it serves. All additive — older graphs migrate on open via
/// the ensure_*_columns ALTERs and import normalization.
/// v12 (quality evidence semantics): QualityRule gains `evidence_examples`
/// (JSON: pass/independent/common_false_positive exemplars) and
/// `signal_expectations` (JSON: keyword lists that should appear in evidence
/// when the rule passes — the contradiction-check basis). GOVERNS gains
/// `covers_descendants` (TEXT "true"/"" — a roll-up verdict that stands for
/// its subtree) and its `inspection_status` CHECK widens to admit `partial`
/// (measured but not fully discharged — bounded, not complete). All additive —
/// older graphs gain columns on open via ensure_taxonomy_columns; the governs
/// CHECK rebuild mirrors the v10 intent.lifecycle pattern.
/// v13 (wiki v2 hard-cut): code-primary repo wiki machinery — `loom wiki` now
/// always emits the v2 bundle (directory of markdown concept files with
/// `sourceFiles`+`symbols`+`provenance` frontmatter, one module page per
/// component intent, code-primary prose with file-path links, no `intent:UUID`
/// in reader-facing prose). The flat `loom.wiki.md`, the `--okf` flag, and the
/// graph-primary OKF emitter are deleted. No schema changes — same graph shape,
/// different wiki projection. Migration: re-export with v13 binary, delete old
/// `loom.wiki.md`, run `loom wiki` to generate the v2 bundle.
pub const SCHEMA_VERSION: &str = "13";
pub const INBOX_KINDS: &[&str] = &[
    "observation",
    "user_request",
    "feature_proposal",
    "bug_suspicion",
    "refactor_suspicion",
    "missing_intent",
    "missing_validation",
    "missing_story",
    "terminology",
    "rough_edge",
    "external_blocker",
    "question",
    "decision_capture",
    "constraint",
    "acceptance_criterion",
    "interface_gap",
    "evidence",
    "risk",
    "follow_up",
    "duplicate_candidate",
    "docs_gap",
    "migration_need",
];

/// The storage type of a property — surfaced by `loom schema` so a driving
/// LLM knows which fields are lists (native since v5) or numbers without
/// guessing from examples.
pub fn prop_type(p: &str) -> &'static str {
    match p {
        x if x == prop::TAGS
            || x == prop::SOURCE_REFS
            || x == prop::IMPORTS
            || x == prop::SYMBOLS
            || x == prop::SYMBOL_FACTS
            || x == prop::LINKS
            || x == prop::SEAM_INTENTS =>
        {
            "list"
        }
        x if x == prop::CONFIDENCE || x == prop::PRIORITY_SCORE => "float",
        _ => "string",
    }
}

/// Derived edge identity: `<prefix>:<from-node-id>:<to-node-id>`.
/// Node ids are uuids (no `:`), so the key parses unambiguously.
pub fn edge_key(etype: &str, from_id: &str, to_id: &str) -> String {
    let prefix = match etype {
        edge::RELATES_TO => "rt",
        edge::HIERARCHY => "hy",
        edge::IMPLEMENTS => "imp",
        edge::GOVERNS => "gov",
        edge::VALIDATES => "val",
        edge::TARGETS => "tgt",
        edge::SERVES => "srv",
        edge::JOURNEYS => "jrn",
        edge::CALLS => "call",
        other => other,
    };
    format!("{prefix}:{from_id}:{to_id}")
}

// ---------------------------------------------------------------------------
// Node labels
// ---------------------------------------------------------------------------

pub mod label {
    pub const INTENT: &str = "Intent";
    pub const CODE_FILE: &str = "CodeFile";
    pub const QUALITY_RULE: &str = "QualityRule";
    pub const VALIDATION: &str = "Validation";
    pub const NOTE: &str = "Note";
    /// A coverage exclusion pattern (the escape hatch) — recorded with a reason.
    pub const IGNORE: &str = "Ignore";
    /// A subtree delegated to ANOTHER loom graph (monorepo/federation):
    /// coverage treats matching files as covered-by-child, not gaps.
    pub const DELEGATION: &str = "Delegation";
    /// An improvement hypothesis — the PRE-DECISION plane: a falsifiable claim
    /// that something is wrong/suboptimal plus a proposed change and a
    /// predicted outcome. Proven against code BEFORE adoption; adoption
    /// converts it into planned intents. Invisible to coverage/completeness
    /// until then — speculation never dilutes the done-condition.
    pub const HYPOTHESIS: &str = "Hypothesis";
    /// A registered tag term — the bounded vocabulary intents may reference in
    /// `tags`. A KEY, not a knowledge node: no edges, no inspection state; its
    /// value is forcing two descriptions of one responsibility to collide.
    pub const VOCAB_TERM: &str = "VocabTerm";
    /// A user persona — a named audience segment (e.g. "admin", "end_user").
    /// Connects to intents via SERVES edges (inspectable: does this intent
    /// actually serve this persona?) and to saga validations via JOURNEYS edges
    /// (structural: this saga exercises this persona's path end-to-end).
    pub const PERSONA: &str = "Persona";
    /// An externally callable boundary surface. HTTP endpoints are the first
    /// concrete kind; the node is deliberately generic for CLI/RPC/event
    /// surfaces later.
    pub const INTERFACE_SURFACE: &str = "InterfaceSurface";
    /// A durable intake card for raw human/LLM language. Inbox items are
    /// candidates, not graph truth; routing them proposes the graph noun they
    /// should become.
    pub const INBOX_ITEM: &str = "InboxItem";
}

/// Every content node label (excludes the `LoomMeta` sentinel).
pub const NODE_LABELS: &[&str] = &[
    label::INTENT,
    label::CODE_FILE,
    label::QUALITY_RULE,
    label::VALIDATION,
    label::NOTE,
    label::IGNORE,
    label::DELEGATION,
    label::HYPOTHESIS,
    label::VOCAB_TERM,
    label::PERSONA,
    label::INTERFACE_SURFACE,
    label::INBOX_ITEM,
];

// ---------------------------------------------------------------------------
// Edge types
// ---------------------------------------------------------------------------

pub mod edge {
    pub const RELATES_TO: &str = "RELATES_TO";
    pub const HIERARCHY: &str = "HIERARCHY";
    pub const IMPLEMENTS: &str = "IMPLEMENTS";
    pub const GOVERNS: &str = "GOVERNS";
    pub const VALIDATES: &str = "VALIDATES";
    /// Hypothesis → Intent: which intents an improvement hypothesis would
    /// touch. Carries the full inspectable meta so per-target grounding and
    /// sync staleness work like every other claim about code.
    pub const TARGETS: &str = "TARGETS";
    /// Persona → Intent: this intent serves this persona. INSPECTABLE — "serving
    /// a persona" is a claim about behavior, not a declaration; earned via
    /// `loom persona serve … ground/issue/independent`. Sync ripple propagates
    /// needs_reverification when the intent's code changes (same one-hop rule as
    /// RELATES_TO).
    pub const SERVES: &str = "SERVES";
    /// Persona → Validation (type=saga): this saga exercises this persona's
    /// end-to-end path. STRUCTURAL — like HIERARCHY, no inspection state; its
    /// value is enabling persona-scoped journey coverage checks.
    pub const JOURNEYS: &str = "JOURNEYS";
    /// Validation → InterfaceSurface: an ordered proof step calls an external
    /// surface. Structural inventory edge; the proof verdict remains on
    /// VALIDATES and saga-stamped RELATES_TO edges.
    pub const CALLS: &str = "CALLS";
}

pub const EDGE_TYPES: &[&str] = &[
    edge::RELATES_TO,
    edge::HIERARCHY,
    edge::IMPLEMENTS,
    edge::GOVERNS,
    edge::VALIDATES,
    edge::TARGETS,
    edge::SERVES,
    edge::JOURNEYS,
    edge::CALLS,
];

// ---------------------------------------------------------------------------
// Agent roles
// ---------------------------------------------------------------------------

/// The agent roles that drive the lifecycle. Every schema field declares the
/// role that *owns* it (the primary writer) — see the required-property tables.
/// A field's owner answers "whose job is it to fill this in?", which makes the
/// metadata self-documenting and lets `loom next` route work by role.
///
/// Roles map onto the work modes: builder→build, analyzer→discovery,
/// fixer→fix, validator→validate, quality→refactor-to-green. `fixer` owns no
/// field outright — it *transitions* analyzer-owned `inspection_status`
/// (failing→passing) and builder-owned `lifecycle` (needs_change→implemented).
pub mod role {
    /// Constructs the graph: intents, hierarchy, codefiles, implements links.
    pub const BUILDER: &str = "builder";
    /// The Socratic loop: grounds edges with criterion/evidence/status.
    pub const ANALYZER: &str = "analyzer";
    /// Resolves failing edges and needs_change intents.
    pub const FIXER: &str = "fixer";
    /// Proves it works: runs validations, confirms intents.
    pub const VALIDATOR: &str = "validator";
    /// The normative/green gate: quality rules and GOVERNS verdicts.
    pub const QUALITY: &str = "quality";
    /// Computed by loom itself (ids, timestamps, scores) — no agent writes it.
    pub const LOOM: &str = "loom";
    /// The shared append-only channel any role may write (notes).
    pub const ANY: &str = "any";
}

/// The agent roles in lifecycle order (excludes the non-agent `loom`/`any`).
pub const ROLES: &[&str] = &[
    role::BUILDER,
    role::ANALYZER,
    role::FIXER,
    role::VALIDATOR,
    role::QUALITY,
];

// ---------------------------------------------------------------------------
// Property names (shared vocabulary across entities)
// ---------------------------------------------------------------------------

pub mod prop {
    // identity / common
    pub const ID: &str = "id";
    pub const NAME: &str = "name";
    pub const DESCRIPTION: &str = "description";
    pub const CREATED_AT: &str = "created_at";
    pub const UPDATED_AT: &str = "updated_at";
    pub const NOTES: &str = "notes";
    // Intent
    pub const ABSTRACTION_LEVEL: &str = "abstraction_level";
    pub const DOMAIN: &str = "domain";
    pub const LAYER: &str = "layer";
    pub const SOURCE_REFS: &str = "source_refs";
    pub const STATUS: &str = "status";
    /// Intent: behavioural facet for completeness. Two families the
    /// happy_path_only audit reads: behavioral (happy | sad | fallback) and
    /// UI-state (populated | empty | loading | error). Open vocabulary —
    /// anything is allowed; empty = unspecified.
    pub const ASPECT: &str = "aspect";
    /// Intent: implementation lifecycle — planned | implemented | needs_change
    /// | deferred (consciously parked; not building now, not superseded).
    /// The prescriptive axis (does the code need to be built/changed?), distinct
    /// from `status` (is this a valid intent?).
    pub const LIFECYCLE: &str = "lifecycle";
    /// Intent: native list of registered VocabTerm names (≤3, sorted, deduped) —
    /// the bounded facet duplicate-responsibility detection collides on. "[]" =
    /// untagged (honest absence; tags are positive evidence only). NOT in the
    /// required-property table (additive; absent on intents from older graphs).
    pub const TAGS: &str = "tags";
    /// Intent: who the behavior is FOR — "user_visible" (a capability the
    /// user can see/feel) | "internal" (machinery serving other intents) |
    /// "" (untriaged). The align interview's framing label: internal intents
    /// are presented as machinery and EXCLUDED from the user interview until
    /// redefined (a redefinition clears the ruling — the new meaning's
    /// audience is unknown again). NOT in the required-property table
    /// (additive; absent on intents from older graphs).
    pub const VISIBILITY: &str = "visibility";
    /// Intent: relationship to the system boundary — "inbound" (exposes a
    /// surface the outside world calls; a provider contract) | "outbound"
    /// (calls an external system; a consumer dependency) | "" (internal —
    /// does not cross the boundary). Owner: builder (a construction-time fact
    /// about the intent, like `aspect`/`layer`). NOT in the required-property
    /// table (additive; absent on intents from older graphs, reads as "").
    pub const BOUNDARY: &str = "boundary";
    // InterfaceSurface
    pub const SURFACE_KIND: &str = "surface_kind";
    pub const METHOD: &str = "method";
    // CodeFile
    pub const PATH: &str = "path";
    pub const LANGUAGE: &str = "language";
    pub const LAST_MODIFIED: &str = "last_modified";
    /// CodeFile: native list of repo-relative paths this file statically
    /// imports — extracted by `loom sync`, consumed by smells/discovery for
    /// undeclared-coupling reconciliation. NOT in the required-property table
    /// (additive in v3; absent on older graphs until the next sync).
    pub const IMPORTS: &str = "imports";
    /// CodeFile: native list of canonical top-level syntax symbols extracted
    /// by `loom sync` (e.g. `fn run`, `class User`). Diagnostic only; not part
    /// of the green condition and NOT in the required-property table.
    pub const SYMBOLS: &str = "symbols";
    /// CodeFile: native list of JSON-encoded SymbolFact objects (label, name,
    /// kind, visibility, source lines, test flag). Diagnostic/accountability
    /// only and NOT in the required-property table.
    pub const SYMBOL_FACTS: &str = "symbol_facts";
    /// CodeFile: FNV-1a 64 hex hash of the file's bytes — `loom sync`'s change
    /// detector (mtime false-flags on checkout; content is the truth). NOT in
    /// the required-property table (additive; absent until the next sync).
    pub const CONTENT_HASH: &str = "content_hash";
    pub const EXTRACTOR_GRADE: &str = "extractor_grade";
    // QualityRule
    pub const DETECTION_LOGIC: &str = "detection_logic";
    pub const SEVERITY: &str = "severity";
    /// QualityRule: how much capability INSPECTING this rule needs —
    /// "low" | "mid" | "high" ("" reads as mid). Owner: quality. A statement
    /// about the work; the harness maps it to models. NOT in the
    /// required-property table (additive; absent on rules from older packs).
    pub const INSPECTION_EFFORT: &str = "inspection_effort";
    /// QualityRule: JSON object with exemplar evidence strings —
    /// `pass`, `independent`, `common_false_positive`. Steers agents away from
    /// laundering weak evidence into green verdicts. NOT in the required-property
    /// table (additive; absent on rules from older packs reads as "").
    pub const EVIDENCE_EXAMPLES: &str = "evidence_examples";
    /// QualityRule: JSON array of keyword groups (each a list of strings). When
    /// the rule passes, the evidence should reference at least one keyword from
    /// each group — the contradiction-check basis. Empty array = no static
    /// expectations (the rule is purely semantic). NOT in the required-property
    /// table (additive; absent on rules from older packs reads as "[]").
    pub const SIGNAL_EXPECTATIONS: &str = "signal_expectations";
    /// GOVERNS: TEXT "true" when the verdict covers all descendant intents
    /// (a roll-up at component/system altitude). The evidence must justify why
    /// the same criterion applies to every child. NOT in the required-property
    /// table (additive; absent on edges from older graphs reads as "").
    pub const COVERS_DESCENDANTS: &str = "covers_descendants";
    /// RELATES_TO: relationship-kind multiset (JSON list) — how two intents are
    /// coupled. Owner: analyzer (judgment tier) + populate (mechanical tier).
    /// (The QualityRule norm category reuses [`KIND`], same column name.)
    pub const KINDS: &str = "kinds";
    /// RELATES_TO: a low-churn flag ("true" = stable). A stable grounded
    /// relationship is exempt from sync's code-change reverification — its
    /// coupling is settled and should not re-open every time either endpoint's
    /// file is touched. Owner: analyzer. Stored TEXT ("" = not stable).
    pub const STABLE: &str = "stable";
    // Validation node
    pub const VALIDATION_TYPE: &str = "validation_type";
    pub const COMMAND: &str = "command";
    pub const LAST_RUN: &str = "last_run";
    pub const LAST_RESULT: &str = "last_result";
    /// Timestamp the EXECUTOR (loom validate) last ran the command — set ONLY
    /// by the executor, never by `loom validation mark` (a hand-mark sets
    /// `last_run` but not this). The proven axis discriminates EXECUTED
    /// (machine-verified) from ASSERTED (hand-marked) on this field: a
    /// command-bearing validation marked passed by hand has `last_run` set but
    /// `last_executed_run` empty, so it reads ASSERTED — closing the
    /// declared-not-executed laundering hole.
    pub const LAST_EXECUTED_RUN: &str = "last_executed_run";
    /// What the executor observed the runner do (G2 falsification-witness):
    /// `discriminating` (the runner asserted >=1 thing) | `ran_inert` (exited 0
    /// with no assertion signal) | "" (never machine-run under G2). Only
    /// `discriminating` feeds the EXECUTED proof tier, so exit-0 alone can no
    /// longer mint EXECUTED. Set ONLY by the executor.
    pub const DISCRIMINATION_STATUS: &str = "discrimination_status";
    // edges (state + meta)
    pub const INSPECTION_STATUS: &str = "inspection_status";
    pub const CRITERION: &str = "criterion";
    pub const CONFIDENCE: &str = "confidence";
    pub const EVIDENCE: &str = "evidence";
    pub const LAST_INSPECTED: &str = "last_inspected";
    pub const INSPECTED_BY: &str = "inspected_by";
    pub const PRIORITY_SCORE: &str = "priority_score";
    /// IMPLEMENTS: finer-than-file anchor (symbol/region) inside the CodeFile.
    pub const LOCATOR: &str = "locator";
    // Note
    pub const KIND: &str = "kind";
    pub const TEXT: &str = "text";
    pub const AUTHOR: &str = "author";
    pub const TARGET_KIND: &str = "target_kind";
    /// Note: optional lane this note is addressed to ("" = everyone) —
    /// builder | analyzer | fixer | validator | quality. Out-of-lane findings
    /// become directed handoff messages; `loom next` surfaces notes addressed
    /// to the work item's owner role first.
    pub const AUDIENCE: &str = "audience";
    pub const TARGET_ID: &str = "target_id";
    // CALLS edge
    pub const STEP_INDEX: &str = "step_index";
    pub const STEP_NAME: &str = "step_name";
    pub const INTENT_ID: &str = "intent_id";
    // Ignore (coverage escape hatch)
    pub const PATTERN: &str = "pattern";
    pub const REASON: &str = "reason";
    // Delegation (federation: subtree owned by another graph)
    /// Delegation: path to the child graph's committed export (loom.graph.json).
    pub const TARGET: &str = "target";
    /// Delegation: content hash of the child export at the last `loom sync` — the
    /// watched-artifact baseline. When the child export's hash changes, sync
    /// ripples staleness to the delegation's seam intents (cross-service ripple).
    pub const EXPORT_HASH: &str = "export_hash";
    /// Delegation: parent intent ids that depend on this child's contract (the
    /// seams). A change to the child export re-opens their claims. JSON list.
    pub const SEAM_INTENTS: &str = "seam_intents";
    // InboxItem
    pub const RAW_TEXT: &str = "raw_text";
    pub const NORMALIZED_CLAIM: &str = "normalized_claim";
    pub const SOURCE: &str = "source";
    pub const LINKS: &str = "links";
    pub const ROUTE_KIND: &str = "route_kind";
    pub const ROUTE_COMMAND: &str = "route_command";
    pub const ROUTE_TARGET_KIND: &str = "route_target_kind";
    pub const ROUTE_TARGET_ID: &str = "route_target_id";
    pub const RESOLUTION: &str = "resolution";
    // Hypothesis (the pre-decision plane)
    /// Hypothesis: what is wrong/suboptimal — falsifiable, provable against
    /// the code as it is NOW.
    pub const CLAIM: &str = "claim";
    /// Hypothesis: the proposed change.
    pub const PROPOSAL: &str = "proposal";
    /// Hypothesis: the measurable result if adopted — the acceptance contract
    /// a post-implementation validation will be written from.
    pub const PREDICTED_OUTCOME: &str = "predicted_outcome";
}

// ---------------------------------------------------------------------------
// Required-property tables — the full property set every entity must carry,
// each paired with the agent role that OWNS it (the primary writer).
// `loom doctor` checks each row of each label/type for property presence (IS
// NULL = drift, since every insert sets every declared property, even to an
// empty string); `loom schema` surfaces the owner so an agent knows its job.
// ---------------------------------------------------------------------------

/// A required property and the role responsible for populating it.
pub type FieldSpec = (&'static str, &'static str);

/// Required properties (with owning role) for a node label, or `&[]` if unknown.
pub fn required_node_props(label: &str) -> &'static [FieldSpec] {
    use prop::*;
    use role::*;
    match label {
        self::label::INTENT => &[
            (ID, LOOM),
            (NAME, BUILDER),
            (DESCRIPTION, BUILDER),
            (ABSTRACTION_LEVEL, BUILDER),
            (DOMAIN, BUILDER),
            (LAYER, BUILDER),
            (SOURCE_REFS, BUILDER),
            (STATUS, VALIDATOR),
            (ASPECT, BUILDER),
            (LIFECYCLE, BUILDER),
            (CREATED_AT, LOOM),
            (UPDATED_AT, LOOM),
        ],
        self::label::CODE_FILE => &[
            (ID, LOOM),
            (PATH, BUILDER),
            (LANGUAGE, LOOM),
            (LAST_MODIFIED, LOOM),
        ],
        self::label::QUALITY_RULE => &[
            (ID, LOOM),
            (NAME, QUALITY),
            (DESCRIPTION, QUALITY),
            (DETECTION_LOGIC, QUALITY),
            (SEVERITY, QUALITY),
        ],
        self::label::VALIDATION => &[
            (ID, LOOM),
            (NAME, BUILDER),
            (DESCRIPTION, BUILDER),
            (VALIDATION_TYPE, BUILDER),
            (COMMAND, BUILDER),
            (LAST_RUN, VALIDATOR),
            (LAST_RESULT, VALIDATOR),
        ],
        self::label::NOTE => &[
            (ID, LOOM),
            (KIND, ANY),
            (TEXT, ANY),
            (AUTHOR, ANY),
            (TARGET_KIND, ANY),
            (TARGET_ID, ANY),
            (CREATED_AT, LOOM),
        ],
        // Note also carries an OPTIONAL `audience` ("" | a role name): a note
        // addressed to a specific lane — the directed-handoff channel. Not in
        // the required table (additive; absent on notes from older graphs).
        self::label::IGNORE => &[
            (ID, LOOM),
            (PATTERN, BUILDER),
            (REASON, BUILDER),
            (AUTHOR, ANY),
            (CREATED_AT, LOOM),
        ],
        self::label::DELEGATION => &[
            (ID, LOOM),
            (PATTERN, BUILDER),
            (TARGET, BUILDER),
            (AUTHOR, ANY),
            (CREATED_AT, LOOM),
        ],
        // Anyone may PROPOSE (claim/proposal/predicted_outcome are `any`);
        // the proof verdict (status + evidence + provenance) is analyzer work,
        // and the prover may not be the proposer (enforced at the command).
        self::label::HYPOTHESIS => &[
            (ID, LOOM),
            (NAME, ANY),
            (CLAIM, ANY),
            (PROPOSAL, ANY),
            (PREDICTED_OUTCOME, ANY),
            (STATUS, ANALYZER),
            (AUTHOR, ANY),
            (EVIDENCE, ANALYZER),
            (LAST_INSPECTED, ANALYZER),
            (INSPECTED_BY, ANALYZER),
            (CREATED_AT, LOOM),
            (UPDATED_AT, LOOM),
        ],
        // The bounded tag vocabulary. `name` is the term (the key intents
        // reference in `tags`); `description` is the contrastive definition an
        // agent disambiguates by when picking from the inlined list.
        self::label::VOCAB_TERM => &[
            (ID, LOOM),
            (NAME, ANY),
            (DESCRIPTION, ANY),
            (AUTHOR, ANY),
            (CREATED_AT, LOOM),
        ],
        // A user persona: a named audience segment. Connects to intents via
        // SERVES (inspectable) and to saga Validations via JOURNEYS (structural).
        self::label::PERSONA => &[
            (ID, LOOM),
            (NAME, BUILDER),
            (DESCRIPTION, BUILDER),
            (AUTHOR, ANY),
            (CREATED_AT, LOOM),
            (UPDATED_AT, LOOM),
        ],
        self::label::INTERFACE_SURFACE => &[
            (ID, LOOM),
            (NAME, BUILDER),
            (DESCRIPTION, BUILDER),
            (SURFACE_KIND, BUILDER),
            (METHOD, BUILDER),
            (TARGET, BUILDER),
            (CREATED_AT, LOOM),
            (UPDATED_AT, LOOM),
        ],
        self::label::INBOX_ITEM => &[
            (ID, LOOM),
            (RAW_TEXT, ANY),
            (NORMALIZED_CLAIM, ANY),
            (KIND, ANY),
            (STATUS, ANY),
            (SOURCE, ANY),
            (AUTHOR, ANY),
            (TAGS, ANY),
            (LINKS, ANY),
            (ROUTE_KIND, ANY),
            (ROUTE_COMMAND, ANY),
            (ROUTE_TARGET_KIND, ANY),
            (ROUTE_TARGET_ID, ANY),
            (RESOLUTION, ANY),
            (CREATED_AT, LOOM),
            (UPDATED_AT, LOOM),
        ],
        _ => &[],
    }
}

/// Required properties (with owning role) for an edge type, or `&[]` if unknown.
pub fn required_edge_props(edge: &str) -> &'static [FieldSpec] {
    use prop::*;
    use role::*;
    match edge {
        self::edge::RELATES_TO => &[
            (INSPECTION_STATUS, ANALYZER),
            (CRITERION, ANALYZER),
            (CONFIDENCE, ANALYZER),
            (EVIDENCE, ANALYZER),
            (LAST_INSPECTED, ANALYZER),
            (INSPECTED_BY, ANALYZER),
            (PRIORITY_SCORE, LOOM),
            (NOTES, ANY),
            (CREATED_AT, LOOM),
        ],
        // HIERARCHY is a structural tree edge, enforced at insert — it is never
        // "inspected", so it carries no inspection_status (dropped in v3).
        self::edge::HIERARCHY => &[(NOTES, ANY), (CREATED_AT, LOOM)],
        self::edge::IMPLEMENTS => &[
            (INSPECTION_STATUS, ANALYZER),
            (CRITERION, ANALYZER),
            (CONFIDENCE, ANALYZER),
            (EVIDENCE, ANALYZER),
            (LAST_INSPECTED, ANALYZER),
            (INSPECTED_BY, ANALYZER),
            (LOCATOR, BUILDER),
            (NOTES, ANY),
            (CREATED_AT, LOOM),
        ],
        self::edge::GOVERNS => &[
            (INSPECTION_STATUS, QUALITY),
            (CRITERION, QUALITY),
            (CONFIDENCE, QUALITY),
            (EVIDENCE, QUALITY),
            (LAST_INSPECTED, QUALITY),
            (INSPECTED_BY, QUALITY),
            (COVERS_DESCENDANTS, QUALITY),
            (NOTES, ANY),
            (CREATED_AT, LOOM),
        ],
        // VALIDATES.inspection_status is the per-intent proof verdict (distinct
        // from the Validation node's last_result, which is its last execution —
        // a node is reusable across intents). Owned by the validator.
        self::edge::VALIDATES => &[
            (INSPECTION_STATUS, VALIDATOR),
            (NOTES, ANY),
            (CREATED_AT, LOOM),
        ],
        // TARGETS mirrors GOVERNS: a claim about code, inspectable + sync-stale-able.
        self::edge::TARGETS => &[
            (INSPECTION_STATUS, ANALYZER),
            (CRITERION, ANALYZER),
            (CONFIDENCE, ANALYZER),
            (EVIDENCE, ANALYZER),
            (LAST_INSPECTED, ANALYZER),
            (INSPECTED_BY, ANALYZER),
            (NOTES, ANY),
            (CREATED_AT, LOOM),
        ],
        // SERVES: Persona → Intent. Inspectable — "this intent serves this
        // persona" is a claim that must be verified against actual behavior.
        // Sync ripple: code changes → SERVES edges → needs_reverification (the
        // persona serving claim was earned against the old code). Inspector role
        // is analyzer (behavioral claim, same as RELATES_TO).
        self::edge::SERVES => &[
            (INSPECTION_STATUS, ANALYZER),
            (CRITERION, ANALYZER),
            (CONFIDENCE, ANALYZER),
            (EVIDENCE, ANALYZER),
            (LAST_INSPECTED, ANALYZER),
            (INSPECTED_BY, ANALYZER),
            (NOTES, ANY),
            (CREATED_AT, LOOM),
        ],
        // JOURNEYS: Persona → Validation (type=saga). Structural — no inspection
        // state (the saga run IS the proof; the VALIDATES edges carry the
        // verdict). Like HIERARCHY: a tree/binding edge, enforced at insert.
        self::edge::JOURNEYS => &[(NOTES, ANY), (CREATED_AT, LOOM)],
        // CALLS: Validation → InterfaceSurface. Structural inventory edge for
        // an ordered saga step. The Validation/VALIDATES/RELATES_TO path carries
        // the actual proof verdict.
        self::edge::CALLS => &[
            (STEP_INDEX, BUILDER),
            (STEP_NAME, BUILDER),
            (INTENT_ID, BUILDER),
            (NOTES, ANY),
            (CREATED_AT, LOOM),
        ],
        _ => &[],
    }
}
