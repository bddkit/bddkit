---
paths:
  - "src/runner.rs"
  - "src/world.rs"
  - "src/polling.rs"
  - "src/http.rs"
  - "src/steps/debug.rs"
  - "src/steps/assert.rs"
---

# Runner, scoping, eventual assertions

Loaded on demand; the root `CLAUDE.md` holds the invariants that apply everywhere.

- **On transport failure (`connection refused`/timeout), `HttpState::send` clears `self.last` first** so the failure dump never shows a stale prior exchange.
- **`@serial(<name>)` orders files, it does not share their state.** `World` (and its `VarStack`) is built per file inside `run_file`, so variables do NOT flow between files of a chain. This is the natural misreading of the tag — a tester who expects `companyId` to survive into the next file gets an «undefined variable» error.
- **Debug output is not part of the atomic per-file dump.** `I am in debug mode`, `Show …`, and `Print response …` write to **stderr** as they run, so with `concurrency > 1` two workers' debug output interleaves. Deliberate — routing every debug line through a per-file buffer would delay it past the step that produced it. Set `concurrency: 1` while debugging.
- **`--fail-fast` makes a run non-reproducible.** Which files got done depends on timing. That is inherent to «stop starting new work», not a defect.
- **`I attach the file` is intercepted in `execute_step`, not routed through `dispatch`.** Like `Include`, it needs the directory of the file the step is written in (`source`, the macro file for a macro body), which `dispatch` does not carry; its `dispatch` arm is `unreachable!`. The file is READ at the step, so the recipe owns the bytes and a replayed assertion resends the same content. A literal path is checked in `validate::check`, in include bodies and in macro bodies (`Registry::validate_macros`) through the one `validate::check_attached_file`. reqwest's `.multipart()` adds its own `Content-Type` with the boundary, so every configured one is stripped first — otherwise the request carries two; the boundary header is pushed onto the recorded exchange only AFTER the real headers are applied.
- **`tokio::spawn` needs `Send + 'static`.** `execute_step` returns `Pin<Box<dyn Future + Send + 'a>>`; keep the `+ Send`. If a new step breaks it, the compiler names the value held across the `await` — scope the guard in a block. Do NOT reach for `tokio::sync::Mutex`: an async mutex held across a DB round-trip serialises the workers, which defeats the whole runner design.
- **`Config.concurrency` defaults to 8.** A suite written for a sequential runner runs 8-way parallel with no config change; set `concurrency: 1` to get the sequential behavior back until the suite is made parallel-safe.

## Eventual assertions

`I expect the next assertion to pass ...` arms exactly one future assertion: inputs are interpolated once, a later modifier silently replaces an earlier one, and an unconsumed modifier is discarded at scenario end or reset. DB attempts query the current connection anew; an HTTP response assertion first checks the saved exchange, then replays its immutable original request at its original API after each mismatch, freshly signing Hawk on every replay. Polling options are inherited global → resource (`options.rs`). A `vars`-group assertion (`variable "x" should …`) answers `Fatal`, never `NotYet`, on a mismatch — nothing runs between attempts, so its subject cannot change (`vars::settled`); a runner test that needs a pollable assertion without network uses the echo plugin's `the echo counter should reach N`, not a variable.
