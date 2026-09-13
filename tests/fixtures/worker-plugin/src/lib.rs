//! Fixture plugin for the host test suite: the `per_worker` counterpart of
//! `tests/fixtures/echo-plugin`. It claims the group "worker".
//!
//! A crate has exactly one `lib` target, so one fixture cannot declare both
//! concurrency modes and both have to be exercised. Everything here that looks
//! like boilerplate — the `catch_unwind` guard on every export, the
//! `free_string` discipline — is the same as the echo fixture's on purpose:
//! unwinding across an FFI boundary is undefined behaviour.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// A handle is an index into this table, never a pointer: a `u64` cannot be
/// dangled by the host and needs no `Send` reasoning on either side.
static INSTANCES: Mutex<Option<HashMap<u64, Instance>>> = Mutex::new(None);
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

struct Instance {
    /// The whole point of this fixture. The counter lives on the INSTANCE, not
    /// in a `static`: under `per_worker` two feature files get two instances,
    /// so two files counting to 1 both read back 1. A static would make that
    /// test pass for the wrong reason and pin nothing.
    counter: u64,
    /// A path to append a line to whenever this instance is dropped, so a host
    /// test can observe `bddkit_drop_instance` from outside the process
    /// without an env var (`std::env::set_var` is `unsafe` in edition 2024 and
    /// process-global, so it would race every other test).
    ///
    /// Appended, never truncated: under `per_worker` there is one drop per
    /// file, and a test counts the lines to tell that apart from one drop per
    /// run. A truncating write would leave exactly one line either way.
    drop_log: Option<String>,
}

/// Deliberately called OUTSIDE `guard`, and safe there only because it
/// provably cannot panic: `CString::new` returns a `Result`, and the fallback
/// literal has no interior NUL. Anything you add here must keep that property.
fn out(s: String) -> *mut c_char {
    CString::new(s)
        .unwrap_or_else(|_| {
            CString::new("{\"ok\":false,\"error\":\"NUL in reply\"}").expect("literal")
        })
        .into_raw()
}

/// Called INSIDE `guard` at every call site. `to_string_lossy` cannot panic
/// today, but the guard must not depend on that: replacing it with something
/// that rejects invalid UTF-8 by unwrapping is a natural-looking edit, and
/// outside the guard it would unwind straight across the FFI boundary.
fn input(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// A panic must never unwind across the FFI boundary — that is undefined
/// behaviour, and the host cannot install this guard on the plugin's behalf.
fn guard(envelope_kind: &str, body: impl FnOnce() -> String) -> *mut c_char {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(reply) => out(reply),
        Err(_) => out(match envelope_kind {
            "dispatch" => r#"{"status":"fatal","error":"the plugin panicked"}"#.to_string(),
            _ => r#"{"ok":false,"error":"the plugin panicked"}"#.to_string(),
        }),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_abi_version() -> u32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_manifest() -> *mut c_char {
    // Guarded even though a constant string cannot panic: "every export is
    // guarded" is an invariant a reader can check, "this one happens to be
    // safe" is a judgement each future edit has to make again.
    guard("envelope", || {
        // `implicit_instance`: a suite with no `resources.worker` section gets
        // one instance named `default` from this (empty) body — the
        // zero-config shape a plugin whose every key has a default is for.
        r#"{"name":"worker","version":"0.1.0","groups":["worker"],"concurrency":"per_worker","implicit_instance":{"worker":{}}}"#
            .to_string()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_list_steps() -> *mut c_char {
    guard("envelope", || {
        serde_json::json!([
            { "pattern": r#"^I count in the worker as "([^"]*)"$"#, "group": "worker", "kind": "action" },
            { "pattern": r#"^I record the worker instance in "([^"]*)"$"#, "group": "worker", "kind": "action" }
        ])
        .to_string()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_validate_config(_request: *const c_char) -> *mut c_char {
    // This instance requires nothing: `drop_log` is the only key it reads and
    // it is optional. The one refusal is a test switch for the case no config
    // can reach — the manifest's own `implicit_instance` body being rejected,
    // which is a plugin bug the host must report at load. The variable is only
    // ever READ here, and set by a host test on the child process it spawns
    // (`Command::env`), never with `std::env::set_var` in the test process.
    guard("envelope", || {
        if std::env::var_os("BDDKIT_WORKER_FIXTURE_REJECT_CONFIG").is_some() {
            return r#"{"ok":false,"error":"rejected by the fixture on request"}"#.to_string();
        }
        r#"{"ok":true}"#.to_string()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_init_instance(request: *const c_char) -> *mut c_char {
    guard("envelope", move || {
        let raw = input(request);
        let value: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => return serde_json::json!({"ok": false, "error": e.to_string()}).to_string(),
        };
        let drop_log = value["config"]["drop_log"].as_str().map(str::to_string);
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::SeqCst);
        let mut guard = INSTANCES.lock().expect("instances");
        guard
            .get_or_insert_with(HashMap::new)
            .insert(handle, Instance { counter: 0, drop_log });
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
        let raw = input(request);
        let value: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                return serde_json::json!({"status": "fatal", "error": e.to_string()}).to_string();
            }
        };
        let args: Vec<String> = value["args"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| v.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        let name = args.first().cloned().unwrap_or_default();

        let mut guard = INSTANCES.lock().expect("instances");
        let table = guard.get_or_insert_with(HashMap::new);
        let Some(instance) = table.get_mut(&handle) else {
            return serde_json::json!({"status": "fatal", "error": "unknown handle"}).to_string();
        };

        match step_index {
            0 => {
                instance.counter += 1;
                serde_json::json!({
                    "status": "passed",
                    "vars": {name: instance.counter.to_string()}
                })
                .to_string()
            }
            // The handle identifies the instance, so two files publishing
            // different values here is the proof that they got two instances.
            1 => serde_json::json!({
                "status": "passed",
                "vars": {name: handle.to_string()}
            })
            .to_string(),
            other => {
                serde_json::json!({"status": "fatal", "error": format!("unknown step {other}")})
                    .to_string()
            }
        }
    })
}

/// Exported on purpose: `per_worker` plus a per-scenario reset is exactly the
/// pairing the host used to refuse and now allows, because an instance that
/// belongs to one file can never be reset out from under another worker.
#[unsafe(no_mangle)]
pub extern "C" fn bddkit_reset_scenario(handle: u64) -> *mut c_char {
    guard("envelope", move || {
        let mut guard = INSTANCES.lock().expect("instances");
        if let Some(instance) = guard.get_or_insert_with(HashMap::new).get_mut(&handle) {
            instance.counter = 0;
        }
        r#"{"ok":true}"#.to_string()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_drop_instance(handle: u64) -> *mut c_char {
    guard("envelope", move || {
        let mut guard = INSTANCES.lock().expect("instances");
        if let Some(instance) = guard.get_or_insert_with(HashMap::new).remove(&handle)
            && let Some(path) = instance.drop_log
        {
            // Append, and on every drop: a test counts the lines to tell one
            // drop per file apart from one drop per run. `O_APPEND` makes a
            // short write atomic, so two workers dropping at once still leave
            // two whole lines.
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut file| writeln!(file, "dropped {handle}"));
        }
        r#"{"ok":true}"#.to_string()
    })
}

/// A string allocated by this crate's allocator must be freed by this crate's
/// allocator. The host calling `free` on it directly is undefined behaviour.
///
/// # Safety
/// `s` must be a pointer this library returned and has not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bddkit_free_string(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}
