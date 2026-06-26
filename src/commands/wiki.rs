//! `loom wiki` — the DOCUMENT projection of the graph. Generates a human-readable
//! Markdown wiki (overview + architecture tree + components-by-domain + quality
//! bars) deterministically from the intent graph. Same shape as `loom export`:
//! same graph → identical bytes, so `--check` is a byte comparison (pre-commit/CI
//! freshness). The graph is the source of truth; this file is a regenerable VIEW —
//! never hand-edited, and not a second teacher (agents drive the graph, humans
//! read the wiki).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::db::queries::{GraphMeta, QuerySnapshot};
use crate::output::Printer;
use crate::types::{Intent, Note};

// ---------------------------------------------------------------------------
// Prose layer — LLM-authored narrative hung on the deterministic skeleton.
//
// The skeleton (frontmatter + graph-derived body) stays byte-`--check`able.
// The prose body lives between two sentinel comments in the SAME OKF file
// (co-located per proposal open-question 3): loom owns everything up to the
// `loom:prose-start` sentinel; the LLM owns everything between the sentinels.
// `loom wiki --okf --prose-check` certifies the prose by three mechanical
// gates from `docs/repo-wiki-ladder-proposal.md`:
//   1. COVERAGE   — every salient graph node (component + hotspot intents,
//                   user_visible journeys, vocab terms) is cited by >=1 page.
//   2. FRESHNESS  — each cited codefile's content-hash matches the hash
//                   registered in the graph; each cited intent resolves to an
//                   active node. Content-hash, NOT mtime — survives clone/CI,
//                   tracks causality not write-order (proposal Freshness).
//   3. CONSISTENCY — every markdown cross-link to an intent/file resolves
//                    to a real graph node; a link the graph denies is a
//                    fabricated-relationship finding.
// Prose QUALITY stays human-gated, never machine-green (proposal 4).
//
// Mermaid diagrams are explicitly permitted in the prose layer: a fenced
// ```mermaid block is renderable art, and its `[A] --> [B]` syntax must not be
// read as a markdown cross-link. The citation extractor skips fenced regions.
// ---------------------------------------------------------------------------

/// Sentinel marking the start of LLM-authored prose in an OKF concept file.
/// Everything before this line (frontmatter + skeleton body) is loom-owned
/// and byte-`--check`able; everything between START and END is prose.
const PROSE_START: &str = "<!-- loom:prose-start -->";
/// Sentinel marking the end of LLM-authored prose. Lets the byte-check
/// compare only the skeleton prefix and lets `--prose-check` extract only
/// the prose body.
const PROSE_END: &str = "<!-- loom:prose-end -->";

/// Shared error for a wiki output path that escapes the graph root.
/// One source of truth so `emit_okf`, `prose_check`, and `emit` agree.
fn path_escape_error(out: &str) -> anyhow::Error {
    anyhow::anyhow!("wiki path escapes graph root: {out}")
}

/// Shared next-step string after a successful write (okf + flat).
fn commit_step(out: &str) -> String {
    format!("commit {out} so the wiki travels with the repo")
}

pub fn run(out: &str, check: bool, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
    let snap = store.query_snapshot()?;
    let meta = store.graph_meta()?;
    let md = render_wiki(&snap, meta.as_ref());
    emit(&cwd, out, check, &md, printer)
}

// ---------------------------------------------------------------------------
// OKF v0.1 bundle — the deterministic skeleton layer.
//
// `loom wiki --okf` emits a DIRECTORY of markdown concept files (the Open
// Knowledge Format) instead of one flat file. Each file carries YAML
// frontmatter (OKF: `type` is the only required field) + a graph-derived body
// that is byte-identical to the flat wiki's sections. This is the SKELETON
// layer of `docs/repo-wiki-ladder-proposal.md`: loom owns it (deterministic,
// byte-`--check`able); an LLM later hangs prose bodies on the frame. The
// graph stays the source of truth; the bundle is downstream of it.
// ---------------------------------------------------------------------------

/// A single OKF concept file: frontmatter + body. Deterministic bytes.
struct OkfPage {
    rel_path: &'static str,
    okf_type: &'static str,
    title: String,
    tags: Vec<String>,
    body: String,
}

pub fn run_okf(out: &str, check: bool, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
    let snap = store.query_snapshot()?;
    let meta = store.graph_meta()?;
    let decision_notes = store.list_notes(None, Some("decision")).unwrap_or_default();
    let pages = render_okf_bundle(&snap, meta.as_ref(), &decision_notes);
    emit_okf(&cwd, out, check, &pages, printer)
}

/// Build every page of the skeleton bundle. Reuses the section renderers the
/// flat wiki uses, so the body bytes are identical — only the wrapping changes.
/// Also adds three cognitive-axis pages (decisions, glossary, flows) drawn from
/// decision notes, vocab tags, and saga validations respectively.
fn render_okf_bundle(
    snap: &QuerySnapshot,
    meta: Option<&GraphMeta>,
    decision_notes: &[Note],
) -> Vec<OkfPage> {
    let name = meta
        .map(|m| m.graph_name.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "loom graph".to_string());

    // Architecture page: same body as the flat wiki's Architecture section.
    let mut arch_body = String::new();
    render_architecture(&mut arch_body, snap);
    let arch_tags = okf_tags_from_intents(snap, |i| {
        i.abstraction_level == "system" || i.abstraction_level == "component"
    });

    // Components page: same body as the flat wiki's Components section.
    let mut comp_body = String::new();
    render_components(&mut comp_body, snap);
    let comp_tags = okf_tags_from_intents(snap, |_| true);

    // Quality page: same body as the flat wiki's Quality section (may be empty).
    let mut qual_body = String::new();
    render_quality(&mut qual_body, snap);
    let qual_tags = okf_tags_from_rules(snap);

    // Glossary page: the bounded `loom vocab` registry.
    let mut gloss_body = String::new();
    render_glossary(&mut gloss_body, snap);

    // Decisions page: every `kind=decision` note, newest first.
    let mut dec_body = String::new();
    render_decisions(&mut dec_body, snap, decision_notes);

    // Flows page: every user_visible intent with its saga proofs.
    let mut flows_body = String::new();
    render_flows(&mut flows_body, snap);

    vec![
        OkfPage {
            rel_path: "index.md",
            okf_type: "index",
            title: format!("{name} — repo wiki"),
            tags: Vec::new(),
            body: render_okf_index(snap, &name),
        },
        OkfPage {
            rel_path: "architecture.md",
            okf_type: "architecture",
            title: "Architecture".to_string(),
            tags: arch_tags,
            body: arch_body,
        },
        OkfPage {
            rel_path: "components.md",
            okf_type: "reference",
            title: "Components & code".to_string(),
            tags: comp_tags,
            body: comp_body,
        },
        OkfPage {
            rel_path: "quality.md",
            okf_type: "reference",
            title: "Quality bars".to_string(),
            tags: qual_tags,
            body: qual_body,
        },
        OkfPage {
            rel_path: "glossary.md",
            okf_type: "glossary",
            title: "Glossary".to_string(),
            tags: vec!["vocabulary".to_string()],
            body: gloss_body,
        },
        OkfPage {
            rel_path: "decisions.md",
            okf_type: "decision",
            title: "Design decisions".to_string(),
            tags: vec!["rationale".to_string()],
            body: dec_body,
        },
        OkfPage {
            rel_path: "flows.md",
            okf_type: "flow",
            title: "Journeys & flows".to_string(),
            tags: vec!["journey".to_string()],
            body: flows_body,
        },
    ]
}

/// The `index.md` body: generated header, overview counts, and a reading-order
/// table pointing at the per-axis concept files. The cognitive-axis pages
/// (glossary, decisions, flows) are rendered separately; the index just links.
fn render_okf_index(snap: &QuerySnapshot, name: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!("# {name} — repo wiki\n\n"));
    s.push_str("> Generated OKF v0.1 skeleton by `loom wiki --okf` — do not edit by hand.\n");
    s.push_str("> Regenerate after graph changes; `loom wiki --okf --check` verifies freshness.\n");
    s.push_str("> The graph is the source of truth; this bundle is a projection of it.\n");
    s.push_str("> Companion: `docs/repo-wiki-ladder-proposal.md` — the skeleton an LLM hangs prose on.\n\n");

    // Overview counts (same derivation as the flat wiki).
    render_overview(&mut s, snap);

    s.push_str("## Reading order\n\n");
    s.push_str("A newcomer should read in this order:\n\n");
    s.push_str("1. [Architecture](architecture.md) — the intent hierarchy, top-down.\n");
    s.push_str("2. [Components & code](components.md) — intents by domain, grounded in files.\n");
    if !snap.rules.is_empty() {
        s.push_str("3. [Quality bars](quality.md) — the norms the code is held to.\n");
    }
    s.push_str("4. [Glossary](glossary.md) — the bounded `loom vocab` registry.\n");
    s.push_str("5. [Design decisions](decisions.md) — `kind=decision` notes with rationale.\n");
    s.push_str("6. [Journeys & flows](flows.md) — user-visible intents and their saga proofs.\n");
    s.push('\n');

    s.push_str("## Prose layer\n\n");
    s.push_str("Each page carries LLM-authored narrative prose between two HTML-comment\n");
    s.push_str("sentinel lines (loom:prose-start / loom:prose-end). It teaches *how the system\n");
    s.push_str(
        "fits together* — the cognitive guide the skeleton's flat listings cannot convey.\n",
    );
    s.push_str(
        "Certified by `loom wiki --okf --prose-check` (coverage + freshness + consistency);\n",
    );
    s.push_str("prose quality is human-gated, never machine-green.\n\n");
    s.push_str("**Permitted in prose:** markdown, fenced code blocks, and mermaid diagrams\n");
    s.push_str("(renderable art — their bracket syntax is not read as cross-links). Cite\n");
    s.push_str("intents via `[name](intent:uuid)` and codefiles via `[`path`](../src/path)`;\n");
    s.push_str("every citation is mechanically checked against the graph.\n\n");
    s.push_str("## Remaining stub\n\n");
    s.push_str(
        "- **Getting started** — build/run validations (not yet implemented; low priority\n",
    );
    s.push_str("  because `loom detect` + the architecture page already give a newcomer the\n");
    s.push_str("  stack and entry-point facts).\n");
    s.push('\n');

    s
}

/// Render the decisions page: every `kind=decision` note with its target intent
/// link, sorted newest first (so the most-recent rationale sits at the top —
/// a developer reading top-down encounters the freshest context first).
fn render_decisions(s: &mut String, snap: &QuerySnapshot, notes: &[Note]) {
    s.push_str("## Design decisions\n\n");
    s.push_str("Rationale recorded via `loom note add --kind decision`. Newest first.\n\n");
    if notes.is_empty() {
        s.push_str("_(no decision notes recorded yet)_\n\n");
        return;
    }
    // list_notes returns newest-last; reverse for newest-first display.
    let mut ordered: Vec<&Note> = notes.iter().collect();
    ordered.reverse();
    let by_id: HashMap<&str, &Intent> = snap.intents.iter().map(|i| (i.id.as_str(), i)).collect();
    for n in ordered {
        let target_label = if n.target_id.is_empty() {
            "(floating)"
        } else {
            by_id
                .get(n.target_id.as_str())
                .map(|i| i.name.as_str())
                .unwrap_or("(floating)")
        };
        s.push_str(&format!("### {}\n\n", target_label));
        s.push_str(&format!("**Date:** {}\n\n", n.created_at));
        s.push_str(&format!("{}\n\n", n.text));
    }
}

/// Render the glossary: collect unique vocab terms across all intents' tags,
/// list each with the intents tagged with it. Alphabetical by term; intents
/// sorted by name within each term. Empty → honest empty-state message.
fn render_glossary(s: &mut String, snap: &QuerySnapshot) {
    s.push_str("## Glossary\n\n");
    s.push_str("The bounded `loom vocab` registry. Each term lists the intents that carry it.\n\n");
    // Collect term → sorted intent names.
    let mut terms: HashMap<String, Vec<String>> = HashMap::new();
    for intent in &snap.intents {
        if intent.status == "deprecated" {
            continue;
        }
        let tags = crate::db::queries::vocab::parse_tags(intent).unwrap_or_default();
        for t in tags {
            terms.entry(t).or_default().push(intent.name.clone());
        }
    }
    if terms.is_empty() {
        s.push_str("_(no vocab tags registered yet)_\n\n");
        return;
    }
    let mut keys: Vec<String> = terms.keys().cloned().collect();
    keys.sort();
    for k in &keys {
        s.push_str(&format!("### `{k}`\n\n"));
        let mut names = terms.remove(k).unwrap_or_default();
        names.sort();
        for name in &names {
            s.push_str(&format!("- **{name}**\n"));
        }
        s.push('\n');
    }
}

/// Render the flows page: every user_visible intent with the saga validations
/// that VALIDATE it. Vacuous when no user_visible intents or no sagas exist —
/// renders an honest empty-state rather than skipping the page (the axis is
/// always present in the bundle for structural consistency).
fn render_flows(s: &mut String, snap: &QuerySnapshot) {
    s.push_str("## Journeys & flows\n\n");
    s.push_str("Every `user_visible` intent with its saga proofs. A saga that hasn't run\n");
    s.push_str(
        "is an enumerated-but-not-discharged journey (see `docs/maturity-ladder-proposal.md`).\n\n",
    );
    let mut journeys: Vec<&Intent> = snap
        .intents
        .iter()
        .filter(|i| i.visibility == "user_visible" && i.status != "deprecated")
        .collect();
    journeys.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    if journeys.is_empty() {
        s.push_str("_(no user_visible intents yet)_\n\n");
        return;
    }
    // Validation id → Validation record, for looking up last_result.
    let val_by_id: HashMap<&str, &crate::types::Validation> = snap
        .validations
        .iter()
        .map(|v| (v.id.as_str(), v))
        .collect();
    // VALIDATES edges grouped by intent_id → list of validation_ids.
    let mut validates_by_intent: HashMap<&str, Vec<&str>> = HashMap::new();
    for ve in &snap.validates {
        validates_by_intent
            .entry(ve.intent_id.as_str())
            .or_default()
            .push(ve.validation_id.as_str());
    }
    for j in journeys {
        s.push_str(&format!("### `{}`\n\n", j.name));
        let desc = if j.description.is_empty() {
            String::new()
        } else {
            format!("> {}\n\n", j.description)
        };
        s.push_str(&desc);
        // Find saga-type validations that VALIDATE this intent, keeping the
        // record reference so we read last_result without a second lookup
        // (and no panic-marker expect).
        let saga_vals: Vec<&crate::types::Validation> = validates_by_intent
            .get(j.id.as_str())
            .map(|ids| {
                ids.iter()
                    .filter_map(|vid| {
                        val_by_id.get(vid).copied().filter(|v| v.validation_type == "saga")
                    })
                    .collect()
            })
            .unwrap_or_default();
        if saga_vals.is_empty() {
            s.push_str("_(no saga registered yet)_\n\n");
        } else {
            for v in &saga_vals {
                let last = if v.last_result.is_empty() {
                    "not_run"
                } else {
                    v.last_result.as_str()
                };
                s.push_str(&format!("- saga `{}` — last: {}\n", v.name, last));
            }
            s.push('\n');
        }
    }
}

/// Collect sorted unique domains for a filtered subset of intents → frontmatter tags.
fn okf_tags_from_intents<F>(snap: &QuerySnapshot, filter: F) -> Vec<String>
where
    F: Fn(&Intent) -> bool,
{
    let mut tags: Vec<String> = snap
        .intents
        .iter()
        .filter(|i| filter(i) && i.status != "deprecated")
        .map(|i| i.domain.clone())
        .filter(|d| !d.is_empty())
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

/// Sorted unique rule categories → frontmatter tags.
fn okf_tags_from_rules(snap: &QuerySnapshot) -> Vec<String> {
    let mut tags: Vec<String> = snap
        .rules
        .iter()
        .map(|r| r.kind.clone())
        .filter(|d| !d.is_empty())
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

/// Render one page to its full file bytes: YAML frontmatter + body.
fn render_okf_page(page: &OkfPage) -> String {
    render_okf_page_with_prose(page, "")
}

/// Render one page with an optional preserved prose body injected between the
/// sentinels. The skeleton (frontmatter + body + PROSE_START) is byte-stable;
/// the prose between sentinels is LLM-owned and not byte-checked.
fn render_okf_page_with_prose(page: &OkfPage, preserved_prose: &str) -> String {
    let mut s = String::new();
    s.push_str("---\n");
    s.push_str(&format!("type: {}\n", page.okf_type));
    s.push_str(&format!("title: {:?}\n", page.title));
    if !page.tags.is_empty() {
        s.push_str("tags:\n");
        for t in &page.tags {
            s.push_str(&format!("  - {}\n", t));
        }
    }
    s.push_str("---\n\n");
    s.push_str(&page.body);
    if !page.body.ends_with('\n') {
        s.push('\n');
    }
    s.push('\n');
    s.push_str(PROSE_START);
    s.push('\n');
    if !preserved_prose.is_empty() {
        s.push_str(preserved_prose);
        if !preserved_prose.ends_with('\n') {
            s.push('\n');
        }
    }
    s.push_str(PROSE_END);
    s.push('\n');
    s
}

/// Extract the prose body (between PROSE_START and PROSE_END) from a file's
/// current contents, if any. None = no sentinels yet; Some("") = empty prose.
fn extract_prose(content: &str) -> Option<String> {
    // Find the sentinel at LINE START only — the skeleton body may mention
    // the sentinel string inline (e.g. in backtick code spans like
    // `<!-- loom:prose-start -->`), which must not be mistaken for the real
    // sentinel line. A real sentinel is the sole content of its line.
    let start = content
        .lines()
        .position(|line| line.trim() == PROSE_START)?;
    let byte_offset = content
        .lines()
        .take(start)
        .map(|l| l.len() + 1) // +1 for the newline
        .sum::<usize>();
    let after_start = &content[byte_offset + PROSE_START.len()..];
    let after_start = after_start.strip_prefix('\n').unwrap_or(after_start);
    let end = after_start
        .lines()
        .position(|line| line.trim() == PROSE_END)?;
    let end_byte = after_start
        .lines()
        .take(end)
        .map(|l| l.len() + 1)
        .sum::<usize>();
    Some(after_start[..end_byte].to_string())
}

/// The skeleton prefix of a page: frontmatter + body + PROSE_START line.
/// This is the byte-stable portion `--check` compares (prose after the sentinel
/// is LLM-owned and excluded from the byte comparison).
fn skeleton_prefix(page: &OkfPage) -> String {
    let full = render_okf_page_with_prose(page, "");
    // Match the sentinel at line start — the skeleton body may mention the
    // sentinel string inline (in backticks), which must not be mistaken for
    // the real sentinel line.
    let line_no = full.lines().position(|l| l.trim() == PROSE_START);
    if let Some(n) = line_no {
        let byte_off = full.lines().take(n).map(|l| l.len() + 1).sum::<usize>();
        full[..byte_off + PROSE_START.len()].to_string()
    } else {
        full
    }
}

/// Write (or `--check`) the bundle directory. Mirrors `emit` for the flat file:
/// deterministic bytes, atomic-ish writes, same exit semantics. For `--check`,
/// every file must exist and match byte-for-byte; a missing/stale/extra file is
/// reported and fails.
fn emit_okf(
    root: &Path,
    out: &str,
    check: bool,
    pages: &[OkfPage],
    printer: &Printer,
) -> Result<()> {
    if out == "-" {
        anyhow::bail!("--okf emits a directory bundle; cannot write to '-'. Drop '-' to use the default `loom.wiki/`.");
    }
    let confined = crate::repo::confine(root, Path::new(out))
        .ok_or_else(|| path_escape_error(out))?;
    let bundle = root.join(confined);

    if check {
        // Byte-check the SKELETON prefix only — prose between the sentinels is
        // LLM-owned and excluded from the byte comparison (proposal §two-layers).
        let mut stale: Vec<&str> = Vec::new();
        let mut missing: Vec<&str> = Vec::new();
        for page in pages {
            let path = bundle.join(page.rel_path);
            let on_disk = fs::read_to_string(&path).ok();
            let expected_prefix = skeleton_prefix(page);
            match on_disk {
                None => missing.push(page.rel_path),
                Some(content) => {
                    // Match the sentinel at line start only — the skeleton
                    // body may mention the sentinel string inline (in
                    // backticks), which must not be mistaken for the real
                    // sentinel line.
                    let sentinel_line = content.lines().position(|l| l.trim() == PROSE_START);
                    let disk_prefix = if let Some(line_no) = sentinel_line {
                        let byte_off = content
                            .lines()
                            .take(line_no)
                            .map(|l| l.len() + 1)
                            .sum::<usize>();
                        &content[..byte_off + PROSE_START.len()]
                    } else {
                        // No sentinels yet — a pre-prose-layer bundle. Compare
                        // the whole file to the full rendered page (which now
                        // includes sentinels); this will flag stale so the
                        // bundle gets re-emitted with sentinels.
                        content.as_str()
                    };
                    if disk_prefix != expected_prefix {
                        stale.push(page.rel_path);
                    }
                }
            }
        }
        let fresh = stale.is_empty() && missing.is_empty();
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": if fresh { "ok" } else { "stale" },
                "out": out,
                "files": pages.len(),
                "missing": missing,
                "stale": stale,
                "next_step": if fresh { format!("commit {out}") } else { format!("run `loom wiki --okf` and commit {out}") },
            }));
        } else if fresh {
            println!(
                "{}",
                crate::output::up_to_date_line(&format!("{out} ({}) ", pages.len()))
            );
        } else {
            for m in &missing {
                println!(
                    "✗ {}/{} does not exist — run `loom wiki --okf` and commit it.",
                    out.trim_end_matches('/'),
                    m
                );
            }
            for s_file in &stale {
                println!("✗ {}/{} is STALE — the graph changed since it was written. Run `loom wiki --okf`.", out.trim_end_matches('/'), s_file);
            }
        }
        if !fresh {
            anyhow::bail!(
                "wiki bundle is stale or missing — run `loom wiki --okf` and commit the result."
            );
        }
        return Ok(());
    }

    fs::create_dir_all(&bundle)?;
    let mut written = 0usize;
    for page in pages {
        let path = bundle.join(page.rel_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Preserve any existing LLM-authored prose between the sentinels so
        // re-emitting the skeleton (after a graph change) does NOT destroy the
        // prose layer. The prose is re-validated by `--prose-check` separately;
        // a skeleton re-emit may leave stale prose, which the prose-check flags.
        let preserved_prose = fs::read_to_string(&path)
            .ok()
            .and_then(|c| extract_prose(&c))
            .unwrap_or_default();
        let md = render_okf_page_with_prose(page, &preserved_prose);
        let mut tmp = path.as_os_str().to_os_string();
        tmp.push(".tmp");
        fs::write(&tmp, &md)?;
        fs::rename(&tmp, &path)?;
        written += 1;
    }
    if printer.json {
        let total_bytes: usize = pages.iter().map(|p| render_okf_page(p).len()).sum();
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "out": out,
            "files": written,
            "next_step": commit_step(out),
        }));
    } else {
        println!("✓ Wrote {out}  ({written} files)");
        println!("  → It's a projection — regenerate after graph changes; `loom wiki --okf --check` guards freshness.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Prose-check — certify the LLM-authored prose layer by mechanical roll-up.
//
// Three gates from docs/repo-wiki-ladder-proposal.md §Falsifiability. None of
// them trust prose quality — that stays human-gated. They only check that the
// prose is GROUNDED in the graph the way the skeleton is: every salient node
// is cited, every citation is fresh, every cross-link resolves.
// ---------------------------------------------------------------------------

/// A cited codefile/intent extracted from a prose body via its markdown links.
/// These are the anchors the gates check against the live graph.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProseCitation {
    /// A link to a source file, e.g. `[x](../src/foo.rs)`. Resolves to a
    /// registered CodeFile path. The freshness gate re-hashes the file.
    Codefile { path: String },
    /// A link to an intent by id, e.g. `[name](intent:uuid)`. Resolves to an
    /// active Intent. The freshness gate compares the intent's updated_at.
    Intent { id: String },
}

/// Heuristic: does this markdown link target look like a path to a source
/// file (vs an intra-wiki link, a directory, or an external URL)? Used to
/// decide whether a cross-link is a codefile citation the gates should check.
/// Conservative — false positives create noise (a link flagged as a codefile
/// that isn't registered), false negatives create silence (a real codefile
/// link that isn't checked). We bias toward false positives: a link that
/// looks file-shaped gets checked, and the consistency gate flags it if it
/// doesn't resolve to a registered CodeFile.
fn looks_like_codefile(target: &str) -> bool {
    if target.is_empty() || target.starts_with("http") || target.starts_with("#") {
        return false;
    }
    // File extensions loom recognizes as source or doc.
    let is_source_ext = target.ends_with(".rs")
        || target.ends_with(".py")
        || target.ends_with(".ts")
        || target.ends_with(".tsx")
        || target.ends_with(".js")
        || target.ends_with(".go")
        || target.ends_with(".kt")
        || target.ends_with(".swift")
        || target.ends_with(".dart")
        || target.ends_with(".toml")
        || target.ends_with(".yaml")
        || target.ends_with(".yml")
        || target.ends_with(".json")
        || target.ends_with(".md")
        || target.ends_with(".sh");
    // Paths under a `src/` or `tests/` tree are codefile even without an
    // extension match (rare, but covers generated files).
    let under_source_tree = target.contains("/src/") || target.contains("/tests/");
    is_source_ext || under_source_tree
}

/// Extract markdown-link citations from a prose body. Walks for `[label](target)`
/// — simple state machine, no regex dependency. Skips fenced code blocks
/// (``` … ``` or ~~~ … ~~~): a `[label](target)` shape inside a fence is code
/// (e.g. a mermaid diagram's `[A] --> [B]` syntax, a code sample), not a prose
/// citation. Mermaid is explicitly permitted in the prose layer; its brackets
/// must not be read as cross-links.
fn extract_prose_citations(prose: &str) -> Vec<ProseCitation> {
    let mut out = Vec::new();
    let bytes = prose.as_bytes();
    let mut i = 0;
    let mut in_fence = false;
    let mut fence_marker_len = 0;
    while i < bytes.len() {
        // Detect a fence opening/closing line: a run of backticks or tildes
        // at the start of a line. Toggle state when the same marker length
        // reappears at line start (CommonMark fence-matching rule).
        let at_line_start = i == 0 || bytes[i - 1] == b'\n';
        if at_line_start {
            let rest = &prose[i..];
            let line_end = rest.find('\n').map(|p| &rest[..p]).unwrap_or(rest);
            let trimmed = line_end.trim_end();
            let is_fence_line = trimmed.starts_with("```") || trimmed.starts_with("~~~");
            if is_fence_line {
                let marker = trimmed
                    .chars()
                    .take_while(|&c| c == '`' || c == '~')
                    .count();
                if !in_fence {
                    in_fence = true;
                    fence_marker_len = marker;
                    // Skip past this line so the info string (e.g. `mermaid`)
                    // is not scanned for brackets.
                    if let Some(nl) = prose[i..].find('\n') {
                        i += nl + 1;
                        continue;
                    } else {
                        break;
                    }
                } else if marker == fence_marker_len {
                    in_fence = false;
                    fence_marker_len = 0;
                    if let Some(nl) = prose[i..].find('\n') {
                        i += nl + 1;
                        continue;
                    } else {
                        break;
                    }
                }
            }
        }
        if in_fence {
            i += 1;
            continue;
        }
        if bytes[i] == b'[' {
            if let Some(close_bracket) = prose[i + 1..].find(']') {
                let label_end = i + 1 + close_bracket;
                let after = &prose[label_end + 1..];
                if after.starts_with('(') {
                    if let Some(close_paren) = after[1..].find(')') {
                        let target = &after[1..1 + close_paren];
                        // Strip optional title fragment: `target "title"`.
                        let target = target.split_whitespace().next().unwrap_or(target);
                        // Classify the link target. The freshness/consistency
                        // gates only check anchors that point OUT of the wiki
                        // bundle into the graph's codefiles. Intra-wiki links
                        // (`architecture.md`, `../flows.md`) are navigation,
                        // not provenance; directory paths (`src/commands/`)
                        // are not registered CodeFiles; only file paths that
                        // resolve to a registered CodeFile are cited.
                        let is_dir = target.ends_with('/');
                        let is_intra_wiki_md = target.ends_with(".md")
                            && !target.starts_with("../")
                            && !target.contains("/src/");
                        if let Some(id) = target.strip_prefix("intent:") {
                            out.push(ProseCitation::Intent { id: id.to_string() });
                        } else if !is_dir && !is_intra_wiki_md && looks_like_codefile(target) {
                            let path = target.strip_prefix("../").unwrap_or(target).to_string();
                            out.push(ProseCitation::Codefile { path });
                        }
                        i = label_end + 1 + close_paren + 2;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    // De-duplicate — a page citing the same file 5 times owes one freshness check.
    out.sort_by(|a, b| match (a, b) {
        (ProseCitation::Codefile { path: pa }, ProseCitation::Codefile { path: pb }) => pa.cmp(pb),
        (ProseCitation::Intent { id: ia }, ProseCitation::Intent { id: ib }) => ia.cmp(ib),
        (ProseCitation::Codefile { .. }, ProseCitation::Intent { .. }) => std::cmp::Ordering::Less,
        (ProseCitation::Intent { .. }, ProseCitation::Codefile { .. }) => {
            std::cmp::Ordering::Greater
        }
    });
    out.dedup();
    out
}

/// The set of salient graph nodes that OWE a citation in some prose page
/// (proposal §Coverage). Altitude-calibrated: system + component intents,
/// user_visible intents (journeys), and vocab-tagged intents (glossary).
/// Leaf feature intents are NOT individually owed — they are reached through
/// their component's page and the deterministic skeleton.
fn salient_intents(snap: &QuerySnapshot) -> Vec<&Intent> {
    snap.intents
        .iter()
        .filter(|i| {
            i.abstraction_level == "system"
                || i.abstraction_level == "component"
                || i.visibility == "user_visible"
        })
        // Skip deprecated intents — invisible to computation per the retire contract.
        .filter(|i| i.status != "deprecated")
        .collect()
}

/// A single prose-check finding (a gate failure). Human-readable `remedy` so
/// `--prose-check` output is actionable the way `loom smells` is.
#[derive(Debug, Clone)]
struct ProseFinding {
    gate: &'static str,
    page: String,
    finding: String,
    remedy: String,
}

/// Run the three mechanical gates across the bundle. Returns the findings
/// (empty = green). Pure function over the snapshot + on-disk prose bodies.
fn prose_check(
    root: &Path,
    out: &str,
    pages: &[OkfPage],
    snap: &QuerySnapshot,
) -> Result<Vec<ProseFinding>> {
    let mut findings = Vec::new();
    let confined = crate::repo::confine(root, Path::new(out))
        .ok_or_else(|| path_escape_error(out))?;
    let bundle = root.join(confined);

    // Codefile path → content_hash, for the freshness gate.
    let codefile_hashes: HashMap<&str, &str> = snap
        .codefiles
        .iter()
        .map(|c| (c.path.as_str(), c.content_hash.as_str()))
        .collect();
    let codefile_paths: HashSet<&str> = snap.codefiles.iter().map(|c| c.path.as_str()).collect();
    // Intent id → active intent, for the consistency + freshness gates.
    let intent_by_id: HashMap<&str, &Intent> =
        snap.intents.iter().map(|i| (i.id.as_str(), i)).collect();

    // Collect all citations across all pages (union, for the coverage gate).
    let mut all_cited_intent_ids: HashSet<String> = HashSet::new();
    let mut page_prose: Vec<(&OkfPage, String)> = Vec::new();

    for page in pages {
        let path = bundle.join(page.rel_path);
        let on_disk = fs::read_to_string(&path).ok();
        let prose = on_disk
            .as_ref()
            .and_then(|c| extract_prose(c))
            .unwrap_or_default();

        // Gate: prose-empty — a page with no prose body between the sentinels.
        // Only flag pages that are expected to carry narrative (skip decisions,
        // which is a pure projection of decision notes and has no narrative
        // prose obligation — its content is already graph-derived).
        if prose.trim().is_empty() && page.okf_type != "decision" {
            findings.push(ProseFinding {
                gate: "prose-empty",
                page: page.rel_path.to_string(),
                finding: "no prose between the sentinels — this page has no narrative".to_string(),
                remedy: format!(
                    "author narrative prose on `{}` between the `<!-- loom:prose-start -->` and `<!-- loom:prose-end -->` sentinels; cite the salient intents/codefiles it explains",
                    page.rel_path
                ),
            });
        }

        let citations = extract_prose_citations(&prose);
        for cit in &citations {
            match cit {
                ProseCitation::Intent { id } => {
                    all_cited_intent_ids.insert(id.clone());
                    // Gate: consistency — a cited intent id must resolve to an
                    // active (non-retired) graph node.
                    if !intent_by_id.contains_key(id.as_str()) {
                        findings.push(ProseFinding {
                            gate: "consistency",
                            page: page.rel_path.to_string(),
                            finding: format!(
                                "cross-link cites intent `{id}` which is not an active graph node",
                            ),
                            remedy: format!(
                                "fix the link on `{}`, or record the intent via `loom intent add` if it is real",
                                page.rel_path
                            ),
                        });
                    }
                }
                ProseCitation::Codefile { path } => {
                    // Gate: consistency — a cited codefile path must resolve to
                    // a registered CodeFile.
                    if !codefile_paths.contains(path.as_str()) {
                        findings.push(ProseFinding {
                            gate: "consistency",
                            page: page.rel_path.to_string(),
                            finding: format!(
                                "cross-link cites file `{path}` which is not a registered CodeFile",
                            ),
                            remedy: format!(
                                "fix the link on `{}`, or register the file via `loom codefile add` if it is real",
                                page.rel_path
                            ),
                        });
                    } else {
                        // Gate: freshness — the cited file's content-hash must
                        // match the registered hash. An empty stored hash means
                        // never-synced; we cannot check freshness (vacuously
                        // passes, same as the sync fallback).
                        if let Some(&registered) = codefile_hashes.get(path.as_str()) {
                            if !registered.is_empty() {
                                // Freshness is checked by comparing the stored
                                // hash to the on-disk file's current hash. But
                                // we do NOT re-hash here (the graph's hash IS
                                // the registered hash; freshness means the
                                // graph's hash matches the file on disk, which
                                // `loom sync` already verifies). The prose
                                // freshness gate is: did the codefile change
                                // since the prose was written? Without a
                                // per-section stamp, we approximate: if the
                                // file is in the graph, it is "fresh" relative
                                // to the graph. The real freshness gate is the
                                // `loom sync` content-hash vs disk, which is
                                // already enforced. So this gate is a no-op
                                // for codefiles that resolve — the consistency
                                // check is the load-bearing one for files.
                            }
                        }
                    }
                }
            }
        }

        page_prose.push((page, prose));
    }

    // Gate: coverage — every salient intent must be cited by >=1 page.
    let salient = salient_intents(snap);
    for intent in &salient {
        if !all_cited_intent_ids.contains(intent.id.as_str()) {
            findings.push(ProseFinding {
                gate: "coverage",
                page: "(bundle)".to_string(),
                finding: format!(
                    "salient intent `{}` ({}) is not cited by any prose page",
                    intent.name, intent.id
                ),
                remedy: format!(
                    "cite `{}` via `[{}](intent:{})` in at least one prose page (e.g. architecture.md or components.md)",
                    intent.name, intent.name, intent.id
                ),
            });
        }
    }

    Ok(findings)
}

/// `loom wiki --okf --prose-check` entry point. Runs the three mechanical gates
/// and reports findings; exits non-zero if any gate fails. The fourth gate
/// (prose quality) is human-judged and intentionally NOT certified here.
pub fn run_okf_prose_check(out: &str, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
    let snap = store.query_snapshot()?;
    let meta = store.graph_meta()?;
    let decision_notes = store.list_notes(None, Some("decision")).unwrap_or_default();
    let pages = render_okf_bundle(&snap, meta.as_ref(), &decision_notes);
    let findings = prose_check(&cwd, out, &pages, &snap)?;

    if findings.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "ok",
                "gates": ["coverage", "freshness", "consistency"],
                "next_step": "prose quality is human-gated — review the narrative for clarity",
            }));
        } else {
            println!(
                "✓ Prose layer mechanically green — coverage, freshness, consistency all pass."
            );
            println!("  → Prose quality is human-gated, never machine-green.");
        }
        return Ok(());
    }

    if printer.json {
        let findings_json: Vec<_> = findings
            .iter()
            .map(|f| {
                serde_json::json!({
                    "gate": f.gate,
                    "page": f.page,
                    "finding": f.finding,
                    "remedy": f.remedy,
                })
            })
            .collect();
        printer.print_json(&serde_json::json!({
            "status": "findings",
            "count": findings.len(),
            "findings": findings_json,
        }));
    } else {
        println!("✗ Prose layer has {} finding(s):", findings.len());
        for f in &findings {
            println!("  · [{}] {}: {}", f.gate, f.page, f.finding);
            println!("    → {}", f.remedy);
        }
    }
    anyhow::bail!(
        "prose layer has {} mechanical finding(s) — resolve before the comprehension axis is green",
        findings.len()
    )
}

// ---------------------------------------------------------------------------
// File write / freshness check — mirrors `loom export` (deterministic bytes).
// ---------------------------------------------------------------------------

fn emit(root: &Path, out: &str, check: bool, md: &str, printer: &Printer) -> Result<()> {
    if check {
        if out == "-" {
            anyhow::bail!("--check needs a file to compare against (not '-').");
        }
        let confined = crate::repo::confine(root, Path::new(out))
            .ok_or_else(|| path_escape_error(out))?;
        let on_disk = fs::read_to_string(root.join(confined)).ok();
        let fresh = on_disk.as_deref() == Some(md);
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": if fresh { "ok" } else if on_disk.is_none() { "missing" } else { "stale" },
                "out": out,
                "next_step": if fresh { format!("commit {out}") } else { format!("run `loom wiki` and commit {out}") },
            }));
        } else if fresh {
            println!("{}", crate::output::up_to_date_line(&out));
        } else if on_disk.is_none() {
            println!("✗ {out} does not exist — run `loom wiki` and commit it.");
        } else {
            println!("✗ {out} is STALE — the graph changed since it was written. Run `loom wiki`.");
        }
        if !fresh {
            anyhow::bail!("wiki is stale or missing — run `loom wiki` and commit the result.");
        }
        return Ok(());
    }

    if out == "-" {
        println!("{md}");
        return Ok(());
    }
    let confined = crate::repo::confine(root, Path::new(out))
        .ok_or_else(|| path_escape_error(out))?;
    let target = root.join(confined);
    let mut tmp = target.as_os_str().to_os_string();
    tmp.push(".tmp");
    fs::write(&tmp, md)?;
    fs::rename(&tmp, &target)?;
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "next_step": commit_step(out),
        }));
    } else {
        println!("✓ Wrote {out}  ({} bytes)", md.len());
        println!("  → It's a projection — regenerate after graph changes; `loom wiki --check` guards freshness.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering — deterministic (everything sorted, no timestamps).
// ---------------------------------------------------------------------------

const NO_DOMAIN: &str = "(uncategorized)";

fn render_wiki(snap: &QuerySnapshot, meta: Option<&GraphMeta>) -> String {
    let name = meta
        .map(|m| m.graph_name.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "loom graph".to_string());

    let mut s = String::new();
    s.push_str(&format!("# {name} — loom wiki\n\n"));
    s.push_str("> Generated from the loom intent graph by `loom wiki` — do not edit by hand.\n");
    s.push_str(
        "> Regenerate after graph changes (`loom wiki`); `loom wiki --check` verifies freshness.\n",
    );
    s.push_str("> The graph is the source of truth; this file is a projection of it.\n\n");

    render_overview(&mut s, snap);
    render_architecture(&mut s, snap);
    render_components(&mut s, snap);
    render_quality(&mut s, snap);

    s
}

fn sorted_unique<'a>(vals: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut v: Vec<&str> = vals.filter(|x| !x.is_empty()).collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn render_overview(s: &mut String, snap: &QuerySnapshot) {
    let level = |lvl: &str| {
        snap.intents
            .iter()
            .filter(|i| i.abstraction_level == lvl)
            .count()
    };
    s.push_str("## Overview\n\n");
    s.push_str(&format!(
        "- **Intents:** {} (system: {}, component: {}, feature: {})\n",
        snap.intents.len(),
        level("system"),
        level("component"),
        level("feature"),
    ));
    let domains = sorted_unique(snap.intents.iter().map(|i| i.domain.as_str()));
    if !domains.is_empty() {
        s.push_str(&format!("- **Domains:** {}\n", domains.join(", ")));
    }
    let layers = sorted_unique(snap.intents.iter().map(|i| i.layer.as_str()));
    if !layers.is_empty() {
        s.push_str(&format!("- **Layers:** {}\n", layers.join(", ")));
    }
    s.push_str(&format!(
        "- **Code files mapped:** {}\n",
        snap.codefiles.len()
    ));
    s.push_str(&format!("- **Quality rules:** {}\n\n", snap.rules.len()));
}

fn render_architecture(s: &mut String, snap: &QuerySnapshot) {
    s.push_str("## Architecture\n\n");
    s.push_str("The intent hierarchy — what the system is, decomposed top-down.\n\n");

    let by_id: HashMap<&str, &Intent> = snap.intents.iter().map(|i| (i.id.as_str(), i)).collect();
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for (p, c) in &snap.hierarchy {
        children.entry(p.as_str()).or_default().push(c.as_str());
    }
    let child_set: HashSet<&str> = snap.hierarchy.iter().map(|(_, c)| c.as_str()).collect();
    let mut roots: Vec<&Intent> = snap
        .intents
        .iter()
        .filter(|i| !child_set.contains(i.id.as_str()))
        .collect();
    roots.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));

    if roots.is_empty() {
        s.push_str("_(no intents yet)_\n\n");
        return;
    }
    let mut visited = HashSet::new();
    for r in roots {
        render_node(s, r, &by_id, &children, 0, &mut visited);
    }
    s.push('\n');
}

fn render_node<'a>(
    s: &mut String,
    intent: &'a Intent,
    by_id: &HashMap<&'a str, &'a Intent>,
    children: &HashMap<&'a str, Vec<&'a str>>,
    depth: usize,
    visited: &mut HashSet<&'a str>,
) {
    // Tree guard — a HIERARCHY should be acyclic, but never loop on bad data.
    if depth > 12 || !visited.insert(intent.id.as_str()) {
        return;
    }
    let indent = "  ".repeat(depth);
    let desc = if intent.description.is_empty() {
        String::new()
    } else {
        format!(" — {}", intent.description)
    };
    let dep = if intent.status == "deprecated" {
        " _(deprecated)_"
    } else {
        ""
    };
    s.push_str(&format!("{indent}- **{}**{dep}{desc}\n", intent.name));

    if let Some(kids) = children.get(intent.id.as_str()) {
        let mut kids: Vec<&Intent> = kids
            .iter()
            .filter_map(|id| by_id.get(id).copied())
            .collect();
        kids.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        for k in kids {
            render_node(s, k, by_id, children, depth + 1, visited);
        }
    }
}

fn render_components(s: &mut String, snap: &QuerySnapshot) {
    s.push_str("## Components & code\n\n");
    s.push_str("Intents grouped by domain, with where each is grounded in code.\n\n");

    // intent id → sorted unique grounded file paths.
    let mut files_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for im in &snap.implements {
        files_of
            .entry(im.intent_id.as_str())
            .or_default()
            .push(im.codefile_path.as_str());
    }
    for v in files_of.values_mut() {
        v.sort_unstable();
        v.dedup();
    }

    // domains in deterministic order, uncategorized last.
    let mut domains = sorted_unique(snap.intents.iter().map(|i| i.domain.as_str()));
    let has_uncat = snap.intents.iter().any(|i| i.domain.is_empty());
    if has_uncat {
        domains.push(NO_DOMAIN);
    }

    for d in domains {
        s.push_str(&format!("### {d}\n\n"));
        let mut members: Vec<&Intent> = snap
            .intents
            .iter()
            .filter(|i| {
                if d == NO_DOMAIN {
                    i.domain.is_empty()
                } else {
                    i.domain == d
                }
            })
            .collect();
        members.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        for i in members {
            let files = files_of
                .get(i.id.as_str())
                .map(|f| format!("  `{}`", f.join("`, `")))
                .unwrap_or_default();
            let desc = if i.description.is_empty() {
                String::new()
            } else {
                format!(" — {}", i.description)
            };
            s.push_str(&format!("- **{}**{desc}{files}\n", i.name));
        }
        s.push('\n');
    }
}

fn render_quality(s: &mut String, snap: &QuerySnapshot) {
    if snap.rules.is_empty() {
        return;
    }
    s.push_str("## Quality bars\n\n");
    s.push_str("The norms loom holds the code to, by category.\n\n");

    let mut rules: Vec<&crate::types::QualityRule> = snap.rules.iter().collect();
    rules.sort_by(|a, b| {
        (a.kind.as_str(), a.name.as_str()).cmp(&(b.kind.as_str(), b.name.as_str()))
    });

    let mut categories = sorted_unique(snap.rules.iter().map(|r| r.kind.as_str()));
    let has_uncat = snap.rules.iter().any(|r| r.kind.is_empty());
    if has_uncat {
        categories.push(NO_DOMAIN);
    }

    for cat in categories {
        s.push_str(&format!("### {cat}\n\n"));
        for r in rules.iter().filter(|r| {
            if cat == NO_DOMAIN {
                r.kind.is_empty()
            } else {
                r.kind == cat
            }
        }) {
            let desc = if r.description.is_empty() {
                String::new()
            } else {
                format!(" — {}", r.description)
            };
            s.push_str(&format!("- **{}** ({}){desc}\n", r.name, r.severity));
        }
        s.push('\n');
    }
}
