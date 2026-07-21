# Benchmark runner adapters

`benchmark-core` owns corpus loading, validation, deterministic graph scoring, and paired agent evaluation.

Capture one `agent-session.schema.json` file per condition. Baseline agents receive repository file listing, text search, and file reading only. Graphine agents receive the seven Graphine MCP tools plus optional bounded file/evidence reading. Keep model, system prompt, commit/dirty digest, time limit, maximum tool calls, and deterministic settings identical in `controls`.

Run:

```text
benchmark-core compare-agents --baseline baseline.json --graphine graphine.json --output comparison.json
```

The evaluator uses normalized deterministic claims, forbidden-claim checks, repository-contained evidence verification, and required uncertainty. Human review and an LLM judge are optional secondary fields only. Provider-reported token counts are captured as given; absent measurements stay null.
