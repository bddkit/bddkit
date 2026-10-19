# The Markdown report: a contract for tools

`bddkit report run.ndjson -o report.md` writes a Markdown file made for programs and agents that read it with `grep`, `sed`, `awk` and `head`. This page is what such a tool may rely on. A person can read the file as ordinary Markdown: the markers are HTML comments, which GitHub and IDE previews hide.

## Template version

The front matter key `bddkit_report` is the version of this contract, currently `1`. It changes when a tool written against this page could break: a marker kind or attribute removed or renamed, the id format changed, a key removed. Adding a key or an attribute does not change it, so a reader ignores what it does not know.

## Front matter

YAML between two `---` lines at the top, always inside `head -25`. Keys, in this order, a key being absent when it has no value: `bddkit_report`, `run_id`, `bddkit` (the version that ran), `started_at` (RFC 3339, UTC, milliseconds), `duration_ms`, `exit`, `signal` (only when interrupted), `files`, `scenarios`, `failed`, `evidence` (`--evidence` was given), `masking` (`false` under `--reveal`: the file may contain secrets), `complete` (the stream ended with `run_finished`; `duration_ms`, `exit` and `signal` are absent when it did not), `detail` (`failed` or `all`) and `lines` (the length of this very file). `files`, `scenarios` and `failed` count what the stream holds, whatever `detail` shows.

## Markers

Every part is wrapped in two marker lines, each a whole line starting at column 0:

```text
<!-- bddkit:begin <kind> id="<id>" key="value" … -->
…
<!-- bddkit:end <kind> id="<id>" -->
```

A marker matches `^<!-- bddkit:(begin|end) [a-z]+( [a-z_]+="[^"\\]*(\\.[^"\\]*)*")* -->$`. Nothing else in the file starts with `<!-- bddkit:` at column 0: a line of content that would — in a fenced block, a failure text, a body — is written with one leading space, the only change bddkit makes to content. So `grep '^<!-- bddkit:'` finds markers and only markers.

Values are escaped as the contents of a JSON string (`\"`, `\\`, `\n`, `\t`, `\u00XX`), and two hyphens in a row are written `--`, since an HTML comment cannot hold them. Parts nest, and an end marker repeats the kind and the `id` of its begin marker, so a tool can cut a part out by fixed strings without counting nesting. A begin marker's attributes come in the fixed order below, so a `grep` on `status="failed"` does not depend on order; an attribute with no value is left out.

| Kind | Attributes, in order |
|---|---|
| `howto`, `run`, `index` | none |
| `file` | `id` (the path as the stream names it), `scenarios`, `failed` |
| `scenario` | `id`, `status`, `file`, `line`, `example` (outline rows), `tags` (space-separated) |
| `failure` | `id` (the scenario's) |
| `evidence` | `id` (the scenario's), `step`, `kind`, `group`, `resource`, `path` |

The end marker of `file`, `scenario`, `failure` and `evidence` carries `id` and, for `evidence`, `step`; `howto`, `run` and `index` have none.

## Ids and steps

A scenario's id is `<file>:<line>`, or `<file>:<line>#<row>` for a Scenario Outline row (`example`, the stream's own 0-based ordinal across the outline's Examples tables). It is stable across runs, unlike the stream's per-run ordinals, so two reports can be compared by id, and an id opens the feature file at the right line.

A scenario's `status` is `passed`, `failed` or `open` (the stream ended inside it). An `open` scenario is shown under the default detail: a run cut short must not count what never finished as a pass.

An `evidence` part names its step by `step`: the step's position among its siblings in the scenario's numbered list, joined by dots for a nested step (`2.1` is the first step of the macro or include that step 2 called). Sections and disabled steps are not numbered apart from the list: a disabled step takes a number, a section does not. `kind` is the stream's own (`http`, `sql`, `image`, a plugin's), and `path` is set when the content is a file, linked relative to the report.

## What each part holds

- **`howto`**: instructions for reading the file, with commands that use this file's own name, a real id and a real line range. A tool does not need to parse it.
- **`run`**: the config, the selection, the masking and evidence state, and a warning for each banner of the HTML report (masking off, no evidence, incomplete stream, artifacts under the temporary directory).
- **`index`**: a table of `id`, `status`, `lines` (`from-to`, 1-based and inclusive, the begin marker line through the end marker line of that scenario in this file) and `name`, failed scenarios first and the rest in file order. Under `--detail failed` a second table counts the scenarios and the passed ones per file.
- **`file`**, holding its **`scenario`** parts: a heading, a location line, the rule, description and comment, the numbered steps (one per line, with a status word `[passed]`, `[failed]`, `[skipped]` or `[disabled]`, the duration and the `vars` it resolved), the `failure` part with the text in a fenced block, and one `evidence` part per item.

A file with nothing to show under the current detail has no `file` part; it still has its row in the per-file table.

Every fenced block uses a fence one backtick longer than the longest run of backticks in its content, three at least, with an info string (`text`, `json`, `sql`). No line is wrapped: a step, an index row, a header is one physical line.
