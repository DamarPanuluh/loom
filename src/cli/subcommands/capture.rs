use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum InboxCmd {
    /// Capture raw input.
    Add {
        text: String,
        #[arg(long, default_value = "human")]
        source: String,
        /// Optional origin ref, e.g. file:src/auth.rs or a node id.
        #[arg(long)]
        link: Option<String>,
    },
    /// List inbox items.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Filter by disposition (new|routed|rejected|duplicate|deferred).
        #[arg(long)]
        status: Option<String>,
    },
    /// Show one inbox item in full.
    Show { key: String },
    /// Mark an item's disposition: routed | rejected | duplicate | deferred.
    Mark {
        key: String,
        status: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Remove an inbox item (e.g. a resolved or accidental capture).
    Remove { key: String },
}

#[derive(Subcommand, Debug)]
pub enum QuestionCmd {
    /// Open a product question for exactly one technical Intent or authored Journey.
    Add {
        text: String,
        #[arg(long, required_unless_present = "journey", conflicts_with = "journey")]
        intent: Option<String>,
        #[arg(long, required_unless_present = "intent", conflicts_with = "intent")]
        journey: Option<String>,
    },
    /// List product questions.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long)]
        status: Option<String>,
    },
    /// Show one product question.
    Show { key: String },
    /// Answer a product question.
    Answer {
        key: String,
        #[arg(long)]
        answer: String,
    },
    /// Close a product question without an answer.
    Close {
        key: String,
        status: String,
        #[arg(long)]
        reason: String,
    },
    /// Remove an accidental product question.
    Remove { key: String },
}

#[derive(Subcommand, Debug)]
pub enum NoteCmd {
    /// Attach a durable note to any node or edge (adjudications, context, warnings).
    Add {
        /// The node (name, id, or unique fragment) or edge (id or prefix) the note is about.
        target: String,
        /// decision | context | warning
        #[arg(long, default_value = "decision")]
        kind: String,
        #[arg(long)]
        text: String,
    },
    /// Remove a mistaken note. Notes are history and have no edit operation;
    /// removal is only for accidental/misattached notes.
    Remove { id: String },
    /// List notes, newest first, optionally scoped to one target node or edge.
    List {
        target: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
}

#[derive(Subcommand, Debug)]
pub enum TaskCmd {
    /// Open a task record (spike|investigation|experiment|review|chore|research).
    Add {
        title: String,
        #[arg(long, default_value = "spike")]
        kind: String,
        /// Intent this task informs — the close/abandon outcome lands as a note on it.
        #[arg(long)]
        target: Option<String>,
        /// Why current/external knowledge is required (required for research).
        #[arg(long)]
        why_external: Option<String>,
        /// Preferred authoritative source guidance (repeatable; research only).
        #[arg(long = "preferred-source")]
        preferred_sources: Vec<String>,
    },
    /// Append one actual page read to a research task's provenance.
    SourceAdd {
        task: String,
        #[arg(long)]
        url: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        publisher: String,
        #[arg(long)]
        source_kind: String,
        #[arg(long)]
        quote: String,
        #[arg(long)]
        published_at: Option<String>,
        #[arg(long)]
        fresh_until: Option<String>,
    },
    /// Mark a task active.
    Start { key: String },
    /// Close a task with a result summary.
    Close {
        key: String,
        #[arg(long)]
        result: String,
    },
    /// Abandon a task.
    Abandon {
        key: String,
        #[arg(long)]
        reason: String,
    },
    /// Delete an accidental task record. Use close/abandon for real work history.
    Remove { key: String },
    /// Show a task record (kind/status/result).
    Show { key: String },
    /// List task records.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
}
