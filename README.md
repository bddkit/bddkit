# bddkit

Acceptance testing for backend services — the API and everything behind it —
written in Gherkin and run by a single Rust binary.

A backend test rarely ends at the HTTP response. The interesting question is
usually what happened *behind* it: did the row change, did the balance move,
did the state settle a second later. bddkit treats every system a scenario
touches as a **named resource** — declared once in the config, reachable from
any scenario — so seeding a row, calling the API, and asserting the row
changed are three steps in one vocabulary instead of three tools with glue
between them.

```gherkin
Scenario: registering a company charges the account
  Given I have "accounts" with "email: <<unique(email)>>, balance: 100"
  And the request body is:
    """
    {"account_id": "<<last_insert_id_accounts>>", "name": "Acme"}
    """
  When I request "/api/v1/companies" using HTTP POST
  Then the response code is 201
  And I expect the next assertion to pass within "10" seconds
  And I should have "accounts" with "balance: 75"
```

## What it solves

**Extending the vocabulary shouldn't require a rebuild.** Domain steps like
`I login as user "..."` are declared in YAML and loaded at startup, so
whoever writes the scenarios can also extend the language they're written in.

**A typo shouldn't cost you three minutes of run time.** Unknown steps,
ambiguous patterns, macro cycles, undeclared connections — all found in one
pass before the first request. Exit code `2` means "run not started", so CI
can tell a broken suite from a failing one. Pattern collisions fail loudly
instead of silently shadowing each other.

**Checking the database shouldn't mean leaving the feature file.** DB steps introspect the real schema — PostgreSQL, MySQL or MariaDB, the same step text on each: primary keys fill themselves the way the column declares them, values coerce to real column types, and a missing `NOT NULL` column fails by name at the step rather than as a driver error. Hawk signing, SRP-6a, and AES are steps too, so a login handshake stays declarative.

**Async systems shouldn't need sleep loops.** Prefix any assertion with
`I expect the next assertion to pass within "10" seconds` and it retries —
re-sending the request or re-running the query — until it holds.

**Test data shouldn't collide, and should be removable afterwards.** Every `<<unique()>>` value in a run shares one prefix drawn once per process, so uniqueness is guaranteed rather than probable — and `<<run_id>>` writes that prefix down, so cleanup afterwards is one step: `I delete "companies" where "slug~: u<<run_id>>%"`.

**A failure should explain itself.** Every failed step prints the full last
exchange — method, URL, headers, bodies, status — with no debug flag and no
re-run. JSON mismatches point at the path that differs.

## Quick start

```bash
docker compose up -d smocker   # local mock API the example talks to
cargo build --release
./target/release/bddkit run --config examples/api.yaml
```

```console
run m4k2p9x7q3b1
  ✓ examples/features/methods.feature — scenarios: 8
  ✓ examples/features/json_matchers.feature — scenarios: 4
  ✓ examples/features/variables.feature — scenarios: 5
  ✓ examples/features/variable_assertions.feature — scenarios: 3
  ✓ examples/features/macros.feature — scenarios: 6
  ✓ examples/features/content_types.feature — scenarios: 4
  ✓ examples/features/eventual.feature — scenarios: 1

run m4k2p9x7q3b1
files: 7, scenarios: 31, failed: 0
```

The example suite talks to a local [Smocker](https://github.com/smocker-dev/smocker)
instance seeded from `examples/mocks/api-server.yaml`, so it runs offline and
its responses are fixed by a file in this repo. Smocker's web UI is on
<http://localhost:8081>.

The database examples are a second suite with its own config and its own
container:

```bash
docker compose up -d db
./target/release/bddkit run --config examples/db.yaml
```

`examples/README.md` covers both suites, what each feature file demonstrates,
and how to narrow a run to one file or one tag.

`bddkit run` flags: `[--config <path>]` (default: `$BDDKIT_CONFIG`, else `./bddkit.yaml` or `./bddkit.yml` in the working directory), positional paths to override the config's, `--tag` (repeatable), `--env` to pick a `.env.<name>` layer, `--fail-fast`, `--junit <file>` and `--cucumber-json <file>` for machine-readable reports. Exit codes: `0` passed, `1` a scenario failed, `2` the run never started, including when no config is given and none of the above is found.

### Reports for CI

`--junit reports/junit.xml` writes JUnit XML — one `<testsuite>` per feature file, one `<testcase>` per scenario, the steps with their status and timing in `<system-out>`, which is what Jenkins, GitLab and GitHub test summaries read. `--cucumber-json reports/cucumber.json` writes Cucumber JSON in the shape cucumber-html-reporter and Allure accept: features with `elements`, elements with `steps`, each step with `keyword`, `name`, `line` and a `result` of `status`, `duration` (nanoseconds) and `error_message`. Both flags are optional and independent, and both are properties of the run, not of the suite — the config never learns them. Console output is unchanged.

Step text is the raw feature text, never interpolated, so two runs' reports differ only in results and timings. A failure carries the same text the console prints — the step error plus the HTTP exchange — and the XML stays well-formed whatever that text contains (a `]]>`, a control character, the NUL bytes of the `<<null>>` sentinel become U+FFFD). Scenarios a `--tag` filtered out and files never started under `--fail-fast` are absent, not skipped; the totals count what ran. A macro call is one step. A file that panicked appears as its one synthetic failed scenario.

**Every report path is created — truncated — before the config is read**, its directory made if missing; a path that cannot be prepared is exit 2 before any request. A run that dies on its config therefore leaves an empty file, which a parser rejects loudly, rather than yesterday's green one. A report that cannot be written after the run is also exit 2 — the one exception to "2 = the run never started", because a report silently lost behind a green code is the worse outcome. `bddkit doctor` accepts the same flags and makes the same check (a `reports` row per path); without them it touches nothing.

## Checking a suite before running it

`bddkit doctor` runs every check a run makes before its first request — config parse, `${VAR}` expansion, ambiguous `default_*`, the plugin lock file and ABI versions, macro conflicts and cycles, undefined steps, malformed `@priority`/`@serial` tags, a `paths` that selects nothing, and a DSN the driver cannot parse — and reports all of them at once instead of stopping at the first:

```console
$ bddkit doctor --config suite.yaml
config: suite.yaml
APP_ENV: dev

  ✓ config                parsed, ${VAR} expanded, defaults resolved
  ✓ plugins               none installed
  ✓ macros                loaded, no conflict or cycle
  ✗ steps
      features/auth.feature:12
        unknown step: "I frobnicate"
  ✓ scheduling            4 chain(s)
  ✓ api main              https://api.example.test (20s timeout)
  - api main              live probe skipped
  ✓ db primary            postgres
  - db primary            live probe skipped

1 problem(s)
static checks only — pass --live to also probe the resources
```

A bare `doctor` opens no socket: it is fast, offline and deterministic, so it works on a train and a `base_url` pointing at a closed port is not a finding. `--live` adds the one class of check a run does not have — whether the resources the config names actually answer: a `GET` at each `base_url` (any status means reachable), a real connection to each database, and each declared plugin instance asked to probe its own resource, every one named individually so one dead DSN says which. A plugin that exports no probe is reported as skipped, never as a failure — a check that never ran has proved nothing.

`bddkit doctor` flags: `[--config <path>]` (same default as `run`: `$BDDKIT_CONFIG`, else `./bddkit.yaml`/`./bddkit.yml`), `--env` to pick a `.env.<name>` layer, `--live`, `--json`, and `run`'s `--junit`/`--cucumber-json` to check that a report path can be written. Exit codes: `0` clean, `1` anything was reported — never `2`, so a script's rule is simply "0 or fix something"; a config that cannot be found at all is reported the same way, as a failed `config` row.

The header states which `APP_ENV` layer was selected, which is the first thing that is wrong when a suite passes locally and fails in CI, and — since `--config` is optional — how the config path itself was chosen: `(--config)`, `(BDDKIT_CONFIG)` or `(found in working directory)`. `--json` emits the same report machine-readably, with that same choice as its own `config_source` field (`"flag"` / `"env"` / `"discovered"`, or absent when no config was found at all) rather than only inside the `config` string — plus one object per check with a `probe` flag separating a resource's static row from its live one.

`doctor` never fixes what it finds, and every static stage is a function `run` itself calls — if the two ever disagree about whether a config is valid, that is a bug in `doctor`. The one finding `run` never makes is a step of a resource group with nothing declared to run on — an HTTP step over `resources.api: {}`, a DB step with no `resources.db`, a plugin step whose group was emptied to switch its implicit instance off. `run` accepts that config and fails in the scenario, because a `--tag` may keep the scenario out; `doctor` applies no tag filter, so there the failure is certain, and it is reported as one — once per file and group, at the first step that needs it.

## What steps exist

`bddkit steps list` prints the whole vocabulary as templates, grouped by resource, with no config and no regex:

```console
$ bddkit steps list db
db:
  I use "<name>" connection
  I have "<table>" where:
  I have "<table>" with "<pairs>"
  I have:
  I update "<table>" with "<pairs>" where "<condition>"
  ...
```

Narrow it with a positional resource (`api`, `db`, `srp`, `vars`, `debug`, `general`, or a plugin's group) and with `--filter <text>`, a case-insensitive substring match. `-v` adds a one-line description under each step — filtering down to one step and adding `-v` is the help-for-one-step path, so there is no separate `describe` command:

```console
$ bddkit steps list --filter "response code" -v
api:
  the response code is <code>
    asserts the HTTP status code of the last response
```

`--json` emits the same listing machine-readably, with the raw pattern included. `[--config <path>]` also loads that suite's plugins, so their steps appear under their own groups — resolved the same way as `run`'s, including the `bddkit.yaml`/`bddkit.yml` default, so inside a suite directory this shows that suite's steps with no flag at all; `--no-config` skips that and always answers with builtins only, which also sidesteps a broken default config. Descriptions follow `--lang`, else `$BDDKIT_LANG`, else English; `ru` and `lv` ship with the binary, and an untranslated step falls back to English rather than to a blank line.

### Asserting over a variable

Text reaches a variable from more places than the response body — `I extract` from a column, a plugin's published vars, `extract … from cookies`. The response body's checks work over any of them:

| Step | Effect |
|---|---|
| `variable "<name>" should contain "<text>"` / `should not contain "<text>"` | plain substring |
| `variable "<name>" should match "<regex>"` / `should not match "<regex>"` | `regex` crate syntax, unanchored |
| `variable "<name>" should be empty` | empty or whitespace only, as for the body |
| `variable "<name>" should contain JSON:` (doc string) | the body's `contains JSON` matching, `@variableType` / `@arrayLength` / `@regExp` included |
| `variable "<name>" should equal JSON:` (doc string) | the body's `equals JSON` matching |
| `variable "<name>" should not contain JSON:` (doc string) | the negation of `should contain JSON:` |
| `extract "<path>" from variable "<name>" as JSON as "<target>"` | a JSON path of the variable's text into `<target>`; `global` suffix as for `extract … from JSON` |
| `extract "<regex>" from variable "<name>" as "<target>"` | the first capture group of the first match into `<target>`; a regex with no group is an error; `global` suffix as above |

A variable that is not set fails the step, and so does a JSON step over a variable whose text is not JSON — naming the variable and the parse error, not reporting a mismatch. Every variable assertion, `should be equal to` included, fails on its first mismatch even under `I expect the next assertion to pass …`: nothing runs between two attempts, so the value cannot change and polling it would only burn the timeout.

## What a resource's config takes

`bddkit resource fields` prints the keys each kind of `resources` entry accepts, which key is mandatory, what value it takes, and what it is for:

```console
$ bddkit resource fields db
db:
  dsn               string    required  connection string; its scheme selects the engine (postgres://, mysql://)
  search_path       nonscalar           Postgres schema search path; refused on MySQL and MariaDB, which have no session equivalent
  options           nonscalar           polling.timeout_secs / polling.interval_ms for eventual assertions against this connection
```

The type column is the answer to "can I set this with a flag": `string`, `boolean` and `number` can, and `nonscalar` — a map or a list — is what `resource add --json` is for. A plugin's group is described by the plugin itself, and a field whose manifest declares no type is a string.

Narrow it with a positional kind (`api`, `db`, `srp`, or a plugin's group). `[--config <path>]` also loads that suite's plugins, so the groups they serve are described too — by the plugins themselves, since only a plugin knows what its own group takes; resolved the same way as `steps list`'s, `bddkit.yaml`/`bddkit.yml` default included, with `--no-config` as the same escape hatch. A plugin whose manifest describes nothing says so and is not an error. `--json` emits the same listing machine-readably.

### `bddkit resource add`

```
bddkit resource add <group> <name> [--config <path>] [--env <name>] [--json '{...}'] [--no-check] [--<field> <value>]...
```

```console
$ bddkit resource add api staging --config suite.yaml --base_url http://staging.local --timeout_secs 5
$ bddkit resource add s3 main --config suite.yaml --no-check --bucket photos --endpoint https://s3.local
```

Writes one new resource into the config's YAML and reports whether it happened. `--config` is resolved the same way as `run`'s (`$BDDKIT_CONFIG`, else `./bddkit.yaml`/`./bddkit.yml`); nothing found is refused with exit 1, same as any other check this command makes, never exit 2. A `--<field> <value>` becomes a body key; the value always arrives as a string, and it is the group's own field table (the host's for `api`/`db`/`srp`, a plugin's manifest for its own group) that converts it — `timeout_secs` becomes a YAML number, and a field the table calls a map or a list refuses the flag and names `--json` instead. `--json '{...}'` supplies the whole body at once and is applied first; any `--<field>` on the same command line then overrides that key, one key at a time, leaving the rest of the JSON body untouched. A name the group's table does not know is refused rather than silently added. The command's own flags (`--config`, `--env`, `--json`, `--no-check`) must be typed before any `--<field>` value — clap treats everything after the first flag it does not itself recognize as more field arguments, so one of these typed later would otherwise vanish into the resource body or be rejected as an unknown field. Before writing anything, the exact prospective file text is assembled, re-parsed and put through the checks `doctor` makes about a config and its resources — the suite's feature files, steps, macros and scheduling tags are deliberately not among them, since a broken `.feature` elsewhere is no reason to refuse a resource. A `${VAR}` placeholder in a value is written out literally and only *expanded* for that check. The live probe that `doctor --live` runs against this one new resource is also run here, by default; `--no-check` skips only that probe, the shape validation still happens. A resource that already exists under that name is refused rather than replaced, because rewriting the surrounding YAML block is how hand-written comments in it get lost — insertion only ever splices in a new block next to the existing text. Exit code is 0 when the file was written and 1 when it was not (a malformed flag, a rejected field, a failed check, an existing resource), matching `doctor`'s "0, or fix something" convention; the command itself never answers 2, though a usage error the argument parser catches before it starts still does.

## Config

`--config` is optional: `run`, `doctor` and `resource add` fall back to `$BDDKIT_CONFIG`, then to `./bddkit.yaml` or `./bddkit.yml` in the working directory (both present is refused, naming both files). No walk-up into parent directories — `paths` and `macro_paths` below resolve against the working directory, not the config file, so a config found in a parent would point at the wrong feature files.

```yaml
concurrency: 8              # files in flight; a file is one tokio task
macro_paths: [macros/]
paths: [features/]

resources:
  api:
    review:
      base_url: http://review.local
      default_headers: { X-Client: bddkit }
  db:
    default:
      dsn: postgres://${DB_USER}:${DB_PASS}@db:5432/review
      search_path: [app, public]

options:
  polling: { timeout_secs: 5, interval_ms: 100 }   # defaults for eventual assertions
```

`${VAR}` expands docker-compose style (`:-`, `:?`, `:+` and friends) from the
real environment and a `.env` / `.env.local` / `.env.<APP_ENV>` layer stack
next to the config. With one resource of a kind its `default_*` is inferred;
with several it must be explicit. `options` set at the root cascades into
every resource and can be overridden per resource.

Variables live for the whole **feature file**, so a later scenario can read
what an earlier one produced. Request state and the current connection reset
per **scenario**. Files run in parallel; `@serial(name)` chains files that
contend, `@priority(N)` moves one up the queue.

## Databases

PostgreSQL, MySQL 8 and MariaDB, from **MariaDB 10.5** — the release that adds `RETURNING`, which bddkit uses to read a generated key back. Nothing refuses an older MariaDB at startup; it connects, and the first insert into a table with a primary key fails with a syntax error from the server. The engine comes from the DSN scheme (`postgres://`, `mysql://`), and because MySQL and MariaDB share one scheme they are told apart by a single `SELECT VERSION()` when the pool is built. Every DB step is spelled the same on all three — `examples/db-features-mysql/db.feature` is one file that passes unchanged against both MySQL and MariaDB.

What does not carry over:

- **Sequences.** MySQL has none, so `I get next value of sequence` fails naming the engine. Postgres and MariaDB both have them.
- **`search_path`.** On MySQL and MariaDB a schema *is* a database, so there is no session-level search path. The key is refused at startup rather than quietly ignored: name the database in the DSN, or qualify a table as `database.table` in the step.
- **`binary` and `varbinary` columns**, `binary(16)` UUIDs included. This layer binds and compares everything as text, and `UUID_TO_BIN`'s `swap_flag` — which decides the byte order — is recorded nowhere in `information_schema`, so bddkit would have to guess and could write bytes the service under test disagrees with. It refuses the column instead. Store the UUID as `char(36)` and bddkit fills it client-side.
- **A primary key filled by a server-side `DEFAULT` that is not `AUTO_INCREMENT`**, on MySQL. There is no `RETURNING` there to read the value back with, so the step fails naming the column before it inserts anything — give the value explicitly.
- **The text a value reads back as is the engine's own.** `I extract` is portable as a step and not as a value: `now()` reads back as `2026-08-29 15:11:50.884052+00` on Postgres and `2026-08-29 15:11:50` on MySQL and MariaDB, a boolean as `true` versus `1`, a bare `numeric` as `0` where `decimal(12,2)` is `0.00`. So `I extract` followed by `variable "x" should be equal to "..."` is portable over `varchar`-ish columns and nowhere else. A `where` against those same columns is unaffected — the engine coerces the bind back into its own type; it is only the extracted value that carries the engine's spelling.

`examples/README.md` has the same ground as a per-engine table, with the workaround for each row.

In debug mode, every DB step prints the query's duration right after its SQL and parameters (`TIME: 3.41 ms`), on a failing query too — a slow query that ends in an error is exactly what this is meant to diagnose. Under `concurrency` greater than 1 that number also includes the wait for a free pooled connection, so it can read higher than the query's own time; for an accurate measurement run with `concurrency: 1`.

### Cleaning up what a run created

Every `<<unique()>>` token is `u` followed by the run's own 12-character prefix and a counter, and `<<run_id>>` is that prefix. A `~` on the column name asks for SQL `LIKE` instead of `=`, so one step removes exactly the rows this run wrote — `examples/db-features/cleanup.feature` is it, working:

```gherkin
@serial(demo) @priority(-100)
Feature: cleaning up what this run created

  Scenario: remove the companies this run created
    When I delete "companies" where "slug~: u<<run_id>>%"
```

Four things to know about it:

- **The operator lives on the column, never in the value.** `slug: 20%` is still exact equality against the text `20%`; only `slug~:` is a pattern. That direction is deliberate: a `%` that arrives through a variable — from an API response, say — must never be able to widen a `DELETE` on its own.
- **The condition grammar is four operators**, and they work in every step that takes a condition: `col:` is `=`, `col!:` is `<>`, `col~:` is `LIKE`, `col!~:` is `NOT LIKE`. With `<<null>>`: `col:` reads `IS NULL` and `col!:` reads `IS NOT NULL`. Under a `~` the value is an SQL `LIKE` pattern in full — `_` matches any single character too, and `\%` / `\_` are the literal characters.
- **A negation with a value never matches a NULL column.** `name!: keeper` is SQL's `name <> 'keeper'`, and that is unknown — not true — for a row whose `name` is NULL, so "everything that isn't `keeper`" quietly leaves those rows behind. It is ordinary three-valued logic and the engine is right; it is just rarely what the sentence in your head meant. Add `name: <<null>>` as a second condition if you want them too.
- **`<<run_id>>` is now a reserved name.** Like `<<null>>`, `<<uuid()>>` and `<<unique()>>`, it is answered before your own variables are looked at, so a variable you set called `run_id` — plausible, if you extract one from a column — is shadowed rather than read.
- **Only `<<unique()>>` carries the run prefix** — the `token` kind and its `email`, `slug`, `url` aliases. `<<unique(number)>>` is microseconds plus a counter, with no prefix in it, so a `LIKE` on `<<run_id>>` never reaches a column filled that way: delete those rows by a column that does carry a token, or by their key.
- **`@priority(-100)` makes the file last in *its own chain*, not last in the run.** Chains run in parallel, so the cleanup file and the files whose data it removes belong in one `@serial(<name>)` chain — otherwise it can delete rows another chain is still using.

## Plugins

`api`, `db` and `srp` are the resource kinds built into the binary. Any **other** key under `resources:` is a group served by a plugin — a shared library bddkit loads at startup — so reaching an object store, a queue or a mailbox is the same move as reaching a second database:

```yaml
resources:
  api:
    review: { base_url: http://review.local }
  s3:                       # served by a plugin, not by bddkit itself
    backups:
      bucket: acme-backups
      endpoint: http://minio:9000
    archive:
      bucket: acme-archive
default_s3: backups
```

The plugin brings its own steps, which read like any other step — a tester cannot tell a built-in from a plugin step, and a macro can call one. Instances are selected the same way as an API or a connection:

```gherkin
Given I use "archive" s3
When I upload file "report.pdf"
```

The selection resets to `default_<group>` at every scenario boundary, exactly like the current API and the current connection. With one instance in a group its `default_<group>` is inferred; with several it must be spelled out.

Which plugins are installed is **machine state, not test config**, and does not belong in the repository with the suite: the config describes the system under test, a path to a `.so` describes one laptop or one CI runner. It comes from a chain of `.bddkit/` directories, read layer by layer:

| Layer | Linux | macOS | Windows |
|---|---|---|---|
| shared | `/etc/bddkit/` | `/Library/Application Support/bddkit/` | `%ProgramData%\bddkit\` |
| user | `$XDG_CONFIG_HOME/bddkit/` or `~/.config/bddkit/` | same | `%LOCALAPPDATA%\bddkit\` |
| project | nearest `.bddkit/` walking up from the config file's directory | same | same |

Each directory is read as `plugins.yaml` then `plugins.local.yaml`; a later file overrides an earlier one **entry by entry, keyed by plugin `name`** — a project lock can pin the plugin CI uses without losing what's installed globally, and `plugins.local.yaml` points a committed entry at a local build. Commit `plugins.yaml`, gitignore `plugins.local.yaml`.

```yaml
# .bddkit/plugins.yaml
plugin:
  - name: s3
    path: /opt/bddkit/libbddkit_s3.so
```

`--bddkit-dir <dir>` (or `$BDDKIT_DIR`, the flag wins) reads exactly that one directory instead of the chain; a directory that does not exist is an error. `bddkit doctor` prints the resolved chain, every candidate file and whether it was found, and which layer each configured plugin came from.

A plugin runs inside the bddkit process with full privileges and there is no sandbox — installing one is the same trust decision as installing any other binary.

Writing one: [`docs/plugin-authoring.md`](docs/plugin-authoring.md) is the complete contract, `tests/fixtures/echo-plugin/` is a minimal plugin to copy, and [`bddkit-s3`](https://github.com/sergeym/bddkit-s3) is a real one to read — it serves the `s3` group in the example above.

## Where to look next

| For | Look at |
|---|---|
| Every step, authoritative | `BUILTIN_STEPS` in `src/steps/mod.rs` |
| How to run the examples | `examples/README.md` |
| A runnable HTTP example | `examples/api.yaml`, `examples/features/` |
| Every HTTP method, 404 included | `examples/features/methods.feature` |
| JSON matchers and paths | `examples/features/json_matchers.feature` |
| Variables: set, extract, reuse | `examples/features/variables.feature` |
| Asserting over a variable, extracting from one | `examples/features/variable_assertions.feature`, `examples/macros/users.yaml` |
| Macros, nesting, Scenario Outline | `examples/features/macros.feature`, `examples/macros/posts.yaml` |
| Non-JSON responses, form login | `examples/features/content_types.feature` |
| Polling an assertion until it passes | `examples/features/eventual.feature` |
| The mock API behind all of it | `examples/mocks/api-server.yaml` |
| Every DB step, worked through | `examples/db-features/db.feature` |
| Cleaning up a run's data | `examples/db-features/cleanup.feature` |
| The same on MySQL and MariaDB | `examples/db-features-mysql/db.feature` |
| SRP handshake, Hawk signing | `tests/features/` |
| Config schema | `src/config.rs` |
| Writing a plugin | `docs/plugin-authoring.md`, `tests/fixtures/echo-plugin/` |

## Design notes

**Why not cucumber-rs.** Three axes diverge: steps register at compile time
via proc-macros (here they load from YAML at run time), `World` is recreated
per scenario (here variables are file-scoped), and concurrency is a flat pool
over scenarios (here it's chains of files). Working around all three leaves
only its run loop while breaking its own step diagnostics — which is the part
worth strengthening. The `gherkin` crate underneath it is reused directly.

**Why no transactional rollback.** The service under test reads over its own
connection and would never see uncommitted rows, so rolling back per scenario
would break any test where a step writes and the API reads. Isolation comes
from unique data instead.

**Where this is going.** `api`, `db`, and `srp` are the resource kinds that
ship, not the ceiling — any other key under `resources:` is a capability group
a plugin serves, so reaching an object store or a mailbox is the same move as
reaching a second database. The seams that made that possible were there from
the start: options cascade per instance, `I use "<name>" <kind>` is one step
shape, and dispatch returns `passed | not yet | fatal` so eventual assertions
work without knowing what they retry. What is still missing is the
`bddkit plugin install` side of it — today `plugins.yaml` is written
by hand.

## Development

```bash
cargo test
```

Some tests need a database; `docker-compose.yml` brings up all three — PostgreSQL on `:5433` (schema from `examples/db/init.sql`), MySQL on `:3307` and MariaDB on `:3308` (both from `examples/db/init-mysql.sql`). The DB suite runs against Postgres by default; `BDDKIT_TEST_ENGINE=mysql` or `=mariadb` points it at the other two, and CI runs all three. The same file brings up [Smocker](https://github.com/smocker-dev/smocker) as the HTTP example's mock API — it seeds `examples/mocks/api-server.yaml` at startup and serves it on `localhost:8080` (web UI on `localhost:8081`).

## License

Apache-2.0. See [LICENSE](LICENSE).
