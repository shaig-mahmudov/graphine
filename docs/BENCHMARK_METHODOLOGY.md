# Benchmark methodology

## Compared conditions

The baseline is the AI agent using normal file search, grep, and file reading. The experiment is the same AI agent using Graphine MCP plus optional reads of evidence files. A paired run uses the same model and model version, task prompt, repository commit and working-tree state, context policy, time limit, tool-call limit, and reproducible temperature/seed/settings when the provider supports them.

Runs must be isolated. Caches are cleared for cold-query comparisons and intentionally retained for separately labeled warm-query comparisons. Question order is fixed or seeded and recorded. Failures, timeouts, unresolved responses, and retries remain in the dataset. No result may be manually repaired after the run.

## Capture and scoring

Each adapter records the raw answer and normalized atomic claims, the evidence opened, and these stable metrics:

- correctness, precision, recall, unsupported claims, and unresolved count;
- input and output tokens reported by the model provider;
- tool calls, file reads, and distinct files opened;
- wall-clock time and per-query latency.

Absent measurements are persisted as `null`; token counts are never estimated or invented. Correctness is defined by the question-specific ground truth and forbidden claims. Reviewers resolve scoring disputes without seeing whether an answer came from baseline or experiment. A stratified report includes fixture, category, difficulty, support level, and warm/cold state, along with medians and p95 where appropriate.

Answer-quality degradation compares paired correctness scores. Token and tool-call reduction use paired medians so a small number of large repositories do not dominate. Unsupported deterministic resolution is counted even if the rest of an answer is correct. Unresolved answers are reported separately and are correct only where ground truth permits unresolved or ambiguous status.

## Engineering goals

These are initial goals, **not achieved claims**:

| Metric | Initial target |
| --- | ---: |
| Structural query precision | at least 95% |
| Structural query recall | at least 90% |
| Spring route precision | at least 98% |
| Unsupported deterministic resolutions | 0 |
| Median input-token reduction | at least 70% |
| Median tool-call reduction | at least 50% |
| Answer-quality degradation | no more than 2% |
| Warm-query p95 | below 100 ms |

## Reproduction checklist

Record repository commit and dirty-state digest, fixture toolchains, model/provider identifiers, agent prompt and tool definitions, model settings, adapter version, limits, cache state, OS/architecture, and run timestamps. Validate the corpus before every run. Preserve the generated run JSON and raw adapter artifacts. A comparison is publishable only when both conditions completed under the same recorded controls.

