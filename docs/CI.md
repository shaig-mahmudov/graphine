# Continuous integration

`.github/workflows/ci.yml` runs strict Rust formatting, Clippy, workspace tests, and benchmark-schema validation on Ubuntu and Windows. Java tests/package run on Ubuntu JDK 17/21 and Windows JDK 21. Separate Ubuntu/Windows E2E jobs exercise fixture → JDT → JSONL → Rust ingestion → SQLite → initialized MCP queries, including overloaded-method resolution and repeated occurrence evidence. The accuracy job evaluates supported Java graph facts and fails below the checked-in precision/recall thresholds.

Surefire and graph-accuracy reports are uploaded even when a job fails. Cargo and Maven caches are keyed by lock/model inputs. Repository administrators should protect `main` with the named `rust-*`, `java-*`, `e2e-*`, and `java-graph-accuracy` checks; branch protection itself cannot be configured from repository files alone.
