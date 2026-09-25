use crate::db::plan::{Column, TableSchema};
use crate::db::platform::Platform;
use crate::db::reference::TableRef;
use crate::db::{bind_all, text_col};
use sqlx::any::AnyRow;
use sqlx::{AnyPool, Row};

/// The four boolean columns come back as `int` (see `Platform::introspect`):
/// `AnyRow`'s `bool` decoding differs per driver, and a later engine's
/// `information_schema` returns a different boolean representation again —
/// reading an integer and comparing `!= 0` is portable across both.
fn flag(r: &AnyRow, col: &str) -> Result<bool, String> {
    let v: i32 = r.try_get(col).map_err(|e| format!("reading {col}: {e}"))?;
    Ok(v != 0)
}

pub async fn introspect(
    pool: &AnyPool,
    platform: &dyn Platform,
    tref: &TableRef,
) -> Result<TableSchema, String> {
    let (sql, binds) = platform.introspect(tref);
    let rows = bind_all(sqlx::query(&sql), &binds)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("introspection of {}: {e}", tref.sql_name()))?;
    if rows.is_empty() {
        return Err(format!(
            "table {} not found or inaccessible",
            tref.sql_name()
        ));
    }
    let mut columns = Vec::with_capacity(rows.len());
    for r in &rows {
        columns.push(Column {
            name: text_col(r, "name")?
                .ok_or_else(|| "reading name: unexpected NULL".to_string())?,
            type_name: text_col(r, "type_name")?
                .ok_or_else(|| "reading type_name: unexpected NULL".to_string())?,
            length: r
                .try_get("length")
                .map_err(|e| format!("reading length: {e}"))?,
            not_null: flag(r, "not_null")?,
            has_default: flag(r, "has_default")?,
            is_identity: flag(r, "is_identity")?,
            is_pk: flag(r, "is_pk")?,
        });
    }
    Ok(TableSchema { columns })
}
