use crate::model::Node;
use crate::store::Store;
use crate::Result;

/// Machine pre-screen: run the rule's authored regex patterns over the
/// intent's grounded files. Computed on read at packet-build time, never
/// stored — hits are candidates for the LLM to confirm or refute, mirroring
/// how debt clusters are computed rather than persisted.
/// What a pattern pre-screen actually did.
///
/// An empty hit list is ambiguous — it means both "no patterns, nothing ran"
/// and "loom scanned and found nothing". Only the second is evidence, and it is
/// the evidence that answers an ABSENCE rule ("no hardcoded secrets here"), so
/// it has to be distinguishable.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct PreScreen {
    pub ran: bool,
    pub patterns: usize,
    pub files: usize,
    pub hits: Vec<crate::prescan::PreScreenHit>,
    /// Hits a recorded adjudication already answered (`loom rule suppress`).
    /// Filtered out of `hits` so a packet never re-litigates a judged false
    /// positive; counted so the suppression is visible, not silent.
    #[serde(default)]
    pub suppressed: usize,
}

pub(in super::super) fn prescreen_for(
    store: &Store,
    rule: Option<&Node>,
    intent_id: &str,
) -> Result<PreScreen> {
    let Some(rule) = rule else {
        return Ok(PreScreen::default());
    };
    let patterns: Vec<String> = rule
        .body
        .get("patterns")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if patterns.is_empty() {
        return Ok(PreScreen::default());
    }
    let mut files = Vec::new();
    // Pre-screen every realizing file: the grounding-file set is intentionally
    // unbounded, while the retained hit set is bounded by PRESCREEN_HIT_CAP.
    for e in store.realizing_groundings(intent_id)? {
        if let Some(cf) = store.get_node(&e.to_id)? {
            files.push(cf.name);
        }
    }
    if files.is_empty() {
        return Ok(PreScreen::default());
    }
    let hits = crate::prescan::prescreen(
        store.root(),
        &files,
        &patterns,
        crate::runner::PRESCREEN_HIT_CAP,
    )?;
    // Drop hits a recorded adjudication already answered: judged once by
    // content hash, they are never re-served for the same matched text.
    let mut open = Vec::with_capacity(hits.len());
    let mut suppressed = 0usize;
    for h in hits {
        if store.is_hit_suppressed(&rule.name, &h.excerpt)? {
            suppressed += 1;
        } else {
            open.push(h);
        }
    }
    Ok(PreScreen {
        ran: true,
        patterns: patterns.len(),
        files: files.len(),
        hits: open,
        suppressed,
    })
}
