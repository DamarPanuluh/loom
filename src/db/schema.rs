//! The loom graph schema, declared in ONE place.
//!
//! grafeo is a schema-*optional* store: it will happily accept any label or
//! property you write, so nothing at the DB layer stops a typo from silently
//! creating a wrong field (a later read just returns `Null`). loom is the *sole*
//! author of the graph — the LLM never writes GQL, it only calls structured
//! subcommands — so schema stability is loom's responsibility, enforced in three
//! layers:
//!   1. This module declares the entire vocabulary (labels, edge types,
//!      properties, version) once. New code and `loom doctor` reference it.
//!   2. `loom doctor` (see `commands::doctor`) verifies the *live* graph against
//!      these declarations and catches drift the type system can't.
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
pub const SCHEMA_VERSION: &str = "3";

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
    /// Sentinel node marking an initialised graph (carries the schema version,
    /// the graph's identity, and its custody).
    pub const META: &str = "LoomMeta";
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
}

pub const EDGE_TYPES: &[&str] = &[
    edge::RELATES_TO,
    edge::HIERARCHY,
    edge::IMPLEMENTS,
    edge::GOVERNS,
    edge::VALIDATES,
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
    pub const VERSION: &str = "version";
    /// LoomMeta: when the graph was last reconciled against disk (`loom sync`).
    pub const LAST_SYNCED: &str = "last_synced";
    // Intent
    pub const ABSTRACTION_LEVEL: &str = "abstraction_level";
    pub const DOMAIN: &str = "domain";
    pub const SOURCE_REFS: &str = "source_refs";
    pub const STATUS: &str = "status";
    /// Intent: behavioural facet for completeness — happy | sad | fallback | …
    /// (open vocabulary; empty = unspecified).
    pub const ASPECT: &str = "aspect";
    /// Intent: implementation lifecycle — planned | implemented | needs_change.
    /// The prescriptive axis (does the code need to be built/changed?), distinct
    /// from `status` (is this a valid intent?).
    pub const LIFECYCLE: &str = "lifecycle";
    // CodeFile
    pub const PATH: &str = "path";
    pub const LANGUAGE: &str = "language";
    pub const LAST_MODIFIED: &str = "last_modified";
    /// CodeFile: JSON array of repo-relative paths this file statically
    /// imports — extracted by `loom sync`, consumed by smells/discovery for
    /// undeclared-coupling reconciliation. NOT in the required-property table
    /// (additive in v3; absent on older graphs until the next sync).
    pub const IMPORTS: &str = "imports";
    /// CodeFile: FNV-1a 64 hex hash of the file's bytes — `loom sync`'s change
    /// detector (mtime false-flags on checkout; content is the truth). NOT in
    /// the required-property table (additive; absent until the next sync).
    pub const CONTENT_HASH: &str = "content_hash";
    // QualityRule
    pub const DETECTION_LOGIC: &str = "detection_logic";
    pub const SEVERITY: &str = "severity";
    /// QualityRule: how much capability INSPECTING this rule needs —
    /// "low" | "mid" | "high" ("" reads as mid). Owner: quality. A statement
    /// about the work; the harness maps it to models. NOT in the
    /// required-property table (additive; absent on rules from older packs).
    pub const INSPECTION_EFFORT: &str = "inspection_effort";
    // Validation node
    pub const VALIDATION_TYPE: &str = "validation_type";
    pub const COMMAND: &str = "command";
    pub const LAST_RUN: &str = "last_run";
    pub const LAST_RESULT: &str = "last_result";
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
    // Ignore (coverage escape hatch)
    pub const PATTERN: &str = "pattern";
    pub const REASON: &str = "reason";
    // Delegation (federation: subtree owned by another graph)
    /// Delegation: path to the child graph's committed export (loom.graph.json).
    pub const TARGET: &str = "target";
    // LoomMeta identity + custody (federation; backfilled on `loom init`)
    /// Stable identity of THIS graph (uuid) — what other looms reference.
    pub const GRAPH_ID: &str = "graph_id";
    /// Human name of this graph (defaults to the repo directory name).
    pub const GRAPH_NAME: &str = "graph_name";
    /// "owned" (we can change this code) | "observed" (mapping someone else's
    /// code: build/fix lanes are disabled — findings, not fixes).
    pub const CUSTODY: &str = "custody";
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
            (ID, LOOM), (NAME, BUILDER), (DESCRIPTION, BUILDER),
            (ABSTRACTION_LEVEL, BUILDER), (DOMAIN, BUILDER), (SOURCE_REFS, BUILDER),
            (STATUS, VALIDATOR), (ASPECT, BUILDER), (LIFECYCLE, BUILDER),
            (CREATED_AT, LOOM), (UPDATED_AT, LOOM),
        ],
        self::label::CODE_FILE => &[
            (ID, LOOM), (PATH, BUILDER), (LANGUAGE, LOOM), (LAST_MODIFIED, LOOM),
        ],
        self::label::QUALITY_RULE => &[
            (ID, LOOM), (NAME, QUALITY), (DESCRIPTION, QUALITY),
            (DETECTION_LOGIC, QUALITY), (SEVERITY, QUALITY),
        ],
        self::label::VALIDATION => &[
            (ID, LOOM), (NAME, BUILDER), (DESCRIPTION, BUILDER),
            (VALIDATION_TYPE, BUILDER), (COMMAND, BUILDER),
            (LAST_RUN, VALIDATOR), (LAST_RESULT, VALIDATOR),
        ],
        self::label::NOTE => &[
            (ID, LOOM), (KIND, ANY), (TEXT, ANY), (AUTHOR, ANY),
            (TARGET_KIND, ANY), (TARGET_ID, ANY), (CREATED_AT, LOOM),
        ],
        // Note also carries an OPTIONAL `audience` ("" | a role name): a note
        // addressed to a specific lane — the directed-handoff channel. Not in
        // the required table (additive; absent on notes from older graphs).
        self::label::IGNORE => &[
            (ID, LOOM), (PATTERN, BUILDER), (REASON, BUILDER),
            (AUTHOR, ANY), (CREATED_AT, LOOM),
        ],
        self::label::DELEGATION => &[
            (ID, LOOM), (PATTERN, BUILDER), (TARGET, BUILDER),
            (AUTHOR, ANY), (CREATED_AT, LOOM),
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
            (ID, LOOM), (INSPECTION_STATUS, ANALYZER), (CRITERION, ANALYZER),
            (CONFIDENCE, ANALYZER), (EVIDENCE, ANALYZER), (LAST_INSPECTED, ANALYZER),
            (INSPECTED_BY, ANALYZER), (PRIORITY_SCORE, LOOM), (NOTES, ANY),
            (CREATED_AT, LOOM),
        ],
        // HIERARCHY is a structural tree edge, enforced at insert — it is never
        // "inspected", so it carries no inspection_status (dropped in v3).
        self::edge::HIERARCHY => &[(ID, LOOM), (NOTES, ANY), (CREATED_AT, LOOM)],
        self::edge::IMPLEMENTS => &[
            (ID, LOOM), (INSPECTION_STATUS, ANALYZER), (CRITERION, ANALYZER),
            (CONFIDENCE, ANALYZER), (EVIDENCE, ANALYZER), (LAST_INSPECTED, ANALYZER),
            (INSPECTED_BY, ANALYZER), (LOCATOR, BUILDER), (NOTES, ANY),
            (CREATED_AT, LOOM),
        ],
        self::edge::GOVERNS => &[
            (ID, LOOM), (INSPECTION_STATUS, QUALITY), (CRITERION, QUALITY),
            (CONFIDENCE, QUALITY), (EVIDENCE, QUALITY), (LAST_INSPECTED, QUALITY),
            (INSPECTED_BY, QUALITY), (NOTES, ANY), (CREATED_AT, LOOM),
        ],
        // VALIDATES.inspection_status is the per-intent proof verdict (distinct
        // from the Validation node's last_result, which is its last execution —
        // a node is reusable across intents). Owned by the validator.
        self::edge::VALIDATES => &[
            (ID, LOOM), (INSPECTION_STATUS, VALIDATOR), (NOTES, ANY), (CREATED_AT, LOOM),
        ],
        _ => &[],
    }
}

// ---------------------------------------------------------------------------
// Meta sentinel
// ---------------------------------------------------------------------------

/// Query that returns the schema version if the graph is initialised.
pub const CHECK_INITIALIZED: &str = "MATCH (m:LoomMeta) RETURN m.version LIMIT 1";

/// Insert the `LoomMeta` sentinel node that marks a graph as initialised.
/// `last_synced` starts empty (never synced). Identity (graph_id/name) and
/// custody are stamped at init so other looms can reference this graph.
pub fn insert_meta(
    version: &str,
    created_at: &str,
    graph_id: &str,
    graph_name: &str,
    custody: &str,
) -> String {
    format!(
        "INSERT (:{meta} {{{version_k}: '{version}', {created_k}: '{created}', \
         {synced_k}: '', {gid_k}: '{gid}', {gname_k}: '{gname}', {custody_k}: '{custody}'}})",
        meta = label::META,
        version_k = prop::VERSION,
        created_k = prop::CREATED_AT,
        synced_k = prop::LAST_SYNCED,
        gid_k = prop::GRAPH_ID,
        gname_k = prop::GRAPH_NAME,
        custody_k = prop::CUSTODY,
        version = esc(version),
        created = esc(created_at),
        gid = esc(graph_id),
        gname = esc(graph_name),
        custody = esc(custody),
    )
}

// ---------------------------------------------------------------------------
// String escaping for GQL literals
// ---------------------------------------------------------------------------

/// Escape a string for embedding in a GQL single-quoted literal.
/// Escapes backslashes first, then single quotes.
pub fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}
