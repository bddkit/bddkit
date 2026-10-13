# Self-test: bddkit testing bddkit

Scenarios that run the `bddkit` binary on a scratch project and assert on what it printed and wrote. A bddkit feature can only see what a system under test returns, so the console, the reports and the event stream of a nested run are reached through the published [`exec`](https://github.com/bddkit/bddkit-exec) plugin.

This is not `examples/`, which is the demonstration surface and must always be green: the fixtures here fail on purpose.

## Running it

From the repository root:

```bash
cargo build
./target/debug/bddkit plugin install exec        # once; installs into your user layer
./target/debug/bddkit run --config selftest/bddkit.yaml
```

`BDDKIT_BIN` names the binary the scenarios run, `target/debug/bddkit` by default. To keep the plugin out of your user layer, point `BDDKIT_DIR` at a scratch directory for both commands.

## Layout

| Path | What |
|---|---|
| `bddkit.yaml` | the suite's config: the `exec` group, anchored at the repository root |
| `macros.yaml` | `I run the masking fixture "<config>" with options "<options>"` |
| `features/privacy.feature` | what must stay out of the console, the JUnit report and the stream |
| `fixtures/masking/` | a project whose six scenarios fail on purpose, with a config per variation (default, `words_only`, a committed `reveal`) |
| `out/` | what the nested run wrote; git-ignored |

The Rust acceptance tests in `tests/` are still the gate that needs no plugin download. This suite proves the same behaviour end to end, from the user's side of the binary.
