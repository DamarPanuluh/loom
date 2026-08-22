use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum PatternCmd {
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        rationale: String,
        #[arg(long)]
        when_to_use: String,
        #[arg(long)]
        when_not_to_use: String,
        #[arg(long = "path")]
        paths: Vec<String>,
        #[arg(long = "intent-tag")]
        intent_tags: Vec<String>,
    },
    Update {
        key: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        rationale: Option<String>,
        #[arg(long)]
        when_to_use: Option<String>,
        #[arg(long)]
        when_not_to_use: Option<String>,
        #[arg(long = "path")]
        paths: Vec<String>,
        #[arg(long = "intent-tag")]
        intent_tags: Vec<String>,
        /// Intentionally remove every path selector.
        #[arg(long, conflicts_with = "paths")]
        clear_paths: bool,
        /// Intentionally remove every intent-tag selector.
        #[arg(long, conflicts_with = "intent_tags")]
        clear_intent_tags: bool,
        #[arg(long)]
        reason: String,
    },
    Show {
        key: String,
    },
    List,
    Lookup {
        #[arg(long = "path")]
        paths: Vec<String>,
        #[arg(long = "intent-tag")]
        intent_tags: Vec<String>,
        /// Skip this many matches in deterministic guidance order.
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    Ratify {
        key: String,
        #[arg(long)]
        evidence: String,
        /// Exact answer the human gave in the host conversation. Lets an LLM
        /// record the decision without acquiring authority to make it.
        #[arg(long)]
        human_decision: Option<String>,
    },
    Retire {
        key: String,
        #[arg(long)]
        reason: String,
    },
    Remove {
        key: String,
        #[arg(long)]
        reason: String,
    },
    Exemplar {
        #[command(subcommand)]
        cmd: PatternExemplarCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum PatternExemplarCmd {
    Add {
        pattern: String,
        codefile: String,
        #[arg(long)]
        locator: String,
    },
    Verdict {
        edge: String,
        verdict: String,
        #[arg(long)]
        criterion: String,
        #[arg(long)]
        evidence: String,
        #[arg(long, default_value_t = 0.9)]
        confidence: f64,
    },
    Remove {
        edge: String,
        #[arg(long)]
        reason: String,
    },
}
