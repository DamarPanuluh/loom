use super::entries::EntryEvidence;
use clap::Parser;
use std::path::Path;

/// Split one command segment into argv-like words with quote awareness.
///
/// Fail closed on shell syntax we do not model (`$`, backticks, redirects,
/// globs, braces). Whitespace-split with quote-stripping previously turned
/// `cargo test -- --test "foo bar"` into three tokens and credited a filter
/// that never ran.
fn argv_has_help_or_version(words: &[String]) -> bool {
    words
        .iter()
        .any(|word| matches!(word.as_str(), "--help" | "-h" | "--version" | "-V"))
}

fn shell_words(segment: &str) -> Vec<String> {
    shell_words_strict(segment).unwrap_or_default()
}

fn shell_words_strict(segment: &str) -> Option<Vec<String>> {
    // Reject operators and expansions we do not interpret. Callers that need
    // compound commands already fail closed at `command_entries`.
    if segment
        .chars()
        .any(|c| matches!(c, '`' | '$' | '>' | '<' | '*' | '?' | '{' | '}' | '~'))
    {
        return None;
    }
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = segment.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    // True once the current token has seen a quote pair (possibly empty).
    let mut quoted_token = false;
    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                quoted_token = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                quoted_token = true;
            }
            '\\' if in_double => {
                // Only shell-escapable characters consume the backslash inside
                // double quotes. `\_` must remain `\_`, or a filter that never
                // ran can be rewritten into a live symbol name.
                let next = chars.next()?;
                if matches!(next, '"' | '\\' | '`' | '$' | '\n') {
                    current.push(next);
                } else {
                    current.push('\\');
                    current.push(next);
                }
            }
            '\\' if !in_single && !in_double => return None,
            c if c.is_whitespace() && !in_single && !in_double => {
                if !current.is_empty() || quoted_token {
                    words.push(std::mem::take(&mut current));
                    quoted_token = false;
                }
            }
            _ => current.push(c),
        }
    }
    if in_single || in_double {
        return None;
    }
    if !current.is_empty() || quoted_token {
        words.push(current);
    }
    // Fail closed on empty argv elements. `cargo test --test ""` cannot select
    // a real surface; inventing a broader match would be false credit.
    if words.iter().any(|word| word.is_empty()) {
        return None;
    }
    // Drop leading env assignments the same way the old splitter did, so
    // `FOO=1 cargo test` still resolves as `cargo test`.
    let mut command_seen = false;
    let filtered: Vec<String> = words
        .into_iter()
        .filter(|word| {
            if command_seen {
                return true;
            }
            // Shell-valid names only: must start with a letter or underscore.
            // `1=x cargo test` is not an env assignment and must not be stripped.
            let is_env_assignment = word.split_once('=').is_some_and(|(name, _)| {
                let mut chars = name.chars();
                matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
                    && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
            }) && !word.starts_with('-');
            if is_env_assignment {
                false
            } else {
                command_seen = true;
                true
            }
        })
        .collect();
    Some(filtered)
}

fn file_stem(path: &str) -> &str {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
}

/// Entry evidence for a fixed, exact entry symbol (e.g. a binary's `main`).
/// Substring matching would credit `main_helper` for a `main` requirement and
/// manufacture a false witness, so the file must define the exact symbol.
fn exact_entry(
    graph: &crate::callgraph::CallGraph,
    source: &'static str,
    file: &str,
    symbol: &str,
) -> Vec<EntryEvidence> {
    if graph.file_defines(file, symbol) {
        vec![EntryEvidence::plain(
            source,
            file.into(),
            Some(symbol.into()),
            true,
        )]
    } else {
        Vec::new()
    }
}

fn entries_in_file(
    graph: &crate::callgraph::CallGraph,
    source: &'static str,
    file: &str,
    symbol_filter: Option<&str>,
) -> Vec<EntryEvidence> {
    // Only harness-executed test functions are entry candidates: `cargo test`
    // runs the `#[test]` functions (and anything they transitively call),
    // never an uncalled helper sitting in the same file. Emitting every
    // matching symbol would let a dead helper that happens to reach grounded
    // code look like an executed entry.
    let test_symbols = graph.file_test_symbols(file);
    // Cargo's filter selects harness test names, not every helper those tests
    // can reach. Matching a helper name would claim an entry the harness never
    // selected (and may run zero tests for).
    let narrowed: Vec<&str> = match symbol_filter.filter(|filter| !filter.is_empty()) {
        Some(filter) => test_symbols
            .iter()
            .map(String::as_str)
            .filter(|symbol| symbol.contains(filter))
            .collect(),
        None => test_symbols.iter().map(String::as_str).collect(),
    };
    if narrowed.is_empty() {
        return Vec::new();
    }
    narrowed
        .into_iter()
        .map(|symbol| EntryEvidence::plain(source, file.into(), Some(symbol.into()), true))
        .collect()
}

fn cargo_test_entries(
    words: &[String],
    graph: &crate::callgraph::CallGraph,
    source: &'static str,
) -> Vec<EntryEvidence> {
    if words.iter().any(|word| {
        word == "--no-run"
            || word == "--list"
            || word == "--doc"
            || word == "--manifest-path"
            || word.starts_with("--manifest-path=")
            || word == "--target"
            || word.starts_with("--target=")
            || word == "--skip"
            || word.starts_with("--skip=")
            || word == "--exact"
            || word == "--ignored"
            || word == "--include-ignored"
            || word == "-p"
            || word == "--package"
            || word.starts_with("--package=")
            || word == "--lib"
            || word == "--bins"
            || word == "--examples"
            || word == "--benches"
            || word == "--all-targets"
            || word == "--workspace"
            || word == "--all"
            || word == "--exclude"
            || word.starts_with("--exclude=")
            || word == "--bin"
            || word.starts_with("--bin=")
            || word == "--example"
            || word.starts_with("--example=")
            || word == "--bench"
            || word.starts_with("--bench=")
    }) || words
        .windows(2)
        .any(|pair| pair[0] == "--" && pair[1] == "--list")
    {
        return Vec::new();
    }
    let test_name = words
        .iter()
        .find_map(|word| {
            word.strip_prefix("--test=")
                .or_else(|| word.strip_prefix("--bench="))
        })
        .or_else(|| {
            words
                .windows(2)
                .find(|pair| pair[0] == "--test")
                .map(|pair| pair[1].as_str())
        });
    let mut positional = Vec::new();
    let mut index = 2;
    while index < words.len() {
        let word = words[index].as_str();
        if word == "--" {
            positional.extend(words[index + 1..].iter().map(String::as_str));
            break;
        }
        if let Some(value) = word
            .strip_prefix("--package=")
            .or_else(|| word.strip_prefix("--features="))
            .or_else(|| word.strip_prefix("--target="))
            .or_else(|| word.strip_prefix("--target-dir="))
            .or_else(|| word.strip_prefix("--manifest-path="))
            .or_else(|| word.strip_prefix("--profile="))
            .or_else(|| word.strip_prefix("--config="))
            .or_else(|| word.strip_prefix("--test="))
            .or_else(|| word.strip_prefix("--bin="))
            .or_else(|| word.strip_prefix("--example="))
            .or_else(|| word.strip_prefix("--bench="))
        {
            if value.is_empty() {
                return Vec::new();
            }
            index += 1;
            continue;
        }
        if let Some(value) = word.strip_prefix("--color=") {
            if !matches!(value, "auto" | "always" | "never") {
                return Vec::new();
            }
            index += 1;
            continue;
        }
        let takes_value = matches!(
            word,
            "-p" | "--package"
                | "-j"
                | "--jobs"
                | "--features"
                | "--target"
                | "--target-dir"
                | "--manifest-path"
                | "--profile"
                | "--color"
                | "--config"
                | "--test"
                | "--bin"
                | "--example"
                | "--bench"
        );
        if takes_value {
            let Some(value) = words.get(index + 1).map(String::as_str) else {
                return Vec::new();
            };
            if value.starts_with('-') || value.is_empty() {
                return Vec::new();
            }
            if word == "--color" && !matches!(value, "auto" | "always" | "never") {
                return Vec::new();
            }
            if matches!(word, "-j" | "--jobs") && !value.chars().all(|c| c.is_ascii_digit()) {
                return Vec::new();
            }
            index += 2;
            continue;
        }
        // Any other dash-prefixed option is not modeled. Ignoring it would
        // broaden the command into "all harness tests" and invent S3 credit.
        if word.starts_with('-') {
            return Vec::new();
        }
        positional.push(word);
        index += 1;
    }
    // Cargo accepts at most one free filter after options. Extra positionals
    // are not a modeled shape and must not silently use only the first.
    if positional.len() > 1 {
        return Vec::new();
    }
    let filter = positional.first().copied();
    // Without an explicit --test/--bin/--example/--bench target, cargo may run
    // any combination of unit/integration targets (and can disable autotests).
    // Guessing `tests/*.rs` would invent entries that never ran.
    let Some(name) = test_name else {
        return Vec::new();
    };
    // Default cargo integration targets are exactly `tests/<name>.rs` under
    // Cargo's auto-discovery. Custom `[[test]] path = ...`, disabled autotests,
    // and workspace-foreign targets are not modeled — without metadata we only
    // credit a single exact auto-discovered file when the name is a plain
    // identifier.
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Vec::new();
    }
    let stem = name.replace('-', "_");
    // Prefer the underscored form Cargo uses for hyphenated names; if both
    // spellings exist as distinct files, fail closed (ambiguous).
    let a = format!("tests/{stem}.rs");
    let b = format!("tests/{name}.rs");
    let mut hits = Vec::new();
    if graph.files().any(|f| f == a.as_str()) {
        hits.push(a.clone());
    }
    if a != b && graph.files().any(|f| f == b.as_str()) {
        hits.push(b);
    }
    if hits.len() != 1 {
        return Vec::new();
    }
    entries_in_file(graph, source, &hits[0], filter)
}

fn cargo_run_entries(
    words: &[String],
    graph: &crate::callgraph::CallGraph,
    source: &'static str,
) -> Vec<EntryEvidence> {
    // Strict known-option parse up to `--`. Unknown/malformed options fail
    // closed so `cargo run --bin svc --bogus` cannot credit svc::main.
    let mut binary: Option<String> = None;
    let mut index = 2;
    while index < words.len() {
        let word = words[index].as_str();
        if word == "--" {
            // Program argv after `--` is unmodeled; help,
            // subcommands, and filters can all change what executes.
            if index + 1 < words.len() {
                return Vec::new();
            }
            break;
        }
        if let Some(value) = word.strip_prefix("--bin=") {
            if value.is_empty() || binary.is_some() {
                return Vec::new();
            }
            // Keep the Cargo target name as written. Do not rewrite '-' to
            // '_' — that would credit `src/bin/svc_api.rs` for `--bin svc-api`.
            binary = Some(value.to_string());
            index += 1;
            continue;
        }
        if word == "--bin" {
            let Some(value) = words.get(index + 1).map(String::as_str) else {
                return Vec::new();
            };
            if value.starts_with('-') || value.is_empty() || binary.is_some() {
                return Vec::new();
            }
            binary = Some(value.to_string());
            index += 2;
            continue;
        }
        if let Some(value) = word.strip_prefix("--color=") {
            if !matches!(value, "auto" | "always" | "never") {
                return Vec::new();
            }
            index += 1;
            continue;
        }
        if word == "--color" {
            let Some(value) = words.get(index + 1).map(String::as_str) else {
                return Vec::new();
            };
            if !matches!(value, "auto" | "always" | "never") {
                return Vec::new();
            }
            index += 2;
            continue;
        }
        // Package/target/manifest selection is not modeled for binary mapping.
        if word == "-p"
            || word == "--package"
            || word.starts_with("--package=")
            || word == "--manifest-path"
            || word.starts_with("--manifest-path=")
            || word == "--target"
            || word.starts_with("--target=")
            || word.starts_with('-')
        {
            return Vec::new();
        }
        // Unexpected positional before `--` is not a known cargo-run shape.
        return Vec::new();
    }
    // Exactly one candidate file. Multiple workspace packages with the same
    // binary name would otherwise let one package's run credit another's main.
    let mut candidates: Vec<&str> = graph
        .files()
        .filter(|file| match &binary {
            Some(binary) => {
                (file.starts_with("src/bin/") || file.contains("/src/bin/"))
                    && file_stem(file) == binary
            }
            None => *file == "src/main.rs" || file.ends_with("/src/main.rs"),
        })
        .collect();
    candidates.sort();
    candidates.dedup();
    if candidates.len() != 1 {
        return Vec::new();
    }
    exact_entry(graph, source, candidates[0], "main")
}

/// Map supported command shapes to indexed entry symbols. Unknown commands
/// yield no evidence rather than guessing. The mapping is intentionally small
/// and deterministic; runtime trace/coverage is the future general solution.
pub fn command_entries(
    command: &str,
    graph: &crate::callgraph::CallGraph,
    source: &'static str,
) -> Vec<EntryEvidence> {
    command_entries_from(command, graph, source, CommandOrigin::Untrusted)
}

/// Where a command string came from determines whether bare `loom` is a
/// trustworthy name for this checkout's binary.
///
/// Generic validation commands may resolve `loom` through an arbitrary PATH,
/// so the public mapper remains fail-closed. Recorded journey steps are
/// different: the journey runner executes them through `subprocess`, which
/// binds both direct and compound bare `loom` invocations to `current_exe`.
/// Only `derived_entries` can confer that narrowly proven origin, after it has
/// matched the validation's exact outer journey runner and artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandOrigin {
    Untrusted,
    CheckoutBoundJourneyStep,
}

fn command_entries_from(
    command: &str,
    graph: &crate::callgraph::CallGraph,
    source: &'static str,
    origin: CommandOrigin,
) -> Vec<EntryEvidence> {
    if command.contains("||")
        || command.contains("&&")
        || command.contains('|')
        || command.contains(';')
    {
        return Vec::new();
    }
    let words = shell_words(command);
    let Some(first) = words.first().map(String::as_str) else {
        return Vec::new();
    };
    // Help/version never execute the claimed surface. Fail closed for every
    // derived command shape (cargo, loom, scripts, binaries).
    if argv_has_help_or_version(&words) {
        return Vec::new();
    }
    let mut out = Vec::new();
    if first == "cargo" && words.get(1).map(String::as_str) == Some("test") {
        out.extend(cargo_test_entries(&words, graph, source));
    } else if first == "cargo" && words.get(1).map(String::as_str) == Some("run") {
        out.extend(cargo_run_entries(&words, graph, source));
    } else {
        let _binary = first.rsplit('/').next().unwrap_or(first);
        // Only a checkout-bound loom binary. Bare `loom` normally resolves
        // through PATH and may be a different install; it is accepted only
        // when the caller proved this is a recorded journey step, whose runner
        // binds bare loom to current_exe. Absolute paths outside the checkout
        // remain untrusted. Accept `./loom` and exact target paths everywhere.
        // Exact checkout-bound binaries only. Lexical `target/**/loom` would
        // accept `target/../../tmp/loom` and credit an external binary.
        let is_checkout_loom = matches!(
            first,
            "./loom" | "target/debug/loom" | "target/release/loom"
        ) || (first == "loom"
            && origin == CommandOrigin::CheckoutBoundJourneyStep);
        if is_checkout_loom {
            // Parse the real typed CLI rather than approximating Clap's flag,
            // enum, positional, help, and nested-subcommand grammar. The
            // explicit route table below then names the exact dispatcher or
            // leaf handler entered by `commands::run`.
            if let Some(handler) = loom_cli_handler(&words) {
                out.extend(exact_entry(graph, source, handler.file, handler.symbol));
            }
        } else if first.contains('/')
            || first.ends_with(".py")
            || first.ends_with(".js")
            || first.ends_with(".ts")
            || first.ends_with(".rs")
            || matches!(first, "python" | "python3" | "node" | "bash" | "sh" | "zsh")
                && words.get(1).is_some_and(|arg| {
                    arg.ends_with(".py") || arg.ends_with(".js") || arg.ends_with(".ts")
                })
        {
            // Direct script paths map only when the file has a single obvious
            // entry symbol named exactly `main`/`run`/`handler`; a script with
            // many possible entry points cannot prove which one executes, and
            // substring look-alikes must not manufacture evidence. The file
            // itself must also be unambiguous: a bare `check.py` must not
            // credit an unrelated program the shell resolves from PATH, so only
            // a single registered file with that name (or an explicit
            // repo-relative path with a single canonical match) qualifies.
            // An interpreter prefix (`python3 script.py`) is consumed; the
            // script argument is the entry surface.
            let script = if matches!(first, "python" | "python3" | "node" | "bash" | "sh" | "zsh") {
                words.get(1).map(String::as_str).unwrap_or(first)
            } else {
                first
            };
            // Unmodeled trailing argv (including --help already handled) can
            // select a different path than bare script entry. Fail closed if
            // anything follows the script token.
            let script_index =
                if matches!(first, "python" | "python3" | "node" | "bash" | "sh" | "zsh") {
                    1
                } else {
                    0
                };
            if words.len() > script_index + 1 {
                return Vec::new();
            }
            let candidate = script.trim_start_matches("./");
            // Bare basenames resolve through PATH/cwd at runtime; only an
            // explicit repo-relative path can be matched against registered
            // files without inventing the wrong surface.
            if !candidate.contains('/') {
                return Vec::new();
            }
            // Exact registered path only. Suffix matching would credit
            // `pkg/tools/check.py` for command `tools/check.py`.
            let matches: Vec<&str> = graph.files().filter(|file| file == &candidate).collect();
            if matches.len() == 1 {
                let file = matches[0];
                for entry in ["main", "run", "handler"] {
                    if graph.file_defines(file, entry)
                        // A definition alone proves nothing: the script must
                        // actually invoke the entry at top level. Only a
                        // file-scope call in the script itself qualifies (e.g.
                        // `if __name__ == "__main__": main()` — its caller has
                        // no enclosing symbol). A call from another dead
                        // function is not execution and must fail closed.
                        && graph.edges().iter().any(|edge| {
                            edge.to_file == file
                                && edge.to_symbol == entry
                                && edge.from_file == file
                                && edge.from_symbol.is_empty()
                        })
                    {
                        out.push(EntryEvidence::plain(
                            source,
                            file.into(),
                            Some(entry.into()),
                            true,
                        ));
                        break;
                    }
                }
            }
        } else {
            // A bare command name resolves through PATH at runtime. Mapping it
            // to `src/bin/<name>.rs` invents a repo surface the shell may never
            // execute. Require `cargo run --bin` (or an explicit path script).
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LoomCliHandler {
    file: &'static str,
    symbol: &'static str,
}

fn cli_handler(file: &'static str, symbol: &'static str) -> Option<LoomCliHandler> {
    Some(LoomCliHandler { file, symbol })
}

/// Resolve a syntactically and semantically valid Loom argv to the exact entry
/// selected by `commands::run`. Unsupported command families return no credit;
/// extending this table requires pointing at a real, uniquely identified
/// dispatcher or leaf handler.
fn loom_cli_handler(words: &[String]) -> Option<LoomCliHandler> {
    let cli = crate::cli::Cli::try_parse_from(words).ok()?;
    let json = cli.json;
    match cli.command? {
        crate::cli::Command::Welcome => cli_handler("src/commands/orient_cmd.rs", "welcome"),
        crate::cli::Command::Sync { .. } => cli_handler("src/commands/status_cmd.rs", "sync_cmd"),
        crate::cli::Command::Status => cli_handler("src/commands/status_cmd.rs", "status"),
        crate::cli::Command::Next { mode, all, full } => match (mode, all, full) {
            // These are the same semantic branches and refusals as
            // `commands::run`; Clap alone cannot express the --full coupling.
            (Some(_), true, false) => cli_handler("src/commands/status_cmd.rs", "queue_list"),
            (None, true, false) => cli_handler("src/commands/status_cmd.rs", "next_all"),
            (None, true, true) if json => cli_handler("src/commands/status_cmd.rs", "next_all"),
            (_, false, false) => cli_handler("src/commands/status_cmd.rs", "next_cmd"),
            _ => None,
        },
        crate::cli::Command::Guide { .. } => cli_handler("src/commands/orient_cmd.rs", "guide"),
        crate::cli::Command::Find { .. } => cli_handler("src/commands/discover_cmd.rs", "find_cmd"),
        crate::cli::Command::Explain { .. } => {
            cli_handler("src/commands/discover_cmd.rs", "explain_cmd")
        }
        crate::cli::Command::Coverage => {
            cli_handler("src/commands/diagnostics_cmd/coverage.rs", "coverage_cmd")
        }
        crate::cli::Command::Impact { .. } => {
            cli_handler("src/commands/diagnostics_cmd/impact.rs", "impact_cmd")
        }
        crate::cli::Command::Audit { cmd: None, .. } => {
            cli_handler("src/commands/diagnostics_cmd/impact.rs", "audit_cmd")
        }
        crate::cli::Command::Deepen { .. } => {
            cli_handler("src/commands/diagnostics_cmd/impact.rs", "deepen_cmd")
        }
        crate::cli::Command::Smells => {
            cli_handler("src/commands/diagnostics_cmd/advisory.rs", "smells_cmd")
        }
        crate::cli::Command::Doctor => {
            cli_handler("src/commands/diagnostics_cmd/findings.rs", "doctor_cmd")
        }
        crate::cli::Command::Whoami => {
            cli_handler("src/commands/diagnostics_cmd/coverage.rs", "whoami_cmd")
        }
        crate::cli::Command::Export { .. } => cli_handler("src/commands/status_cmd.rs", "export"),
        crate::cli::Command::Observe { .. } => {
            cli_handler("src/commands/proof_cmd/validate.rs", "observe_cmd")
        }
        crate::cli::Command::Decide { .. } => {
            cli_handler("src/commands/capture_cmd.rs", "decide_cmd")
        }
        crate::cli::Command::Door { .. } => cli_handler("src/commands/capture_cmd.rs", "door"),
        crate::cli::Command::Codefile {
            cmd: crate::cli::CodefileCmd::List { .. },
        } => cli_handler("src/commands/codefile_cmd.rs", "dispatch"),
        crate::cli::Command::Inbox {
            cmd: crate::cli::InboxCmd::Mark { .. } | crate::cli::InboxCmd::Remove { .. },
        } => cli_handler("src/commands/capture_cmd.rs", "inbox"),
        _ => None,
    }
}
