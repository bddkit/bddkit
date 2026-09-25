use super::platform::Platform;

/// A table column from introspection data, as reported by the current
/// platform's `Platform::introspect` query. `type_name` is the platform's
/// native type name, as consumed by `Platform::bind`/`returning`.
#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub type_name: String,
    /// Declared length/precision (e.g. `char(36)`), where the platform needs
    /// it to tell columns of the same `type_name` apart. NULL where it never
    /// matters — Postgres always returns NULL here.
    // Read by the MySQL dialect only; still unread from live code until
    // Db::connect selects a platform by vendor (next task).
    #[allow(dead_code)]
    pub length: Option<i64>,
    pub not_null: bool,
    pub has_default: bool,
    pub is_identity: bool,
    pub is_pk: bool,
}

#[derive(Debug, Clone)]
pub struct TableSchema {
    pub columns: Vec<Column>,
}

impl TableSchema {
    pub fn col(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }
    pub fn pk_columns(&self) -> Vec<&Column> {
        self.columns.iter().filter(|c| c.is_pk).collect()
    }
}

/// Where the text value of one PK column's `last_insert_*` variable comes
/// from. Only consulted when the platform produced no `RETURNING` clause —
/// `ops::insert` reads the row back for that instead. See `Platform::returning`.
#[derive(Debug, Clone, PartialEq)]
pub enum PkSource {
    /// The text value is already known before the INSERT runs: given in the
    /// step, or a client-generated UUIDv7 (`build_insert` step 2).
    Known(String),
    /// Not known yet — read it off the INSERT's own result
    /// (`AnyQueryResult::last_insert_id`), never a second query.
    AutoIncrement,
    /// No source exists: a server-side default that is not auto-increment,
    /// or (composite PK) one part filled that way. Carries the column name
    /// so the caller can fail naming it. `build_insert` already refuses this
    /// case itself when there is no RETURNING (see below) — a variant this
    /// well-typed still beats an early bailout with no trace of why.
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsertPlan {
    pub sql: String,
    pub binds: Vec<Option<String>>,
    pub logs: Vec<String>,
    /// Whether `sql` ends in a `RETURNING`-equivalent clause.
    pub has_returning: bool,
    /// One entry per PK column, `(last_insert_* variable name, where its
    /// value comes from)` — one `Vec`, not two zipped by position, so
    /// misalignment between a name and its source cannot compile in one
    /// column being renamed and not the other.
    pub pk_vars: Vec<(String, PkSource)>,
}

/// Builds an INSERT: applies the PK and NOT NULL fill rules from spec §8.
/// `values` are the pairs given in the step (`None` = SQL NULL). `index` is the
/// variable-name suffix for the table form (`Some(0)` → `_0`).
pub fn build_insert(
    platform: &dyn Platform,
    schema: &TableSchema,
    sql_name: &str,
    bare: &str,
    values: &[(String, Option<String>)],
    index: Option<usize>,
) -> Result<InsertPlan, String> {
    // Given columns must exist and be bindable.
    for (col, _) in values {
        plain_column(col)?;
        match schema.col(col) {
            None => return Err(format!("column {col:?} is missing from table {sql_name}")),
            Some(c) => platform.check_bindable(c)?,
        }
    }
    let given: std::collections::HashSet<&str> = values.iter().map(|(c, _)| c.as_str()).collect();

    let mut cols: Vec<String> = Vec::new();
    let mut exprs: Vec<String> = Vec::new();
    let mut binds: Vec<Option<String>> = Vec::new();
    let mut logs: Vec<String> = Vec::new();
    let mut param = 1usize;
    // PK column name -> its client-generated UUIDv7, for pk_vars below.
    let mut generated_uuids: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    // 1. Given values — as-is, cast to the column's type.
    for (col, val) in values {
        let ty = &schema.col(col).expect("checked above").type_name;
        cols.push(col.clone());
        exprs.push(platform.bind(param, ty));
        binds.push(val.clone());
        param += 1;
    }

    // 2. Primary keys with no value.
    for pk in schema.pk_columns() {
        if given.contains(pk.name.as_str()) {
            continue;
        }
        if pk.has_default || pk.is_identity {
            continue; // the platform will generate it itself (default or identity/auto-increment).
        }
        if platform.wants_client_uuid(pk) {
            let id = uuid::Uuid::now_v7().to_string();
            logs.push(format!("PK {} := {id} (UUIDv7)", pk.name));
            cols.push(pk.name.clone());
            exprs.push(platform.bind(param, &pk.type_name));
            binds.push(Some(id.clone()));
            generated_uuids.insert(pk.name.clone(), id);
            param += 1;
        } else {
            return Err(format!(
                "primary key {} ({}) has no value and is not generated (no default, not uuid)",
                pk.name, pk.type_name
            ));
        }
    }

    // 3. NOT NULL with no value and no default.
    for c in &schema.columns {
        if given.contains(c.name.as_str()) || c.is_pk || c.has_default || !c.not_null {
            continue;
        }
        if platform.is_timestamplike(&c.type_name) {
            logs.push(format!("{} := now()", c.name));
            cols.push(c.name.clone());
            // "now()" is portable across Postgres, MySQL and MariaDB — no need
            // to route it through Platform.
            exprs.push("now()".to_string());
        } else {
            return Err(format!(
                "column {} ({}) is NOT NULL, has no value and no default",
                c.name, c.type_name
            ));
        }
    }

    // 4. Assemble the SQL.
    let pk_cols = schema.pk_columns();
    let body = if cols.is_empty() {
        platform.insert_no_columns(sql_name)
    } else {
        format!(
            "INSERT INTO {sql_name} ({}) VALUES ({})",
            cols.join(", "),
            exprs.join(", ")
        )
    };
    let returning_clause = platform.returning(&pk_cols);
    let has_returning = returning_clause.is_some();
    let sql = match returning_clause {
        Some(clause) => format!("{body} {clause}"),
        None => body,
    };

    // 5. One (variable name, source) pair per PK column, in `pk_cols` order —
    // built in a single pass so the name and its source can never drift apart.
    let suffix = index.map(|i| format!("_{i}")).unwrap_or_default();
    let single = pk_cols.len() == 1;
    let pk_vars: Vec<(String, PkSource)> = pk_cols
        .iter()
        .map(|pk| {
            let name = if single {
                format!("last_insert_id_{bare}{suffix}")
            } else {
                format!("last_insert_{bare}_{}{suffix}", pk.name)
            };
            // A value already given (Some or explicit NULL) always wins over
            // how the column would otherwise have been filled — matching
            // step 2 above, which also skips a given column outright.
            let source = if let Some((_, v)) = values.iter().find(|(c, _)| c == &pk.name) {
                match v {
                    Some(s) => PkSource::Known(s.clone()),
                    // Given <<null>>: MySQL/MariaDB treat NULL into an
                    // AUTO_INCREMENT column the same as omitting it, so it
                    // still generates — anything else given as NULL has
                    // nothing to report.
                    None if pk.is_identity => PkSource::AutoIncrement,
                    None => PkSource::Unknown(pk.name.clone()),
                }
            } else if pk.is_identity {
                PkSource::AutoIncrement
            } else if let Some(id) = generated_uuids.get(&pk.name) {
                PkSource::Known(id.clone())
            } else {
                // has_default but not identity: a server-side DEFAULT we
                // cannot read back without RETURNING.
                PkSource::Unknown(pk.name.clone())
            };
            (name, source)
        })
        .collect();

    // Refuse before the INSERT runs, not after: a platform with no RETURNING
    // and a PK column with no source would otherwise commit the row and only
    // then report failure (invariant 1 — check everything checkable first).
    if !has_returning
        && let Some((_, PkSource::Unknown(col))) = pk_vars
            .iter()
            .find(|(_, s)| matches!(s, PkSource::Unknown(_)))
    {
        return Err(format!(
            "primary key {col} is server-generated (a DEFAULT that is not auto-increment) \
             and this platform has no RETURNING to read it back with — give the value explicitly"
        ));
    }

    Ok(InsertPlan {
        sql,
        binds,
        logs,
        has_returning,
        pk_vars,
    })
}

/// The comparison one condition pair asks for. It is written as a suffix on
/// the COLUMN name (`slug~: u123%`), never inferred from the value's text —
/// so a `%` that arrives inside a variable can never turn an equality into a
/// pattern behind the author's back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Like,
    NotLike,
}

impl Op {
    /// The SQL keyword. A `match`, not an `if`, on purpose: this is the one
    /// place a new operator can be forgotten silently, because the arms of
    /// `build_where` below already pattern-match on `Op` and so refuse to
    /// compile until a new variant is placed. Adding `Gte` here without a
    /// keyword is a compile error; adding it to an `if op == Op::Eq` was not.
    fn sql(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ne => "<>",
            Op::Like => "LIKE",
            Op::NotLike => "NOT LIKE",
        }
    }
}

/// Splits `slug!~` into (`slug`, `Op::NotLike`). Longest suffix first, so
/// `!~` cannot degrade into `!`. A leftover OPERATOR-SHAPED character is an
/// error naming what was written, because the alternative is a schema lookup
/// for a column called `price>=`, which reports a missing column and sends
/// the author to their DDL over a typo in their step. The guard covers
/// `!~<>=` only — `slug*:` still reports a missing column, which is the right
/// answer for a character no operator will ever start with.
fn split_operator(raw: &str) -> Result<(&str, Op), String> {
    const SUFFIXES: [(&str, Op); 3] = [("!~", Op::NotLike), ("~", Op::Like), ("!", Op::Ne)];
    let (name, op) = SUFFIXES
        .iter()
        .find_map(|(s, op)| raw.strip_suffix(s).map(|n| (n, *op)))
        .unwrap_or((raw, Op::Eq));
    let name = name.trim_end();
    if name.is_empty() || name.ends_with(|c: char| "!~<>=".contains(c)) {
        return Err(format!(
            "unknown operator in condition column {raw:?}; available: \
             `col:` (=), `col!:` (<>), `col~:` (LIKE), `col!~:` (NOT LIKE)"
        ));
    }
    Ok((name, op))
}

/// An operator belongs to a condition. Anywhere a value is WRITTEN it is a
/// typo, and letting it through would report a missing column `slug~` —
/// sending the author to look at their schema instead of at their step.
pub fn plain_column(raw: &str) -> Result<(), String> {
    match split_operator(raw)? {
        (_, Op::Eq) => Ok(()),
        _ => Err(format!(
            "column {raw:?}: an operator is only allowed in a condition, \
             not where a value is set"
        )),
    }
}

/// Builds `col <op> <bind> AND …`, in the platform's own bind syntax.
/// Parameter numbering starts at `start` (for UPDATE, where SET takes the
/// first parameters).
///
/// NULL is `col: <<null>>` → `IS NULL` and `col!: <<null>>` → `IS NOT NULL`,
/// both with no bind; a LIKE against NULL is refused rather than built, since
/// it can only ever match nothing.
///
/// Under `~`/`!~` the value is an SQL LIKE pattern in full: `%` and `_` both
/// match, and `\%` / `\_` are the literal characters — backslash is the
/// default LIKE escape on all three engines, so no ESCAPE clause is built.
/// The COLUMN side goes through `cast_text`, so a non-text column is matched
/// against its own text rendering instead of failing on the operator.
pub fn build_where(
    platform: &dyn Platform,
    schema: &TableSchema,
    pairs: &[(String, Option<String>)],
    start: usize,
) -> Result<(String, Vec<Option<String>>), String> {
    let mut parts = Vec::new();
    let mut binds = Vec::new();
    let mut param = start;
    for (raw, val) in pairs {
        let (col, op) = split_operator(raw)?;
        let c = schema
            .col(col)
            .ok_or_else(|| format!("column {col:?} is missing from the table"))?;
        platform.check_bindable(c)?;
        match (op, val) {
            (Op::Eq, None) => parts.push(format!("{col} IS NULL")),
            (Op::Ne, None) => parts.push(format!("{col} IS NOT NULL")),
            (Op::Like | Op::NotLike, None) => {
                return Err(format!(
                    "column {col:?}: a LIKE has nothing to match against <<null>>; \
                     write `{col}: <<null>>` or `{col}!: <<null>>`"
                ));
            }
            // A pattern: the COLUMN is rendered as text and the value binds
            // as text. An ordering operator must NOT be added here — under
            // `cast_text` a comparison is lexicographic, and `amount>=: 9`
            // would answer `'9' >= '10'`. It belongs in the arm below, which
            // binds the column's own type and lets the engine compare.
            (Op::Like | Op::NotLike, Some(_)) => {
                parts.push(format!(
                    "{} {} {}",
                    platform.cast_text(col),
                    op.sql(),
                    platform.bind(param, "text")
                ));
                binds.push(val.clone());
                param += 1;
            }
            (Op::Eq | Op::Ne, Some(_)) => {
                parts.push(format!(
                    "{col} {} {}",
                    op.sql(),
                    platform.bind(param, &c.type_name)
                ));
                binds.push(val.clone());
                param += 1;
            }
        }
    }
    Ok((parts.join(" AND "), binds))
}

pub fn build_update(
    platform: &dyn Platform,
    schema: &TableSchema,
    sql_name: &str,
    set: &[(String, Option<String>)],
    where_: &[(String, Option<String>)],
) -> Result<(String, Vec<Option<String>>), String> {
    if where_.is_empty() {
        return Err("UPDATE without WHERE is forbidden; give a condition for a bulk change".into());
    }
    let mut sets = Vec::new();
    let mut binds = Vec::new();
    let mut param = 1usize;
    for (col, val) in set {
        plain_column(col)?;
        let c = schema
            .col(col)
            .ok_or_else(|| format!("column {col:?} is missing from the table"))?;
        platform.check_bindable(c)?;
        sets.push(format!("{col} = {}", platform.bind(param, &c.type_name)));
        binds.push(val.clone());
        param += 1;
    }
    let (where_sql, where_binds) = build_where(platform, schema, where_, param)?;
    binds.extend(where_binds);
    Ok((
        format!(
            "UPDATE {sql_name} SET {} WHERE {where_sql}",
            sets.join(", ")
        ),
        binds,
    ))
}

pub fn build_delete(
    platform: &dyn Platform,
    schema: &TableSchema,
    sql_name: &str,
    where_: &[(String, Option<String>)],
) -> Result<(String, Vec<Option<String>>), String> {
    if where_.is_empty() {
        return Err(
            "DELETE without WHERE is forbidden; use the \"I delete all\" step for a full wipe"
                .into(),
        );
    }
    let (where_sql, binds) = build_where(platform, schema, where_, 1)?;
    Ok((format!("DELETE FROM {sql_name} WHERE {where_sql}"), binds))
}

pub fn build_delete_all(sql_name: &str) -> String {
    format!("DELETE FROM {sql_name}")
}

pub fn build_exists(
    platform: &dyn Platform,
    schema: &TableSchema,
    sql_name: &str,
    where_: &[(String, Option<String>)],
) -> Result<(String, Vec<Option<String>>), String> {
    if where_.is_empty() {
        return Err("an existence check requires a condition".into());
    }
    let (where_sql, binds) = build_where(platform, schema, where_, 1)?;
    Ok((
        format!("SELECT 1 FROM {sql_name} WHERE {where_sql} LIMIT 1"),
        binds,
    ))
}

#[cfg(test)]
mod tests {
    use super::super::platform::{MARIADB, MYSQL, PG};
    use super::*;

    fn names(p: &InsertPlan) -> Vec<&str> {
        p.pk_vars.iter().map(|(n, _)| n.as_str()).collect()
    }

    fn col(
        name: &str,
        ty: &str,
        not_null: bool,
        has_default: bool,
        is_identity: bool,
        is_pk: bool,
    ) -> Column {
        Column {
            name: name.into(),
            type_name: ty.into(),
            length: None,
            not_null,
            has_default,
            is_identity,
            is_pk,
        }
    }

    // users(id uuid pk with no default, email text not null, created_at timestamptz not null)
    fn users_uuid() -> TableSchema {
        TableSchema {
            columns: vec![
                col("id", "uuid", true, false, false, true),
                col("email", "text", true, false, false, false),
                col("created_at", "timestamptz", true, false, false, false),
            ],
        }
    }

    #[test]
    fn generates_uuid_pk_and_fills_timestamp() {
        let p = build_insert(
            &PG,
            &users_uuid(),
            "users",
            "users",
            &[("email".into(), Some("a@b.net".into()))],
            None,
        )
        .unwrap();
        // email is given ($1), id is generated ($2::uuid), created_at → now()
        assert!(
            p.sql.starts_with(
                "INSERT INTO users (email, id, created_at) VALUES ($1::text, $2::uuid, now())"
            ),
            "{}",
            p.sql
        );
        assert!(p.sql.ends_with("RETURNING (id)::text"), "{}", p.sql);
        assert_eq!(p.binds.len(), 2);
        assert_eq!(p.binds[0], Some("a@b.net".to_string()));
        assert!(uuid::Uuid::parse_str(p.binds[1].as_ref().unwrap()).is_ok());
        assert_eq!(names(&p), vec!["last_insert_id_users"]);
    }

    #[test]
    fn omits_identity_and_default_pk() {
        // companies(id int identity pk, slug text not null)
        let s = TableSchema {
            columns: vec![
                col("id", "int4", true, true, true, true),
                col("slug", "text", true, false, false, false),
            ],
        };
        let p = build_insert(
            &PG,
            &s,
            "companies",
            "companies",
            &[("slug".into(), Some("x".into()))],
            None,
        )
        .unwrap();
        assert_eq!(
            p.sql,
            "INSERT INTO companies (slug) VALUES ($1::text) RETURNING (id)::text"
        );
        assert_eq!(names(&p), vec!["last_insert_id_companies"]);
    }

    #[test]
    fn missing_plain_not_null_is_error() {
        let s = TableSchema {
            columns: vec![
                col("id", "int4", true, true, true, true),
                col("qty", "int4", true, false, false, false),
            ],
        };
        let err = build_insert(&PG, &s, "t", "t", &[], None).unwrap_err();
        assert!(err.contains("qty"), "{err}");
    }

    #[test]
    fn unknown_column_is_error() {
        let err = build_insert(
            &PG,
            &users_uuid(),
            "users",
            "users",
            &[("nope".into(), Some("1".into()))],
            None,
        )
        .unwrap_err();
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn provided_null_binds_none() {
        let s = TableSchema {
            columns: vec![
                col("id", "int4", true, true, true, true),
                col("deleted_at", "timestamptz", false, false, false, false),
            ],
        };
        let p = build_insert(&PG, &s, "t", "t", &[("deleted_at".into(), None)], None).unwrap();
        assert_eq!(p.binds, vec![None]);
        assert!(p.sql.contains("$1::timestamptz"), "{}", p.sql);
    }

    #[test]
    fn composite_pk_yields_per_column_vars() {
        let s = TableSchema {
            columns: vec![
                col("a", "int4", true, false, false, true),
                col("b", "int4", true, false, false, true),
            ],
        };
        let p = build_insert(
            &PG,
            &s,
            "pair",
            "pair",
            &[
                ("a".into(), Some("1".into())),
                ("b".into(), Some("2".into())),
            ],
            None,
        )
        .unwrap();
        assert_eq!(names(&p), vec!["last_insert_pair_a", "last_insert_pair_b"]);
        assert!(
            p.sql.ends_with("RETURNING (a)::text, (b)::text"),
            "{}",
            p.sql
        );
    }

    #[test]
    fn table_index_suffixes_var_name() {
        let p = build_insert(
            &PG,
            &users_uuid(),
            "users",
            "users",
            &[("email".into(), Some("a@b.net".into()))],
            Some(3),
        )
        .unwrap();
        assert_eq!(names(&p), vec!["last_insert_id_users_3"]);
    }

    fn companies() -> TableSchema {
        TableSchema {
            columns: vec![
                col("id", "int4", true, true, true, true),
                col("slug", "text", true, false, false, false),
                col("deleted_at", "timestamptz", false, false, false, false),
            ],
        }
    }

    #[test]
    fn where_uses_typed_casts_and_is_null() {
        let (sql, binds) = build_where(
            &PG,
            &companies(),
            &[
                ("slug".into(), Some("x".into())),
                ("deleted_at".into(), None),
            ],
            1,
        )
        .unwrap();
        assert_eq!(sql, "slug = $1::text AND deleted_at IS NULL");
        assert_eq!(binds, vec![Some("x".to_string())]);
    }

    #[test]
    fn where_param_numbering_respects_start() {
        let (sql, _) =
            build_where(&PG, &companies(), &[("slug".into(), Some("x".into()))], 4).unwrap();
        assert_eq!(sql, "slug = $4::text");
    }

    #[test]
    fn where_unknown_column_is_error() {
        assert!(build_where(&PG, &companies(), &[("nope".into(), Some("1".into()))], 1).is_err());
    }

    #[test]
    fn update_sets_then_where_numbering() {
        let (sql, binds) = build_update(
            &PG,
            &companies(),
            "companies",
            &[("slug".into(), Some("new".into()))],
            &[("id".into(), Some("7".into()))],
        )
        .unwrap();
        assert_eq!(
            sql,
            "UPDATE companies SET slug = $1::text WHERE id = $2::int4"
        );
        assert_eq!(binds, vec![Some("new".to_string()), Some("7".to_string())]);
    }

    #[test]
    fn update_requires_where() {
        assert!(
            build_update(
                &PG,
                &companies(),
                "companies",
                &[("slug".into(), Some("x".into()))],
                &[]
            )
            .is_err()
        );
    }

    #[test]
    fn operator_parsing_holds_at_its_edges() {
        // `parse_oneliner` trims the whole column, so `slug ~: x` arrives as
        // "slug ~" and the trim_end inside split_operator is what saves it.
        let (sql, _) = build_delete(
            &PG,
            &companies(),
            "companies",
            &[("slug ~".into(), Some("a%".into()))],
        )
        .unwrap();
        assert!(
            sql.contains("LIKE"),
            "a space before the operator is allowed: {sql}"
        );

        for raw in ["!", "~", "slug!!", "slug~!", "slug>="] {
            let err =
                build_where(&PG, &companies(), &[(raw.into(), Some("x".into()))], 1).unwrap_err();
            assert!(
                err.contains("unknown operator") && err.contains(raw),
                "{raw:?} must be refused by name, got: {err}"
            );
        }
    }

    #[test]
    fn the_operator_comes_from_the_column_not_from_the_value() {
        // The whole point of the suffix: an unmarked column is `=` even when
        // the value is full of wildcards, so a `%` arriving through a variable
        // cannot widen a delete behind the author's back.
        let (sql, binds) = build_delete(
            &PG,
            &companies(),
            "companies",
            &[("slug".into(), Some("50%_x".into()))],
        )
        .unwrap();
        assert_eq!(sql, "DELETE FROM companies WHERE slug = $1::text");
        assert_eq!(binds, vec![Some("50%_x".to_string())]);
    }

    #[test]
    fn a_tilde_asks_for_like_and_the_value_is_the_pattern() {
        let (sql, binds) = build_delete(
            &PG,
            &companies(),
            "companies",
            &[("slug~".into(), Some("demo_run_uabc123%".into()))],
        )
        .unwrap();
        assert_eq!(
            sql,
            "DELETE FROM companies WHERE (slug)::text LIKE $1::text"
        );
        assert_eq!(
            binds,
            vec![Some("demo_run_uabc123%".to_string())],
            "the author asked for a pattern: it reaches the server untouched"
        );
    }

    #[test]
    fn bang_is_not_equal_and_bang_tilde_is_not_like() {
        let (sql, _) = build_delete(
            &PG,
            &companies(),
            "companies",
            &[
                ("id!".into(), Some("7".into())),
                ("slug!~".into(), Some("tmp%".into())),
            ],
        )
        .unwrap();
        assert_eq!(
            sql,
            "DELETE FROM companies WHERE id <> $1::int4 AND (slug)::text NOT LIKE $2::text"
        );
    }

    #[test]
    fn null_reads_is_null_and_is_not_null() {
        let (sql, binds) = build_where(
            &PG,
            &companies(),
            &[("deleted_at".into(), None), ("slug!".into(), None)],
            1,
        )
        .unwrap();
        assert_eq!(sql, "deleted_at IS NULL AND slug IS NOT NULL");
        assert!(binds.is_empty(), "neither side binds anything");
    }

    #[test]
    fn a_like_against_null_is_refused_naming_the_alternative() {
        let err = build_where(&PG, &companies(), &[("slug~".into(), None)], 1).unwrap_err();
        assert!(err.contains("slug"), "{err}");
        assert!(err.contains("<<null>>"), "{err}");
    }

    #[test]
    fn an_unknown_operator_is_an_error_naming_it() {
        // Not "column \"price>=\" is missing from the table": that answer sends
        // the author to their DDL for what is a typo in the step.
        let err =
            build_where(&PG, &companies(), &[("slug>=".into(), Some("1".into()))], 1).unwrap_err();
        assert!(err.contains("slug>="), "{err}");
        assert!(err.contains("unknown operator"), "{err}");
    }

    #[test]
    fn an_operator_is_refused_where_a_value_is_written() {
        let err = build_update(
            &PG,
            &companies(),
            "companies",
            &[("slug~".into(), Some("x%".into()))],
            &[("id".into(), Some("1".into()))],
        )
        .unwrap_err();
        assert!(err.contains("only allowed in a condition"), "{err}");
        let err = build_insert(
            &PG,
            &companies(),
            "companies",
            "companies",
            &[("slug!".into(), Some("x".into()))],
            None,
        )
        .unwrap_err();
        assert!(err.contains("only allowed in a condition"), "{err}");
    }

    #[test]
    fn operators_reach_an_assertion_too() {
        // Nothing gates them by step any more: the syntax is explicit, so
        // there is no silent loosening left to protect an assertion from.
        let (sql, _) = build_exists(
            &PG,
            &companies(),
            "companies",
            &[("slug~".into(), Some("uabc%".into()))],
        )
        .unwrap();
        assert_eq!(
            sql,
            "SELECT 1 FROM companies WHERE (slug)::text LIKE $1::text LIMIT 1"
        );
    }

    #[test]
    fn like_is_spelled_in_the_mysql_dialect() {
        let (sql, _) = build_delete(
            &MYSQL,
            &companies(),
            "companies",
            &[("slug~".into(), Some("uabc123%".into()))],
        )
        .unwrap();
        assert_eq!(sql, "DELETE FROM companies WHERE CAST(slug AS CHAR) LIKE ?");
    }

    #[test]
    fn delete_requires_where() {
        assert!(build_delete(&PG, &companies(), "companies", &[]).is_err());
    }

    #[test]
    fn delete_all_has_no_where() {
        assert_eq!(build_delete_all("companies"), "DELETE FROM companies");
    }

    #[test]
    fn exists_selects_one() {
        let (sql, _) = build_exists(
            &PG,
            &companies(),
            "companies",
            &[("slug".into(), Some("x".into()))],
        )
        .unwrap();
        assert_eq!(sql, "SELECT 1 FROM companies WHERE slug = $1::text LIMIT 1");
    }

    // companies(id int auto_increment pk, slug text not null) — MySQL/MariaDB shape:
    // is_identity true, has_default false (auto_increment is its own thing, not a
    // DEFAULT in information_schema's sense).
    fn mysql_companies() -> TableSchema {
        TableSchema {
            columns: vec![
                col("id", "int", true, false, true, true),
                col("slug", "text", true, false, false, false),
            ],
        }
    }

    #[test]
    fn no_returning_and_no_value_reads_the_pk_off_auto_increment() {
        let p = build_insert(
            &MYSQL,
            &mysql_companies(),
            "companies",
            "companies",
            &[("slug".into(), Some("x".into()))],
            None,
        )
        .unwrap();
        assert!(!p.has_returning);
        assert_eq!(
            p.pk_vars,
            vec![("last_insert_id_companies".into(), PkSource::AutoIncrement)]
        );
    }

    #[test]
    fn no_returning_and_a_given_value_is_known_even_on_an_identity_column() {
        let p = build_insert(
            &MYSQL,
            &mysql_companies(),
            "companies",
            "companies",
            &[
                ("id".into(), Some("5".into())),
                ("slug".into(), Some("x".into())),
            ],
            None,
        )
        .unwrap();
        assert_eq!(
            p.pk_vars,
            vec![(
                "last_insert_id_companies".into(),
                PkSource::Known("5".into())
            )]
        );
    }

    #[test]
    fn no_returning_and_an_explicit_null_on_an_identity_column_is_still_auto_increment() {
        // <<null>> given for id: MySQL/MariaDB generate on NULL into
        // AUTO_INCREMENT exactly as they do when the column is omitted —
        // reported as Unknown here would be both the wrong diagnosis and
        // wrong advice ("give the value explicitly" on a column that just did).
        let p = build_insert(
            &MYSQL,
            &mysql_companies(),
            "companies",
            "companies",
            &[("id".into(), None), ("slug".into(), Some("x".into()))],
            None,
        )
        .unwrap();
        assert_eq!(
            p.pk_vars,
            vec![("last_insert_id_companies".into(), PkSource::AutoIncrement)]
        );
    }

    #[test]
    fn no_returning_and_a_client_uuid_is_known_before_the_insert_runs() {
        // A lone char(36) PK with no default: MySQL's wants_client_uuid is
        // true for it, so build_insert generates the value up front — it is
        // Known, not AutoIncrement, even with no RETURNING to fall back on.
        let s = TableSchema {
            columns: vec![Column {
                name: "id".into(),
                type_name: "char".into(),
                length: Some(36),
                not_null: true,
                has_default: false,
                is_identity: false,
                is_pk: true,
            }],
        };
        let p = build_insert(&MYSQL, &s, "users", "users", &[], None).unwrap();
        assert!(!p.has_returning);
        let (name, source) = &p.pk_vars[0];
        assert_eq!(name, "last_insert_id_users");
        match source {
            PkSource::Known(id) => assert!(uuid::Uuid::parse_str(id).is_ok(), "{id}"),
            other => panic!("expected a client-generated uuid, got {other:?}"),
        }
    }

    #[test]
    fn no_returning_and_a_non_identity_default_pk_is_refused_before_the_insert_runs() {
        // A server-side DEFAULT that is not AUTO_INCREMENT: has_default true,
        // is_identity false. MySQL has no RETURNING to read it back with, so
        // build_insert must refuse — before returning a plan, per invariant 1.
        let s = TableSchema {
            columns: vec![
                col("id", "int", true, true, false, true),
                col("tag", "text", true, false, false, false),
            ],
        };
        let err = build_insert(
            &MYSQL,
            &s,
            "defaulted",
            "defaulted",
            &[("tag".into(), Some("x".into()))],
            None,
        )
        .unwrap_err();
        assert!(err.contains("id"), "{err}");
        assert!(err.contains("server-generated"), "{err}");
    }

    #[test]
    fn the_same_non_identity_default_pk_is_fine_under_returning() {
        // Identical schema, MariaDB instead: RETURNING exists, so the same
        // "no source" shape that MySQL must refuse is simply read back.
        let s = TableSchema {
            columns: vec![
                col("id", "int", true, true, false, true),
                col("tag", "text", true, false, false, false),
            ],
        };
        let p = build_insert(
            &MARIADB,
            &s,
            "defaulted",
            "defaulted",
            &[("tag".into(), Some("x".into()))],
            None,
        )
        .unwrap();
        assert!(p.has_returning);
    }
}
