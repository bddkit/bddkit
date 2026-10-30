---
paths:
  - "src/privacy.rs"
  - "src/http.rs"
  - "src/world.rs"
  - "src/vars.rs"
  - "src/hawk.rs"
  - "src/report/**"
  - "src/events.rs"
  - "src/doctor.rs"
  - "src/config.rs"
---

# Privacy (secrets are masked, never rewritten)

Loaded on demand; the root `CLAUDE.md` holds the invariants that apply everywhere.

The spec is `docs/events-design.md` section 7; the tester's view is the README's Privacy section. The mechanism is one set of secret values and one filter; the rules that keep it correct:

- **Masking is render-only.** `Exchange`, `RequestRecipe`, `VarStack` values and plugin arguments keep their real values — assertions, polling replays and plugins depend on them. Text is masked where it is PRODUCED for a human or a file, never in the state it came from.
- **One handle per file, shared on purpose.** `World::redactor` is a `Redactor` forked from `RunContext::redactor` at the start of `run_file`, and `VarStack`, `HttpState` and `Trace` hold clones of it. `I include` swaps the whole `VarStack` for a fresh one, so `run_include` hands the fresh stack the same handle — drop that line and a secret saved inside an include is not masked in the caller's failure (`a_secret_saved_inside_an_include_is_masked_in_the_callers_failure`). The mutex inside is a `std::sync::Mutex` and is never held across an await.
- **Order is position, then value, then the cut.** Values under sensitive names are hidden by position (`Exchange::spans`, `json_hits`), then the set is applied, THEN a body is cut to 600 characters, so a secret straddling the cut cannot show its first characters. Mask before escaping too: strings are masked before they become JSON (`Trace`, `doctor::Report::row`); reports are XML-escaped only after `run_file`'s final masking pass.
- **Register before the request leaves.** `HttpState` registers a header, query value, form value or JSON body when the STEP sets it (a scenario that fails before sending still prints that step's text in its report), and `execute` again before `client.execute` (a literal `?token=` in the path, a default header, a signed header), because a refused connection's error quotes the URL; the response half is registered after. Dropping either half breaks a test in `tests/acceptance.rs` (`a_json_value_sent_under_a_sensitive_name_is_secret_everywhere_afterwards`, `a_token_only_the_response_carries_is_masked_by_the_response_scan`).
- **Reports get a final pass.** A value can join the set after the step that mentions it was recorded, so `run_file` masks every scenario name, failure, step text and warning once more at the end. The event stream is masked as it is emitted and never rewritten (spec 7.8).
- **The process sink is append-only.** `privacy::record` is called by `main::run`/`doctor_command` (via `set_flags`) and by `config::load_str` BEFORE the typed parse, because a serde type error quotes the offending value; `mask_text` masks the `error:` lines, `doctor` rows and the `resource add` block from it. Tests share the process, so a unit test that loads a config must never set `reveal` or a large `min_length`.
- **`display_text` is for every output that names a step** — console, both reports, the stream, polling messages, the debug `macro` line, nested macro and include errors. A new place that prints `step.text` raw leaks a short literal secret (shorter than `min_length` never joins the set; only position can hide it).
- **Error text that quotes input is a leak.** `dotenvy`'s own message quotes the whole `NAME=secret` line, so `load_env_file` reports a file and a position only. A new parser error on a config path gets the same scrutiny.
