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
  Given I have "accounts" with "email: user-<<unique()>>@example.com, balance: 100"
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

**Test data shouldn't collide, and should be removable afterwards.** Every `<<unique()>>` value in a run shares one prefix drawn once per process, so uniqueness is guaranteed rather than probable — and `<<run_id>>` writes that prefix down, so cleanup afterwards is one step: `I delete "companies" where "slug~: u<<run_id>>%"`. `<<unique()>>` is a bare token; `unique(email)`, `unique(slug)` and `unique(url)` are aliases of it and give no address, slug or URL shape, so build the shape around it, as in `user-<<unique()>>@example.com`.

**A failure should explain itself.** Every failed step prints the full last
exchange — method, URL, headers, bodies, status — with no debug flag and no
re-run. JSON mismatches point at the path that differs.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/bddkit/bddkit/main/install.sh | sh
```

```powershell
irm https://raw.githubusercontent.com/bddkit/bddkit/main/install.ps1 | iex
```

Installs a prebuilt binary from the [latest release](https://github.com/bddkit/bddkit/releases/latest) — Linux (x86_64/aarch64), macOS (x86_64/aarch64) or Windows (x86_64) — into `~/.local/bin` (`%LOCALAPPDATA%\bddkit\bin` on Windows). `install.sh` also works from Git Bash, MSYS2 and Cygwin: it hands off to `install.ps1`. No prebuilt binary for your platform: `cargo install --git https://github.com/bddkit/bddkit`.

### Upgrading

`bddkit upgrade` replaces the binary with a newer release and, when the new release speaks a different plugin ABI, moves every plugin bddkit installed to a release built for it — all together, or nothing changes. It only replaces a binary `install.sh` or `install.ps1` put there (they record it in `install.json` in the user data directory); a `cargo install` build, a package or a container image is refused with the right alternative.

```sh
bddkit upgrade --check          # the plan: what would change, changing nothing
bddkit upgrade                  # to the latest release (asks first; --yes where there is no terminal)
bddkit upgrade --version 0.4.1  # a given release, older ones included
bddkit upgrade --rollback       # the previous binary and lock files, back
```

The new release is downloaded, checksummed and asked for its version and plugin ABI before anything is replaced, and each new plugin version is installed beside the old one and loaded by the NEW binary. A plugin with no release for the new ABI stops the upgrade; `--force` goes ahead and leaves it as it is. Hand-written lock entries and everything in a project layer (`.bddkit/` in your repository) are listed, never changed. Nothing is deleted, so `--rollback` can restore it all; if the new binary will not start, the previous one is kept as `previous/bddkit` in the user data directory and `previous/bddkit upgrade --rollback` does the same. Rollback puts the lock files back exactly as they were before the upgrade, so a `plugin install` or `update` made since is undone too.

Old plugin versions stay on disk after an upgrade so that a rollback has something to go back to, and `plugin remove` and `plugin update` leave a version that another lock file or a backup still points into. `bddkit plugin prune` removes the ones no lock file points into any more, the `.before-upgrade` backups a rollback would restore included, so the versions of your latest upgrade stay until the next one (`--dry-run` lists them and their size, `--yes` skips the question and is required without a terminal); it looks only at the shared, user and `--bddkit-dir` data directories, never at a project layer, and nothing runs it for you.

Nothing checks for a newer release on its own: `run` and `doctor` open no socket for it. In CI pin the version instead of upgrading: `curl -fsSL https://raw.githubusercontent.com/bddkit/bddkit/main/install.sh | sh -s -- --version 0.4.1`, or `BDDKIT_VERSION=0.4.1`.

## Quick start

### Your own suite: `bddkit init`

```bash
bddkit init
```

`init` asks a few questions, in this order: the base URL of the API under test (an empty answer means none), a database (none, PostgreSQL, MySQL or MariaDB, with a DSN — prefer an environment reference such as `${BDDKIT_DB_DSN}` over a literal password), which plugins of the official index to install (nothing is ticked: arrows move, Space ticks, Enter accepts), whether they are for you on this machine or vendored into `.bddkit/` for everyone with the repository, whether scenarios of different files may run at the same time (`concurrency` 8, which is safe only when every scenario brings its own data, see `<<unique()>>`; otherwise 1), and last whether to allow anonymous usage counts. Every list is built without a preselected answer, so nothing is chosen that you did not choose; Ctrl-C at any question, and Esc at a list, leaves without writing anything (exit `130`); a text question has no Esc.

Then it writes `bddkit.yaml` (a comment above each key), `features/example.feature` (one scenario that passes without a network, with a commented-out request to your API that the report shows as a disabled step), the missing lines of `.gitignore` (`.bddkit/*.local.yaml` and `.env.local`, only inside a git work tree), installs the plugins one at a time, prints for each plugin group that needs a resource the `bddkit resource add …` command that declares it, and ends with the `doctor` report. A plugin that fails to install does not stop the others or undo the config, and neither does an environment reference such as `${BDDKIT_DB_DSN}` that is not exported yet: the closing `doctor` report fails on it (exit `1`) until it is. `init` never overwrites a config: when `bddkit.yaml` (or `bddkit.yml`, or the file `--config` names) exists it refuses with exit `2` and points at `bddkit resource add` and `bddkit plugin install`, which extend a suite.

`--layer` cannot be combined with `--bddkit-dir` / `$BDDKIT_DIR`, which replaces every layer with one directory. Without a terminal (stdin or stderr is not one, or a CI system's variable is set, see [Telemetry](#telemetry)) nothing is asked and every answer is a flag, the same flags that skip a question in an interactive run: `--api-url <url>` or `--no-api`, `--db none|postgres|mysql|mariadb` with `--dsn <dsn>`, `--plugin <name>` (repeatable) with `--layer user|project`, `--parallel true|false` and `--telemetry true|false`. Without flags that means no API, no database, no plugins and `--parallel true`; without `--telemetry` no `telemetry` key is written, so the decision stays with each person who runs the suite. `--dsn` without `--db`, `--db postgres|mysql|mariadb` without `--dsn` and `--layer` without `--plugin` are refused with exit `2` rather than ignored. Exit codes: `0` everything written and `doctor` clean, `1` written but a plugin failed to install or `doctor` found something, `2` refused or an answer missing.

The `telemetry` key `init` writes is the suite's decision about anonymous usage counts, see [Telemetry](#telemetry).

### The shipped example suite

```bash
docker compose up -d smocker   # local mock API the example talks to
cargo build --release
./target/release/bddkit run --config examples/api.yaml
```

```console
run m4k2p9x7q3b1
  ✓ examples/features/methods.feature — scenarios: 8
  ✓ examples/features/json_matchers.feature — scenarios: 4
  ✓ examples/features/variables.feature — scenarios: 6
  ✓ examples/features/variable_assertions.feature — scenarios: 3
  ✓ examples/features/macros.feature — scenarios: 8
  ✓ examples/features/content_types.feature — scenarios: 6
  ✓ examples/features/eventual.feature — scenarios: 1

run m4k2p9x7q3b1
files: 7, scenarios: 36, failed: 0
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

`bddkit run` flags: `[--config <path>]` (default: `$BDDKIT_CONFIG`, else `./bddkit.yaml` or `./bddkit.yml` in the working directory), positional paths to override the config's, `--tag` (repeatable), `--env` to pick a `.env.<name>` layer, `--fail-fast`, `--junit <file>` and `--cucumber-json <file>` for machine-readable reports, `--artifacts-dir <dir>` (also `$BDDKIT_ARTIFACTS_DIR`, the flag wins) to root the evidence files plugins write — one `<dir>/<six-digit counter>/` per plugin step, `<temp dir>/bddkit-<run id>` when unset, never cleaned up by bddkit, so give each run its own directory when you archive (a second run into the same one starts counting at `000000` again and overwrites); the per-file workspace stays under the temporary directory either way — `--events <path>` (or `$BDDKIT_EVENTS`) for a live stream of the run, and `--evidence[=<sources>]` to put what the steps did into it (see "Following a run live" below). Exit codes: `0` passed, `1` a scenario failed, `2` the run never started, including when no config is given and none of the above is found, and `128` plus the signal number when the run was interrupted — `130` for SIGINT/Ctrl-C, `143` for SIGTERM, `129` for SIGHUP (see "Interrupting a run" below).

### Interrupting a run

A Ctrl-C, the polite "please stop" signal a CI job's cancellation sends, or a closed terminal (SIGINT/SIGTERM/SIGHUP on Linux and macOS, Ctrl-C/Ctrl-Break on Windows) makes `bddkit run` stop starting new work the same way `--fail-fast` does — no new feature file, no new scenario — then asks every loaded plugin to release what it holds (stopping any process it started, gracefully first, then by force) exactly as a normal run does when the pool drains. `bddkit` then exits with the shell's convention of 128 plus the signal number — `130` for SIGINT (and for Ctrl-C/Ctrl-Break on Windows), `143` for SIGTERM, `129` for SIGHUP — so a script can tell an interruption apart from a test failure (`1`) or a setup error (`2`), and a cancelled CI job (`143`) apart from an operator's Ctrl-C (`130`). A second signal means "stop now": `bddkit` exits immediately without waiting for cleanup to finish, still with the code of the first signal. A CI system typically gives only a few seconds between its polite stop signal and a hard kill (`SIGKILL`), which no program can intercept — keep plugin shutdown grace periods within that window, or raise the CI's own cancellation timeout.

### Reports for CI

`--junit reports/junit.xml` writes JUnit XML — one `<testsuite>` per feature file, one `<testcase>` per scenario, the steps with their status and timing in `<system-out>`, which is what Jenkins, GitLab and GitHub test summaries read. `--cucumber-json reports/cucumber.json` writes Cucumber JSON in the shape cucumber-html-reporter and Allure accept: features with `elements`, elements with `steps`, each step with `keyword`, `name`, `line` and a `result` of `status`, `duration` (nanoseconds) and `error_message`. Features and scenarios also carry `description` (`""` when there is none) and `tags` as `{"name": "@smoke", "line": 3}`, a scenario's being the ones `--tag` sees (for an outline row that includes the tags of its `Examples` block, so `--tag` can pick one block's rows; `@serial` and `@priority` on an `Examples:` block do not schedule anything, they are read from the feature, a rule or a scenario only); a feature's `line` is its `Feature:` line. Both flags are optional and independent, and both are properties of the run, not of the suite — the config never learns them. Console output is unchanged.

Step text is the raw feature text, never interpolated, so two runs' reports differ only in results and timings. A failure carries the same text the console prints — the step error plus the HTTP exchange — and the XML stays well-formed whatever that text contains (a `]]>`, a control character, the NUL bytes of the `<<null>>` sentinel become U+FFFD). Scenarios a `--tag` filtered out and files never started under `--fail-fast` are absent, not skipped; the totals count what ran. A macro call is one step. A file that panicked appears as its one synthetic failed scenario.

**Every report path is created — truncated — before the config is read**, its directory made if missing; a path that cannot be prepared is exit 2 before any request. A run that dies on its config therefore leaves an empty file, which a parser rejects loudly, rather than yesterday's green one. A report that cannot be written after the run is also exit 2 — the one exception to "2 = the run never started", because a report silently lost behind a green code is the worse outcome. `bddkit doctor` accepts the same flags and makes the same check (a `reports` row per path); without them it touches nothing.

### Why a scenario failed

A failed scenario carries a class next to its text, so "a test caught a bug" and "staging was down" can be told apart by a CI rule, a report filter or a count. The class is that of the failure that ended the scenario; a macro or an include passes its inner step's class up unchanged.

| Class | When |
|---|---|
| `assertion` | a check did not hold, including after polling ran out |
| `transport` | connection refused, DNS, TLS, an HTTP timeout |
| `db_connection` | a database connection was lost or could not be acquired mid-run |
| `db_statement` | the database refused a statement: a constraint, a missing column or table, a bad literal |
| `input` | the scenario itself is wrong: an undefined variable, a value that is not of its parameter's type, a malformed step argument, an include that cannot be resolved |
| `plugin` | a plugin answered `fatal`, or did not answer |
| `reset` | a plugin's per-scenario reset failed |
| `panic` | the file panicked: a bug in bddkit |
| `unclassified` | a host fault no class fits: the system clock or entropy could not be read |

The console summary splits the failures when there are any: `files: 12, scenarios: 310, failed: 5 (4 assertion, 1 transport)`. The event stream carries `failure_kind` on `scenario_finished` and `kind` on a failed `step_finished`; the JUnit `<failure>` has it as `type`; the HTML report shows a badge on each failed scenario and a filter by class; the Markdown report puts it on the `failure` marker as `kind`. Cucumber JSON has no field for it. The exit code is `1` for any failed scenario, whatever its class. A polling assertion that never held is `assertion`, and one that kept meeting an unreachable service is `transport`. A database that cannot be reached when the run starts is not a scenario failure at all: the run exits `2` before the first request.

### Following a run live

`--junit` and `--cucumber-json` describe a run after it is over. `bddkit run --events <path>` writes one JSON object per line while it happens, for whatever wants to follow a run — a local UI, a log tail, a program that ships the run somewhere. The path is a regular file, a FIFO, or anything else that opens for writing by name (`/dev/fd/N` from a process substitution); `-` is not special and stdout is never a target, since it carries the console output. `$BDDKIT_EVENTS` supplies the path when the flag is absent (an empty value counts as unset); the flag wins. Console output, both reports and the exit code are unchanged.

Every line has `type`, `seq` (from 0, strictly increasing — the order of the stream) and `t` (milliseconds since the run started); a consumer must ignore a `type` or a field it does not know. The first line is `run_started` (with `schema: 1` and `artifacts_dir`, the absolute artifact root), the last is `run_finished` (with `exit`, the code the process returns, and the totals). Between them, per file: `file_started`, `scenario_started`, `step_started`, `step_finished` or `step_skipped`, `scenario_finished`, `file_finished`. Lines of different files interleave; the lines of one file are in causal order. An outline row has no line of its own, so `scenario_started` carries `example`, its 0-based ordinal across the `Examples` tables. Every field is listed in `docs/events-design.md`, sections 5.2 to 5.4.

A step inside a macro or an included scenario is an event of its own, with `parent` (the `step` number of its caller) and `source` (the file its text is written in); an included step has its own `line`, a macro body step has the definition's line and its `index` in the body. The reports still show a macro call or an include as one step. Only top-level steps are ever `step_skipped`: when a nested step fails the rest of its body does not run and is not reported. A step's `warnings` include those of the steps nested in it, so do not add them up across levels.

**The stream is in one of three states.** *Empty*: the run was refused (exit 2) and the reason is on stderr — the path is opened, and truncated, before the config is read, but nothing is written until the run has started; a process killed before that point leaves the same empty file. *Terminated*: the last line is `run_finished`; a `signal` field on it (`SIGINT`, `SIGTERM`, `SIGHUP`, `CTRL_C`, `CTRL_BREAK`) says the run was interrupted rather than finished, its totals are those of the files that had finished by then, and scenarios and steps still in flight stay open — there is no `scenario_finished` for them. *Cut short*: lines but no `run_finished` — the process was killed, or exited on a second signal. Nothing is ever written after `run_finished`, even by files still running when an interrupt arrives.

The path is opened once and held, never created and closed again, so a FIFO's reader does not see an end-of-file before the first event; opening a FIFO blocks until a reader has it open. The open truncates: a `/dev/fd/N` that points at an existing regular file empties it. A write that fails mid-run (a full disk, a reader that went away) does not stop the run: the rest of the stream is dropped, the error is printed after the run, and the exit code is `2` — the same exception that covers a report that cannot be written. A reader that holds the FIFO open and stops reading stalls the end of the run: the process waits for the last lines to be taken, a first signal only prints that it is still writing, and a second one stops it, with the first one's exit code and the stream cut short. `bddkit doctor --events <path>` reports a `reports` row like the other paths, except that a path which already exists and is neither a regular file nor a directory is reported `ok` without being opened. The stream carries the same failure text the console and the JUnit report do; without `--evidence` it holds no evidence (resolved values, exchanges, statements).

**Evidence.** `bddkit run --events <path> --evidence` adds what the steps did, also for the steps that passed (the console still shows it only for a failure). `--evidence` without an events path, from the flag or `$BDDKIT_EVENTS`, is a refused run (exit `2`), and `run_started.evidence` records it. It is off by default because evidence puts the bodies and query parameters of passing steps on disk, and masking cannot recognise what nobody marked; the rules of the Privacy section apply to it unchanged. It adds: `vars` on `step_started` — the `<<…>>` slots the step resolved, as `[expression, value]` pairs in order of appearance (captures, then the docstring, then table cells row by row), with the step's `text` still the raw feature text; a value is `***` in a `secret` parameter or under a sensitive variable name, and `<<null>>` is the JSON `null`; the slots of an `I include … with:` table are not listed. An `evidence` event for each HTTP exchange (an explicit send and every polling replay), each SQL statement and each diagnostic a plugin's reply carries, whatever its status: `file`, `scenario`, `step` (the innermost step in flight, so a step inside a macro owns its own), `group`, `resource`, `kind`, an optional `title`, and either `content` or a file reference (`path`, `size`, `media_type`). For `http` the `content` is an object (`method`, `url`, `status`, `duration_us`, and `request` and `response` each with `headers` as `[name, value]` pairs and `body`); for `sql` it is `sql`, `binds` and `duration_us`. Text of up to 16 KiB stays in the event; longer text is written under the artifact root, after the privacy filter, and referenced — per body inside an `http` object — with a `path` relative to the root, and the event is written only after the file is complete. A file a plugin names outside the root is copied in first; a file a plugin names is referenced as it is and never filtered. And an `attempt` event (`n` from 1, `message`) for every `not_yet` of an eventual assertion, which quotes the values that did not match.

**What evidence records.** `--evidence=<sources>` takes a comma-separated list of sources after an `=` — so a bare `--evidence` followed by a feature path still means the flag and the path — and they combine: `marked` records the steps a scenario's author marked, whether the scenario passed or failed; `failures` records every step of a scenario that failed and nothing of one that passed; `all` records every step, and is what a bare `--evidence` means. So `--evidence=marked,failures` keeps the marked steps of every scenario and everything of the failed ones. `$BDDKIT_EVIDENCE` gives the same list when the flag is absent (an empty value is unset), and the config key `evidence: [marked, failures]` is the suite's default, used only when the run has a stream — a local run without `--events` ignores it, while the flag or the variable without a stream is refused (exit `2`). An unknown source is refused in all three forms (exit `2`), naming the valid ones. The `evidence` row of `bddkit doctor` shows the sources the variable or the config gives (with `doctor --events` or `$BDDKIT_EVENTS` standing for the run's stream) and fails where `run` would refuse: a malformed `$BDDKIT_EVIDENCE`, one without a stream, or a malformed `$BDDKIT_EVIDENCE_BUFFER` while a source is active. `run_started` keeps `evidence` (true when any source is active) and adds `evidence_sources`, and every `scenario_finished` carries `evidence`: `full`, `partial` or `none` of its steps recorded — which is how a report tells a step with no exchange from a step whose exchange was not recorded.

A scenario marks steps in two ways. `I am collecting evidence` … `I am not collecting evidence` marks the steps between the two, at any depth: a macro or an included scenario called from a marked step is marked too, and written inside a macro body or an included scenario the steps switch the mark for the rest of the calling scenario, as the debug-mode steps do. The mark ends with the scenario whatever happened. An `@evidence` tag marks every step of a scenario, its Backgrounds included, and on a rule or a feature every scenario under it; a step cannot switch a tag's mark off. A mark never records anything by itself — the person running bddkit opts in to `marked` — and the steps change nothing else about the scenario.

Under `failures`, a step that is not recorded live is deferred: its events are held until the scenario ends, written just before `scenario_finished` if it failed (each with `"deferred": true` and the `step` that produced it, in the order they happened) and dropped if it passed. A deferred step's resolved values come as a `vars` event of their own (`file`, `scenario`, `step`, `vars`, `deferred`), since `step_started` was written before anyone knew the outcome. Text over 16 KiB of a deferred step stays in memory and reaches the artifact root only if the scenario fails, so a passing scenario leaves nothing of it on disk — up to 64 MiB of such text per file in flight (`$BDDKIT_EVIDENCE_BUFFER`, in bytes, changes it; read only when a source is active); past that, long text is written at once, as under `all`, and stays on disk when the scenario passes, since bddkit deletes nothing. The cap bounds long text only: the held events themselves (each at most 16 KiB of text) are not counted, so a scenario with thousands of polling replays can hold more than the cap in memory. A held event's `t` is the time it was written, at the end of the scenario; its timing is in `duration_us`. A plugin is told evidence is wanted (the dispatch's `evidence` key) whenever a step's evidence might be kept, live or deferred, and a file it writes for a scenario that then passes stays on disk unreferenced — as does the copy the host makes at once of a diagnostic file named outside the artifact root. Every value is masked before it is held, and again with the secrets known when it is written, and an interrupted run writes nothing of the scenarios still running: they were stopped, not failed.

### Reading a run: `bddkit report`

`bddkit report run.ndjson` turns a stream written by `run --events` into `run.html` beside it: one self-contained file that opens from disk and shows every file and scenario with its description, tags and comments, the steps as written with status, duration, warnings and nested steps, sections as headings, disabled steps struck through, and the failure text of what failed. With `--evidence` in the run it also shows, under the step that owns them, each HTTP exchange (JSON pretty-printed, with a toggle back to the raw text), each SQL statement with its binds, screenshots and the polling attempts. Failed scenarios start open, passed ones closed; the filters (status, text, tag, failure class) and the open scenario live in the URL hash, so a link can point at one scenario. Banners say when masking was off, when the run collected no evidence or recorded it only from some sources (naming them and the flag that records every step, and a scenario not wholly recorded carries a `partial` or `none` badge), when the stream is incomplete (a killed run still renders) and when the artifacts are under the temporary directory. The page carries a Content-Security-Policy that allows no network access, and the same stream always gives the same bytes.

`-o <file>` names the output and can be repeated; the format follows the extension, `.html` or `.htm` for HTML and `.md` or `.markdown` for Markdown (see below), so `-o report.html -o report.md` writes both from one read of the stream. Nothing is written unless every output is. Artifacts are linked by a path relative to the report's directory, unless `--embed-artifacts` is given (below): they are read from `run_started.artifacts_dir`, or from `--artifacts <dir>` when they have moved, so run with `--artifacts-dir` next to where the report will be kept and archive the two together. Exit `0` is every output written, `2` is nothing written (not a stream, unreadable input, unwritable output, an unknown extension).

`--embed-artifacts` carries the files the report links to — screenshots, plugin diagnostics written to a file, HTTP bodies spilled to a file — inside the page, so the one file can be mailed or opened from a build's artifact list without the directory beside it; a report that will be mailed should be rendered with it. `--embed-limit <size>` caps the on-disk bytes taken (default `20MB`; a whole number with `B`, `KB`, `MB` or `GB`, decimal, case-insensitive, bare meaning bytes): the smallest files are taken first and the first one that does not fit ends the selection, so `0` embeds nothing. The page grows by about a third on top, because it is base64, and the summary line printed per output (`report.html: 12.4 MB, 310 scenarios, 84 files embedded (lossless, 31.0 MB → 22.6 MB), 3 left as links`) states its real size. `--embed-images` picks what happens to an 8-bit PNG on its way in, and never to the file on disk: `lossless` (the default) stores it as lossless WebP, exact pixels, and `lossy` as lossy WebP at `--embed-quality <1-100>` (default 80, only with `lossy`); in both the copy is whichever of the re-encoding and the original is smaller, and a 16-bit, animated or undecodable PNG, a JPEG and every other file go in as they are. `original` embeds every file untouched. A file the page cannot carry stays a link and the page says why next to it: over the limit, not found (say, the temporary directory was cleaned), or an unsafe path — only relative paths made of plain names are ever read, so a crafted stream cannot pull a file from outside the artifacts directory into a page that is mailed. None of those is an error and the exit code stays `0`. The three options without `--embed-artifacts`, and `--embed-quality` without `--embed-images lossy`, are refused with exit `2` and nothing written. On a terminal (and with `CI` unset or empty) one line on stderr shows the stages, reading the stream and embedding artifacts; anywhere else nothing is drawn. An interrupt (SIGINT, SIGTERM, SIGHUP) removes the temporary files and exits with `128` plus the signal number; an output already renamed stays.

**A report for agents.** `bddkit run --events run.ndjson [--evidence]`, then `bddkit report run.ndjson -o report.md [--detail failed|all]`, writes Markdown for a coding agent or a CI bot that reads files through `grep`, `sed` and `awk` and pays for every line it loads. The first lines say how to read the file; every part (the index, each file, each scenario, each failure, each piece of evidence) sits between two marker lines that a plain `grep '^<!-- bddkit:'` finds and nothing else produces; the index gives each scenario's exact line range in that file, so `sed -n '140,212p'` cuts one out. By default only failed scenarios are in the file, in full, and the rest are counted; `--detail all` puts every scenario in. `--detail` without a Markdown output, and `--embed-artifacts` without an HTML one, are refused with exit `2`. Artifacts are linked, never embedded. The file is as masked as the stream it comes from, says so in its front matter when masking was off, and the same stream gives the same bytes. The format, for anyone writing a tool over it, is in [docs/report-markdown.md](docs/report-markdown.md).

In CI the report is wanted exactly when `run` exited 1, so it has to run after a failure. GitLab: put `bddkit report run.ndjson` in `after_script`, or in a job with `when: always` reading the stream as an artifact. GitHub Actions: a step with `if: always()`. A plain shell script: `bddkit run --events run.ndjson || status=$?; bddkit report run.ndjson; exit ${status:-0}`.

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

### Uploading a file

`I attach the file "<path>" to the request as "<name>"` adds a file part to the request being built; with the form-parameters step the request goes out as `multipart/form-data`, the parameters as its text parts. With no attachment, form parameters stay URL-encoded.

```gherkin
Given the request form parameters are:
  | name     | value                                   |
  | metadata | {"idDocType":"ID_CARD","country":"DEU"} |
And I attach the file "fixtures/id_front.png" to the request as "content"
When I request "/applicants/<<applicantId>>/documents" using HTTP POST
```

The path is relative to the feature file the step is written in — the rule `I include` uses — and `<<variables>>` work in the path and the part name. Attach more than once to send several files, under one part name or several. The part's content type comes from the file extension, `application/octet-stream` when unknown. The multipart `Content-Type` replaces any set by `default_headers` or a header step. A literal path that does not exist is reported by `bddkit doctor` and stops `bddkit run` before the first request (exit 2); a path holding a `<<variable>>` fails at the step. The file is read when the step runs, so a retried assertion replays the same content, and a failure dump lists each part's name, file name, type and size, never its bytes. Hawk signing of a multipart request is refused. The whole file is held in memory.

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

### Seeing values while debugging

The `debug` group prints to stderr and changes nothing in the scenario:

| Step | Prints |
|---|---|
| `Show "<name>" variable` | one variable, failing if it is not set |
| `Show all variables` | every variable in scope |
| `Print table:` (data table) | the table as written, `<<variables>>` filled in every cell, the header row included, each column as wide as its longest value |
| `Print response headers` / `Print response body` | the last response's headers, or its body highlighted |
| `Print response body as "<path>"` | one JSON path, XPath or CSS selector selection of the last response |
| `I am in debug mode` / `I am not in debug mode` | turns on/off the generated SQL with its binds and timing, and plugin exchanges |
| `I am collecting evidence` / `I am not collecting evidence` | prints nothing: marks the steps between the two for `--evidence=marked` (see "What evidence records") |

```gherkin
And Print table:
  | order       | status          | total          |
  | <<orderId>> | <<orderStatus>> | <<orderTotal>> |
```

```
| order      | status | total  |
| ord-7f3a91 | paid   | 149.90 |
```

Under `concurrency` greater than 1 this output is not part of a file's atomic report, so two files' lines can interleave; run with `concurrency: 1` while debugging.

### Describing a scenario

Write what a scenario is for as its description, the free text between the `Scenario:` line and its first step. The same goes for `Feature:`, `Background:`, `Rule:` and `Examples:`. bddkit keeps it as written, with its paragraphs and relative indentation, and puts it in the event stream and the Cucumber JSON report (`#` comment lines are left out). A description line cannot start with a step keyword (`Given`, `When`, `Then`, `And`, `But`, `*`) or `@`: Gherkin reads such a line as the next step or tag, so "And then the user…" becomes an unknown step and the run exits with `2`.

### Comments, disabled steps and sections

The event stream keeps the notes and structure written with `#` in a feature file:

1. A comment block is consecutive `#` lines with no blank line between them.
2. A block directly above a step, with no blank line between, is that step's comment.
3. A commented line is a disabled step when it starts with a step keyword of the file's language and its remaining text matches a loaded step, macro or plugin. A keyword alone is not enough: `# Then we wait for the queue` remains a comment if the registry has no such step.
4. A block directly above a disabled step is that disabled step's comment.
5. A block followed by a blank line starts a section. Its first non-empty line is the title and the remaining lines are its note; it groups the following items until the next section or the end of that scenario or Background.
6. A block after the last item, with nothing below it, is a closing section, shown by a report as an end note.

```gherkin
Scenario: refund a paid order
  The customer pays, then asks for the money back.

  # Setup
  # An order paid by card.

  Given I have "orders" with "status: paid"
  # The service needs a moment to settle the payment.
  When I request "/api/v1/orders/<<id>>/refund" using HTTP POST
  # Disabled until #123 is fixed: the status comes back as 202.
  # Then the response code is 200

  # Checks

  Then I should have "refunds" with "order_id: <<id>>"
```

Here Setup and Checks are sections; the note above the request belongs to that step, and the disabled response assertion has its reason attached. Disabled steps never run, interpolate runtime variables or count toward step numbering. Outline placeholders in disabled steps are replaced before matching; prose comments and section text stay as written. Closing annotations after an Examples table repeat only for rows from that table. Comments and sections repeat with their Background or outline row and stay at their own include level. Sections and disabled steps after a failure are still emitted among the skipped steps.

Each comment line loses its `#` and one following space; a lone `#` keeps a paragraph break, with empty lines trimmed from the block's edges. Hashes inside docstrings or table cells remain content. A block directly above a Feature, Scenario, Background, Rule or Examples keyword (or its tags) is that element's comment. The file's `# language:` directive is never comment text.

These annotations reach the event stream as `comment`, `step_disabled` and `section`, with the same secret masking as other text. JUnit and Cucumber JSON keep their existing output. `doctor` counts disabled steps and lists `file:line` locations for commented lines that start with a step keyword but match no loaded step, without failing the check. A plugin that is not loaded makes its commented steps ordinary comments.

### Rules

`Rule:` blocks are supported. A scenario inside a rule runs after the feature's own scenarios, in file order, and runs the feature's Background first and then the rule's own Background. It carries its own tags, its rule's and its feature's, so `--tag` sees all three (a row of a Scenario Outline also carries the tags written above its `Examples:` block), and a `@serial` or `@priority` tag on a rule schedules the whole file, as one on a scenario does. `I include` can name a scenario inside a rule, and an include with no name counts the scenarios inside rules toward its "exactly one".

### Including other feature files

Scenarios often follow the same setup steps, and duplicating them across files gets tedious. `I include "<file>"` pulls one scenario from another feature file and runs it inline, with access to the same HTTP state, database connection, and plugin instances — giving the caller a chance to reuse setup work and inspect what it left behind. Variables are isolated: the included scenario runs in a fresh `VarStack`, so the caller's globals are invisible inside it, and only variables declared via the `@exports` tag come back.

| Step | Effect |
|---|---|
| `I include "<file>"` | Runs the file's sole scenario. A Scenario Outline with no `Examples:` block at all is a plain Scenario for this purpose, run as written. A Scenario Outline with an `Examples:` table is only includable this way if that table holds exactly one data row; more than one row (or an Examples table present with zero data rows) requires a `with:` table to pick one, and is otherwise a validation error; the included scenario's HTTP/DB/plugin state flows back to the caller, but variables are isolated |
| `I include "<file>" scenario "<name>"` | Runs the scenario named exactly `<name>` from `<file>` — no match, or more than one scenario sharing that name, is a validation error |
| `I include "<file>" with prefix "<p>"` / `I include "<file>" scenario "<name>" with prefix "<p>"` | Same as above, but every exported variable comes back renamed `<p>_<name>` instead of `<name>` |

A `with:` table feeds a single row of parameters into an included Scenario Outline — the table must have exactly one data row (a header row plus one values row); anything else (zero data rows, or two or more) is a hard validation error, "with: must have exactly one data row." The caller builds the cells by interpolating `<<...>>` values **against the caller's own variables**, not the included file's scope. `with prefix "<p>"` can be combined with a `with:` table (`I include "<file>" with prefix "<p>" with:`) — the prefix comes first, since `as "<name>"` elsewhere already means "store the export in `<name>`", not "prefix it". The included scenario's `@exports(a,b,*)` tag declares which result variables come back to the caller; undeclared variables are discarded when the scenario ends. `@exports(*)` brings back all of them, `@exports(user_id,token)` brings back only those two — the comma-separated names take no space between them, since Gherkin tags cannot contain spaces. For a scenario with no `@exports` tag, nothing returns. A prefix renames every exported variable the same way, including ones matched by a `*` glob — this is what lets the same flow be included twice in one scenario (registering a buyer, then a seller) without the second call's exports overwriting the first's.

State that carries across the include boundary unchanged — no reset happens because of the include itself: the HTTP response from the last request, the current API resource and its headers, the current database connection, and plugin instances with their per-scenario state. (Separately, and unrelated to including: a plugin's instance selection resets like the API does, but only at an ordinary scenario boundary — an include does not introduce one.) Variables do not carry across — only what the `@exports` tag names comes back; all other variables are discarded.

An include can nest up to 16 levels deep, checked at validation time; a cycle (file A includes file B which includes file A) is a static error, reported before the first request, naming every file in the cycle. The path `"<file>"` is a compile-time literal — a string, never a variable — and is resolved relative to the directory of the feature file that wrote the step. A missing file, a file that isn't `.feature`, or — when `scenario "<name>"` is used — a name that matches zero or more than one scenario in the resolved file, is caught at validation time, before the first request.

`bddkit doctor` adds two hints under its `steps` row, with `file:line`, and never fails on them: an `extract … from JSON`, `cookies` or `response body` on the line right after an `I include` reads the response the helper happened to leave last, which changes the day the helper sends one more request — export the value with `@exports` instead; and an `@exports` name that no `set variable`, `extract … as`, macro or nested include in the helper writes. A helper with a plugin step gets no second hint at all, because a plugin can publish variables `doctor` cannot see.

**Known limitations, deliberately out of scope:**
- Calling every scenario in a file, or every Examples row of a Scenario Outline, in one include step — would require a new step form that has not yet been designed.

### Writing a macro

A macro file is a YAML list of `step:` templates with a `do:` body. A template is a [Cucumber Expression](https://cucumber.io/docs/cucumber/cucumber-expressions/): `(main )` is optional text, `post/article` an alternation, and `\(`, `\/`, `\{`, `\\` mean the character itself. Parameters use a bddkit extension, because Cucumber's own parameters have a type but no name: `{title}` is a named parameter that matches any text, `{n:uint}` restricts it to a type — `uint`, `int`, `float`, `word`, or `method` (an HTTP method) — and `{pw:secret}` matches what `{pw}` does and marks the value as a secret to hide (see Privacy). The body refers to a parameter as `<<title>>`.

An alternation takes the whole word between spaces, optional parts included: `(the )post/article` means `(the )post` or `article`, not an optional `the` in front of either.

A typed position also accepts a whole `<<variable>>`, and the variable's value is checked against the type when the step runs: `the response code is <<expected>>` works, and a value that is not a number fails that step. The same holds for the built-in steps' own typed positions (a status code, an array length, seconds and milliseconds, the HTTP method).

A literal `(`, `)`, `/`, `\` or `{` in a template must be escaped. Unescaped, a malformed template (an unclosed `{`, an invalid escape, a `/` next to a parameter as in `"/users/{id}"`) is refused at load with an error naming the file and line, and a well-formed one that no longer reads as intended (`(setup)`, `a/b`) stops the run before the first request with an "unknown step" for every caller.

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

evidence: [marked, failures]   # when a run has --events and no --evidence; see "What evidence records"
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

## Privacy

bddkit hides secrets in everything it prints or writes about a run: the failure text on the console, both reports (`--junit`, `--cucumber-json`), the event stream (`--events`), the debug output on stderr, `bddkit doctor` in its text and `--json` forms, the block `bddkit resource add` prints, and the `error:` line of a command that failed. There is one set of secret values and one filter that replaces every occurrence of a value with `***`. A value enters the set in five ways.

1. **The step's author marks a parameter `secret`.** `{key:secret}` in a step declaration or a macro template matches exactly what `{key}` does, and the interpolated value joins the set when the step runs. A literal value is also written `***` in the step text of the console, both reports and the stream; a value that came through a `<<slot>>` leaves the text as written. The Hawk key, the AES key and the SRP password are `secret`, and the SRP steps also register their private `<prefix>_a` and `<prefix>_sessionKey`.
2. **The scenario's author puts a privacy word in a variable's name.** A variable is sensitive when `secret` is one of the `_`-separated words of its name, at any position, in any case: `access_token_secret`, `secret_reg_verifier` and `reg_secret_verifier` are, `secretary_id` and `sessionSecret` are not. Its value joins the set whenever it is written. To mask a token an API returned, save it as `as "login_secret"`, and write `"Bearer <<login_secret>>"` rather than a literal token in the step that uses it.
3. **A resource names its own sensitive headers.** On an API resource, `authorization`, `cookie` and `set-cookie` are masked by default: the header keeps its name and shows `***`. Add `sensitive_headers: [x-api-key]` to mask more, or `sensitive_headers_only: [x-api-key]` to replace the defaults (`[]` masks no header at all). The usable part of the value — the credential after `Bearer `, each cookie value — is registered too.
4. **The system under test names a field.** A JSON key, form field or query parameter is sensitive when `password`, `secret` or `token` is one of its words, split on `_`, `-` and lower-to-upper case: `access_token`, `accessToken` and `refresh-token` are, `tokenizer` is not. The value is hidden where it stands, in the request and in the response, and the text around it is left exactly as it was. It joins the set before the failure dump of the step that received it is built, and what a request carries is registered before the request leaves. A string that is at least `min_length` characters long joins the set; a number is hidden where it stands and never joins. The scalars of an array take the sensitivity of the key above it.
5. **The configuration.** As soon as `bddkit.yaml` is parsed and its `${…}` references expanded, the value under every key with a field word in its name (`password`, `secret_access_key`, `client_secret`) and the password of every URL with credentials (`postgres://app:hunter22@db/app`) join the set, plugin groups included. A secret that arrives through `${DB_PASSWORD}` or a `.env` file is caught the same way.

```yaml
privacy:
  words: [sensitive, gdpr]     # more words that make a variable name sensitive
  fields: [otp]                # more field words for names the system under test chose
  min_length: 7                # shortest value that joins the set (default 7)
  reveal: false                # true shows every value; see below
```

A list given as `words`, `fields` or `sensitive_headers` **adds** to the default (`words: [secret]`, `fields: [password, secret, token]`); `words_only`, `fields_only` and `sensitive_headers_only` **replace** it, and an empty `fields_only: []` switches that rule off. Both forms in one place is refused. A value shorter than `min_length` is hidden where its position is known — a `secret` parameter, a sensitive header, a sensitive field — but is not added to the set, because hiding every `42` in the output would destroy it.

Every option can be set three ways, and the highest wins: **flag, then environment variable, then config key, then the default.** Lists fold the same way, lowest layer first: `*_only` discards what is below it, the other form appends to it.

| Config key | Flag | Environment variable |
|---|---|---|
| `privacy.words` / `words_only` | `--privacy-words` / `--privacy-words-only` | `BDDKIT_PRIVACY_WORDS` / `BDDKIT_PRIVACY_WORDS_ONLY` |
| `privacy.fields` / `fields_only` | `--privacy-fields` / `--privacy-fields-only` | `BDDKIT_PRIVACY_FIELDS` / `BDDKIT_PRIVACY_FIELDS_ONLY` |
| `privacy.min_length` | `--privacy-min-length` | `BDDKIT_PRIVACY_MIN_LENGTH` |
| `privacy.reveal` | `--privacy-reveal` (or `--reveal`) | `BDDKIT_PRIVACY_REVEAL` |

A list flag or variable takes comma-separated words; an empty variable is unset, so only a flag or the config can say "replace with nothing". A boolean accepts `true`, `false`, `1` and `0`; anything else is exit 2 in `run` and a failed row in `doctor`. `sensitive_headers` and `sensitive_headers_only` belong to one API resource, so they exist as config keys only.

**Seeing everything.** `--reveal`, `privacy.reveal: true` or `BDDKIT_PRIVACY_REVEAL=1` turns masking off for a run, and says so on stderr (`doctor` too). A `reveal: true` committed for one debugging session turns masking off in CI as well, so CI should set `BDDKIT_PRIVACY_REVEAL=false`: the environment beats the config. `run_started.privacy` in the event stream records what was in force (`masking`, `words`, `fields`, `min_length`), so a consumer can refuse or flag a run that arrived unmasked, and the `privacy` row of `bddkit doctor` states whether masking is on and which layer decided. The same row lists the `<<slot>>` names in feature files and macros that have `password`, `token` or `key` as a word and no privacy word — a hint that never fails the report and never changes what is masked.

What masking does not do:

- It works forwards. A value is hidden from the moment it enters the set, so a secret in a body that is not JSON, or under a name no field word matches, stays visible until a later step saves it into a sensitive variable.
- A field word can be too eager. With `token` in the list, `next_page_token` is hidden wherever it appears afterwards; replace the list with `fields_only` when that hurts.
- Database steps are not covered by `fields`: a column called `password` in `I have … with "password: …"` is not recognised. Pass the value through a sensitive variable.
- A derived value — a hash, a signature, a ciphertext of a secret — is a different string and is not recognised.
- Marking is by name. `_secert` hides nothing and only the `doctor` hint notices. A family is hidden whole: `as "secret_reg"` hides every SRP variable of that login, the public ones included.
- Some places hide a short value only by value, so only when it is long enough: a multipart text part, the message of an assertion that quotes a body, and `Print response body as "<path>"`.
- A literal in a step's own text reaches the event stream unmasked unless its parameter is `secret`, because `step_started` is emitted before the request that would register it.
- A form value containing `&` or `=` is hidden by position only up to that character, and a DSN password holding a raw `/`, `#` or `?` is mis-split by URL parsing (percent-encode it); in both cases the value is still caught wherever it appears whole, if it is long enough.
- Images are not filtered.
- A masked assertion is harder to debug. Use `--reveal`.

**Upgrading.** Failure dumps change for every existing suite: the values of `Authorization`, `Cookie` and `Set-Cookie`, and of any JSON key, form field or query parameter with `password`, `secret` or `token` as a word, are now `***`. Whoever relied on reading them there needs `--reveal`, or a `fields_only` list of their own.

## Telemetry

bddkit can count how it is used, anonymously, and it does nothing of the kind until a person has said yes. The counts answer four questions and nothing else: how many runs happen, and how many of them in CI; how large suites are and how long their scenarios take; how often a run fails or never starts; which kinds of resources suites use (APIs, which database engines, SRP, which official plugins) and where the steps spend their time.

**Sending.** While telemetry is on, each command writes its payload to `telemetry-last.json` in your user layer (`bddkit telemetry show` prints it), and to the file `BDDKIT_TELEMETRY_LOG` names if you set one, then sends it once as `POST https://telemetry.bddkit.dev/v1/run`. The request waits at most 500 ms to connect and 1.5 s in all, DNS included, so a dead network or a broken resolver costs a command at most 1.5 s; it is not retried, prints nothing and never changes the exit code, a run you interrupt sends nothing, and a signal while the request is out ends the command at once with its own exit code. `BDDKIT_TELEMETRY_URL` points it at a collector of your own. Since sending starts with this release, an existing yes now takes effect: turn it off with `bddkit telemetry off`, `BDDKIT_TELEMETRY=0`, `DO_NOT_TRACK=1` or `telemetry: false` in `bddkit.yaml`.

**What a payload holds.** A fixed list of numbers, booleans and names from bddkit's own vocabulary: the bddkit version, the OS and architecture, which CI system (only that one of its variables is set), the command and its exit code, how long it took, how many files, scenarios and failures a run had and how many of those failures were of each class (`assertion`, `transport`, `db_connection`, `db_statement`, `input`, `plugin`, `reset`, `panic`), how long scenarios took (median, 90th percentile, longest, and how many fell under 100 ms, 1 s, 10 s and 60 s), which options were on (never their values), how many APIs, SRP configurations and databases of which engine, how many instances of which official plugin, and how many steps of each group ran and for how long. A plugin is named only when its lock entry says it was installed from a repository of the `bddkit` owner (`bddkit plugin install` writes that; a hand-written entry says nothing); any other is counted as "other". The full list is in [`docs/telemetry.md`](docs/telemetry.md).

**What is never in it.** Addresses, host names, URLs, DSNs, file paths; the names of feature files, features, scenarios, tags, resources, connections, environments, macros and private plugins; step text, variable values, request or response bodies, headers, SQL; any config value; your user name, host name, repository or git remote; error and panic messages. There is no identifier of any kind: nothing links two runs.

**Who decides.** One boolean, off until someone answers yes, set by the first of these that has an opinion:

| Source | Turns it off | Turns it on |
|---|---|---|
| flag, on `run` and `doctor` | `--telemetry false` | `--telemetry true` |
| environment | `BDDKIT_TELEMETRY=0` (or `false`) | `BDDKIT_TELEMETRY=1` (or `true`) |
| environment, the cross-tool convention | `DO_NOT_TRACK=1` | |
| the suite's config | `telemetry: false` in `bddkit.yaml` | `telemetry: true` |
| the `.bddkit/` chain | `enabled: false` in `telemetry.yaml` | `enabled: true` |

`BDDKIT_TELEMETRY` beats `DO_NOT_TRACK` when both are set. An empty variable is unset, and a value of `BDDKIT_TELEMETRY` that is neither of the four spellings counts as off. The suite's own key is the team's decision and its CI follows it, and it beats every `telemetry.yaml`. Among those files a later layer beats an earlier one, so `/etc/bddkit` (the shared layer) is a machine's or a CI image's default, which a person's own answer in the user layer overrides. A config that cannot be found, read or parsed means off, unless the flag or `BDDKIT_TELEMETRY` says on: a team that turned it off in `bddkit.yaml` must not report a run whose config it just broke. `plugin`, `report` and `upgrade` need no config, so for them a missing one is not a reason to stay silent, but one that exists and cannot be read is.

**The question.** When nothing has decided, an interactive `run` or `doctor` asks once, at the very end, after the summary: a `Yes`/`No` list with nothing highlighted, so a yes takes a deliberate move. Esc, `q`, Ctrl-C or end of input answer nothing and the question comes back next time; the exit code is the run's whatever happens. On a terminal that cannot draw the list (`TERM=dumb` or unset) it is a `[y/n]` line with no default. It is never asked in CI (any of `CI`, `GITHUB_ACTIONS`, `GITLAB_CI`, `JENKINS_URL`, `BUILDKITE`, `CIRCLECI`, `TF_BUILD`, `TEAMCITY_VERSION`, `BITBUCKET_BUILD_NUMBER` set), without a terminal, or after a run that was refused before it started. The answer is stored in the user layer's `telemetry.yaml`, so it is yours alone and never edits a committed file. Apart from this question `run` and `doctor` print nothing about telemetry.

**Commands.**

```bash
bddkit telemetry status             on or off, which source decided, and where a payload would go
bddkit telemetry show               the last payload recorded on this machine
bddkit telemetry off                for this user (telemetry.yaml in the user layer)
bddkit telemetry on --layer shared  for every user of this machine (/etc/bddkit)
```

`on` and `off` say so when a source above the file still decides otherwise. `bddkit doctor` has a `telemetry` row with the same wording (`off (decided by DO_NOT_TRACK)`); it is always `ok`. These commands send nothing and never ask.

**Where the files live.** `telemetry.yaml` and `telemetry-last.json` are in the user layer (`$XDG_CONFIG_HOME/bddkit`, else `~/.config/bddkit`; `%LOCALAPPDATA%\bddkit` on Windows), or in the one directory `--bddkit-dir` / `$BDDKIT_DIR` names. Neither belongs in a repository. The endpoint is one constant in the binary, which a distribution can replace at build time with `BDDKIT_TELEMETRY_ENDPOINT` (empty builds a binary that never sends) and a private collector can replace at run time with `BDDKIT_TELEMETRY_URL`; a debug build sends only to the latter.

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

`bddkit plugin install <name>` fetches a plugin release and registers it in a lock file for you — see [`docs/plugin-authoring.md` §7](docs/plugin-authoring.md#7-installing-a-plugin) for `update`, `remove`, `show` and `list` alongside it.

Which plugins are installed is **machine state, not test config**, and by default does not belong in the repository with the suite: the config describes the system under test, a path to a `.so` describes one laptop or one CI runner. The one deliberate exception is vendoring — installing into the `project` layer commits `.bddkit/plugins/` along with the lock file that points into it, for a suite that wants to carry its own plugin build; `project.local` is the gitignored variant for a local build that never leaves the machine. Otherwise it comes from a chain of `.bddkit/` directories, read layer by layer:

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

Writing one: [`docs/plugin-authoring.md`](docs/plugin-authoring.md) is the complete contract, `tests/fixtures/echo-plugin/` is a minimal plugin to copy, and [`bddkit-s3`](https://github.com/bddkit/bddkit-s3) is a real one to read — it serves the `s3` group in the example above.

## Where to look next

| For | Look at |
|---|---|
| Every step, authoritative | `BUILTIN_STEPS` in `src/steps/mod.rs` |
| How to run the examples | `examples/README.md` |
| A runnable HTTP example | `examples/api.yaml`, `examples/features/` |
| Every HTTP method, 404 included | `examples/features/methods.feature` |
| JSON matchers and paths | `examples/features/json_matchers.feature` |
| Variables: set, extract, reuse, typed positions, printing them | `examples/features/variables.feature` |
| Asserting over a variable, extracting from one | `examples/features/variable_assertions.feature`, `examples/macros/users.yaml` |
| Macros, nesting, Scenario Outline, typed parameters | `examples/features/macros.feature`, `examples/macros/posts.yaml` |
| Non-JSON responses, form login, file upload | `examples/features/content_types.feature` |
| Polling an assertion until it passes | `examples/features/eventual.feature` |
| The mock API behind all of it | `examples/mocks/api-server.yaml` |
| Every DB step, worked through | `examples/db-features/db.feature` |
| Cleaning up a run's data | `examples/db-features/cleanup.feature` |
| The same on MySQL and MariaDB | `examples/db-features-mysql/db.feature` |
| SRP handshake, Hawk signing | `tests/features/` |
| Hiding secrets, and the design behind it | the Privacy section above, `docs/events-design.md` section 7 |
| What the usage counts hold, field by field | the Telemetry section above, `docs/telemetry.md` |
| bddkit tested by bddkit (masking, end to end) | `selftest/README.md` |
| Config schema | `src/config.rs` |
| Writing a plugin | `docs/plugin-authoring.md`, `tests/fixtures/echo-plugin/` |
| Teaching a coding agent to use bddkit | `skills/bddkit/` (the Agent Skill bddkit.dev publishes: `SKILL.md` and its `references/`) |

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

**Where this is going.** `api`, `db`, and `srp` are the resource kinds that ship, not the ceiling — any other key under `resources:` is a capability group a plugin serves, so reaching an object store or a mailbox is the same move as reaching a second database. The seams that made that possible were there from the start: options cascade per instance, `I use "<name>" <kind>` is one step shape, and dispatch returns `passed | not yet | fatal` so eventual assertions work without knowing what they retry. `bddkit plugin install <name>` fetches one from the index into your `.bddkit/` chain — see `docs/plugin-authoring.md` §7.

## Development

```bash
cargo test
```

Some tests need a database; `docker-compose.yml` brings up all three — PostgreSQL on `:5433` (schema from `examples/db/init.sql`), MySQL on `:3307` and MariaDB on `:3308` (both from `examples/db/init-mysql.sql`). The DB suite runs against Postgres by default; `BDDKIT_TEST_ENGINE=mysql` or `=mariadb` points it at the other two, and CI runs all three. The same file brings up [Smocker](https://github.com/smocker-dev/smocker) as the HTTP example's mock API — it seeds `examples/mocks/api-server.yaml` at startup and serves it on `localhost:8080` (web UI on `localhost:8081`).

## License

Apache-2.0. See [LICENSE](LICENSE).
