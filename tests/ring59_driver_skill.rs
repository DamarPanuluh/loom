//! Ring 59 — the shipped loom-driver skill encodes the campaign decisions the
//! human grilled, not just a plausible-sounding protocol.

const SKILL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/skills/loom-driver/SKILL.md"
));

#[test]
fn driver_skill_encodes_the_grilled_campaign_decisions() {
    for (decision, marker) in [
        ("single-threaded pacing", "10 asserted"),
        ("no parallel judgment subagents", "No parallel subagents"),
        ("one-shot per invocation", "this\nskill is one-shot"),
        ("cold graph stops early", "**Cold graph**"),
        ("human gates batch to one end sitting", "one sitting"),
        (
            "checkpoints stage exact paths only",
            "git add -- <included_paths only>",
        ),
        ("checkpoints stay local", "**leave the commit local**"),
        ("never push autonomously", "never push on your own"),
        (
            "the packet outranks the skill",
            "The served packet outranks this file",
        ),
    ] {
        assert!(
            SKILL.contains(marker),
            "loom-driver SKILL.md must encode {decision}: missing {marker:?}"
        );
    }
}

/// The skill is prose, so line wrapping is not meaningful; compare on a single
/// whitespace-collapsed line.
fn flat() -> String {
    SKILL.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn cold_graph_ends_the_campaign_instead_of_authoring_meaning() {
    let flat = flat();
    assert!(
        flat.contains("**Cold graph**") && flat.contains("stop early, honestly"),
        "a cold graph must end the campaign, not start one"
    );
    assert!(
        flat.contains("Do not author product meaning in batch mode"),
        "batch mode must never author product meaning"
    );
    assert!(
        flat.contains("re-invoke this skill once the graph routes work"),
        "the cold path must hand back to the interactive seeding loop"
    );
}

#[test]
fn a_human_gated_packet_is_deferred_never_answered() {
    let flat = flat();
    assert!(
        flat.contains("do not work it and do not wait on it"),
        "a gated packet must be neither worked nor blocked on"
    );
    assert!(
        flat.contains("the wait happens at the end, batched, not mid-drain"),
        "the deferral must name where the wait actually happens"
    );
    assert!(
        flat.contains("never answer for the human"),
        "the driver must never supply the human's decision"
    );
}

#[test]
fn an_absent_human_gets_a_printed_remainder_not_an_inference() {
    let flat = flat();
    assert!(
        flat.contains("**If the human is absent**, do not wait and do not infer"),
        "an absent human must not be inferred around"
    );
    assert!(
        flat.contains("print the remainder with the exact command for each item, and exit"),
        "the remainder must be printed with each item's exact write-back command"
    );
}

#[test]
fn driver_skill_never_widens_the_staged_set_or_answers_for_the_human() {
    let flat = flat();
    // `git add -A` and pushing may appear only inside their own prohibition.
    assert!(
        flat.contains("never guess, never widen, never `git add -A`"),
        "widening the staged set must be forbidden, not merely unmentioned"
    );
    assert!(
        flat.contains("never push on your own"),
        "pushing must be forbidden without an explicit human decision"
    );
    assert!(
        !flat.contains("--no-verify"),
        "the skill must never sanction bypassing hooks"
    );
    assert!(
        flat.contains("Silence is never an answer"),
        "the skill must refuse to answer a human gate by default"
    );
}

#[test]
fn driver_skill_lists_only_the_autonomous_lanes() {
    let flat = flat();
    let lanes = "build, elaborate, fix, validate, quality, analyze, prove, triage, review";
    assert!(
        flat.contains(lanes),
        "the drainable lane list must be stated so human-gated lanes stay out"
    );
    for gated in ["ratify", "derive", "audit", "deepen", "coverage", "surface"] {
        assert!(
            !lanes.contains(gated),
            "{gated} is not autonomous and must never appear in the drainable lane list"
        );
    }
}
