# Continuous integration

`.github/workflows/ci.yml` pins Rust 1.95 and runs strict formatting, Clippy, workspace tests, and benchmark-schema validation on Ubuntu and Windows. Java tests/package run on Ubuntu JDK 17/21 and Windows JDK 21. Separate Ubuntu/Windows E2E jobs exercise Java/Spring and Rust safe/trusted fixtures through both protocol-v2 workers, SQLite activation, and initialized MCP queries.

Rust gates cover workspace/target discovery, traits/default methods, static and dynamic dispatch, types, fields, enums, generics, tests, macros, features, build scripts, local procedural macros, wrong-language/malformed/crash/timeout streams, fingerprint changes, and prior-generation preservation. Safe-mode tests assert no network or project-code execution; trusted tests execute build-time code only after explicit selection. Rust graph accuracy fails below 0.97 precision or 0.95 recall and forbids deterministic runtime-implementation claims.

Linux and Windows package jobs build archives containing the host executable, Java analyzer JAR, and platform Rust worker, then run a clean-install doctor smoke test. Surefire and graph-accuracy reports are uploaded even when a job fails. Repository administrators should protect `main` with the named Rust, Java, E2E, accuracy, and packaging checks.
