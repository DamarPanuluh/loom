# loom — Rethink: the lived graph

**Status:** PARTIALLY LANDED — 2026-07-18. Rings 1–2 of §8 are implemented (provenance +
ratification + INV-8, the `wanted` rung, `loom next --mode ratify`, grammar served via
`loom schema`); `design.md` §5/§6 now record the `Wanted` rung and INV-8. Rings 3–5 (journal,
journey baselines, drive mode) remain design-only.
Written under a grant of authority to rethink the whole system, holding one constraint: stay
faithful to the dream — *a falsifiable graph of what a codebase is supposed to do, where that
lives, and how it is proven, so an LLM can understand and safely evolve a codebase across a
long horizon.*

The conclusion up front: the v2 spine (three truth-class planes, evidence-gated writes, sync
staleness, honest maturity) survives contact with the rethink intact. What changes is **where
truth enters the system** — and one blind spot the spine inherited from v1 without noticing.

---

## 1. The blind spot: intents are the one unfalsifiable thing in a falsifiable graph

v2's organizing law is that every fact carries a truth class and can be re-opened by evidence.
Groundings stale. Verdicts stale. Proofs re-run. Facets carry provenance. Everything is
falsifiable — **except the intent itself.** An intent, once minted, is an axiom: the graph
records *whether the code does X* with full rigor, but records *whether anyone actually wants X*
as nothing more than a `confirmed_at` timestamp and the social convention that a human typed it.

Every awkward rule in the current system is scar tissue around this blind spot:

- The LLM driver doc must plead in prose: *"never invent an entire spine as `implemented`"*,
  *"do not answer product questions for the human"* — because once an LLM-minted intent exists,
  it is indistinguishable from a human-wanted one.
- `loom bootstrap suggest` needs a Proposal/adopt ceremony as a airlock, because direct minting
  is unsafe.
- Human-in-the-loop lives in side channels — `confirmed_at`, `Question` nodes — instead of on
  the intent as evidence.

The fix is not more ceremony. It is to make **wantedness a fact like any other**: asserted,
evidence-bearing, stale-able, routed when missing. Once wantedness is a separate gated fact,
*authorship becomes cheap* — anyone, human or LLM, may mint an intent, because minting no longer
smuggles in ratification.

## 2. The core move: ratification

Every `Intent` gains two things:

**Provenance (immutable, stamped at birth):**

```text
origin:        human | llm | drive | import      # who minted it
formalized_by: (agent string)                    # who phrased/decomposed it
```

**Ratification (an asserted, evidence-bearing, stale-able state):**

```text
ratification:  unratified | ratified | rejected | needs_reconfirmation
ratified_by:   (human identity)
ratified_at:   timestamp
utterances:    [] journal refs — the recorded human expressions of this want
```

Rules, riding machinery that already exists:

1. **Anyone may mint; only a human may ratify.** `loom intent add` is open to every lane —
   an `llm:*` agent minting an intent is normal, not an exception. `loom intent ratify` is the
   **one write in the system rejected for every `llm:*` lane** (INV-7 extended; unknown agents
   already fail closed). This replaces paragraphs of "seeding-mode" prose with a single
   write-boundary rule.
2. **Ratification needs evidence** (INV-6 applies): a journal ref, an utterance, a source doc —
   *why do we believe this is wanted.* Human-minted intents are born ratified with the minting
   utterance as evidence. LLM-minted intents are born `unratified`.
3. **Sync stales ratification** exactly as it stales verdicts: a material change to the intent's
   description or criterion moves `ratified → needs_reconfirmation`. Wantedness rots like every
   other asserted fact; loom notices.
4. **Unratified ≠ hidden.** An unratified intent is a full graph citizen — groundable, provable,
   queryable, decomposable. It is simply *failing* the ladder (below). The LLM can build out an
   entire hypothesized spine overnight; the human wakes up to a ratification queue, not a fait
   accompli.

The division of labor collapses into one symmetric sentence:

> **The LLM may author everything and ratify nothing. The human must author nothing and may
> ratify everything.**

That sentence *is* the completed concept: human-in-the-loop stops being a workflow convention
and becomes a write permission on a single state, recorded on the intent node with evidence.

## 3. The ladder gains a rung: `wanted`

```text
seeded → wanted → realized → proven → hardened → excellent → exported
```

- **`wanted`** — every active intent is ratified (none `unratified`/`needs_reconfirmation`).
  Sits between seeded and realized: an unratified spine correctly blocks everything above it,
  and the compass — lowest unmet rung — routes the human to ratification as *the* next move.
- `loom next --mode ratify` serves the queue. It is the canonical **human-presence queue**
  (`llm-driver.md` already demands loom distinguish these; now the distinction is structural:
  the ratify queue is precisely the work no LLM lane can write).
- This is the user's "LLM can add intents anyway — but still failing": minted freely, first-class
  immediately, and honestly red on the ladder until a human says *yes, wanted* and a proof says
  *yes, true*. Two independent failure axes:

| | unproven | proven |
|---|---|---|
| **unratified** | hypothesis (LLM speculation, cheap to reject) | **discovered behavior** — the code does something nobody asked for. Ratify it or deprecate it; either way the graph learns. |
| **ratified** | classic backlog (build queue) | done |

The bottom-left-to-top-right diagonal is the normal build flow. The top-right-of-unratified cell
is new and valuable: code archaeology by the LLM surfaces real behaviors as unratified intents,
and ratification review doubles as scope audit.

## 4. Drive mode: intents harvested from use, not authored from speculation

This is the cockpit-skill insight folded all the way in. v2 grows its graph like a librarian —
catalog first, use later. The rethink adds the flight-recorder path: **use first, catalog falls
out.**

`loom drive` opens a recorded session. Three parties, exactly as in cockpit: the human plays
the user, the LLM plays the frontend, the system under test is the codebase (its CLI, its API,
its journeys). Per exchange:

1. Human utters an intent in natural language ("capture a topic and route it").
2. The LLM matches the utterance against existing intents (`loom find`) — or mints a new one.
   An intent minted from a live human utterance gets `origin=drive` and is **born ratified**:
   the utterance *is* the ratification evidence, and it is recorded verbatim in the journal.
3. The LLM executes real commands. No mocks, no invented endpoints — a failing backend is
   rendered as a failure (cockpit hard rule 2, adopted wholesale, including the read-back rule:
   an error response does not prove the write failed).
4. The exchange — utterance, matched intent, commands, verbatim output, outcome — is appended
   to the journal and linked to the intent.

Consequences that complete the loop:

- **Human-in-the-loop is literally recorded on the intent node** — not as a timestamp but as
  the utterance trail. `confirmed_at` is subsumed: confirmation is the last ratifying evidence
  event.
- **Journeys are frozen drives.** A chain that proved useful gets `loom drive freeze <name>`:
  the recorded exchanges compile into a `journeys/*.yaml` spec, with the verbatim transcript
  stored as its **baseline**. Journeys stop being hand-authored guesses about usage; they are
  usage, made deterministic.
- **A failed drive step is a Finding** with a journal ref — cockpit's `findings.md`, already a
  first-class loom node.

## 5. The journal: evidence becomes append-only and citable

A new store, `.loom/journal/` (append-only, INV-9 below): every drive exchange, proof run,
and ratification event, verbatim. Evidence fields across the graph may cite `journal:<ref>`.

This upgrades journey proofs from binary to differential:

- **Replay compares against the baseline transcript**, not just `expect:` clauses. Deviations
  (drifted output, new warnings, latency cliffs) are reported even when exit codes still pass —
  far stronger S3-or-stronger journey-proof evidence.
- **Lazy repair fork on deviation** (cockpit's rule, loom's machinery): either the tool changed
  — update the journey and re-freeze the baseline, journaled — or it regressed — mint a Finding.
  The same stale-or-wrong fork sync already applies to groundings, extended to journeys.

## 6. What does NOT change

The rethink is faithful because the dream's machinery already carries it:

| spine element | fate |
|---|---|
| three truth-class planes | unchanged — ratification is just another asserted fact |
| `sync` / staleness | unchanged — gains one more thing to stale |
| `next` as the only required-work queue | unchanged — gains `--mode ratify` |
| `debt` advisory feed | untouched |
| Finding / CodeRule / QualityRule | untouched; drives mint Findings through the front door |
| evidence gate (INV-6), lane gate (INV-7) | unchanged; ratification is their showcase |
| journeys as Validations | kept; upgraded from authored+binary to harvested+differential |
| bootstrap Proposal flow | kept for bulk spines, but no longer load-bearing for safety |

New invariants:

- **INV-8 — Ratification is human-only.** `intent ratify` (and any future ratifying write) is
  rejected for every `llm:*` lane; unknown agents fail closed. No flag overrides it.
- **INV-9 — Provenance is immutable; the journal is append-only.** `origin` is never rewritten;
  journal entries are never edited or deleted. History doesn't get rewritten.

## 7. Migration of the existing dogfood graph

The 34 curated intents predate ratification. Two options:

1. **Bulk-grandfather** (recommended): `origin=import`, ratified in one audited pass with
   evidence "v2 curated dogfood graph, human-reviewed through 2026-07"; one Note records the
   grandfathering. Avoids a 34-item wall of fake work.
2. **Dogfood the queue**: leave them unratified and drain `--mode ratify` for real. Honest but
   noisy; the utterance evidence would be reconstructed, not recorded.

## 8. Build sequencing (small rings, each green before the next)

1. **Provenance + ratification state** on Intent; `loom intent ratify` with lane gate (INV-8)
   and evidence gate; birth rules per origin; migration command.
2. **Ladder + routing:** the `wanted` rung; `loom next --mode ratify`; sync stales ratification
   on material description change.
3. **Journal store:** append-only `.loom/journal/`, `journal:<ref>` citations, INV-9 tests.
4. **Journey baselines:** freeze verbatim transcripts; replay reports deviations; lazy-repair
   fork (re-freeze vs Finding).
5. **Drive mode:** `loom drive` record loop (match/mint → execute → journal → link), then
   `loom drive freeze` compiling exchanges into `journeys/*.yaml` + baseline.

Rings 1–2 alone deliver the user-visible concept ("LLM mints freely, still failing until a
human ratifies"). Rings 3–5 complete the lived-graph loop.

## 9. Horizon — candidates beyond the current debt list (deliberately unplanned)

The 2026-07-18 debt round (8 planned intents in the graph, unratified) covers grammar
unification, find-ability, ratification provenance, `loom context`, sync freshness, and rings
3–5. What comes after is NOT planned yet, on purpose: the drive/context/journal rings will make
the real gaps observable, and the next round should be derived from that lived evidence — not
from speculation. Candidates to re-evaluate then, in no order:

- **Harness-native delivery** — loom as an MCP server (or equivalent), so an LLM pulls
  `loom context` packets in-band instead of shelling out. The biggest lever on "ready whenever."
- **Hypothesis loop maturity** — propose → prove → adopt for *changes* (the milestone-2 nodes
  exist; the loop around them is thin).
- **Context efficacy measurement** — a dogfood metric for whether `loom context` packets
  actually change LLM outcomes (packets cited in successful work vs ignored). Without this,
  "code intelligence" is a claim, not a fact — which would be very un-loom.
- **Federation maturity** — the upstream-intent machinery is young; multi-repo/team wantedness
  raises new ratification questions (whose authority ratifies a shared intent?).
- **InterfaceSurface completion** — public seams as first-class context.

Rule for the next round: open it by reading `loom status`, the journal, and the deviation log —
then write the round's intents from what they show.

---

*Faithfulness check, stated once: the dream was never "a human curates a graph an LLM consults."
It was truth that doesn't rot, ranked honestly, so both parties can act safely. The rethink
moves the human from curator to authority, the LLM from petitioner to author, and lets lived
usage — not speculation — grow the graph. Every mechanism it needs already exists in the spine;
that is how we know it is the same dream.*
