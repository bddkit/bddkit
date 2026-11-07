//! Fixture plugin for `handle_scope: "group"`, `eager_init` and `deprecated`.
//! It claims two groups, `gone` and `gtwo`, so a test can tell one handle per
//! group from one per instance.
//!
//! Switches, all read here and set by the host test on the CHILD process
//! (`Command::env`), never with `set_var`: a manifest and a symbol table are
//! fixed, so no config can reach them.
//!
//! - `BDDKIT_GROUP_FIXTURE_MODE=per_worker`: the manifest says `per_worker`
//!   and declares no `eager_init`; the default is `shared` with
//!   `eager_init: ["gone"]`.
//! - `BDDKIT_GROUP_FIXTURE_BREAK=<case>`: a manifest the host must refuse at
//!   load: `target` (a deprecated word for a group not claimed), `group` (a
//!   deprecated word that is a group), `host` (one that is a host group),
//!   `eager_unclaimed` (`eager_init` for a group not claimed), `eager_per_worker`
//!   (`eager_init` with `per_worker`).
//! - `BDDKIT_GROUP_FIXTURE_FAIL_INSTANCE=<name>`: `init_instance` refuses and
//!   names that instance.
//! - `BDDKIT_GROUP_FIXTURE_FAIL_SELECT=1` (with `hooks`): `select_instance`
//!   answers `ok: false`.
//! - `BDDKIT_GROUP_FIXTURE_HOST_GROUPS=1`: the manifest claims `api`, `srp` and
//!   `db` instead, with `eager_init: ["db"]` and `deprecated: {"connection":
//!   "db"}`, one more step reads the selected `api` instance and another takes
//!   a built-in's text, `the response code is <n>`: what only a `dev-unreserve`
//!   build of the host accepts.
//! - `BDDKIT_GROUP_FIXTURE_LOG=<path>`: every `init`, `select`, `reset` and
//!   `drop` appends one line, so a test sees the calls from outside.
//!
//! An instance body with a `refuse` key is refused by `validate_config`, with
//! that key's value as the error.
//!
//! The cargo feature `hooks` adds `bddkit_reset_scenario`,
//! `bddkit_select_instance` and `bddkit_failure_context`.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

static HANDLES: Mutex<Option<HashMap<u64, Handle>>> = Mutex::new(None);
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
static BARRIER: (Mutex<(usize, u64)>, Condvar) = (Mutex::new((0, 0)), Condvar::new());
const BARRIER_TIMEOUT: Duration = Duration::from_secs(2);

struct Handle {
    /// What `I set the group value` stored: it lives on the HANDLE, so it is
    /// visible after a switch only if one handle serves both instances.
    value: String,
    instances: usize,
}

fn wait_at_barrier(parties: usize) -> bool {
    let (state, released) = &BARRIER;
    let mut guard = state.lock().expect("barrier");
    guard.0 += 1;
    if guard.0 >= parties {
        guard.0 = 0;
        guard.1 += 1;
        released.notify_all();
        return true;
    }
    let generation = guard.1;
    let (mut guard, timeout) = released
        .wait_timeout_while(guard, BARRIER_TIMEOUT, |s| s.1 == generation)
        .expect("barrier");
    if timeout.timed_out() {
        guard.0 -= 1;
        return false;
    }
    true
}

fn log(line: String) {
    if let Some(path) = std::env::var_os("BDDKIT_GROUP_FIXTURE_LOG")
        && let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        // One write: with `writeln!` the line and its newline are two, and
        // two workers' lines interleave.
        let _ = file.write_all(format!("{line}\n").as_bytes());
    }
}

/// Cannot panic: see the worker fixture's `out`.
fn out(s: String) -> *mut c_char {
    CString::new(s)
        .unwrap_or_else(|_| {
            CString::new("{\"ok\":false,\"error\":\"NUL in reply\"}").expect("literal")
        })
        .into_raw()
}

fn input(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

fn guard(kind: &str, body: impl FnOnce() -> String) -> *mut c_char {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(reply) => out(reply),
        Err(_) => out(match kind {
            "dispatch" => r#"{"status":"fatal","error":"the plugin panicked"}"#.to_string(),
            _ => r#"{"ok":false,"error":"the plugin panicked"}"#.to_string(),
        }),
    }
}

fn host_groups() -> bool {
    std::env::var_os("BDDKIT_GROUP_FIXTURE_HOST_GROUPS").is_some()
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_abi_version() -> u32 {
    4
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_manifest() -> *mut c_char {
    guard("envelope", || {
        let per_worker = std::env::var("BDDKIT_GROUP_FIXTURE_MODE").as_deref() == Ok("per_worker");
        let mut manifest = serde_json::json!({
            "name": "group",
            "version": "0.1.0",
            "groups": ["gone", "gtwo"],
            "concurrency": if per_worker { "per_worker" } else { "shared" },
            "handle_scope": "group",
            "eager_init": if per_worker { vec![] } else { vec!["gone"] },
            "deprecated": { "oldswitch": "gone" }
        });
        if host_groups() {
            manifest["groups"] = serde_json::json!(["api", "srp", "db"]);
            manifest["eager_init"] = serde_json::json!(["db"]);
            manifest["deprecated"] = serde_json::json!({ "connection": "db" });
        }
        match std::env::var("BDDKIT_GROUP_FIXTURE_BREAK").as_deref() {
            Ok("target") => {
                manifest["deprecated"] = serde_json::json!({ "oldswitch": "elsewhere" })
            }
            Ok("group") => manifest["deprecated"] = serde_json::json!({ "gtwo": "gone" }),
            Ok("host") => manifest["deprecated"] = serde_json::json!({ "db": "gone" }),
            Ok("eager_unclaimed") => manifest["eager_init"] = serde_json::json!(["elsewhere"]),
            Ok("eager_per_worker") => {
                manifest["concurrency"] = "per_worker".into();
                manifest["eager_init"] = serde_json::json!(["gone"]);
            }
            _ => {}
        }
        manifest.to_string()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_list_steps() -> *mut c_char {
    guard("envelope", || {
        let (first, second) = if host_groups() {
            ("db", "srp")
        } else {
            ("gone", "gtwo")
        };
        let mut steps = serde_json::json!([
            { "pattern": r#"^I set the group value to "([^"]*)"$"#, "group": first, "kind": "action" },
            { "pattern": r#"^I read the group value into "([^"]*)"$"#, "group": first, "kind": "action" },
            { "pattern": r#"^I read the selected instance into "([^"]*)"$"#, "group": first, "kind": "action" },
            { "pattern": r#"^I read the group handle into "([^"]*)"$"#, "group": first, "kind": "action" },
            { "pattern": r#"^I wait at the group barrier for (\d+) parties$"#, "group": first, "kind": "action" },
            { "pattern": r#"^I read the gtwo handle into "([^"]*)"$"#, "group": second, "kind": "action" }
        ]);
        if host_groups() {
            steps.as_array_mut().expect("an array").push(serde_json::json!(
                { "pattern": r#"^I read the selected api instance into "([^"]*)"$"#, "group": "api", "kind": "action" }
            ));
            steps.as_array_mut().expect("an array").push(serde_json::json!(
                { "pattern": r#"^the response code is (\d+)$"#, "group": "api", "kind": "assertion" }
            ));
        }
        steps.to_string()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_validate_config(request: *const c_char) -> *mut c_char {
    guard("envelope", move || {
        let value: serde_json::Value = serde_json::from_str(&input(request)).unwrap_or_default();
        match value["config"]["refuse"].as_str() {
            Some(reason) => serde_json::json!({"ok": false, "error": reason}).to_string(),
            None => r#"{"ok":true}"#.to_string(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_init_instance(request: *const c_char) -> *mut c_char {
    guard("envelope", move || {
        let value: serde_json::Value = match serde_json::from_str(&input(request)) {
            Ok(v) => v,
            Err(e) => return serde_json::json!({"ok": false, "error": e.to_string()}).to_string(),
        };
        let group = value["group"].as_str().unwrap_or_default();
        let instances: Vec<String> = value["instances"]
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        if let Ok(bad) = std::env::var("BDDKIT_GROUP_FIXTURE_FAIL_INSTANCE")
            && instances.contains(&bad)
        {
            return serde_json::json!({"ok": false, "instance": bad, "error": "unreachable"})
                .to_string();
        }
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::SeqCst);
        log(format!(
            "init {group} instances={} concurrency={}",
            instances.len(),
            value["concurrency"]
        ));
        HANDLES
            .lock()
            .expect("handles")
            .get_or_insert_with(HashMap::new)
            .insert(
                handle,
                Handle {
                    value: String::new(),
                    instances: instances.len(),
                },
            );
        serde_json::json!({"ok": true, "handle": handle}).to_string()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_dispatch(
    handle: u64,
    step_index: u32,
    request: *const c_char,
) -> *mut c_char {
    guard("dispatch", move || {
        let value: serde_json::Value = match serde_json::from_str(&input(request)) {
            Ok(v) => v,
            Err(e) => {
                return serde_json::json!({"status": "fatal", "error": e.to_string()}).to_string();
            }
        };
        let arg = value["args"][0].as_str().unwrap_or_default().to_string();
        let instance = value["instance"].as_str().unwrap_or("<none>").to_string();

        // Outside the table lock: the second party must be able to dispatch.
        if step_index == 4 {
            return match arg.parse::<usize>() {
                Ok(parties) if wait_at_barrier(parties) => {
                    serde_json::json!({"status": "passed"}).to_string()
                }
                _ => serde_json::json!({
                    "status": "fatal",
                    "error": "the other parties never reached the barrier"
                })
                .to_string(),
            };
        }

        let mut guard = HANDLES.lock().expect("handles");
        let Some(state) = guard.get_or_insert_with(HashMap::new).get_mut(&handle) else {
            return serde_json::json!({"status": "fatal", "error": "unknown handle"}).to_string();
        };
        let published = match step_index {
            0 => {
                state.value = arg;
                return serde_json::json!({"status": "passed"}).to_string();
            }
            1 => state.value.clone(),
            2 | 6 => instance,
            7 => return serde_json::json!({"status": "passed"}).to_string(),
            3 | 5 => format!("{handle}/{}", state.instances),
            other => {
                return serde_json::json!({"status": "fatal", "error": format!("unknown step {other}")})
                    .to_string();
            }
        };
        serde_json::json!({"status": "passed", "vars": {arg: published}}).to_string()
    })
}

#[cfg(feature = "hooks")]
#[unsafe(no_mangle)]
pub extern "C" fn bddkit_reset_scenario(handle: u64) -> *mut c_char {
    guard("envelope", move || {
        log(format!("reset {handle}"));
        r#"{"ok":true}"#.to_string()
    })
}

#[cfg(feature = "hooks")]
#[unsafe(no_mangle)]
pub extern "C" fn bddkit_failure_context(handle: u64) -> *mut c_char {
    guard("envelope", move || {
        log(format!("context {handle}"));
        r#"{"diagnostics":[]}"#.to_string()
    })
}

#[cfg(feature = "hooks")]
#[unsafe(no_mangle)]
pub extern "C" fn bddkit_select_instance(_handle: u64, request: *const c_char) -> *mut c_char {
    guard("envelope", move || {
        let value: serde_json::Value = serde_json::from_str(&input(request)).unwrap_or_default();
        log(format!(
            "select {}.{}",
            value["group"].as_str().unwrap_or_default(),
            value["instance"].as_str().unwrap_or_default()
        ));
        if std::env::var_os("BDDKIT_GROUP_FIXTURE_FAIL_SELECT").is_some() {
            return r#"{"ok":false,"error":"select refused"}"#.to_string();
        }
        r#"{"ok":true}"#.to_string()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_drop_instance(handle: u64) -> *mut c_char {
    guard("envelope", move || {
        HANDLES
            .lock()
            .expect("handles")
            .get_or_insert_with(HashMap::new)
            .remove(&handle);
        log(format!("drop {handle}"));
        r#"{"ok":true}"#.to_string()
    })
}

/// # Safety
/// `s` must be a pointer this library returned and has not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bddkit_free_string(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}
