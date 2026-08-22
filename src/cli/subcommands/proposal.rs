use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum ProposalCmd {
    /// Capture a structured proposal from text or a file.
    Add {
        #[arg(long)]
        title: String,
        /// Read the proposal body from a file (use --file or --text, not both).
        #[arg(long)]
        file: Option<PathBuf>,
        /// Inline proposal body text (use --file or --text, not both).
        #[arg(long)]
        text: Option<String>,
    },
    /// List captured proposals.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Show a proposal by id, name, or unique fragment.
    Show { key: String },
    /// Delete a mistaken proposal capture, including its embedded items. Adopted
    /// spawned work remains separate graph history.
    Remove { key: String },
    /// Proposal item subcommands.
    Item {
        #[command(subcommand)]
        cmd: ProposalItemCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProposalItemCmd {
    /// Add a numbered item to a proposal.
    Add {
        proposal: String,
        #[arg(long)]
        text: String,
        /// Optional kind label (e.g. intent, task, observation).
        #[arg(long)]
        kind: Option<String>,
    },
    /// Adopt an item, optionally spawning an intent or task.
    Adopt {
        proposal: String,
        number: usize,
        /// Spawn a planned Intent or a proposed TaskRecord.
        #[arg(long)]
        r#as: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// Defer an item with a reason.
    Defer {
        proposal: String,
        number: usize,
        #[arg(long)]
        reason: String,
    },
    /// Reject an item with a reason.
    Reject {
        proposal: String,
        number: usize,
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum JudgmentCmd {
    /// Stage a proposed ratify/reject/redefine for an intent, with the
    /// evidence a human will review. Staging is not gated — recommending is
    /// not deciding. One live proposal per (kind, intent).
    Propose {
        /// ratify | reject | redefine
        kind: String,
        /// The intent the proposal judges (name, id, or fragment).
        intent: String,
        /// Why the judgment holds: reject reason, ratify evidence, or
        /// redefine rationale. Substantive — this is what the human reviews.
        #[arg(long)]
        evidence: String,
        /// The replacement statement. Required for redefine; ignored
        /// otherwise.
        #[arg(long)]
        description: Option<String>,
    },
    /// The human's review surface: every staged proposal, oldest first,
    /// with the exact confirm command for each.
    Digest {
        /// Also show decided (confirmed/withdrawn) proposals.
        #[arg(long)]
        all: bool,
    },
    /// Execute a staged proposal through the SAME human gate the direct
    /// command demands: ratify/reject require the human's answer (mediated
    /// via --human-decision, or the typed challenge at a solo terminal);
    /// redefine applies the staged statement with its ripple.
    Confirm {
        key: String,
        /// Exact answer the human gave in the host conversation. Required
        /// for ratify/reject when the executor is an llm:* agent.
        #[arg(long)]
        human_decision: Option<String>,
    },
    /// Drop a staged proposal (the candidate was wrong, or the intent
    /// changed since staging). Requires a substantive reason.
    Withdraw {
        key: String,
        #[arg(long)]
        reason: String,
    },
}
