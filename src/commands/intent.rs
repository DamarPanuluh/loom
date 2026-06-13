use anyhow::Result;
use uuid::Uuid;

use crate::cli::{IntentCmd, SourceCmd, TagCmd};
use crate::db::queries::{
    add_source_ref, confirm_intent, delete_intent, edges_for_intent, get_intent, insert_intent,
    list_hierarchy_for_intent, list_implements_for_intent, list_intents, notes_for_target,
    remove_source_ref, set_intent_layer, set_intent_lifecycle,
};
use crate::db::schema::role;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::gate;
use crate::output::{fmt_edge_row, fmt_intent, fmt_intent_row, Printer};
use crate::types::Intent;

pub fn run(cmd: IntentCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    match cmd {
        IntentCmd::Add {
            name,
            description,
            level,
            domain,
            layer,
            aspect,
            lifecycle,
            sources,
            tags,
            visibility,
        } => {
            gate::acting_in_lane("add an intent", &[role::BUILDER], None)?;
            // Validate and canonicalize abstraction level + lifecycle.
            let level = level
                .parse::<crate::types::AbstractionLevel>()
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            lifecycle
                .parse::<crate::types::LifecycleState>()
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            if name.trim().is_empty() {
                anyhow::bail!(
                    "--name must not be empty. State the responsibility this intent owns."
                );
            }
            if description.trim().is_empty() {
                anyhow::bail!("--description must not be empty. State the observable behavior or design responsibility this intent captures.");
            }
            // planned/needs_change promise code changes — meaningless on a
            // repo this graph merely observes.
            if lifecycle != "implemented" {
                crate::db::queries::ensure_owned(
                    &db,
                    &format!("declare a '{lifecycle}' intent (a promise to change the code)"),
                )?;
            }
            if !matches!(visibility.as_str(), "" | "user_visible" | "internal") {
                anyhow::bail!(
                    "Invalid --visibility '{visibility}'. Valid: user_visible (a capability the \
                     user can see/feel) | internal (machinery serving other intents). Omit when \
                     untriaged — the align interview triages it."
                );
            }

            // Tags are validated against the registry — unknown terms error
            // with the full registry inlined (the agent sees the menu at the
            // moment of choice). Empty = untagged, always honest.
            let tags = crate::commands::vocab::validate_tags(&db, &tags)?;
            let has_tags = !tags.is_empty();
            let tags = crate::db::queries::encode_tags(tags)?;
            let source_refs = sources.clone();
            let now = chrono::Utc::now().to_rfc3339();
            let id = Uuid::new_v4().to_string();

            let intent = Intent {
                id: id.clone(),
                name: name.clone(),
                description,
                abstraction_level: level.to_string(),
                domain,
                layer,
                source_refs,
                status: "proposed".to_string(),
                aspect,
                tags,
                visibility,
                lifecycle,
                created_at: now.clone(),
                updated_at: now,
            };

            insert_intent(&db, &intent)?;

            // Connecting the intent into the HIERARCHY tree is the FIRST step
            // (the vertical spine is what makes the graph complete). A `system`
            // intent is a root and gets decomposed downward; anything else needs
            // a parent. Lead with that so a cold driver never leaves intents
            // floating, then point at grounding.
            let is_root = intent.abstraction_level == "system";
            let tree_step = if is_root {
                format!("Decompose it: add child intents, then link with `loom edge hierarchy {} <child-id>` (this is the tree's root).", id)
            } else {
                format!("Attach it to the tree: `loom edge hierarchy <parent-id> {}` (every non-system intent needs exactly one parent).", id)
            };
            // The tag affordance, in-band: only when there IS a registry to
            // pick from and the intent arrived untagged. One line, never a nag
            // — tags stay optional at write time but matter once code is
            // grounded and the audit asks whether duplicate detection is armed.
            let registry_size = crate::db::queries::list_vocab_terms(&db)?.len();
            let tag_step = (!has_tags && registry_size > 0).then(|| format!(
                "Optional now, audit-relevant once grounded: tag it from the {registry_size}-term vocabulary (`loom vocab list`, then `loom intent tag add {id} <term>`) so duplicate-responsibility detection has its strongest signal."
            ));

            if printer.json {
                let mut v = serde_json::to_value(&intent)?;
                if let Some(obj) = v.as_object_mut() {
                    let mut steps = vec![
                        tree_step,
                        "Ground it to code: `loom edge implement <intent> <codefile> --locator \"<symbol>\"` (the symbol as it appears in the file — e.g. `def foo`, `fn foo`; required for leaf intents).".to_string(),
                        "Relate it to other intents — `loom next` will surface unexplored pairs (optional).".to_string(),
                        "If this is a feature, add its sad/fallback siblings (--aspect).".to_string(),
                    ];
                    if let Some(ts) = &tag_step {
                        steps.push(ts.clone());
                    }
                    obj.insert("next_steps".to_string(), serde_json::json!(steps));
                }
                printer.print_json(&v);
            } else {
                println!("✓ Intent created");
                println!("{}", fmt_intent(&intent));
                println!("  → Next: {}", tree_step);
                println!("          then ground it: `loom edge implement {} <codefile> --locator \"<symbol>\"` (symbol as written in the file).", id);
                if let Some(ts) = &tag_step {
                    println!("          {ts}");
                }
            }
        }

        IntentCmd::Confirm { id, visibility } => {
            // Confirmation is a *verdict* that the intent is valid — validator
            // lane, so the builder cannot ratify its own proposals.
            let by = gate::acting_in_lane("confirm an intent", &[role::VALIDATOR], None)?;
            let id = crate::db::queries::resolve_intent(&db, &id)?;
            let intent = get_intent(&db, &id)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
                    id
                )
            })?;
            if intent.status == "deprecated" {
                anyhow::bail!(
                    "Intent '{}' is retired (status=deprecated). Retirement is permanent history: create a successor intent and link the lineage instead of confirming it.",
                    id
                );
            }
            if let Some(v) = visibility.as_deref() {
                if !matches!(v, "user_visible" | "internal") {
                    anyhow::bail!(
                        "Invalid --visibility '{v}'. Valid: user_visible (a capability the user \
                         can see/feel) | internal (machinery serving other intents — leaves the \
                         align interview until redefined)."
                    );
                }
            }
            let now = chrono::Utc::now().to_rfc3339();
            // Atomic: the status flip, its freshness stamp, and the audience
            // ruling land together. The stamp is what `loom next --mode align`
            // ranks by — a confirm without it would ratify the meaning while
            // leaving the intent looking drift-suspect forever. The visibility
            // ruling is part of the same interview outcome ("this is internal,
            // stop asking"): splitting them would let one land without the other.
            let found = crate::db::with_transaction(&db, || {
                let found = confirm_intent(&db, &id, &now)?;
                if found {
                    crate::db::queries::record_confirmation(&db, &id, &by, &now)?;
                    if let Some(v) = visibility.as_deref() {
                        crate::db::queries::set_intent_visibility(&db, &id, v, &now)?;
                        crate::db::queries::insert_note(
                            &db,
                            &crate::types::Note {
                                id: Uuid::new_v4().to_string(),
                                kind: "decision".into(),
                                text: format!("visibility ruled {v} during alignment"),
                                author: by.clone(),
                                target_kind: "intent".into(),
                                target_id: id.clone(),
                                audience: String::new(),
                                created_at: now.clone(),
                            },
                        )?;
                    }
                }
                Ok(found)
            })?;
            if !found {
                anyhow::bail!(
                    "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
                    id
                );
            }
            let confirmed_msg = match visibility.as_deref() {
                Some("internal") => "confirmed + ruled internal — out of the align interview until its meaning is redefined",
                Some("user_visible") => "confirmed + ruled user-visible",
                _ => "confirmed",
            };
            if printer.json {
                let mut payload =
                    serde_json::json!({"status":"ok","id":id,"new_status":"confirmed"});
                if let Some(v) = visibility.as_deref() {
                    payload["visibility"] = serde_json::json!(v);
                }
                let payload =
                    crate::output::with_anchor(payload, &db, "`loom next` serves the next item")?;
                printer.print_json(&payload);
            } else {
                println!("✓ Intent {} {}", id, confirmed_msg);
                crate::output::print_anchor(&db, "`loom next` serves the next item")?;
            }
        }

        IntentCmd::Update {
            id,
            name,
            layer,
            description,
            reword,
            reason,
            extra,
        } => {
            // Syntax friction kills the loop mid-interview (dogfood: an agent
            // stalled on "what's the update syntax?" and went doc-hunting) —
            // the two observed stumbles teach the full shape right here.
            if let Some(first) = extra.first() {
                anyhow::bail!(
                    "Unexpected positional text {first:?} — new wording travels through flags:\n  \
                     loom intent update \"{id}\" --description \"<new meaning>\" --reason \"<why it moved>\"\n  \
                     (--reword when only the words change; --name \"<new>\" for a cosmetic rename)"
                );
            }
            if reason.trim().is_empty() {
                anyhow::bail!(
                    "--reason is required: the recorded WHY behind the change (it lands as a decision \
                     note beside the old wording — the export diff is not the only history). Pick the shape:\n  \
                     concept evolved:    loom intent update \"{id}\" --description \"<new meaning>\" --reason \"<what was decided>\"   (ripples staleness one hop)\n  \
                     clearer words only: loom intent update \"{id}\" --description \"<same concept, clearer>\" --reword --reason \"<what was confusing>\"   (no ripple)\n  \
                     rename:             loom intent update \"{id}\" --name \"<new name>\" --reason \"<why>\"   (cosmetic, no ripple)"
                );
            }
            // Evolution is builder-owned, like add/retire: the meaning
            // statement is design, and design decisions belong to the
            // graph's owners.
            let by = gate::acting_in_lane("update an intent", &[role::BUILDER], None)?;
            crate::db::queries::ensure_owned(
                &db,
                "update an intent (the design decision belongs to the graph's owners)",
            )?;
            gate::require_substantive("reason", &reason, "why the meaning moved")?;
            let id = crate::db::queries::resolve_intent(&db, &id)?;
            let intent = get_intent(&db, &id)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
                    id
                )
            })?;
            if intent.status == "deprecated" {
                anyhow::bail!(
                    "Intent '{}' is retired (status=deprecated). Retirement is permanent history: create a successor intent (`loom intent add` + `--replaced-by` lineage) instead of rewriting it.",
                    id
                );
            }
            let new_name = name.as_deref().filter(|n| *n != intent.name);
            let new_layer = layer.as_deref().filter(|l| *l != intent.layer);
            let new_desc = description.as_deref().filter(|d| *d != intent.description);
            if new_name.is_none() && new_layer.is_none() && new_desc.is_none() {
                anyhow::bail!(
                    "Nothing to change: pass --name, --layer, and/or --description with a value that differs from the current one (`loom intent show {}` prints them).",
                    id
                );
            }
            let now = chrono::Utc::now().to_rfc3339();
            // Atomic: the new wording, its decision notes (which preserve the
            // OLD wording — the export diff is not the only history), and the
            // semantic ripple land together or not at all. A redefinition
            // whose ripple is missing would be the lie this command exists to
            // prevent: green verdicts standing on words that no longer exist.
            //
            // Lifecycle is NOT auto-flipped to needs_change: that would fake
            // a verdict nobody made. The flipped IMPLEMENTS grounding routes
            // the honest question — "does the code still do what this now
            // says?" — through the fix queue, where a real inspection decides.
            let ripple = crate::db::with_transaction(&db, || {
                crate::db::queries::update_intent_meaning(&db, &id, new_name, new_desc, &now)?;
                if let Some(layer) = new_layer {
                    set_intent_layer(&db, &id, layer, &now)?;
                    crate::db::queries::insert_note(
                        &db,
                        &crate::types::Note {
                            id: Uuid::new_v4().to_string(),
                            kind: "decision".into(),
                            text: format!(
                                "layer changed: '{}' → '{}' ({})",
                                if intent.layer.is_empty() {
                                    "<undeclared>"
                                } else {
                                    &intent.layer
                                },
                                if layer.is_empty() {
                                    "<undeclared>"
                                } else {
                                    layer
                                },
                                reason
                            ),
                            author: by.clone(),
                            target_kind: "intent".into(),
                            target_id: id.clone(),
                            audience: String::new(),
                            created_at: now.clone(),
                        },
                    )?;
                }
                if let Some(n) = new_name {
                    crate::db::queries::insert_note(
                        &db,
                        &crate::types::Note {
                            id: Uuid::new_v4().to_string(),
                            kind: "decision".into(),
                            text: format!("renamed: '{}' → '{}' ({})", intent.name, n, reason),
                            author: by.clone(),
                            target_kind: "intent".into(),
                            target_id: id.clone(),
                            audience: String::new(),
                            created_at: now.clone(),
                        },
                    )?;
                }
                if let Some(d) = new_desc {
                    if reword {
                        // REWORDING: the concept the user confirmed stays;
                        // only the words get clearer (the "terminology
                        // confusing, keep concept" interview outcome). No
                        // semantic ripple — no claim's meaning moved — and
                        // the visibility ruling survives. The "reworded:"
                        // stamp still resets the align clock, so the intent
                        // exits the interview queue exactly like a
                        // redefinition does.
                        crate::db::queries::insert_note(
                            &db,
                            &crate::types::Note {
                                id: Uuid::new_v4().to_string(),
                                kind: "decision".into(),
                                text: format!("reworded: {}\nwas: {}", reason, intent.description),
                                author: by.clone(),
                                target_kind: "intent".into(),
                                target_id: id.clone(),
                                audience: String::new(),
                                created_at: now.clone(),
                            },
                        )?;
                        let _ = d;
                        return Ok(None);
                    }
                    crate::db::queries::insert_note(
                        &db,
                        &crate::types::Note {
                            id: Uuid::new_v4().to_string(),
                            kind: "decision".into(),
                            text: format!("redefined: {}\nwas: {}", reason, intent.description),
                            author: by.clone(),
                            target_kind: "intent".into(),
                            target_id: id.clone(),
                            audience: String::new(),
                            created_at: now.clone(),
                        },
                    )?;
                    let _ = d; // the new wording lives on the node; the note keeps the old
                               // The audience ruling was made about the OLD meaning —
                               // a redefinition makes it unknown again (the align
                               // interview re-triages it). Cleared with the ripple, for
                               // the same reason the ripple exists.
                    if !intent.visibility.is_empty() {
                        crate::db::queries::set_intent_visibility(&db, &id, "", &now)?;
                    }
                    return Ok(Some(crate::db::queries::ripple_intent_redefinition(
                        &db,
                        &id,
                        new_name.unwrap_or(&intent.name),
                        &now,
                    )?));
                }
                Ok(None)
            })?;
            let rippled = ripple.as_ref().is_some_and(|r| {
                r.relates_to_flagged
                    + r.governs_flagged
                    + r.targets_flagged
                    + r.implements_flagged
                    + r.validations_invalidated
                    > 0
            });
            let next_step = if rippled {
                "`loom next --mode fix` re-inspects staled claims; `loom next --mode quality` re-earns flagged quality green; `loom validate` re-runs invalidated proofs."
            } else {
                "`loom next` serves the next item"
            };
            if printer.json {
                let payload = crate::output::with_anchor(
                    serde_json::json!({
                        "status": "ok", "id": id,
                        "renamed": new_name.is_some(),
                        "layer_changed": new_layer.is_some(),
                        "redefined": new_desc.is_some() && !reword,
                        "reworded": new_desc.is_some() && reword,
                        "visibility_cleared": new_desc.is_some() && !reword && !intent.visibility.is_empty(),
                        "ripple": ripple,
                    }),
                    &db,
                    next_step,
                )?;
                printer.print_json(&payload);
            } else {
                match (new_name, new_desc) {
                    (_, Some(_)) if reword => println!(
                        "✓ Intent {id} reworded (same concept, clearer words — no ripple)."
                    ),
                    (Some(n), Some(_)) => println!("✓ Intent {id} renamed to '{n}' and redefined."),
                    (Some(n), None) => {
                        println!("✓ Intent {id} renamed to '{n}' (cosmetic — no ripple).")
                    }
                    (None, Some(_)) => println!("✓ Intent {id} redefined."),
                    (None, None) => {
                        if let Some(layer) = new_layer {
                            println!("✓ Intent {id} layer → '{}' (metadata — no ripple).", layer);
                        } else {
                            unreachable!("bailed above");
                        }
                    }
                }
                if new_layer.is_some() && (new_name.is_some() || new_desc.is_some()) {
                    println!("  layer changed (metadata — no ripple).");
                }
                if new_desc.is_some() && !reword && !intent.visibility.is_empty() {
                    println!(
                        "  visibility ruling '{}' cleared — the new meaning's audience is unknown; the align interview re-triages it.",
                        intent.visibility
                    );
                }
                if let Some(r) = &ripple {
                    if rippled {
                        println!("  SEMANTIC RIPPLE (claims earned against the old wording):");
                        if r.relates_to_flagged > 0 {
                            println!(
                                "    · {} RELATES_TO verdict(s) → needs_reverification",
                                r.relates_to_flagged
                            );
                        }
                        if r.implements_flagged > 0 {
                            println!("    · {} IMPLEMENTS grounding(s) → needs_reverification (does the code still do what this now says?)", r.implements_flagged);
                        }
                        if r.governs_flagged > 0 {
                            println!("    · {} GOVERNS verdict(s) → needs_reverification (green re-earned against the new meaning)", r.governs_flagged);
                        }
                        if r.targets_flagged > 0 {
                            println!(
                                "    · {} hypothesis TARGETS edge(s) → needs_reverification",
                                r.targets_flagged
                            );
                        }
                        if r.validations_invalidated > 0 {
                            println!("    · {} validation(s) → not_run (they proved the old acceptance contract)", r.validations_invalidated);
                        }
                    } else {
                        println!("  No earned claims touched this intent — nothing to re-verify.");
                    }
                }
                crate::output::print_anchor(&db, next_step)?;
            }
        }

        IntentCmd::Mark {
            id,
            lifecycle,
            reason,
        } => {
            // Lifecycle is builder-owned; the fixer transitions it
            // (needs_change → implemented) as part of resolving issues.
            let by = gate::acting_in_lane(
                "set an intent lifecycle",
                &[role::BUILDER, role::FIXER],
                None,
            )?;
            crate::db::queries::ensure_owned(
                &db,
                "change an intent's lifecycle (a claim about building/changing the code)",
            )?;
            lifecycle
                .parse::<crate::types::LifecycleState>()
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            let id = crate::db::queries::resolve_intent(&db, &id)?;
            let now = chrono::Utc::now().to_rfc3339();
            let found = set_intent_lifecycle(&db, &id, &lifecycle, &now)?;
            if !found {
                anyhow::bail!(
                    "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
                    id
                );
            }
            // Record the rationale as a note (append-only memory).
            if let Some(ref r) = reason {
                let note = crate::types::Note {
                    id: Uuid::new_v4().to_string(),
                    kind: "decision".to_string(),
                    text: format!("lifecycle → {}: {}", lifecycle, r),
                    author: by.clone(),
                    target_kind: "intent".to_string(),
                    target_id: id.clone(),
                    audience: String::new(),
                    created_at: now.clone(),
                };
                crate::db::queries::insert_note(&db, &note)?;
            }
            // Always anchor (invariant 1): a lifecycle transition moves the
            // compass phase — most sharply the terminal needs_change→implemented
            // fixer transition, which previously printed no guidance at all.
            // Hint per destination state.
            let next_step = match lifecycle.as_str() {
                "planned" | "needs_change" => "`loom next --mode build` will surface it.",
                // An implemented leaf without a passed validation routes to the
                // validate queue (stats.rs) — point there.
                "implemented" => {
                    "if this leaf is fully grounded, prove it: `loom next --mode validate`"
                }
                _ => "`loom next` serves the next item",
            };
            if printer.json {
                let payload = serde_json::json!({
                    "status": "ok", "id": id, "lifecycle": lifecycle,
                });
                printer.print_json(&crate::output::with_anchor(payload, &db, next_step)?);
            } else {
                println!("✓ Intent {} → lifecycle '{}'", id, lifecycle);
                crate::output::print_anchor(&db, next_step)?;
            }
        }

        IntentCmd::Delete { id } => {
            gate::acting_in_lane("delete an intent", &[role::BUILDER], None)?;
            let id = crate::db::queries::resolve_intent(&db, &id)?;
            // Atomic: node, edges, and all their notes go together.
            let deleted = crate::db::with_transaction(&db, || delete_intent(&db, &id))?;
            if !deleted {
                anyhow::bail!(
                    "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
                    id
                );
            }
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "id": id, "deleted": true,
                    "next_step": "`loom status` re-checks the compass",
                }));
            } else {
                println!("✓ Intent {} deleted (with its edges and notes).", id);
                println!("  → Next: `loom status` re-checks the compass");
            }
        }

        IntentCmd::Retire {
            id,
            reason,
            replaced_by,
        } => {
            gate::acting_in_lane("retire an intent", &[role::BUILDER], None)?;
            crate::db::queries::ensure_owned(
                &db,
                "retire an intent (the design decision belongs to the graph's owners)",
            )?;
            gate::require_substantive("reason", &reason, "why this design was superseded")?;
            let id = crate::db::queries::resolve_intent(&db, &id)?;
            let successor = match &replaced_by {
                Some(k) => {
                    let sid = crate::db::queries::resolve_intent(&db, k)?;
                    if sid == id {
                        anyhow::bail!("--replaced-by points at the intent being retired — pass a different successor or omit --replaced-by.");
                    }
                    Some(sid)
                }
                None => None,
            };
            // Fallout BEFORE the flip, so the report reflects this retirement.
            let fallout = crate::db::queries::retire_fallout(&db, &id)?;
            let now = chrono::Utc::now().to_rfc3339();
            // Atomic: the status flip and its decision/lineage notes land
            // together or not at all — a half-retired intent (deprecated but
            // unexplained) would defeat the whole "history stays traceable" point.
            if !crate::db::with_transaction(&db, || {
                crate::db::queries::retire_intent(&db, &id, &reason, successor.as_deref(), &now)
            })? {
                anyhow::bail!(
                    "Intent '{}' not found. Run `loom intent list` (or `loom find \"<words>\"`).",
                    id
                );
            }
            let next_step =
                "`loom status` re-checks the compass; `loom coverage` shows any new gaps.";
            if printer.json {
                let payload = crate::output::with_anchor(
                    serde_json::json!({
                        "status": "ok", "id": id, "retired": true,
                        "replaced_by": successor, "fallout": fallout,
                    }),
                    &db,
                    next_step,
                )?;
                printer.print_json(&payload);
            } else {
                println!("✓ Intent {id} retired (status=deprecated — history kept, computation stops counting it).");
                if let Some(s) = &successor {
                    println!("  replaced by: {s}");
                }
                let f = &fallout;
                if f.orphaned_children.is_empty()
                    && f.solely_grounded_files.is_empty()
                    && f.dangling_validations.is_empty()
                {
                    println!(
                        "  No fallout: no children, no solely-owned files, no dangling proofs."
                    );
                } else {
                    println!("  TRIGGERED WORK:");
                    for c in &f.orphaned_children {
                        println!("    · child '{c}' lost its parent — re-parent (`loom edge hierarchy <new-parent> …`) or retire it too");
                    }
                    for p in &f.solely_grounded_files {
                        println!("    · {p} lost its only owner — it now reads UNREACHED (ground under a successor or `loom ignore`)");
                    }
                    for v in &f.dangling_validations {
                        println!("    · validation '{v}' proves only retired design — re-link (`loom edge validates …`) or `loom validation delete`");
                    }
                }
                if f.edges_leaving_computation > 0 {
                    println!("  {} RELATES_TO edge(s) leave every queue/centrality computation (kept as history); verified ones are flagged, so living neighbours surface in `loom next --mode align` for the user to re-affirm.", f.edges_leaving_computation);
                }
                crate::output::print_anchor(&db, next_step)?;
            }
        }

        IntentCmd::List {
            status,
            level,
            limit,
        } => {
            // Validate filter values against the domain vocabulary.
            if let Some(ref s) = status {
                s.parse::<crate::types::IntentStatus>()
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
            }
            let level = match level {
                Some(l) => Some(
                    l.parse::<crate::types::AbstractionLevel>()
                        .map_err(|e| anyhow::anyhow!("{}", e))?
                        .to_string(),
                ),
                None => None,
            };
            let mut intents = list_intents(&db, status.as_deref(), level.as_deref())?;
            let total = crate::output::apply_limit(&mut intents, limit);
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "intents": intents,
                    "total": total,
                    "truncated": intents.len() < total,
                }));
            } else {
                if intents.is_empty() {
                    println!("(no intents found)");
                } else {
                    println!(
                        "  {status:>20}   {level:<15}  {name:<40}  id",
                        status = "STATUS",
                        level = "LEVEL",
                        name = "NAME",
                    );
                    println!("  {}", "-".repeat(90));
                    for i in &intents {
                        println!("{}", fmt_intent_row(i));
                    }
                    if let Some(m) = crate::output::more_marker(
                        total,
                        intents.len(),
                        "loom intent list --limit 0",
                    ) {
                        println!("  {m}");
                    }
                }
            }
        }

        IntentCmd::Source { subcommand } => {
            // source_refs is builder-owned (it's part of declaring the intent).
            gate::acting_in_lane("edit an intent's source refs", &[role::BUILDER], None)?;
            let now = chrono::Utc::now().to_rfc3339();
            match subcommand {
                SourceCmd::Add { id, path } => {
                    let id = crate::db::queries::resolve_intent(&db, &id)?;
                    if !add_source_ref(&db, &id, &path, &now)? {
                        anyhow::bail!("Intent '{}' not found. Run `loom intent list` (or `loom find \"<words>\"`).", id);
                    }
                    let parsed = get_intent(&db, &id)?
                        .map(|i| i.source_refs)
                        .unwrap_or_default();
                    if printer.json {
                        printer.print_json(&serde_json::json!({
                            "status": "ok", "id": id, "added": path,
                            "source_refs": parsed,
                            "next_step": format!("`loom intent show {id}`"),
                        }));
                    } else {
                        println!("✓ Source ref added to intent {id}: {path}");
                        println!("  → Next: `loom intent show {id}`");
                    }
                }
                SourceCmd::Remove { id, path } => {
                    let id = crate::db::queries::resolve_intent(&db, &id)?;
                    match remove_source_ref(&db, &id, &path, &now)? {
                        None => anyhow::bail!("Intent '{}' not found. Run `loom intent list` (or `loom find \"<words>\"`).", id),
                        Some(false) => anyhow::bail!(
                            "Intent {} has no source ref '{}' — `loom intent show {}` lists them.",
                            id, path, id
                        ),
                        Some(true) => {
                            if printer.json {
                                printer.print_json(&serde_json::json!({
                                    "status": "ok", "id": id, "removed": path,
                                    "next_step": format!("`loom intent show {id}`"),
                                }));
                            } else {
                                println!("✓ Source ref removed from intent {id}: {path}");
                                println!("  → Next: `loom intent show {id}`");
                            }
                        }
                    }
                }
            }
        }

        IntentCmd::Tag { subcommand } => {
            // Tags are builder-owned intent metadata, like source_refs.
            gate::acting_in_lane("edit an intent's tags", &[role::BUILDER], None)?;
            let now = chrono::Utc::now().to_rfc3339();
            match subcommand {
                TagCmd::Add { id, term } => {
                    let id = crate::db::queries::resolve_intent(&db, &id)?;
                    let intent = get_intent(&db, &id)?
                        .ok_or_else(|| anyhow::anyhow!("Intent '{}' not found. Run `loom intent list` (or `loom find \"<words>\"`).", id))?;
                    let mut tags = crate::db::queries::parse_tags(&intent)?;
                    tags.push(term);
                    // validate_tags normalizes, dedupes (idempotent re-add),
                    // enforces the cap, and nudges on unknown terms.
                    let tags = crate::commands::vocab::validate_tags(&db, &tags)?;
                    crate::db::queries::set_intent_tags(&db, &id, tags.clone(), &now)?;
                    if printer.json {
                        printer.print_json(&serde_json::json!({
                            "status": "ok", "id": id, "tags": tags,
                            "next_step": format!("`loom intent show {id}`"),
                        }));
                    } else {
                        println!("✓ Intent {id} tagged: [{}]", tags.join(", "));
                        println!("  → Next: `loom intent show {id}`");
                    }
                }
                TagCmd::Remove { id, term } => {
                    let id = crate::db::queries::resolve_intent(&db, &id)?;
                    let intent = get_intent(&db, &id)?
                        .ok_or_else(|| anyhow::anyhow!("Intent '{}' not found. Run `loom intent list` (or `loom find \"<words>\"`).", id))?;
                    let term = crate::db::queries::normalize_term(&term)?;
                    let mut tags = crate::db::queries::parse_tags(&intent)?;
                    let before = tags.len();
                    tags.retain(|t| *t != term);
                    if tags.len() == before {
                        anyhow::bail!(
                            "Intent {} carries no tag '{}' — `loom intent show {}` lists them.",
                            id,
                            term,
                            id
                        );
                    }
                    crate::db::queries::set_intent_tags(&db, &id, tags.clone(), &now)?;
                    if printer.json {
                        printer.print_json(&serde_json::json!({
                            "status": "ok", "id": id, "removed": term, "tags": tags,
                            "next_step": format!("`loom intent show {id}`"),
                        }));
                    } else {
                        println!("✓ Tag '{term}' removed from intent {id}");
                        println!("  → Next: `loom intent show {id}`");
                    }
                }
            }
        }

        IntentCmd::Show { id } => {
            let id = crate::db::queries::resolve_intent(&db, &id)?;
            let intent = get_intent(&db, &id)?;
            match intent {
                None => anyhow::bail!(
                    "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
                    id
                ),
                Some(ref i) => {
                    let mut edges = edges_for_intent(&db, &id)?;
                    let edges_total =
                        crate::output::apply_limit(&mut edges, crate::output::SECTION_CAP);
                    let mut hierarchy = list_hierarchy_for_intent(&db, &id)?;
                    let hierarchy_total =
                        crate::output::apply_limit(&mut hierarchy, crate::output::SECTION_CAP);
                    let mut implements = list_implements_for_intent(&db, &id)?;
                    let implements_total =
                        crate::output::apply_limit(&mut implements, crate::output::SECTION_CAP);
                    let mut notes = notes_for_target(&db, &id)?;
                    let notes_total = notes.len();
                    if notes_total > crate::output::SECTION_CAP {
                        // notes_for_target returns oldest-first; keep the NEWEST.
                        notes.drain(..notes_total - crate::output::SECTION_CAP);
                    }
                    if printer.json {
                        printer.print_json(&serde_json::json!({
                            "intent": i,
                            "edges": edges,
                            "edges_total": edges_total,
                            "hierarchy": hierarchy,
                            "hierarchy_total": hierarchy_total,
                            "implements": implements,
                            "implements_total": implements_total,
                            "notes": notes,
                            "notes_total": notes_total,
                        }));
                    } else {
                        println!(
                            "── Intent ─────────────────────────────────────────────────────────"
                        );
                        println!("{}", fmt_intent(i));
                        println!();
                        println!(
                            "── RELATES_TO edges ({}) ────────────────────────────────────────────",
                            edges_total
                        );
                        if edges.is_empty() {
                            println!("  (none)");
                        } else {
                            for e in &edges {
                                println!("{}", fmt_edge_row(e));
                            }
                            if let Some(m) = crate::output::more_marker(
                                edges_total,
                                edges.len(),
                                &format!("loom cluster {id}"),
                            ) {
                                println!("  {m}");
                            }
                        }
                        println!();
                        println!(
                            "── Hierarchy ({}) ───────────────────────────────────────────────────",
                            hierarchy_total
                        );
                        if hierarchy.is_empty() {
                            println!("  (none — no parent/child intents)");
                        } else {
                            for h in &hierarchy {
                                if h.parent_id == id {
                                    println!("  ↓ child:  {} ({})", h.child_name, h.child_id);
                                } else {
                                    println!("  ↑ parent: {} ({})", h.parent_name, h.parent_id);
                                }
                            }
                            if let Some(m) = crate::output::more_marker(
                                hierarchy_total,
                                hierarchy.len(),
                                &format!("loom cluster {id}"),
                            ) {
                                println!("  {m}");
                            }
                        }
                        println!();
                        println!(
                            "── Implements ({}) ──────────────────────────────────────────────────",
                            implements_total
                        );
                        if implements.is_empty() {
                            println!("  (none — intent not yet grounded to code)");
                        } else {
                            for im in &implements {
                                let loc = if im.locator.is_empty() {
                                    String::new()
                                } else {
                                    format!("  @ {}", im.locator)
                                };
                                println!(
                                    "  → {}{}  [{}]",
                                    im.codefile_path, loc, im.inspection_status
                                );
                            }
                            if let Some(m) = crate::output::more_marker(
                                implements_total,
                                implements.len(),
                                &format!("loom cluster {id}"),
                            ) {
                                println!("  {m}");
                            }
                        }
                        println!();
                        println!(
                            "── Notes ({}) ───────────────────────────────────────────────────────",
                            notes_total
                        );
                        if notes.is_empty() {
                            println!("  (none)");
                        } else {
                            for n in &notes {
                                println!("  [{}] {}  ({})", n.kind, n.text, n.author);
                            }
                            if let Some(m) = crate::output::more_marker(
                                notes_total,
                                notes.len(),
                                &format!("loom note list --intent {id}"),
                            ) {
                                println!("  {m}");
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
