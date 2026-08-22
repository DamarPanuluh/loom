//! Ring 65 — the ring order points one way.
//!
//! `src/lib.rs` documents the architecture as rings, with `commands` and `cli`
//! at the top. A doc comment cannot defend that: `src/proof.rs` imported
//! `crate::commands::truncate` for an eight-line string helper, and nothing
//! noticed, because an inversion compiles exactly as well as a dependency.
//!
//! This test is structural. Like `ring23_chokepoint`, it reads loom's own
//! source, because an invariant about WHICH RING may depend on which cannot be
//! observed at runtime — only by looking.
//!
//! The rule: the driver plane may reach down into the rest of the crate, and
//! nothing outside the driver plane may reach up into it. The exceptions are
//! named below with the reason each one is not an inversion.

/// The top ring: the argv grammar, the dispatcher and its handlers, plus the
/// MCP surface, which is a sibling driver over the same handlers rather than a
/// lower ring.
const DRIVER_PLANE: &[&str] = &[
    "src/main.rs",
    "src/cli.rs",
    "src/cli/",
    "src/commands.rs",
    "src/commands/",
    "src/mcp.rs",
];

/// Reaching up into the driver plane from below.
const UPWARD: &[&str] = &["crate::commands::", "crate::commands{", "crate::cli::"];

/// Modules outside the driver plane that may name `crate::cli`, and why.
///
/// Both entries need the REAL argv grammar to answer "which command is this?"
/// about a string they did not write. A private copy of the grammar would be
/// the actual architectural fault: it would drift from the surface it claims
/// to describe, and drift silently. Depending on the one grammar is the
/// correct direction for that question.
///
/// This allowlist covers `crate::cli` only. Nothing outside the driver plane
/// may name `crate::commands` — a handler is behavior, not vocabulary.
const CLI_GRAMMAR_READERS: &[(&str, &str)] = &[
    (
        "src/candidate_surface_policy.rs",
        "Polices a candidate-authored Journey surface: it must reject argv the \
         real CLI would accept differently, so it parses against `cli::Cli`.",
    ),
    (
        "src/proofstrength/command.rs",
        "Resolves a proof's recorded shell command to the handler it exercises, \
         which requires parsing that command with the grammar it was written \
         against.",
    ),
];

fn source_files() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("src is readable").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(std::env::current_dir().unwrap())
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read_to_string(&path).expect("readable")));
            }
        }
    }
    let mut out = Vec::new();
    walk(std::path::Path::new("src"), &mut out);
    assert!(out.len() > 30, "the source scan must actually find loom");
    out
}

fn in_driver_plane(path: &str) -> bool {
    DRIVER_PLANE
        .iter()
        .any(|p| path == *p || (p.ends_with('/') && path.starts_with(p)))
}

#[test]
fn nothing_below_the_driver_plane_reaches_up_into_it() {
    let mut violations: Vec<String> = Vec::new();
    for (path, body) in source_files() {
        if in_driver_plane(&path) {
            continue;
        }
        let allowed_cli_reader = CLI_GRAMMAR_READERS.iter().any(|(p, _)| *p == path);
        for marker in UPWARD {
            if !body.contains(marker) {
                continue;
            }
            if allowed_cli_reader && marker.starts_with("crate::cli") {
                continue;
            }
            violations.push(format!("{path} names `{marker}`"));
        }
    }
    assert!(
        violations.is_empty(),
        "the ring order in src/lib.rs points down; these point up:\n  {}\n\n\
         Move the shared piece to a lower ring (as `src/text.rs` holds the \
         display truncation `commands` used to own), or, if the module \
         genuinely needs the argv grammar, add it to CLI_GRAMMAR_READERS with \
         the reason.",
        violations.join("\n  ")
    );
}

/// An allowlist that outlives its reason is worse than no allowlist: it grants
/// a permanent exemption for a dependency nobody has to justify again. Every
/// entry must still be a file that still needs it.
#[test]
fn every_allowlisted_grammar_reader_still_exists_and_still_reads_the_grammar() {
    let files = source_files();
    for (path, reason) in CLI_GRAMMAR_READERS {
        assert!(
            !reason.trim().is_empty(),
            "{path} is allowlisted with no reason"
        );
        let Some((_, body)) = files.iter().find(|(p, _)| p == path) else {
            panic!("allowlisted {path} no longer exists — drop the entry");
        };
        assert!(
            body.contains("crate::cli"),
            "allowlisted {path} no longer names crate::cli — drop the entry"
        );
    }
}

/// The driver plane is defined by path, so a renamed or newly split driver
/// module would silently leave it and start failing for a dependency that is
/// legal. Assert the plane still matches real files.
#[test]
fn the_declared_driver_plane_matches_real_paths() {
    for entry in DRIVER_PLANE {
        let path = std::path::Path::new(entry.trim_end_matches('/'));
        assert!(
            path.exists(),
            "DRIVER_PLANE names {entry}, which does not exist — update the plane"
        );
    }
}
