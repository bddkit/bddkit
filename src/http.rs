use crate::hawk;
use crate::options::Options;
use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct Exchange {
    pub method: String,
    pub url: String,
    pub req_headers: Vec<(String, String)>,
    pub req_body: Option<String>,
    pub status: u16,
    pub resp_headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Clone)]
struct RequestRecipe {
    api: String,
    method: String,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    body: Option<String>,
    form: Option<Vec<(String, String)>>,
    signer: Option<hawk::Credentials>,
}

#[derive(Debug)]
pub enum ReplayError {
    NotYet(String),
    Fatal(String),
}

impl ReplayError {
    fn into_message(self) -> String {
        match self {
            Self::NotYet(message) | Self::Fatal(message) => message,
        }
    }
}

impl Exchange {
    pub fn json(&self) -> Result<Value, String> {
        serde_json::from_str(&self.body)
            .map_err(|e| format!("response body is not valid JSON: {e}"))
    }

    /// Returns the cookie value from the `Set-Cookie` headers.
    pub fn set_cookie(&self, name: &str) -> Option<String> {
        for (h, v) in &self.resp_headers {
            if !h.eq_ignore_ascii_case("set-cookie") {
                continue;
            }
            let pair = v.split(';').next().unwrap_or("");
            if let Some((k, val)) = pair.split_once('=')
                && k.trim() == name
            {
                return Some(val.trim().to_string());
            }
        }
        None
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}

impl std::fmt::Display for Exchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "  {} {}", self.method, self.url)?;
        for (k, v) in &self.req_headers {
            writeln!(f, "    {k}: {v}")?;
        }
        if let Some(b) = &self.req_body {
            writeln!(f, "    {}", truncate(b, 600))?;
        }
        writeln!(f, "  ← {}", self.status)?;
        write!(f, "    {}", truncate(&self.body, 600))
    }
}

/// One API resource: its own client (and therefore its own timeout and
/// connection pool), a base address, and the headers every scenario starts with.
pub struct ApiResource {
    client: reqwest::Client,
    base_url: url::Url,
    default_headers: Vec<(String, String)>,
    options: Options,
}

impl ApiResource {
    pub fn new(
        base_url: &str,
        timeout_secs: u64,
        default_headers: Vec<(String, String)>,
        options: Options,
    ) -> Result<Self> {
        // `reqwest` accepts a zero timeout and then fails every request
        // instantly, which reads as an unreachable service rather than as the
        // config error it is.
        if timeout_secs == 0 {
            anyhow::bail!("timeout_secs is 0: every request would time out immediately");
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(timeout_secs))
                .build()
                .context("failed to create HTTP client")?,
            base_url: url::Url::parse(base_url)
                .with_context(|| format!("invalid base_url {base_url:?}"))?,
            default_headers,
            options,
        })
    }

    /// One GET at `base_url` over this resource's own client, so the probe
    /// carries the timeout the suite configured. Any answer means reachable —
    /// the status is reported, not judged, because `doctor` checks that the
    /// resource is there, not that the application routes its own root.
    pub async fn probe(&self) -> Result<u16, String> {
        self.client
            .get(self.base_url.clone())
            .send()
            .await
            .map(|response| response.status().as_u16())
            .map_err(|error| error.to_string())
    }
}

/// All of the run's API resources. Built once and shared via `Arc`: the
/// client holds a connection pool, and recreating it per scenario would lose the pool.
pub struct Apis {
    by_name: HashMap<String, ApiResource>,
    default: String,
}

impl Apis {
    /// `default: None` means "no APIs declared at all" (see `Config::resolve_default_api`) —
    /// legal for a DB-only config. An explicit `Some(default)` not named among
    /// the resources is an error: the rest of the code relies on the default
    /// being resolvable and shouldn't have to recheck it.
    pub fn new(by_name: HashMap<String, ApiResource>, default: Option<String>) -> Result<Self> {
        let default = match default {
            Some(d) if by_name.contains_key(&d) => d,
            Some(d) => anyhow::bail!("default API resource {d:?} is not declared in resources.api"),
            None => String::new(),
        };
        Ok(Self { by_name, default })
    }

    pub fn get(&self, name: &str) -> Result<&ApiResource, String> {
        self.by_name.get(name).ok_or_else(|| {
            if self.by_name.is_empty() {
                "no API resource is declared in the config (resources.api), \
                 but the scenario reaches for an API"
                    .to_string()
            } else {
                format!("API resource {name:?} is not declared in resources.api")
            }
        })
    }

    pub fn options(&self, name: &str) -> Result<&Options, String> {
        Ok(&self.get(name)?.options)
    }

    pub fn default_name(&self) -> &str {
        &self.default
    }

    fn default_headers(&self) -> Vec<(String, String)> {
        self.by_name
            .get(&self.default)
            .map(|r| r.default_headers.clone())
            .unwrap_or_default()
    }
}

pub struct HttpState {
    apis: Arc<Apis>,
    current: String,
    headers: Vec<(String, String)>,
    query: Vec<(String, String)>,
    body: Option<String>,
    form: Option<Vec<(String, String)>>,
    last: Option<Exchange>,
    replay: Option<RequestRecipe>,
    /// Hawk credentials for the next initial send. A successful request keeps
    /// them in its replay recipe; API switches and scenario resets clear only
    /// credentials that have not been sent yet.
    signer: Option<hawk::Credentials>,
}

impl HttpState {
    pub fn new(apis: Arc<Apis>) -> Self {
        let mut state = Self {
            apis,
            current: String::new(),
            headers: Vec::new(),
            query: Vec::new(),
            body: None,
            form: None,
            last: None,
            replay: None,
            signer: None,
        };
        state.reset();
        state
    }

    /// Restore the default request builder at a scenario boundary and forget
    /// both the previous exchange and its replay recipe.
    pub fn reset(&mut self) {
        let default = self.apis.default_name().to_string();
        let headers = self.apis.default_headers();
        self.switch_to(default, headers);
        self.last = None;
        self.replay = None;
    }

    /// Switch the pending request builder to another API. The previous exchange
    /// and recipe stay attached to their originating API so an assertion can
    /// still inspect or replay them after the switch.
    pub fn use_api(&mut self, name: &str) -> Result<(), String> {
        let headers = self.apis.get(name)?.default_headers.clone();
        self.switch_to(name.to_string(), headers);
        Ok(())
    }

    fn switch_to(&mut self, name: String, headers: Vec<(String, String)>) {
        self.current = name;
        self.headers = headers;
        self.query.clear();
        self.body = None;
        self.form = None;
        self.signer = None;
    }

    /// Sign the next distinct `send()` with Hawk. Automatic replays retain the
    /// credentials and sign afresh; a later explicit send needs another call.
    pub fn sign_next(&mut self, id: &str, key: &str) {
        self.signer = Some(hawk::Credentials {
            id: id.to_string(),
            key: key.to_string(),
        });
    }

    #[allow(dead_code)] // only read by tests, to assert the api switch
    pub fn current(&self) -> &str {
        &self.current
    }

    /// Replace: removes all values with this name, then sets one.
    pub fn set_header(&mut self, name: &str, value: &str) {
        self.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
        self.headers.push((name.to_string(), value.to_string()));
    }

    /// Appends to the header's existing values.
    pub fn add_header(&mut self, name: &str, value: &str) {
        self.headers.push((name.to_string(), value.to_string()));
    }

    pub fn set_query(&mut self, name: &str, value: &str) {
        self.query.retain(|(k, _)| k != name);
        self.query.push((name.to_string(), value.to_string()));
    }

    pub fn set_body(&mut self, body: String) {
        self.body = Some(body);
        self.form = None;
    }

    pub fn clear_body(&mut self) {
        self.body = None;
        self.form = None;
    }

    pub fn set_form(&mut self, pairs: Vec<(String, String)>) {
        self.form = Some(pairs);
        self.body = None;
    }

    pub fn last(&self) -> Option<&Exchange> {
        self.last.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn store_test_exchange(&mut self, ex: Exchange) {
        self.last = Some(ex);
    }

    pub fn options_for_last_response(&self) -> Result<&Options, String> {
        let recipe = self.replay.as_ref().ok_or("request has not been sent")?;
        self.apis.options(&recipe.api)
    }

    pub async fn send(&mut self, path: &str, method: &str) -> Result<(), String> {
        // Clear both before constructing the new request so no failure can
        // expose an exchange or recipe belonging to an earlier explicit send.
        self.last = None;
        self.replay = None;
        let recipe = RequestRecipe {
            api: self.current.clone(),
            method: method.to_string(),
            path: path.to_string(),
            query: self.query.clone(),
            headers: self.headers.clone(),
            body: self.body.clone(),
            form: self.form.clone(),
            signer: self.signer.take(),
        };
        let exchange = self
            .execute(&recipe)
            .await
            .map_err(ReplayError::into_message)?;
        self.last = Some(exchange);
        self.replay = Some(recipe);
        Ok(())
    }

    pub async fn replay_last(&mut self) -> Result<(), ReplayError> {
        self.last = None;
        let recipe = self
            .replay
            .clone()
            .ok_or_else(|| ReplayError::Fatal("request has not been sent".to_string()))?;
        let exchange = self.execute(&recipe).await?;
        self.last = Some(exchange);
        Ok(())
    }

    async fn execute(&self, recipe: &RequestRecipe) -> Result<Exchange, ReplayError> {
        let api = self.apis.get(&recipe.api).map_err(ReplayError::Fatal)?;
        let mut url = api
            .base_url
            .join(&recipe.path)
            .map_err(|e| ReplayError::Fatal(format!("invalid path {:?}: {e}", recipe.path)))?;
        if !recipe.query.is_empty() {
            let mut qp = url.query_pairs_mut();
            for (k, v) in &recipe.query {
                qp.append_pair(k, v);
            }
        }
        let method = reqwest::Method::from_bytes(recipe.method.as_bytes()).map_err(|e| {
            ReplayError::Fatal(format!("invalid HTTP method {:?}: {e}", recipe.method))
        })?;

        // Built before headers: a pending Hawk signer needs the exact bytes
        // being sent to hash the payload.
        let sent_body = if let Some(form) = &recipe.form {
            Some(
                form.iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("&"),
            )
        } else {
            recipe.body.clone()
        };

        let mut request_headers = recipe.headers.clone();
        if let Some(credentials) = &recipe.signer {
            if recipe.form.is_some() {
                return Err(ReplayError::Fatal(
                    "Hawk signing supports raw request bodies only, not form bodies".to_string(),
                ));
            }
            let nonce = generate_nonce().map_err(ReplayError::Fatal)?;
            let timestamp = unix_timestamp().map_err(ReplayError::Fatal)?;
            request_headers = apply_signer(
                request_headers,
                credentials,
                &url,
                &recipe.method,
                sent_body.as_deref().unwrap_or(""),
                timestamp,
                &nonce,
            )
            .map_err(ReplayError::Fatal)?;
        }

        let mut req = api.client.request(method.clone(), url.clone());
        for (k, v) in &request_headers {
            req = req.header(k, v);
        }
        if let Some(form) = &recipe.form {
            req = req.form(form);
        } else if let Some(b) = &recipe.body {
            req = req.body(b.clone());
        }

        let request = req
            .build()
            .map_err(|e| ReplayError::Fatal(format!("failed to build HTTP request: {e}")))?;
        let resp = api
            .client
            .execute(request)
            .await
            .map_err(|e| ReplayError::NotYet(format!("request failed: {e}")))?;
        let status = resp.status().as_u16();
        let resp_headers = resp
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    v.to_str().unwrap_or("<non-UTF8>").to_string(),
                )
            })
            .collect();
        let body = resp
            .text()
            .await
            .map_err(|e| ReplayError::NotYet(format!("failed to read response body: {e}")))?;

        Ok(Exchange {
            method: method.to_string(),
            url: url.to_string(),
            req_headers: request_headers,
            req_body: sent_body,
            status,
            resp_headers,
            body,
        })
    }
}

/// 16 random bytes, Base64-encoded, as a fresh Hawk nonce.
fn generate_nonce() -> Result<String, String> {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).map_err(|_| "failed to read system entropy".to_string())?;
    Ok(STANDARD.encode(buf))
}

/// Current Unix time in seconds, for the Hawk `ts` field.
fn unix_timestamp() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| "system clock is set before the Unix epoch".to_string())
}

/// Signs a cloned header vector with Hawk: rejects an ambiguous multi-valued
/// `Content-Type`, then replaces any existing `Authorization` header
/// (case-insensitively) with the generated one. Takes `headers` by value so
/// the caller's own vector is never touched — only ever a clone is signed.
fn apply_signer(
    mut headers: Vec<(String, String)>,
    credentials: &hawk::Credentials,
    url: &url::Url,
    method: &str,
    body: &str,
    timestamp: u64,
    nonce: &str,
) -> Result<Vec<(String, String)>, String> {
    let content_types: Vec<&str> = headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str())
        .collect();
    if content_types.len() > 1 {
        return Err(
            "multiple Content-Type headers: Hawk signing needs one unambiguous value".to_string(),
        );
    }
    let content_type = content_types.first().copied();

    let header = hawk::authorization(
        url,
        method,
        body,
        content_type,
        credentials,
        timestamp,
        nonce,
    )?;
    headers.retain(|(k, _)| !k.eq_ignore_ascii_case("authorization"));
    headers.push(("Authorization".to_string(), header));
    Ok(headers)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use axum::extract::OriginalUri;
    use axum::http::{HeaderMap, Method};
    use axum::routing::{any, post};
    use axum::{Json, Router};
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[test]
    fn a_zero_timeout_is_refused_when_the_resource_is_built() {
        // `reqwest` accepts a zero timeout and then times out every request
        // instantly. Refusing it here rather than in `doctor` is what keeps the
        // two from disagreeing about whether a config is valid.
        let error = match ApiResource::new("http://x", 0, Vec::new(), Options::default()) {
            Ok(_) => panic!("a zero timeout cannot serve a request"),
            Err(error) => format!("{error:#}"),
        };
        assert!(error.contains("timeout_secs"), "{error}");
    }

    fn apis_with(name: &str, base: &str, default_headers: Vec<(String, String)>) -> Arc<Apis> {
        let mut by_name = HashMap::new();
        by_name.insert(
            name.to_string(),
            ApiResource::new(base, 5, default_headers, Options::default()).expect("valid base_url"),
        );
        Arc::new(Apis::new(by_name, Some(name.to_string())).expect("default declared"))
    }

    fn state() -> HttpState {
        HttpState::new(apis_with("main", "http://localhost:1/", Vec::new()))
    }

    fn two_apis() -> Arc<Apis> {
        let mut by_name = HashMap::new();
        by_name.insert(
            "first".to_string(),
            ApiResource::new(
                "http://first.local/",
                5,
                vec![("x-source".to_string(), "first".to_string())],
                Options::default(),
            )
            .expect("valid base_url"),
        );
        by_name.insert(
            "second".to_string(),
            ApiResource::new(
                "http://second.local/",
                5,
                vec![("x-source".to_string(), "second".to_string())],
                Options {
                    polling: crate::options::PollingOptions {
                        timeout: Duration::from_secs(8),
                        interval: Duration::from_millis(100),
                    },
                },
            )
            .expect("valid base_url"),
        );
        Arc::new(Apis::new(by_name, Some("first".to_string())).expect("default declared"))
    }

    fn local_apis(first: &str, second: &str) -> Arc<Apis> {
        let mut by_name = HashMap::new();
        by_name.insert(
            "first".to_string(),
            ApiResource::new(first, 5, Vec::new(), Options::default()).expect("valid first URL"),
        );
        by_name.insert(
            "second".to_string(),
            ApiResource::new(second, 5, Vec::new(), Options::default()).expect("valid second URL"),
        );
        Arc::new(Apis::new(by_name, Some("first".to_string())).expect("default is declared"))
    }

    pub(crate) async fn spawn_app(app: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test app");
        });
        (format!("http://{address}/"), server)
    }

    async fn spawn_echo_app(server: &'static str) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new().route(
            "/saved",
            any(
                move |method: Method,
                      OriginalUri(uri): OriginalUri,
                      headers: HeaderMap,
                      body: String| async move {
                    Json(json!({
                        "server": server,
                        "method": method.as_str(),
                        "uri": uri.to_string(),
                        "header": headers
                            .get("x-recipe")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or(""),
                        "body": body,
                    }))
                },
            ),
        );
        spawn_app(app).await
    }

    #[tokio::test]
    async fn probing_a_served_base_url_reports_the_status_it_answered_with() {
        // Any answer at all means reachable. The status is reported, never
        // judged: `doctor` checks that the resource is there, not that the
        // application routes its own root.
        let (base, _server) = spawn_app(Router::new()).await;
        let api = ApiResource::new(&base, 5, Vec::new(), Options::default()).expect("valid base");
        let status = api.probe().await.expect("the stub answers");
        assert_eq!(status, 404, "an empty router answers 404");
    }

    #[tokio::test]
    async fn probing_a_closed_port_reports_the_transport_failure() {
        let api = ApiResource::new("http://127.0.0.1:1/", 5, Vec::new(), Options::default())
            .expect("valid base");
        let error = api.probe().await.expect_err("port 1 is closed");
        assert!(!error.is_empty(), "the failure must say something");
    }

    #[tokio::test]
    async fn replay_preserves_the_original_api_method_path_query_headers_and_raw_body() {
        let (first, _first_server) = spawn_echo_app("first").await;
        let (second, _second_server) = spawn_echo_app("second").await;
        let mut state = HttpState::new(local_apis(&first, &second));
        state.set_query("phase", "initial");
        state.set_header("x-recipe", "original");
        state.set_body(r#"{"request":"saved"}"#.to_string());
        state
            .send("/saved", "PATCH")
            .await
            .expect("initial request succeeds");

        state.use_api("second").expect("second API exists");
        state.set_query("phase", "mutated");
        state.set_header("x-recipe", "mutated");
        state.set_body("mutated".to_string());
        state.replay_last().await.expect("replay succeeds");

        let exchange = state.last().expect("replay stores an exchange");
        assert_eq!(
            (
                exchange.method.as_str(),
                exchange.json().expect("stub response is JSON"),
            ),
            (
                "PATCH",
                json!({
                    "server": "first",
                    "method": "PATCH",
                    "uri": "/saved?phase=initial",
                    "header": "original",
                    "body": r#"{"request":"saved"}"#,
                }),
            )
        );
    }

    #[tokio::test]
    async fn replay_preserves_form_parameters() {
        let (base, _server) = spawn_echo_app("form").await;
        let mut state = HttpState::new(apis_with("main", &base, Vec::new()));
        state.set_form(vec![
            ("name".to_string(), "Jane Doe".to_string()),
            ("role".to_string(), "admin/owner".to_string()),
        ]);
        state
            .send("/saved", "POST")
            .await
            .expect("initial form request succeeds");
        state.set_body("mutated".to_string());

        state.replay_last().await.expect("replay succeeds");

        assert_eq!(
            state
                .last()
                .expect("replay stores an exchange")
                .json()
                .expect("stub response is JSON")["body"],
            "name=Jane+Doe&role=admin%2Fowner"
        );
    }

    #[tokio::test]
    async fn replay_generates_fresh_hawk_authorization() {
        let authorizations = Arc::new(Mutex::new(Vec::new()));
        let captured = authorizations.clone();
        let app = Router::new().route(
            "/hawk",
            post(move |headers: HeaderMap| {
                let captured = captured.clone();
                async move {
                    captured.lock().expect("capture lock").push(
                        headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("")
                            .to_string(),
                    );
                    Json(json!({"ok": true}))
                }
            }),
        );
        let (base, _server) = spawn_app(app).await;
        let mut state = HttpState::new(apis_with("main", &base, Vec::new()));
        state.set_header("content-type", "application/json");
        state.set_body(r#"{"request":"saved"}"#.to_string());
        state.sign_next("session", "secret");
        state
            .send("/hawk", "POST")
            .await
            .expect("initial signed request succeeds");

        state.replay_last().await.expect("replay succeeds");

        let authorizations = authorizations.lock().expect("capture lock");
        assert_eq!(authorizations.len(), 2);
        assert!(authorizations.iter().all(|value| !value.is_empty()));
        assert_ne!(authorizations[0], authorizations[1]);
    }

    #[tokio::test]
    async fn replay_transport_failure_clears_the_stale_exchange() {
        let (base, server) = spawn_echo_app("temporary").await;
        let mut state = HttpState::new(apis_with("main", &base, Vec::new()));
        state
            .send("/saved", "GET")
            .await
            .expect("initial request succeeds");
        server.abort();
        let _ = server.await;

        let error = state.replay_last().await.expect_err("replay must fail");

        assert!(matches!(error, ReplayError::NotYet(_)));
        assert!(state.last().is_none());
    }

    #[tokio::test]
    async fn replay_request_construction_errors_are_fatal() {
        let state = state();
        let recipe = RequestRecipe {
            api: "main".to_string(),
            method: "GET".to_string(),
            path: "/saved".to_string(),
            query: Vec::new(),
            headers: vec![("bad\nheader".to_string(), "value".to_string())],
            body: None,
            form: None,
            signer: None,
        };

        let error = state
            .execute(&recipe)
            .await
            .expect_err("invalid headers must fail request construction");

        assert!(matches!(error, ReplayError::Fatal(_)), "{error:?}");
    }

    #[test]
    fn use_api_switches_the_current_resource() {
        let mut s = HttpState::new(two_apis());
        s.use_api("second").expect("resource is declared");
        assert_eq!(s.current(), "second");
    }

    #[test]
    fn use_api_replaces_headers_with_the_new_resource_defaults() {
        let mut s = HttpState::new(two_apis());
        s.set_header("authorization", "Bearer first-host");
        s.use_api("second").expect("resource is declared");
        assert_eq!(
            s.headers,
            vec![("x-source".to_string(), "second".to_string())]
        );
    }

    #[test]
    fn use_api_clears_the_pending_body() {
        let mut s = HttpState::new(two_apis());
        s.set_body("{\"a\":1}".to_string());
        s.use_api("second").expect("resource is declared");
        assert!(s.body.is_none());
    }

    #[test]
    fn use_api_clears_the_pending_query() {
        let mut s = HttpState::new(two_apis());
        s.set_query("page", "2");
        s.use_api("second").expect("resource is declared");
        assert!(s.query.is_empty());
    }

    #[test]
    fn use_api_keeps_the_last_exchange() {
        let mut s = HttpState::new(two_apis());
        s.last = Some(Exchange {
            method: "GET".into(),
            url: "http://first.local/x".into(),
            req_headers: vec![],
            req_body: None,
            status: 200,
            resp_headers: vec![],
            body: "{}".into(),
        });
        s.use_api("second").expect("resource is declared");
        assert!(
            s.last().is_some(),
            "the previous response must survive an API switch"
        );
    }

    #[tokio::test]
    async fn last_response_options_stay_with_its_originating_api() {
        let (first, _first_server) = spawn_echo_app("first").await;
        let (second, _second_server) = spawn_echo_app("second").await;
        let mut apis = HashMap::new();
        apis.insert(
            "first".to_string(),
            ApiResource::new(&first, 5, Vec::new(), Options::default()).expect("valid first URL"),
        );
        apis.insert(
            "second".to_string(),
            ApiResource::new(
                &second,
                5,
                Vec::new(),
                Options {
                    polling: crate::options::PollingOptions {
                        timeout: Duration::from_secs(8),
                        interval: Duration::from_millis(100),
                    },
                },
            )
            .expect("valid second URL"),
        );
        let mut s = HttpState::new(Arc::new(
            Apis::new(apis, Some("first".to_string())).expect("default is declared"),
        ));
        s.send("/saved", "GET")
            .await
            .expect("initial request succeeds");
        s.use_api("second").expect("resource is declared");
        assert_eq!(
            s.options_for_last_response()
                .expect("last response has an API")
                .polling
                .timeout,
            Duration::from_secs(5)
        );
    }

    #[test]
    fn use_api_with_an_undeclared_name_is_an_error() {
        let mut s = HttpState::new(two_apis());
        assert!(s.use_api("third").is_err());
    }

    #[test]
    fn reset_returns_to_the_default_resource() {
        let mut s = HttpState::new(two_apis());
        s.use_api("second").expect("resource is declared");
        s.reset();
        assert_eq!(s.current(), "first");
    }

    #[test]
    fn signing_is_cleared_when_an_api_is_switched() {
        let mut state = HttpState::new(two_apis());
        state.sign_next("id", "key");
        state.use_api("second").unwrap();
        assert!(state.signer.is_none());
    }

    #[test]
    fn signing_is_cleared_by_scenario_reset() {
        let mut state = state();
        state.sign_next("id", "key");
        state.reset();
        assert!(state.signer.is_none());
    }

    #[tokio::test]
    async fn a_signed_form_request_fails_before_transport() {
        let mut state = state();
        state.set_form(vec![("name".into(), "value".into())]);
        state.sign_next("id", "key");
        assert!(state.send("/x", "POST").await.unwrap_err().contains("form"));
    }

    #[test]
    fn signer_replaces_only_the_sent_authorization_header() {
        let mut s = state();
        s.set_header("authorization", "Bearer stale");
        let credentials = hawk::Credentials {
            id: "id".to_string(),
            key: "key".to_string(),
        };
        let url = url::Url::parse("http://localhost/x").unwrap();

        let signed = apply_signer(
            s.headers.clone(),
            &credentials,
            &url,
            "GET",
            "",
            1,
            "fixed-nonce",
        )
        .expect("signing succeeds");

        assert!(
            !signed
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v == "Bearer stale"),
            "stale Authorization must be replaced, not kept alongside the new one"
        );
        assert_eq!(
            signed
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                .count(),
            1
        );
        assert!(
            signed
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v.starts_with("Hawk "))
        );
        // The state's own header vector must be untouched — apply_signer only
        // ever sees a clone.
        assert_eq!(
            s.headers,
            vec![("authorization".to_string(), "Bearer stale".to_string())]
        );
    }

    #[test]
    fn api_resource_rejects_an_invalid_base_url() {
        assert!(ApiResource::new("not-a-url", 5, Vec::new(), Options::default()).is_err());
    }

    #[test]
    fn apis_reject_a_default_naming_an_undeclared_resource() {
        let mut by_name = HashMap::new();
        by_name.insert(
            "a".to_string(),
            ApiResource::new("http://a.local/", 5, Vec::new(), Options::default())
                .expect("valid base_url"),
        );
        assert!(Apis::new(by_name, Some("b".to_string())).is_err());
    }

    #[test]
    fn apis_with_no_resources_and_no_default_construct_fine() {
        // Legal: a DB-only config with not a single API resource (see resolve_default_api).
        assert!(Apis::new(HashMap::new(), None).is_ok());
    }

    #[test]
    fn getting_a_resource_when_none_are_declared_names_the_missing_section() {
        let apis = Apis::new(HashMap::new(), None).expect("an empty set is not an error");
        let err = match apis.get("stub") {
            Ok(_) => panic!("there are no resources at all"),
            Err(e) => e,
        };
        assert!(err.contains("resources.api"), "{err}");
    }

    #[test]
    fn new_state_starts_on_the_default_resource() {
        let s = HttpState::new(apis_with("main", "http://localhost:1/", Vec::new()));
        assert_eq!(s.current(), "main");
    }

    #[test]
    fn new_state_seeds_headers_from_the_resource_defaults() {
        let defaults = vec![("accept".to_string(), "application/json".to_string())];
        let s = HttpState::new(apis_with("main", "http://localhost:1/", defaults));
        assert_eq!(
            s.headers,
            vec![("accept".to_string(), "application/json".to_string())]
        );
    }

    #[test]
    fn set_header_replaces_existing_case_insensitively() {
        let mut s = state();
        s.set_header("Accept", "text/plain");
        s.set_header("accept", "application/json");
        assert_eq!(s.headers.len(), 1);
        assert_eq!(s.headers[0].1, "application/json");
    }

    #[test]
    fn add_header_appends() {
        let mut s = state();
        s.set_header("Accept", "a");
        s.add_header("Accept", "b");
        assert_eq!(s.headers.len(), 2);
    }

    #[tokio::test]
    async fn failed_send_leaves_no_stale_exchange() {
        // Port 1 is unreachable: send fails at the transport layer. `last`
        // must stay None, or a failure dump would show a stale exchange.
        let mut s = state();
        assert!(s.send("/x", "GET").await.is_err());
        assert!(
            s.last().is_none(),
            "a failed send must not leave an exchange behind"
        );
    }

    #[test]
    fn body_and_form_are_mutually_exclusive() {
        let mut s = state();
        s.set_body("{}".into());
        s.set_form(vec![("a".into(), "1".into())]);
        assert!(s.body.is_none());
        s.set_body("{}".into());
        assert!(s.form.is_none());
    }

    #[test]
    fn extracts_cookie_from_set_cookie_header() {
        let ex = Exchange {
            method: "GET".into(),
            url: "http://x/".into(),
            req_headers: vec![],
            req_body: None,
            status: 200,
            resp_headers: vec![
                (
                    "set-cookie".into(),
                    "jwt_token=abc123; Path=/; HttpOnly".into(),
                ),
                ("set-cookie".into(), "refresh_token=def; Path=/".into()),
            ],
            body: String::new(),
        };
        assert_eq!(ex.set_cookie("jwt_token").as_deref(), Some("abc123"));
        assert_eq!(ex.set_cookie("refresh_token").as_deref(), Some("def"));
        assert_eq!(ex.set_cookie("absent"), None);
    }

    #[test]
    fn exchange_display_shows_request_and_response() {
        let ex = Exchange {
            method: "POST".into(),
            url: "http://x/login".into(),
            req_headers: vec![("Content-Type".into(), "application/json".into())],
            req_body: Some("{\"a\":1}".into()),
            status: 422,
            resp_headers: vec![],
            body: "{\"error\":\"no\"}".into(),
        };
        let s = ex.to_string();
        assert!(s.contains("POST http://x/login"));
        assert!(s.contains("Content-Type: application/json"));
        assert!(s.contains("← 422"));
    }
}
