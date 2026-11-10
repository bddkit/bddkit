---
paths:
  - "tests/**"
  - "selftest/**"
---

# Tests

Loaded on demand; the root `CLAUDE.md` holds the invariants that apply everywhere. Subsystem gotchas live next to the code: `src/plugin/CLAUDE.md` (for `tests/plugin*.rs`).

- **Core tests run over the `echo`, `worker` and `group` fixture plugins, never over a real API or database.** The HTTP API, SRP and the databases are plugins (`bddkit-api`, `bddkit-db`) with their own tests; the `group` fixture's `BDDKIT_GROUP_FIXTURE_HOST_GROUPS` mode claims their groups where a test needs them. A test that merely needs some step takes a core one such as `Show all variables`. The worker fixture's `I wait at the worker barrier for <n> parties` is the barrier behind the parallelism proofs (`write_parallel_fixture`); a project that needs the echo fixture's evidence or a plugin of its own passes `--bddkit-dir <project>/.bddkit` so a developer's installed plugins never decide the result.
