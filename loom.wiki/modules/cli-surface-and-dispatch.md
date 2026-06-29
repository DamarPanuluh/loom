---
type: module
title: "CLI surface and dispatch"
tags:
  - cli
sourceFiles:
  - build.rs
  - src/cli.rs
  - src/commands/batch.rs
  - src/commands/codefile.rs
  - src/commands/edge.rs
  - src/commands/export.rs
  - src/commands/ignore.rs
  - src/commands/import.rs
  - src/commands/init.rs
  - src/commands/intent.rs
  - src/commands/mod.rs
  - src/commands/note.rs
  - src/commands/paths.rs
  - src/commands/resolve.rs
  - src/commands/rule.rs
  - src/commands/validate.rs
  - src/commands/validation.rs
  - src/commands/whoami.rs
  - src/db/mod.rs
  - src/db/queries/intent.rs
  - src/db/queries/scoring.rs
  - src/db/sqlite/writes.rs
  - src/main.rs
symbols:
  - const CONCURRENCY_PACK
  - const DATA_PACK
  - const DOCKER_APPLIES
  - const DOCKER_BUILD_APPLIES
  - const DOCKER_PACK
  - const ISO5055_MAIN_NO_DEAD_OR_DUPLICATE
  - const ISO5055_PACK
  - const ISO5055_SEC_NO_HARDCODED_SECRETS
  - const MOBILE_LIFECYCLE_SAFE_STATE
  - const MOBILE_PACK
  - const PACKS
  - const SECURITY_DEEP_PACK
  - const SERVICE_PACK
  - const WEBUI_PACK
  - enum CommandOutcome
  - enum CoverageVerdict
  - enum ProofRelevance
  - fn add_if
  - fn command_only_prints
  - fn command_source_files
  - fn confidence_label
  - fn conftest_chain
  - fn count_before
  - fn count_raw_imports
  - fn deepest_subcommand
  - fn discover_coverage
  - fn dispatch
  - fn edit_distance
  - fn git_build_id
  - fn grade_label
  - fn grounding_by_validation
  - fn group_validations_by_intent
  - fn is_env_var_name
  - fn leading_count
  - fn legacy_score_rule_for_intent
  - fn manual_verdict_is_sticky
  - fn normalize_applies_when
  - fn normalize_apply_signal
  - fn pack_rule_effort
  - fn parse_lcov
  - fn passed_count
  - fn prepare_additions
  - fn print_add_result
  - fn proof_relevance
  - fn recommend_rules_from_snapshot
  - fn remove_import_lines
  - fn resolve_codefiles_with_db
  - fn resolve_intent_from_snapshot
  - fn resolve_validation_from_list
  - fn run
  - fn run_add_with_sqlite
  - fn run_check_with_db
  - fn run_delete_with_sqlite
  - fn run_list_with_db
  - fn run_mark_with_sqlite
  - fn run_recommend_with_db
  - fn run_remove_with_sqlite
  - fn run_show_with_db
  - fn run_update_with_sqlite
  - fn run_validation_command
  - fn run_with_sqlite
  - fn saga_command_uses_builtin_engine
  - fn saga_missing_env
  - fn score_applies_when
  - fn score_rule_for_intent
  - fn strip_literals_and_comments
  - fn symbol_used_in_source_file
  - fn tap_pass_count
  - fn teach_unknown
  - fn terminate_command_tree
  - fn transitive_imports
  - fn validation_mark_edge_status
  - fn validation_mark_next_step
  - fn validation_result_edge_status
  - handle_confirm
  - handle_retire
  - handle_update
  - impl CoverageReport
  - impl PackRule
  - import_has_any
  - import_has_any_owned
  - new
  - path_has_any
  - path_has_any_owned
  - pub const LONG_VERSION
  - pub enum CodefileCmd
  - pub enum Command
  - pub enum CorpusCmd
  - pub enum DelegateCmd
  - pub enum DomainCmd
  - pub enum EdgeCmd
  - pub enum ExploreSubCmd
  - pub enum IgnoreCmd
  - pub enum InboxCmd
  - pub enum IntentCmd
  - pub enum LayerCmd
  - pub enum NoteCmd
  - pub enum PersonaCmd
  - pub enum PopulateCmd
  - pub enum RuleCmd
  - pub enum SkillCmd
  - pub enum SliceCmd
  - pub enum SourceCmd
  - pub enum TagCmd
  - pub enum ValidationCmd
  - pub enum VocabCmd
  - pub fn align_candidates_from_snapshot_notes
  - pub fn pack_names
  - pub fn parse_or_teach
  - pub fn resolve_root
  - pub fn ripple_intent_redefinition
  - pub fn run
  - pub fn run_all
  - pub struct Cli
  - pub struct PackRule
  - pub(crate) const EXPORT_STALE_WARNING
  - pub(crate) const INBOX_TRIAGE_COMMAND
  - pub(crate) const POPULATE_NEXT_COMMAND
  - pub(crate) const REQUIRED_HUMAN_GATED_DEBT_KEY
  - pub(crate) fn check_proof_command_shape
  - pub(crate) fn codefile_not_found
  - pub(crate) fn proof_discrimination
  - pub(crate) fn sorted_pair
  - resolve_intent_with_db
  - struct AppliesWhen
  - struct ApplySignal
  - struct CoverageReport
  - struct Grounding
  - struct IntentRuleSignals
  - struct RuleRecommendation
  - struct RuleRecommendationIntent
  - struct RuleRecommendationRule
  - symbol_executed
  - text_has_any
  - text_has_any_owned
  - type Pack
  - validate_apply_signal
  - validation_has_all
  - validation_has_all_owned
  - with_applies_when
  - with_evidence
provenance:
  build.rs: f4c28cfa329bcb8a
  src/cli.rs: a5eb5eb0002de356
  src/commands/batch.rs: 02cfa7c548c190a6
  src/commands/codefile.rs: 6abe76cbe37a1922
  src/commands/edge.rs: db5833205f278f03
  src/commands/export.rs: 9ce985c5714d8937
  src/commands/ignore.rs: b09685c5e3368319
  src/commands/import.rs: f93dd8f99757c7ba
  src/commands/init.rs: a1dc464a4c6cf3e3
  src/commands/intent.rs: ec7ad101ea150ed2
  src/commands/mod.rs: 91cf32d3f7d5791e
  src/commands/note.rs: 230f52cc758d8442
  src/commands/paths.rs: 3b0fb2aabc0889ed
  src/commands/resolve.rs: 6e16bf8f5c8ea1d0
  src/commands/rule.rs: ef348f83662d37bd
  src/commands/validate.rs: de34c26c9f567eb8
  src/commands/validation.rs: d726cbcb95e66827
  src/commands/whoami.rs: 2a7a1af42cbd7f32
  src/db/mod.rs: c0c82ec69ea7831f
  src/db/queries/intent.rs: 754f0308ba488a87
  src/db/queries/scoring.rs: 5569894f303e54f3
  src/db/sqlite/writes.rs: baa4f8f3186e71f9
  src/main.rs: 2545218dfc241471
---

# CLI surface and dispatch

> clap-derive command definitions and dispatch to command handlers; bare invocation prints orientation

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
