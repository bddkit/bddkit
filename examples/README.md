# Running the examples

One suite, one config. It needs `cargo build --release` first (or use `cargo run --release --` in place of `./target/release/bddkit`).

## HTTP suite — `examples/api.yaml`

Talks to a local [Smocker](https://github.com/smocker-dev/smocker) instance, so the suite runs offline and its responses are fixed by a file in this repo.

```bash
docker compose up -d smocker                      # from the repo root
./target/release/bddkit run --config examples/api.yaml
```

The mocks live in `examples/mocks/api-server.yaml` and are seeded into Smocker at startup — after editing them, `docker compose restart smocker` reloads. Smocker's web UI (mocks, request history) is on <http://localhost:8081>; the mocked API itself is on <http://localhost:8080>.

| File | What it demonstrates |
|---|---|
| `features/methods.feature` | GET/POST/PUT/PATCH/DELETE/HEAD/OPTIONS, `Location`/`Allow` headers, a 404 asserted like any other answer; the scenarios are grouped under `Rule:` blocks, and the write rule's `Background:` sets the JSON `Content-Type` once; the POST is marked with `I am collecting evidence` / `I am not collecting evidence`, so `--events run.ndjson --evidence=marked` keeps that exchange and no other |
| `features/json_matchers.feature` | `@variableType`, `@regExp`, `@arrayLength`, paths into arrays, `contains` vs `equals`, grouped under `Rule:` blocks |
| `features/variables.feature` | `set variable`, extraction from JSON and cookies, reuse across steps and scenarios, `<<unique()>>`, a `<<variable>>` in a typed position (an HTTP method, a status code), `Show all variables` and `Print table:` |
| `features/variable_assertions.feature` | substring, regex and JSON assertions over a variable, extraction from a variable by JSON path or regex, and a macro in `macros/users.yaml` that publishes its extracts with `global` |
| `features/macros.feature` | YAML-declared steps from `macros/posts.yaml`, macro calling a macro, Scenario Outline, a Cucumber Expression template with an alternation and optional text, a typed parameter (`{code:uint}`) given a literal or a `<<variable>>` |
| `features/content_types.feature` | HTML/XML/plain-text responses, a form login, multipart file uploads — a file plus a text field with a `<<variable>>` in the path, and two files under one part name (`fixtures/`) |
| `features/response_headers.feature` | `contains` / `matches` and their negations over a response header: a repeated `Link` line with double quotes inside the expected text, and a `Vary` list |
| `features/eventual.feature` | An assertion polled until it passes; a number written with or without quotes (`I sleep 0 seconds` / `I sleep "0" seconds`) |
| `features/include_setup.feature` + `features/include_caller.feature` | `I include "<file>" scenario "<name>"`, a Scenario Outline included with a `with:` table, `@exports` isolation — an unexported variable set inside the included scenario does not exist in the caller — and `with prefix "<p>"`, including the same flow twice without the second call's exports overwriting the first's. Neither file makes an HTTP request, so this pair also runs with no Smocker container: `./target/release/bddkit run --config examples/api.yaml examples/features/include_setup.feature examples/features/include_caller.feature` |

`eventual.feature` is only interesting on a fresh mock session: `/jobs/1` answers "pending" twice and then "done", and Smocker keeps that counter until the container restarts. Run `docker compose restart smocker` to see the polling actually retry — on a session where the job is already done, the assertion just passes on its first attempt.

## Database suites

The three database suites (Postgres, MySQL, MariaDB) and the table of what does not port between the engines live with the [`bddkit-db`](https://github.com/bddkit/bddkit-db) plugin, in its `examples/`.

## Narrowing a run

```bash
# one file (a positional path overrides the config's `paths`)
./target/release/bddkit run --config examples/api.yaml examples/features/methods.feature

# one tag (repeatable; feature-level tags apply to every scenario in the file)
./target/release/bddkit run --config examples/api.yaml --tag json
```

Exit codes: `0` everything passed, `1` a scenario failed, `2` the run never started (bad config, unknown step, no scenario matched the filter).
