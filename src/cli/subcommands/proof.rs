use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum RuleCmd {
    /// Seed a pre-authored rule pack (e.g. iso5055).
    Seed { pack: String },
    /// Record a verdict; the outcome is positional (creates the governs edge if needed).
    Verdict {
        rule: String,
        intent: String,
        /// passing | failing | independent
        outcome: String,
        #[arg(long, default_value = "")]
        criterion: String,
        #[arg(long, default_value = "")]
        evidence: String,
        #[arg(long, default_value_t = 0.9)]
        confidence: f64,
    },
    /// List quality rules.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Show one rule with its guidance fields.
    Show { key: String },
    /// Author a one-off quality rule (outside a pack).
    Add {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        category: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Edit a quality rule. Customizing a builtin/seeded rule is allowed but
    /// will intentionally surface in pack_drift until reseeded or accepted.
    Update {
        key: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        category: Option<String>,
        #[arg(long)]
        severity: Option<String>,
        #[arg(long)]
        effort: Option<String>,
        /// Replacement inspection_guide text.
        #[arg(long)]
        guide: Option<String>,
        /// Replacement detection_hints array; repeat --hint to set multiple.
        #[arg(long = "hint")]
        hint: Vec<String>,
        /// Replacement patterns array; repeat --pattern to set multiple.
        #[arg(long = "pattern")]
        pattern: Vec<String>,
        #[arg(long)]
        reason: String,
    },
    /// Remove a quality rule (and its governs edges).
    Remove { key: String },
    /// Stop a rule governing an intent (removes the governs edge by name).
    Unlink { rule: String, intent: String },
    /// Judge one pre-screen hit as not-what-the-rule-means, keyed by the
    /// matched text's content hash: judged once, it answers the same text on
    /// every future scan (any rule×intent pair, any shifted line) and expires
    /// by construction when the matched text changes.
    Suppress {
        rule: String,
        /// The matched text as shown in the hit listing (after `    `).
        #[arg(long)]
        excerpt: String,
        /// Why this hit is not what the rule means. The audit, not a formality.
        #[arg(long)]
        reason: String,
    },
    /// Withdraw a suppression; the hit re-opens on the next scan.
    Unsuppress {
        rule: String,
        /// Content hash (a prefix works) or the exact matched text.
        #[arg(long)]
        key: String,
    },
    /// The auditable ledger of hit suppressions, optionally scoped to one rule.
    Suppressions { rule: Option<String> },
}

#[derive(Subcommand, Debug)]
pub enum ValidationCmd {
    /// Add a validation and link it to an intent.
    Add {
        #[arg(long)]
        name: String,
        /// test | assertion | benchmark | manual_check | journey | scenario | contract
        #[arg(long, default_value = "test")]
        r#type: String,
        #[arg(long, default_value = "")]
        command: String,
        #[arg(long)]
        intent: String,
    },
    /// Run one validation, every validation for an intent, or --all pending.
    Run {
        #[arg(default_value = "")]
        key: String,
        /// Run every pending validation.
        #[arg(long)]
        all: bool,
    },
    /// Record a proof result by hand; the outcome is positional.
    Verdict {
        key: String,
        /// passed | failed | blocked
        outcome: String,
        #[arg(long, default_value = "")]
        evidence: String,
        #[arg(long, default_value = "")]
        reason: String,
    },
    /// Show a validation (type/command/status + the intent it validates).
    Show { key: String },
    /// Edit a validation's type and/or command.
    Update {
        key: String,
        #[arg(long)]
        r#type: Option<String>,
        #[arg(long)]
        command: Option<String>,
    },
    /// Unlink a validation from an intent (removes the validates edge by name).
    Unlink { validation: String, intent: String },
    /// Remove a validation (e.g. a stale proof). Cascades its validates/calls edges.
    Remove { key: String },
    /// List validations.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
}

#[derive(Subcommand, Debug)]
pub enum HypothesisCmd {
    /// Add a hypothesis targeting an intent.
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        claim: String,
        #[arg(long, default_value = "")]
        proposal: String,
        #[arg(long, default_value = "")]
        predicted_outcome: String,
        #[arg(long)]
        target: String,
    },
    /// Prove or refute; the outcome is positional.
    Prove {
        key: String,
        /// supported | refuted
        outcome: String,
        #[arg(long, default_value = "")]
        evidence: String,
    },
    /// Adopt a supported hypothesis (spawns a planned intent).
    Adopt {
        key: String,
        #[arg(long)]
        spawned: Option<String>,
    },
    /// Reject a hypothesis.
    Reject {
        key: String,
        #[arg(long)]
        reason: String,
    },
    /// Refine a proposed hypothesis. Proven/adopted/rejected hypotheses are
    /// history; remove only mistaken hypotheses.
    Update {
        key: String,
        #[arg(long)]
        claim: Option<String>,
        #[arg(long)]
        proposal: Option<String>,
        #[arg(long)]
        predicted_outcome: Option<String>,
        #[arg(long)]
        reason: String,
    },
    /// Delete a mistaken hypothesis. Cascades its target edges.
    Remove { key: String },
    /// Show a hypothesis (claim/proposal/predicted outcome + target).
    Show { key: String },
    /// List hypotheses.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
}
