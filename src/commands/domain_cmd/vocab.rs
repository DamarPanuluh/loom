use super::*;

pub(crate) fn vocab(graph: Option<&Path>, cmd: VocabCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        VocabCmd::Add { term, why } => {
            store.add_vocab_term(&term, &why)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "term": term,
                    "why": why,
                }),
                "loom status",
                format!("registered vocab term '{term}'"),
            )?;
            Ok(())
        }
        VocabCmd::Remove { term } => {
            store.remove_vocab_term(&term)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "removed": true,
                    "term": term,
                }),
                "loom status",
                format!("removed vocab term '{term}' (and untagged any nodes carrying it)"),
            )?;
            Ok(())
        }
        VocabCmd::Rename { from, to, reason } => {
            if reason.trim().is_empty() {
                bail!("vocab rename needs substantive --reason");
            }
            let from = from.trim();
            let to = to.trim();
            if from.is_empty() || to.is_empty() {
                bail!("vocab terms must not be empty");
            }
            if from == to {
                bail!("vocab rename needs distinct <from> and <to> terms");
            }
            let terms = store.list_vocab()?;
            let from_why = terms
                .iter()
                .find(|(term, _)| term == from)
                .map(|(_, why)| why.clone())
                .ok_or_else(|| anyhow!("no vocab term '{from}'"))?;
            let to_existing = terms.iter().any(|(term, _)| term == to);
            if !to_existing {
                store.add_vocab_term(to, &from_why)?;
            }
            let tags = store.snapshot()?.tags;
            let mut retagged = 0usize;
            for tag in tags.iter().filter(|tag| tag.term == from) {
                store.set_tag(&tag.target_id, tag.target_kind, to)?;
                store.remove_tag(&tag.target_id, tag.target_kind, from)?;
                retagged += 1;
            }
            store.remove_vocab_term(from)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "from": from,
                    "to": to,
                    "merged": to_existing,
                    "retagged": retagged,
                    "reason": reason,
                }),
                "loom status",
                format!("renamed vocab term '{from}' → '{to}' ({retagged} tag(s) moved)"),
            )?;
            Ok(())
        }
        VocabCmd::List => {
            let terms = store.list_vocab()?;
            if json {
                let rows: Vec<_> = terms
                    .iter()
                    .map(|(term, why)| serde_json::json!({ "term": term, "why": why }))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for (term, why) in terms {
                    println!("{term}  — {why}");
                }
            }
            Ok(())
        }
    }
}
