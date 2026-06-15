//! Graph metadata types shared by SQLite storage and analysis.

use serde::Serialize;

/// Version + freshness + identity + custody of the graph, read from the
/// LoomMeta sentinel.
#[derive(Debug, Clone, Serialize)]
pub struct GraphMeta {
    pub version: String,
    pub created_at: String,
    /// RFC3339 of the last `loom sync`, or "" if never synced.
    pub last_synced: String,
    /// Stable identity (uuid) other looms reference; "" on pre-identity graphs
    /// until `loom init` backfills it.
    pub graph_id: String,
    /// Human name (defaults to the repo directory name at init).
    pub graph_name: String,
    /// "owned" | "observed" ("" on pre-identity graphs = owned).
    pub custody: String,
}

impl GraphMeta {
    /// True when this graph maps code its drivers do not own. Build/fix lanes
    /// are disabled for observed graphs.
    pub fn observed(&self) -> bool {
        self.custody == "observed"
    }
}

/// The default per-target routine-transition ceiling when the graph has not
/// set one explicitly. `0` disables compaction.
pub const DEFAULT_TRANSITION_CAP: usize = 20;
