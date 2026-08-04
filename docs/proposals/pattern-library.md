# Pattern library — implementation authority (B0–B5)

**Status:** approved and implemented. This document is normative.

## Contract

A `Pattern` is human-ratified, prescriptive, repository-specific guidance. Its
strict body contains `rationale`, `when_to_use`, `when_not_to_use`, and
`applicability` (`path_globs[]`, `intent_tags[]`). All three prose fields are
semantic and nonempty; unknown fields are rejected. Applicability may be empty,
which makes the pattern manual-only. A Pattern never persists a snippet,
excerpt, `active`, or `stale` field.

Draft capture may come from a human or model. Ratification is a fact-model
`ratification` claim and uses the same human-authority INV-8 seam as Intent
ratification: direct use retains the typed challenge; a host LLM may instead
record the human's exact answer with `--human-decision`. Normative body or applicability edits
demote ratification to `needs_reconfirmation` and reopen every exemplar verdict;
a name-only relabel does neither. Pattern facts do not participate in the
Intent-only maturity ladder.

## Exemplars and live truth

`Exemplar` is an asserted `Pattern → CodeFile` edge owned by the analyzer lane.
It requires a nonempty locator resolving **exactly one** symbol. Passing verdicts
use a dedicated locator-backed evidence floor; generic weak relationship evidence
must not settle them. Exemplar has no `GroundingRole`, never owns file coverage,
and is excluded from proof reach.

The source excerpt is derived from the current working tree when shown; it is
never stored or exported. A changed or missing symbol, ambiguous locator, stale
verdict, or unsynced working-tree edit fails closed. Unrelated edits in the same
file spare a symbol-scoped exemplar. Health is derived live as one of `draft`,
`manual_only`, `ungrounded`, `unreviewed`, `stale`, `routable`, `deprecated`.

## Lookup

Lookup accepts repository-relative paths and exact intent tags. It applies OR
within paths, OR within tags, and AND across populated selector families. Tags
are not inherited. Selectorless patterns are manual-only. The matcher lives in
the Pattern model module so later packet routing can reuse it unchanged.

## Automatic delivery

Build and fix WorkItems receive the same typed, live guidance used by `pattern
lookup`. Build uses the target Intent's tags; fix uses tags from every Intent
endpoint of the target edge. Both use the packet's complete final read set.
Non-coding packets receive no guidance. Every exemplar is rechecked against the
working tree at packet construction and a Pattern fails closed as a whole.

Delivery is deterministic: at most 5 exemplar items and 12 KiB total source
excerpt bytes. Counts report matched, included, and omitted items; byte clipping
has an explicit marker. The included exact lookup command recovers omitted
matches. Source text is presentation-only and is never persisted or exported.

Patterns do not reuse `Implements` or `Validates`,
do not route only from existing use sites, and add no maturity gate or rung.
Cross-graph Pattern federation is deferred.
