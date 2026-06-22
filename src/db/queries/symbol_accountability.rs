//! Symbol accountability: a structural instrument over CodeFile.symbol_facts
//! and IMPLEMENTS locators. It does not try to prove call-graph reachability;
//! it asks whether behavior-significant syntax surfaces have an owner, an
//! accepted file-level owner, or an actionable gap.

use serde::Serialize;
use std::collections::HashMap;

use super::symbol_match::{contains_identifier_word, symbol_identifier};
use crate::types::{CodeFile, Implements, Intent, Note, SymbolFact};

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct SymbolAccountabilitySummary {
    pub total_symbols: usize,
    pub instrumented_files: usize,
    pub required: usize,
    pub grounded: usize,
    pub accepted: usize,
    pub adjudicated: usize,
    pub support: usize,
    pub test_support: usize,
    pub debris_candidates: usize,
    pub raw_actionable_gaps: usize,
    pub actionable_gaps: usize,
    pub resolved_pct: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ActionableSymbolGap {
    pub path: String,
    pub label: String,
    pub name: String,
    pub kind: String,
    pub line_start: usize,
    pub reason: String,
    pub owner_intents: Vec<String>,
    pub suggested_action: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AdjudicatedSymbolGap {
    pub path: String,
    pub label: String,
    pub name: String,
    pub kind: String,
    pub line_start: usize,
    pub reason: String,
    pub owner_intents: Vec<String>,
    pub ruling: String,
    pub ruled_by: String,
    pub ruled_at: String,
    pub reopens_when: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SymbolTeaching {
    pub principle: String,
    pub inspect: Vec<String>,
    pub avoid: Vec<String>,
    pub done_when: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SymbolAccountabilityReport {
    pub summary: SymbolAccountabilitySummary,
    pub raw_actionable_symbol_gaps: Vec<ActionableSymbolGap>,
    pub actionable_symbol_gaps: Vec<ActionableSymbolGap>,
    pub adjudicated_symbol_gaps: Vec<AdjudicatedSymbolGap>,
    pub teaching: SymbolTeaching,
}

#[derive(Debug, Clone)]
struct Owner {
    id: String,
    name: String,
    locator: String,
}

#[cfg(test)]
pub fn symbol_accountability_from_parts(
    codefiles: &[CodeFile],
    intents: &[Intent],
    implements: &[Implements],
) -> SymbolAccountabilityReport {
    symbol_accountability_from_parts_with_notes(codefiles, intents, implements, &[])
}

pub fn symbol_accountability_from_parts_with_notes(
    codefiles: &[CodeFile],
    intents: &[Intent],
    implements: &[Implements],
    notes: &[Note],
) -> SymbolAccountabilityReport {
    let intent_names: HashMap<&str, &str> = intents
        .iter()
        .map(|intent| (intent.id.as_str(), intent.name.as_str()))
        .collect();
    let mut owners_by_path: HashMap<&str, Vec<Owner>> = HashMap::new();
    for im in implements {
        let Some(name) = intent_names.get(im.intent_id.as_str()) else {
            continue;
        };
        owners_by_path
            .entry(im.codefile_path.as_str())
            .or_default()
            .push(Owner {
                id: im.intent_id.clone(),
                name: (*name).to_string(),
                locator: im.locator.clone(),
            });
    }
    let cf_id_by_path: HashMap<&str, &str> = codefiles
        .iter()
        .map(|cf| (cf.path.as_str(), cf.id.as_str()))
        .collect();
    let mut newest_grounding: HashMap<&str, &str> = HashMap::new();
    let mut newest_claim: HashMap<&str, &str> = HashMap::new();
    for im in implements {
        let grounding = newest_grounding.entry(im.intent_id.as_str()).or_default();
        if im.created_at.as_str() > *grounding {
            *grounding = &im.created_at;
        }
        let claim = newest_claim.entry(im.codefile_path.as_str()).or_default();
        if im.created_at.as_str() > *claim {
            *claim = &im.created_at;
        }
    }
    let mut last_decision: HashMap<&str, &Note> = HashMap::new();
    for note in notes {
        if note.kind != "decision" || note.target_id.is_empty() {
            continue;
        }
        let existing = last_decision.entry(note.target_id.as_str()).or_insert(note);
        if note.created_at > existing.created_at {
            *existing = note;
        }
    }

    let mut summary = SymbolAccountabilitySummary::default();
    let mut raw_gaps = Vec::new();
    let mut open_gaps = Vec::new();
    let mut adjudicated_gaps = Vec::new();
    for cf in codefiles {
        if cf.symbol_facts.is_empty() {
            continue;
        }
        summary.instrumented_files += 1;
        let owners = owners_by_path
            .get(cf.path.as_str())
            .cloned()
            .unwrap_or_default();
        let risky = risky_file(&owners, cf.symbol_facts.len());
        let required_in_file = cf
            .symbol_facts
            .iter()
            .filter(|fact| {
                !fact.is_test && !owners.is_empty() && required_symbol(&cf.path, fact, risky)
            })
            .count();

        for fact in &cf.symbol_facts {
            summary.total_symbols += 1;
            if fact.is_test {
                summary.test_support += 1;
                continue;
            }
            if owners.is_empty() {
                summary.debris_candidates += 1;
                continue;
            }
            if !required_symbol(&cf.path, fact, risky) {
                summary.support += 1;
                continue;
            }

            summary.required += 1;
            let locators: Vec<String> = owners.iter().map(|owner| owner.locator.clone()).collect();
            if fact_is_grounded(fact, &locators) {
                summary.grounded += 1;
            } else if accepted_file_owner(&owners, required_in_file) {
                summary.accepted += 1;
            } else {
                let gap = ActionableSymbolGap {
                    path: cf.path.clone(),
                    label: fact.label.clone(),
                    name: fact.name.clone(),
                    kind: fact.kind.clone(),
                    line_start: fact.line_start,
                    reason: gap_reason(fact, risky).to_string(),
                    owner_intents: owners
                        .iter()
                        .map(|owner| format!("{} ({})", owner.name, owner.id))
                        .collect(),
                    suggested_action: suggested_action(&cf.path, fact, &owners),
                };
                raw_gaps.push(gap.clone());
                if let Some(note) = adjudicating_note(
                    cf,
                    &owners,
                    &cf_id_by_path,
                    &newest_claim,
                    &newest_grounding,
                    &last_decision,
                ) {
                    adjudicated_gaps.push(adjudicated_gap(&gap, note));
                } else {
                    open_gaps.push(gap);
                }
            }
        }
    }
    raw_gaps.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.label.cmp(&b.label)));
    open_gaps.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.label.cmp(&b.label)));
    adjudicated_gaps.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.ruled_at.cmp(&b.ruled_at))
    });
    summary.raw_actionable_gaps = raw_gaps.len();
    summary.adjudicated = adjudicated_gaps.len();
    summary.actionable_gaps = open_gaps.len();
    let resolved = summary.grounded + summary.accepted + summary.adjudicated;
    let pct = if summary.required == 0 {
        100.0
    } else {
        (resolved as f64 / summary.required as f64 * 1000.0).round() / 10.0
    };
    summary.resolved_pct = pct.clamp(0.0, 100.0);
    SymbolAccountabilityReport {
        summary,
        raw_actionable_symbol_gaps: raw_gaps,
        actionable_symbol_gaps: open_gaps,
        adjudicated_symbol_gaps: adjudicated_gaps,
        teaching: symbol_teaching(),
    }
}

fn risky_file(owners: &[Owner], _symbol_count: usize) -> bool {
    let empty_locators = owners
        .iter()
        .filter(|owner| owner.locator.trim().is_empty())
        .count();
    owners.len() >= 3 || empty_locators >= 2
}

fn required_symbol(path: &str, fact: &SymbolFact, risky: bool) -> bool {
    externally_public(path, fact) || risky
}

fn externally_public(path: &str, fact: &SymbolFact) -> bool {
    if fact.visibility == "private" {
        return false;
    }
    if path.ends_with(".rs") {
        return path == "src/lib.rs" || path.starts_with("src/bin/");
    }
    true
}

fn accepted_file_owner(owners: &[Owner], required_in_file: usize) -> bool {
    owners.len() == 1 && owners[0].locator.trim().is_empty() && required_in_file == 1
}

fn gap_reason(fact: &SymbolFact, risky: bool) -> &'static str {
    if fact.visibility != "private" {
        "public symbol has no precise active IMPLEMENTS locator"
    } else if risky {
        "risky file needs symbol-level ownership"
    } else {
        "required symbol has no precise active IMPLEMENTS locator"
    }
}

fn suggested_action(path: &str, fact: &SymbolFact, owners: &[Owner]) -> String {
    if owners.len() == 1 {
        format!(
            "refine the grounding: `loom edge implement {} {} --locator \"{}\"`",
            owners[0].id, path, fact.label
        )
    } else {
        format!(
            "`loom codefile show {path}`; decide which owner claims `{}` and refine that IMPLEMENTS locator, split the intent, or record a decision note if broad file ownership is deliberate",
            fact.label
        )
    }
}

pub fn fact_is_grounded(fact: &SymbolFact, locators: &[String]) -> bool {
    locators.iter().any(|locator| {
        let l = locator.trim();
        if l.is_empty() {
            return false;
        }
        l == fact.label
            // Word-boundary, not raw substring: a label `get` must not count as
            // grounded by a locator `widget`, nor `Note` by `Notebook`. Siblings
            // below are already boundary-aware; this branch was the over-matcher.
            || contains_identifier_word(l, &fact.label)
            || contains_identifier_word(l, &fact.name)
            || contains_identifier_word(l, symbol_identifier(&fact.label))
    })
}

fn adjudicating_note<'a>(
    cf: &CodeFile,
    owners: &[Owner],
    cf_id_by_path: &HashMap<&str, &str>,
    newest_claim: &HashMap<&str, &str>,
    newest_grounding: &HashMap<&str, &str>,
    last_decision: &HashMap<&str, &'a Note>,
) -> Option<&'a Note> {
    let file_anchor = newest_claim.get(cf.path.as_str()).copied().unwrap_or("");
    let file_note = cf_id_by_path
        .get(cf.path.as_str())
        .and_then(|cfid| current_decision(cfid, file_anchor, last_decision));
    let owner_note = owners
        .iter()
        .filter_map(|owner| {
            let anchor = newest_grounding
                .get(owner.id.as_str())
                .copied()
                .unwrap_or("");
            current_decision(owner.id.as_str(), anchor, last_decision)
        })
        .max_by_key(|note| note.created_at.as_str());
    file_note
        .into_iter()
        .chain(owner_note)
        .max_by_key(|note| note.created_at.as_str())
}

fn current_decision<'a>(
    target: &str,
    anchor: &str,
    last_decision: &HashMap<&str, &'a Note>,
) -> Option<&'a Note> {
    last_decision
        .get(target)
        .copied()
        .filter(|note| note.created_at.as_str() > anchor)
}

fn adjudicated_gap(gap: &ActionableSymbolGap, note: &Note) -> AdjudicatedSymbolGap {
    AdjudicatedSymbolGap {
        path: gap.path.clone(),
        label: gap.label.clone(),
        name: gap.name.clone(),
        kind: gap.kind.clone(),
        line_start: gap.line_start,
        reason: gap.reason.clone(),
        owner_intents: gap.owner_intents.clone(),
        ruling: note.text.clone(),
        ruled_by: note.author.clone(),
        ruled_at: note.created_at.clone(),
        reopens_when: "a newer grounding lands on the file or owning intent".into(),
    }
}

pub fn symbol_teaching() -> SymbolTeaching {
    SymbolTeaching {
        principle: "Symbols make important code impossible to hide; they are an accountability instrument, not a demand to ground every helper.".into(),
        inspect: vec![
            "start with actionable_symbol_gaps; raw_actionable_symbol_gaps is the audit trail before decision-note adjudication".into(),
            "use `loom codefile show <path>` before deciding which intent owns a symbol".into(),
            "public/exported symbols and risky multi-owner/broad-locator files deserve precise locators".into(),
        ],
        avoid: vec![
            "do not create intents for every private helper".into(),
            "do not bulk-ground symbols without checking intent meaning".into(),
            "do not treat 100% raw symbol coverage as the goal".into(),
        ],
        done_when: "stale locators are fixed and actionable symbol gaps are grounded, intentionally accepted with a decision note, or turned into real intent split/build work".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Note;

    fn intent(id: &str, name: &str) -> Intent {
        Intent {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            criterion: String::new(),
            abstraction_level: "feature".into(),
            domain: String::new(),
            layer: String::new(),
            source_refs: Vec::new(),
            status: "active".into(),
            aspect: "happy".into(),
            tags: Vec::new(),
            visibility: "internal".into(),
            boundary: String::new(),
            lifecycle: "implemented".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
        }
    }

    fn codefile(path: &str, facts: Vec<SymbolFact>) -> CodeFile {
        CodeFile {
            id: path.into(),
            path: path.into(),
            language: "rust".into(),
            last_modified: String::new(),
            imports: Vec::new(),
            symbols: facts.iter().map(|fact| fact.label.clone()).collect(),
            symbol_facts: facts,
            content_hash: String::new(),
            extractor_grade: String::new(),
        }
    }

    fn fact(label: &str, visibility: &str, is_test: bool) -> SymbolFact {
        let name = label.split_whitespace().last().unwrap_or(label).to_string();
        SymbolFact {
            label: label.into(),
            name,
            kind: "fn".into(),
            visibility: visibility.into(),
            line_start: 1,
            line_end: 2,
            is_test,
            string_literals: Vec::new(),
            panic_marker_count: 0,
            panic_markers: Vec::new(),
            body_hash: String::new(),
            shape_hash: String::new(),
        }
    }

    #[test]
    fn fact_is_grounded_matches_on_word_boundaries() {
        let f = fact("get", "private", false); // label "get", name "get"
                                               // Exact and word-bounded locators ground it.
        assert!(fact_is_grounded(&f, &["get".into()]));
        assert!(fact_is_grounded(&f, &["fn get()".into()]));
        // A longer identifier that merely CONTAINS the label as a substring must
        // NOT count as grounded (the bug: `get` matching `widget_handler`).
        assert!(!fact_is_grounded(&f, &["widget_handler".into()]));
        assert!(!fact_is_grounded(&f, &["target".into()]));
        // Empty locator never grounds.
        assert!(!fact_is_grounded(&f, &[String::new()]));
    }

    fn implements(intent_id: &str, path: &str, locator: &str) -> Implements {
        Implements {
            id: format!("imp:{intent_id}:{path}"),
            intent_id: intent_id.into(),
            codefile_id: path.into(),
            intent_name: String::new(),
            codefile_path: path.into(),
            inspection_status: "passing".into(),
            criterion: String::new(),
            confidence: 0.0,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: String::new(),
            locator: locator.into(),
            notes: String::new(),
            created_at: "t".into(),
        }
    }

    fn decision_note(id: &str, target_kind: &str, target_id: &str, created_at: &str) -> Note {
        Note {
            id: id.into(),
            kind: "decision".into(),
            text: "broad ownership is deliberate here".into(),
            author: "llm".into(),
            target_kind: target_kind.into(),
            target_id: target_id.into(),
            resolution: String::new(),
            audience: String::new(),
            created_at: created_at.into(),
        }
    }

    #[test]
    fn public_symbol_without_locator_is_actionable() {
        let report = symbol_accountability_from_parts(
            &[codefile(
                "pkg/a.py",
                vec![
                    fact("def run", "public", false),
                    fact("def stop", "public", false),
                ],
            )],
            &[intent("i", "run")],
            &[implements("i", "pkg/a.py", "")],
        );
        assert_eq!(report.summary.required, 2);
        assert_eq!(report.summary.actionable_gaps, 2);
        assert_eq!(report.actionable_symbol_gaps[0].label, "def run");
    }

    #[test]
    fn tiny_single_owner_file_accepts_file_level_grounding() {
        let report = symbol_accountability_from_parts(
            &[codefile("pkg/a.py", vec![fact("def run", "public", false)])],
            &[intent("i", "run")],
            &[implements("i", "pkg/a.py", "")],
        );
        assert_eq!(report.summary.accepted, 1);
        assert_eq!(report.summary.actionable_gaps, 0);
    }

    #[test]
    fn private_helpers_and_tests_are_not_actionable_in_simple_files() {
        let report = symbol_accountability_from_parts(
            &[codefile(
                "src/a.rs",
                vec![
                    fact("fn helper", "private", false),
                    fact("fn test_helper", "private", true),
                ],
            )],
            &[intent("i", "run")],
            &[implements("i", "src/a.rs", "fn run")],
        );
        assert_eq!(report.summary.support, 1);
        assert_eq!(report.summary.test_support, 1);
        assert_eq!(report.summary.actionable_gaps, 0);
    }

    #[test]
    fn risky_multi_owner_file_requires_private_symbol_precision() {
        let report = symbol_accountability_from_parts(
            &[codefile(
                "src/a.rs",
                vec![fact("fn dispatch", "private", false)],
            )],
            &[
                intent("i", "run"),
                intent("j", "route"),
                intent("k", "dispatch"),
            ],
            &[
                implements("i", "src/a.rs", ""),
                implements("j", "src/a.rs", "fn route"),
                implements("k", "src/a.rs", "fn other"),
            ],
        );
        assert_eq!(report.summary.required, 1);
        assert_eq!(report.summary.actionable_gaps, 1);
        assert_eq!(
            report.actionable_symbol_gaps[0].reason,
            "risky file needs symbol-level ownership"
        );
    }

    #[test]
    fn locator_word_match_is_boundary_checked() {
        let symbol = fact("pub fn run", "public", false);
        assert!(fact_is_grounded(&symbol, &["run()".to_string()]));
        assert!(!fact_is_grounded(&symbol, &["runtime".to_string()]));
    }

    #[test]
    fn decision_note_adjudicates_until_newer_grounding() {
        let file = codefile("pkg/a.py", vec![fact("def run", "public", false)]);
        let owner = intent("i", "run");
        let mut first_claim = implements("i", "pkg/a.py", "def other");
        first_claim.created_at = "t1".into();
        let report = symbol_accountability_from_parts_with_notes(
            std::slice::from_ref(&file),
            std::slice::from_ref(&owner),
            std::slice::from_ref(&first_claim),
            &[decision_note("n", "codefile", "pkg/a.py", "t2")],
        );
        assert_eq!(report.summary.raw_actionable_gaps, 1);
        assert_eq!(report.summary.adjudicated, 1);
        assert_eq!(report.summary.actionable_gaps, 0);
        assert_eq!(report.summary.resolved_pct, 100.0);
        assert_eq!(report.adjudicated_symbol_gaps[0].ruled_at, "t2");

        let mut newer_claim = implements("i", "pkg/a.py", "def other");
        newer_claim.created_at = "t3".into();
        let reopened = symbol_accountability_from_parts_with_notes(
            &[file],
            &[owner],
            &[newer_claim],
            &[decision_note("n", "codefile", "pkg/a.py", "t2")],
        );
        assert_eq!(reopened.summary.raw_actionable_gaps, 1);
        assert_eq!(reopened.summary.adjudicated, 0);
        assert_eq!(reopened.summary.actionable_gaps, 1);
    }
}
