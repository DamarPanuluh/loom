use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum EdgeCmd {
    /// Ground an intent in a codefile (creates an uninspected implements edge).
    Implement {
        intent: String,
        codefile: String,
        #[arg(long)]
        locator: Option<String>,
        /// Grounding role: realizes (default; owns coverage) | consumes | configures | verifies
        #[arg(long)]
        role: Option<String>,
    },
    /// Attach validation-specific code evidence used to derive the S3 call witness.
    Exercises {
        validation: String,
        codefile: String,
        /// Optional entry-point symbol in the exercised file.
        #[arg(long)]
        locator: Option<String>,
    },
    /// Put an interface surface under contract: a validation that exercises it
    /// (creates a `calls` edge). When the code behind the surface changes, sync
    /// resets this contract — the integration-monitoring signal.
    Call { validation: String, surface: String },
    /// Remove an edge (prune a redundant grounding/relationship). Asserted edges
    /// only — derived edges are rebuilt by `loom sync`.
    Remove {
        edge_id: String,
        /// Record why (writes a decision note on the source node).
        #[arg(long)]
        reason: Option<String>,
    },
    /// Correct the locator (symbol) on an asserted edge — e.g. a moved grounding.
    SetLocator { edge_id: String, locator: String },
    /// Add a relationship edge between two intents.
    /// kind: hierarchy | requires | scenario-of | variant-of | triggers | sequence | relates
    Relate {
        kind: String,
        from: String,
        to: String,
    },
    /// Record a verdict on an existing edge (ground|issue|independent).
    Verdict {
        edge_id: String,
        /// ground | issue | independent
        verdict: String,
        #[arg(long, default_value = "")]
        criterion: String,
        #[arg(long, default_value = "")]
        evidence: String,
        #[arg(long, default_value_t = 0.9)]
        confidence: f64,
    },
    /// Inspect two intents: ensure a `relates` edge and record a verdict.
    Explore {
        a: String,
        b: String,
        /// ground | issue | independent
        verdict: String,
        #[arg(long, default_value = "")]
        criterion: String,
        #[arg(long, default_value = "")]
        evidence: String,
        #[arg(long, default_value_t = 0.9)]
        confidence: f64,
    },
    /// Show one edge.
    Show { edge_id: String },
    /// List edges.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Keep only edges incident to this intent (id, name, or unique fragment).
        #[arg(long)]
        intent: Option<String>,
        /// Keep only edges incident to this codefile (id, path, or unique fragment).
        #[arg(long)]
        codefile: Option<String>,
    },
    /// Reclassify a grounding edge's role (realizes|consumes|configures|verifies).
    /// Keeps the edge + verdict history; a changed role re-opens the claim
    /// (stale_cause: role_changed) to be re-verdicted under the new criterion.
    SetRole {
        edge_id: String,
        /// realizes | consumes | configures | verifies
        role: String,
        #[arg(long)]
        reason: String,
    },
    /// Rehome a grounding edge to a successor intent (a true mis-attachment,
    /// not just the wrong role). Supersede-not-delete: the old edge keeps its
    /// history but stops counting; a fresh unverified edge on the successor
    /// carries stale_cause: rehomed.
    Rehome {
        edge_id: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        reason: String,
    },
    /// Re-point an asserted edge's target at a successor node — the recorded
    /// operation of a file rename/split. In place: the edge keeps its id,
    /// locator/role, and verdict facts; sync's reverification re-anchors
    /// evidence that moved intact and stales what genuinely changed.
    Retarget {
        edge_id: String,
        /// Successor node (name, path, id, or fragment) — typically the
        /// renamed/split-to codefile.
        #[arg(long)]
        to: String,
        #[arg(long)]
        reason: String,
    },
    /// Declare that a local intent depends on an upstream (federated) intent.
    DependsOn {
        /// Local intent (name, id, or fragment).
        intent: String,
        /// Upstream shadow (name like upstream/<alias>/..., id, or fragment).
        upstream: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ChallengeCmd {
    /// Record one adversarial attempt against a pending high-risk edge claim.
    Record {
        edge: String,
        /// survived | counterexample | inconclusive
        outcome: String,
        /// The concrete condition or attack expected to falsify the claim.
        #[arg(long)]
        hypothesis: String,
        /// What was attempted and observed, including file:line or journal:id.
        #[arg(long)]
        evidence: String,
        /// Consequence if the counterexample holds; required for counterexample.
        #[arg(long)]
        impact: Option<String>,
        #[arg(long, default_value_t = 0.8)]
        confidence: f64,
    },
    /// Show the current challenge for one edge.
    Show { edge: String },
    /// List current and historical challenge facts.
    List {
        #[arg(long)]
        state: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
}
