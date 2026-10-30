---
paths:
  - "src/upgrade.rs"
  - "install.sh"
  - "install.ps1"
---

# `bddkit upgrade`

Loaded on demand; the root `CLAUDE.md` holds the invariants that apply everywhere.

- **`upgrade` trusts only its receipt, and the staged binary is the only judge of the new plugins.** `install.sh`/`install.ps1` write `install.json` (`path`, `version`, `target`) into `dirs::user_data` — computed from the environment alone, so `--bddkit-dir` cannot hide it — and `upgrade` proceeds only when the canonicalised `current_exe()` equals the receipt's path (the one exception: `--rollback` may run as `<data>/previous/bddkit`, which is how a new binary that will not start is undone). The running process speaks the OLD plugin ABI, so a new-ABI plugin is fetched with `Context::fetch_into` (no `Library::load`, no lock write, no deleting the previous version) and vouched for by `<staged> plugin check <path>`, a hidden command; `install_into` is the load-and-register half only `plugin install`/`update` use. `install::host_release` takes the ABI as a parameter for the same reason. The staging directory sits beside the binary, not in the temp directory: a rename cannot cross filesystems, and a directory that cannot be created there is the «not writable» refusal before a byte is downloaded. Nothing is deleted before the switch, and `Cleanup` removes only the plugin directories THIS run created. `<data>/previous/pending.json` names exactly the lock files this upgrade backed up as `<file>.before-upgrade`, so a rollback after a same-ABI upgrade does not restore backups an older one left; the next switch drops the old ones. Tests never replace `CARGO_BIN_EXE_bddkit`: they copy it into a sandbox and serve a `#!/bin/sh` script as the «new release» (`Stub::publish_bddkit`), which is also why the hooks live in the script and not in the host. Windows (`self-replace`) is not exercised by any test.
