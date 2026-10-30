---
paths:
  - "src/report/**"
  - "tests/report.rs"
  - "docs/report-markdown.md"
---

# Reports

Loaded on demand; the root `CLAUDE.md` holds the invariants that apply everywhere.

- **Report paths are truncated before the config is read, in `run` and `doctor` alike.** `report::prepare` is the first thing `run` does — before `config::load` — so a run that dies on its config leaves an EMPTY `--junit`/`--cucumber-json` file a CI parser rejects loudly, never yesterday's green one; `tests/acceptance.rs::a_report_path_is_truncated_before_the_config_is_read` pins the order. `doctor` calls the same function for its `reports` stage, also first, so it truncates too — that is the promise "the same check", not a side effect to remove. The result model carries per-step status and timing (`report::StepResult`, `passed`/`failed`/`skipped` only) because both formats need them; the console never prints them. The XML writer is hand-rolled: `report::xml` escapes the five characters and replaces what XML 1.0 cannot carry at all (NUL — the `<<null>>` sentinel — and other control characters) with U+FFFD, since a `&#0;` reference is just as invalid as the byte.
