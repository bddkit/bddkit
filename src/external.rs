//! `bddkit <name>` runs `bddkit-<name>` (issue #171), the way git and cargo do.
//! The core knows nothing about what such a tool is: no telemetry, no config.

use anyhow::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// `[a-z][a-z0-9-]*`: a name that cannot climb out of a directory.
fn is_tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_runnable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// `bddkit-<name>` beside the running binary first, then on `path`.
fn find(name: &str, exe_dir: Option<&Path>, path: Option<&OsStr>) -> Option<PathBuf> {
    let file = format!("bddkit-{name}{}", std::env::consts::EXE_SUFFIX);
    let on_path = path.into_iter().flat_map(std::env::split_paths);
    exe_dir
        .map(Path::to_path_buf)
        .into_iter()
        .chain(on_path)
        // An empty PATH entry would make a relative candidate that `Command`
        // then resolves on its own.
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(&file))
        .find(|candidate| is_runnable(candidate))
}

/// Runs the tool and returns its exit code; on Unix it does not return on
/// success (the process is replaced). `Err` is a refusal to start.
pub fn run(args: Vec<OsString>) -> Result<i32> {
    let name = args
        .first()
        .and_then(|a| a.to_str())
        .filter(|n| is_tool_name(n));
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .context("cannot locate the running bddkit")?;
    let tool = name.and_then(|n| find(n, exe.parent(), std::env::var_os("PATH").as_deref()));
    let Some(tool) = tool else {
        use clap::CommandFactory;
        use clap::error::{ContextKind, ContextValue};
        // Today's clap error, typo tip included: parse the name again with
        // external subcommands off, then add the hint.
        let shown = args[0].to_string_lossy();
        let mut error = crate::Cli::command()
            .allow_external_subcommands(false)
            .external_subcommand_value_parser(None::<clap::builder::ValueParser>)
            .try_get_matches_from([OsStr::new("bddkit"), args[0].as_os_str()])
            .expect_err("an unknown subcommand is refused");
        let mut tips = match error.get(ContextKind::Suggested) {
            Some(ContextValue::StyledStrs(tips)) => tips.clone(),
            _ => Vec::new(),
        };
        tips.push(format!("no built-in command or `bddkit-{shown}` on PATH").into());
        error.insert(ContextKind::Suggested, ContextValue::StyledStrs(tips));
        error.exit()
    };
    let mut command = std::process::Command::new(tool);
    command
        .args(&args[1..])
        .env("BDDKIT_BIN", &exe)
        .env("BDDKIT_VERSION", env!("CARGO_PKG_VERSION"));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Only returns on failure: signals and exit code are the tool's own.
        Err(command.exec()).with_context(|| format!("cannot start {}", args[0].to_string_lossy()))
    }
    #[cfg(not(unix))]
    {
        // Ctrl-C and Ctrl-Break reach both processes: with a listener alive
        // here the default handler does not kill this one, and the tool
        // decides what happens. Never polled, only kept.
        #[cfg(windows)]
        let _listeners = (
            tokio::signal::windows::ctrl_c().ok(),
            tokio::signal::windows::ctrl_break().ok(),
        );
        let status = command
            .status()
            .with_context(|| format!("cannot start {}", args[0].to_string_lossy()))?;
        Ok(status.code().unwrap_or(1))
    }
}

#[cfg(test)]
mod tests {
    use super::is_tool_name;

    #[test]
    fn only_plain_lowercase_names_are_looked_up() {
        assert!(is_tool_name("ui"));
        assert!(is_tool_name("a-b2"));
        for bad in ["", "Ui", "2ui", "-ui", "a/b", "a.b", "../x", "a_b"] {
            assert!(!is_tool_name(bad), "{bad}");
        }
    }
}
