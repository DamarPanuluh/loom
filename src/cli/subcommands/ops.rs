use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum HookCmd {
    /// Install idempotent structural sync hooks and optional local CI.
    Install {
        /// Install a blocking pre-push hook that executes this repository-relative script.
        #[arg(long, value_name = "PATH")]
        pre_push: Option<PathBuf>,
    },
    /// Remove only hooks previously installed by Loom.
    Remove,
}

#[derive(Subcommand, Debug)]
pub enum McpCmd {
    /// Speak MCP over stdio until stdin closes. Register it with an MCP client
    /// as `loom mcp serve` (add `--graph <path>` when the client's working
    /// directory is not the repo).
    Serve,
    /// Drive one complete MCP session through the real stdio serve loop and
    /// return its ordered responses as one JSON document.
    Transcript {
        /// JSON array of JSON-RPC 2.0 request objects, in session order.
        #[arg(long, value_name = "JSON")]
        requests_json: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum WikiCmd {
    /// Plan (create or re-ground) a draft wiki page and the intents it will
    /// document. Leaves it `draft` for `wiki next`; write the prose, then
    /// `wiki record`.
    Plan {
        /// Page title (its stable name).
        title: String,
        /// Output path for the authored markdown (e.g. docs/wiki/architecture.md).
        #[arg(long)]
        path: String,
        /// An intent this page documents (repeatable: --covers A --covers B).
        #[arg(long = "covers")]
        covers: Vec<String>,
    },
    /// Mark an authored page fresh — stamp the scope fingerprint of everything it
    /// documents (the prose must already be written at the page's path).
    Record {
        /// Page title.
        title: String,
    },
    /// Emit a brief for the next page that needs writing (a draft, or a stale
    /// page whose documented scope drifted).
    Next,
    /// List wiki pages and their freshness.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Remove a wiki page by title.
    Remove {
        /// Page title.
        title: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum GraphCmd {
    /// Link an upstream graph via its committed export (loom.graph.json).
    Link {
        /// Path to the upstream `loom.graph.json`.
        path: PathBuf,
        /// Human alias for this upstream (default: the upstream graph's name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Unlink an upstream graph by alias or graph-id.
    ///
    /// Default keeps UpstreamIntent shadows orphaned (doctor flags them). Pass
    /// `--prune` when the upstream is permanently gone so shadows are disposed
    /// in the same step; remaining DependsOn claims refuse unless `--cascade`.
    Unlink {
        /// Alias or graph-id of the upstream to remove.
        key: String,
        /// Also delete this upstream's UpstreamIntent shadow nodes.
        #[arg(long)]
        prune: bool,
        /// With `--prune`, also cascade-delete DependsOn edges that still
        /// target those shadows (default refuses and lists the blocked ones).
        #[arg(long, requires = "prune")]
        cascade: bool,
    },
    /// Dispose orphan UpstreamIntent shadows left after `graph unlink`.
    ///
    /// Shadows whose alias is no longer in the upstream registry are removed.
    /// Orphans still targeted by local DependsOn edges are left in place
    /// unless `--cascade` is set (which removes those edges too).
    PruneOrphans {
        /// Only dispose orphans for this former alias (default: all orphans).
        #[arg(long)]
        alias: Option<String>,
        /// Also cascade-delete DependsOn edges that still target orphan shadows.
        #[arg(long)]
        cascade: bool,
    },
    /// List linked upstream graphs.
    List,
}

#[derive(Subcommand, Debug)]
pub enum AuditCmd {
    /// Inspect or accept a historical integrity incident.
    Incident {
        #[command(subcommand)]
        cmd: AuditIncidentCmd,
    },
    /// Seal a typed batch authorization over a legacy judgment burst.
    ///
    /// Requires contemporaneous evidence (journal events, apply/command
    /// records, validation runs, import tickets). A prose note written
    /// afterward is acknowledgment, not sufficient proof. Does not rewrite
    /// fact timestamps.
    ///
    /// A seal written AFTER the burst's final fact is accepted only when the
    /// authority is human and the evidence is a trusted digest-bound
    /// `batch_intent` record — the human-gated batch path's recorded
    /// HumanDecision for this exact subject set, predating the burst.
    /// The burst actor's own later seal is never accepted.
    AttestBurst {
        /// Burst key as reported by audit: `{actor}@{YYYY-MM-DDTHH:MM}`.
        subject: String,
        /// ratification | adjudication
        #[arg(long)]
        claim: String,
        /// Shared batch criterion / predicate.
        #[arg(long)]
        criterion: String,
        /// Contemporaneous evidence refs (repeatable). Prefer `journal:<id>`.
        #[arg(long = "evidence", required = true)]
        evidence: Vec<String>,
        /// Who authorized the batch (human for ratification).
        #[arg(long)]
        authority: String,
        /// Who executed the writes (often the LOOM_AGENT / llm).
        #[arg(long)]
        executor: String,
        /// The human's answer, when the seal is mediated by a host answer
        /// (same gate as `loom intent ratify --human-decision`). Without it,
        /// a retrospective seal demands the interactive typed challenge.
        #[arg(long)]
        human_decision: Option<String>,
        /// Required when the batch claims mechanical routing safety.
        #[arg(long)]
        routing_class: Option<String>,
        /// Permitted operation (default: ratify / verdict by claim).
        #[arg(long)]
        operation: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum AuditIncidentCmd {
    /// Accept an exact live burst as disclosed history, never as authorization.
    Accept {
        /// Burst key as reported by audit: `{actor}@{YYYY-MM-DDTHH:MM}`.
        subject: String,
        /// ratification | adjudication
        #[arg(long)]
        claim: String,
        /// Why this historical integrity exception is consciously accepted.
        #[arg(long)]
        reason: String,
        /// The human's exact answer from the host conversation. Without it,
        /// a direct terminal invocation demands the typed human challenge.
        #[arg(long)]
        human_decision: Option<String>,
    },
    /// List every disclosed incident, including imported history.
    List,
    /// Show the disclosure for one burst and claim.
    Show {
        /// Burst key as reported by audit: `{actor}@{YYYY-MM-DDTHH:MM}`.
        subject: String,
        /// ratification | adjudication
        #[arg(long)]
        claim: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum RoleCmd {
    /// Claim a role for the current LOOM_AGENT_PROFILE. Requires
    /// LOOM_AGENT=llm:<role>; every later loom command run under that
    /// identity refreshes the lease (heartbeat).
    Claim {
        role: ClaimRoleArg,
        /// Deliberately take over a lease whose heartbeat has expired.
        #[arg(long)]
        take_stale: bool,
    },
    /// Release the current profile's lease on a role.
    Release { role: ClaimRoleArg },
    /// Every claimable role: holder, freshness, and the queue debt behind it.
    List,
}

/// The six claimable driver roles — the legal `LOOM_AGENT=llm:<role>` lanes.
/// `sync` (loom's derived writer) and `human` are never claimable.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ClaimRoleArg {
    Builder,
    Analyzer,
    Fixer,
    Validator,
    Quality,
    Rectify,
}

impl ClaimRoleArg {
    pub fn owner_role(self) -> crate::registry::OwnerRole {
        use crate::registry::OwnerRole;
        match self {
            ClaimRoleArg::Builder => OwnerRole::Builder,
            ClaimRoleArg::Analyzer => OwnerRole::Analyzer,
            ClaimRoleArg::Fixer => OwnerRole::Fixer,
            ClaimRoleArg::Validator => OwnerRole::Validator,
            ClaimRoleArg::Quality => OwnerRole::Quality,
            ClaimRoleArg::Rectify => OwnerRole::Rectify,
        }
    }
}
