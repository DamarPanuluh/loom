# loom
- Prioritize lower maturity rungs before advancing to higher ones (incremental maturity progression). Confidence: 0.80
- Keep loom's honesty over green-washing: avoid fake green states that hide real debt. Confidence: 0.75
- Ensure loom's self-teaching surface clearly explains what LLMs should do for each axis. Confidence: 0.70
- For the proven rung: run the validation's test command, then record the verdict via `loom validation verdict <name> passed --evidence '...'`. Confidence: 0.75
- For the hardened rung: verify locators still resolve in the codebase, then re-record fresh verdicts via `loom edge verdict <id> ground|issue|independent --criterion '...' --evidence '...'`. Confidence: 0.75
- For the excellent rung: triage stale findings via `loom finding verdict <id> justified|needed|blocked --reason '...'`, re-justifying each stale finding with an updated reason. Confidence: 0.75
- When blocked on a rung with unowned codefiles, work them methodically using `loom coverage` to list unowned files. Confidence: 0.70
