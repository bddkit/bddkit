use serde_json::Value;

pub fn validate(path: &str) -> Result<(), String> {
    let path = path.trim();
    let path = path
        .strip_prefix("root.")
        .unwrap_or_else(|| if path == "root" { "" } else { path });
    for segment in path.split('.').filter(|segment| !segment.is_empty()) {
        split_indices(segment)?;
    }
    Ok(())
}

/// Reads a value by a path like `a.b[0].c`. An optional `root.` prefix
/// is allowed for readability. Full JSONPath isn't needed — the reference
/// only ever used this syntax.
pub fn read<'a>(root: &'a Value, path: &str) -> Result<&'a Value, String> {
    let path = path.trim();
    let path = path
        .strip_prefix("root.")
        .unwrap_or_else(|| if path == "root" { "" } else { path });
    let mut cur = root;
    let mut walked = String::from("root");

    for segment in path.split('.').filter(|s| !s.is_empty()) {
        let (name, indices) = split_indices(segment)?;
        if !name.is_empty() {
            walked.push('.');
            walked.push_str(name);
            cur = cur
                .get(name)
                .ok_or_else(|| format!("path {walked} does not exist"))?;
        }
        for i in indices {
            walked.push_str(&format!("[{i}]"));
            cur = cur
                .get(i)
                .ok_or_else(|| format!("path {walked} does not exist"))?;
        }
    }
    Ok(cur)
}

/// `items[0][1]` -> ("items", [0, 1])
fn split_indices(segment: &str) -> Result<(&str, Vec<usize>), String> {
    let open = match segment.find('[') {
        None => return Ok((segment, Vec::new())),
        Some(i) => i,
    };
    let (name, rest) = segment.split_at(open);
    let mut indices = Vec::new();
    let mut rest = rest;
    while !rest.is_empty() {
        let close = rest
            .find(']')
            .ok_or_else(|| format!("unclosed bracket in path segment {segment:?}"))?;
        let n: usize = rest[1..close]
            .parse()
            .map_err(|_| format!("index must be a number in path segment {segment:?}"))?;
        indices.push(n);
        rest = &rest[close + 1..];
        if !rest.is_empty() && !rest.starts_with('[') {
            return Err(format!(
                "trailing garbage after index in path segment {segment:?}"
            ));
        }
    }
    Ok((name, indices))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc() -> Value {
        json!({
            "id": 7,
            "data": { "items": [ {"email": "a@b.net"}, {"email": "c@d.net"} ] },
            "list": [10, 20, 30]
        })
    }

    #[test]
    fn reads_top_level_key() {
        assert_eq!(read(&doc(), "id").unwrap(), &json!(7));
    }

    #[test]
    fn reads_nested_key() {
        assert_eq!(
            read(&doc(), "data.items[1].email").unwrap(),
            &json!("c@d.net")
        );
    }

    #[test]
    fn reads_array_index() {
        assert_eq!(read(&doc(), "list[2]").unwrap(), &json!(30));
    }

    #[test]
    fn accepts_root_prefix() {
        assert_eq!(read(&doc(), "root.id").unwrap(), &json!(7));
    }

    #[test]
    fn bare_root_returns_whole_document() {
        assert_eq!(read(&doc(), "root").unwrap(), &doc());
    }

    #[test]
    fn leading_index_on_array_root() {
        let arr = json!([{"id": 1}, {"id": 2}]);
        assert_eq!(read(&arr, "[1].id").unwrap(), &json!(2));
    }

    #[test]
    fn missing_key_reports_the_walked_path() {
        let err = read(&doc(), "data.items[0].name").unwrap_err();
        assert!(err.contains("root.data.items[0].name"), "{err}");
    }

    #[test]
    fn index_out_of_range_is_an_error() {
        assert!(read(&doc(), "list[9]").is_err());
    }

    #[test]
    fn non_numeric_index_is_an_error() {
        assert!(read(&doc(), "list[x]").is_err());
    }

    #[test]
    fn validation_rejects_a_non_numeric_index_without_reading_a_document() {
        assert!(validate("data.items[x].id").is_err());
    }

    #[test]
    fn validation_accepts_a_path_that_may_be_absent_from_a_document() {
        assert!(validate("data.items[0].missing").is_ok());
    }
}
