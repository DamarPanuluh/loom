---
type: module
title: "sync flag engine"
tags:
  - workflow
sourceFiles:
  - src/commands/sync.rs
symbols:
  - const REPORT_CAP
  - fn affected_intents
  - fn backfill_mechanical_kinds
  - fn build_sync_report
  - fn cap_json_arrays
  - fn codefile_changed
  - fn compact_transitions
  - fn compute_coupled_intent_pairs
  - fn flag_code_ripple_for_intents
  - fn flag_governs
  - fn flag_relates
  - fn flag_serves
  - fn flush_pending_hash_updates
  - fn group_intents_by_codefile
  - fn invalidate_delegation_validations
  - fn invalidate_validations
  - fn load_sync_data
  - fn next_sync_step
  - fn print_missing_files
  - fn print_stale_locators
  - fn print_sync_json
  - fn print_sync_report
  - fn print_sync_text
  - fn ripple_delegations
  - fn run_with_sqlite
  - fn scan_codefile
  - fn scan_files_and_flag_changes
  - fn update_physical_facts_and_flag_locators
  - pub fn run
  - struct CodeRippleTarget
  - struct ScannedCodeFile
  - struct SyncContext
  - struct SyncData
  - struct SyncState
  - type SqliteStore
provenance:
  src/commands/sync.rs: f88a57b61311bbf2
---

# sync flag engine

> mtime-delta detection propagating one hop: RELATES_TO neighbours and passing GOVERNS go needs_reverification, linked validations go not_run; files missing on disk are reported

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
