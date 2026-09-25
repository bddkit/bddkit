use crate::json::path;
use crate::polling::{AttemptError, AttemptResult};
use crate::world::World;
use aes::cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use super::assert;
use super::markup;
use regex::Regex;

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_aes256_key(key_hex: &str) -> Result<[u8; 32], String> {
    if key_hex.len() != 64 {
        return Err("AES-256 key must be exactly 64 hexadecimal characters".to_string());
    }

    let mut key = [0; 32];
    for (byte, pair) in key.iter_mut().zip(key_hex.as_bytes().as_chunks::<2>().0) {
        let high = hex_nibble(pair[0])
            .ok_or_else(|| "AES-256 key must contain only hexadecimal characters".to_string())?;
        let low = hex_nibble(pair[1])
            .ok_or_else(|| "AES-256 key must contain only hexadecimal characters".to_string())?;
        *byte = (high << 4) | low;
    }
    Ok(key)
}

fn encrypt_aes256_cbc(plaintext: &str, key: &[u8; 32], iv: &[u8; 16]) -> String {
    let ciphertext = cbc::Encryptor::<aes::Aes256>::new(key.into(), iv.into())
        .encrypt_padded_vec::<Pkcs7>(plaintext.as_bytes());
    STANDARD.encode(ciphertext)
}

pub fn encrypt_with_aes(
    world: &mut World,
    plaintext: &str,
    key_hex: &str,
    prefix: &str,
) -> Result<(), String> {
    let key = decode_aes256_key(key_hex)?;
    let mut iv = [0; 16];
    getrandom::fill(&mut iv).map_err(|_| "failed to read system entropy".to_string())?;
    let ciphertext = encrypt_aes256_cbc(plaintext, &key, &iv);
    let iv_hex = iv.iter().map(|byte| format!("{byte:02x}")).collect();

    world.vars.set(&format!("{prefix}_ciphertext"), ciphertext);
    world.vars.set(&format!("{prefix}_ivHex"), iv_hex);
    Ok(())
}

pub fn set_variable(w: &mut World, name: &str, value: &str, global: bool) -> Result<(), String> {
    if global {
        w.vars.set_global(name, value.to_string());
    } else {
        w.vars.set(name, value.to_string());
    }
    Ok(())
}

/// Scalar values are stored as-is; strings without JSON quotes.
fn scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub fn extract_from_json(w: &mut World, p: &str, name: &str, global: bool) -> Result<(), String> {
    let ex = w.http.last().ok_or("no request has been sent yet")?;
    let v = ex.json()?;
    let value = scalar(path::read(&v, p)?);
    set_variable(w, name, &value, global)
}

pub fn extract_json_from_variable(
    w: &mut World,
    p: &str,
    source: &str,
    name: &str,
    global: bool,
) -> Result<(), String> {
    let v = variable_json(w, source).map_err(AttemptError::into_message)?;
    let value = scalar(path::read(&v, p)?);
    set_variable(w, name, &value, global)
}

/// The first capture group of the first match — never the whole match, so a
/// pattern without a group is an error rather than a silent change of meaning.
pub fn extract_regex_from_variable(
    w: &mut World,
    pattern: &str,
    source: &str,
    name: &str,
    global: bool,
) -> Result<(), String> {
    let re = Regex::new(pattern).map_err(|e| format!("invalid regex {pattern:?}: {e}"))?;
    if re.captures_len() < 2 {
        return Err(format!("regex {pattern:?} has no capture group to extract"));
    }
    let text = variable(w, source).map_err(AttemptError::into_message)?;
    let caps = re
        .captures(text)
        .ok_or_else(|| format!("regex {pattern:?} does not match variable {source:?}: {text:?}"))?;
    let value = caps
        .get(1)
        .ok_or_else(|| format!("capture group 1 of {pattern:?} did not take part in the match"))?
        .as_str()
        .to_string();
    set_variable(w, name, &value, global)
}

pub fn extract_from_cookies(
    w: &mut World,
    cookie: &str,
    name: &str,
    global: bool,
) -> Result<(), String> {
    let ex = w.http.last().ok_or("no request has been sent yet")?;
    let value = ex
        .set_cookie(cookie)
        .ok_or_else(|| format!("cookie {cookie:?} not found in the response"))?;
    if global {
        w.vars.set_global(name, value);
    } else {
        w.vars.set(name, value);
    }
    Ok(())
}

pub fn extract_from_markup(w: &mut World, selector: &str, name: &str) -> Result<(), String> {
    let ex = w.http.last().ok_or("no request has been sent yet")?;
    let kind = markup::classify(markup::content_type(&ex.resp_headers));
    let value = markup::select(kind, &ex.body, selector, false).map_err(|e| e.to_string())?;
    w.vars.set(name, value);
    Ok(())
}

fn variable<'a>(w: &'a World, name: &str) -> Result<&'a str, AttemptError> {
    w.vars
        .get(name)
        .ok_or_else(|| AttemptError::Fatal(format!("variable {name:?} is not set")))
}

fn variable_json(w: &World, name: &str) -> Result<serde_json::Value, AttemptError> {
    serde_json::from_str(variable(w, name)?)
        .map_err(|e| AttemptError::Fatal(format!("variable {name:?} is not valid JSON: {e}")))
}

/// Nothing runs between two attempts, so a variable cannot change under an
/// armed `I expect the next assertion to pass …`: a mismatch is final, and
/// polling it again would only burn the timeout to report the same thing.
fn settled(result: AttemptResult) -> AttemptResult {
    result.map_err(|e| AttemptError::Fatal(e.into_message()))
}

pub fn variable_equals(w: &World, name: &str, expected: &str, negate: bool) -> AttemptResult {
    let got = variable(w, name)?;
    match (got == expected, negate) {
        (true, false) | (false, true) => Ok(()),
        (false, false) => Err(AttemptError::Fatal(format!(
            "    expected: {expected}\n    actual:   {got}"
        ))),
        (true, true) => Err(AttemptError::Fatal(format!(
            "value must not equal {expected:?}, but it does"
        ))),
    }
}

fn subject(name: &str) -> String {
    format!("variable {name:?}")
}

pub fn variable_contains(w: &World, name: &str, needle: &str, negate: bool) -> AttemptResult {
    let text = variable(w, name)?;
    settled(assert::check_contains(&subject(name), text, needle, negate))
}

pub fn variable_matches(w: &World, name: &str, pattern: &str, negate: bool) -> AttemptResult {
    let text = variable(w, name)?;
    settled(assert::check_matches(&subject(name), text, pattern, negate))
}

pub fn variable_empty(w: &World, name: &str) -> AttemptResult {
    settled(assert::check_empty(&subject(name), variable(w, name)?))
}

fn json_check(
    w: &World,
    name: &str,
    docstring: Option<&String>,
    check: impl FnOnce(&serde_json::Value, &serde_json::Value) -> AttemptResult,
) -> AttemptResult {
    let expected = assert::expected_json(docstring)?;
    settled(check(&variable_json(w, name)?, &expected))
}

pub fn variable_contains_json(w: &World, name: &str, docstring: Option<&String>) -> AttemptResult {
    json_check(w, name, docstring, assert::check_contains_json)
}

pub fn variable_equals_json(w: &World, name: &str, docstring: Option<&String>) -> AttemptResult {
    json_check(w, name, docstring, assert::check_equals_json)
}

pub fn variable_not_contains_json(
    w: &World,
    name: &str,
    docstring: Option<&String>,
) -> AttemptResult {
    json_check(w, name, docstring, |actual, expected| {
        assert::check_not_contains_json(&subject(name), actual, expected)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::Pkcs7};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn world() -> World {
        let resources = HashMap::from([(
            "main".to_string(),
            crate::http::ApiResource::new(
                "http://x.local",
                5,
                Vec::new(),
                crate::options::Options::default(),
            )
            .expect("valid base URL"),
        )]);
        let apis = Arc::new(
            crate::http::Apis::new(resources, Some("main".to_string()))
                .expect("default API exists"),
        );
        World::new(
            apis,
            Arc::new(crate::unique::Generator::new()),
            crate::db::DbHandle::new(None, String::new()),
            None,
            None,
            crate::options::Options::default(),
        )
    }

    #[test]
    fn aes256_cbc_matches_a_known_fixed_vector() {
        let key =
            decode_aes256_key("000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F")
                .expect("valid key");
        let iv = [
            0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
            0x1e, 0x1f,
        ];

        assert_eq!(
            encrypt_aes256_cbc("555555", &key, &iv),
            "NHHyjRAmNadQ7tSNhxFESA=="
        );
    }

    #[test]
    fn aes_step_exports_decryptable_ciphertext_and_lowercase_iv() {
        let mut world = world();
        let key = [0x11; 32];
        encrypt_with_aes(&mut world, "plain text", &"11".repeat(32), "otp")
            .expect("encryption succeeds");

        let iv_hex = world.vars.get("otp_ivHex").expect("IV exported");
        assert_eq!(iv_hex.len(), 32);
        assert!(
            iv_hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
        let iv: Vec<u8> = iv_hex
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII hex"), 16)
                    .expect("valid hex")
            })
            .collect();
        let ciphertext = STANDARD
            .decode(
                world
                    .vars
                    .get("otp_ciphertext")
                    .expect("ciphertext exported"),
            )
            .expect("valid Base64");
        let plaintext = cbc::Decryptor::<aes::Aes256>::new_from_slices(&key, &iv)
            .expect("fixed key and IV sizes")
            .decrypt_padded_vec::<Pkcs7>(&ciphertext)
            .expect("valid padding");

        assert_eq!(plaintext, b"plain text");
    }

    #[test]
    fn aes_step_rejects_non_hex_key_without_exports() {
        let mut world = world();
        let error = encrypt_with_aes(&mut world, "secret", &"z0".repeat(32), "otp")
            .expect_err("non-hex key is rejected");

        assert!(error.contains("hexadecimal"), "{error}");
        assert_eq!(world.vars.get("otp_ciphertext"), None);
        assert_eq!(world.vars.get("otp_ivHex"), None);
    }

    #[test]
    fn aes_step_rejects_non_256_bit_key_without_exports() {
        let mut world = world();
        let error = encrypt_with_aes(&mut world, "secret", &"11".repeat(16), "otp")
            .expect_err("short key is rejected");

        assert!(error.contains("64"), "{error}");
        assert_eq!(world.vars.get("otp_ciphertext"), None);
        assert_eq!(world.vars.get("otp_ivHex"), None);
    }

    #[test]
    fn extract_from_markup_writes_the_selected_text() {
        let mut w = world();
        w.http.store_test_exchange(crate::http::Exchange {
            method: "GET".to_string(),
            url: "http://x.local/".to_string(),
            req_headers: Vec::new(),
            req_body: None,
            status: 200,
            resp_headers: vec![("content-type".to_string(), "text/html".to_string())],
            body: r#"<html><body><h1 id="title">Hi</h1></body></html>"#.to_string(),
        });

        extract_from_markup(&mut w, "h1#title", "pageTitle").expect("extraction succeeds");
        assert_eq!(w.vars.get("pageTitle"), Some("Hi"));
    }

    #[test]
    fn extract_from_markup_propagates_the_error_without_setting_the_variable() {
        let mut w = world();
        w.http.store_test_exchange(crate::http::Exchange {
            method: "GET".to_string(),
            url: "http://x.local/".to_string(),
            req_headers: Vec::new(),
            req_body: None,
            status: 200,
            resp_headers: vec![("content-type".to_string(), "text/html".to_string())],
            body: r#"<html><body></body></html>"#.to_string(),
        });

        let err = extract_from_markup(&mut w, ".missing", "pageTitle").unwrap_err();
        assert!(err.contains("missing"), "{err}");
        assert_eq!(w.vars.get("pageTitle"), None);
    }

    fn world_with(name: &str, value: &str) -> World {
        let mut w = world();
        w.vars.set(name, value.to_string());
        w
    }

    #[test]
    fn a_json_step_over_a_non_json_variable_names_the_variable_and_the_parse_error() {
        let w = world_with("out", "not json");
        let doc = r#"{"a": 1}"#.to_string();
        let Err(AttemptError::Fatal(msg)) = variable_contains_json(&w, "out", Some(&doc)) else {
            panic!("a non-JSON variable must be fatal");
        };
        assert!(
            msg.contains(r#"variable "out" is not valid JSON: expected"#),
            "{msg}"
        );
    }

    #[test]
    fn contains_json_over_a_multi_line_variable_uses_the_body_matcher() {
        let w = world_with("out", "{\n  \"id\": 7,\n  \"tags\": [\"a\", \"b\"]\n}\n");
        let doc = r#"{"id": "@variableType(int)", "tags": ["b"]}"#.to_string();
        assert_eq!(variable_contains_json(&w, "out", Some(&doc)), Ok(()));
    }

    #[test]
    fn a_text_mismatch_over_a_variable_is_fatal_not_not_yet() {
        let w = world_with("id", "abc");
        assert!(matches!(
            variable_matches(&w, "id", r"^\d+$", false),
            Err(AttemptError::Fatal(_))
        ));
    }

    #[test]
    fn variable_equals_mismatch_is_fatal() {
        let w = world_with("x", "1");
        assert!(matches!(
            variable_equals(&w, "x", "2", false),
            Err(AttemptError::Fatal(_))
        ));
    }

    #[test]
    fn an_unset_variable_is_fatal_with_the_variable_equals_message() {
        let Err(AttemptError::Fatal(msg)) = variable_empty(&world(), "nope") else {
            panic!("an unset variable must be fatal");
        };
        assert_eq!(msg, r#"variable "nope" is not set"#);
    }

    #[test]
    fn extract_regex_takes_the_first_group_of_the_first_match() {
        let mut w = world_with("out", "id=12 id=34");
        extract_regex_from_variable(&mut w, r"id=(\d+)", "out", "id", false).expect("matches");
        assert_eq!(w.vars.get("id"), Some("12"));
    }

    #[test]
    fn extract_regex_without_a_capture_group_is_refused() {
        let mut w = world_with("out", "id=12");
        let err = extract_regex_from_variable(&mut w, r"id=\d+", "out", "id", false).unwrap_err();
        assert!(err.contains("no capture group"), "{err}");
    }

    /// Every variable already outlives its scenario, so only a macro frame
    /// can tell `global` apart: what is not global is gone once it pops.
    #[test]
    fn extract_json_from_variable_global_outlives_a_macro_frame() {
        let mut w = world_with("out", r#"{"data": {"id": 5}}"#);
        w.vars.push_frame();
        extract_json_from_variable(&mut w, "data.id", "out", "kept", true).expect("path exists");
        extract_json_from_variable(&mut w, "data.id", "out", "dropped", false)
            .expect("path exists");
        w.vars.pop_frame(&[]).expect("a frame was pushed");
        assert_eq!(
            (w.vars.get("kept"), w.vars.get("dropped")),
            (Some("5"), None)
        );
    }

    #[test]
    fn extract_regex_from_variable_global_outlives_a_macro_frame() {
        let mut w = world_with("out", "id=12");
        w.vars.push_frame();
        extract_regex_from_variable(&mut w, r"id=(\d+)", "out", "kept", true).expect("matches");
        extract_regex_from_variable(&mut w, r"id=(\d+)", "out", "dropped", false).expect("matches");
        w.vars.pop_frame(&[]).expect("a frame was pushed");
        assert_eq!(
            (w.vars.get("kept"), w.vars.get("dropped")),
            (Some("12"), None)
        );
    }
}
