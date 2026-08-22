use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum JourneyCmd {
    /// Lint authored Journey surface manifests for portable, durable proofs.
    Lint { journey: Option<String> },
    /// Register an authored semantic Journey root from a JSON or YAML artifact.
    Add { spec: PathBuf },
    /// Show one Journey by stable id, node id, or unique fragment.
    Show { journey: String },
    /// Remove an authored Journey and its derived projections.
    Remove { journey: String },
    /// List authored Journey roots.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Show the Journey-root map and every unrooted, non-exempt Intent.
    Map,
    /// Emit a read-only packet for deriving technical Intents from a Journey.
    Derive {
        journey: String,
        /// Inspect one strict candidate manifest without accepting it. The value
        /// may be inline JSON or the path to a JSON manifest.
        #[arg(long, value_name = "MANIFEST_OR_JSON")]
        candidate_json: Option<String>,
    },
    /// Accept one exact, human-authorized technical derivation manifest.
    DeriveAccept {
        journey: String,
        #[arg(long)]
        manifest: PathBuf,
        /// Exact answer authorizing this hash-bound manifest.
        #[arg(long)]
        human_decision: String,
    },
    /// Emit a read-only contract for a real CLI projection in the target repo.
    Surface { journey: String },
    /// Accept one hash-bound reusable CLI surface manifest.
    SurfaceAccept {
        journey: String,
        #[arg(long)]
        manifest: PathBuf,
    },
    /// Compile the Journey's surfaced CLI into a runnable proof profile.
    Compile {
        journey: String,
        #[arg(long, default_value = "proof")]
        profile: String,
    },
    /// Run a compiled Journey profile and record what Loom observes.
    Run {
        journey: String,
        #[arg(long, default_value = "proof")]
        profile: String,
    },
    /// Resume one pending host-mediated Journey step with the human's exact answer.
    Resume {
        /// Opaque token returned by the pending Journey run.
        token: String,
        /// Exact stable id of one option presented by the pending gate.
        #[arg(long)]
        choice: String,
        /// Exact answer supplied by the human through the host conversation.
        #[arg(long)]
        human_decision: String,
        /// Substantive revision required only by an option marked free-form.
        #[arg(long)]
        free_form: Option<String>,
    },
    /// Diagnose a Journey with optional input overrides, without settling proof.
    Diagnose {
        journey: String,
        #[arg(long, default_value = "proof")]
        profile: String,
        /// Override one authored input as KEY=JSON (repeatable).
        #[arg(long = "input", value_name = "KEY=JSON")]
        input: Vec<String>,
    },
    /// Run one proof in a detached, freshly imported release candidate.
    ///
    /// Not a cheap target-repository pre-flight: cold rehearsal currently
    /// assumes loom's own layout (`journeys/surfaces/` manifests and reserved
    /// inventory components). In a target repo use `loom journey lint` and
    /// `loom journey diagnose` instead.
    RehearseCold { journey: String },
    /// Freeze the current observed result as the profile baseline.
    Freeze {
        journey: String,
        #[arg(long, default_value = "proof")]
        profile: String,
    },
    /// Report stale compiled Journey artifacts, optionally for one Journey.
    Drift { journey: Option<String> },
}

#[derive(Subcommand, Debug)]
pub enum DriveCmd {
    /// Compile recorded drive exchanges into a local journey YAML file.
    Freeze { name: String },
}

#[derive(Subcommand, Debug)]
pub enum BootstrapCmd {
    /// Draft a Proposal of behavior clues from derived signals (registered
    /// codefiles, tests/, README H2s) to inform authored Journey roots.
    /// Never writes product meaning, Intents, edges, or verdicts.
    Suggest,
}
