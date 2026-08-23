//! Shared test scaffolding.
//!
//! Plane: test support. Nothing here is compiled into a shipped binary.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Process-wide counter so two roots minted inside the same nanosecond differ.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique temporary directory that removes itself on drop.
///
/// Named `<prefix>-<pid>-<nanos>-<seq>`: `cargo test` runs tests as threads in
/// ONE process, so pid alone does not separate them and the clock alone can
/// collide. Dropping removes the tree — a test that panics still cleans up,
/// because `Drop` runs during the unwind.
pub(crate) struct TmpRoot(PathBuf);

impl TmpRoot {
    pub(crate) fn new(prefix: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before the unix epoch")
            .as_nanos();
        let seq = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{}-{nanos}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create temp root");
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// Write `text` to `rel` under this root, creating parent directories.
    pub(crate) fn write(&self, rel: &str, text: &str) {
        let path = self.0.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent directory");
        }
        std::fs::write(path, text).expect("write test file");
    }
}

impl Drop for TmpRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
