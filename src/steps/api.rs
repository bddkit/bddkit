use crate::world::World;

pub fn set_header(w: &mut World, name: &str, value: &str) -> Result<(), String> {
    w.http.set_header(name, value);
    Ok(())
}

/// Argument order matches `set_header`. The step text names them in
/// reverse order — the swap happens on the dispatcher side, in one place.
pub fn add_header(w: &mut World, name: &str, value: &str) -> Result<(), String> {
    w.http.add_header(name, value);
    Ok(())
}

pub fn set_query(w: &mut World, name: &str, value: &str) -> Result<(), String> {
    w.http.set_query(name, value);
    Ok(())
}

pub fn set_body(w: &mut World, docstring: Option<&String>) -> Result<(), String> {
    let b = docstring.ok_or("step `the request body is:` requires a doc string")?;
    w.http.set_body(b.clone());
    Ok(())
}

pub fn clear_body(w: &mut World) -> Result<(), String> {
    w.http.clear_body();
    Ok(())
}

pub fn set_form(w: &mut World, table: Option<&Vec<Vec<String>>>) -> Result<(), String> {
    let rows = table.ok_or("step `the request form parameters are:` requires a table")?;
    let head = rows.first().ok_or("table is empty")?;
    if head.len() != 2 || head[0] != "name" || head[1] != "value" {
        return Err("table must have exactly two columns: name | value".into());
    }
    let pairs = rows[1..]
        .iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    w.http.set_form(pairs);
    Ok(())
}

/// `base_dir` is the directory of the file the step is written in (the macro
/// file for a macro body), the same anchor `I include` uses. The file is read
/// here, not at send time, so a missing one fails at the step and a retried
/// assertion replays the content the step saw.
pub fn attach_file(
    w: &mut World,
    base_dir: &std::path::Path,
    file: &str,
    part: &str,
) -> Result<(), String> {
    let path = crate::include::join(file, base_dir);
    let bytes = std::fs::read(&path).map_err(|e| {
        format!(
            "I attach the file {file:?}: cannot read {}: {e}",
            crate::feature::display_path(&path)
        )
    })?;
    w.http.attach_file(part, &path, bytes);
    Ok(())
}

pub async fn request(w: &mut World, path: &str, method: &str) -> Result<(), String> {
    w.http.send(path, method).await
}

/// Records Hawk credentials for the next `I request ...` only; it does not
/// sign anything itself. `HttpState::send` consumes them.
pub fn sign_next_request_with_hawk(w: &mut World, id: &str, key: &str) -> Result<(), String> {
    w.http.sign_next(id, key);
    Ok(())
}

/// Switches the scenario's current API. An error if the name is not declared.
pub fn use_api(w: &mut World, name: &str) -> Result<(), String> {
    w.http.use_api(name)
}
