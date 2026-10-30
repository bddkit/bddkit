---
paths:
  - "src/macros.rs"
  - "src/include.rs"
  - "src/annotate.rs"
  - "src/validate.rs"
  - "src/feature.rs"
  - "tests/macros/**"
---

# Macros, includes, disabled steps

Loaded on demand; the root `CLAUDE.md` holds the invariants that apply everywhere.

- **A disabled step is decided by the loaded registry.** A plugin that is not loaded turns its commented steps into ordinary comments. `annotate::resolve` is the one classifier shared by `run`, `I include` and `doctor`; it matches after outline substitution and never interpolates or executes a disabled step. Background, rule Background and include annotations stay at their own execution level, and annotations after a failure still reach the stream.
- **`I include` isolates variables via a full `VarStack` swap, not a pushed frame like macros.** The caller's globals must be invisible inside the included scenario, since file-level variables are a property of the file being run, not of every code path it can reach — a full swap (build a fresh `VarStack` at include entry, restore the original at exit) is the only way to guarantee that. The include path is a compile-time literal, resolved relative to the file the step is written in. All validation lives in the shared `validate::check` path both `run` and `doctor` call, so `doctor` needed no new stage for include-specific checks.

## Macros (declarative domain steps)

YAML files listed under `macro_paths` declare domain steps (`I login as user "..."`) without a rebuild. Matching is transparent — a step is a built-in or a macro, the caller can't tell. Macros take scalar parameters, run in isolated frames that publish results through named/glob `exports`, and may nest up to depth 16. Conflicts and cycles are caught at startup validation (exit 2). Macro call tables/docstrings and loops are intentionally out of scope. Templates are Cucumber Expressions with typed parameters — see the step-declarations gotcha.
