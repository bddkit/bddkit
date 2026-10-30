---
paths:
  - "src/steps/db.rs"
  - "src/steps/plugin.rs"
  - "tests/fixtures/**"
  - "docs/plugin-authoring.md"
---

# DB and plugin step functions

Loaded on demand; the root `CLAUDE.md` holds the invariants that apply everywhere.

DB steps: read `src/db/CLAUDE.md` (vocabulary, SQL building, engine gotchas) before changing `src/steps/db.rs`.

Plugin steps: read `src/plugin/CLAUDE.md` and `docs/plugin-authoring.md` before changing `src/steps/plugin.rs` or a fixture plugin.
