//! `loom hypothesis *` — the pre-decision plane.
//!
//! State machine: proposed → supported | refuted → adopted | rejected.
//! Separation of duties: anyone proposes, the ANALYZER lane proves (and the
//! prover may not be the proposer), the BUILDER lane decides. A hypothesis is
//! outside coverage/completeness by construction. `loom next --mode prove` and
//! the `hypothesis_accumulation` smell keep the plane from becoming passive
//! memory; only adoption (which spawns ordinary planned intents) turns it into
//! implementation work.

use anyhow::Result;
use uuid::Uuid;

use crate::cli::HypothesisCmd;
use crate::db::queries::{
    get_hypothesis, insert_hypothesis, insert_note, insert_targets, insert_validates,
    insert_validation, list_hypotheses, list_targets_for_hypothesis, notes_for_target,
    resolve_hypothesis, resolve_intent, set_hypothesis_status, set_targets_status_for_hypothesis,
    update_hypothesis_verdict,
};
use crate::db::schema::role;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::gate;
use crate::output::Printer;
use crate::types::{Hypothesis, HypothesisStatus, Note, Validation};

pub fn run(cmd: HypothesisCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    match cmd {
        HypothesisCmd::Add {
            name,
            claim,
            proposal,
            predicted_outcome,
            targets,
            author,
        } => {
            // Proposing is open to every lane (like notes) — but never vacuous:
            // an unfalsifiable claim or an unmeasurable prediction is a wishlist
            // entry, not a hypothesis.
            let author = crate::agent::acting(author.as_deref());
            gate::require_substantive(
                "claim", &claim,
                "what is wrong/suboptimal in the code AS IT IS NOW (the prover will check exactly this)",
            )?;
            gate::require_substantive("proposal", &proposal, "the change being proposed")?;
            gate::require_substantive(
                "predicted-outcome",
                &predicted_outcome,
                "the measurable result if adopted (the post-implementation acceptance contract)",
            )?;

            let now = chrono::Utc::now().to_rfc3339();
            let id = Uuid::new_v4().to_string();
            let h = Hypothesis {
                id: id.clone(),
                name: name.clone(),
                claim,
                proposal,
                predicted_outcome,
                status: "proposed".to_string(),
                author: author.clone(),
                evidence: String::new(),
                inspected_by: String::new(),
                last_inspected: String::new(),
                created_at: now.clone(),
                updated_at: now.clone(),
            };
            insert_hypothesis(&db, &h)?;

            let mut linked = Vec::new();
            for t in &targets {
                let iid = resolve_intent(&db, t)?;
                insert_targets(&db, &id, &iid, &now)?;
                linked.push(iid);
            }

            if printer.json {
                let mut v = serde_json::to_value(&h)?;
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("targets".to_string(), serde_json::json!(linked));
                    obj.insert("next_steps".to_string(), serde_json::json!([
                        format!("Prove it (a DIFFERENT agent, analyzer lane): `loom hypothesis prove {id} --verdict supported|refuted --evidence \"…\"`."),
                        "Link more affected intents: `loom hypothesis target <hypothesis> <intent>`.",
                    ]));
                }
                printer.print_json(&v);
            } else {
                println!("✓ Hypothesis proposed");
                println!("{}", fmt_hypothesis(&h));
                if !linked.is_empty() {
                    println!("  targets:     {} intent(s) linked", linked.len());
                }
                println!("  → Next: a DIFFERENT agent proves it — `loom hypothesis prove {id} --verdict supported|refuted --evidence \"…\"`.");
            }
        }

        HypothesisCmd::Target { hypothesis, intent } => {
            let hid = resolve_hypothesis(&db, &hypothesis)?;
            let iid = resolve_intent(&db, &intent)?;
            if crate::db::queries::get_targets_between(&db, &hid, &iid)?.is_some() {
                anyhow::bail!("Hypothesis already targets that intent — `loom hypothesis show {hid}` lists current targets.");
            }
            let now = chrono::Utc::now().to_rfc3339();
            insert_targets(&db, &hid, &iid, &now)?;
            let next_step = format!(
                "`loom hypothesis show {hid}` lists targets; `loom hypothesis prove {hid} …` when evidence is ready"
            );
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "edge_id": crate::db::schema::edge_key(crate::db::schema::edge::TARGETS, &hid, &iid), "hypothesis_id": hid, "intent_id": iid,
                    "next_step": next_step,
                }));
            } else {
                println!("✓ TARGETS edge created: {hid} → {iid}");
                println!("  → Next: {next_step}");
            }
        }

        HypothesisCmd::Prove {
            id,
            verdict,
            evidence,
            inspected_by,
        } => {
            // Proving is analyzer work: read the code, check the claim.
            let prover = gate::acting_in_lane(
                "prove a hypothesis",
                &[role::ANALYZER],
                inspected_by.as_deref(),
            )?;
            if !matches!(verdict.as_str(), "supported" | "refuted") {
                anyhow::bail!(
                    "--verdict must be 'supported' (the claimed problem is real) or 'refuted' \
                     (looked — it is not). Adoption/rejection are separate DECISIONS: \
                     `loom hypothesis adopt|reject`."
                );
            }
            gate::require_substantive(
                "evidence",
                &evidence,
                "what was actually found while checking the claim against the code",
            )?;
            let hid = resolve_hypothesis(&db, &id)?;
            let h = get_hypothesis(&db, &hid)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Hypothesis '{}' not found. Run `loom hypothesis list`.",
                    hid
                )
            })?;
            if matches!(h.status.as_str(), "adopted" | "confirmed" | "rejected") {
                anyhow::bail!(
                    "Hypothesis '{}' is already decided ({}) — the decision is recorded; \
                     propose a new hypothesis instead of re-litigating this one.",
                    h.name,
                    h.status
                );
            }
            // Proposer ≠ prover. Only enforceable when both declare roles —
            // bare llm/human is solo mode, which stays supported (same contract
            // as the lane gates).
            if gate::role_of(&h.author).is_some()
                && gate::role_of(&prover).is_some()
                && h.author == prover
            {
                anyhow::bail!(
                    "Separation of duties: '{}' proposed this hypothesis and cannot also prove it. \
                     Hand the proof to a different agent (or a different role).",
                    prover
                );
            }
            let now = chrono::Utc::now().to_rfc3339();
            let target_status = if verdict == "supported" {
                "passing"
            } else {
                "independent"
            };
            // Atomic: the verdict and every TARGETS stamp (a (2+N)-statement
            // mutation) land together or not at all — matches the Adopt path
            // and the multi-statement-mutation rule. A mid-loop failure must
            // not leave the hypothesis flipped over half-stamped targets.
            crate::db::with_transaction(&db, || {
                update_hypothesis_verdict(&db, &hid, &verdict, &evidence, &prover, &now)?;
                set_targets_status_for_hypothesis(
                    &db,
                    &hid,
                    target_status,
                    "hypothesis proof establishes whether this target is affected",
                    &evidence,
                    &prover,
                    &now,
                )?;
                Ok(())
            })?;
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "id": hid, "verdict": verdict,
                    "next_step": if verdict == "supported" {
                        format!("Decide (builder lane): spawn planned intents, then `loom hypothesis adopt {hid} --spawned <intent>…` — or `loom hypothesis reject {hid} --reason \"…\"`.")
                    } else {
                        format!("Close it out: `loom hypothesis reject {hid} --reason \"…\"` (refuted claims usually end here).")
                    },
                }));
            } else {
                println!("✓ Hypothesis '{}' → {}", h.name, verdict);
                if verdict == "supported" {
                    println!("  → Next: builder decides — spawn planned intents, then `loom hypothesis adopt {hid} --spawned <intent>…`, or reject with a reason.");
                } else {
                    println!("  → Next: `loom hypothesis reject {hid} --reason \"…\"` (refuted claims usually end here).");
                }
            }
        }

        HypothesisCmd::Adopt {
            id,
            spawned,
            reason,
        } => {
            // Adoption converts speculation into a promise to change the code —
            // builder lane, owned custody only.
            let by = gate::acting_in_lane("adopt a hypothesis", &[role::BUILDER], None)?;
            crate::db::queries::ensure_owned(
                &db,
                "adopt a hypothesis (a promise to change the code)",
            )?;
            let hid = resolve_hypothesis(&db, &id)?;
            let h = get_hypothesis(&db, &hid)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Hypothesis '{}' not found. Run `loom hypothesis list`.",
                    hid
                )
            })?;
            if h.status != "supported" {
                anyhow::bail!(
                    "Only a SUPPORTED hypothesis can be adopted — '{}' is '{}'. \
                     {}",
                    h.name,
                    h.status,
                    if h.status == "proposed" {
                        format!("Prove it first: `loom hypothesis prove {hid} --verdict … --evidence \"…\"`.")
                    } else {
                        "A refuted/decided hypothesis cannot become work.".to_string()
                    }
                );
            }
            if spawned.is_empty() {
                let Some(ref r) = reason else {
                    anyhow::bail!(
                        "Adoption must convert into work: pass --spawned <planned-intent> \
                         (create it first with `loom intent add --lifecycle planned`), or \
                         --reason explaining the conversion (e.g. targets marked needs_change)."
                    );
                };
                gate::require_substantive(
                    "reason",
                    r,
                    "how this adoption converts into work when no spawned intent is linked",
                )?;
            }
            let mut spawned_ids = Vec::new();
            let mut spawned_names = Vec::new();
            for s in &spawned {
                let iid = resolve_intent(&db, s)?;
                let i = crate::db::queries::get_intent(&db, &iid)?
                    .ok_or_else(|| anyhow::anyhow!("Intent '{}' not found. Run `loom intent list`; for --spawned the intent must exist first.", iid))?;
                spawned_ids.push(iid);
                spawned_names.push(i.name);
            }

            let now = chrono::Utc::now().to_rfc3339();
            // Atomic conversion: the status flip, the lineage notes (both
            // directions), the outcome Validation, and its VALIDATES links
            // are ONE decision — half an adoption is worse than none.
            crate::db::with_transaction(&db, || {
                set_hypothesis_status(&db, &hid, "adopted", &by, &now)?;

                // Lineage, both directions. On the hypothesis: what it became. On
                // each spawned intent: where it came from + the predicted outcome —
                // the acceptance contract the validator will eventually prove.
                let mut text = format!(
                    "adopted{}",
                    if spawned_names.is_empty() {
                        String::new()
                    } else {
                        format!(
                            ": spawned {}",
                            spawned_ids
                                .iter()
                                .zip(&spawned_names)
                                .map(|(i, n)| format!("'{n}' ({i})"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                );
                if let Some(ref r) = reason {
                    text.push_str(&format!(" — {r}"));
                }
                insert_note(
                    &db,
                    &Note {
                        id: Uuid::new_v4().to_string(),
                        kind: "decision".to_string(),
                        text,
                        author: by.clone(),
                        target_kind: "hypothesis".to_string(),
                        target_id: hid.clone(),
                        audience: String::new(),
                        created_at: now.clone(),
                    },
                )?;
                if !spawned_ids.is_empty() {
                    let validation_id = Uuid::new_v4().to_string();
                    insert_validation(
                        &db,
                        &Validation {
                            id: validation_id.clone(),
                            name: format!("hypothesis outcome: {}", h.name),
                            description: format!(
                                "hypothesis:{}\nPredicted outcome to verify after adoption: {}",
                                hid, h.predicted_outcome
                            ),
                            validation_type: "manual_check".to_string(),
                            command: String::new(),
                            last_run: String::new(),
                            last_result: "not_run".to_string(),
                        },
                    )?;
                    for iid in &spawned_ids {
                        insert_validates(
                            &db,
                            &validation_id,
                            iid,
                            "hypothesis outcome proof",
                            &now,
                        )?;
                    }
                }

                for iid in &spawned_ids {
                    insert_note(&db, &Note {
                    id: Uuid::new_v4().to_string(),
                    kind: "decision".to_string(),
                    text: format!(
                        "spawned from hypothesis '{}' ({}) — predicted outcome (acceptance contract): {}",
                        h.name, hid, h.predicted_outcome
                    ),
                    author: by.clone(),
                    target_kind: "intent".to_string(),
                    target_id: iid.clone(),
                    audience: String::new(),
                    created_at: now.clone(),
                })?;
                }
                Ok(())
            })?;

            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "id": hid, "adopted": true,
                    "spawned": spawned_ids,
                    "next_step": "The spawned planned intents are ordinary work now — `loom next --mode build` serves them.",
                }));
            } else {
                println!("✓ Hypothesis '{}' adopted.", h.name);
                for (i, n) in spawned_ids.iter().zip(&spawned_names) {
                    println!("  spawned: '{n}' ({i}) — carries the predicted outcome as its acceptance contract");
                }
                println!("  → The hypothesis plane is done with this one: `loom next --mode build` serves the spawned work.");
            }
        }

        HypothesisCmd::Reject { id, reason } => {
            let by = gate::acting_in_lane("reject a hypothesis", &[role::BUILDER], None)?;
            gate::require_substantive(
                "reason",
                &reason,
                "why this hypothesis is not being pursued",
            )?;
            let hid = resolve_hypothesis(&db, &id)?;
            let h = get_hypothesis(&db, &hid)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Hypothesis '{}' not found. Run `loom hypothesis list`.",
                    hid
                )
            })?;
            if matches!(h.status.as_str(), "adopted" | "confirmed") {
                anyhow::bail!(
                    "Hypothesis '{}' was adopted — its spawned intents are real work now. \
                     Retire/delete those intents if the direction is wrong; the hypothesis \
                     record stays as history.",
                    h.name
                );
            }
            let now = chrono::Utc::now().to_rfc3339();
            set_hypothesis_status(&db, &hid, "rejected", &by, &now)?;
            insert_note(
                &db,
                &Note {
                    id: Uuid::new_v4().to_string(),
                    kind: "decision".to_string(),
                    text: format!("rejected: {reason}"),
                    author: by,
                    target_kind: "hypothesis".to_string(),
                    target_id: hid.clone(),
                    audience: String::new(),
                    created_at: now,
                },
            )?;
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "id": hid, "rejected": true,
                    "next_step": "`loom next` for the next item",
                }));
            } else {
                println!("✓ Hypothesis '{}' rejected (decision recorded).", h.name);
                println!("  → Next: `loom next` for the next item");
            }
        }

        HypothesisCmd::List { status, limit } => {
            if let Some(ref s) = status {
                s.parse::<HypothesisStatus>()
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
            }
            let mut hs = list_hypotheses(&db, status.as_deref())?;
            let total = crate::output::apply_limit(&mut hs, limit);
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "hypotheses": hs,
                    "total": total,
                    "truncated": hs.len() < total,
                }));
            } else if hs.is_empty() {
                println!(
                    "(no hypotheses{})",
                    status
                        .map(|s| format!(" with status '{s}'"))
                        .unwrap_or_default()
                );
            } else {
                println!(
                    "  {status:>10}   {name:<40}  id",
                    status = "STATUS",
                    name = "NAME"
                );
                println!("  {}", "-".repeat(90));
                for h in &hs {
                    println!(
                        "  [{status:>10}]  {name:<40}  {id}",
                        status = h.status,
                        name = h.name,
                        id = h.id
                    );
                }
                if let Some(m) =
                    crate::output::more_marker(total, hs.len(), "loom hypothesis list --limit 0")
                {
                    println!("  {m}");
                }
            }
        }

        HypothesisCmd::Show { id } => {
            let hid = resolve_hypothesis(&db, &id)?;
            let h = get_hypothesis(&db, &hid)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Hypothesis '{}' not found. Run `loom hypothesis list`.",
                    hid
                )
            })?;
            let targets = list_targets_for_hypothesis(&db, &hid)?;
            let targets_total = targets.len();
            let mut notes = notes_for_target(&db, &hid)?;
            let notes_total = notes.len();
            if notes_total > crate::output::SECTION_CAP {
                // notes come oldest-first; keep the NEWEST.
                notes.drain(..notes_total - crate::output::SECTION_CAP);
            }
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "hypothesis": h,
                    "targets": targets,
                    "targets_total": targets_total,
                    "notes": notes,
                    "notes_total": notes_total,
                }));
            } else {
                println!("── Hypothesis ─────────────────────────────────────────────────────");
                println!("{}", fmt_hypothesis(&h));
                println!();
                println!(
                    "── Targets ({}) ─────────────────────────────────────────────────────",
                    targets_total
                );
                if targets.is_empty() {
                    println!(
                        "  (none — `loom hypothesis target {hid} <intent>` links affected intents)"
                    );
                } else {
                    for t in targets.iter().take(crate::output::SECTION_CAP) {
                        println!(
                            "  → {}  [{}]  ({})",
                            t.intent_name, t.inspection_status, t.intent_id
                        );
                    }
                    let shown = targets.len().min(crate::output::SECTION_CAP);
                    if let Some(m) = crate::output::more_marker(
                        targets_total,
                        shown,
                        &format!("full list: loom hypothesis show {hid} --json"),
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
                        &format!("loom note list --edge {hid}"),
                    ) {
                        println!("  {m}");
                    }
                }
            }
        }
    }
    Ok(())
}

fn fmt_hypothesis(h: &Hypothesis) -> String {
    let proved = if h.last_inspected.is_empty() {
        "(not yet proven)".to_string()
    } else {
        format!("{} by {}", h.last_inspected, h.inspected_by)
    };
    let evidence = if h.evidence.is_empty() {
        "(none yet)"
    } else {
        &h.evidence
    };
    format!(
        "  id:                {}\n  name:              {}\n  status:            {}\n  author:            {}\
         \n  claim:             {}\n  proposal:          {}\n  predicted_outcome: {}\
         \n  evidence:          {}\n  proven:            {}\n  created:           {}",
        h.id, h.name, h.status, h.author,
        h.claim, h.proposal, h.predicted_outcome,
        evidence, proved, h.created_at,
    )
}
