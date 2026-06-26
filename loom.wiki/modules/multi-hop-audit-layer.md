---
type: module
title: "multi-hop audit layer"
tags:
  - analysis
sourceFiles:
  - src/db/queries/graph_algo.rs
symbols:
  - BRIDGE_WEIGHT
  - apply_code_ripples
  - compute_smells_from_parts
  - detect_intent_island
  - detect_reciprocal_dependency
  - detect_transitive_layering_violation
  - fn a_reciprocal_grounded_pair_is_a_dependency_cycle
  - pub fn betweenness
  - pub fn ripple_bump_by_intent
provenance:
  src/db/queries/graph_algo.rs: 0f916d16b2a862a0
---

# multi-hop audit layer

> Derived audit signals computed over arbitrary-depth traversals of the intent and import graph (RELATES_TO cycles, transitive layering violations, intent islands, bridge centrality), surfaced via smells and graph_state without putting heavy traversal on the per-turn hot path

## Responsibility

_(LLM-authored: what this module does and why it exists.)_

## Key entry points

_(LLM-authored: the main entry symbols and where to start reading.)_

## Key flows

_(LLM-authored: how a request travels through this module.)_

## Common modification points

_(LLM-authored: where to look when changing this module's behavior.)_

## Risk points

_(LLM-authored: gotchas, edge cases, and failure modes.)_


<!-- loom:prose-start -->




<!-- loom:prose-end -->
