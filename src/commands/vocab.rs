//! `loom vocab` — the bounded tag vocabulary.
//!
//! The interaction contract that makes tags work for an LLM driver:
//! 1. The registry is INLINED at every decision point (errors carry the full
//!    list) — LLMs pick well from a presented list and guess badly from an
//!    unseen one. This is also why the registry must stay small.
//! 2. Tags are OPTIONAL — an untagged intent is honest, a wrong tag lies.
//! 3. Drift is detected (`vocab_drift` smell) and converged (`merge`),
//!    never prevented by a closed list.

use anyhow::Result;
use uuid::Uuid;

use crate::cli::VocabCmd;
use crate::db::queries::{
    nearest_terms, normalize_term, parse_tags, suggest_vocab_terms, tag_counts, terms_look_alike,
    MAX_TAGS_PER_INTENT,
};
use crate::db::schema::role;
use crate::db::{ensure_initialized, GraphReadHandle, GraphReadRepository};
use crate::gate;
use crate::output::Printer;
use crate::types::{Intent, VocabTerm};

/// Past this size the registry stops doing its job: an agent that cannot hold
/// the whole list in context at the moment of choice falls back to guessing,
/// and guessed tags don't collide.
const REGISTRY_SOFT_CAP: usize = 75;

pub fn run(cmd: VocabCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    match cmd {
        VocabCmd::List => {
            let db = GraphReadHandle::open(&cwd)?;
            run_list_with_db(&db, printer)
        }
        VocabCmd::Suggest { limit } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_suggest_with_db(&db, limit, printer)
        }
        VocabCmd::Add { term, why, author } => {
            run_add_with_sqlite(&cwd, term, why, author, printer)
        }
        VocabCmd::Merge { from, to } => run_merge_with_sqlite(&cwd, from, to, printer),
    }
}

fn run_list_with_db(db: &dyn GraphReadRepository, printer: &Printer) -> Result<()> {
    let terms = db.list_vocab_terms()?;
    let active_intents: Vec<_> = db
        .list_intents(None, None)?
        .into_iter()
        .filter(|intent| intent.status != "deprecated")
        .collect();
    let counts = tag_counts(&active_intents)?;
    if printer.json {
        let items: Vec<serde_json::Value> = terms
            .iter()
            .map(|t| {
                serde_json::json!({
                    "term": t.name,
                    "description": t.description,
                    "intents": counts.get(&t.name).copied().unwrap_or(0),
                    "author": t.author,
                    "created_at": t.created_at,
                })
            })
            .collect();
        printer.print_json(&serde_json::json!({
            "terms": items,
            "total": items.len(),
            "truncated": false,
        }));
    } else if terms.is_empty() {
        println!("(empty registry — tags are optional, but registered terms let duplicate-responsibility detection see across unrelated files)");
        println!("  → loom vocab add <term> --why \"<what it covers; what it does NOT — name the neighbour>\"");
    } else {
        for t in &terms {
            let n = counts.get(&t.name).copied().unwrap_or(0);
            println!("  {:<24} {:>3} intent(s)  — {}", t.name, n, t.description);
        }
        println!("\n  tag an intent: loom intent tag add <intent> <term>   (max {MAX_TAGS_PER_INTENT}, pick the most specific)");
    }
    Ok(())
}

fn run_suggest_with_db(
    db: &dyn GraphReadRepository,
    limit: usize,
    printer: &Printer,
) -> Result<()> {
    // Read-only (no lane gate): mine THIS graph for candidate keys.
    let snapshot = db.query_snapshot()?;
    let registered: std::collections::HashSet<String> =
        db.list_vocab_terms()?.into_iter().map(|t| t.name).collect();
    let mut suggestions = suggest_vocab_terms(&snapshot.intents, &registered, 2);
    let total = crate::output::apply_limit(&mut suggestions, limit);

    // Coverage context — "is the duplicate detector armed?": coded
    // intents (≥1 IMPLEMENTS) and how many carry ≥1 tag.
    let coded: Vec<&crate::types::Intent> = snapshot
        .intents
        .iter()
        .filter(|i| snapshot.with_code.contains(&i.id))
        .collect();
    let coded_count = coded.len();
    let tagged_count = coded
        .iter()
        .filter(|i| parse_tags(i).map(|t| !t.is_empty()).unwrap_or(false))
        .count();
    let armed_note = if coded_count == 0 {
        String::new()
    } else if tagged_count == 0 {
        format!("0 of {coded_count} coded intent(s) tagged — duplicate detection is UNARMED (lexical fallback only); `loom smells` shows it")
    } else {
        format!("{tagged_count} of {coded_count} coded intent(s) tagged — tag more to strengthen duplicate detection (`loom smells`)")
    };

    if printer.json {
        let items: Vec<serde_json::Value> = suggestions
            .iter()
            .map(|s| {
                serde_json::json!({
                    "term": s.term,
                    "intents": s.intent_count,
                    "examples": s.examples,
                })
            })
            .collect();
        printer.print_json(&serde_json::json!({
            "suggestions": items,
            "total": total,
            "truncated": total > suggestions.len(),
            "coded_intents": coded_count,
            "tagged_coded_intents": tagged_count,
            "next_step": "register the ones that name a real shared responsibility: `loom vocab add <term> --why \"<what it covers; what it does NOT — name the neighbour>\"`, then `loom intent tag add <intent> <term>`; re-run `loom smells`",
        }));
    } else if suggestions.is_empty() {
        println!(
            "(no recurring terms found — too few or too distinct intents; tags stay optional)"
        );
        if !armed_note.is_empty() {
            println!("  {armed_note}");
        }
    } else {
        println!("Candidate vocabulary terms — mined from THIS graph's intents, ranked by how many share each (collision potential):\n");
        println!("  {:<22} {:>7}  examples", "term", "intents");
        for s in &suggestions {
            println!(
                "  {:<22} {:>7}  {}",
                s.term,
                s.intent_count,
                s.examples.join(", ")
            );
        }
        if let Some(m) =
            crate::output::more_marker(total, suggestions.len(), "loom vocab suggest --limit 0")
        {
            println!("  {m}");
        }
        println!();
        if !armed_note.is_empty() {
            println!("  {armed_note}");
        }
        println!("  → register one: loom vocab add <term> --why \"<what it covers; what it does NOT — name the neighbour>\", then loom intent tag add <intent> <term>");
    }
    Ok(())
}

fn prepare_add_term(
    term: String,
    why: String,
    author: Option<String>,
    terms: &[VocabTerm],
) -> Result<(VocabTerm, usize)> {
    let agent = gate::acting_in_lane("register a vocab term", &[role::BUILDER], author.as_deref())?;
    let term = normalize_term(&term)?;
    gate::require_substantive(
        "why",
        &why,
        "what the term covers AND what it does not (name the neighbouring term)",
    )?;
    if let Some(existing) = terms.iter().find(|existing| existing.name == term) {
        anyhow::bail!(
            "Term '{}' is already registered: \"{}\"\n\
             Use it directly: loom intent tag add <intent> {}",
            term,
            existing.description,
            term
        );
    }
    // A new term that reads like an existing one is drift at the door.
    // No force flag: if it is genuinely distinct, a name that doesn't
    // read like the neighbour is strictly better — keys must be
    // distinguishable to be worth colliding on.
    if let Some(twin) = terms
        .iter()
        .find(|existing| terms_look_alike(&term, &existing.name))
    {
        anyhow::bail!(
            "'{}' reads like the registered term '{}' (\"{}\").\n\
             Either use '{}', or pick a name that doesn't look like it — \
             synonym terms split the keyspace and intents stop colliding.",
            term,
            twin.name,
            twin.description,
            twin.name
        );
    }
    let vt = VocabTerm {
        id: Uuid::new_v4().to_string(),
        name: term,
        description: why,
        author: agent,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    Ok((vt, terms.len() + 1))
}

fn print_add_result(term: &VocabTerm, size: usize, printer: &Printer) -> Result<()> {
    if printer.json {
        let mut v = serde_json::to_value(term)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("registry_size".into(), serde_json::json!(size));
            obj.insert(
                "next_step".into(),
                serde_json::json!(format!(
                    "Tag intents with it: loom intent tag add <intent> {}",
                    term.name
                )),
            );
            if size > REGISTRY_SOFT_CAP {
                obj.insert("warning".into(), serde_json::json!(format!(
                    "registry has {size} terms — past ~{REGISTRY_SOFT_CAP} agents can't hold it in context and stop colliding; run `loom smells` (vocab_drift) and merge"
                )));
            }
        }
        printer.print_json(&v);
    } else {
        println!("✓ Vocab term registered: {}", term.name);
        println!("  \"{}\"", term.description);
        println!(
            "  → Tag intents with it: loom intent tag add <intent> {}",
            term.name
        );
        if size > REGISTRY_SOFT_CAP {
            println!(
                "  ⚠ registry now holds {size} terms — past ~{REGISTRY_SOFT_CAP} the list stops fitting in an \
                 agent's working context and collisions die. Check `loom smells` for vocab_drift and merge."
            );
        }
    }
    Ok(())
}

fn run_add_with_sqlite(
    root: &std::path::Path,
    term: String,
    why: String,
    author: Option<String>,
    printer: &Printer,
) -> Result<()> {
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let terms = store.list_vocab_terms()?;
    let (term, size) = prepare_add_term(term, why, author, &terms)?;
    store.insert_vocab_term(&term)?;
    print_add_result(&term, size, printer)
}

fn prepare_merge_terms(from: &str, to: &str) -> Result<(String, String)> {
    gate::acting_in_lane("merge vocab terms", &[role::BUILDER], None)?;
    let from = normalize_term(from)?;
    let to = normalize_term(to)?;
    if from == to {
        anyhow::bail!(
            "'{from}' and '{to}' are the same term — pick two distinct terms (`loom vocab list`)."
        );
    }
    Ok((from, to))
}

fn print_merge_result(from: &str, to: &str, retagged: usize, printer: &Printer) {
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok", "from": from, "to": to, "retagged_intents": retagged,
            "next_step": "re-check duplicates: `loom smells`",
        }));
    } else {
        println!(
            "✓ '{from}' merged into '{to}' — {retagged} intent(s) retagged, '{from}' deleted."
        );
        println!("  → Next: re-check duplicates: `loom smells`");
    }
}

fn run_merge_with_sqlite(
    root: &std::path::Path,
    from: String,
    to: String,
    printer: &Printer,
) -> Result<()> {
    let (from, to) = prepare_merge_terms(&from, &to)?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let retagged = store.merge_vocab_terms(&from, &to, &now)?;
    print_merge_result(&from, &to, retagged, printer);
    Ok(())
}

pub fn validate_tags_from_registry(
    raw: &[String],
    terms: &[VocabTerm],
    active_intents: &[Intent],
) -> Result<Vec<String>> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let mut tags = raw
        .iter()
        .map(|t| normalize_term(t))
        .collect::<Result<Vec<_>>>()?;
    tags.sort();
    tags.dedup();
    if tags.len() > MAX_TAGS_PER_INTENT {
        anyhow::bail!(
            "{} tags — the cap is {MAX_TAGS_PER_INTENT}. Keep the most specific ones: \
             broad tags collide with everything and say nothing.",
            tags.len()
        );
    }
    let counts = tag_counts(active_intents)?;
    for t in &tags {
        if terms.iter().any(|v| v.name == *t) {
            continue;
        }
        if terms.is_empty() {
            anyhow::bail!(
                "No term '{t}' — the registry is empty. Register it first:\n  \
                 loom vocab add {t} --why \"<what it covers; what it does NOT — name the neighbour>\""
            );
        }
        let ranked = nearest_terms(t, terms, &counts);
        let nearest: Vec<String> = ranked
            .iter()
            .take(3)
            .map(|(v, n)| format!("  {} ({} intent(s)) — \"{}\"", v.name, n, v.description))
            .collect();
        let mut registry: Vec<String> = terms
            .iter()
            .map(|v| format!("{}({})", v.name, counts.get(&v.name).copied().unwrap_or(0)))
            .collect();
        let elided = registry.len().saturating_sub(60);
        registry.truncate(60);
        let more = if elided > 0 {
            format!(" +{elided} more")
        } else {
            String::new()
        };
        anyhow::bail!(
            "No registered term '{t}'. Nearest:\n{}\n\
             registry ({} terms): {}{}\n\
             Pick one of those, or register it: loom vocab add {t} --why \"<what it covers; what it does NOT — name the neighbour>\"\n\
             (Tagging is optional — skipping is more honest than shoehorning into a wrong term.)",
            nearest.join("\n"),
            terms.len(),
            registry.join(" "),
            more
        );
    }
    Ok(tags)
}

#[cfg(test)]
mod tests {
    use super::validate_tags_from_registry;
    use crate::types::{Intent, VocabTerm};

    fn registry() -> Vec<VocabTerm> {
        [
            ("authz", "permission checks"),
            ("retry", "re-attempts"),
            ("cache", "derived data"),
        ]
        .into_iter()
        .map(|(name, desc)| VocabTerm {
            id: format!("vt-{name}"),
            name: name.into(),
            description: desc.into(),
            author: "llm".into(),
            created_at: "t".into(),
        })
        .collect()
    }

    fn active_intents() -> Vec<Intent> {
        Vec::new()
    }

    #[test]
    fn accepts_known_terms_normalized_and_deduped() {
        let terms = registry();
        let intents = active_intents();
        let tags = validate_tags_from_registry(
            &[" Retry ".into(), "retry".into(), "authz".into()],
            &terms,
            &intents,
        )
        .unwrap();
        assert_eq!(tags, vec!["authz".to_string(), "retry".to_string()]);
        assert!(
            validate_tags_from_registry(&[], &terms, &intents)
                .unwrap()
                .is_empty(),
            "untagged is always valid"
        );
    }

    #[test]
    fn unknown_term_error_is_the_affordance() {
        let terms = registry();
        let intents = vec![Intent {
            id: "i0".into(),
            name: "n".into(),
            description: "d".into(),
            abstraction_level: "feature".into(),
            domain: "d".into(),
            source_refs: Vec::new(),
            layer: String::new(),
            status: "proposed".into(),
            aspect: String::new(),
            tags: vec!["retry".into()],
            visibility: String::new(),
            boundary: String::new(),
            lifecycle: "implemented".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
        }];

        let err = validate_tags_from_registry(&["retrying".into()], &terms, &intents)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("retry (1 intent(s))"),
            "nearest with usage:\n{err}"
        );
        assert!(
            err.contains("registry (3 terms)"),
            "full menu inline:\n{err}"
        );
        assert!(
            err.contains("loom vocab add retrying"),
            "registration path:\n{err}"
        );
        assert!(
            err.contains("optional"),
            "abstaining must stay legitimate:\n{err}"
        );
    }

    #[test]
    fn cap_is_enforced_with_teaching_error() {
        let terms = registry();
        let intents = active_intents();
        let err = validate_tags_from_registry(
            &[
                "authz".into(),
                "retry".into(),
                "cache".into(),
                "extra".into(),
            ],
            &terms,
            &intents,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("cap is 3"), "{err}");
    }
}
