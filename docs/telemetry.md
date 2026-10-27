# Telemetry payload, schema 1

What `bddkit` counts when telemetry is on, field by field. The tester's view — who decides, the question, the commands — is the README's Telemetry section; the code is `src/telemetry.rs` and `src/telemetry/`. A field is added only with a new schema number, and every change to this file is a changelog entry.

The payload is a closed struct of numbers, booleans and names from bddkit's own vocabulary. No field can hold text from a suite, so no masking is involved. The next release sends it to `POST <endpoint>/v1/run`; this one only records it (`bddkit telemetry show`, `BDDKIT_TELEMETRY_LOG`).

## Every payload

| Field | Type | Meaning |
|---|---|---|
| `schema` | int | `1` |
| `bddkit` | string | the version |
| `os` | string | `linux`, `macos` or `windows` |
| `arch` | string | `x86_64`, `aarch64`, other values as Rust names them |
| `ci` | enum or `null` | `github`, `gitlab`, `jenkins`, `buildkite`, `circleci`, `azure`, `teamcity`, `bitbucket`, `other`; `null` outside CI. Detected only by whether each system's own variable is set and not empty (`GITHUB_ACTIONS`, `GITLAB_CI`, `JENKINS_URL`, `BUILDKITE`, `CIRCLECI`, `TF_BUILD`, `TEAMCITY_VERSION`, `BITBUCKET_BUILD_NUMBER`; else `CI` gives `other`), never from a value |
| `command` | enum | `run`, `doctor`, `report`, `init`, `plugin_install`, `plugin_update`, `plugin_remove`, `upgrade` |
| `exit` | int | the code the process returns |
| `stage` | enum or `null` | for a `run` that exited 2 after its config was read: `plugins`, `steps`, `selection`, `validation`, `scheduling`, `resources`, `reports`, `events`. A refusal before the config was read sends nothing, so there is no `config` stage. `null` when the run started, and for the few refusals no stage names (the artifacts directory, an invalid privacy option) |
| `duration_ms` | int | the whole command |

The commands that send a payload are `run`, `doctor`, `report`, `plugin install`, `plugin update` (not `--dry-run`), `plugin remove`, `upgrade`, and an `init` that ends with a yes. No other command sends anything, `telemetry status|show|on|off` included. A run an interrupt ended sends nothing.

## `run`

| Field | Type | Meaning |
|---|---|---|
| `files` | int | feature files that ran |
| `scenarios` | int | scenarios that ran, outline rows counted one each |
| `examples` | int | of those, outline rows |
| `failed` | int | scenarios failed |
| `concurrency` | int | the effective worker count |
| `fail_fast` | bool | |
| `scenario_ms` | object or `null` | `p50`, `p90`, `max` (nearest rank, in ms) and `buckets`: counts of scenarios under 100 ms, under 1 s, under 10 s, under 60 s, and 60 s and over. `null` when no scenario finished. The durations themselves are not sent |
| `features` | object | what was ON, whichever source turned it on, never a value: `junit`, `cucumber_json`, `events` (the flag or `$BDDKIT_EVENTS`), `artifacts_dir`, `tags`, `paths`, `env` are booleans; `evidence` is the list of active sources (`marked`, `failures`, `all`), never which steps were marked |
| `privacy` | object | `reveal`: masking was off, by the flag, `BDDKIT_PRIVACY_REVEAL` or `privacy.reveal`; `custom_words`, `custom_fields`: whether the lists differ from the defaults, never the words |
| `resources` | object | `api`, `srp`: counts; `db`: count per engine (`postgres`, `mysql`, `mariadb`); `plugins`: declared instances per official plugin name; `plugins_other`: instances of every other plugin |
| `usage` | object | per step group that ran at least one step: `steps` (leaf steps executed) and `ms` (their summed duration). Groups are the builtin ones (`api`, `db`, `srp`, `vars`, `debug`, `general`), the groups of official plugins, and `other` for the groups of every other plugin. A macro call and an `I include` are not counted: the steps they run are, so their time is not counted twice |

A plugin is official when its lock entry's `source` is a repository of the `bddkit` owner. An entry with no `source`, which is what a hand-written one has, is never official; the field is in a file the person can edit, so one that names a `bddkit` repository is believed.

A MariaDB server is counted as `mysql` in a payload built without a pool (`doctor`, with or without `--live`, and a run refused before its pools): the two share the `mysql://` scheme and only a connection's `SELECT VERSION()` tells them apart.

## `doctor`

The common fields, `privacy` and `resources` as for `run`, and:

| Field | Type | Meaning |
|---|---|---|
| `doctor.live` | bool | `--live` was given |
| `doctor.failed` | int | failed rows |
| `doctor.stages_failed` | list | the names of the stages with a failed row, from doctor's own fixed set |

The run-only counters (`files`, `scenarios`, `scenario_ms`, `usage`, …) are absent: `doctor` runs no scenario.

## `report`

The common fields and `report`: `formats` (`html`, `md`), `detail` (`failed` or `all`, Markdown only), `embed` (`none`, `original`, `lossless` or `lossy`, HTML only), `events` (events read), `evidence` (the stream carried evidence), `complete` (it ended with `run_finished`), `artifacts` (an artifact directory was given) and `result` (`ok` or `error`).

## `plugin install`, `plugin update`, `plugin remove`

The common fields and `plugin`, a list with one entry per plugin the command acted on:

| Field | Meaning |
|---|---|
| `name`, `version` | for an official plugin only: the plugin's own name once its manifest was read, or the index's name for it when the command failed earlier; the version once it is known to exist. Never what was typed |
| `other` | `true` for any other plugin, whose name and repository are not sent |
| `from` | `registry` (a name looked up in the index) or `repository` (`owner/repo` given). Absent for `remove`, which looks nothing up |
| `layer` | `shared`, `user`, `project`, with `.local`, or `override` |
| `result` | `ok`, or the class of the failure: `not_found` (no such name or release, or nothing to remove), `no_build` (nothing published for this OS and architecture or this plugin ABI), `checksum`, `network`, `refused` (the library failed to load: wrong ABI, bad manifest), `other` |

An entry with `other: true` and neither `from` nor `layer` is a name that was asked for and matched nothing.

## `upgrade`

The common fields and `upgrade`: `from` (the running version), `to` (the version, only once the release holding it has been downloaded and has said so itself; absent otherwise), `abi_changed`, `plugins_switched`, `plugins_refused` (no release for the new ABI, or refused by the new binary), `plugins_left` (hand-written and project entries, listed and never changed), `result` (`ok`, `refused`, `failed`, `rolled_back`) and `check` (`--check`).

## `init`

Sent only when the answer to the telemetry question is yes (`--telemetry true` or the last question) and the sources above the key `init` just wrote still say on. The common fields and `init`:

| Field | Meaning |
|---|---|
| `interactive` | questions were asked on a terminal |
| `plugins` | plugins chosen, a count per official plugin name |
| `plugins_other` | chosen plugins that are not official, or whose name was not known when they failed |
| `plugins_failed` | installs that failed |
| `resources` | what was configured, in the shape of `run`'s `resources` (`plugins` counts the installs that succeeded) |
| `settings` | `parallel` and `project_layer`: whether the answer differs from the default |

## Not in schema 1

The failure class of a scenario (`assertion`, `transport`, …) is planned for schema 2, after the classes are a product feature in the reports.
