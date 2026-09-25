//! Reference HTTP service for acceptance tests. Started in-process on a random
//! port: docker-compose arrives in M2 together with Postgres.

#![allow(dead_code)]

pub mod db;

use axum::extract::Query;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use num_bigint::BigUint;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::Barrier;

pub async fn spawn() -> String {
    let app = Router::new()
        .route("/ping", get(ping))
        .route("/echo", post(echo))
        .route("/users", get(users))
        .route("/login", post(login))
        .route("/headers", get(headers))
        .route("/xml", get(xml_doc))
        .route("/html", get(html_doc))
        .route("/html-loose", get(html_doc_loose))
        .route("/plain", get(plain_text))
        .route("/no-content", get(no_content))
        .route("/srp/register", post(srp_register))
        .route("/srp/step1", post(srp_step1))
        .route("/srp/step2", post(srp_step2))
        .route("/hawk", post(hawk));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    format!("http://{addr}/")
}

/// A second reference service. Answers the same `/ping`, but with a different
/// body: the API-switch test must see that the request landed here.
pub async fn spawn_secondary() -> String {
    let app = Router::new().route(
        "/ping",
        get(|| async { Json(json!({"status": "ok", "source": "secondary"})) }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    format!("http://{addr}/")
}

/// A stub with a barrier: `/barrier` does not respond until `n` requests are
/// in flight at once. A sequential runner hangs on it until timeout; a
/// parallel one passes — this proves parallelism without comparing run
/// times, i.e. without a flaky test.
pub async fn spawn_barrier(n: usize) -> String {
    let barrier = Arc::new(Barrier::new(n));
    let app = Router::new().route(
        "/barrier",
        get(move || {
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                Json(json!({"status": "ok"}))
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    format!("http://{addr}/")
}

async fn no_content() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn ping() -> Json<Value> {
    Json(json!({"status": "ok", "version": 3}))
}

async fn echo(body: String) -> impl IntoResponse {
    let parsed: Value = serde_json::from_str(&body).unwrap_or(json!({}));
    (
        StatusCode::CREATED,
        Json(json!({"received": parsed, "id": 42})),
    )
}

async fn users(Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    if q.get("email").is_some_and(|e| e == "a@b.net") {
        return Json(json!([{"id": 1, "email": "a@b.net"}]));
    }
    Json(json!([
        {"id": 1, "email": "a@b.net", "roles": ["ROLE_USER"]},
        {"id": 2, "email": "c@d.net", "roles": ["ROLE_ADMIN"]}
    ]))
}

async fn login(body: String) -> impl IntoResponse {
    let parsed: Value = serde_json::from_str(&body).unwrap_or(json!({}));
    if parsed.get("password").and_then(Value::as_str) == Some("correct") {
        let mut h = HeaderMap::new();
        h.insert(
            "set-cookie",
            "jwt_token=tok-abc; Path=/; HttpOnly"
                .parse()
                .expect("header"),
        );
        h.insert("x-trace", "trace-1".parse().expect("header"));
        return (
            StatusCode::OK,
            h,
            Json(json!({"success": true, "jwt": "tok-abc"})),
        );
    }
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        HeaderMap::new(),
        Json(json!({"success": false, "errorMessage": "Wrong password"})),
    )
}

async fn headers(h: HeaderMap) -> Json<Value> {
    let accept: Vec<String> = h
        .get_all("accept")
        .iter()
        .map(|v| v.to_str().unwrap_or("").to_string())
        .collect();
    Json(json!({"accept": accept}))
}

async fn xml_doc() -> impl IntoResponse {
    let mut h = HeaderMap::new();
    h.insert("content-type", "application/xml".parse().expect("header"));
    (
        StatusCode::OK,
        h,
        r#"<?xml version="1.0"?><users><user id="1"><email>a@b.net</email></user><user id="2"><email>c@d.net</email></user></users>"#,
    )
}

async fn html_doc() -> impl IntoResponse {
    let mut h = HeaderMap::new();
    h.insert("content-type", "text/html".parse().expect("header"));
    (
        StatusCode::OK,
        h,
        r#"<html><body><h1 id="title">Hello</h1><p class="msg">World</p></body></html>"#,
    )
}

/// Realistic tag soup — doctype, an unclosed void element, an unquoted
/// attribute — the exact shape that broke the old XPath-only debug path.
async fn html_doc_loose() -> impl IntoResponse {
    let mut h = HeaderMap::new();
    h.insert("content-type", "text/html".parse().expect("header"));
    (
        StatusCode::OK,
        h,
        "<!doctype html><html><body><img src=\"a.png\"><div class=box id=main>Loose</div></body></html>",
    )
}

async fn plain_text() -> impl IntoResponse {
    let mut h = HeaderMap::new();
    h.insert("content-type", "text/plain".parse().expect("header"));
    (StatusCode::OK, h, "hello")
}

/// Checks only the header's shape and the body — it does not verify the Hawk
/// MAC. Duplicating Hawk cryptography in the stub would test the stub against
/// itself; the real hashing rules are covered by `src/hawk.rs`'s own tests.
/// Fixed expectations (`session-1` id, this exact body) match the id/key/body
/// the acceptance feature sends.
async fn hawk(h: HeaderMap, body: String) -> impl IntoResponse {
    let auth = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);

    let well_formed = auth.starts_with(r#"Hawk id="session-1", ts=""#)
        && auth.contains("nonce=")
        && auth.contains("hash=")
        && auth.contains("mac=");
    if !well_formed || parsed != json!({"code": "555555"}) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "unexpected Hawk header or body", "authorization": auth})),
        );
    }
    (
        StatusCode::OK,
        Json(json!({"authorization": auth, "body": parsed})),
    )
}

/// RFC 5054 Appendix A, 1024-bit group. Deliberately small: this handshake runs
/// on every acceptance pass, and 4096-bit exponentiation would dominate the
/// suite's runtime. The hashing rules under test do not depend on group size.
const SRP_PRIME_HEX: &str = "\
EEAF0AB9ADB38DD69C33F80AFA8FC5E86072618775FF3C0B9EA2314C9C256576D674DF7496EA81D3383B4813D692C6E0\
E0D5D8E250B98BE48E495C1D6089DAD15DC7D7B46154D6B6CE8EF4AD69B15D4982559B297BCF1885C529F566660E57EC\
68EDBC3C05726CC02FD4CBF4976EAA9AFD5138FE8376435B9FC61D2FC0EB06E3";

/// Fixed server ephemeral, so step2 can recompute B without session state.
/// A test server gains nothing from being unpredictable.
const SRP_SERVER_PRIVATE_HEX: &str =
    "5c2e91a0d7b34f6812ae09d5c73b6e4f2a81d09c5e6b47a3f0982d1c6b5a4e30";

/// identity -> (salt, verifier)
static SRP_ACCOUNTS: LazyLock<Mutex<HashMap<String, (String, String)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn srp_prime() -> BigUint {
    BigUint::parse_bytes(SRP_PRIME_HEX.as_bytes(), 16).expect("constant prime parses")
}

fn srp_generator() -> BigUint {
    BigUint::from(2u32)
}

fn srp_hex(value: &BigUint) -> String {
    format!("{value:x}")
}

fn srp_hash(text: &str) -> BigUint {
    BigUint::from_bytes_be(&Sha256::digest(text.as_bytes()))
}

/// k = H(PAD(N) | PAD(g))
fn srp_k() -> BigUint {
    let n = srp_prime();
    let width = (n.bits() as usize).div_ceil(8);
    let pad = |value: &BigUint| {
        let bytes = value.to_bytes_be();
        let mut out = vec![0u8; width - bytes.len()];
        out.extend_from_slice(&bytes);
        out
    };
    let mut input = pad(&n);
    input.extend_from_slice(&pad(&srp_generator()));
    BigUint::from_bytes_be(&Sha256::digest(&input))
}

fn srp_b_pub(verifier_hex: &str) -> BigUint {
    let n = srp_prime();
    let b_priv =
        BigUint::parse_bytes(SRP_SERVER_PRIVATE_HEX.as_bytes(), 16).expect("constant parses");
    let v = BigUint::parse_bytes(verifier_hex.as_bytes(), 16).expect("stored verifier is hex");
    (srp_k() * v + srp_generator().modpow(&b_priv, &n)) % n
}

async fn srp_register(body: String) -> impl IntoResponse {
    let payload: Value = serde_json::from_str(&body).expect("registration body is JSON");
    let identity = payload["identity"].as_str().expect("identity").to_string();
    let salt = payload["salt"].as_str().expect("salt").to_string();
    let verifier = payload["verifier"].as_str().expect("verifier").to_string();
    SRP_ACCOUNTS
        .lock()
        .expect("store is not poisoned")
        .insert(identity, (salt, verifier));
    (StatusCode::OK, Json(json!({})))
}

async fn srp_step1(body: String) -> impl IntoResponse {
    let payload: Value = serde_json::from_str(&body).expect("step1 body is JSON");
    let identity = payload["identity"].as_str().expect("identity");
    let account = SRP_ACCOUNTS
        .lock()
        .expect("store is not poisoned")
        .get(identity)
        .cloned();
    match account {
        Some((salt, verifier)) => (
            StatusCode::OK,
            Json(json!({"salt": salt, "b": srp_hex(&srp_b_pub(&verifier))})),
        ),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "unknown identity"})),
        ),
    }
}

async fn srp_step2(body: String) -> impl IntoResponse {
    let payload: Value = serde_json::from_str(&body).expect("step2 body is JSON");
    let identity = payload["identity"].as_str().expect("identity");
    let a_pub_hex = payload["a"].as_str().expect("a");
    let m1_hex = payload["m1"].as_str().expect("m1");

    let account = SRP_ACCOUNTS
        .lock()
        .expect("store is not poisoned")
        .get(identity)
        .cloned();
    let Some((_, verifier)) = account else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "unknown identity"})),
        );
    };

    let n = srp_prime();
    let b_priv =
        BigUint::parse_bytes(SRP_SERVER_PRIVATE_HEX.as_bytes(), 16).expect("constant parses");
    let v = BigUint::parse_bytes(verifier.as_bytes(), 16).expect("stored verifier is hex");
    let a_pub = BigUint::parse_bytes(a_pub_hex.as_bytes(), 16).expect("client A is hex");
    let b_pub = srp_b_pub(&verifier);
    let b_pub_hex = srp_hex(&b_pub);

    // S = (A * v^u)^b mod N — the server's half of the shared secret.
    let u = srp_hash(&format!("{a_pub_hex}{b_pub_hex}"));
    let s = (&a_pub * v.modpow(&u, &n)).modpow(&b_priv, &n);
    let s_hex = srp_hex(&s);

    let expected_m1 = srp_hash(&format!("{a_pub_hex}{b_pub_hex}{s_hex}"));
    let received_m1 = BigUint::parse_bytes(m1_hex.as_bytes(), 16).expect("client M1 is hex");
    if expected_m1 != received_m1 {
        // Loud rejection: without it a broken client would compare two equally
        // wrong proofs and the scenario would pass.
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "bad client proof"})),
        );
    }

    // M1 enters M2 zero-padded to the digest width, and M2 goes on the wire in
    // the same padded form.
    let m2 = srp_hash(&format!("{a_pub_hex}{expected_m1:064x}{s_hex}"));
    (StatusCode::OK, Json(json!({"m2": format!("{m2:064x}")})))
}

/// Builds the fixture plugin and returns the path to its shared library.
/// Cargo caches it, so only the first test in a run pays for the build.
pub fn build_fixture_plugin() -> std::path::PathBuf {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = root.join("target/fixture-plugin");
    let out = std::process::Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(root.join("tests/fixtures/echo-plugin/Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .output()
        .expect("failed to run cargo for the fixture plugin");
    assert!(
        out.status.success(),
        "fixture plugin build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let path = target.join("debug").join(format!(
        "{}echo_plugin{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    assert!(
        path.exists(),
        "fixture plugin artifact missing at {}",
        path.display()
    );
    path
}

/// Builds the `per_worker` fixture and returns the path to its shared library.
/// A separate crate because a crate has one `lib` target and both concurrency
/// modes have to be exercised.
pub fn build_worker_plugin() -> std::path::PathBuf {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = root.join("target/worker-plugin");
    let out = std::process::Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(root.join("tests/fixtures/worker-plugin/Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .output()
        .expect("failed to run cargo for the worker plugin");
    assert!(
        out.status.success(),
        "worker plugin build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let path = target.join("debug").join(format!(
        "{}worker_plugin{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    assert!(
        path.exists(),
        "worker plugin artifact missing at {}",
        path.display()
    );
    path
}
