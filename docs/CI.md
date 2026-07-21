# Continuous integration

`.github/workflows/ci.yml` pins the declared Rust 1.86 MSRV and runs strict formatting, Clippy, workspace tests, and benchmark-schema validation on Ubuntu and Windows. Java tests/package run on Ubuntu JDK 17/21 and Windows JDK 21. Separate Ubuntu/Windows E2E jobs exercise both Java and Spring fixtures through JDT, the semantic pass, JSONL validation, Rust ingestion, SQLite, and initialized MCP queries. The accuracy job evaluates supported Java and Spring graph facts and fails below the checked-in precision/recall thresholds.

Surefire and graph-accuracy reports are uploaded even when a job fails. Cargo and Maven caches are keyed by lock/model inputs. Repository administrators should protect `main` with the named `rust-*`, `java-*`, `e2e-*`, and `java-graph-accuracy` checks; branch protection itself cannot be configured from repository files alone.
