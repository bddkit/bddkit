pub mod introspect;
pub mod ops;
pub mod plan;
pub mod platform;
pub mod reference;
pub mod value;

use crate::config::Connection;
use crate::db::platform::{MARIADB, MYSQL, PG, Platform};
use crate::db::reference::TableRef;
use crate::options::Options;
use plan::TableSchema;
use sqlx::AnyPool;
use sqlx::any::{AnyArguments, AnyPoolOptions, AnyRow};
use sqlx::{Any, ColumnIndex, Row, query::Query};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

/// Binds text parameters (`Option<String>`). Every value crosses the wire as
/// text; the SQL string itself carries the type, through whatever
/// `Platform::bind` produced — a `$N::type` cast on Postgres, a bare `?` on
/// MySQL and MariaDB, which coerce implicitly. Shared by step execution
/// (`ops`) and introspection (`introspect`).
pub fn bind_all<'q>(
    mut q: Query<'q, Any, AnyArguments<'q>>,
    binds: &'q [Option<String>],
) -> Query<'q, Any, AnyArguments<'q>> {
    for b in binds {
        q = q.bind(b);
    }
    q
}

/// Reads a column as text, tolerating a driver that reports it as a blob
/// instead of text — proven live: `information_schema.DATA_TYPE` comes back
/// as a blob on MySQL 8, and `CAST(x AS CHAR)` on a plain `TEXT` column comes
/// back as a blob on MariaDB. `sqlx::Any`'s `String` decode rejects a blob
/// outright, so on that failure this falls back to `Vec<u8>` (which `Any`
/// does decode from a blob, per `sqlx-core`'s `any/types/blob.rs`) and
/// converts with `String::from_utf8`. Shared by introspection and `ops`.
pub fn text_col<I>(row: &AnyRow, index: I) -> Result<Option<String>, String>
where
    I: ColumnIndex<AnyRow> + Copy,
{
    if let Ok(v) = row.try_get::<Option<String>, I>(index) {
        return Ok(v);
    }
    let bytes: Option<Vec<u8>> = row
        .try_get(index)
        .map_err(|e| format!("reading {index:?}: {e}"))?;
    match bytes {
        None => Ok(None),
        Some(b) => String::from_utf8(b)
            .map(Some)
            .map_err(|e| format!("reading {index:?}: not valid UTF-8: {e}")),
    }
}

/// Picks the platform family from the DSN scheme alone. That is enough to
/// configure `after_connect` and to validate `search_path` before any network
/// round trip: MySQL and MariaDB share the `mysql://` scheme and have
/// identical `session_setup` behavior (a schema is a database in both), so
/// telling them apart needs a live connection — done later, in `Db::connect`,
/// by probing `SELECT VERSION()`.
fn family_for_scheme(dsn: &str) -> Result<&'static dyn Platform, String> {
    match dsn.split("://").next().unwrap_or("") {
        "postgres" | "postgresql" => Ok(&PG),
        // sqlx's MySQL driver answers to both schemes, so refusing "mariadb"
        // here would reject a DSN the driver itself would have connected.
        "mysql" | "mariadb" => Ok(&MYSQL),
        other => Err(format!(
            "unknown database scheme {other:?} (expected postgres, postgresql, mysql or mariadb)"
        )),
    }
}

/// The half of `Db::connect` that opens no socket: the DSN's scheme names a
/// known platform, and the session setup that platform would run is legal for
/// it. Returns the platform family's name — on the `mysql://` family that is
/// still "mysql", because MySQL and MariaDB are only told apart by a query.
pub fn check_dsn(c: &Connection) -> Result<&'static str, String> {
    let family = family_for_scheme(&c.dsn)?;
    family.session_setup(&c.search_path)?;
    // The scheme is only the first word of the DSN. `AnyPoolOptions::connect`
    // parses the whole URL before it opens anything, so a bad port or a
    // malformed authority fails `run` statically — checking the scheme alone
    // here would certify a config the run refuses, which is the one thing
    // `doctor` must never do.
    use std::str::FromStr;
    sqlx::any::AnyConnectOptions::from_str(&c.dsn).map_err(|e| e.to_string())?;
    Ok(family.name())
}

/// Everything one connection needs to run and plan a query.
pub(crate) struct ConnectionState {
    pool: AnyPool,
    platform: &'static dyn Platform,
    options: Options,
}

/// Connections + the introspection cache. One per run: a connection belongs
/// to the system under test, not to a test set.
pub struct Db {
    connections: HashMap<String, ConnectionState>,
    cache: Mutex<HashMap<(String, String), Arc<TableSchema>>>,
}

impl Db {
    pub async fn connect(conns: &BTreeMap<String, Connection>, max: u32) -> Result<Db, String> {
        let mut connections = HashMap::new();
        for (name, c) in conns {
            let opts = AnyPoolOptions::new().max_connections(max.max(1));
            let family =
                family_for_scheme(&c.dsn).map_err(|e| format!("connection {name}: {e}"))?;
            let stmts = family
                .session_setup(&c.search_path)
                .map_err(|e| format!("connection {name}: {e}"))?;
            let pool = if stmts.is_empty() {
                opts.connect(&c.dsn).await
            } else {
                opts.after_connect(move |conn, _meta| {
                    let stmts = stmts.clone();
                    Box::pin(async move {
                        for stmt in &stmts {
                            sqlx::query(stmt).execute(&mut *conn).await?;
                        }
                        Ok(())
                    })
                })
                .connect(&c.dsn)
                .await
            };
            let pool = pool.map_err(|e| format!("connection {name}: {e}"))?;

            // Postgres's scheme already fully determines it; MySQL and
            // MariaDB share one scheme, so only there is a probe needed.
            let platform: &'static dyn Platform = if family.name() == "postgres" {
                family
            } else {
                let row = sqlx::query("SELECT VERSION()")
                    .fetch_one(&pool)
                    .await
                    .map_err(|e| format!("connection {name}: detecting the vendor: {e}"))?;
                let version = text_col(&row, 0)
                    .map_err(|e| format!("connection {name}: detecting the vendor: {e}"))?
                    .unwrap_or_default();
                if version.contains("MariaDB") {
                    &MARIADB
                } else {
                    &MYSQL
                }
            };
            connections.insert(
                name.clone(),
                ConnectionState {
                    pool,
                    platform,
                    options: c.effective_options.clone(),
                },
            );
        }
        Ok(Db {
            connections,
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// The single place that knows "not declared" for a connection name.
    pub(crate) fn connection(&self, name: &str) -> Result<&ConnectionState, String> {
        self.connections
            .get(name)
            .ok_or_else(|| format!("connection {name:?} is not declared in resources.db"))
    }

    pub fn options(&self, name: &str) -> Result<&Options, String> {
        Ok(&self.connection(name)?.options)
    }

    /// The resolved platform for one connection: the vendor `Db::connect`
    /// settled on, not just the scheme family it started from.
    #[allow(dead_code)] // only tests read it, same as HttpState::current
    pub(crate) fn platform(&self, name: &str) -> Result<&'static dyn Platform, String> {
        Ok(self.connection(name)?.platform)
    }

    /// Introspection with caching. std::Mutex is NOT held across an await.
    pub async fn schema(&self, conn: &str, tref: &TableRef) -> Result<Arc<TableSchema>, String> {
        let key = (conn.to_string(), tref.sql_name());
        if let Some(s) = self.cache.lock().expect("cache mutex").get(&key) {
            return Ok(s.clone());
        }
        let state = self.connection(conn)?;
        let schema = Arc::new(introspect::introspect(&state.pool, state.platform, tref).await?);
        self.cache
            .lock()
            .expect("cache mutex")
            .insert(key, schema.clone());
        Ok(schema)
    }
}

/// A DB handle for one file run: a reference to the shared pools + the current
/// connection. `current` resets at the scenario boundary (§8: connection scope is the scenario).
pub struct DbHandle {
    db: Option<Arc<Db>>,
    default: String,
    current: String,
}

impl DbHandle {
    pub fn new(db: Option<Arc<Db>>, default: String) -> Self {
        Self {
            db,
            current: default.clone(),
            default,
        }
    }

    pub fn reset(&mut self) {
        self.current = self.default.clone();
    }

    pub fn current(&self) -> &str {
        &self.current
    }

    pub fn resources(&self) -> Result<&Arc<Db>, String> {
        self.db.as_ref().ok_or_else(|| {
            "no database connection is declared in the config (resources.db)".to_string()
        })
    }

    pub fn set_current(&mut self, name: &str) -> Result<(), String> {
        self.resources()?.connection(name)?; // check that the connection exists
        self.current = name.to_string();
        Ok(())
    }

    pub fn options(&self) -> Result<&Options, String> {
        self.resources()?.options(&self.current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `check_dsn` parses the DSN through `AnyConnectOptions`, which resolves
    /// the driver by scheme. `main` installs the drivers once at startup; unit
    /// tests share one process and run in parallel, so without this the result
    /// depends on which test happened to run first.
    fn drivers() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(sqlx::any::install_default_drivers);
    }

    #[test]
    fn a_handle_without_connections_reports_no_database() {
        let h = DbHandle::new(None, String::new());
        assert!(
            h.resources().is_err(),
            "resources() is an error without connections"
        );
    }

    #[test]
    fn a_handle_without_connections_cannot_switch() {
        let mut h = DbHandle::new(None, String::new());
        assert!(
            h.set_current("x").is_err(),
            "switching without connections is an error"
        );
    }

    #[test]
    fn reset_returns_to_the_default_connection() {
        let mut h = DbHandle::new(None, "main".to_string());
        h.current = "audit".to_string();
        h.reset();
        assert_eq!(h.current(), "main");
    }

    #[test]
    fn check_dsn_names_the_platform_without_opening_a_socket() {
        drivers();
        // Port 1 is closed on any sane host: this must not connect.
        let c = Connection {
            dsn: "postgres://u:p@127.0.0.1:1/x".into(),
            search_path: vec!["app".into()],
            ..Default::default()
        };
        assert_eq!(check_dsn(&c).unwrap(), "postgres");
    }

    #[test]
    fn check_dsn_refuses_a_dsn_the_driver_cannot_parse() {
        // `Db::connect` parses the whole URL before it opens anything, so a
        // typo past the scheme is a static failure for `run` — and must be one
        // here too, or `doctor` certifies a config the run refuses.
        drivers();
        let c = Connection {
            dsn: "postgres://u:p@127.0.0.1:notaport/x".into(),
            ..Default::default()
        };
        let error = check_dsn(&c).unwrap_err();
        assert!(error.contains("port"), "{error}");
    }

    #[test]
    fn check_dsn_refuses_a_search_path_the_platform_cannot_honour() {
        drivers();
        let c = Connection {
            dsn: "mysql://u:p@127.0.0.1:1/x".into(),
            search_path: vec!["app".into()],
            ..Default::default()
        };
        let error = check_dsn(&c).unwrap_err();
        assert!(error.contains("search_path"), "{error}");
    }

    #[test]
    fn family_for_scheme_picks_by_dsn_prefix() {
        assert_eq!(
            family_for_scheme("postgres://x/y").unwrap().name(),
            "postgres"
        );
        assert_eq!(
            family_for_scheme("postgresql://x/y").unwrap().name(),
            "postgres"
        );
        // MariaDB shares the mysql:// scheme; the exact vendor is only
        // settled after connecting (see Db::connect), so this is the family.
        assert_eq!(family_for_scheme("mysql://x/y").unwrap().name(), "mysql");
        // sqlx-mysql declares URL_SCHEMES = ["mysql", "mariadb"], so a DSN the
        // driver accepts must not be refused a layer above it.
        assert_eq!(family_for_scheme("mariadb://x/y").unwrap().name(), "mysql");
        let err = match family_for_scheme("mssql://x/y") {
            Ok(_) => panic!("an unknown scheme must be refused"),
            Err(e) => e,
        };
        assert!(err.contains("mssql"), "{err}");
    }

    #[test]
    fn platform_reports_the_same_not_declared_error_as_connection() {
        let db = Db {
            connections: HashMap::new(),
            cache: Mutex::new(HashMap::new()),
        };
        let err = match db.platform("x") {
            Ok(_) => panic!("an undeclared connection must be refused"),
            Err(e) => e,
        };
        assert!(err.contains("x"), "{err}");
    }

    #[tokio::test]
    async fn an_unknown_scheme_is_refused_by_name() {
        sqlx::any::install_default_drivers();
        let mut conns = BTreeMap::new();
        conns.insert(
            "primary".to_string(),
            Connection {
                dsn: "mssql://x/y".to_string(),
                ..Default::default()
            },
        );
        let err = match Db::connect(&conns, 1).await {
            Ok(_) => panic!("connecting to an unknown scheme should fail"),
            Err(e) => e,
        };
        // A bare substring of "connection" (e.g. "c") would pass on any error
        // at all, since Db::connect wraps every failure with
        // "connection {name}: ...". "primary" pins that the name survives,
        // and "mssql" pins the actual behavior under Any: the scheme is
        // rejected at parse time, not dialed as a hostname and failed on DNS.
        assert!(err.contains("primary"), "{err}");
        assert!(err.contains("mssql"), "{err}");
    }
}
