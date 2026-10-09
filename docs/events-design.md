# Run event stream, privacy and artifacts — specification

**Status: specification.** The maintainer took the decisions below on 2026-10-06, the one on sensitive fields (7.5) on 2026-10-07, and the one on configuration values (7.6) on 2026-10-08; this replaces the design draft of 2026-10-04 and its open questions Q1–Q14. Three labels are used throughout. **Decided** — confirmed by the maintainer. **Detail** — chosen by the spec author to make a decision implementable and never discussed; section 13 lists every one of them, and a reviewer should confirm or change each. **Fact** — read from the code at the baseline.

**Baseline.** `bddkit/bddkit` `main` at `f303902` (0.2.2 plus unreleased work), `bddkit-browser` at `07c7beb`, `bddkit-exec` at `b641e71`, `bddkit-s3` at `0db5f06` (all 0.1.0), read on 2026-10-06. References are by function or type name, not line number. Section 14 lists every claim this specification leans on — re-check them against the files before trusting the prose.

**Scope.** Only what changes in the `bddkit` binary. The hub, the sender that ships a run to it, the upload protocol and the UI are separate projects and appear here only as the reason a rule exists.

## 1. Why

`--junit` and `--cucumber-json` describe a run after it is over. Anything that wants to follow a run while it happens — a local UI, a hub, a CI log tail, a later replay — has nothing to read, and a step that passed has no way to show what it did. Closing that gap means writing far more of a run to disk than bddkit does today, which makes two further things necessary rather than optional: a way to keep secrets out of what is written, and a stable place for the files a step produces.

## 2. Goals and non-goals

**Goals**

- **G1 Live.** A consumer learns that a step started or finished without waiting for its file to end.
- **G2 Complete.** The stream alone is enough to rebuild what the reports contain.
- **G3 Crash-tolerant.** Any prefix of the stream is line-valid; a run that dies leaves everything written so far readable.
- **G4 Free when off.** Without the flags: no file, no channel, no allocation per step, and console output and both reports unchanged except where masking (section 7) deliberately changes them.
- **G5 Additive.** A new event type or field must not break an older consumer.
- **G6 Secrets stay on the runner.** Masking happens in the core, before anything is written or printed. A consumer downstream never has the chance to leak what it never received.

**Non-goals**

- A network sink in the core. bddkit writes to a path; shipping is someone else's job (section 12).
- `--events -` (stdout). Decided against: stdout carries the human console output.
- Cucumber Messages compatibility. Decided against (section 12).
- Debug output (`Show …`, `Print …`, SQL debug lines) as events. Excluded.
- Running a single scenario, and a machine-readable inventory of a suite. Separate questions.
- Any change to the plugin ABI. Deferred to work package 6, which is not specified here (section 10).

## 3. Work packages

Each package is one issue. Packages 1–3 are independent of each other and can be built in parallel.

| # | Package | Sections | Depends on |
|---|---|---|---|
| 1 | Signal exit codes: SIGTERM 143, SIGHUP 129 | 6 | nothing |
| 2 | Event stream, lifecycle: `--events`, the writer, nested steps | 5 | nothing |
| 3 | Privacy: `privacy` config, `secret` type, name rule, sensitive fields, value filter, `--reveal` | 7 | nothing |
| 4 | Artifacts: `--artifacts-dir`, relative paths, a host allocator | 8 | 2 |
| 5 | Evidence: `--evidence`, `vars`, `evidence` and `attempt` events | 9 | 2, 3, 4 |
| 6 | Plugins: evidence from passing steps, secrets, sensitivity in step specs | 10 | 5, and a design discussion |

Package 3 is worth shipping on its own: it closes a leak that exists today (section 4, last fact).

## 4. What exists today (Fact)

- **Results.** `report::FileResult` → `ScenarioResult { name, line, failure, steps, duration }` → `StepResult { keyword, text, line, status, duration, warnings }`. `StepStatus` is `Passed | Failed | Skipped`. `text` is the raw feature text.
- **Where results are built.** `runner::run_file`: after the first failure every remaining top-level step is recorded `Skipped` without running. The failure text is the step text, the error, and `world.http.last()` when there is one. A failed plugin scenario reset is the scenario's failure before any step runs.
- **Scheduling.** `runner::run_all` runs chains on a worker pool; a file is one task; a panicking file becomes one synthetic failed scenario (`runner::panicked_file`). The file's console output is printed with one `print!` under the results lock.
- **Outline rows share a line.** `feature::expand_outlines` gives every row `line: sc.position.line` and tells them apart only by a `[row values]` suffix on `name`.
- **Nested execution.** `runner::execute_step` recurses for a macro call and for `I include`, up to depth 16. A macro body step is an `ExpandedStep` with an empty `keyword` and the CALLER's `line`; `MacroStep` has no line of its own, `MacroDef` has `source` and `line`. An included scenario runs the included file's Background steps first, each with its own line in that file.
- **Interpolation.** `runner::prepare` interpolates captures, the docstring and table cells after `Registry::find_with_params` and before dispatch. A slot is `<<name>>` or `<<function(args)>>`; the functions are `unique`, `uuid`, `null`, `run_id`. A variable name matches `[^\W\d]\w*`, and names in the wild are both `snake_case` and `camelCase` (`last_insert_id_accounts`, `sessionKey`).
- **Variable writes.** Every write goes through `VarStack::set` or `VarStack::set_global`. Prefixed names are built as `<prefix>_<field>`: SRP writes `_salt`, `_verifier`, `_a`, `_A`, `_M1`, `_M2`, `_sessionKey`; AES writes `_ciphertext`, `_ivHex`; `I include … with prefix "<p>"` renames each export to `<p>_<name>`.
- **Step declarations.** Builtins and macro templates are Cucumber Expressions with `{name}` or `{name:type}`; `expression::TYPES` holds `text`, `any`, `uint`, `int`, `float`, `word`, `method`. Built-in steps that take a secret today: the Hawk key, the AES key, the SRP password. Plugin steps are declared as regexes in `StepSpec { pattern, group, kind, description }`.
- **Report paths.** `ReportArgs { junit, cucumber_json }` is shared by `run` and `doctor`. `report::prepare` creates-or-truncates each path and closes it, first thing, before the config is read; the reports are written after the run. A report that cannot be written is exit 2.
- **Process exit and signals.** `main` leaves through `std::process::exit`. `main::wait_for_interrupt` waits for SIGINT or SIGTERM (Ctrl-C or Ctrl-Break on Windows); `main::handle_interrupt` stops new work, runs `Plugins::shutdown`, and exits 130 for either signal; a second signal exits at once. SIGHUP is not handled. `tests/plugin.rs::sigint_stops_new_work_and_still_drops_plugin_instances` pins the SIGINT path.
- **Artifacts.** `Plugins::next_artifacts_dir` hands each plugin dispatch a fresh `<temp>/bddkit-<run_id>/<six digits>` path, not created by the host. The allocator lives on `Plugins`, so a run with no plugin has none. The per-file workspace is `<temp>/bddkit-<run_id>/workspace/<six digits>`, created by the host and never deleted.
- **Plugin evidence.** `DispatchResult.diagnostics` is a list of `Diagnostic { title, kind, content, path }`. On `passed` the runner uses only `vars` and drops the rest. `bddkit-exec` writes text over 16 KiB (`INLINE_LIMIT`) to a file and returns its path. `bddkit-browser`'s `I take a screenshot` writes the PNG and can mention its path only on stderr in debug mode.
- **Config.** `Config` has no `deny_unknown_fields` and keeps unknown top-level keys in `extra`; only keys starting with `default_` are ever checked. Paths that are properties of a run come from flags; `--config` also reads `$BDDKIT_CONFIG` through `std::env::var_os`, with the flag winning. `clap` is built with the `derive` feature only.
- **Requests and bodies.** A request keeps its query parameters and its form fields as name–value pairs and its body as a string (`http::RequestRecipe`); a form is sent as `name=value` pairs joined by `&`. A response body is held as a string. `HttpState::send` and `replay_last` store the exchange only after `execute` has returned it.
- **A leak that exists now.** `impl Display for Exchange` prints every request header in full, then the request and response bodies cut to 600 characters. That text is the failure dump on the console and inside both reports. Nothing in `src/` masks anything.

## 5. The event stream (package 2)

### 5.1 CLI surface

**Decided.** `bddkit run --events <path>`. The path is a regular file, or anything else that can be opened for writing by name — a FIFO, a `/dev/fd/N` from process substitution. `-` is not special and stdout is not a target.

**Decided.** The path can also come from the environment, so a wrapper can set it without rewriting someone else's command line. **Detail:** the variable is `BDDKIT_EVENTS`, read by `run` only, with the flag winning, the way `$BDDKIT_CONFIG` is read today.

**Decided.** The path is opened exactly once, at the point `report::prepare` runs today — first, before the config is read — and the handle is kept until the process ends. It does not go through `report::prepare`: create-then-close-then-reopen makes a FIFO's reader see end-of-file before the first event. On a regular file the open truncates, so a refused run leaves an empty stream. Opening a FIFO blocks until a reader has it open; that is the operator's contract, not something to work around.

**Detail.** `doctor --events <path>` reports a `reports` row like the other paths. For a path that already exists and is not a regular file it reports `ok` without opening it, because opening it would hand the reader an end-of-file.

### 5.2 Format

NDJSON: one JSON object per line, `\n`-terminated, UTF-8. Every line carries:

- `type` — the event name. A consumer **must ignore** a type or a field it does not know.
- `seq` — `u64` from 0, strictly increasing, assigned by the single writer. The stream's total order.
- `t` — milliseconds since the run started, from a monotonic clock at the emit site.

**Decided.** Wall-clock time appears once: `run_started.started_at_unix_ms`. Everything else is relative.

The first line is `run_started` and carries `schema: 1`. A breaking change bumps `schema`; a new type or field does not.

**Decided.** In every string of the stream the SQL NULL sentinel (`vars::NULL_SENTINEL`) is written as `<<null>>`, the way `runner::debug_display` already prints it — never as NUL bytes and never as U+FFFD.

| What the file holds | Meaning |
|---|---|
| nothing | the run was refused (exit 2); the reason is on stderr |
| lines, the last one `run_finished` | the run ended; `exit` is the exit code and `signal`, when present, says it was interrupted |
| lines, no `run_finished` | the process was killed, or exited on a second signal |

**Decided.** A refused run writes no event at all.

### 5.3 Identity and nesting

- `file` — the feature path as the reports print it (`feature::display_path`).
- `scenario` — ordinal of the scenario within this file in this run, from 0. A join key inside one stream; a `--tag` filter shifts it.
- Stable scenario identity is `line` plus `example` — the 0-based row ordinal across the outline's `Examples` tables, absent for a plain scenario. `ExpandedScenario` gains a field for it.
- `step` — a counter per scenario, from 0, assigned when the step starts, **at any depth**.

**Decided.** Steps run inside a macro or an included scenario are events too. The calling step is the group: every nested step carries `parent`, the `step` number of its caller. A consumer builds the tree from `parent`; the depth is at most 16. The reports do not change — there a macro call or an include stays one step.

A nested step also carries `source`, the file its text is written in. For a step of an included scenario, including that file's Background, `line` is its own line in `source`. **Detail:** for a macro body step `line` is the macro definition's line (`MacroDef.line`) and `index` is the step's 0-based position in the body, because `MacroStep` carries no line.

Only top-level steps are ever reported as skipped. When a nested step fails, the rest of that body does not run and is not reported, and the failure closes each enclosing step in turn.

### 5.4 Lifecycle events

| `type` | Emitted | Fields beyond the envelope |
|---|---|---|
| `run_started` | where `run` prints `run <id>` today | `schema`, `bddkit`, `run_id`, `started_at_unix_ms`, `concurrency`, `files`, plus `privacy`, `evidence`, `artifacts_dir` from sections 7–9 |
| `file_started` | top of `run_file` | `file`, `name` |
| `scenario_started` | after the scenario reset, before the first step | `file`, `scenario`, `name`, `line`, `example` |
| `step_started` | once the step's arguments are interpolated, or at once if that fails | `file`, `scenario`, `step`, `keyword`, `text`, `line`; nested steps add `parent`, `source`, `index` |
| `step_finished` | when the step returns | `file`, `scenario`, `step`, `status` (`passed` or `failed`), `duration_us`, `warnings` |
| `step_skipped` | for each top-level step after a failure | `file`, `scenario`, `step`, `keyword`, `text`, `line` |
| `scenario_finished` | after the last step | `file`, `scenario`, `status`, `duration_us`, `failure` (the text the console prints, or `null`) |
| `file_finished` | end of `run_file`; by the worker for a panicked file | `file`, `scenarios`, `failed`, `panicked` |
| `run_finished` | last line | `exit`, `signal` (only when interrupted), `files`, `scenarios`, `failed`, `duration_ms` |

`text` is the raw step text, with one exception defined in 7.2. `keyword` is as the feature file has it, trailing space included; a macro body step has an empty keyword. The totals in `file_finished` and `run_finished` are the ones `report::print_summary` counts, so a panicked file is one scenario, one failed. A panicked file can leave a scenario or a step open; `file_finished` with `panicked: true` closes them, and a consumer must accept that.

Values below are illustrative.

```
{"type":"run_started","seq":0,"t":0,"schema":1,"bddkit":"0.3.0","run_id":"3n2k9a0f1x7q","started_at_unix_ms":1791244800000,"concurrency":8,"files":2}
{"type":"file_started","seq":1,"t":1,"file":"features/company.feature","name":"Companies"}
{"type":"scenario_started","seq":2,"t":1,"file":"features/company.feature","scenario":0,"name":"registering a company charges the account","line":12}
{"type":"step_started","seq":3,"t":2,"file":"features/company.feature","scenario":0,"step":0,"keyword":"Given ","text":"I register a buyer","line":13}
{"type":"step_started","seq":4,"t":2,"file":"features/company.feature","scenario":0,"step":1,"parent":0,"source":"macros/buyer.yaml","line":4,"index":0,"keyword":"","text":"I request \"/api/v1/buyers\" using HTTP POST"}
{"type":"file_started","seq":5,"t":3,"file":"features/login.feature","name":"Login"}
{"type":"step_finished","seq":6,"t":40,"file":"features/company.feature","scenario":0,"step":1,"status":"passed","duration_us":37900,"warnings":[]}
{"type":"step_finished","seq":7,"t":40,"file":"features/company.feature","scenario":0,"step":0,"status":"passed","duration_us":38100,"warnings":[]}
{"type":"run_finished","seq":58,"t":1310,"exit":1,"files":2,"scenarios":5,"failed":1,"duration_ms":1310}
```

### 5.5 The writer

- **One writer, no mutex.** Emit sites send an event into a channel; one writer owns the handle, assigns `seq`, serialises, writes. Lines of different files interleave; a line is never torn.
- **Decided: the channel is unbounded.** A send never waits, so a slow reader cannot slow a test or change its timing, and no event is dropped. Growth is bounded by the number of steps times the size cap of section 9.3.
- **Writes stay off the async workers.** The target can be a pipe whose reader has stalled; a blocked write must park a dedicated thread, or a `spawn_blocking` task, never an executor thread.
- **Flush once per batch:** drain what is queued, write, flush.
- **Plumbing.** `World` holds an optional emitter with the current file and scenario, the per-scenario step counter and a stack of in-flight step numbers that `execute_step` pushes and pops; the top of the stack is what an evidence event attaches to. With no `--events` the emitter is `None` and each emit site is one branch.
- **Shutdown.** `main` exits through `process::exit`, so `run` emits `run_finished`, closes the channel and waits for the writer before it returns. `run_finished` is emitted after the reports are written, so its `exit` is the code the process really returns.
- **Plugins never write to the stream.** They have no handle to it and get no callback.

### 5.6 Failure semantics

- **The path cannot be opened** → exit 2 before the config is read, as for `--junit`.
- **A write fails mid-run** (disk full, the pipe's reader went away) → the writer keeps the first error, stops writing and keeps draining. The run is not aborted. After the run the error is printed and the exit code is 2: the existing exception for a lost report, extended to the stream for the same reason.
- **Interrupt** → section 6.3.

## 6. Signals and exit codes (package 1)

### 6.1 Codes

**Decided.** The exit code follows the shell convention of 128 plus the signal number, which Docker and Kubernetes tooling reads the same way.

| Signal | Today | After |
|---|---|---|
| SIGINT (Ctrl-C) | 130 | 130 |
| SIGTERM (Kubernetes, a cancelled CI job) | 130 | 143 |
| SIGHUP (the terminal closed) | not handled: killed with no plugin cleanup | 129, with the same cleanup |
| Ctrl-C or Ctrl-Break on Windows | 130 | 130 |

SIGKILL cannot be caught; nothing changes there.

### 6.2 Behaviour

All three signals take the path `handle_interrupt` takes today: stop starting new work, run `Plugins::shutdown`, exit. The stderr line names the signal. **Detail:** a second signal still exits immediately, with the code of the first.

This changes documented behaviour: the exit-code list and "Interrupting a run" in `README.md`, and invariant 6 in `CLAUDE.md`, which is headed "exit codes 0/1/2/130".

### 6.3 The stream on an interrupt

**Decided.** An interrupted run ends its stream with `run_finished` carrying `exit` and `signal` (`"SIGINT"`, `"SIGTERM"`, `"SIGHUP"`, `"CTRL_C"`, `"CTRL_BREAK"`), so a consumer can tell "asked to stop" from "killed".

Files already in flight keep running until the process exits, so the terminator races their events. The writer settles it: the handler sends the terminator through the same channel after plugin cleanup; the writer writes it, flushes, and drops everything that arrives afterwards; the handler waits for the writer and then exits. A second signal exits without the terminator, which is why a stream without `run_finished` stays a legal state. This is the part of the design most worth an adversarial review.

## 7. Privacy (package 3)

### 7.1 Model

**Decided.** There is one mechanism — a set of secret values and a filter that replaces every occurrence of them with `***` in what bddkit writes or prints — and four ways a value gets into the set:

| Who marks | How | What it covers |
|---|---|---|
| the step's author | the parameter type `secret` in the step declaration | a step's secret arguments, literal ones included |
| the scenario's author | a privacy word in the variable's name | anything saved into a variable |
| a resource | declaring which of its own values are sensitive | what travels outside any body: authorization headers, cookies |
| the system under test | a field word in the name it gave the value | what nobody in the suite named: a token in a login response |

The core knows nothing resource-specific. It owns the two word lists, the minimum length and the filter; the API module owns its header names and knows where its own named values are.

### 7.2 The `secret` parameter type

**Decided.** `expression::TYPES` gains `secret`. It matches exactly what `text` matches and marks the parameter:

```
I sign the next request with Hawk id "{id}" and key "{key:secret}"
```

The interpolated value of a `secret` parameter enters the set when the step is prepared. A value that arrived through a slot leaves the step text as written (`key "<<hawk_key>>"`). A literal one is replaced in the output text: `key "***"`. That is a deliberate exception to "step text in a report is the raw feature text", and it applies to the console, both reports and the stream alike.

Macro templates use the same parser and therefore the same syntax: `step: I log in as {user} with password {pw:secret}`.

**Detail.** Under the quoting rule proposed in issue #79, `secret` is a `required` type like `text`: its values can contain spaces and quotes stay mandatory.

**Detail.** The built-in declarations are updated: the Hawk `key`, the AES `key`, the SRP `password`. The SRP steps also register the private values they compute (`<prefix>_a`, `<prefix>_sessionKey`) whatever the prefix is.

### 7.3 Privacy words in variable names

**Decided.** A variable is sensitive when a privacy word is one of the `_`-separated words of its name, at any position, compared without case:

| Name | Sensitive | Why |
|---|---|---|
| `access_token_secret` | yes | last word |
| `secret_reg_verifier` | yes | first word |
| `reg_secret_verifier` | yes | a middle word — what `as "reg_secret"` turns into |
| `buyer_token_secret` | yes | export `token_secret` included `with prefix "buyer"` |
| `secretary_id` | no | not a whole word |
| `sessionSecret` | no | camelCase is not split; write `session_secret` |

The value enters the set whenever such a variable is written. `VarStack::set` and `set_global` are the only write paths, which makes them the one place to hook; a macro parameter with such a name is covered by the same hook.

### 7.4 What a resource declares

**Decided.** The API module masks the values of sensitive request and response headers. The names are per resource and replace the module's default when given:

```yaml
resources:
  api:
    backend:
      base_url: https://staging.example.test
      sensitive_headers: [authorization, cookie, set-cookie]
```

**Detail.** The default is `authorization`, `cookie`, `set-cookie`, matched without case; `API_FIELDS` gains the entry. A masked header keeps its name and shows `***` as its value, and the value joins the set.

This is why the setting does not live in the `privacy` section: if the API and the database later become plugins, their settings move with them unchanged.

### 7.5 Sensitive fields

**Decided (2026-10-07).** A value is also sensitive because of the name the system under test gave it. Without this rule masking only works forwards: a login response that returns a token would show it in that step's failure dump and evidence, before any later step could save it into a sensitive variable.

A field is sensitive when a field word is one of the words of its name. **Detail:** the name is split on `_`, `-` and lower-to-upper case boundaries and compared without case:

| Field name | Sensitive with `fields: [password, secret, token]` | Why |
|---|---|---|
| `password` | yes | the word itself |
| `access_token`, `accessToken`, `refresh-token` | yes | `token` is a word of each |
| `client_secret` | yes | `secret` is a word |
| `tokenizer` | no | not a whole word |
| `key` | no | not in the list |

Case boundaries are split here and not in 7.3 because these names belong to the system under test: the suite's author cannot rename them.

**Detail: where the API module looks,** on every exchange, request and response alike:

- object keys at any depth of a body that parses as JSON;
- the names of form fields;
- the names of query parameters.

What happens to the value under a sensitive name:

- a string is masked where it stands, and joins the set when it is at least `min_length` long;
- a number is masked where it stands and never joins the set;
- `true`, `false` and `null` are left alone, and an object or an array is descended into, not masked whole.

The rest of a body is left exactly as it was sent or received: a body is never re-serialised to mask it.

The values join the set before the exchange is stored, so they are already masked in that step's own failure dump and evidence, and in everything after it.

**Detail.** The host applies the same rule to the content of a plugin diagnostic whose `kind` is `json`.

The list lives in the `privacy` section (7.7), not on a resource, because it is vocabulary like `words`. Where to look for the names stays each resource's own business.

### 7.6 The configuration

**Decided (2026-10-08).** Values from `bddkit.yaml` are masked systematically, not wherever someone remembered to. As soon as the config is parsed and its `${…}` references are expanded — before a resource is built, a connection is opened or a plugin validates its config — the host walks the whole document, plugin group bodies included, and adds to the set:

- **Detail:** the value under any key whose name contains a field word (7.5, same splitting): `password`, `secret_access_key`, `client_secret`, `api_token`;
- **Detail:** the password of any value that is a URL with credentials in it: `postgres://app:hunter2@db/app` adds `hunter2`;
- the values of sensitive `default_headers` (7.4).

The walk runs on the expanded document, so a secret that arrives through `${DB_PASSWORD}` or a `.env` layer is caught the same way as one written in the file.

**Decided: one gate.** Masking is a property of output, not of a call site. Everything any command prints passes through the same filter: `run`, `doctor` in its human and `--json` forms, `resource add` (which prints the block it would have written), and the `error:` line `main` prints when a command fails. That includes text bddkit did not write — a driver's connection error, a plugin's `validate_config` or `probe_config` refusal — and therefore covers whatever a sender later takes from stderr.

Before the walk nothing from the file is known: if the config cannot be parsed, the defaults of 7.7 apply. **Detail:** `--reveal` is accepted by `doctor` as well as by `run`.

### 7.7 The `privacy` section

**Decided.** The vocabulary is configurable, because a team may not want the word `secret` at all:

```yaml
privacy:
  words: [secret, sensitive, gdpr]
  fields: [password, secret, token, otp]
  min_length: 8
```

`words` are for names the scenario's author chooses (7.3); `fields` are for names the system under test chose (7.5). Without the section: `words: [secret]`, `fields: [password, secret, token]`, `min_length: 8`. A given list replaces its default, it does not extend it. Every word masks the same way.

**Detail.** `privacy` is a named field of `Config` with `deny_unknown_fields` on its own struct, as hand-written YAML has everywhere else. Each word of either list must match `[A-Za-z0-9]+`. An empty `words` is refused at load; an empty `fields` is allowed and switches the field rule off.

### 7.8 The filter

- **Where it applies.** Everything bddkit emits about a run: the failure text (console and both reports), step warnings, and every string of the event stream, including evidence and the text files the host writes for it. **Detail:** also the host's own debug output on stderr (`I am in debug mode`, `Show …`, `Print …`).
- **When.** Where the text is produced, inside `run_file`, with that file's set — not at print time, when the file's `World` is gone. The set is the run-wide values (declared by resources at startup) plus the values the file registered.
- **Short values.** A value shorter than `min_length` is masked where its position is known — a `secret` parameter, a `vars` entry, a sensitive header, a sensitive field — but does not enter the set: masking every `42` in the output would destroy it.
- **Before any cut.** The failure dump shortens a body to 600 characters. Masking comes first, so a secret that straddles the cut cannot show its first characters.
- **Detail.** Longer values are replaced before shorter ones, so a value that contains another is masked whole. The mask is the literal `***`.
- **Plugins.** A plugin receives real values; it needs them. What it sends back as `error` and `diagnostics` is rendered by the host and passes through the filter like any other text.

### 7.9 `--reveal`

**Decided.** `bddkit run --reveal` turns masking off for that run. It is a flag and only a flag: there is no way to disable masking in `bddkit.yaml`, because a line committed for one debugging session would switch it off in CI.

**Decided.** The stream records what was in force: `run_started.privacy` is `{"masking": true, "words": ["secret"], "fields": ["password", "secret", "token"]}`. A hub can then refuse or flag a run that arrived unmasked.

**Detail.** `--reveal` prints one line on stderr saying masking is off.

### 7.10 `doctor`

**Decided.** `doctor` points at names that look sensitive and carry no privacy word: `password`, `token` or `key` as a word of the name. This list only hints; it never changes what is masked.

**Detail.** The check reads the `<<…>>` slots in feature files and macro bodies. It is one `privacy` row with status `ok` and the names in its detail — `doctor` has no warning status, and a heuristic must not fail a suite over `primary_key`.

### 7.11 Known limits

- **Masking is forward-only for what nothing names.** A value is masked from the moment it enters the set. A secret in a body that is not JSON, or under a name no field word matches, stays visible until a later step saves it into a sensitive variable.
- **A field word can be too eager.** With `token` in the list, the value of `next_page_token` is masked wherever it appears afterwards. Replace the list when that hurts.
- **Database steps are not covered by `fields`.** A column named `password` in `I have … with "password: …"` is not matched; pass the value through a sensitive variable.
- **Derived values are not recognised:** a hash, a signature or a ciphertext of a secret is a different string. Sensitive headers cover the common case.
- **Marking is by name.** `_secert` masks nothing and nothing complains, apart from the `doctor` hint. Renaming a variable changes what is masked.
- **A family is masked whole.** `as "secret_reg"` masks every SRP variable of that login, the public ones included.
- **Images are not filtered.** A screenshot of a password field ships as it is.
- **A masked assertion is harder to debug.** Use `--reveal`.

## 8. Artifacts (package 4)

**Decided.** `bddkit run --artifacts-dir <dir>` sets the root under which the run's evidence files are written. **Detail:** it also reads `BDDKIT_ARTIFACTS_DIR`, on the same terms as `BDDKIT_EVENTS`. Without it the root stays where it is today, `<temp>/bddkit-<run_id>`.

**Decided.**

- **Paths in the stream are relative to the root,** with forward slashes. The events file and the directory are then a self-contained pair: archive them, move them, replay them.
- **Everything the stream references lies under the root.** If a plugin's diagnostic points at a file outside it, the host copies the file in before emitting the event.
- **The stream carries no URL.** Whoever serves a file makes the link: a local UI reads the disk, a hub signs a short-lived one.
- **The core deletes nothing.** Retention belongs to whoever owns the directory.
- **A file is always a reference, never inline,** whatever its size.

**Decided: three guarantees a consumer may build on.**

1. An event that references a file is emitted only after the file is completely written. The event is the signal that the file can be read; nobody has to watch the directory.
2. The path is relative, as above.
3. The event carries `size` in bytes and `media_type`, which an uploader needs before it has read the file.

**The workspace is not an artifact.** Files a test works with — an object downloaded from a bucket, a file to upload — live in the per-file workspace, are never referenced by the stream and are never shipped, unless a plugin itself names one as evidence.

**Detail.**

- The allocator moves out of `Plugins` into a run-wide value both the host and the plugins use, so a run with no plugin can still write a file. The layout stays `<root>/<six-digit counter>/<name>`.
- The workspace does not move: it stays under the temporary directory whatever `--artifacts-dir` says.
- `run_started.artifacts_dir` is the absolute root as resolved at run time, a convenience for a consumer on the same machine.
- `media_type` comes from the file extension through a small table, with `application/octet-stream` as the fallback.

## 9. Evidence (package 5)

### 9.1 The switch

**Decided.** `--events` alone writes the lifecycle of section 5. Evidence — resolved values, HTTP exchanges, SQL, plugin diagnostics, polling attempts — is written only when asked for. **Detail:** the flag is `bddkit run --evidence`; it requires `--events`, and `run_started.evidence` records it.

Off by default because evidence puts response bodies and query parameters of passing steps on disk, and masking cannot recognise what nobody marked (7.11).

### 9.2 Resolved values: `vars`

**Decided.** `step_started` keeps the raw text and carries the resolved slots beside it, the way a SQL log shows a statement and its parameters:

```
{"type":"step_started","seq":12,"t":51,"file":"features/company.feature","scenario":0,"step":2,"keyword":"Given ","line":14,"text":"I have \"accounts\" with \"email: <<unique(email)>>, balance: 100\"","vars":[["unique(email)","u3n2k9a0f1x7q7@example.test"]]}
{"type":"step_started","seq":15,"t":60,"file":"features/company.feature","scenario":0,"step":3,"keyword":"And ","line":15,"text":"I sign the next request with Hawk id \"session-1\" and key \"<<hawk_key>>\"","vars":[["hawk_key","***"]]}
```

- `vars` is a list of `[expression, value]` pairs, not a map: `<<unique(email)>>` yields a new value each time and can occur twice in one step. The order is the order of appearance — captures, then the docstring, then table cells row by row.
- The text stays identical from run to run; only `vars` differs.
- `<<null>>` is the JSON `null`, not a string.
- A value is `***` when its variable's name is sensitive, when it sits in a `secret` parameter, or when the filter matches it.
- `interpolate` has to return the pairs it resolved; today it returns only the resulting string.

### 9.3 The `evidence` event

**Decided.** There is one event type for all evidence, whatever produced it. The core has no `http_exchange` and no `db_query` type: the built-in API and database speak the form a plugin's diagnostic already has, so moving them into plugins later changes nothing in the stream.

Fields beyond the envelope: `file`, `scenario`, `step` (the innermost step in flight), `group` (`api`, `db`, or a plugin group), `resource` (the instance name), `kind`, an optional `title`, and either `content` or a file reference (`path`, `size`, `media_type`).

`kind` is free-form text, as it is for plugin diagnostics; a consumer shows a kind it does not know as text. For two kinds `content` is an object by convention:

- `http` — `method`, `url`, `request` and `response` each with `headers` (a list of `[name, value]` pairs, order and repeats kept) and `body`, plus `status`, and `duration_us` when measured.
- `sql` — `sql`, `binds`, and `duration_us` when measured.

Illustrative values again:

```
{"type":"evidence","seq":21,"t":88,"file":"features/company.feature","scenario":0,"step":4,"group":"api","resource":"backend","kind":"http","content":{"method":"POST","url":"https://staging.example.test/api/v1/companies","request":{"headers":[["authorization","***"],["content-type","application/json"]],"body":"{\"name\":\"Acme\"}"},"status":201,"response":{"headers":[["content-type","application/json"]],"body":"{\"id\":42}"}}}
{"type":"evidence","seq":24,"t":93,"file":"features/company.feature","scenario":0,"step":5,"group":"db","resource":"main","kind":"sql","content":{"sql":"SELECT 1 FROM accounts WHERE balance = $1::numeric","binds":["75"]}}
{"type":"evidence","seq":30,"t":140,"file":"features/company.feature","scenario":0,"step":7,"group":"browser","resource":"shop","kind":"image","title":"Screenshot","path":"000007/screenshot.png","size":48213,"media_type":"image/png"}
```

**Decided: the size rule.** Text of at most 16 KiB stays in the event — a 300-byte response or a query belongs in the timeline, not behind a link. Longer text is written to a file under the artifact root and referenced. Sixteen KiB is the limit `bddkit-exec` already applies, so host and plugins share one number. Inside an `http` object the rule applies per body: a long body is replaced by `{"path", "size", "media_type"}`. Text the host writes to a file goes through the filter first.

**Sources.**

- **API.** One event per exchange, emitted wherever `HttpState` stores one — an explicit send and every polling replay. The host holds a body as a `String`, so a non-text body is evidenced as it is held; keeping its bytes is out of scope.
- **Database.** One event per statement, at the sites that print SQL in debug mode today.
- **Plugins.** The diagnostics of a reply become `evidence` events whenever the reply carries any, on any status, with `title`, `kind`, `content` and `path` passed through and `size` and `media_type` added by the host. This needs no ABI change. Telling a plugin that evidence is wanted is package 6.

### 9.4 `attempt`

One event per `not_yet` of an eventual assertion, host and plugin paths alike: `file`, `scenario`, `step`, `n` (from 1), `message`. Written only with `--evidence`, because the message quotes the values that did not match.

## 10. Plugins (package 6) — deferred

Not specified here; it needs its own discussion before any code. What that discussion has to settle:

- **A hint in the dispatch request** that evidence is wanted, so a plugin does not take a screenshot nobody will read. It would be an eighth key. `docs/plugin-authoring.md` says "All seven keys are always present"; the three published plugins read the request by key and would not break on one more. Is it an additive key, or an `ABI_VERSION` bump?
- **Secrets from a plugin.** An optional `secrets` list in the reply, so a plugin can register values the way a resource does in 7.4.
- **Sensitivity in a step spec.** Plugin steps are regexes with unnamed groups, so `{name:secret}` does not apply; `StepSpec` would need an optional field naming the secret groups.
- **`media_type` in a diagnostic,** instead of the host guessing from an extension.
- **Files a plugin writes** are not filtered by the host. Is that the plugin's duty, and does it then need the secret set?

Until then plugin steps take part in everything else: lifecycle events, variable-name masking of what they write, the filter over what they return, and their failure-path diagnostics as evidence.

## 11. Invariants this touches

A reviewer should confirm each still holds, or agree to the stated change.

- **Invariant 1, matching needs no values.** Untouched: `secret` matches as `text` does, and `vars` is collected after `find`.
- **Invariant 6, one `print!` per file.** Untouched: the stream never goes through stdout.
- **Invariant 6, exit codes.** Changed on purpose: 143 and 129 appear (section 6), and exit 2 gains the lost-stream case next to the lost-report one.
- **"Step text in a report is the raw feature text."** Changed on purpose, in one case: a literal value in a `secret` parameter is written as `***` (7.2).
- **"Failure always dumps the full HTTP exchange."** Narrowed on purpose: sensitive header values, sensitive fields and registered secrets are masked in it unless `--reveal` is given.
- **Everything a command prints goes through one filter.** New: `doctor`, `resource add` and the `error:` line of `main` are masked like the failure dump (7.6).
- **Report paths are truncated before the config is read, in `run` and `doctor` alike.** Kept for the events path, by a separate open-once code path in `run` and a non-opening check in `doctor` for a path that is not a regular file.
- **`doctor` reaches every pre-run check `run` makes, and exits 0 or 1.** Kept: `privacy` parsing is shared with `run`, and the name hint is a row that cannot fail.
- **Only JSON strings cross the plugin boundary; a plugin must never print to stdout.** Untouched: no callback, no shared handle, no ABI change in packages 1–5.
- **`execute_step` stays `Send`; no async mutex across a round-trip.** The emitter is a channel sender and a few integers.
- **Dependency versions are frozen.** No new crate is needed. `tokio` already has `sync`; the environment variables are read with `std::env::var_os`, as `BDDKIT_CONFIG` is, so `clap` needs no `env` feature.
- **The host's resource field tables are hand-written.** `API_FIELDS` gains `sensitive_headers` with the struct field.

## 12. Alternatives considered

- **Sending events over the network from the core** (WebSocket or HTTP to a hub). If the hub is down or slow the choices are to stall tests, drop events or buffer on disk — and the last is a file again. It also puts a network client and an outbound channel into a process that holds database credentials. The file is the seam; a sender beside bddkit owns the transport.
- **Stdout, `--events -`.** Collides with the console, and a reader that died cannot resume.
- **Cucumber Messages.** A ready NDJSON protocol with ready formatters, built on pickles and cross-referenced ids. An HTTP exchange or a query fits it only as an opaque attachment, and that data is the point of this stream. A converter can be written from the stream later.
- **OpenTelemetry in the core.** Attractive — steps as spans, the service's own trace joined through a propagated header — and too heavy as a dependency of a test runner. A converter outside the core can do it from the stream.
- **Per-file buffering, like the console.** Atomic, but a file's events would arrive only when the file ends.
- **Masking in the hub.** Too late: the secret has already left the runner.
- **A switch in the config to disable masking.** Gets committed.

## 13. Details to confirm

Every **Detail** above, in one place. None was discussed; each is the spec author's choice.

1. Environment variable names `BDDKIT_EVENTS` and `BDDKIT_ARTIFACTS_DIR`; read by `run` only. (5.1, 8)
2. `doctor` does not open an events path that is not a regular file. (5.1)
3. A macro body step reports the definition's line plus `index`. (5.3)
4. A second signal exits with the code of the first. (6.2)
5. The built-in declarations that become `secret`, the SRP values registered regardless of name, and `secret` being quoted like `text` under #79. (7.2)
6. The default `sensitive_headers`: `authorization`, `cookie`, `set-cookie`. (7.4)
7. `privacy` validation: word shape, empty list refused. (7.7)
8. The host's stderr debug output is masked too. (7.8)
9. Longest-first replacement; the mask is `***`. (7.8)
10. `--reveal` announces itself on stderr. (7.9)
11. The `doctor` hint reads slots and is an `ok` row. (7.10)
12. One allocator for host and plugins; the workspace stays in the temporary directory; `run_started.artifacts_dir`; `media_type` from the extension. (8)
13. The flag name `--evidence`, and that it requires `--events`. (9.1)
14. Sensitive fields: names split on `_`, `-` and case boundaries; the default `fields: [password, secret, token]`; the three places the API module looks; numbers masked in place; `json` diagnostics covered; an empty `fields` allowed. (7.5, 7.7)
15. The config walk: field words on keys, credentials inside URLs; `--reveal` on `doctor` too. (7.6)

## 14. Claims to verify

Each is a fact this specification depends on. If one is wrong, the section named after it needs rework.

1. Reports are written after `run_all` from collected results; `report::prepare` creates, truncates and closes each path before `config::load`, and `doctor` calls the same function. → `main::run`, `ReportArgs`, `report::prepare`, `doctor.rs` `reports` stage. (5.1)
2. `run` prints `run <id>` after validation and after `Db::connect`. → `main::run`. (5.4)
3. `run_file` records `Skipped` for every top-level step after a failure and never runs them. → `runner::run_file`. (5.3, 5.4)
4. Every row of an outline has the same `line`. → `feature::expand_outlines`. (5.3)
5. A macro body step is built with an empty `keyword` and the caller's `line`; `MacroStep` has no line; `MacroDef` has `source` and `line`. → `runner::execute_step`, `macros.rs`. (5.3)
6. An included scenario runs the included file's Background first, and its steps carry their own lines. → `runner::run_include`. (5.3)
7. Macro and include nesting share one depth counter capped at 16. → `runner::execute_step`, `run_include`. (5.3)
8. A file is one task and its scenarios run in order, so events of one file are in causal order. → `runner::run_all`, `run_file`. (5.2)
9. `main` exits through `std::process::exit`. → `main.rs`. (5.5)
10. `tokio` is locked at 1.53 with `sync`, and an unbounded `mpsc` sender can be used from sync code. → `Cargo.toml`, `Cargo.lock`. (5.5)
11. SIGINT and SIGTERM both exit 130 today, SIGHUP is not handled, a second signal exits at once. → `main::wait_for_interrupt`, `handle_interrupt`. (6)
12. On an interrupt the process can exit while a file is still running. → `main::handle_interrupt`, invariant 6. (6.3)
13. `README.md` and `CLAUDE.md` state the code 130; one test pins the SIGINT path. → `README.md` "Interrupting a run", `CLAUDE.md` invariant 6, `tests/plugin.rs`. (6.2)
14. `expression::TYPES` is the only table of parameter types and one parser serves builtins and macro templates. → `steps/expression.rs`. (7.2)
15. The Hawk key, the AES key and the SRP password are plain `{name}` parameters today. → `BUILTIN_STEPS` in `steps/mod.rs`. (7.2)
16. Every variable write goes through `VarStack::set` or `set_global`. → grep `vars.set` across `src/`. (7.3)
17. Prefixed names are `<prefix>_<field>` in SRP, AES and `I include … with prefix`. → `steps/srp.rs`, `steps/vars.rs`, `runner::run_include`. (7.3)
18. A variable name matches `[^\W\d]\w*`. → `vars::PLACEHOLDER`. (7.3)
19. `Config` keeps unknown top-level keys in `extra` and checks only `default_*` ones, so a misspelt `privacy` key would be ignored silently. → `config.rs`, `check_group_defaults`. (7.7)
20. `ApiConfig` has `base_url`, `timeout_secs`, `default_headers`, `options`, and `API_FIELDS` mirrors it by hand. → `config.rs`. (7.4)
21. The failure dump prints every request header in full and both bodies cut to 600 characters, and nothing in `src/` masks anything. → `impl Display for Exchange` in `http.rs`; the absence was checked by grep only. (4, 7)
22. The failure text is built inside `run_file`, where the file's `World` is still alive. → `runner::run_file`. (7.8)
23. The host's debug output is `eprintln!` at a handful of sites. → `db/ops.rs`, `steps/debug.rs`, `runner.rs`. (7.8)
24. `doctor` has the statuses `ok`, `failed`, `skipped` and exits 0 or 1. → `doctor.rs`. (7.10)
25. The artifact allocator lives on `Plugins` and does not exist when no plugin is loaded; the workspace is created by the host and never deleted. → `plugin/mod.rs`, `main::load_plugins`, `world.rs`. (8)
26. Outside tests `HttpState::last` is assigned only in `send` and `replay_last`, and a polling replay of an HTTP assertion goes through `replay_last`. → `http.rs`, `steps/assert.rs::replay_response`. (9.3)
27. The host holds a response body as a `String`. → `http::Exchange`. (9.3)
28. On `passed` the runner uses only `vars` from a plugin reply. → `runner::execute_step`, plugin arm. (9.3)
29. `bddkit-exec` inlines text up to 16 KiB and writes more to a file. → `bddkit-exec/src/steps.rs`, `INLINE_LIMIT`. (9.3)
30. `bddkit-browser`, `bddkit-exec` and `bddkit-s3` read the dispatch request by key and tolerate an unknown one. → each plugin's `steps.rs`. **Checked by reading the parsers, not by a test.** (10)
31. `interpolate` returns only the resulting string. → `vars.rs`. (9.2)
32. Nothing removes an artifact or workspace directory: outside tests the only directory removals are in `plugin/manage.rs`, on plugin library directories. **Checked by grep only.** (8)
33. A request keeps its query and its form as name–value pairs and its body as a string, and the exchange is stored only after `execute` returns. → `http::RequestRecipe`, `HttpState::execute`, `send`, `replay_last`. (7.5)
34. A plugin diagnostic's `content` is appended to the failure text as it came. → `DispatchResult::render_failure` in `plugin/abi.rs`. (7.5)
35. `${…}` references are expanded on the config's text before it is deserialised. → `config::expand_env`. (7.6)
36. `resource add` prints the block it would have written when it writes nothing. → `resource.rs`. (7.6)
37. `main` prints `error: {error:#}` for a failed command, and `doctor` prints a plugin's `probe_config` refusal. → `main.rs`, `doctor.rs`. (7.6)
38. Whether a YAML parse error can quote a value from the file. **Not checked.** (7.6)
