//! Fixture plugin for the host test suite, and the worked example for
//! docs/plugin-authoring.md. It claims the group "echo".

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// A handle is an index into this table, never a pointer: a `u64` cannot be
/// dangled by the host and needs no `Send` reasoning on either side.
static INSTANCES: Mutex<Option<HashMap<u64, Instance>>> = Mutex::new(None);
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

struct Instance {
    prefix: String,
    attempts: u64,
    /// Instance-config-driven, not an env var: `std::env::set_var` is `unsafe`
    /// in edition 2024 and process-global, so it would race every other test.
    fail_reset: bool,
    /// Same reasoning: a path to write a marker file to when this instance is
    /// dropped, so a host test can observe `bddkit_drop_instance` from outside
    /// the process without an env var.
    drop_log: Option<String>,
}

/// Deliberately called OUTSIDE `guard`, and safe there only because it
/// provably cannot panic: `CString::new` returns a `Result`, and the fallback
/// literal has no interior NUL. Anything you add here must keep that property.
fn out(s: String) -> *mut c_char {
    CString::new(s)
        .unwrap_or_else(|_| CString::new("{\"ok\":false,\"error\":\"NUL in reply\"}").expect("literal"))
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
        serde_json::json!({
            "name": "echo", "version": "0.1.0", "groups": ["echo"], "concurrency": "shared",
            // What a suite with no `resources.echo` section gets: one instance
            // named `default`, built from this body. Every key an instance
            // needs must be here — the host validates it at load like any
            // declared instance, and a body this plugin's own
            // `bddkit_validate_config` refuses fails the load.
            "implicit_instance": {"echo": {"prefix": "implicit-"}},
            "fields": {"echo": [
                {"name": "prefix", "required": true, "type": "string",
                 "description": "prepended to every echoed value", "example": "p-"},
                {"name": "loud", "type": "boolean",
                 "description": "upper-cases what is echoed", "example": "true"},
                {"name": "probe_error",
                 "description": "message bddkit_probe_config answers with; omit to probe clean"}
            ]}
        })
        .to_string()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_list_steps() -> *mut c_char {
    guard("envelope", || {
        serde_json::json!([
            { "pattern": r#"^I echo "(?P<value>[^"]*)" as "(?P<variable>[^"]*)"$"#, "group": "echo", "kind": "action",
              "description": "echoes its argument back into a variable, prefixed by the instance config" },
            { "pattern": r#"^the echo counter should reach (?P<count>\d+)$"#, "group": "echo", "kind": "assertion",
              "description": "asserts the instance has served this many steps" },
            { "pattern": r#"^the echo should fail$"#, "group": "echo", "kind": "assertion" }
        ])
        .to_string()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_validate_config(request: *const c_char) -> *mut c_char {
    guard("envelope", move || {
        let raw = input(request);
        let value: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => return serde_json::json!({"ok": false, "error": e.to_string()}).to_string(),
        };
        if value["config"]["prefix"].as_str().is_none() {
            return r#"{"ok":false,"error":"echo instance requires a string \"prefix\""}"#
                .to_string();
        }
        // The half a manifest's `type` exists for: this key is a boolean, and a
        // plugin holding that line is what turns the host writing `'false'`
        // into a refusal the author of the config can do nothing about. The
        // manifest declares `"type": "boolean"` so the host writes `false`.
        match &value["config"]["loud"] {
            serde_json::Value::Null | serde_json::Value::Bool(_) => r#"{"ok":true}"#.to_string(),
            _ => r#"{"ok":false,"error":"echo instance's \"loud\" must be true or false"}"#
                .to_string(),
        }
    })
}

/// Optional: the live half of the config contract. `validate_config` above is
/// pure and offline because the host runs it eagerly for every declared
/// instance; this one is allowed to be slow and to open a connection, and runs
/// only when somebody asks. Echo has nothing to reach, so the failure branch
/// is config-driven — a real plugin puts its client here.
#[unsafe(no_mangle)]
pub extern "C" fn bddkit_probe_config(request: *const c_char) -> *mut c_char {
    guard("envelope", move || {
        let raw = input(request);
        let value: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => return serde_json::json!({"ok": false, "error": e.to_string()}).to_string(),
        };
        match value["config"]["probe_error"].as_str() {
            Some(error) => serde_json::json!({"ok": false, "error": error}).to_string(),
            None => r#"{"ok":true}"#.to_string(),
        }
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
        let Some(prefix) = value["config"]["prefix"].as_str() else {
            return r#"{"ok":false,"error":"echo instance requires a string \"prefix\""}"#.to_string();
        };
        let fail_reset = value["config"]["fail_reset"].as_bool().unwrap_or(false);
        let drop_log = value["config"]["drop_log"].as_str().map(str::to_string);
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::SeqCst);
        let mut guard = INSTANCES.lock().expect("instances");
        guard.get_or_insert_with(HashMap::new).insert(
            handle,
            Instance { prefix: prefix.to_string(), attempts: 0, fail_reset, drop_log },
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
        let raw = input(request);
        let value: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                return serde_json::json!({"status": "fatal", "error": e.to_string()}).to_string();
            }
        };
        let args: Vec<String> = value["args"]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_str().unwrap_or_default().to_string()).collect())
            .unwrap_or_default();

        let mut guard = INSTANCES.lock().expect("instances");
        let table = guard.get_or_insert_with(HashMap::new);
        let Some(instance) = table.get_mut(&handle) else {
            return serde_json::json!({"status": "fatal", "error": "unknown handle"}).to_string();
        };

        match step_index {
            0 => {
                let name = args.get(1).cloned().unwrap_or_default();
                let value = format!("{}{}", instance.prefix, args.first().cloned().unwrap_or_default());
                serde_json::json!({"status": "passed", "vars": {name: value}}).to_string()
            }
            1 => {
                instance.attempts += 1;
                let target: u64 = args.first().and_then(|a| a.parse().ok()).unwrap_or(0);
                if instance.attempts >= target {
                    // A variable published from an assertion is only kept when
                    // the assertion finally passes.
                    serde_json::json!({
                        "status": "passed",
                        "vars": {"echo_attempts": instance.attempts.to_string()}
                    })
                    .to_string()
                } else {
                    serde_json::json!({
                        "status": "not_yet",
                        "vars": {"echo_attempts": "leaked"},
                        "error": format!("counter is {} of {target}", instance.attempts)
                    })
                    .to_string()
                }
            }
            2 => serde_json::json!({
                "status": "fatal",
                "error": "the echo step was asked to fail",
                "diagnostics": [
                    {"title": "echo state", "kind": "text",
                     "content": format!("prefix={} attempts={}", instance.prefix, instance.attempts)}
                ]
            })
            .to_string(),
            other => {
                serde_json::json!({"status": "fatal", "error": format!("unknown step {other}")})
                    .to_string()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn bddkit_reset_scenario(handle: u64) -> *mut c_char {
    guard("envelope", move || {
        let mut guard = INSTANCES.lock().expect("instances");
        let Some(instance) = guard.get_or_insert_with(HashMap::new).get_mut(&handle) else {
            return r#"{"ok":true}"#.to_string();
        };
        if instance.fail_reset {
            return r#"{"ok":false,"error":"the echo instance refuses to reset"}"#.to_string();
        }
        instance.attempts = 0;
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
            let _ = std::fs::write(path, "dropped");
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
