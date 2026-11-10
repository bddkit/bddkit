---
paths:
  - "src/privacy.rs"
  - "src/world.rs"
  - "src/vars.rs"
  - "src/report/**"
  - "src/events.rs"
  - "src/doctor.rs"
  - "src/config.rs"
---

# Privacy (secrets are masked, never rewritten)

Loaded on demand; the root `CLAUDE.md` holds the invariants that apply everywhere.

The spec is `docs/events-design.md` section 7; the tester's view is the README's Privacy section. The mechanism is one set of secret values and one filter; the rules that keep it correct:

- **Masking is render-only.** `Exchange`, `RequestRecipe`, `VarStack` values and plugin arguments keep their real values — assertions, polling replays and plugins depend on them. Text is masked where it is PRODUCED for a human or a file, never in the state it came from.
- **One handle per file, shared on purpose.** `World::redactor` is a `Redactor` forked from `RunContext::redactor` at the start of `run_file`, and `VarStack` and `Trace` hold clones of it. `I include` swaps the whole `VarStack` for a fresh one, so `run_include` hands the fresh stack the same handle — drop that line and a secret saved inside an include is not masked in the caller's failure (`a_secret_saved_inside_an_include_is_masked_in_the_callers_failure`). The mutex inside is a `std::sync::Mutex` and is never held across an await.
- **Order is position, then value, then the cut.** Values under sensitive names are hidden by position (`Exchange::spans`, `json_hits`), then the set is applied, THEN a body is cut to 600 characters, so a secret straddling the cut cannot show its first characters. Mask before escaping too: strings are masked before they become JSON (`Trace`, `doctor::Report::row`); reports are XML-escaped only after `run_file`'s final masking pass.
- **A plugin registers what it learns through its reply.** The `secrets` and `scan` keys of a dispatch reply join the set before anything else in the reply is read (`docs/plugin-authoring.md`); the HTTP exchange and what it sets are the `api` plugin's.
- **Reports get a final pass.** A value can join the set after the step that mentions it was recorded, so `run_file` masks every scenario name, failure, step text and warning once more at the end. The event stream is masked as it is emitted and never rewritten (spec 7.8).
- **The process sink is append-only.** `privacy::record` is called by `main::run`/`doctor_command` (via `set_flags`) and by `config::load_str` BEFORE the typed parse, because a serde type error quotes the offending value; `mask_text` masks the `error:` lines, `doctor` rows and the `resource add` block from it. Tests share the process, so a unit test that loads a config must never set `reveal` or a large `min_length`.
- **`display_text` is for every output that names a step** — console, both reports, the stream, polling messages, the debug `macro` line, nested macro and include errors. A new place that prints `step.text` raw leaks a short literal secret (shorter than `min_length` never joins the set; only position can hide it).
- **A step no single entry matched has no position to mask.** `Registry::unknown_step_detail` and `find`'s ambiguity error quote the raw text, so `steps::record_literal_secrets` puts a literal secret into the process sink (`privacy::record_value`) first, and `mask_text` hides it by value. `privacy.min_length` applies there, unlike `display_text`.
- **Error text that quotes input is a leak.** `dotenvy`'s own message quotes the whole `NAME=secret` line, so `load_env_file` reports a file and a position only. A new parser error on a config path gets the same scrutiny.
