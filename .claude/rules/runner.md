---
paths:
  - "src/runner.rs"
  - "src/world.rs"
  - "src/polling.rs"
  - "src/steps/debug.rs"
---

# Runner, scoping, eventual assertions

Loaded on demand; the root `CLAUDE.md` holds the invariants that apply everywhere.

- **`@serial(<name>)` orders files, it does not share their state.** `World` (and its `VarStack`) is built per file inside `run_file`, so variables do NOT flow between files of a chain. This is the natural misreading of the tag — a tester who expects `companyId` to survive into the next file gets an «undefined variable» error.
- **Debug output is not part of the atomic per-file dump.** `I am in debug mode`, `Show …`, and `Show …` write to **stderr** as they run, so with `concurrency > 1` two workers' debug output interleaves. Deliberate — routing every debug line through a per-file buffer would delay it past the step that produced it. Set `concurrency: 1` while debugging.
- **`--fail-fast` makes a run non-reproducible.** Which files got done depends on timing. That is inherent to «stop starting new work», not a defect.
- **`tokio::spawn` needs `Send + 'static`.** `execute_step` returns `Pin<Box<dyn Future + Send + 'a>>`; keep the `+ Send`. If a new step breaks it, the compiler names the value held across the `await` — scope the guard in a block. Do NOT reach for `tokio::sync::Mutex`: an async mutex held across a DB round-trip serialises the workers, which defeats the whole runner design.
- **`Config.concurrency` defaults to 8.** A suite written for a sequential runner runs 8-way parallel with no config change; set `concurrency: 1` to get the sequential behavior back until the suite is made parallel-safe.

## Eventual assertions

`I expect the next assertion to pass ...` arms exactly one future assertion: inputs are interpolated once, a later modifier silently replaces an earlier one, and an unconsumed modifier is discarded at scenario end or reset. A plugin assertion that can change between attempts answers `not_yet`, and the plugin replays whatever it needs. Polling options are inherited global → resource (`options.rs`). A `vars`-group assertion (`variable "x" should …`) answers `Fatal`, never `NotYet`, on a mismatch — nothing runs between attempts, so its subject cannot change (`vars::settled`); a runner test that needs a pollable assertion without network uses the echo plugin's `the echo counter should reach N`, not a variable.
