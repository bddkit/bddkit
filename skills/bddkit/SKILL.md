---
name: bddkit
description: Use when writing, running, fixing or reading the results of bddkit acceptance tests — Gherkin .feature files run by the bddkit binary against an HTTP API and the databases and services behind it, configured by a bddkit.yaml.
---

# bddkit

Runs Gherkin `.feature` files against an HTTP API, PostgreSQL/MySQL/MariaDB and plugin-served groups. A suite is `bddkit.yaml` plus features; `${VAR}` comes from the environment or a `.env.local` beside the config:

```yaml
paths: [features/]
macro_paths: [macros/]
resources:
  api:
    main: { base_url: "http://localhost:8080/" }
  db:
    app: { dsn: "postgres://app:${DB_PASS}@localhost/app" }
```

## Writing steps

- Never guess a step: `bddkit steps list --json`, or `--filter <text> -v`. Macros are not listed: read the `step:` templates under `macro_paths`.
- `<<var>>` is a runtime variable, `<name>` an outline placeholder.
- Each `<<unique()>>` is a new token `u<run_id><n>`; `email`/`slug`/`url` are the same token, so write `user-<<unique()>>@example.com`. Reuse one through `set variable`.
- Variables live per file, request state per scenario. Files run in parallel: unique data or `@serial(<name>)`.
- `@variableType(int|float|string|bool|array|object|null)`; there is no `number`, and `doctor` does not check matchers.
- Never weaken an assertion to go green.

## Check and run

`bddkit doctor` after every edit: exit 0 clean, 1 findings, no network. `bddkit run [paths] [--tag t]`:

| Exit | Meaning | Do |
|---|---|---|
| 1 | a scenario failed | read its class: `assertion`, `transport`, `db_statement`, `input` |
| 2 | never started (config, unknown step, a database unreachable at start), or a report not written | `bddkit doctor`; if clean, `bddkit doctor --live` names the dead resource |

## Reading results

`bddkit run --events run.ndjson` then `bddkit report run.ndjson -o report.md`. `grep -n '^<!-- bddkit:' report.md` lists every part; the `index` gives each scenario's line range for `sed -n`.

## More

| Task | Read |
|---|---|
| A domain step in YAML | [references/macros.md](references/macros.md) |
| Reusing a scenario: `I include` | [references/include.md](references/include.md) |
| Credentials and masking | [references/secrets.md](references/secrets.md) |
| CI, report formats, report.md in full | [references/reports.md](references/reports.md) |
