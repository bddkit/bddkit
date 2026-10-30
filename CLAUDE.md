# CLAUDE.md — bddkit

Agent orientation: the fast path into this codebase for an AI agent. This file holds only what applies to every change; subsystem rules load on demand (see "Where the rest lives").

`bddkit`: Gherkin-based acceptance-testing tool in Rust. Testers write `.feature` files (readable by non-devs); the tool validates every step up front, then runs each scenario against the systems under test. The host ships HTTP API and database steps; plugins add other kinds of system (browser, mobile, shell, object storage, …) as new step groups. `README.md` is the user-facing description of the same tool — read it for the vocabulary and config shape as a tester sees them.

## Language convention (do not get this wrong)

- **Everything in the codebase is English: code comments, runtime error messages, logs, this file, all other documentation.** No exceptions.
- **Identifiers, Gherkin step text, commit messages: English** (unchanged).
- Matcher DSL tokens (`@variableType`, `@arrayLength`, `@regExp`) are the language surface — leave as-is.
- This is about the codebase only. Conversational replies to the operator follow the operator's language (mirror Russian/Ukrainian/etc. as they write) — that is a separate axis from what goes into the repo.

## Markdown convention

**Never soft-wrap prose in `.md` files.** One paragraph — or one list item — is one physical line, however long. The renderer (browser, IDE preview, GitHub) wraps it to the reader's width; a hard line break at column 80 only fights that and makes every later edit produce a reflow diff instead of a content diff. Applies to this file, `README.md`, and anything else Markdown. Hard breaks stay meaningful where they are semantic: separate lines for list items, table rows, and code blocks.

## Architectural invariants — breaking any of these breaks the design

1. **Step matching needs no variable values.** `Registry::find` runs on RAW step text. Interpolation happens AFTER `find`, BEFORE `dispatch`, and ONLY on captured args / docstrings / table cells — never on whole step text. This is what lets `validate::check` verify every step before the first request. If you ever need a variable's value to match a step, you have broken the design — stop. A typed parameter (`{name:type}`) is the one place a value is checked against the step: the pattern accepts a literal of the type or a whole `<<…>>` slot by SHAPE, and `runner::prepare` checks the interpolated value after `find` — matching still needs no value. The step text a report, the console and the stream show is the raw feature text with ONE exception: a literal value in a `secret` parameter is written `***` (`Registry::display_text`; a value that came through a slot is left as written).
2. **State scoping is asymmetric.** Variables (`VarStack`) live per feature-FILE and persist across scenarios within it. HTTP state (`HttpState`) resets per SCENARIO (`World::reset_scenario` → `HttpState::reset`), and that reset includes the **current API resource**, which returns to `Apis::default_name()` — a scenario never inherits the API another one switched to. This mirrors Behat/Imbo — confirmed from Imbo source, not guessed. Background steps re-run before every scenario.
3. **`contains` JSON is deliberately loose; `equals` is strict.** contains = object subset + order-independent array containment + extras allowed (Imbo semantics, on purpose). equals = exact keys, element-by-element, same length. Do not "tighten" contains — that's a separate function.
4. **The dispatch match is exhaustive with NO catch-all.** `steps::dispatch` names every `StepId`. The compiler enforces completeness; a `_ =>` arm would silently defeat that when new steps are added. Never add a catch-all.
5. **Uniqueness = run_id + atomic counter.** `<<unique()>>` gives per-run prefix (time+random, sortable, greppable) + a monotonic counter → intra-run uniqueness by construction. Do not draw randomness per-value.
6. **Worker-pool runner over chains, exit codes 0/1/2/128+N.** The scheduling unit is a **chain**: files sharing a `@serial(<name>)` tag run one after another, every other file is a chain of one. `runner::build_chains` groups and sorts them by `@priority(<N>)` (higher first, default 0, a file takes the max over its tags); `runner::run_all` spawns `concurrency` tokio tasks, at least one and never more than the chain count, over one shared cursor. Each file is run by a nested `tokio::spawn` the worker awaits immediately — that bounds concurrency to the pool and isolates a panic to one file. `--fail-fast` sets an atomic flag checked at exactly two points: before a worker claims the next chain, and before it starts the next file within a chain it has already claimed. There is no checkpoint between scenarios — a file that has started always runs to completion, so a result is never partial: it stops *starting* work, it never aborts a request in flight or truncates a file mid-run. Exit: 0 all-pass, 1 any scenario failed, 2 anything that failed before the first request (validation, config, resource construction, a malformed scheduling tag) — plus ONE deliberate exception after the run: a `--junit`/`--cucumber-json` report, or the `--events` stream, that cannot be written is exit 2, because a report silently lost behind a green code is worse than a lie about when the failure happened. Failure ALWAYS dumps the HTTP exchange — masked by the privacy filter unless `--reveal`, never left out — and a file's whole output is composed by `report::render_file` and written with a single `print!` so workers cannot interleave. A fifth path is an interrupt (SIGINT/SIGTERM/SIGHUP, Ctrl-C/Ctrl-Break on Windows): `main.rs::handle_interrupt`, spawned alongside `run_all` with the `Interrupts` that `run` registered synchronously BEFORE printing `run <id>` (a signal before that is the OS default — killed, nothing cleaned, an empty event stream; one after is queued however late the task is first polled), first takes the run's ending (`RunContext::claim_end`, first caller wins — `run` takes it too once the pool drains; the winner owns the exit, the plugin shutdown and the last line of the event stream), then forces the SAME stop flag via `RunContext::force_stop` (unlike `request_stop`, not gated by `--fail-fast`), runs `Plugins::shutdown` early instead of waiting for the pool to drain, ends the event stream with the signal, and exits 128 plus the signal number (130 SIGINT, 143 SIGTERM, 129 SIGHUP; 130 for both Windows events) — a second signal skips waiting for that cleanup and exits immediately, with the FIRST signal's code. `run_file` keeps running to completion for a file already in flight; the process simply exits out from under it once cleanup (or the second signal) is done, so a file's report can be partial or missing on this one path, unlike every other exit.

## Cross-cutting gotchas

- **A failure class is set where the error is born, and never parsed back out of its text.** `failure::StepError { kind, message }` is what a step returns; `FailureKind` is exhaustive in every `match`, with no catch-all. There is deliberately NO `From<String> for StepError` (nor for `AttemptError`): a `?` on a plain `String` must not compile, because a conversion that silently makes `Unclassified` is how the scheme rots. Name the class at the site: `StepError::input(…)`, `AttemptError::not_yet(…)` (an assertion that may hold later), `AttemptError::assertion(…)` (one that cannot change), `db::sql_failure` (class from the driver error), `db::db_read` (an answer that cannot be read). Helpers that only ever fail one way keep returning `String` and the step side attaches the class (`.map_err(StepError::input)`). A macro or an include re-words the message with `StepError::wrap` and keeps the inner class. A polling assertion that runs out keeps the class of its LAST attempt (`Polling::after_not_yet`), so a service that stayed down is `transport`, not `assertion`; `ReplayError::NotYet` carries that. `Unclassified` is left for the two host faults no class fits (the clock and the system entropy), built only through `StepError::host` — a new site that produces it is a bug until proven otherwise. Two known limits, deliberate: a plugin's `fatal` is always `plugin`, even from an assertion (the host cannot tell them apart without a plugin-supplied flag and an ABI bump), and `PoolTimedOut` is `db_connection` although under `concurrency` it can be the suite's own pool size. `Db::connect` is eager, so an unreachable database exits 2 before any scenario and never gets a class; `db_connection` is only what a statement or an acquisition hits mid-run (`db::sql_kind`).
- **`gen` is a reserved keyword in edition 2024.** Never name a variable/field/param `gen`; use `generator`. (`r#gen` compiles but diverges from the codebase — don't.)
- **reqwest 0.13 gates `.form()` behind the `form` feature.** Already enabled. Adding a feature to a pinned crate is allowed; bumping the version is not.
- **gherkin `Table` rows are guaranteed rectangular** ("each row is always the same length as the first row"). Table indexing after checking the header row cannot panic on parsed input.
- **Two substitution syntaxes, different phases:** `<placeholder>` = Scenario Outline, substituted at parse time in `feature::substitute`. `<<variable>>` = runtime, substituted in `vars::interpolate`. `substitute` MUST leave `<<...>>` intact (a naive `replace("<key>",…)` corrupts `<<key>>` — it uses a single-pass regex `<<(\w+)>>|<(\w+)>` to protect double brackets).

## Configuration options

- **Every configuration option has three forms: a config key, a flag and an environment variable, and the highest wins — flag, then environment, then config, then the default.** The names are mechanical (`privacy.min_length` → `--privacy-min-length` → `BDDKIT_PRIVACY_MIN_LENGTH`), an empty variable is unset, a boolean is `true|false|1|0`, and a list option comes as `x` (adds) and `x_only` (replaces). The privacy options (`privacy.rs`) and the run's own options (`settings.rs`: one `SettingsArgs` flattened into `run` and `doctor`, one `settings::resolve` with the environment injected, `Config::apply` laying it over the loaded file and resolving again) follow it; the run's lists (`tags`, `paths`, `macro_paths`) are the one deviation, replacing the config's list with no `_only` twin; the exceptions are what cannot have all three: the config's own location, the report and events paths (opened before the config is read), per-resource keys, the `default_<group>` of a plugin group and debug mode (a per-scenario step).

## Privacy (secrets are masked, never rewritten)

The spec is `docs/events-design.md` section 7; the tester's view is the README's Privacy section. Rules for editing the mechanism load from `.claude/rules/privacy.md`. The ones that bite from anywhere: masking is render-only (`Exchange`, `RequestRecipe`, `VarStack` values and plugin arguments keep real values); every output that names a step goes through `Registry::display_text`; error text that quotes input is a leak.

## Resources and selection

APIs, databases and SRP configurations are global named resources (`resources.api` / `resources.db` / `resources.srp`, plus optional `default_api` / `default_db` / `default_srp`, inferred when a kind has exactly one resource). Any scenario reaches any of them by name — `I use "<name>" api`, `I use "<conn>" connection`. One `reqwest::Client` per API resource, built once into an `Arc<Apis>`; one `sqlx::Any` pool per DB connection for the whole run (`Db::connect`), each carrying the `&'static dyn Platform` its DSN scheme selects — plus, for the `mysql://` family, one `SELECT VERSION()` per connection to tell MySQL from MariaDB.

Test selection lives on the CLI: a positional path argument overrides `cfg.paths`, and a repeatable `--tag` filters scenarios (feature-level tags are merged onto every scenario at load; an empty selection exits 2).

**Known gap, deliberate:** `resources.srp` accepts multiple named SRP resources, but only the default one is reachable — no step exists yet to select a non-default one by name.

A `resources.<group>` key that is none of those is a plugin group, served by a loaded `cdylib`: see `src/plugin/CLAUDE.md` and `docs/plugin-authoring.md`.

## Where the rest lives

Subsystem rules load when you touch the code. Read the file first if you are about to change that area and it has not loaded.

| Touching | Read |
| --- | --- |
| `src/db/`, `src/steps/db.rs`, DB steps and SQL | `src/db/CLAUDE.md` |
| `src/plugin/`, `src/steps/plugin.rs`, plugin ABI, fixtures | `src/plugin/CLAUDE.md`, `docs/plugin-authoring.md` |
| `src/steps/` registry, expressions, adding a built-in step | `src/steps/CLAUDE.md` |
| `src/srp/` | `src/srp/CLAUDE.md` |
| `src/runner.rs`, `src/world.rs`, `src/http.rs`, polling, eventual assertions | `.claude/rules/runner.md` |
| `src/events.rs`, `src/artifacts.rs`, evidence | `.claude/rules/events.md`, `docs/events-design.md` |
| `src/privacy.rs` and anything that renders or logs text | `.claude/rules/privacy.md` |
| `src/report/` | `.claude/rules/reports.md`, `docs/report-markdown.md` |
| `src/telemetry*` | `.claude/rules/telemetry.md`, `docs/telemetry.md` |
| `src/doctor.rs`, `src/resource.rs`, `src/config.rs`, `src/dirs.rs`, `src/main.rs` | `.claude/rules/doctor-config.md` |
| `src/init.rs` / `src/upgrade.rs` | `.claude/rules/init.md` / `.claude/rules/upgrade.md` |
| `src/macros.rs`, `src/include.rs`, `src/annotate.rs`, `src/validate.rs` | `.claude/rules/macros-includes.md` |
| `tests/` | `.claude/rules/tests.md` |
| Per-module description of all of `src/` | `docs/design/module-map.md` |

## Documentation outside this repository

Plugins live in their own repositories (names in `plugin-registry.yaml`, e.g. `bddkit/bddkit-exec`), and other reference material may sit outside the tree. This file does not say where a checkout or a document is on the operator's machine, on purpose. When a task needs such material and your memory has no entry saying where to read it, do not guess a path: ask the operator for a URL or a folder, then save the answer as a `reference` memory so the next session does not ask again.

## Module map (`src/`), one line per area

`main.rs` CLI and orchestration · `runner.rs` worker pool, chains, `run_file` · `world.rs` per-file state · `steps/` registry, expressions, step functions · `db/` SQL building and engine platforms · `plugin/` third-party `cdylib` step libraries · `http.rs` / `hawk.rs` API resources and signing · `json/` path reader and matcher · `srp/` SRP-6a · `vars.rs` variables and interpolation · `feature.rs` / `macros.rs` / `annotate.rs` / `validate.rs` parsing, macros, static check · `privacy.rs` secrets and the filter · `events.rs` / `report/` event stream and reports · `config.rs` / `dirs.rs` / `settings.rs` / `options.rs` configuration and its layers · `doctor.rs` / `resource.rs` / `init.rs` / `upgrade.rs` / `telemetry*` the other commands · `failure.rs` `FailureKind` and `StepError`.

## Dev workflow

- do not use `superpowers` for simple tasks and non-code changes.
- when changing Rust code, use `rust-best-practices` skill.
- TDD per task: failing test → RED → implement → GREEN → commit. Tests assert real behavior, never mock theater.
- Dependency VERSIONS are frozen (features may be added). Edition 2024, toolchain 1.97 — CI pins the exact version in the workflows (`toolchain: "1.97.1"`), so a new clippy lint in a fresh stable can never redden an unrelated PR. Do NOT pin it with a `rust-toolchain.toml`: on a machine whose `stable` already is that version rustup installs a second copy of the same toolchain and can leave it half-installed.
- `cargo test` (unit + `tests/acceptance.rs` e2e against the axum stub). `cargo clippy --all-targets` must be clean — no warnings. `cargo fmt --check` must be clean too, at the root and in both fixture crates (`tests/fixtures/echo-plugin`, `tests/fixtures/worker-plugin`) — run `cargo fmt` (and the fixture equivalents) before committing rather than letting CI catch it. `HttpState::current` carries an `#[allow(dead_code)]` because only tests read it; that is correct, don't delete the method.
- `selftest/` runs the binary on a fixture that fails on purpose, through the published `exec` plugin, and asserts on the console, the JUnit report and the stream (`./target/debug/bddkit run --config selftest/bddkit.yaml`, after `bddkit plugin install exec`; CI's "Self-test" step does the same). It is not `examples/`: that must stay green. A new cross-cutting behaviour that only shows in bddkit's own output (masking, reports, exit codes) gets a scenario there as well as a Rust test.
- Acceptance gate: never weaken a `.feature` or an assertion to force green. A red feature file means a real bug — fix the code or report it, don't patch the test.
- Load-bearing acceptance tests, each pinning an invariant that is easy to break silently: `one_scenario_can_call_two_different_apis` (resources), `two_feature_files_run_at_the_same_time` + `a_single_worker_cannot_release_the_barrier` + `two_files_in_one_serial_chain_never_run_together` (parallelism and per-file variable isolation, proven by a barrier stub rather than timing), `tests/features/srp_login.feature` (registration + authenticated login end to end).
- Subagent reports have miscounted tests and quoted non-existent text — verify claims against files, don't trust report prose.
