---
type: module
title: "priority-scored work queues"
tags:
  - workflow
sourceFiles:
  - src/commands/cluster.rs
  - src/commands/next/refactor.rs
symbols:
  - run_refactor
provenance:
  src/commands/cluster.rs: 3648c72ec7c6b3b2
  src/commands/next/refactor.rs: c85673c5ca3c63f8
---

# priority-scored work queues

> loom next: one queue per agent role (discovery/fix/build/validate/quality), scored by centrality + urgency - staleness; returns one item with full context so no second lookup is needed

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
