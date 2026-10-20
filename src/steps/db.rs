use crate::db::{ops, value};
use crate::failure::StepError;
use crate::polling::{AttemptError, AttemptResult};
use crate::world::World;

/// Switches the scenario's current connection. An error if the name is unknown.
pub fn use_connection(w: &mut World, name: &str) -> Result<(), StepError> {
    w.db.set_current(name).map_err(StepError::input)
}

pub fn debug_on(w: &mut World) -> Result<(), StepError> {
    w.debug = true;
    Ok(())
}

pub fn debug_off(w: &mut World) -> Result<(), StepError> {
    w.debug = false;
    Ok(())
}

pub async fn have_with(w: &mut World, table: &str, kv: &str) -> Result<(), StepError> {
    let values = value::parse_oneliner(kv).map_err(StepError::input)?;
    ops::insert(w, table, &values, None).await
}

pub async fn have_where(
    w: &mut World,
    table: &str,
    rows: Option<&Vec<Vec<String>>>,
) -> Result<(), StepError> {
    let rows = rows.ok_or_else(|| StepError::input("step requires a table"))?;
    let sets = value::pairs_from_wide(rows).map_err(StepError::input)?;
    for (i, values) in sets.iter().enumerate() {
        ops::insert(w, table, values, Some(i)).await?;
    }
    Ok(())
}

pub async fn have_multi(w: &mut World, rows: Option<&Vec<Vec<String>>>) -> Result<(), StepError> {
    let rows = rows.ok_or_else(|| StepError::input("step requires a table"))?;
    // The first row is the header (| table | record |), then (table, kv) pairs.
    for row in rows.iter().skip(1) {
        let table = row
            .first()
            .ok_or_else(|| StepError::input("empty table row"))?;
        let kv = row.get(1).map(String::as_str).unwrap_or("");
        let values = value::parse_oneliner(kv).map_err(StepError::input)?;
        ops::insert(w, table, &values, None).await?;
    }
    Ok(())
}

pub async fn update(w: &mut World, table: &str, set: &str, where_: &str) -> Result<(), StepError> {
    ops::update(w, table, set, where_).await
}

pub async fn delete_where(w: &mut World, table: &str, where_: &str) -> Result<(), StepError> {
    ops::delete(w, table, where_).await
}

pub async fn delete_all(w: &mut World, table: &str) -> Result<(), StepError> {
    ops::delete_all(w, table).await
}

pub async fn extract_from_db(
    w: &mut World,
    column: &str,
    table: &str,
    where_: &str,
    var: &str,
) -> Result<(), StepError> {
    ops::extract(w, column, table, where_, var).await
}

async fn assert_presence(
    w: &mut World,
    table: &str,
    pairs: &[(String, Option<String>)],
    negate: bool,
) -> AttemptResult {
    let found = ops::exists(w, table, pairs)
        .await
        .map_err(AttemptError::Fatal)?;
    match (found, negate) {
        (true, false) | (false, true) => Ok(()),
        (false, false) => Err(AttemptError::not_yet(format!(
            "expected a row in {table}, but there is none"
        ))),
        (true, true) => Err(AttemptError::not_yet(format!(
            "a row in {table} exists, but must not"
        ))),
    }
}

pub async fn should_have(w: &mut World, table: &str, kv: &str) -> AttemptResult {
    let pairs = value::parse_oneliner(kv).map_err(AttemptError::input)?;
    assert_presence(w, table, &pairs, false).await
}

pub async fn should_not_have(w: &mut World, table: &str, kv: &str) -> AttemptResult {
    let pairs = value::parse_oneliner(kv).map_err(AttemptError::input)?;
    assert_presence(w, table, &pairs, true).await
}

pub async fn should_have_table(
    w: &mut World,
    table: &str,
    rows: Option<&Vec<Vec<String>>>,
) -> AttemptResult {
    let rows = rows.ok_or_else(|| AttemptError::input("step requires a table".to_string()))?;
    let pairs = value::pairs_from_tall(rows).map_err(AttemptError::input)?;
    assert_presence(w, table, &pairs, false).await
}

pub async fn should_not_have_table(
    w: &mut World,
    table: &str,
    rows: Option<&Vec<Vec<String>>>,
) -> AttemptResult {
    let rows = rows.ok_or_else(|| AttemptError::input("step requires a table".to_string()))?;
    let pairs = value::pairs_from_tall(rows).map_err(AttemptError::input)?;
    assert_presence(w, table, &pairs, true).await
}

pub async fn call_procedure(w: &mut World, name: &str, args: &str) -> Result<(), StepError> {
    ops::call_procedure(w, name, args).await
}

pub async fn call_function(
    w: &mut World,
    name: &str,
    args: &str,
    var: &str,
) -> Result<(), StepError> {
    ops::call_function(w, name, args, var).await
}

pub async fn get_sequence(w: &mut World, seq: &str, var: &str) -> Result<(), StepError> {
    ops::next_sequence(w, seq, var).await
}
