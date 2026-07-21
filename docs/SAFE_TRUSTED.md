# Safe and trusted analysis

`graphine analyze <project> --mode safe` is the default security posture. It reads registered source/pom files, existing output directories, explicit classpath entries, and literal artifacts already in the local Maven cache. It never starts Maven. A missing dependency remains visible as an unresolved diagnostic.

`graphine analyze <project> --mode trusted` explicitly authorizes Maven execution for that invocation. Graphine constructs an argument vector for `dependency:build-classpath`, runs it in the canonical registered root, filters the environment, bounds combined Maven output, and applies the request timeout. It never concatenates a shell command and never exposes this path through MCP.

Trusted mode is not a sandbox: Maven extensions, lifecycle/plugin behavior, settings, and downloaded artifacts can execute code with the worker's OS permissions. Use it only for a repository and Maven configuration you trust. Safe mode is appropriate for unreviewed repositories or when an existing local classpath is sufficient.

Both modes keep repository code local. Neither mode uploads source to Graphine. Partial generation activation remains disabled unless `--allow-partial` or `allow_partial_activation` is explicitly set.
