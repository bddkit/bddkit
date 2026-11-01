---
paths:
  - "tests/**"
  - "selftest/**"
---

# Tests

Loaded on demand; the root `CLAUDE.md` holds the invariants that apply everywhere. Subsystem gotchas live next to the code: `src/db/CLAUDE.md` (for `tests/db*.rs`), `src/plugin/CLAUDE.md` (for `tests/plugin*.rs`).

- **Integration tests that spawn the stub AND block on `Command::output()` need a multi-thread runtime.** Use `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`. Single-threaded starves the spawned server → requests hang to timeout.
- **DB tests run the compiled binary as a subprocess and need a live database.** `docker compose up -d` first, then run single-threaded (`--test-threads=1`) — the fixture schema `apibdd_it` is shared, so parallel tests race. `BDDKIT_TEST_ENGINE` picks the engine (`postgres` default, `mysql`, `mariadb`) and with it the DSN variable (`BDDKIT_TEST_DSN` / `BDDKIT_TEST_MYSQL_DSN` / `BDDKIT_TEST_MARIADB_DSN`); `tests/db_engines.rs` and `tests/db_mysql_insert.rs` read those variables directly and **skip** an engine whose variable is unset, so they pass by testing nothing unless you export them. Set `BDDKIT_TEST_TIMINGS=1` and pass `--nocapture` to print a per-test timing table. `cargo test --bin bddkit` has no DB dependency and stays fast.

- **A test that exercises the built-in HTTP API or a database itself carries `// moves-to: api` (or `db`) in the comment and attribute block directly above its `fn`, doc comment included.** Anything else must run over the `echo` and `worker` fixture plugins, so it survives the API and the databases leaving the core (epic #183). The split deletes the whole block and the test with it: grep `moves-to:` for the list. A core config that still needs `resources.api` writes the empty `api: {}` until #190. The worker fixture's `I wait at the worker barrier for <n> parties` is the barrier behind the parallelism proofs (`write_parallel_fixture`); a project that needs the echo fixture's evidence or a plugin of its own passes `--bddkit-dir <project>/.bddkit` so a developer's installed plugins never decide the result.
