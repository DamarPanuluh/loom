use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum CheckpointCmd {
    /// Inspect an exact Intent or cohesive bundle without staging or committing.
    Recommend {
        /// Intent id, name, or unique fragment; repeat for a cohesive bundle.
        #[arg(long = "intent", required = true)]
        intents: Vec<String>,
    },
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleasePhaseArg {
    /// Verify the candidate in one detached fresh-v12 workspace.
    IsolatedDogfood,
    /// Repeat the verification from independent empty workspaces and compare
    /// the semantic attestation, never build-directory bytes.
    FreshFixpoint,
    /// Run both gates and stop before every release, install, commit, or push mutation.
    GatedPreparation,
}

#[derive(Subcommand, Debug)]
pub enum ReleaseCmd {
    /// Seal one exact, current derivation batch after a human approves it.
    AuthorizeDerivations {
        /// Directory containing the reviewed loom.journey-derivation/v1 manifests.
        #[arg(long)]
        manifest_dir: PathBuf,
        /// Exact answer supplied by the human through the host conversation.
        #[arg(long)]
        human_decision: String,
    },
    /// Rehearse release gates in detached candidates without changing the caller.
    Rehearse {
        #[arg(long, value_enum)]
        phase: ReleasePhaseArg,
    },
    /// Internal typed source snapshot used by dogfood/fixpoint bootstrap.
    #[command(hide = true)]
    Snapshot {
        #[arg(long)]
        destination: PathBuf,
    },
}
