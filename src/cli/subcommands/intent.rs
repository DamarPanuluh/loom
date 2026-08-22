use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum IntentCmd {
    /// What stands on this behavior: every intent that transitively requires it,
    /// is a scenario of it, or decomposes into it — nearest first, each with
    /// whether it currently has a passing proof. The intent-graph twin of
    /// `loom impact`, which answers the same question for code.
    Dependents {
        key: String,
        /// How many edges to walk back (default 5).
        #[arg(long, default_value_t = 5)]
        depth: usize,
    },
    /// Add an intent.
    Add {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        description: String,
        /// system | component | feature | cross_cutting
        #[arg(long, default_value = "feature")]
        level: String,
        /// planned | implemented | needs_change | blocked
        #[arg(long, default_value = "planned")]
        lifecycle: String,
        /// user_visible | internal
        #[arg(long)]
        visibility: Option<String>,
        /// Architecture layer label (arms the layering detector once an order is declared).
        #[arg(long)]
        layer: Option<String>,
        /// Scenario aspect: happy | sad | fallback | edge_case
        #[arg(long)]
        aspect: Option<String>,
        /// Permit a symbol-looking name (requires a behavioral description).
        #[arg(long)]
        allow_symbol_name: bool,
    },
    /// Show an intent by id, name, or unique fragment.
    Show { key: String },
    /// Deliberately waive a completeness axis for this intent (recorded, re-opens
    /// when the intent's meaning changes).
    Waive {
        key: String,
        /// scenarios | prerequisites | boundary | proof | journey | questions
        axis: String,
        #[arg(long)]
        reason: String,
    },
    /// Declare that an Intent deliberately has no Journey ancestry. This is a
    /// human product decision, not a general completeness waiver.
    JourneyExempt {
        key: String,
        /// Stable exemption class, such as `infrastructure` or `repository_maintenance`.
        #[arg(long)]
        kind: String,
        /// Why this behavior is intentionally not rooted in a user Journey.
        #[arg(long)]
        reason: String,
        /// Exact answer the human gave in the host conversation. Required for
        /// non-interactive or llm:* execution; omit for a direct TTY challenge.
        #[arg(long)]
        human_decision: Option<String>,
    },
    /// Require Journey ancestry again by withdrawing a prior exemption.
    JourneyRequire {
        key: String,
        /// Why Journey ancestry is required again.
        #[arg(long)]
        reason: String,
        /// Exact answer the human gave in the host conversation. Required for
        /// non-interactive or llm:* execution; omit for a direct TTY challenge.
        #[arg(long)]
        human_decision: Option<String>,
    },
    /// Reactivate a retired (deprecated) intent → planned.
    Reactivate {
        key: String,
        #[arg(long)]
        reason: String,
    },
    /// List intents.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Ratify an intent (or --all unratified): the human authority's
    /// evidence-bearing "yes, this is wanted". A host LLM may record an
    /// explicit --human-decision, but may never make the decision itself.
    Ratify {
        key: Option<String>,
        /// Ratify every active unratified intent (bulk grandfathering).
        #[arg(long)]
        all: bool,
        /// Why this behavior is wanted: an utterance, source doc, or decision.
        #[arg(long)]
        evidence: Option<String>,
        /// Exact answer the human gave in the host conversation. Required for
        /// non-interactive or llm:* execution; omit for a direct TTY challenge.
        #[arg(long)]
        human_decision: Option<String>,
    },
    /// Say a behavior is NOT wanted. The cheap, high-leverage half of the
    /// authority: no typed challenge — the substantive reason IS the act —
    /// and every place the code still performs it becomes tracked work.
    Reject {
        key: String,
        /// Why this is not wanted. Recorded verbatim on the rejection.
        #[arg(long)]
        reason: String,
        /// Exact answer the human gave in the host conversation. Required for
        /// non-interactive or llm:* execution.
        #[arg(long)]
        human_decision: Option<String>,
    },
    /// One mutation verb for an intent. --description redefines (ripples one
    /// hop; --reword: same concept, no ripple). --name relabels, --level /
    /// --visibility / --aspect correct attributes, --lifecycle moves the
    /// prescriptive state — none of those ripple. Every update records --reason.
    Update {
        key: String,
        #[arg(long)]
        description: Option<String>,
        /// New name (a label change; the description stays the criterion).
        #[arg(long)]
        name: Option<String>,
        /// system | component | feature | cross_cutting
        #[arg(long)]
        level: Option<String>,
        /// user_visible | internal
        #[arg(long)]
        visibility: Option<String>,
        /// happy | sad | fallback | edge_case
        #[arg(long)]
        aspect: Option<String>,
        /// planned | implemented | needs_change | blocked
        #[arg(long)]
        lifecycle: Option<String>,
        /// Rectify-lane handoff: `escalated` moves a discovered behavior to
        /// human ratify; `clear` removes the handoff marker.
        #[arg(long)]
        rectify: Option<String>,
        #[arg(long)]
        reason: String,
        /// Clearer words, same concept: no ripple.
        #[arg(long)]
        reword: bool,
    },
    /// Hard-delete a mistaken intent (typo/duplicate only). Retire superseded
    /// design instead; refuses intents that still have hierarchy children.
    Remove {
        key: String,
        #[arg(long)]
        reason: String,
    },
    /// Retire superseded design (status → deprecated).
    Retire {
        key: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        replaced_by: Option<String>,
    },
    /// Ratify the current meaning (resets the alignment clock).
    Confirm { key: String },
    /// Record an evidence-backed semantic-impact assessment after a code change.
    /// `criterion_changed` routes wantedness back to the human ratify queue.
    Impact {
        key: String,
        /// preserved | changed_within_intent | criterion_changed
        #[arg(long)]
        classification: String,
        /// Concrete code or runtime evidence supporting the classification.
        #[arg(long)]
        evidence: String,
    },
    /// Tag from the registered vocabulary.
    Tag {
        #[command(subcommand)]
        cmd: IntentTagCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum IntentTagCmd {
    /// Tag an intent with a registered vocabulary term.
    Add { key: String, term: String },
    /// Remove a vocabulary tag from an intent.
    Remove { key: String, term: String },
}
