# workflow
- Refers to repositories and projects by short names (e.g., "loom") rather than full paths or URLs. Confidence: 0.85
- After implementing changes, commit all, push, and reinstall the global binary. Confidence: 0.80
- Dogfood/test changes locally before deploying to global binary. Confidence: 0.75
- Commit until tree clean before switching to a new task. Confidence: 0.70
- Prefers direct CLI execution to observe live behavior (`loom --help` / `loom --version` / `loom status`) over static source/README exploration when inspecting tools. Confidence: 0.85
- When debugging a tool failure, grep the source for the exact error string (e.g., `broken_journey_proof_chain`) to locate the implementing rule, then read that module. Confidence: 0.7
- Prefers `python3 -c` one-liners over jq or throwaway scripts for ad-hoc JSON inspection. Confidence: 0.65
- Delegates fixes with terse, context-aware prompts (e.g., "fix that doctor issue") expecting autonomous diagnosis and remediation without re-specifying context. Confidence: 0.72
