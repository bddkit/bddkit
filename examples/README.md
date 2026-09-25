# Running the examples

Four suites, four configs. All need `cargo build --release` first (or use `cargo run --release --` in place of `./target/release/bddkit`).

## HTTP suite — `examples/api.yaml`

Talks to a local [Smocker](https://github.com/smocker-dev/smocker) instance, so the suite runs offline and its responses are fixed by a file in this repo.

```bash
docker compose up -d smocker                      # from the repo root
./target/release/bddkit run --config examples/api.yaml
```

The mocks live in `examples/mocks/api-server.yaml` and are seeded into Smocker at startup — after editing them, `docker compose restart smocker` reloads. Smocker's web UI (mocks, request history) is on <http://localhost:8081>; the mocked API itself is on <http://localhost:8080>.

| File | What it demonstrates |
|---|---|
| `features/methods.feature` | GET/POST/PUT/PATCH/DELETE/HEAD/OPTIONS, `Location`/`Allow` headers, a 404 asserted like any other answer |
| `features/json_matchers.feature` | `@variableType`, `@regExp`, `@arrayLength`, paths into arrays, `contains` vs `equals` |
| `features/variables.feature` | `set variable`, extraction from JSON and cookies, reuse across steps and scenarios, `<<unique()>>` |
| `features/variable_assertions.feature` | substring, regex and JSON assertions over a variable, extraction from a variable by JSON path or regex, and a macro in `macros/users.yaml` that publishes its extracts with `global` |
| `features/macros.feature` | YAML-declared steps from `macros/posts.yaml`, macro calling a macro, Scenario Outline |
| `features/content_types.feature` | HTML/XML/plain-text responses, a form login |
| `features/eventual.feature` | An assertion polled until it passes |

`eventual.feature` is only interesting on a fresh mock session: `/jobs/1` answers "pending" twice and then "done", and Smocker keeps that counter until the container restarts. Run `docker compose restart smocker` to see the polling actually retry — on a session where the job is already done, the assertion just passes on its first attempt.

## Database suite — `examples/db.yaml`

```bash
docker compose up -d db                          # Postgres on :5433, schema from examples/db/init.sql
./target/release/bddkit run --config examples/db.yaml
```

`db-features/db.feature` walks through every DB step: inserts (one-liner, table, multi-table), update, delete, extraction into a variable, row-presence assertions, the four condition operators (`col:`, `col!:`, `col~:`, `col!~:`), procedures, functions, sequences, and a second connection. It is deliberately Postgres-only — see the portability table below for what that costs.

`db-features/cleanup.feature` is the run-scoped cleanup pattern: `<<run_id>>` plus a `~:` on the column — the operator that asks for `LIKE` — removes exactly the companies this run inserted. Both files carry `@serial(demo)`, so they are one chain and the cleanup — tagged `@priority(-100)` — is the last file of it. Without the shared `@serial` tag the two files are separate chains and can run at the same time, which is the one way this pattern goes wrong.

## Database suite on MySQL — `examples/db-mysql.yaml`

```bash
docker compose up -d mysql                    # MySQL on :3307, schema from examples/db/init-mysql.sql
./target/release/bddkit run --config examples/db-mysql.yaml
```

`db-features-mysql/db.feature` is the **engine-neutral** file: it uses only vocabulary that behaves identically on MySQL and on MariaDB — the three insert forms (one-liner, data table, multi-table wide form), update, delete, `I extract`, both row-presence assertions, and a connection switch. It passes unchanged on both, which is the whole point of it; if a step needs an engine-specific spelling it does not belong in that file.

## Database suite on MariaDB — `examples/db-mariadb.yaml`

```bash
docker compose up -d mariadb                  # MariaDB on :3308
./target/release/bddkit run --config examples/db-mariadb.yaml
```

MariaDB is not MySQL on another port: it is everything MySQL does, **plus** sequences and `RETURNING`. So this config runs two directories — the engine-neutral file above, reused rather than copied, and `db-features-mariadb/db.feature`, which is only the remainder:

| Scenario | What it shows |
|---|---|
| sequences | `I get next value of sequence`, which on MySQL fails naming the engine |
| a primary key filled by a server-side `DEFAULT` | `tickets.id` defaults to `NEXTVAL(ticket_seq)` — server-generated but not `AUTO_INCREMENT`, so its value is not on the insert's result packet. `RETURNING` is what puts it in `<<last_insert_id_tickets>>`; on MySQL the same step is refused before the INSERT runs, naming the column |

That second row is the only way `RETURNING` is observable from Gherkin at all — the clause itself appears only in debug output. Its extra schema is `examples/db/init-mariadb.sql`, mounted into the `mariadb` service alone: `CREATE SEQUENCE` would fail MySQL's own startup.

Both MySQL/MariaDB init scripts run only on a container's first start with an empty data directory. If the containers already exist, `docker compose down -v && docker compose up -d` to apply them.

## What does not port between the three

The root `README.md` states the support level; this is the same ground per row, with the workaround.

| Feature | Postgres | MySQL | MariaDB | Why |
|---|---|---|---|---|
| `I get next value of sequence` | yes | **no** | yes | MySQL has no sequences at all. Postgres and MariaDB both do — `examples/db-features/db.feature` and `examples/db-features-mariadb/db.feature` each use it — so the step is portable everywhere except MySQL, and a file using it is no longer engine-neutral |
| `search_path` in `resources.db` | yes | **no** | **no** | A schema *is* a database here, so there is no session-level search path. bddkit refuses it at startup instead of ignoring it — name the database in the DSN, or qualify a step's table as `database.table` |
| `binary`/`varbinary` columns | n/a | **no** | **no** | `UUID_TO_BIN(uuid, swap_flag)` reorders bytes depending on a flag that `information_schema` does not record, so bddkit cannot know which layout the service under test reads. It refuses rather than write disagreeing bytes. Use `char(36)` — as `examples/db/init-mysql.sql` does — and bddkit fills it with a client-side UUIDv7. Every width is refused, not just 16: this layer binds and compares as text, so a WHERE against any binary column would match nothing |
| A PK filled by a server-side `DEFAULT` that is not `AUTO_INCREMENT` | yes | **no** | yes | Reading the value back needs `RETURNING`, which MySQL does not have (MariaDB has it from 10.5). The step is refused before the INSERT rather than after it commits — give the value explicitly, or use `AUTO_INCREMENT` |
| The *text* an `I extract` yields | each its own | each its own | each its own | Portable as a step, not as a value. Every engine renders its own types: `now()` is `2026-08-29 15:11:50.884052+00` on Postgres and `2026-08-29 15:11:50` on both others, a boolean is `true` vs `1`, and a bare `numeric` is `0` where `decimal(12,2)` is `0.00`. So `variable "x" should be equal to "..."` on an extracted timestamp or boolean is engine-specific even though the steps around it are not — the example only extracts from `varchar` columns for that reason |
| `col~:` / `col!~:` (`LIKE`) | case-sensitive | **case-insensitive** | **case-insensitive** | The operator, the pattern and the escaping are identical; only the matching is not. MySQL and MariaDB compare under the column's collation, which is case-insensitive by default (`..._ci`), where Postgres `LIKE` never folds case. A pattern that must behave the same on all three should not rely on case — the run prefix `<<run_id>>` is lowercase base36, so the cleanup pattern is unaffected |
| `I call procedure` / `I call function` | yes | yes | yes | The *steps* are portable; the routine *bodies* are not. The Postgres example's are `LANGUAGE sql` with `nextval()` and `\|\|`, which is why they are absent here rather than translated |

Every other DB step is spelled the same and behaves the same on all three — which is what `examples/db-features-mysql/db.feature` demonstrates by passing, unchanged, from two different configs.

## Narrowing a run

```bash
# one file (a positional path overrides the config's `paths`)
./target/release/bddkit run --config examples/api.yaml examples/features/methods.feature

# one tag (repeatable; feature-level tags apply to every scenario in the file)
./target/release/bddkit run --config examples/api.yaml --tag json
```

Exit codes: `0` everything passed, `1` a scenario failed, `2` the run never started (bad config, unknown step, no scenario matched the filter).
