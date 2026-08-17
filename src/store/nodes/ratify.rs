use super::super::*;

impl Store {
    /// Redefine an intent's description — the semantic twin of `sync`. Ripples
    /// one hop: every settled asserted verdict touching the intent re-opens to
    /// needs_reverification, linked validations reset to not_run, completeness
    /// waivers are cleared (a waiver granted against the OLD meaning must be
    /// re-earned against the new one), and the old wording is preserved in a
    /// decision note. A name-only change does not call this (no ripple).
    /// Builder lane.
    pub fn redefine_intent(&self, id: &str, new_description: &str) -> Result<usize> {
        self.check_lane(registry::OwnerRole::Builder)?;
        let intent = self
            .get_node(id)?
            .ok_or_else(|| anyhow!("no intent '{id}'"))?;
        if intent.node_type != NodeType::Intent {
            bail!("'{id}' is not an intent");
        }
        if intent.status == "deprecated" {
            bail!("cannot redefine a deprecated intent");
        }
        // preserve old wording
        self.add_note(
            id,
            "decision",
            &format!("redefined; previous description: {}", intent.description),
        )?;
        let now = now(&self.conn)?;
        self.conn.execute(
            "UPDATE node SET description=?2,updated_at=?3 WHERE id=?1",
            params![id, new_description, now],
        )?;
        // A redefinition invalidates every completeness waiver: the reasons
        // were given for the previous meaning.
        let cleared = self.conn.execute(
            "DELETE FROM facet WHERE target_id=?1 AND target_kind='node' AND key LIKE 'waiver:%'",
            params![id],
        )?;
        if cleared > 0 {
            self.add_note(
                id,
                "decision",
                &format!("{cleared} completeness waiver(s) re-opened by redefinition"),
            )?;
        }
        // Wantedness rots with meaning: a ratified intent whose criterion
        // changed is no longer known-wanted. Stale the ratification exactly as
        // the loop below stales verdicts; the ratify queue re-serves it.
        if self.ratification(id)? == "ratified" {
            // Demotion, not authorization: no human is required to notice that
            // meaning drifted, and requiring one would mean stale wantedness
            // could only be spotted by the person it was hidden from.
            self.assert_fact(
                crate::store::Assertion::new(
                    crate::store::Subject::Node(id.to_string()),
                    crate::model::Claim::Ratification,
                    "needs_reconfirmation",
                    "sync",
                )
                .criterion("redefined after ratification")
                .cited(vec![crate::evidence::CitedEvidence::Claim(
                    "the criterion the authority approved was rewritten".into(),
                )]),
            )?;
            self.add_note(id, "ratify", "ratification staled by redefinition")?;
        }
        // ripple one hop: implements/targets/governs/validates/relationships touching it
        let cause = format!("intent '{}' description updated", intent.name);
        let mut reopened = 0usize;
        // Implements is Intent→CodeFile, so a grounding hangs off the FROM
        // side; the old to-side query never matched it, silently leaving
        // grounding verdicts settled across a redefinition (H-1). Targets/
        // governs/validates are X→Intent and hang off the TO side.
        for e in self.edges_with(Some(EdgeKind::Implements), Some(id), None)? {
            if self.edge_superseded(&e.id)? {
                continue; // a superseded grounding is history, not re-opened
            }
            if self.stale_edge(&e.id, &cause)? {
                reopened += 1;
            }
        }
        for k in [EdgeKind::Targets, EdgeKind::Governs, EdgeKind::Validates] {
            for e in self.edges_with(Some(k), None, Some(id))? {
                if self.stale_edge(&e.id, &cause)? {
                    reopened += 1;
                }
                if k == EdgeKind::Validates {
                    // A failed reset would leave the proof showing its old
                    // result while the command reports success (M-11) — surface it.
                    // loom-stability-exempt: resets a proof to not_run on ripple
                    self.set_node_status(&e.from_id, "not_run")?;
                }
            }
        }
        for k in [
            EdgeKind::Relates,
            EdgeKind::Requires,
            EdgeKind::ScenarioOf,
            EdgeKind::VariantOf,
            EdgeKind::Triggers,
            EdgeKind::Sequence,
        ] {
            for e in self.edges_with(Some(k), Some(id), None)? {
                if self.stale_edge(&e.id, &cause)? {
                    reopened += 1;
                }
            }
            for e in self.edges_with(Some(k), None, Some(id))? {
                if self.stale_edge(&e.id, &cause)? {
                    reopened += 1;
                }
            }
        }
        Ok(reopened)
    }

    /// Retire an intent: status → deprecated. Invisible to computation, visible
    /// to history. Builder lane.
    /// Ratify an intent: the human authority's evidence-bearing "yes, this is
    /// wanted". INV-8 is about who decides, not who types: an LLM lane may
    /// record an explicit mediated [`HumanDecision`], but may never supply the
    /// decision itself. The ordinary direct path remains denied to every lane.
    /// Record that a behavior is NOT wanted. Same boundary, same authority
    /// check, same journal — refusal is an act of the same kind as approval.
    pub fn reject_intent(&self, id: &str, reason: &str, presence: &str) -> Result<()> {
        let decision = crate::ratification::HumanDecision::direct(presence)?;
        self.apply_human_decision(id, "rejected", reason, &decision, None)
    }

    pub fn reject_intent_from_human(
        &self,
        id: &str,
        reason: &str,
        decision: &crate::ratification::HumanDecision,
    ) -> Result<()> {
        self.apply_human_decision(id, "rejected", reason, decision, None)
    }

    pub fn ratify_intent(&self, id: &str, evidence: &str, presence: &str) -> Result<()> {
        let decision = crate::ratification::HumanDecision::direct(presence)?;
        self.apply_human_decision(id, "ratified", evidence, &decision, None)
    }

    pub fn ratify_intent_from_human(
        &self,
        id: &str,
        evidence: &str,
        decision: &crate::ratification::HumanDecision,
    ) -> Result<()> {
        self.apply_human_decision(id, "ratified", evidence, decision, None)
    }

    pub fn ratify_intent_from_human_batch(
        &self,
        id: &str,
        evidence: &str,
        decision: &crate::ratification::HumanDecision,
        batch_id: &str,
    ) -> Result<()> {
        self.apply_human_decision(id, "ratified", evidence, decision, Some(batch_id))
    }

    pub fn ratify_pattern(&self, id: &str, evidence: &str, presence: &str) -> Result<()> {
        let decision = crate::ratification::HumanDecision::direct(presence)?;
        self.apply_human_decision(id, "ratified", evidence, &decision, None)
    }

    pub fn ratify_pattern_from_human(
        &self,
        id: &str,
        evidence: &str,
        decision: &crate::ratification::HumanDecision,
    ) -> Result<()> {
        self.apply_human_decision(id, "ratified", evidence, decision, None)
    }

    pub fn invalidate_pattern(&self, id: &str) -> Result<usize> {
        if self.ratification(id)? == "ratified" {
            self.assert_fact(
                crate::store::Assertion::new(
                    crate::store::Subject::Node(id.to_string()),
                    crate::model::Claim::Ratification,
                    "needs_reconfirmation",
                    "sync",
                )
                .criterion("pattern guidance or applicability changed")
                .cited(vec![crate::evidence::CitedEvidence::Claim(
                    "the guidance the human approved was rewritten".into(),
                )]),
            )?;
        }
        let mut reopened = 0;
        for edge in self.edges_with(Some(EdgeKind::Exemplar), Some(id), None)? {
            if self.stale_edge(&edge.id, "pattern guidance or applicability changed")? {
                reopened += 1;
            }
        }
        Ok(reopened)
    }

    /// Both halves of the authority — approval and refusal — through one gate.
    fn apply_human_decision(
        &self,
        id: &str,
        state: &str,
        evidence: &str,
        decision: &crate::ratification::HumanDecision,
        batch_id: Option<&str>,
    ) -> Result<()> {
        let presence = decision.presence();
        // Fail before journaling. The assertion boundary repeats this check so
        // no alternate caller can bypass it, but doing it here avoids leaving
        // a journal event for a write that was refused.
        if !decision.permits_mediated_recording() {
            self.require_human_authority()?;
        }
        // The prose anchors the WANT; the journal entry below anchors the ACT.
        // Both are required: without this check the journal ref loom writes
        // itself would make every ratification self-anchoring, which is the
        // circularity the whole evidence spine exists to refuse.
        if crate::model::is_placeholder(evidence) {
            bail!(
                "ratification needs substantive evidence: why this behavior is wanted \
                 (an utterance, a source doc, a decision)"
            );
        }
        let event = if state == "rejected" {
            "rejection"
        } else {
            "ratification"
        };
        // The journal entry is written FIRST, so the ref the fact cites is real
        // by construction rather than by convention. This is also what makes
        // "every ratified intent has a journal entry behind it" a checkable
        // invariant — the predicate that identifies the 39 facet-only
        // ratifications this graph carried from before the spine.
        let entry = self.append_journal(event, id, {
            let mut payload = serde_json::json!({
            "evidence": evidence,
            "ratified_by": "human",
            "presence": presence,
            "human_decision": decision,
            });
            if let Some(batch_id) = batch_id {
                payload["batch_id"] = serde_json::json!(batch_id);
                payload["decision_mode"] = serde_json::json!("batch");
            }
            let node = self
                .get_node(id)?
                .ok_or_else(|| anyhow!("no node '{id}'"))?;
            if node.node_type == NodeType::Pattern {
                payload["pattern_body"] = node.body;
            }
            payload
        })?;
        let mut cited = crate::evidence::cite(self.root(), evidence)?;
        cited.push(crate::evidence::CitedEvidence::Journal(entry.id.clone()));
        // Authority (INV-8), the deprecated check, and the evidence floor all
        // live at the boundary now — this function only shapes the assertion.
        let mut assertion = crate::store::Assertion::new(
            crate::store::Subject::Node(id.to_string()),
            crate::model::Claim::Ratification,
            state,
            "human",
        )
        .criterion(presence)
        .confidence(1.0)
        .cited(cited);
        if decision.permits_mediated_recording() {
            assertion = assertion.mediated_human_decision();
        }
        if let Some(batch_id) = batch_id {
            assertion = assertion.batch(batch_id);
        }
        self.assert_fact(assertion)?;
        // A mint-time ratification writes no note: the fact and the journal
        // entry already record that the minting act WAS the ratification, and a
        // note on every solo mint is pure audit-trail bloat.
        if presence != "mint" {
            self.add_note(id, "ratify", &format!("{state}: {evidence}"))?;
        }
        Ok(())
    }

    pub fn retire_intent(&self, id: &str, reason: &str, replaced_by: Option<&str>) -> Result<()> {
        self.check_lane(registry::OwnerRole::Builder)?;
        let intent = self
            .get_node(id)?
            .ok_or_else(|| anyhow!("no intent '{id}'"))?;
        if intent.node_type != NodeType::Intent {
            bail!("'{id}' is not an intent");
        }
        let note = match replaced_by {
            Some(r) => format!("retired: {reason} (replaced by {r})"),
            None => format!("retired: {reason}"),
        };
        self.add_note(id, "decision", &note)?;
        // loom-stability-exempt: retires a node
        self.set_node_status(id, "deprecated")?;
        Ok(())
    }
}
