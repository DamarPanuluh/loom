use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum FindingCmd {
    /// Add an asserted evidence-backed finding for triage.
    Add {
        text: String,
        #[arg(long, default_value = "code_audit")]
        source: String,
        #[arg(long, default_value = "code_audit")]
        kind: String,
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        impact: String,
        #[arg(long, default_value_t = 0.7)]
        confidence: f64,
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        link: Option<String>,
    },
    /// List findings with their adjudication state.
    List {
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        state: Option<String>,
    },
    /// Record a judgment on a finding.
    Verdict {
        id: String,
        verdict: String,
        #[arg(long)]
        reason: String,
        /// Where you looked: `file:line` in the flagged code, or a `journal:`
        /// ref. A settling verdict needs one — the reason says WHAT you decided,
        /// this says what you decided it FROM. Open states (`needed`,
        /// `blocked`) do not.
        #[arg(long, default_value = "")]
        evidence: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum IgnoreCmd {
    /// Exclude a glob from coverage with a recorded reason.
    Add {
        glob: String,
        #[arg(long)]
        reason: String,
    },
    /// Remove a coverage-ignore rule by its glob.
    Remove { glob: String },
    /// List ignore rules.
    List,
}

/// Output format for a scan adapter (`--format`).
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ScanFormatArg {
    /// Line-oriented text parsed by a regex map (GCC-style default).
    Lines,
    /// A JSON array/JSONL of finding objects; `--map` renames looked-up
    /// fields (`items=…,file=…,line=…,msg=…,code=…`, dotted paths allowed).
    Json,
}

impl From<ScanFormatArg> for crate::scan::ScanFormat {
    fn from(arg: ScanFormatArg) -> Self {
        match arg {
            ScanFormatArg::Lines => Self::Lines,
            ScanFormatArg::Json => Self::Json,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum ScanCmd {
    /// Register an external diagnostic tool (any language's linter/checker).
    Add {
        /// Adapter name (e.g. clippy, eslint, ruff).
        name: String,
        /// The command to run, e.g. "cargo clippy --message-format=short".
        command: String,
        /// Parser map. `lines`: regex with named groups `file` and `line`
        /// (optional `msg`, `code`); default GCC-style `file:line[:col]:
        /// message`. `json`: comma-separated `field=path` lookups
        /// (`items|file|line|msg|code`, dotted paths allowed).
        #[arg(long)]
        map: Option<String>,
        /// Output format (default: lines).
        #[arg(long, value_enum, default_value_t = ScanFormatArg::Lines)]
        format: ScanFormatArg,
    },
    /// Edit a registered adapter in place. Use this when the command or parser
    /// map changed; run scan afterwards to refresh derived findings.
    Update {
        name: String,
        #[arg(long)]
        command: Option<String>,
        #[arg(long)]
        map: Option<String>,
        /// Switch the output format (lines | json).
        #[arg(long, value_enum)]
        format: Option<ScanFormatArg>,
    },
    /// List registered adapters.
    List,
    /// Remove an adapter.
    Remove { name: String },
    /// Run one adapter (or all) and convert diagnostics into derived findings.
    Run { name: Option<String> },
}

#[derive(Subcommand, Debug)]
pub enum ThresholdCmd {
    /// Show the current gates (configured values, or shipped defaults if unset).
    List,
    /// Hand-set one gate (e.g. `max_args 8`); persists to config.thresholds.
    Set {
        /// Gate name: max_file_loc | max_symbol_complexity | max_symbol_loc |
        /// max_nesting | max_args.
        gate: String,
        /// The new threshold (strict `>` bound; must be >= 1).
        value: u64,
    },
    /// Reset one gate to its shipped default, or all gates when omitted.
    Reset {
        /// Gate name; omit to reset every gate.
        gate: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum PolicyCmd {
    /// Show the current policy (configured values, or shipped defaults if unset).
    Show,
    /// Set the review-confidence floor (a fraction in [0.0, 1.0]); persists to
    /// config.evidence_policy.
    SetFloor {
        /// The new floor: verdicts strictly below it route to review.
        value: f64,
    },
    /// Set the bounded adversarial-review frontier. Zero disables it; the
    /// shipped default is five and the maximum is one hundred.
    SetAdversarialFrontier { value: usize },
    /// Add an owner lane to the human-gated set (builder | analyzer | fixer |
    /// validator | quality).
    GateAdd {
        /// The lane whose work packets should carry a human gate.
        role: String,
    },
    /// Remove an owner lane from the human-gated set.
    GateRemove {
        /// The lane to stop gating.
        role: String,
    },
    /// Reset the whole policy to the shipped defaults (drops the config).
    Reset,
}
