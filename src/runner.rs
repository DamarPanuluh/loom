//! Runner — the ONLY place a [`RunRecord`] comes into existence.
//!
//! Plane: observation. Everything here executes something and reports what it
//! saw; nothing here accepts an outcome from a caller.
//!
//! Contract: `Run` evidence means *loom did this and watched*. That is the whole
//! difference between "54 of 59 proofs passed" and "an agent wrote a sentence
//! saying 54 of 59 proofs passed". Because [`crate::evidence::CitedEvidence`]
//! has no `Run` variant, there is no path from caller input to this type — the
//! guarantee is structural, not procedural.
//!
//! Every run captures its `covered` set: the file → content-hash map in force
//! when it ran. That is the anchor. When any covered file changes, the run
//! expires and whatever it justified re-opens — a proof stops counting the
//! moment the code beneath it moves.

use crate::evidence::RunRecord;
use crate::model::RunProducer;
use crate::store::Store;
use crate::Result;
use std::path::Path;
use std::time::{Duration, Instant};

/// Bytes of each stream kept for humans. The captured stream is already bounded
/// to a head+tail window by `subprocess::run` before it reaches here, and the
/// fingerprint is taken over that bounded text.
const EXCERPT_BYTES: usize = 8192;

/// Default wall-clock limit for an observed command.
pub const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// What a run observed, before it is bound to a fact.
pub enum Observation {
    Ran(Box<RunRecord>),
    /// Could not run — a missing command, an untrusted import, a timeout. This
    /// is an honest outcome and is recorded as such; it is never a pass.
    Blocked {
        reason: String,
    },
}

/// Execute `command` at the graph root and record what happened.
///
/// `covered` names the files this run's result depends on; their current hashes
/// are captured so the run expires when any of them changes. `assertions` is how
/// many content checks the run actually made — zero means it only proved the
/// process exited, which is liveness, not behavior.
pub fn observe_command(
    root: &Path,
    producer: RunProducer,
    command: &str,
    covered: &[String],
    assertions: usize,
    timeout_secs: u64,
) -> Result<Observation> {
    if command.trim().is_empty() {
        return Ok(Observation::Blocked {
            reason: "no command to run — a manual check must be attested, not inferred".into(),
        });
    }
    let started = Instant::now();
    let captured = match crate::subprocess::run(command, root, Duration::from_secs(timeout_secs)) {
        Ok(Some(c)) => c,
        // A timeout is not a failure of the behavior — it is a failure to
        // observe, and loom refuses to guess which.
        Ok(None) => {
            return Ok(Observation::Blocked {
                reason: format!("`{command}` timed out after {timeout_secs}s"),
            })
        }
        Err(e) => {
            return Ok(Observation::Blocked {
                reason: format!("could not start `{command}`: {e}"),
            })
        }
    };
    // A command that failed because loom's OWN infrastructure got in the way is
    // not a failing behavior. A child that needed the graph while its parent
    // held the lock aborts with a reserved exit code (not a scraped stderr
    // string, which a failing test could print verbatim), so this is exact:
    // that ambiguity once let `observe` record a false failing verdict against a
    // passing journey. Refuse to attribute it to the code.
    if captured.status.code() == Some(i64::from(crate::store::LOCK_CONTENTION_EXIT_CODE)) {
        return Ok(Observation::Blocked {
            reason: format!(
                "`{command}` could not be observed: it needed the graph while loom held it. \
                 This is loom's own infrastructure failing, not the behavior."
            ),
        });
    }
    Ok(Observation::Ran(Box::new(record(
        root,
        producer,
        command,
        covered,
        assertions,
        captured.status.code().unwrap_or(-1),
        &captured.stdout,
        &captured.stderr,
        started.elapsed().as_millis() as u64,
    ))))
}

/// Build a record from an already-observed result. Used by probes that do their
/// own work in-process (locator resolution, pattern pre-screens) rather than
/// spawning — the point is the same: loom looked, loom reports.
#[allow(clippy::too_many_arguments)]
pub fn record(
    root: &Path,
    producer: RunProducer,
    command: &str,
    covered: &[String],
    assertions: usize,
    exit_code: i64,
    stdout: &[u8],
    stderr: &[u8],
    duration_ms: u64,
) -> RunRecord {
    RunRecord {
        producer,
        command: command.to_string(),
        cwd: String::new(),
        exit_code,
        stdout_hash: crate::artifact::fingerprint(&String::from_utf8_lossy(stdout)),
        stderr_hash: crate::artifact::fingerprint(&String::from_utf8_lossy(stderr)),
        stdout_excerpt: excerpt(stdout),
        stderr_excerpt: excerpt(stderr),
        covered: covered_hashes(root, covered),
        assertions,
        duration_ms,
        ran_at: crate::journal::now_iso(),
        loom_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Hash every covered file as it stands right now. A file that cannot be read
/// is recorded with an empty hash so its later appearance still counts as a
/// change — absence is a state, not a gap.
fn covered_hashes(root: &Path, files: &[String]) -> std::collections::BTreeMap<String, String> {
    files
        .iter()
        .map(|f| {
            let hash = std::fs::read_to_string(root.join(f))
                .map(|c| crate::artifact::fingerprint(&c))
                .unwrap_or_default();
            (f.clone(), hash)
        })
        .collect()
}

/// Whether every file a run covered still hashes to what it did at run time.
///
/// An empty `covered` map is NOT drift: a Command/Journey proof with no file
/// anchor (no coverage attached) is re-verified by re-running it in its lane,
/// exactly as Seam/Locator runs are re-resolved by their own recheck arms.
/// Hashing "no files" is vacuously intact, so this reports `None` (holds) and
/// leaves the run to be re-earned rather than falsely re-opening it.
pub fn covered_intact(root: &Path, run: &RunRecord) -> Option<String> {
    for (file, hash) in &run.covered {
        let current = std::fs::read_to_string(root.join(file))
            .map(|c| crate::artifact::fingerprint(&c))
            .unwrap_or_default();
        if &current != hash {
            return Some(file.clone());
        }
    }
    None
}

/// A bounded excerpt of captured output that keeps BOTH ends.
///
/// A test runner prints its verdict LAST — "test result: ok. 12 passed; 0
/// failed", "==== 12 passed in 0.4s ====". A head-only excerpt dropped exactly
/// the line proof grading reads to credit a real suite (`reported_assertions`),
/// so a passing 100-test run over 8 KB of output graded as liveness-only. The
/// tail is as load-bearing as the head, so when output exceeds the budget keep
/// the first and last halves with a marker between; the full text is still
/// hashed, so integrity is unaffected.
fn excerpt(bytes: &[u8]) -> String {
    if bytes.len() <= EXCERPT_BYTES {
        return String::from_utf8_lossy(bytes).to_string();
    }
    let half = EXCERPT_BYTES / 2;
    let head = String::from_utf8_lossy(&bytes[..half]);
    let tail = String::from_utf8_lossy(&bytes[bytes.len() - half..]);
    let omitted = bytes.len() - 2 * half;
    format!("{head}\n…[{omitted} bytes omitted]…\n{tail}")
}

/// Re-resolve a grounding's locator against the live symbol table.
///
/// This is loom checking a claim for itself rather than believing prose about
/// it: "the behavior lives at `fn capture_payment` in src/pay.rs" is either
/// true of the file on disk right now or it is not, and loom can look. The
/// result is a Run because it IS one — an observation loom made, expiring when
/// the file changes.
pub fn locator_probe(root: &Path, file: &str, locator: Option<&str>) -> Option<RunRecord> {
    let content = std::fs::read_to_string(root.join(file)).ok()?;
    let locator = locator.map(str::trim).filter(|l| !l.is_empty())?;
    let extraction = crate::extract::extract(file, &content);
    // The same candidate expansion the ripple uses: verbatim, the last
    // whitespace token with any `:line` suffix stripped, then its final `::`
    // segment. `fn capture_payment`, `capture_payment:88`, `Store::open`.
    let mut candidates: Vec<String> = vec![locator.to_string()];
    if let Some(tok) = locator.split_whitespace().next_back() {
        let tok = tok.split(':').next().unwrap_or(tok);
        candidates.push(tok.to_string());
        if let Some(seg) = tok.rsplit("::").next() {
            candidates.push(seg.to_string());
        }
    }
    // ALL symbols carrying the name, not the first. Two functions called
    // `helper` in one file are not distinguishable by a locator, so the honest
    // anchor covers both: either one being rewritten re-opens the claim,
    // because loom cannot tell which one the grounding meant.
    let hits: Vec<&crate::extract::Symbol> = extraction
        .symbols
        .iter()
        .filter(|sym| candidates.iter().any(|c| c == &sym.name))
        .collect();
    let hit = hits.first().copied();
    let (exit_code, detail) = match hit {
        // The fingerprint of the symbol's BODY, not just its position. A
        // grounding says the behavior lives in this symbol, so the symbol being
        // rewritten falsifies it exactly as much as the symbol disappearing —
        // and re-checking presence alone would spare every edit that stayed
        // inside a function, which is most of them.
        Some(sym) => {
            let lines: Vec<&str> = content.lines().collect();
            let folded: String = hits
                .iter()
                .map(|s| {
                    lines
                        .get(s.line_start.saturating_sub(1)..s.line_end.min(lines.len()))
                        .map(|w| w.join("\n"))
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join("\n---\n");
            (
                0,
                format!(
                    "{} '{}' at {}:{} ({} match{}) [{}]",
                    sym.kind,
                    sym.name,
                    file,
                    sym.line_start,
                    hits.len(),
                    if hits.len() == 1 { "" } else { "es" },
                    crate::artifact::fingerprint(&folded)
                ),
            )
        }
        None => (1, format!("no live symbol matching '{locator}' in {file}")),
    };
    Some(record(
        root,
        RunProducer::Locator,
        &format!("resolve '{locator}' in {file}"),
        std::slice::from_ref(&file.to_string()),
        1,
        exit_code,
        detail.as_bytes(),
        &[],
        0,
    ))
}

/// Scan a quality rule's own patterns over the files realizing an intent, and
/// record what loom found — including finding nothing.
///
/// This is the answer to absence-shaped rules, which are otherwise unanchorable.
/// "No hardcoded secrets in the code realizing X" cannot cite a span: there is
/// nothing to point at. But it CAN be a run — *loom scanned these patterns over
/// these files at these hashes and found zero hits* — and that is re-checkable,
/// expiring the moment any covered file changes.
///
/// The machinery already existed and threw its answer away: `prescreen_for`
/// computes exactly this to populate a quality packet, then discards it.
pub fn prescreen_probe(
    root: &Path,
    rule_name: &str,
    patterns: &[String],
    files: &[String],
) -> Option<RunRecord> {
    if patterns.is_empty() || files.is_empty() {
        return None;
    }
    let hits = crate::prescan::prescreen(root, files, patterns, 200).ok()?;
    // Canonical, sorted rendering so re-scanning identical files is a
    // byte-identical no-op rather than a fresh fact.
    let mut lines: Vec<String> = hits
        .iter()
        .map(|h| format!("{}:{} {}", h.path, h.line, h.pattern))
        .collect();
    lines.sort();
    let detail = lines.join("\n");
    Some(record(
        root,
        RunProducer::Prescreen,
        &format!(
            "scan '{rule_name}' ({} pattern(s)) over {} realizing file(s)",
            patterns.len(),
            files.len()
        ),
        files,
        patterns.len(),
        i64::from(!hits.is_empty()),
        detail.as_bytes(),
        &[],
        0,
    ))
}

/// The files a run over this intent's code depends on: every file grounded to
/// it. Used to build the `covered` set for a proof.
/// Anchor a seam grounding: is the seam this file USES still in it?
pub fn seam_probe(root: &Path, file: &str, locator: Option<&str>) -> Option<RunRecord> {
    let content = std::fs::read_to_string(root.join(file)).ok()?;
    let locator = locator.map(str::trim).filter(|l| !l.is_empty())?;
    let present = seam_present(&content, locator);
    Some(record(
        root,
        RunProducer::Seam,
        &format!("seam '{locator}' in {file}"),
        // Deliberately NOT covered: a seam claim survives content churn. The
        // probe re-runs instead, which is the whole point of the distinction.
        &[],
        1,
        i64::from(!present),
        if present {
            format!("seam '{locator}' present in {file}")
        } else {
            format!("seam '{locator}' gone from {file}")
        }
        .as_bytes(),
        &[],
        0,
    ))
}

/// Whether a grounding's seam locator still resolves in the file content. The
/// locator names the seam (a route, topic, config key, or symbol), so if it —
/// or its most significant token — is gone, the seam moved.
pub fn seam_present(src: &str, locator: &str) -> bool {
    let loc = locator.trim();
    if loc.is_empty() || src.contains(loc) {
        return true;
    }
    match loc.split_whitespace().next_back() {
        Some(tok) if !tok.is_empty() => src.contains(tok),
        _ => false,
    }
}

pub fn files_grounding(store: &Store, intent_id: &str) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for e in store.edges_with(
        Some(crate::model::EdgeKind::Implements),
        Some(intent_id),
        None,
    )? {
        if store.edge_superseded(&e.id)? {
            continue;
        }
        if let Some(cf) = store.get_node(&e.to_id)? {
            files.push(cf.name);
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_command_is_blocked_not_passed() {
        let tmp = std::env::temp_dir();
        let obs =
            observe_command(&tmp, RunProducer::Command, "   ", &[], 0, 5).expect("no io failure");
        match obs {
            Observation::Blocked { reason } => assert!(reason.contains("manual check")),
            Observation::Ran(_) => panic!("an empty command must never produce a run record"),
        }
    }

    #[test]
    fn a_run_records_the_hashes_of_what_it_covered() {
        let dir = tempdir();
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        let obs = observe_command(&dir, RunProducer::Command, "true", &["a.txt".into()], 1, 30)
            .expect("no io failure");
        let Observation::Ran(run) = obs else {
            panic!("`true` runs")
        };
        assert_eq!(run.exit_code, 0);
        assert_eq!(run.covered.len(), 1);
        assert!(covered_intact(&dir, &run).is_none(), "nothing changed yet");

        // Edit the covered file: the run no longer describes this codebase.
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        assert_eq!(covered_intact(&dir, &run).as_deref(), Some("a.txt"));
    }

    /// A test runner prints its verdict LAST. When output overruns the excerpt
    /// budget, a head-only clip drops exactly that line and a passing suite
    /// grades as liveness-only. The excerpt must keep the tail so proof grading
    /// (`parse_runner_summary`) can still credit the real run.
    #[test]
    fn the_excerpt_keeps_a_trailing_runner_verdict() {
        let mut out = "x\n".repeat(EXCERPT_BYTES).into_bytes();
        assert!(out.len() > EXCERPT_BYTES, "output must overrun the budget");
        let verdict = "test result: ok. 100 passed; 0 failed";
        out.extend_from_slice(verdict.as_bytes());

        let clipped = excerpt(&out);
        assert!(
            clipped.contains(verdict),
            "the trailing verdict survived: {clipped:?}"
        );
        assert!(
            crate::proofstrength::parse_runner_summary(&clipped).is_some(),
            "proof grading can still read the summary"
        );
    }

    fn tempdir() -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "loom-runner-{}-{}",
            std::process::id(),
            crate::journal::now_iso().replace([':', '.', '-'], "")
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }
}
