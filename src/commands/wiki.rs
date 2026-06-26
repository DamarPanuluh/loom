//! `loom wiki` — the v2 code-primary repo wiki. Emits a directory bundle of
//! markdown concept files whose prose links to source files (not intent UUIDs),
//! with the intent graph as an invisible manifest that certifies the prose via
//! coverage, freshness, and graph-aware consistency gates.
//!
//! v2 hard-cuts v1 (2026-06-26): the flat `loom.wiki.md` and the graph-primary
//! OKF emitter are deleted. loom's only wiki is the v2 bundle. The graph is
//! the manifest — invisible to the reader, present for the gates. The reader
//! sees `[`src/saga/runner.rs`](../src/saga/runner.rs)`; loom internally
//! resolves that file to its intent and runs coverage/freshness/consistency
//! against *that*. The `intent:UUID` never appears in reader-facing prose.
//!
//! Two layers of one artifact:
//! - **Manifest layer** (frontmatter: `sourceFiles` + `symbols` + `provenance`
//!   stamp — byte-`--check`able, machine-owned). Same graph → identical bytes.
//! - **Prose layer** (code-primary narrative with file-path links — gate-checked,
//!   LLM-owned). Checked by coverage + freshness + consistency, never by bytes.
//!
//! `loom wiki --okf --prose-check` certifies the prose by three mechanical gates
//! from `docs/repo-wiki-ladder-proposal.md`:
//!   1. COVERAGE — every salient intent with grounded files has its files
//!      appear in some page's `sourceFiles`.
//!   2. FRESHNESS — the manifest's `provenance` stamp matches the current
//!      graph's content hashes (byte-checked via `--check`).
//!   3. CONSISTENCY — every file path in `sourceFiles` and every codefile link
//!      in prose resolves to a registered CodeFile.
//!
//! Prose QUALITY stays human-gated, never machine-green (proposal 4).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::db::queries::{GraphMeta, QuerySnapshot};
use crate::output::Printer;
use crate::types::{Intent, Note};

// ---------------------------------------------------------------------------
// Prose layer — LLM-authored narrative hung on the manifest frame.
//
// The manifest (frontmatter + graph-derived body) stays byte-`--check`able.
// The prose body lives between two sentinel comments in the SAME file
// (co-located per proposal open-question 3): loom owns everything up to the
// `loom:prose-start` sentinel; the LLM owns everything between the sentinels.
//
// Mermaid diagrams are explicitly permitted in the prose layer: a fenced
// ```mermaid block is renderable art, and its `[A] --> [B]` syntax must not be
// read as a markdown cross-link. The citation extractor skips fenced regions.
// ---------------------------------------------------------------------------

/// Sentinel marking the start of LLM-authored prose in a wiki concept file.
const PROSE_START: &str = "<!-- loom:prose-start -->";
/// Sentinel marking the end of LLM-authored prose.
const PROSE_END: &str = "<!-- loom:prose-end -->";

/// Shared error for a wiki output path that escapes the graph root.
fn path_escape_error(out: &str) -> anyhow::Error {
    anyhow::anyhow!("wiki path escapes graph root: {out}")
}

/// Shared next-step string after a successful write.
fn commit_step(out: &str) -> String {
    format!("commit {out} so the wiki travels with the repo")
}

/// Slugify a component name for a module page filename: lowercase, non-alnum → hyphen.
fn slugify(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    // Collapse runs of hyphens, strip leading/trailing.
    let mut prev_hyphen = false;
    let mut out = String::with_capacity(slug.len());
    for c in slug.chars() {
        if c == '-' {
            if !prev_hyphen && !out.is_empty() {
                out.push('-');
            }
            prev_hyphen = true;
        } else {
            out.push(c);
            prev_hyphen = false;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

// ---------------------------------------------------------------------------
// OKF v0.1 bundle — the manifest layer + prose layer.
// ---------------------------------------------------------------------------

/// A single wiki concept file: manifest frontmatter + body + prose.
struct OkfPage {
    rel_path: String,
    okf_type: &'static str,
    title: String,
    tags: Vec<String>,
    /// The graph-derived body (byte-stable skeleton content).
    body: String,
    /// Code-native grounding: the source files this page explains. The manifest
    /// resolves these to intents via IMPLEMENTS edges at check time.
    source_files: Vec<String>,
    /// Symbols from the grounded code (locators from IMPLEMENTS edges).
    symbols: Vec<String>,
}

/// `loom wiki` entry point — emits the v2 code-primary bundle (hard-cut: no flat wiki).
pub fn run(out: &str, check: bool, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
    let snap = store.query_snapshot()?;
    let meta = store.graph_meta()?;
    let decision_notes = store.list_notes(None, Some("decision")).unwrap_or_default();
    let pages = render_okf_bundle(&snap, meta.as_ref(), &decision_notes);
    let codefile_hashes: HashMap<&str, &str> = snap
        .codefiles
        .iter()
        .map(|c| (c.path.as_str(), c.content_hash.as_str()))
        .collect();
    emit_okf(&cwd, out, check, &pages, &codefile_hashes, printer)
}

/// Build every page of the v2 bundle: topical pages + per-component module pages.
fn render_okf_bundle(
    snap: &QuerySnapshot,
    meta: Option<&GraphMeta>,
    decision_notes: &[Note],
) -> Vec<OkfPage> {
    let name = meta
        .map(|m| m.graph_name.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "loom graph".to_string());

    // --- Topical pages (bounded, cross-cutting) ---

    let mut arch_body = String::new();
    render_architecture(&mut arch_body, snap);
    let arch_tags = okf_tags_from_intents(snap, |i| {
        i.abstraction_level == "system" || i.abstraction_level == "component"
    });

    let mut comp_body = String::new();
    render_components(&mut comp_body, snap);
    let comp_tags = okf_tags_from_intents(snap, |_| true);

    let mut qual_body = String::new();
    render_quality(&mut qual_body, snap);
    let qual_tags = okf_tags_from_rules(snap);
    let mut gloss_body = String::new();
    render_glossary(&mut gloss_body, snap);

    let mut dec_body = String::new();
    render_decisions(&mut dec_body, snap, decision_notes);

    let mut flows_body = String::new();
    render_flows(&mut flows_body, snap);

    let mut pages = vec![
        OkfPage {
            rel_path: "index.md".to_string(),
            okf_type: "index",
            title: format!("{name} — repo wiki"),
            tags: Vec::new(),
            body: render_okf_index(snap, &name),
            source_files: Vec::new(),
            symbols: Vec::new(),
        },
        OkfPage {
            rel_path: "architecture.md".to_string(),
            okf_type: "architecture",
            title: "Architecture".to_string(),
            tags: arch_tags,
            body: arch_body,
            source_files: Vec::new(),
            symbols: Vec::new(),
        },
        OkfPage {
            rel_path: "components.md".to_string(),
            okf_type: "reference",
            title: "Components & code".to_string(),
            tags: comp_tags,
            body: comp_body,
            source_files: Vec::new(),
            symbols: Vec::new(),
        },
        OkfPage {
            rel_path: "quality.md".to_string(),
            okf_type: "reference",
            title: "Quality bars".to_string(),
            tags: qual_tags,
            body: qual_body,
            source_files: Vec::new(),
            symbols: Vec::new(),
        },
        OkfPage {
            rel_path: "glossary.md".to_string(),
            okf_type: "glossary",
            title: "Glossary".to_string(),
            tags: vec!["vocabulary".to_string()],
            body: gloss_body,
            source_files: Vec::new(),
            symbols: Vec::new(),
        },
        OkfPage {
            rel_path: "decisions.md".to_string(),
            okf_type: "decision",
            title: "Design decisions".to_string(),
            tags: vec!["rationale".to_string()],
            body: dec_body,
            source_files: Vec::new(),
            symbols: Vec::new(),
        },
        OkfPage {
            rel_path: "flows.md".to_string(),
            okf_type: "flow",
            title: "Journeys & flows".to_string(),
            tags: vec!["journey".to_string()],
            body: flows_body,
            source_files: Vec::new(),
            symbols: Vec::new(),
        },
    ];

    // --- Module pages (one per component intent) ---

    // Group IMPLEMENTS by intent_id for sourceFiles derivation.
    let mut files_of: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut locators_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for im in &snap.implements {
        files_of
            .entry(im.intent_id.as_str())
            .or_default()
            .push(im.codefile_path.as_str());
        if !im.locator.is_empty() {
            locators_of
                .entry(im.intent_id.as_str())
                .or_default()
                .push(im.locator.as_str());
        }
    }

    let mut components: Vec<&Intent> = snap
        .intents
        .iter()
        .filter(|i| i.abstraction_level == "component" && i.status != "deprecated")
        .collect();
    components.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));

    for comp in &components {
        let slug = slugify(&comp.name);
        let rel_path = format!("modules/{slug}.md");
        let mut source_files: Vec<String> = files_of
            .get(comp.id.as_str())
            .map(|v| {
                let mut s: Vec<String> = v.iter().map(|s| s.to_string()).collect();
                s.sort();
                s.dedup();
                s
            })
            .unwrap_or_default();
        // Also include files from child features (the component's subtree).
        if source_files.is_empty() {
            // A component with no direct IMPLEMENTS: gather from descendants.
            let descendants = collect_descendants(comp.id.as_str(), snap);
            let mut gathered: Vec<String> = Vec::new();
            for did in &descendants {
                if let Some(files) = files_of.get(did.as_str()) {
                    for f in files {
                        gathered.push(f.to_string());
                    }
                }
            }
            gathered.sort();
            gathered.dedup();
            source_files = gathered;
        }
        let mut symbols: Vec<String> = locators_of
            .get(comp.id.as_str())
            .map(|v| {
                let mut s: Vec<String> = v.iter().map(|s| s.to_string()).collect();
                s.sort();
                s.dedup();
                s
            })
            .unwrap_or_default();
        // Include descendant locators if we gathered from descendants.
        if symbols.is_empty() {
            let descendants = collect_descendants(comp.id.as_str(), snap);
            let mut gathered: Vec<String> = Vec::new();
            for did in &descendants {
                if let Some(locs) = locators_of.get(did.as_str()) {
                    for l in locs {
                        gathered.push(l.to_string());
                    }
                }
            }
            gathered.sort();
            gathered.dedup();
            symbols = gathered;
        }
        let body = render_module_page(comp);
        pages.push(OkfPage {
            rel_path,
            okf_type: "module",
            title: comp.name.clone(),
            tags: vec![comp.domain.clone()]
                .into_iter()
                .filter(|d| !d.is_empty() && d != "unknown")
                .collect(),
            source_files,
            symbols,
            body,
        });
    }

    pages
}

/// Collect all descendant intent ids of a given parent (transitive via HIERARCHY).
fn collect_descendants(parent_id: &str, snap: &QuerySnapshot) -> Vec<String> {
    let mut children_map: HashMap<&str, Vec<&str>> = HashMap::new();
    for (p, c) in &snap.hierarchy {
        children_map.entry(p.as_str()).or_default().push(c.as_str());
    }
    let mut result = Vec::new();
    let mut queue: Vec<&str> = children_map.get(parent_id).cloned().unwrap_or_default();
    let mut visited: HashSet<&str> = HashSet::new();
    while let Some(id) = queue.pop() {
        if !visited.insert(id) {
            continue;
        }
        result.push(id.to_string());
        if let Some(kids) = children_map.get(id) {
            for k in kids {
                queue.push(k);
            }
        }
    }
    result
}

/// Render a module page body (the skeleton; prose goes between sentinels).
fn render_module_page(comp: &Intent) -> String {
    let mut s = String::new();
    s.push_str(&format!("# {}\n\n", comp.name));
    let desc = if comp.description.is_empty() {
        String::new()
    } else {
        format!("> {}\n\n", comp.description)
    };
    s.push_str(&desc);
    s.push_str("## Responsibility\n\n");
    s.push_str("_(LLM-authored: what this module does and why it exists.)_\n\n");
    s.push_str("## Key entry points\n\n");
    s.push_str("_(LLM-authored: the main entry symbols and where to start reading.)_\n\n");
    s.push_str("## Key flows\n\n");
    s.push_str("_(LLM-authored: how a request travels through this module.)_\n\n");
    s.push_str("## Common modification points\n\n");
    s.push_str("_(LLM-authored: where to look when changing this module's behavior.)_\n\n");
    s.push_str("## Risk points\n\n");
    s.push_str("_(LLM-authored: gotchas, edge cases, and failure modes.)_\n\n");
    s
}

/// The `index.md` body: generated header, overview counts, reading order.
fn render_okf_index(snap: &QuerySnapshot, name: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!("# {name} — repo wiki\n\n"));
    s.push_str("> Generated by `loom wiki` — do not edit the manifest (frontmatter) by hand.\n");
    s.push_str("> Regenerate after graph changes; `loom wiki --check` verifies freshness.\n");
    s.push_str("> The graph is the source of truth; this bundle is a projection of it.\n");
    s.push_str(
        "> Companion: `docs/repo-wiki-ladder-proposal.md` — the v2 code-primary design.\n\n",
    );

    render_overview(&mut s, snap);

    // Count module pages.
    let module_count = snap
        .intents
        .iter()
        .filter(|i| i.abstraction_level == "component" && i.status != "deprecated")
        .count();

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
    if module_count > 0 {
        s.push_str(&format!(
            "7. [Module pages](modules/) — one deep-dive per component ({module_count} pages).\n"
        ));
    }
    s.push('\n');

    s.push_str("## Prose layer\n\n");
    s.push_str("Each page carries LLM-authored narrative prose between two HTML-comment\n");
    s.push_str("sentinel lines (loom:prose-start / loom:prose-end). It teaches *how the system\n");
    s.push_str(
        "fits together* — the cognitive guide the manifest's flat listings cannot convey.\n",
    );
    s.push_str("Certified by `loom wiki --prose-check` (coverage + freshness + consistency);\n");
    s.push_str("prose quality is human-gated, never machine-green.\n\n");
    s.push_str("**Permitted in prose:** markdown, fenced code blocks, and mermaid diagrams\n");
    s.push_str("(renderable art — their bracket syntax is not read as cross-links). Cite\n");
    s.push_str("codefiles via `[`path`](../src/path)` — every citation is mechanically checked\n");
    s.push_str("against the graph. Do NOT use `intent:UUID` links — the reader's vocabulary\n");
    s.push_str("is the codebase's, not the graph's.\n\n");
    s.push('\n');

    s
}

/// Render the decisions page: every `kind=decision` note, newest first.
fn render_decisions(s: &mut String, snap: &QuerySnapshot, notes: &[Note]) {
    s.push_str("## Design decisions\n\n");
    s.push_str("Rationale recorded via `loom note add --kind decision`. Newest first.\n\n");
    if notes.is_empty() {
        s.push_str("_(no decision notes recorded yet)_\n\n");
        return;
    }
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

/// Render the glossary: collect unique vocab terms across all intents' tags.
fn render_glossary(s: &mut String, snap: &QuerySnapshot) {
    s.push_str("## Glossary\n\n");
    s.push_str("The bounded `loom vocab` registry. Each term lists the intents that carry it.\n\n");
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

/// Render the flows page: every user_visible intent with its saga proofs.
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
    let val_by_id: HashMap<&str, &crate::types::Validation> = snap
        .validations
        .iter()
        .map(|v| (v.id.as_str(), v))
        .collect();
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
        let saga_vals: Vec<&crate::types::Validation> = validates_by_intent
            .get(j.id.as_str())
            .map(|ids| {
                ids.iter()
                    .filter_map(|vid| {
                        val_by_id
                            .get(vid)
                            .copied()
                            .filter(|v| v.validation_type == "saga")
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

/// Render one page with an optional preserved prose body. The manifest
/// (frontmatter including sourceFiles, symbols, provenance stamp) is
/// byte-stable; the prose between sentinels is LLM-owned and not byte-checked.
fn render_okf_page_with_prose(
    page: &OkfPage,
    preserved_prose: &str,
    codefile_hashes: &HashMap<&str, &str>,
) -> String {
    let mut s = String::new();
    // --- Frontmatter (the manifest layer) ---
    s.push_str("---\n");
    s.push_str(&format!("type: {}\n", page.okf_type));
    s.push_str(&format!("title: {:?}\n", page.title));
    if !page.tags.is_empty() {
        s.push_str("tags:\n");
        for t in &page.tags {
            s.push_str(&format!("  - {}\n", t));
        }
    }
    if !page.source_files.is_empty() {
        s.push_str("sourceFiles:\n");
        for f in &page.source_files {
            s.push_str(&format!("  - {}\n", f));
        }
    }
    if !page.symbols.is_empty() {
        s.push_str("symbols:\n");
        for sym in &page.symbols {
            s.push_str(&format!("  - {}\n", sym));
        }
    }
    // Provenance stamp: file path → content hash. Byte-stable (sorted by path).
    if !page.source_files.is_empty() {
        s.push_str("provenance:\n");
        for f in &page.source_files {
            let hash = codefile_hashes.get(f.as_str()).copied().unwrap_or("");
            if !hash.is_empty() {
                s.push_str(&format!("  {}: {}\n", f, hash));
            }
        }
    }
    s.push_str("---\n\n");

    // --- Body (graph-derived, byte-stable) ---
    s.push_str(&page.body);
    if !page.body.ends_with('\n') {
        s.push('\n');
    }
    s.push('\n');

    // --- Prose sentinels (LLM-owned region) ---
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
    let start = content
        .lines()
        .position(|line| line.trim() == PROSE_START)?;
    let byte_offset = content
        .lines()
        .take(start)
        .map(|l| l.len() + 1)
        .sum::<usize>();
    let after_start = &content[byte_offset + PROSE_START.len()..];
    let end = after_start
        .lines()
        .position(|line| line.trim() == PROSE_END)?;
    let prose_end_byte = after_start
        .lines()
        .take(end)
        .map(|l| l.len() + 1)
        .sum::<usize>();
    Some(after_start[..prose_end_byte].to_string())
}

/// The skeleton prefix of a page: frontmatter + body + PROSE_START line.
/// This is the byte-stable portion `--check` compares.
fn skeleton_prefix(page: &OkfPage, codefile_hashes: &HashMap<&str, &str>) -> String {
    let full = render_okf_page_with_prose(page, "", codefile_hashes);
    // Everything up to and including the PROSE_START line.
    if let Some(pos) = full.find(PROSE_START) {
        full[..pos + PROSE_START.len() + 1].to_string() // +1 for the trailing \n
    } else {
        full
    }
}

/// Write (or `--check`) the bundle directory. Deterministic bytes, atomic-ish
/// writes, same exit semantics as `loom export`. For `--check`, every file's
/// manifest prefix must match byte-for-byte; prose between sentinels is excluded.
fn emit_okf(
    root: &Path,
    out: &str,
    check: bool,
    pages: &[OkfPage],
    codefile_hashes: &HashMap<&str, &str>,
    printer: &Printer,
) -> Result<()> {
    if out == "-" {
        anyhow::bail!("wiki emits a directory bundle; cannot write to '-'. Drop '-' to use the default `loom.wiki/`.");
    }
    let confined =
        crate::repo::confine(root, Path::new(out)).ok_or_else(|| path_escape_error(out))?;
    let bundle = root.join(confined);

    if check {
        let mut stale: Vec<String> = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        for page in pages {
            let path = bundle.join(&page.rel_path);
            let on_disk = fs::read_to_string(&path).ok();
            let expected_prefix = skeleton_prefix(page, codefile_hashes);
            match on_disk {
                None => missing.push(page.rel_path.clone()),
                Some(content) => {
                    let sentinel_line = content.lines().position(|l| l.trim() == PROSE_START);
                    let disk_prefix = if let Some(line_no) = sentinel_line {
                        let byte_off = content
                            .lines()
                            .take(line_no)
                            .map(|l| l.len() + 1)
                            .sum::<usize>();
                        &content[..byte_off + PROSE_START.len() + 1] // +1 for the trailing \n (matches skeleton_prefix)
                    } else {
                        content.as_str()
                    };
                    if disk_prefix != expected_prefix {
                        stale.push(page.rel_path.clone());
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
                "next_step": if fresh { format!("commit {out}") } else { format!("run `loom wiki` and commit {out}") },
            }));
        } else if fresh {
            println!(
                "{}",
                crate::output::up_to_date_line(format!("{out} ({}) ", pages.len()))
            );
        } else {
            for m in &missing {
                println!(
                    "✗ {}/{} does not exist — run `loom wiki` and commit it.",
                    out.trim_end_matches('/'),
                    m
                );
            }
            for s_file in &stale {
                println!(
                    "✗ {}/{} is STALE — the graph changed since it was written. Run `loom wiki`.",
                    out.trim_end_matches('/'),
                    s_file
                );
            }
        }
        if !fresh {
            anyhow::bail!(
                "wiki bundle is stale or missing — run `loom wiki` and commit the result."
            );
        }
        return Ok(());
    }

    fs::create_dir_all(&bundle)?;
    let mut written = 0usize;
    for page in pages {
        let path = bundle.join(&page.rel_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Preserve existing LLM-authored prose between the sentinels.
        let preserved_prose = fs::read_to_string(&path)
            .ok()
            .and_then(|c| extract_prose(&c))
            .unwrap_or_default();
        let md = render_okf_page_with_prose(page, &preserved_prose, codefile_hashes);
        let mut tmp = path.as_os_str().to_os_string();
        tmp.push(".tmp");
        fs::write(&tmp, &md)?;
        fs::rename(&tmp, &path)?;
        written += 1;
    }
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "out": out,
            "files": written,
            "next_step": commit_step(out),
        }));
    } else {
        println!("✓ Wrote {out}  ({written} files)");
        println!("  → It's a projection — regenerate after graph changes; `loom wiki --check` guards freshness.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Manifest resolver — file-path → intent at check time (invisible to readers).
// ---------------------------------------------------------------------------

/// Resolve a registered codefile path to the intent id(s) that IMPLEMENT it.
pub fn resolve_file_to_intent_ids(snap: &QuerySnapshot, path: &str) -> Vec<String> {
    let mut ids: Vec<String> = snap
        .implements
        .iter()
        .filter(|im| im.codefile_path == path)
        .map(|im| im.intent_id.clone())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// True when the graph records a non-`independent` RELATES_TO edge between two intents.
fn intents_coupled_in_graph(snap: &QuerySnapshot, a: &str, b: &str) -> bool {
    snap.relates.iter().any(|e| {
        e.inspection_status != "independent"
            && ((e.from_id == a && e.to_id == b) || (e.from_id == b && e.to_id == a))
    })
}

const RELATION_WORDS: &[&str] = &[
    " calls ",
    " imports ",
    " depends on ",
    " uses ",
    " invokes ",
];

fn paragraph_has_relational_claim(para: &str) -> bool {
    let lower = para.to_ascii_lowercase();
    RELATION_WORDS.iter().any(|w| lower.contains(w))
}

// ---------------------------------------------------------------------------
// Prose-check — certify the LLM-authored prose layer by mechanical roll-up.
//
// Three gates from docs/repo-wiki-ladder-proposal.md §Falsifiability. None of
// them trust prose quality — that stays human-gated. They only check that the
// prose is GROUNDED in the graph the way the manifest is: every salient node
// is covered, every citation resolves, every file is fresh.
// ---------------------------------------------------------------------------

/// A cited codefile extracted from a prose body via its markdown links.
/// v2: only file-path citations (no intent:UUID in reader-facing prose).
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProseCitation {
    /// A link to a source file, e.g. `[x](../src/foo.rs)`. Resolves to a
    /// registered CodeFile path.
    Codefile { path: String },
}

/// Heuristic: does this markdown link target look like a path to a source
/// file (vs an intra-wiki link, a directory, or an external URL)?
fn looks_like_codefile(target: &str) -> bool {
    if target.is_empty() || target.starts_with("http") || target.starts_with("#") {
        return false;
    }
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
    let under_source_tree = target.contains("/src/") || target.contains("/tests/");
    is_source_ext || under_source_tree
}

/// Extract markdown-link citations from a prose body. Walks for `[label](target)`
/// — simple state machine, no regex dependency. Skips fenced code blocks.
fn extract_prose_citations(prose: &str) -> Vec<ProseCitation> {
    let mut out = Vec::new();
    let bytes = prose.as_bytes();
    let mut i = 0;
    let mut in_fence = false;
    let mut fence_marker_len = 0;
    while i < bytes.len() {
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
                if let Some(stripped) = after.strip_prefix('(') {
                    if let Some(close_paren) = stripped.find(')') {
                        let target = &stripped[..close_paren];
                        let target = target.split_whitespace().next().unwrap_or(target);
                        let is_dir = target.ends_with('/');
                        let is_intra_wiki_md = target.ends_with(".md")
                            && !target.starts_with("../")
                            && !target.contains("/src/");
                        // v2: no intent:UUID extraction. Only file-path citations.
                        if !is_dir && !is_intra_wiki_md && looks_like_codefile(target) {
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
    out.sort_by(|a, b| match (a, b) {
        (ProseCitation::Codefile { path: pa }, ProseCitation::Codefile { path: pb }) => pa.cmp(pb),
    });
    out.dedup();
    out
}

/// The set of salient graph nodes that OWE coverage in some wiki page
/// (proposal §Coverage). Altitude-calibrated: system + component intents,
/// user_visible intents (journeys). Leaf feature intents are NOT individually
/// owed — they are reached through their component's module page.
fn salient_intents(snap: &QuerySnapshot) -> Vec<&Intent> {
    snap.intents
        .iter()
        .filter(|i| {
            i.abstraction_level == "system"
                || i.abstraction_level == "component"
                || i.visibility == "user_visible"
        })
        .filter(|i| i.status != "deprecated")
        .collect()
}

/// A single prose-check finding (a gate failure).
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
    let confined =
        crate::repo::confine(root, Path::new(out)).ok_or_else(|| path_escape_error(out))?;
    let bundle = root.join(confined);

    let codefile_paths: HashSet<&str> = snap.codefiles.iter().map(|c| c.path.as_str()).collect();

    // Build: intent_id → its IMPLEMENTS file paths (for coverage gate).
    let mut files_of_intent: HashMap<&str, Vec<&str>> = HashMap::new();
    for im in &snap.implements {
        files_of_intent
            .entry(im.intent_id.as_str())
            .or_default()
            .push(im.codefile_path.as_str());
    }
    for v in files_of_intent.values_mut() {
        v.sort_unstable();
        v.dedup();
    }

    // Collect all sourceFiles across all pages (union, for the coverage gate).
    let mut all_cited_files: HashSet<String> = HashSet::new();

    for page in pages {
        let path = bundle.join(&page.rel_path);
        let on_disk = fs::read_to_string(&path).ok();
        let prose = on_disk
            .as_ref()
            .and_then(|c| extract_prose(c))
            .unwrap_or_default();

        // Gate: prose-empty — a page with no prose body between the sentinels.
        // Skip decision pages (pure projection, no narrative obligation) and
        // module pages with no sourceFiles (a component with no grounded code).
        let skip_empty = page.okf_type == "decision";
        if prose.trim().is_empty() && !skip_empty {
            findings.push(ProseFinding {
                gate: "prose-empty",
                page: page.rel_path.clone(),
                finding: "no prose between the sentinels — this page has no narrative".to_string(),
                remedy: format!(
                    "author narrative prose on `{}` between the `<!-- loom:prose-start -->` and `<!-- loom:prose-end -->` sentinels; cite the codefiles it explains via `[`path`](../src/path)`",
                    page.rel_path
                ),
            });
        }

        // Collect sourceFiles from frontmatter (the manifest layer).
        for f in &page.source_files {
            all_cited_files.insert(f.clone());
        }

        // Gate: consistency — every sourceFiles entry resolves to a registered CodeFile
        // and, via the manifest resolver, to at least one grounded intent.
        for f in &page.source_files {
            if !codefile_paths.contains(f.as_str()) {
                findings.push(ProseFinding {
                    gate: "consistency",
                    page: page.rel_path.clone(),
                    finding: format!(
                        "sourceFiles entry `{f}` is not a registered CodeFile"
                    ),
                    remedy: format!(
                        "fix the sourceFiles in `{}` frontmatter, or register the file via `loom codefile add` if it is real",
                        page.rel_path
                    ),
                });
            } else if resolve_file_to_intent_ids(snap, f).is_empty() {
                findings.push(ProseFinding {
                    gate: "consistency",
                    page: page.rel_path.clone(),
                    finding: format!(
                        "sourceFiles entry `{f}` is registered but ungrounded — no IMPLEMENTS edge maps it to an intent"
                    ),
                    remedy: format!(
                        "ground `{f}` with `loom edge implement <intent> {f} --locator \"<symbol>\"`, or remove it from `{}` sourceFiles",
                        page.rel_path
                    ),
                });
            }
        }

        // Gate: consistency — every codefile link in prose resolves.
        let citations = extract_prose_citations(&prose);
        for cit in &citations {
            match cit {
                ProseCitation::Codefile { path } => {
                    all_cited_files.insert(path.clone());
                    if !codefile_paths.contains(path.as_str()) {
                        findings.push(ProseFinding {
                            gate: "consistency",
                            page: page.rel_path.clone(),
                            finding: format!(
                                "prose links to file `{path}` which is not a registered CodeFile"
                            ),
                            remedy: format!(
                                "fix the link on `{}`, or register the file via `loom codefile add` if it is real",
                                page.rel_path
                            ),
                        });
                    }
                }
            }
        }

        // Gate: fabricated-relationship — relational prose between two cited files
        // must be backed by a non-independent RELATES_TO edge between their intents.
        for para in prose.split("\n\n") {
            if !paragraph_has_relational_claim(para) {
                continue;
            }
            let para_citations = extract_prose_citations(para);
            if para_citations.len() < 2 {
                continue;
            }
            for i in 0..para_citations.len() {
                for j in (i + 1)..para_citations.len() {
                    let (pa, pb) = match (&para_citations[i], &para_citations[j]) {
                        (
                            ProseCitation::Codefile { path: pa },
                            ProseCitation::Codefile { path: pb },
                        ) => (pa.as_str(), pb.as_str()),
                    };
                    let ia = resolve_file_to_intent_ids(snap, pa);
                    let ib = resolve_file_to_intent_ids(snap, pb);
                    if ia.is_empty() || ib.is_empty() {
                        continue;
                    }
                    let coupled = ia.iter().any(|a| {
                        ib.iter()
                            .any(|b| intents_coupled_in_graph(snap, a.as_str(), b.as_str()))
                    });
                    if !coupled {
                        findings.push(ProseFinding {
                            gate: "fabricated-relationship",
                            page: page.rel_path.clone(),
                            finding: format!(
                                "prose claims a relationship between `{pa}` and `{pb}` but no backing RELATES_TO edge exists between their grounded intents"
                            ),
                            remedy: format!(
                                "fix the prose on `{}`, or record the coupling with `loom edge explore <intent-a> <intent-b> ground`",
                                page.rel_path
                            ),
                        });
                    }
                }
            }
        }
    }

    // Gate: coverage — every salient intent with grounded files must have at
    // least one of its files appear in some page's sourceFiles or prose links.
    let salient = salient_intents(snap);
    for intent in &salient {
        let its_files = files_of_intent.get(intent.id.as_str());
        if let Some(files) = its_files {
            // Does any of this intent's files appear in the union?
            let covered = files.iter().any(|f| all_cited_files.contains(*f));
            if !covered && !files.is_empty() {
                findings.push(ProseFinding {
                    gate: "coverage",
                    page: "(bundle)".to_string(),
                    finding: format!(
                        "salient intent `{}` ({}) has grounded files but none appear in any page's sourceFiles or prose",
                        intent.name, intent.id
                    ),
                    remedy: format!(
                        "cite one of {} in a page's sourceFiles or prose (e.g. the module page `modules/{}.md`)",
                        files.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", "),
                        slugify(&intent.name)
                    ),
                });
            }
        }
        // Intents with NO grounded files are vacuously covered (nothing to cite).
    }

    Ok(findings)
}

/// `loom wiki --prose-check` entry point. Runs the three mechanical gates
/// and reports findings; exits non-zero if any gate fails.
pub fn run_prose_check(out: &str, printer: &Printer) -> Result<()> {
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

/// Map a prose-check gate to the comprehension-queue work kind.
fn wiki_queue_kind(gate: &str) -> &'static str {
    match gate {
        "fabricated-relationship" => "fabricated-link",
        "stale-manifest" => "stale-page",
        "prose-empty" | "coverage" => "write-gap",
        _ => "write-gap",
    }
}

/// Collect manifest-stale pages (skeleton prefix drift) for the wiki lane.
fn manifest_stale_pages(
    root: &Path,
    out: &str,
    pages: &[OkfPage],
    codefile_hashes: &HashMap<&str, &str>,
) -> Result<Vec<ProseFinding>> {
    let mut findings = Vec::new();
    let confined =
        crate::repo::confine(root, Path::new(out)).ok_or_else(|| path_escape_error(out))?;
    let bundle = root.join(confined);
    for page in pages {
        let path = bundle.join(&page.rel_path);
        let on_disk = fs::read_to_string(&path).ok();
        let expected_prefix = skeleton_prefix(page, codefile_hashes);
        if let Some(content) = on_disk {
            let sentinel_line = content.lines().position(|l| l.trim() == PROSE_START);
            let disk_prefix = if let Some(line_no) = sentinel_line {
                let byte_off = content
                    .lines()
                    .take(line_no)
                    .map(|l| l.len() + 1)
                    .sum::<usize>();
                &content[..byte_off + PROSE_START.len() + 1]
            } else {
                content.as_str()
            };
            if disk_prefix != expected_prefix {
                findings.push(ProseFinding {
                    gate: "stale-manifest",
                    page: page.rel_path.clone(),
                    finding: format!(
                        "manifest provenance for `{}` no longer matches the live graph",
                        page.rel_path
                    ),
                    remedy: format!(
                        "re-read the grounded files and rewrite `{}`, then run `loom wiki` to refresh the manifest",
                        page.rel_path
                    ),
                });
            }
        }
    }
    Ok(findings)
}

/// `loom next --mode wiki` — drain the comprehension queue one finding at a time.
pub fn run_next_wiki(out: &str, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
    let snap = store.query_snapshot()?;
    let gs = store.graph_state(&snap)?;
    let meta = store.graph_meta()?;
    let decision_notes = store.list_notes(None, Some("decision")).unwrap_or_default();
    let pages = render_okf_bundle(&snap, meta.as_ref(), &decision_notes);
    let codefile_hashes: HashMap<&str, &str> = snap
        .codefiles
        .iter()
        .map(|c| (c.path.as_str(), c.content_hash.as_str()))
        .collect();

    let mut findings = prose_check(&cwd, out, &pages, &snap)?;
    findings.extend(manifest_stale_pages(&cwd, out, &pages, &codefile_hashes)?);

    // Priority: fabricated-link > stale-page > write-gap (coverage before prose-empty).
    fn rank(gate: &str) -> u8 {
        match gate {
            "fabricated-relationship" => 0,
            "stale-manifest" => 1,
            "coverage" => 2,
            "consistency" => 3,
            "prose-empty" => 4,
            _ => 5,
        }
    }
    findings.sort_by(|a, b| {
        rank(a.gate)
            .cmp(&rank(b.gate))
            .then_with(|| a.page.cmp(&b.page))
            .then_with(|| a.finding.cmp(&b.finding))
    });

    if findings.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty",
                "mode": "wiki",
                "message": "Comprehension queue is empty — prose layer mechanically green.",
                "next_step": gs.next_action,
                "graph_state": crate::output::pulse_json(&gs),
            }));
        } else {
            println!("✓ Comprehension queue empty — prose layer mechanically green.");
            println!();
            println!("  {}", crate::output::fmt_pulse(&gs));
        }
        return Ok(());
    }

    let f = &findings[0];
    let queue_kind = wiki_queue_kind(f.gate);
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "mode": "wiki",
            "queue_kind": queue_kind,
            "gate": f.gate,
            "page": f.page,
            "finding": f.finding,
            "remedy": f.remedy,
            "queue_total": findings.len(),
            "next_step": f.remedy,
            "graph_state": crate::output::pulse_json(&gs),
        }));
    } else {
        println!(
            "── Next Wiki Item  [{queue_kind} — {} remaining] ──",
            findings.len()
        );
        println!();
        println!("  Page:    {}", f.page);
        println!("  Gate:    {}", f.gate);
        println!("  Finding: {}", f.finding);
        println!();
        println!("  → {}", f.remedy);
        println!();
        println!("  {}", crate::output::fmt_pulse(&gs));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering — deterministic (everything sorted, no timestamps).
// ---------------------------------------------------------------------------

const NO_DOMAIN: &str = "(uncategorized)";

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
