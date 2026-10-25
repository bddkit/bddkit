# Secrets

Output is masked (`***`); requests are not. A value is masked only once something marks it:

- **Config.** Write `${DB_PASS}`; keep the value in CI secrets or a gitignored `.env.local`. `doctor` and `run` fail while it is unset. Keys like `password`, `client_secret` and DSN passwords are masked automatically. `${VAR}` works in the config only.
- **Variables.** A name with `secret` as a word (`login_secret`) is masked. Send a token as `"Bearer <<login_secret>>"`, never as a literal: a literal in step text reaches the event stream.
- **Parameters.** `{password:secret}` in a macro. A test password in a feature is a literal: pass it through such a parameter.
- **By position.** `Authorization`, `Cookie` and `Set-Cookie` headers, and JSON, form and query fields named `password`, `secret` or `token`.
- **Not covered.** DB columns, non-JSON bodies, hashes of a secret, images.
- **`--reveal`** is for local use only. Output with `masking: false` holds secrets: never share it.
- **`--evidence`** writes passing bodies to disk; prefer `--evidence=failures`.
- A plugin runs with full privileges.
