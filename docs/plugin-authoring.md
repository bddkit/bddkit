# Writing a bddkit plugin

This is the complete contract. A plugin is written against this document, not against bddkit's source — nothing here requires reading the host.

## 1. What a plugin is

A plugin is a shared library (`cdylib`) that bddkit opens with `libloading` (`dlopen`) at startup and calls over a small C ABI. It adds two things to a run: **steps** that testers write in `.feature` files, and a **resource group** — a `resources.<group>` block in the config whose contents the host never interprets, exactly the way `resources.api` and `resources.db` work for the kinds that ship in the binary.

**There is no sandbox, and there will not be one.** A plugin runs inside the bddkit process, on the host's threads, with the host's full privileges: its file access, its network, its environment, its ability to `exit()`. `dlopen` runs the library's initialisers before bddkit has looked at a single byte of it. Installing a plugin is the same trust decision as installing any other binary on the machine, and the host's checks (ABI version, manifest name, group ownership) are consistency checks, not a security boundary.

The host and the plugin exchange **only JSON strings**. Rust has no stable ABI, so there is no shared type between them — the contract is a documented JSON schema plus the C representation of a pointer, and nothing else. That is what makes a plugin in another language possible in principle: everything in section 3 is JSON, and the Rust in this document is one way of producing it.

## 2. The crate shape

```toml
# Cargo.toml
# Its own workspace root on purpose: without this, a parent workspace builds
# the plugin implicitly and `cargo clippy --all-targets` in the parent sees it.
[workspace]

[package]
name = "bddkit-s3"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
serde_json = "1.0"
```

The build produces `libbddkit_s3.so` (Linux), `libbddkit_s3.dylib` (macOS) or `bddkit_s3.dll` (Windows) in `target/<profile>/`. That file's path is what goes into the lock file in section 7.

**`tests/fixtures/echo-plugin/` in the bddkit repository is a complete, working plugin — 260 lines, every export, every reply shape, the panic guard, the NUL handling.** Copy it and replace the body. It is also what the host's own test suite runs against, so it cannot rot: a change to the ABI that the fixture does not follow turns the suite red.

For a plugin that does real work rather than echoing its arguments back, read
[`sergeym/bddkit-s3`](https://github.com/sergeym/bddkit-s3): an S3/MinIO plugin
with 22 steps, written against this document and nothing else — no part of it
required reading the host's source, which is the claim this document opens
with. It is where to look for what a skeleton cannot show: validating an
instance config the host is blind to, deciding where `not_yet` ends and `fatal`
begins, attaching an HTTP exchange to a failure without leaking the credential
inside it, and refusing a file name that arrived from a `.feature` file.

## 3. The ABI

`ABI_VERSION` is **1**. Every symbol is `extern "C"` and `#[unsafe(no_mangle)]`.

| Symbol | Signature | Required |
|---|---|---|
| `bddkit_abi_version` | `() -> u32` | yes |
| `bddkit_manifest` | `() -> *mut c_char` | yes |
| `bddkit_list_steps` | `() -> *mut c_char` | yes |
| `bddkit_validate_config` | `(*const c_char) -> *mut c_char` | yes |
| `bddkit_init_instance` | `(*const c_char) -> *mut c_char` | yes |
| `bddkit_dispatch` | `(u64, u32, *const c_char) -> *mut c_char` | yes |
| `bddkit_drop_instance` | `(u64) -> *mut c_char` | yes |
| `bddkit_reset_scenario` | `(u64) -> *mut c_char` | no |
| `bddkit_probe_config` | `(*const c_char) -> *mut c_char` | no |
| `bddkit_free_string` | `(*mut c_char)` | yes |

A missing required symbol fails the load with a message naming the symbol. **Returning NULL from any of the string-returning exports is an error**, reported as "plugin `<name>` returned nothing from `<call>`" — there is no reply shape that means "nothing to say"; say it with an envelope.

At load the host does, in order: `dlopen`; call `bddkit_abi_version` and refuse anything but `1`; resolve every required symbol, then `bddkit_reset_scenario` if it is exported; call `bddkit_manifest`; refuse a manifest whose `name` differs from the lock entry's name, or whose `groups` is empty, or whose `concurrency` is a value outside the closed set of section 6; refuse a manifest that describes the config of a group it does not claim, or declares an `implicit_instance` for one; call `bddkit_list_steps`; refuse any step whose `group` the manifest does not claim; refuse a plugin that declares `shared`, exports `bddkit_reset_scenario`, and meets a run whose `concurrency` is greater than 1 (see that export below); call `bddkit_validate_config` once per instance — every one the config declares, plus the one synthesized from `implicit_instance` for a group the config has no section for. Nothing else is called until a scenario reaches a step.

Three questions can be asked about a `resources.<group>` entry, and each has its own mechanism. Together they state the boundary this document draws: the host knows *that* a group has a config, the plugin knows *what* it means, and only the plugin can find out whether it works.

| Question | Mechanism | Opens a connection |
|---|---|---|
| What keys does this group take? | `fields` in the manifest | no |
| Is this config well-formed? | `bddkit_validate_config` | no |
| Does it actually reach anything? | `bddkit_probe_config` | yes |

### `bddkit_abi_version`

Returns `1`. This is the one call made before the version is known, so it is an unavoidable act of faith on both sides: export it with exactly this signature.

### `bddkit_manifest`

```json
{
  "name": "s3",
  "version": "1.2.0",
  "groups": ["s3"],
  "concurrency": "shared",
  "fields": {
    "s3": [
      { "name": "bucket",   "required": true,  "description": "bucket the steps read and write", "example": "acceptance" },
      { "name": "endpoint", "required": false, "description": "S3-compatible endpoint; omit for AWS" },
      { "name": "path_style", "type": "boolean", "description": "address the bucket in the path", "example": "true" }
    ]
  }
}
```

- `name` (string, required) — must be byte-identical to the `name` in the lock entry that pointed at this file, otherwise the load fails. It is the name every diagnostic uses.
- `version` (string, required) — required so a manifest without one is a load failure; the host does not otherwise compare it against anything.
- `groups` (array of strings, required, non-empty) — the `resources.<group>` sections this plugin serves. Two loaded plugins may not claim the same group, and section 8 lists the names that are reserved.
- `concurrency` (string, optional, default `"shared"`) — see section 6.
- `fields` (object, optional) — what each group's `resources.<group>.<instance>` body accepts, keyed by group because one manifest may claim several. A key naming a group this manifest does not claim fails the load, for the same reason an unclaimed step group does: it would shadow the description of whichever plugin legitimately owns it. Describing nothing is fine, and is what a plugin written before this key existed does. This is what `bddkit resource fields --config <suite>` prints, beside the keys the host's own `api`, `db` and `srp` entries take.

It also gates `bddkit resource add <group> <name> --<field> <value>`, which is the one place the host acts on this list rather than printing it, and three consequences follow for whoever writes the manifest. A `--<field>` naming a key this list does not declare is refused before anything is written, so a key `validate_config` accepts but the manifest omits cannot be set by a flag at all — only through `--json`. A group that declares no `fields` is the opposite: with no list to check against, every `--<field>` is accepted and passed through as a string, and refusing a bad key is left entirely to `bddkit_validate_config`. And a key named `config`, `env`, `json` or `no-check` can never be set by a flag, because those four long flags belong to the command itself — such a key is refused outright with a message naming `--json` as the way to set it, so a group that needs one of those names should expect its users to reach it only that way.

Each entry is `{ "name", "required", "type", "description", "example" }`. `name` is the only required one; `required` defaults to `false`, `type` defaults to `"string"`, and `description` and `example` default to absent.

`type` is one of `"string"`, `"boolean"`, `"number"` or `"nonscalar"`, and it says what FORM the value takes — never anything about which values are acceptable, which stays `bddkit_validate_config`'s sole business. It exists because a flag's value is text and the host is the one writing YAML: without it, `--path_style false` writes the string `'false'`, and a key whose value is a boolean can then never be set by a flag at all — `validate_config` refuses it, correctly, on every input. Declaring `"type": "boolean"` makes the same flag write `false`. `"number"` takes an integer or a fraction; `"nonscalar"` is a key no flag can express, and a `--<field>` naming one is refused with a message pointing at `--json`, exactly as the host's own `default_headers` is. A name outside those four fails the load, with the field and the accepted names in the message: silently reading it as a string would put back the very failure the key removes.

A closed set of choices is a string with names worth reading, not a boolean — `url_style: path | virtual-hosted` beats `path_style: true | false` for the same reason a dropdown beats a checkbox in someone else's plugin API. `type` is for the cases where no such naming exists: a genuine flag, and a number.

Nothing about `type` reaches a running suite. A hand-written config is still passed to the plugin verbatim, and `type` says nothing about how the group body is parsed — it is consulted only when `resource add` turns a flag's text into YAML. **`example` is always a string**, including for a key whose value is a boolean or a number — write `"example": "true"`, never `"example": true`. A non-string fails the parse of the whole manifest, and the plugin then does not load at all, with a message about a malformed manifest that does not mention `example`. It is a hint for whoever writes the config, not a value a tool pastes in: the config for that key still takes `true`.

This is deliberately **not** JSON Schema: no nesting, no enums, no ranges, no defaults, no validation semantics of any kind. `type` is the one exception and it is not a rule about the value — it says what form the value takes, because the host is the one writing the YAML and cannot render `false` without knowing whether the key wants a boolean or a string. Enforcement is `bddkit_validate_config`'s job, which is the boundary this whole document keeps — a second, weaker copy of the rules here could only ever disagree with the first. Carry what the keys cannot express in `description`: that a key has a default, that it needs a scheme, that two keys are mutually exclusive.

Describing a key you do not accept, or accepting one you do not describe, is a bug of exactly the kind the `bddkit_list_steps` index note warns about. If your `validate_config` checks the config against a list of known keys, derive one list from the other or keep them adjacent, so a key added to one cannot be missed in the other.

- `implicit_instance` (object, optional) — the config body an instance of a group takes when the suite declares **no** `resources.<group>` section at all, keyed by group. For a plugin whose every key has a default this is what makes the zero-config case work: without it the first step of the group fails with "no instance of the resource group is selected, and no default_<group> is set", and nothing the plugin does can change that, because the refusal comes before `bddkit_init_instance` is ever called.

```json
{
  "name": "exec",
  "version": "0.1.0",
  "groups": ["exec"],
  "concurrency": "per_worker",
  "implicit_instance": { "exec": {} }
}
```

For every group listed here whose section is absent from the config, the host synthesizes one instance named `default` from the declared body, with the suite's global `options` cascaded over it exactly as for a declared instance that has no `options` key of its own. It then goes where a declared instance goes: through `bddkit_validate_config` at load — a body your own validator refuses fails the load with `plugin "<name>": implicit_instance.<group>: <error>`, which is the right time to find out — into `bddkit doctor` (listed and, under `--live`, probed), and it becomes `default_<group>` by the existing rule that a group's only instance is its default. Any declared `resources.<group>` section, **even an empty map**, switches the synthesis off for that group: a suite that names its instances gets exactly the ones it named, and `bddkit resource add <group> <name>` on a group that has only the implicit instance creates the section and, by that rule, retires the implicit one. An explicit `default_<group>` naming a group with no section is still an error, as for any undeclared instance. A key naming a group this manifest does not claim fails the load, as for `fields`.

Unknown keys are ignored, so a newer plugin can add manifest keys without breaking an older host — a plugin declaring `implicit_instance` loads on a host that predates the key, and simply meets the old refusal.

### `bddkit_list_steps`

An **array** at the top level:

```json
[
  { "pattern": "^I upload file \"(?P<file>[^\"]+)\" to \"(?P<bucket>[^\"]+)\"$", "group": "s3", "kind": "action",
    "description": "uploads a local file to the bucket this instance is configured for" },
  { "pattern": "^the bucket should contain \"(?P<key>[^\"]+)\"$", "group": "s3", "kind": "assertion",
    "description": "asserts an object with this key exists in the bucket" }
]
```

- `pattern` (string, required) — a Rust `regex` crate pattern. Anchor it with `^…$`. Each capture group becomes one positional argument at dispatch, in order.
- `group` (string, required) — must be one the manifest claims.
- `kind` (string, required) — `"action"` or `"assertion"`. Only an assertion may answer `not_yet`, and only an assertion consumes an armed eventual-assertion modifier.
- `description` (string, optional) — one line stating the step's effect, shown by `bddkit steps list --config <file> -v`. Localizing it is your job, not the host's: bddkit never translates a plugin's text. Omit it and the step lists without a description.

**Name your capture groups.** `bddkit steps list` renders a pattern as a template by replacing each group with its name, so `(?P<bucket>[^"]+)` reads as `<bucket>` while a bare `([^"]+)` degrades to `<value1>`. Named groups stay addressable by index, so nothing about dispatch changes.

**The index of a step in this array is its identity.** It is the `u32` the host passes to `bddkit_dispatch`, so the array's order is part of your plugin's internal contract with itself — dispatch on the index, and keep the `match` arms and the array in one place so they cannot drift.

The `regex` crate has no backtracking and no lookahead. Patterns are matched against the **raw step text**, before any variable interpolation, so a pattern can never depend on a variable's value.

If your pattern also matches a bddkit built-in step, another plugin's step, or a macro, that step is **ambiguous**: the run stops before the first request with a message listing every definition that matched. It is not "first wins" — nothing is silently shadowed, but nothing works either. Make your patterns specific.

### `bddkit_validate_config`

Called once per declared instance at startup, before the first request, with the same request payload as `init_instance`. Nothing is connected yet — this is a schema check, not a reachability check. Rejecting here is what turns a typo in the config into exit code 2 instead of a failure halfway through the suite.

Request:

```json
{
  "group": "s3",
  "instance": "backups",
  "config": { "bucket": "acme-backups", "endpoint": "http://minio:9000" },
  "options": { "polling": { "timeout_secs": 30, "interval_ms": 100 } }
}
```

All four keys are always present. `config` is the instance's YAML body converted verbatim to JSON, minus the reserved `options` key. The host has no schema for it and never looks inside.

Reply — the **envelope**, shared with `drop_instance` and `reset_scenario`:

```json
{ "ok": true }
```

```json
{ "ok": false, "error": "resources.s3.backups requires a string \"bucket\"" }
```

`error` is optional; if it is missing on a failure the host substitutes "the plugin reported a failure with no message" rather than losing the failure. Always send one — the host prefixes it with `resources.<group>.<instance>: `, so write the part after the colon.

### `bddkit_probe_config`

Optional, and the live counterpart of `validate_config`: the one call that is allowed to open a connection. A plugin that does not export it simply has no live check, and that is reported as **not available** — never as a failure. This is where a probe differs from `bddkit_reset_scenario`, which the host treats as `ok` when it is absent: having no per-scenario state to clear really is success, while a reachability check that never ran has proved nothing.

Request: identical to `validate_config`. Reply: the same envelope.

Called by `bddkit doctor --live`, once per declared instance of your groups. A plugin that does not export it gets a `skipped` row saying so, not a failed one.

**Stateless.** There is no handle, no `init_instance` beforehand and no `drop_instance` after: build whatever client you need, answer, and tear it down before you reply. A handle-based probe would make every caller run a three-call lifecycle and own the cleanup on each error path, for a call that happens once and holds nothing.

**Never called during a run.** No scenario probes. This is asked by a person or a script that wants to know whether a configuration reaches anything, so it is allowed to be slow and allowed to need the network — the two things `validate_config` must never be.

The split from `validate_config` is the point of having both. `validate_config` runs eagerly at startup for every declared instance, which is what turns a config typo into exit code 2 before the first request; that only works because it connects to nothing and cannot hang. Do not "strengthen" it into a reachability check. Put the reachability check here.

Say what failed, not which layer failed: an endpoint that does not answer, a bucket that does not exist and credentials the server rejected are three different problems, and the error text is the entire value of this call.

The `echo` fixture is the exception that proves the shape rather than the substance: it has nothing to reach, so its probe just returns whatever `probe_error` holds in the instance config. Copy its structure — the guard, the parse, the envelope — and put your client where its `match` is.

### `bddkit_init_instance`

Called **lazily**: the first time a scenario runs a step of that group with that instance selected. Under `shared` that happens once for the whole run; under `per_worker` it happens once per feature file that reaches such a step, because each file gets its own instance — see section 6. A declared instance nobody uses is validated but never initialised. This is where you open connections, spawn processes, authenticate.

Request: identical to `validate_config`.

Reply:

```json
{ "ok": true, "handle": 7 }
```

```json
{ "ok": false, "error": "cannot reach http://minio:9000: connection refused" }
```

`handle` is a `u64` you choose — an index into your own table, never a pointer. `ok: true` with no `handle` is an error. Handle `0` is legal.

**A failed init is not cached.** The step that triggered it fails, and the *next* step of that group tries again — so an expensive initialisation that keeps failing is re-attempted per step, not once per run. Fail fast, and put anything slow-but-optional behind the first dispatch that needs it.

**Every call must return a handle distinct from every other live instance of your plugin.** The host initialises without holding its lock, so two workers can call `init_instance` for the same instance at the same time; the loser's handle is then passed to `bddkit_drop_instance` while the winner's stays in use. A plugin that returns a singleton handle would have its live instance destroyed underneath it by that drop. Under `per_worker` several handles being live at once is not a race but the normal state of a parallel run: every feature file that runs a step of yours holds its own.

### `bddkit_dispatch`

`bddkit_dispatch(handle, step_index, request)`. `step_index` is the position in your `list_steps` array.

Request:

```json
{
  "args": ["report.pdf", "backups"],
  "docstring": null,
  "table": null,
  "artifacts_dir": "/tmp/bddkit-3n2k9a0f1x7q/000007",
  "workspace_dir": "/tmp/bddkit-3n2k9a0f1x7q/workspace/000004",
  "debug": false,
  "options": { "polling": { "timeout_secs": 30, "interval_ms": 100 } }
}
```

All seven keys are always present; `docstring` and `table` are `null` when the step carries neither.

- `args` — the regex capture groups in order, **already interpolated**: `<<variable>>`, `<<unique()>>` and `<<null>>` have been resolved before the payload was built. A plugin never sees raw step text and never sees bddkit's variable syntax.
- `docstring` — the step's doc string, interpolated, or `null`.
- `table` — the step's data table as an array of rows of strings, cells interpolated, or `null`. **Row 0 is the header row**; bddkit does not strip it, and gherkin guarantees every row has the same length as row 0.
- `artifacts_dir` — a fresh, unique directory path for this one dispatch, under `<temp dir>/bddkit-<run id>/<six-digit counter>`. **The host does not create it.** Call `create_dir_all` before writing, and write nothing if you have nothing to write — most dispatches never touch it.
- `workspace_dir` — the working directory of the feature file this call belongs to, for files the run produces rather than evidence about a failure. **The host has already created it**, unlike `artifacts_dir`. It is the same directory for every dispatch in one feature file and a different one in every other file, so a plugin can accept a bare file name from a tester without two files colliding on it. It is shared with the host's own steps, so a file written by one and read by the other needs no path in the feature text. Nothing deletes it: a failed run's files stay inspectable.
- `debug` — true while the scenario is inside `I am in debug mode`. It resets at the scenario boundary.
- `options` — the polling options resolved for this instance, after the global → instance cascade and after any armed eventual-assertion modifier. Informational: **the retry loop is the host's**, see section 5.

Reply:

```json
{
  "status": "passed",
  "vars": { "uploaded_etag": "d41d8cd9" },
  "diagnostics": [],
  "error": null
}
```

```json
{
  "status": "fatal",
  "error": "403 Forbidden",
  "diagnostics": [
    { "title": "PUT /backups/report.pdf", "kind": "http", "content": "HTTP/1.1 403 Forbidden\n…", "path": null },
    { "title": "Screenshot",              "kind": "image", "content": null, "path": "/tmp/bddkit-3n2k9a0f1x7q/000007/fail.png" }
  ]
}
```

- `status` (required) — `"passed"`, `"not_yet"` or `"fatal"`. Anything else fails the parse and the step.
- `vars` (optional, default `{}`) — string → string, written into the scenario's variables so later steps can read `<<uploaded_etag>>`. **Kept only when `status` is `passed`**; on `not_yet` and `fatal` they are dropped, so an intermediate observation cannot leak into the scenario.
- `diagnostics` (optional, default `[]`) — evidence, rendered into the failure dump. **Kept only on the failure path**; a `passed` reply's diagnostics are discarded. There is currently no channel for evidence from a step that succeeded.
- `error` (optional, default `null`) — the failure message. See section 5.

A diagnostic is `{"title", "kind", "content", "path"}`: `title` and `kind` are required strings, `content` and `path` are optional. `kind` is free-form text — `text`, `json`, `http` and `image` are the conventional values, and an unrecognised one degrades to "print it as text" rather than failing the parse. The host renders each as `--- <title> (<kind>) ---`, then `content`, then `path`, appended to `error`.

### `bddkit_drop_instance`

`bddkit_drop_instance(handle)`, replying with an envelope. **When it is called depends on the mode in section 6:** a `per_worker` instance is dropped when its feature file ends — mid-run, while other files are still working — and a `shared` one after the worker pool has drained. Both happen on **every** exit path, including a failed run and `--fail-fast`: a file that panics never reaches its own cleanup, so the host sweeps whatever is still registered at the end of the run instead. One more case is neither, and it is `per_worker` only: a per-file handle whose very first dispatch failed at the boundary — a malformed reply, a NUL byte, no reply at all — is dropped right there, before the step's error is reported, because nothing else holds it yet. A `shared` handle in the same situation is kept and reused, since it is already the run's one instance for that group. Close connections and kill child processes here whichever mode you declare: a plugin's external process must not outlive the run, and under `per_worker` it must not outlive its file. A failure is printed as a warning and does not change the exit code.

### `bddkit_reset_scenario`

`bddkit_reset_scenario(handle)`, replying with an envelope. Optional — a plugin with no per-scenario state need not export it, and the host then treats every reset as `ok`.

**A plugin that keeps per-scenario state must declare `per_worker`.** Its instance belongs to one feature file, so the only boundary that resets it is that file's own and no other worker is inside it. A `shared` instance is one per run, so one worker's boundary would reset the instance every other worker is also mid-scenario with — an assertion whose counter an unrelated file keeps zeroing, or a file with no plugin steps at all failing on someone else's reset. A `shared` plugin exporting this symbol is therefore refused at load when the run's `concurrency` is greater than 1: exit 2, naming the plugin, before the first request, and the message names `per_worker` as the remedy. At `concurrency: 1` the pairing is legal, because there is one worker and one scenario boundary. If your plugin needs no per-scenario state at all, leave the symbol out and it runs shared at any concurrency.

Called at the scenario boundary for every instance **the current feature file has used**, before the Background steps of the next scenario — never for an instance only some other file has touched, and not at all in a file that has run no step of yours. This is the plugin's half of bddkit's state rule: HTTP state and the selected instance reset per scenario, variables do not. Clear anything the previous scenario left behind.

**A failed reset fails that scenario**, exactly as a failing Background step would — running a scenario against state the plugin could not clear is how a suite goes green for the wrong reason. Answer `{"ok":false,"error":…}` only when you mean it.

### `bddkit_free_string`

```rust
/// # Safety
/// `s` must be a pointer this library returned and has not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bddkit_free_string(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}
```

The host copies every reply and immediately hands the original pointer back to **this** function. It never frees plugin memory itself, and you must not free host memory: the `*const c_char` request pointer belongs to the host and is valid only for the duration of the call.

## 4. Four rules only you can keep

The host cannot check any of these. Each one is undefined behaviour or a corrupted test report when broken.

**Wrap every export in `catch_unwind` — including the argument decoding.** Unwinding across an FFI boundary is undefined behaviour. The subtle half is *where* the guard starts:

```rust
fn guard(kind: &str, body: impl FnOnce() -> String) -> *mut c_char {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(reply) => out(reply),
        Err(_) => out(match kind {
            "dispatch" => r#"{"status":"fatal","error":"the plugin panicked"}"#.to_string(),
            _ => r#"{"ok":false,"error":"the plugin panicked"}"#.to_string(),
        }),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_validate_config(request: *const c_char) -> *mut c_char {
    guard("envelope", move || {
        let raw = input(request);   // INSIDE the guard, on purpose
        …
    })
}
```

Writing `guard("envelope", input(request), |raw| …)` would evaluate `input(request)` *before* entering the guard, and the export would then be exactly as panic-free as that one helper. The same applies to whatever produces the final string: in the fixture, `out` runs outside the guard and is safe there only because it provably cannot panic — `CString::new` returns a `Result` and the fallback literal has no interior NUL. Anything added to such a helper has to preserve that property.

Guard the trivial exports too. "Every export is guarded" is an invariant a reader can check in one pass; "this one happens to be safe" is a judgement every future edit has to make again.

**Free your own strings in `bddkit_free_string`.** A string allocated by your crate's allocator must be released by your crate's allocator. The host and the plugin can be built with different allocators, different `std` versions, even different compilers; freeing plugin memory with the host allocator is undefined behaviour.

**Never print to stdout.** bddkit composes a whole feature file's output into one string and writes it with a single `print!`, precisely so that parallel workers cannot interleave. A line your plugin prints lands in the middle of another file's failure dump, attributed to the wrong test. Return `diagnostics` instead — they end up inside the dump, attached to the step that produced them.

**stderr** is the escape hatch, and only for tracing while someone is debugging: bddkit's own debug steps use it, precisely because it is outside the per-file dump. It still interleaves between parallel workers, so gate it behind the request's `debug` flag and tell your users to set `concurrency: 1` when they turn it on. Nothing you write there ends up in the test report.

**`create_dir_all(artifacts_dir)` before writing into it.** The host allocates a fresh unique path per dispatch but does not create the directory, because most dispatches write nothing and a `mkdir` per step for the rare case is waste. `workspace_dir` is the opposite and the exception: the host creates that one before handing it over, because it is one directory per feature file rather than a fresh one per dispatch, and several writers share it. Do not `create_dir_all` it and do not delete it.

## 5. `passed | not_yet | fatal`

- **`passed`** — the step succeeded. `vars` are published; `diagnostics` are dropped.
- **`fatal`** — the step failed and retrying cannot help. The scenario fails with `error` plus the rendered diagnostics.
- **`not_yet`** — *one fresh observation says the condition is not met yet.* Only an assertion may answer this. An action that answers `not_yet` fails the scenario with "a plugin action answered not_yet, which only an assertion may do".

**Never sleep inside a plugin.** `not_yet` means "I looked, just now, and it was not there". The host owns the retry loop, the interval and the timeout — it is what the tester armed with `I expect the next assertion to pass within "30" seconds, checking every "500" milliseconds`, and what the `options.polling` cascade configures. A plugin that sleeps burns the host's budget from inside and makes the timeout mean nothing.

Every attempt is a full `bddkit_dispatch` with a fresh `artifacts_dir`. Re-observe: query again, fetch again, list the bucket again. Do not cache the first answer.

**Without an armed modifier there is no second attempt**, and `not_yet` is simply a failure. That is deliberate: an assertion is not implicitly eventual, the tester opts in per step.

**Send an `error` with every `not_yet` and every `fatal`.** It is optional in the schema, and the host substitutes "the plugin reported a failure with no message" rather than losing the failure — but that message is what the tester reads when the poll finally times out, so make it say what was observed: `"the bucket holds 2 objects, expected 3"`, not `"failed"`.

## 6. `concurrency`

bddkit runs feature files in parallel — `concurrency: 8` by default — and the manifest's `concurrency` is what the host promises about your handles. It is not a request for a thread count.

- **`shared`** (the default) — one instance serves the whole run, and the host may call that one handle from several workers at the same time. In Rust that usually means a `Mutex` around your instance table and around anything mutable inside an instance.
- **`per_worker`** — the host never calls one handle from two workers at once. **This host keeps that promise by giving you one instance per feature file**, created lazily on that file's first step of yours and dropped when the file ends. That is stricter than the mode demands — a file runs on one worker and its scenarios are sequential — and it may later become one instance per worker, with no ABI change and no manifest change. So do not depend on an instance outliving the file that created it, and do not expect two files to share one: state you accumulate is per file, and several instances are alive at once whenever the run is parallel.

The value set is closed: an unknown `concurrency` string fails the manifest parse. A new scheduling mode arrives with an `ABI_VERSION` bump, not with a tolerant parse, so that a mode the host does not implement can never be silently degraded into one it does.

A plugin that genuinely cannot be shared declares `per_worker` and then runs at any concurrency — it does not need its users to set `concurrency: 1`. That is also the mode for per-scenario state, as `bddkit_reset_scenario` above spells out.

## 7. Installing a plugin

There is no `bddkit plugin install` yet — that is a later milestone. Write the lock file by hand:

```yaml
# .bddkit/plugins.yaml
plugin:
  - name: s3
    path: ./libbddkit_s3.so
```

- `plugin` is a list of `{name, path}`. Both are required; a missing `path` is a parse error naming the file.
- **`name` must equal the manifest's `name`.** A mismatch fails the load with both names in the message — it means the lock file points at the wrong file, and every later diagnostic would name the wrong plugin.
- `path` is absolute, or **relative to the lock file's own directory** — that is `.bddkit/`, not the project root. `./libbddkit_s3.so` in the example above resolves to `.bddkit/libbddkit_s3.so`.
- **`~` is not expanded.** A `~/plugins/libs3.so` reaches `dlopen` verbatim and fails with a confusing "no such file".
- Extra keys are ignored, so a file written by a future `plugin install` (`version`, `source`, `sha256`, `target`) still loads here.

Two files are read, and the project one overrides the user one **entry by entry, keyed by `name`**:

| Scope | Location |
|---|---|
| project | `<directory of the --config file>/.bddkit/plugins.yaml` |
| user | `$HOME/.config/bddkit/plugins.yaml` |

The project lock is anchored to the config file's directory, not the working directory, so `bddkit run --config suites/cfg.yaml` finds the same plugins from anywhere. A missing lock file means no plugins, which is the normal case. An unset `HOME` means no user lock.

This file is **machine state and does not belong in the committed test config**: the config describes the system under test, a `.so` path describes one machine.

Then declare instances in the ordinary config, under the group name:

```yaml
paths: [features/]
resources:
  api: {}
  s3:
    backups:
      bucket: acme-backups
      endpoint: http://minio:9000
      options:
        polling: { timeout_secs: 30, interval_ms: 100 }
    archive:
      bucket: acme-archive
default_s3: backups
```

`options` is the one reserved key inside an instance body; the host takes it out, cascades it over the global `options`, and passes the rest through as `config`. `default_<group>` is inferred when the group declares exactly one instance, must be explicit when it declares several, and a `default_<group>` naming a group the config does not declare is a startup error (it is how `default_s4` for `default_s3` gets caught).

A `resources.<group>` block with no plugin serving that group exits 2 with "no installed plugin serves the group". A loaded plugin whose group the config never mentions is fine — it is simply never used.

Testers then select an instance with `I use "archive" s3`, which the host builds from the loaded group names. The selection resets to `default_<group>` at every scenario boundary, and macros may call plugin steps like any other step.

## 8. Footguns

**`<<null>>` arrives as a real NUL byte.** bddkit's SQL-NULL sentinel is `"\u{0}__bddkit_null__\u{0}"`. JSON escapes control characters, so the payload the host sends is clean ASCII — but after your `serde_json` parse, that argument is a Rust `String` **containing two literal NUL bytes**. `CString::new(arg)` on it returns `Err(NulError)`. Any string of yours that becomes a `CString` — a reply, a path, an argument to a C library — must handle it. The fixture's `out` shows the pattern: `CString::new(s).unwrap_or_else(|_| CString::new("{\"ok\":false,\"error\":\"NUL in reply\"}").expect("literal"))`. Deciding that `<<null>>` means something in *your* domain is your call; crashing on it is not.

**Your library is never unloaded.** bddkit deliberately leaks the mapping at process exit rather than `dlclose` it, because running a thread-local destructor or an `atexit` handler after the code page is unmapped is a segfault in someone's CI, and the process is exiting anyway. It also runs every plugin call on tokio's blocking-pool threads, which outlive the run. So: your `Drop` impls, thread-locals and `atexit` handlers may never run. Everything that must be released — connections, child processes, temp files — is released in `bddkit_drop_instance`, which *is* always called.

**Four group names are reserved.** `api`, `db`, `srp` and `connection` are refused with "which bddkit serves itself". The first three are the resource kinds in the binary; `connection` is the built-in step word for the `db` group (`I use "<conn>" connection`), and a plugin claiming it would generate a group-switch step that exactly overlaps the built-in one.

**A YAML tag inside an instance body does not parse.** `resources.<group>` is captured through a flattened map, and serde's buffering for flattened fields cannot represent a YAML tagged value:

```yaml
resources:
  s3:
    backups:
      key: !Foo bar        # error: untagged and internally tagged enums do not support enum input
```

Anchors and aliases are fine; tags are not. Design your instance config out of plain scalars, sequences and mappings.

**`list_steps` is array-rooted.** It has no place to grow a top-level field — a version marker, a capability list — without the host learning to accept both an array and an object. Everything a step needs goes in the step object, where unknown keys are already ignored.

**There is no SDK crate.** Your `Manifest`, `StepSpec` and `DispatchResult` are a hand-copied mirror of the host's types in `src/plugin/abi.rs`. Nothing makes them agree; this document is the only thing preventing the two from drifting. Two consequences: pin the bddkit version you tested against in your README, and prefer building JSON with `serde_json::json!` over deriving `Serialize` on a struct that only looks like the host's — a typo'd key name is then visible in the source instead of hidden behind a `#[serde(rename)]` you forgot.
