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
    get_vocab_term, insert_vocab_term, list_active_intents, list_vocab_terms,
    merge_vocab_terms, nearest_terms, normalize_term, tag_counts, terms_look_alike,
    MAX_TAGS_PER_INTENT,
};
use crate::db::schema::role;
use crate::db::{ensure_initialized, GrafeoDb, LoomDb};
use crate::gate;
use crate::output::Printer;
use crate::types::VocabTerm;

/// Past this size the registry stops doing its job: an agent that cannot hold
/// the whole list in context at the moment of choice falls back to guessing,
/// and guessed tags don't collide.
const REGISTRY_SOFT_CAP: usize = 75;

pub fn run(cmd: VocabCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    match cmd {
        VocabCmd::Add { term, why, author } => {
            let agent = gate::acting_in_lane("register a vocab term", &[role::BUILDER], author.as_deref())?;
            let term = normalize_term(&term)?;
            gate::require_substantive(
                "why",
                &why,
                "what the term covers AND what it does not (name the neighbouring term)",
            )?;
            if let Some(existing) = get_vocab_term(&db, &term)? {
                anyhow::bail!(
                    "Term '{}' is already registered: \"{}\"\n\
                     Use it directly: loom intent tag add <intent> {}",
                    term, existing.description, term
                );
            }
            // A new term that reads like an existing one is drift at the door.
            // No force flag: if it is genuinely distinct, a name that doesn't
            // read like the neighbour is strictly better — keys must be
            // distinguishable to be worth colliding on.
            let terms = list_vocab_terms(&db)?;
            if let Some(twin) = terms.iter().find(|t| terms_look_alike(&term, &t.name)) {
                anyhow::bail!(
                    "'{}' reads like the registered term '{}' (\"{}\").\n\
                     Either use '{}', or pick a name that doesn't look like it — \
                     synonym terms split the keyspace and intents stop colliding.",
                    term, twin.name, twin.description, twin.name
                );
            }
            let vt = VocabTerm {
                id:          Uuid::new_v4().to_string(),
                name:        term.clone(),
                description: why,
                author:      agent,
                created_at:  chrono::Utc::now().to_rfc3339(),
            };
            insert_vocab_term(&db, &vt)?;
            let size = terms.len() + 1;
            if printer.json {
                let mut v = serde_json::to_value(&vt)?;
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("registry_size".into(), serde_json::json!(size));
                    obj.insert("next_step".into(), serde_json::json!(format!(
                        "Tag intents with it: loom intent tag add <intent> {}", vt.name
                    )));
                    if size > REGISTRY_SOFT_CAP {
                        obj.insert("warning".into(), serde_json::json!(format!(
                            "registry has {size} terms — past ~{REGISTRY_SOFT_CAP} agents can't hold it in context and stop colliding; run `loom smells` (vocab_drift) and merge"
                        )));
                    }
                }
                printer.print_json(&v);
            } else {
                println!("✓ Vocab term registered: {}", vt.name);
                println!("  \"{}\"", vt.description);
                println!("  → Tag intents with it: loom intent tag add <intent> {}", vt.name);
                if size > REGISTRY_SOFT_CAP {
                    println!(
                        "  ⚠ registry now holds {size} terms — past ~{REGISTRY_SOFT_CAP} the list stops fitting in an \
                         agent's working context and collisions die. Check `loom smells` for vocab_drift and merge."
                    );
                }
            }
        }

        VocabCmd::List => {
            let terms = list_vocab_terms(&db)?;
            let counts = tag_counts(&list_active_intents(&db)?)?;
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
                printer.print_json(&items);
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
        }

        VocabCmd::Merge { from, to } => {
            gate::acting_in_lane("merge vocab terms", &[role::BUILDER], None)?;
            let from = normalize_term(&from)?;
            let to = normalize_term(&to)?;
            if from == to {
                anyhow::bail!("'{from}' and '{to}' are the same term — pick two distinct terms (`loom vocab list`).");
            }
            if get_vocab_term(&db, &from)?.is_none() {
                anyhow::bail!("Term '{from}' is not registered — `loom vocab list` shows the registry.");
            }
            if get_vocab_term(&db, &to)?.is_none() {
                anyhow::bail!(
                    "Target term '{to}' is not registered — merge dissolves '{from}' INTO an existing term; register '{to}' first if it should exist."
                );
            }
            let now = chrono::Utc::now().to_rfc3339();
            let retagged = merge_vocab_terms(&db, &from, &to, &now)?;
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "from": from, "to": to, "retagged_intents": retagged,
                    "next_step": "re-check duplicates: `loom smells`",
                }));
            } else {
                println!("✓ '{from}' merged into '{to}' — {retagged} intent(s) retagged, '{from}' deleted.");
                println!("  → Next: re-check duplicates: `loom smells`");
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The shared write-time gate for tags (intent add --tag / intent tag add)
// ---------------------------------------------------------------------------

/// Normalize + validate a set of tags against the registry. On an unknown
/// term the error IS the affordance: nearest matches with definitions, then
/// the whole registry inline — the agent at the keyboard sees the full menu at
/// the moment of choice instead of being sent to a docs page.
pub fn validate_tags(db: &dyn LoomDb, raw: &[String]) -> Result<Vec<String>> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let mut tags = raw.iter().map(|t| normalize_term(t)).collect::<Result<Vec<_>>>()?;
    tags.sort();
    tags.dedup();
    if tags.len() > MAX_TAGS_PER_INTENT {
        anyhow::bail!(
            "{} tags — the cap is {MAX_TAGS_PER_INTENT}. Keep the most specific ones: \
             broad tags collide with everything and say nothing.",
            tags.len()
        );
    }
    let terms = list_vocab_terms(db)?;
    let counts = tag_counts(&list_active_intents(db)?)?;
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
        let ranked = nearest_terms(t, &terms, &counts);
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
        let more = if elided > 0 { format!(" +{elided} more") } else { String::new() };
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
    use super::validate_tags;
    use crate::db::queries::{insert_vocab_term, set_intent_tags, insert_intent};
    use crate::db::GrafeoDb;
    use crate::types::{Intent, VocabTerm};

    fn db_with_registry() -> GrafeoDb {
        let db = GrafeoDb::in_memory();
        for (name, desc) in [("authz", "permission checks"), ("retry", "re-attempts"), ("cache", "derived data")] {
            insert_vocab_term(&db, &VocabTerm {
                id: format!("vt-{name}"), name: name.into(), description: desc.into(),
                author: "llm".into(), created_at: "t".into(),
            }).unwrap();
        }
        db
    }

    #[test]
    fn accepts_known_terms_normalized_and_deduped() {
        let db = db_with_registry();
        let tags = validate_tags(&db, &[" Retry ".into(), "retry".into(), "authz".into()]).unwrap();
        assert_eq!(tags, vec!["authz".to_string(), "retry".to_string()]);
        assert!(validate_tags(&db, &[]).unwrap().is_empty(), "untagged is always valid");
    }

    #[test]
    fn unknown_term_error_is_the_affordance() {
        let db = db_with_registry();
        // Make usage counts visible in the inline registry.
        insert_intent(&db, &Intent {
            id: "i0".into(), name: "n".into(), description: "d".into(),
            abstraction_level: "feature".into(), domain: "d".into(), source_refs: "[]".into(),
            status: "proposed".into(), aspect: String::new(), tags: "[]".into(),
            lifecycle: "implemented".into(), created_at: "t".into(), updated_at: "t".into(),
        }).unwrap();
        set_intent_tags(&db, "i0", vec!["retry".into()], "t").unwrap();

        let err = validate_tags(&db, &["retrying".into()]).unwrap_err().to_string();
        assert!(err.contains("retry (1 intent(s))"), "nearest with usage:\n{err}");
        assert!(err.contains("registry (3 terms)"), "full menu inline:\n{err}");
        assert!(err.contains("loom vocab add retrying"), "registration path:\n{err}");
        assert!(err.contains("optional"), "abstaining must stay legitimate:\n{err}");
    }

    #[test]
    fn cap_is_enforced_with_teaching_error() {
        let db = db_with_registry();
        let err = validate_tags(
            &db,
            &["authz".into(), "retry".into(), "cache".into(), "extra".into()],
        ).unwrap_err().to_string();
        assert!(err.contains("cap is 3"), "{err}");
    }
}
