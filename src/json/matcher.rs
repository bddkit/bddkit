use regex::Regex;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Mismatch {
    pub path: String,
    pub expected: String,
    pub actual: String,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "  {}\n    expected: {}\n    actual:   {}",
            self.path, self.expected, self.actual
        )
    }
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 200 {
        format!("{}…", s.chars().take(200).collect::<String>())
    } else {
        s
    }
}

static MATCHER: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"^@(\w+)\((.*)\)$").expect("constant regex"));

/// Applies the `@name(args)` matcher. Returns `None` if the string is not a matcher.
fn apply_matcher(expected: &str, actual: &Value, path: &str) -> Option<Result<(), Mismatch>> {
    // The regex is compiled once: `cmp` recursively walks every node
    // of the response, and compiling it per node would cost more than the comparison itself.
    let caps = MATCHER.captures(expected)?;
    let name = caps.get(1)?.as_str();
    let arg = caps.get(2)?.as_str().trim();
    let fail = |exp: String| {
        Some(Err(Mismatch {
            path: path.into(),
            expected: exp,
            actual: short(actual),
        }))
    };

    match name {
        "variableType" => {
            let ok = match arg {
                "string" => actual.is_string(),
                "int" => actual.is_i64() || actual.is_u64(),
                "float" => actual.is_f64(),
                "bool" => actual.is_boolean(),
                "array" => actual.is_array(),
                "object" => actual.is_object(),
                "null" => actual.is_null(),
                _ => return fail(format!("@variableType({arg}) — unknown type")),
            };
            if ok {
                Some(Ok(()))
            } else {
                fail(format!("@variableType({arg})"))
            }
        }
        "arrayLength" => {
            let want: usize = match arg.parse() {
                Ok(n) => n,
                Err(_) => return fail("@arrayLength(<number>)".into()),
            };
            match actual.as_array() {
                Some(a) if a.len() == want => Some(Ok(())),
                Some(a) => fail(format!(
                    "@arrayLength({want}), actual length is {}",
                    a.len()
                )),
                None => fail(format!(
                    "@arrayLength({want}), but the value is not an array"
                )),
            }
        }
        "regExp" => {
            let pattern = arg
                .strip_prefix('/')
                .and_then(|s| s.rfind('/').map(|i| &s[..i]));
            let pattern = match pattern {
                Some(p) => p,
                None => return fail("@regExp(/pattern/)".into()),
            };
            let re = match Regex::new(pattern) {
                Ok(re) => re,
                Err(e) => return fail(format!("@regExp: invalid pattern: {e}")),
            };
            let s = match actual.as_str() {
                Some(s) => s,
                None => return fail(format!("@regExp({arg}), but the value is not a string")),
            };
            if re.is_match(s) {
                Some(Ok(()))
            } else {
                fail(format!("@regExp({arg})"))
            }
        }
        _ => fail(format!("unknown matcher @{name}")),
    }
}

fn cmp(actual: &Value, expected: &Value, path: &str, strict: bool) -> Result<(), Mismatch> {
    if let Some(s) = expected.as_str()
        && let Some(r) = apply_matcher(s, actual, path)
    {
        return r;
    }
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => {
            if strict && a.len() != e.len() {
                return Err(Mismatch {
                    path: path.into(),
                    expected: format!("object with {} fields", e.len()),
                    actual: format!("object with {} fields", a.len()),
                });
            }
            for (k, ev) in e {
                let child = format!("{path}.{k}");
                let av = a.get(k).ok_or_else(|| Mismatch {
                    path: child.clone(),
                    expected: short(ev),
                    actual: "field is missing".into(),
                })?;
                cmp(av, ev, &child, strict)?;
            }
            Ok(())
        }
        (Value::Array(a), Value::Array(e)) => {
            if strict {
                if a.len() != e.len() {
                    return Err(Mismatch {
                        path: path.into(),
                        expected: format!("array of length {}", e.len()),
                        actual: format!("array of length {}", a.len()),
                    });
                }
                for (i, ev) in e.iter().enumerate() {
                    cmp(&a[i], ev, &format!("{path}[{i}]"), strict)?;
                }
                return Ok(());
            }
            // Loose mode: every expected element must be found somewhere,
            // order does not matter, extra elements are allowed (Imbo semantics).
            for ev in e {
                let found = a.iter().any(|av| cmp(av, ev, "", false).is_ok());
                if !found {
                    return Err(Mismatch {
                        path: path.into(),
                        expected: format!("array contains {}", short(ev)),
                        actual: short(actual),
                    });
                }
            }
            Ok(())
        }
        _ => {
            if actual == expected {
                Ok(())
            } else {
                Err(Mismatch {
                    path: path.into(),
                    expected: short(expected),
                    actual: short(actual),
                })
            }
        }
    }
}

/// Subset match: objects allow extra fields, arrays check unordered containment.
pub fn contains(actual: &Value, expected: &Value) -> Result<(), Mismatch> {
    cmp(actual, expected, "root", false)
}

/// Full deep equality.
pub fn equals(actual: &Value, expected: &Value) -> Result<(), Mismatch> {
    cmp(actual, expected, "root", true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn contains_ignores_extra_object_fields() {
        let a = json!({"id": 1, "name": "x", "extra": true});
        assert!(contains(&a, &json!({"id": 1})).is_ok());
    }

    #[test]
    fn contains_fails_on_missing_field_naming_the_path() {
        let a = json!({"id": 1});
        let m = contains(&a, &json!({"data": {"id": 2}})).unwrap_err();
        assert_eq!(m.path, "root.data");
    }

    #[test]
    fn contains_allows_extra_array_elements() {
        let a = json!({"items": [{"id": 1}, {"id": 2}, {"id": 3}]});
        assert!(contains(&a, &json!({"items": [{"id": 1}]})).is_ok());
    }

    #[test]
    fn contains_ignores_array_order() {
        let a = json!({"items": [{"id": 3}, {"id": 1}]});
        assert!(contains(&a, &json!({"items": [{"id": 1}]})).is_ok());
    }

    #[test]
    fn contains_fails_when_element_absent() {
        let a = json!({"items": [{"id": 1}]});
        assert!(contains(&a, &json!({"items": [{"id": 9}]})).is_err());
    }

    #[test]
    fn equals_rejects_extra_object_fields() {
        let a = json!({"id": 1, "extra": true});
        assert!(equals(&a, &json!({"id": 1})).is_err());
    }

    #[test]
    fn equals_rejects_reordered_arrays() {
        let a = json!([1, 2]);
        assert!(equals(&a, &json!([2, 1])).is_err());
        assert!(equals(&a, &json!([1, 2])).is_ok());
    }

    #[test]
    fn matcher_variable_type() {
        assert!(contains(&json!({"a": "x"}), &json!({"a": "@variableType(string)"})).is_ok());
        assert!(contains(&json!({"a": 1}), &json!({"a": "@variableType(string)"})).is_err());
        assert!(contains(&json!({"a": null}), &json!({"a": "@variableType(null)"})).is_ok());
        assert!(contains(&json!({"a": [1]}), &json!({"a": "@variableType(array)"})).is_ok());
    }

    #[test]
    fn matcher_array_length() {
        assert!(contains(&json!({"a": [1, 2, 3]}), &json!({"a": "@arrayLength(3)"})).is_ok());
        let m = contains(&json!({"a": [1]}), &json!({"a": "@arrayLength(3)"})).unwrap_err();
        assert!(m.expected.contains("arrayLength"), "{m:?}");
    }

    #[test]
    fn matcher_reg_exp() {
        let a = json!({"name": "Supercompany42"});
        assert!(contains(&a, &json!({"name": "@regExp(/Supercompany[0-9]+/)"})).is_ok());
        assert!(contains(&a, &json!({"name": "@regExp(/^Other/)"})).is_err());
    }

    #[test]
    fn unknown_matcher_is_reported() {
        let m = contains(&json!({"a": 1}), &json!({"a": "@nope(1)"})).unwrap_err();
        assert!(m.expected.contains("unknown matcher"), "{m:?}");
    }

    #[test]
    fn literal_string_that_is_not_a_matcher_compares_as_is() {
        assert!(contains(&json!({"a": "@home"}), &json!({"a": "@home"})).is_ok());
    }

    #[test]
    fn mismatch_display_shows_path_expected_actual() {
        let m = contains(&json!({"code": 422}), &json!({"code": 200})).unwrap_err();
        let s = m.to_string();
        assert!(
            s.contains("root.code") && s.contains("expected") && s.contains("actual"),
            "{s}"
        );
    }
}
