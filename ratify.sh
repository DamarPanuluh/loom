#!/usr/bin/env bash
# Ratification runner — you decide, your clipboard types.
#
# Before each prompt the intent's full name is copied to your clipboard, so
# confirming is Cmd-V then Enter. The evidence is already written, from the
# answers you gave. Nothing here can write a ratification without you: the
# tty gate and the challenge are still doing their job, you are just not
# retyping 60 characters to prove you are awake.
#
# Skip one by pressing Enter on an empty line (or typing anything else).
set -uo pipefail
B="${LOOM_BIN:-loom}"
ok=0; skip=0

printf "\n=== %s/22 =====================================\n" 1
printf '  %s\n' 'an operator ratifies an intent as wanted'
printf '%s' 'an operator ratifies an intent as wanted' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify 0950a847d7afcc73ed56c8e21a6607af --evidence 'wanted, and ratified by exercising it: the tty gate and typed challenge are what make this record mean a human was present. An LLM was refused this exact write minutes before I ran it, citing INV-8 and finding 62b197cc.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 2
printf '  %s\n' 'paginated list output tells callers how to continue'
printf '%s' 'paginated list output tells callers how to continue' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify 09db358fc80d90bb8d329b0e6f459d9e --evidence 'wanted: a list that does not say whether more exists forces every caller to guess or over-fetch. Exposing items, total, window, has_more and next_offset is what lets an agent page without inventing a convention.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 3
printf '  %s\n' 'loom is reachable in-band as tools, not only as a subprocess'
printf '%s' 'loom is reachable in-band as tools, not only as a subprocess' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify 225041d7f794fda26514239a1cc1d1b4 --evidence 'wanted, secondarily: the CLI is the primary and authoritative surface. MCP is a convenience for in-process agents and must never diverge from the CLI'\''s behaviour — it wraps the same functions, and that is the only reason it is safe to keep.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 4
printf '  %s\n' 'external research is routed with durable advisory provenance'
printf '%s' 'external research is routed with durable advisory provenance' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify 3ce98ed5ec61f4b5b714f8d3297f742a --evidence 'wanted: loom'\''s value is that its records are checkable. Outside knowledge cannot be, so it enters as ADVISORY with the page actually read, an exact quote, a fingerprint and an expiry — and never becomes a verified fact about this code.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 5
printf '  %s\n' 'loom maintains a falsifiable graph for LLM-driven codebase work'
printf '%s' 'loom maintains a falsifiable graph for LLM-driven codebase work' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify 418b5ebb5624aafa8df49c4f826c1d20 --evidence 'wanted: this is the whole point. Durable memory an LLM drives, where every claim points at something re-checkable and rots when that thing moves.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 6
printf '  %s\n' 'wantedness is earned from evidence, not demanded up front'
printf '%s' 'wantedness is earned from evidence, not demanded up front' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify 483fd508eaa6a27648a6ce86b54fe6f2 --evidence 'wanted: demanding a yes before anything exists is how the old wanted rung became a wall of 51 prompts, 39 of them answered by fabrication. Earning it from evidence is the correction.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 7
printf '  %s\n' 'an operator captures a topic through door and routes it from the landing menu'
printf '%s' 'an operator captures a topic through door and routes it from the landing menu' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify 4eefda1c44aaba4f1cb78be93db5712a --evidence 'wanted: a thought that has nowhere to land is lost. One entrance that accepts a raw utterance and routes it later is what keeps capture cheap enough to actually happen.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 8
printf '  %s\n' 'proof strength is derived from the proof'\''s shape'
printf '%s' 'proof strength is derived from the proof'\''s shape' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify 82b78e81c798713d558ca1ca67faee3f --evidence 'wanted: a proof'\''s grade must come from what it demonstrably does — that it ran, asserted content, and reached the code it proves — not from a label someone typed on it.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 9
printf '  %s\n' 'find surfaces each matched intent'\''s grounding'
printf '%s' 'find surfaces each matched intent'\''s grounding' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify 873d9c05264943d486f1535988af1765 --evidence 'wanted: a search result that names a behaviour without saying where it lives makes the reader open the graph again to finish the question.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 10
printf '  %s\n' 'the human is asked only where judgment and evidence disagree'
printf '%s' 'the human is asked only where judgment and evidence disagree' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify 8e6ace5eeb00d56ac7f81f83b9aa54ed --evidence 'wanted: this is what keeps the human queue small enough to actually read. Everything the evidence settles alone must never reach a person.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 11
printf '  %s\n' 'loom can fail its own falsifiability check'
printf '%s' 'loom can fail its own falsifiability check' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify a2fd4889adcbef6ceba52d1229a317ff --evidence 'wanted: a checker that cannot fail itself is decoration. The self-audit found real bursts in this graph, including one I created, and refused to let me hide it.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 12
printf '  %s\n' 'the next router serves the highest-priority asserted residue with a prompt contract'
printf '%s' 'the next router serves the highest-priority asserted residue with a prompt contract' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify a665d1c5fda0a34600d81165c1ceedc6 --evidence 'wanted: this drove an entire session'\''s work. Being handed the next correct unit with its role, its allowed and forbidden actions and its required evidence is what lets a worker act without holding a plan.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 13
printf '  %s\n' 'ratified live patterns guide build and repair work'
printf '%s' 'ratified live patterns guide build and repair work' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify aa3acab2245c17c66e98b4d734c3f4c5 --evidence 'wanted: a convention nobody reads is not a convention. Patterns put house style into the packet at the moment work happens, backed by exemplars from live code that rot when that code moves — so guidance cannot quietly go stale.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 14
printf '  %s\n' 'loom writes the graph from the work rather than asking for it twice'
printf '%s' 'loom writes the graph from the work rather than asking for it twice' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify b90a37b6b4afa1035a2ef2ef8630f2a6 --evidence 'wanted: a graph maintained by a second bookkeeping step is a graph that drifts. Deriving it from what the work already produced is what keeps it honest without extra ceremony.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 15
printf '  %s\n' 'ordered steps are served in order, one readiness gate not a task list'
printf '%s' 'ordered steps are served in order, one readiness gate not a task list' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify bcfca1ac56612642e6d9e20c114a7b62 --evidence 'wanted: ordered work had no expression — sequence was declarable and inert. Expressing order as a readiness constraint rather than a queue keeps one router instead of a plan that can drift from the graph.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 16
printf '  %s\n' 'loom runs proofs and reports what it observed'
printf '%s' 'loom runs proofs and reports what it observed' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify be6570d7348fa6fe2f64b70f8b6d3d60 --evidence 'wanted: the caller must never supply the outcome. loom running the command and recording what it saw is the difference between a proof and a claim about a proof.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 17
printf '  %s\n' 'a change re-opens exactly the claims that pointed at what changed'
printf '%s' 'a change re-opens exactly the claims that pointed at what changed' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify c24c2964b5b7da2c39b27532236b7952 --evidence 'wanted: demonstrated repeatedly this session. Every edit I made re-opened precisely the claims grounded in what changed and nothing else — that precision is what makes the re-proving affordable.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 18
printf '  %s\n' 'loom answers what a change here could reach'
printf '%s' 'loom answers what a change here could reach' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify cde73673e54340c65e7e91db1c0cc44b --evidence 'wanted: knowing the blast radius before editing is the difference between a refactor and a gamble. Exact and heuristic resolutions reported separately, never blended.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 19
printf '  %s\n' 'loom answers what other behaviors stand on this one'
printf '%s' 'loom answers what other behaviors stand on this one' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify dfca4ba16e3a816c05754855a8c2e35d --evidence 'wanted: loom could answer blast radius for code and not for behaviour. Knowing which behaviours stand on this one — and which of them nothing would catch breaking — is the question you ask before touching anything.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 20
printf '  %s\n' 'divergences route to a human-only queue'
printf '%s' 'divergences route to a human-only queue' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify e56229425a63eeb67ae20ca43cf10c1b --evidence 'wanted: where judgment and evidence disagree is exactly where a machine must stop. Routing only those to a person is what makes the human queue meaningful rather than a backlog.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 21
printf '  %s\n' 'changing a file re-opens the asserted edges grounded in it'
printf '%s' 'changing a file re-opens the asserted edges grounded in it' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify f2d36098dd6d84a9b2d81b316e3ded3d --evidence 'wanted: a verdict that survives the code it described is a lie with a timestamp. Re-opening on change is what makes a green graph mean something.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi
printf "\n=== %s/22 =====================================\n" 22
printf '  %s\n' 'a green graph is pointed at its weakest standing claim'
printf '%s' 'a green graph is pointed at its weakest standing claim' | pbcopy
printf '  (name copied — Cmd-V then Enter to confirm, or Enter alone to skip)\n'
if "$B" intent ratify ff5d38e1d8e842cecd760ea1e4d3f9d7 --evidence 'wanted: a green graph that cannot say what is weakest is just a green light. The deepening queue re-orders forever rather than draining, which is deliberate — a graph is never finished, only currently-weakest-somewhere.'; then ok=$((ok+1)); else skip=$((skip+1)); printf "  skipped\n"; fi

printf "\nratified=%s skipped=%s\n" "$ok" "$skip"
printf "next: tell Claude, or run: loom sync && loom status\n"
