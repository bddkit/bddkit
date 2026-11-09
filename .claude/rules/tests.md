---
paths:
  - "tests/**"
  - "selftest/**"
---

# Tests

Loaded on demand; the root `CLAUDE.md` holds the invariants that apply everywhere. Subsystem gotchas live next to the code: `src/plugin/CLAUDE.md` (for `tests/plugin*.rs`).

- **Integration tests that spawn the stub AND block on `Command::output()` need a multi-thread runtime.** Use `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`. Single-threaded starves the spawned server → requests hang to timeout.
- **A test that exercises the built-in HTTP API itself carries `// moves-to: api` in the comment and attribute block directly above its `fn`, doc comment included.** Anything else must run over the `echo`, `worker` and `group` fixture plugins, so it survives the API leaving the core (epic #183); the databases already left, and the `group` fixture's `BDDKIT_GROUP_FIXTURE_HOST_GROUPS=db` mode stands in for `bddkit-db` where a test needs the `db` group. The split deletes the whole block and the test with it: grep `moves-to:` for the list. The same block carries `#[cfg(not(feature = "dev-unreserve"))]`, so a build without the built-ins does not compile it; a test that merely needs some step takes a core one such as `Show all variables` instead. The worker fixture's `I wait at the worker barrier for <n> parties` is the barrier behind the parallelism proofs (`write_parallel_fixture`); a project that needs the echo fixture's evidence or a plugin of its own passes `--bddkit-dir <project>/.bddkit` so a developer's installed plugins never decide the result.
