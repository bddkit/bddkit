# CI and reports

```sh
export BDDKIT_PRIVACY_REVEAL=false
bddkit run --events run.ndjson --evidence=failures --artifacts-dir artifacts \
  --junit junit.xml || status=$?
bddkit report run.ndjson -o report.md -o report.html
exit ${status:-0}
```

- Render after a failure too: GitLab `when: always`, GitHub `if: always()`.
- `.md` is for agents (failed scenarios only; `--detail all` shows all). `.html` is for people. `--junit` and `--cucumber-json` feed CI summaries.
- Archive `artifacts/` with the report, or use `--embed-artifacts` for one file.
- `report` exits 0, or 2 with nothing written. A killed run still renders, with `complete: false`.

## report.md

- `head -25` is the front matter: `exit`, `files`, `scenarios`, `failed`, `masking`, `complete`, `lines`.
- Parts sit between `<!-- bddkit:begin <kind> id="…" -->` and `<!-- bddkit:end <kind> id="…" -->` at column 0. `grep -n '^<!-- bddkit:'` finds them and nothing else.
- `index`: each scenario's `id` (`<file>:<line>`, `#<row>` for an outline row), `status` and `lines`. Cut one out with `sed -n '<from>,<to>p'`.
- A scenario holds numbered steps (`[passed]`, `[failed]`, `[skipped]`, `[disabled]`), a `failure` part with `kind`, and `evidence` parts (HTTP, SQL) under `--evidence`.
- Full format: `docs/report-markdown.md` in the bddkit repository.
