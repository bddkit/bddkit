use crate::unique::{Generator, parse_kind};
use std::collections::HashMap;
use std::sync::LazyLock;

/// Frame stack: reads look downward, writes always go to the top.
/// The runner creates one frame per feature file; macro frames arrive in M3.
pub struct VarStack {
    frames: Vec<HashMap<String, String>>,
    globals: HashMap<String, String>,
}

impl VarStack {
    pub fn new() -> Self {
        Self {
            frames: vec![HashMap::new()],
            globals: HashMap::new(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        for frame in self.frames.iter().rev() {
            if let Some(v) = frame.get(name) {
                return Some(v.as_str());
            }
        }
        self.globals.get(name).map(String::as_str)
    }

    pub fn set(&mut self, name: &str, value: String) {
        self.frames
            .last_mut()
            .expect("the frame stack is never empty")
            .insert(name.to_string(), value);
    }

    pub fn set_global(&mut self, name: &str, value: String) {
        self.globals.insert(name.to_string(), value);
    }

    #[cfg(test)]
    pub fn remove(&mut self, name: &str) {
        for frame in self.frames.iter_mut() {
            frame.remove(name);
        }
        self.globals.remove(name);
    }

    pub fn push_frame(&mut self) {
        self.frames.push(HashMap::new());
    }

    /// Pops the top frame, copying into the parent only what `exports` lists.
    /// Supports a glob of the form `last_insert_id_*`.
    pub fn pop_frame(&mut self, exports: &[String]) -> Result<(), String> {
        // Check BEFORE popping: `Vec::pop` is irreversible, and the root frame must not be popped.
        if self.frames.len() <= 1 {
            return Err("cannot pop the root frame".into());
        }
        let top = self
            .frames
            .pop()
            .ok_or("attempted to pop a frame from an empty stack")?;
        let mut exported: Vec<(String, String)> = Vec::new();
        for pattern in exports {
            if let Some(prefix) = pattern.strip_suffix('*') {
                for (k, v) in &top {
                    if k.starts_with(prefix) {
                        exported.push((k.clone(), v.clone()));
                    }
                }
            } else {
                let v = top.get(pattern).ok_or_else(|| {
                    format!("macro declared export {pattern:?}, but the variable is not set")
                })?;
                exported.push((pattern.clone(), v.clone()));
            }
        }
        for (k, v) in exported {
            self.set(&k, v);
        }
        Ok(())
    }

    pub fn all_vars(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        for (name, value) in &self.globals {
            result.push((name.clone(), value.clone()));
        }
        for frame in &self.frames {
            for (name, value) in frame {
                result.push((name.clone(), value.clone()));
            }
        }
        result
    }
}

impl Default for VarStack {
    fn default() -> Self {
        Self::new()
    }
}

/// SQL NULL sentinel. NUL bytes never occur in `.feature` text, so the
/// value can never be confused with real data. `<<null>>` expands into it,
/// and the DB layer reads it back as NULL — a single point of substitution is kept.
pub const NULL_SENTINEL: &str = "\u{0}__bddkit_null__\u{0}";

static SLOT: LazyLock<regex::Regex> = LazyLock::new(|| {
    // <<name>> or <<function(arguments)>>
    regex::Regex::new(r"(?u)<<([^\W\d]\w*)(?:\(([^)]*)\))?>>").expect("constant regex")
});

/// Substitutes `<<…>>`. Applied ONLY to arguments, doc strings, and table
/// cells — never to the whole step text, or pre-run validation would be impossible.
pub fn interpolate(input: &str, vars: &VarStack, generator: &Generator) -> Result<String, String> {
    let mut out = String::with_capacity(input.len());
    let mut last = 0usize;
    for c in SLOT.captures_iter(input) {
        let m = c.get(0).expect("group 0 always exists");
        out.push_str(&input[last..m.start()]);
        let name = c.get(1).expect("group 1 is required").as_str();
        let args = c.get(2).map(|g| g.as_str());
        let value = match (name, args) {
            ("unique", a) => generator.next(parse_kind(a.unwrap_or(""))?),
            ("uuid", _) => uuid::Uuid::now_v7().to_string(),
            ("null", None) => NULL_SENTINEL.to_string(),
            // The prefix every `<<unique()>>` token already carries, written
            // down so a suite can delete exactly what this run created.
            ("run_id", None) => generator.run_id().to_string(),
            (n, None) => vars
                .get(n)
                .ok_or_else(|| format!("variable {n:?} is not set"))?
                .to_string(),
            (n, Some(_)) => return Err(format!("unknown function {n:?}")),
        };
        out.push_str(&value);
        last = m.end();
    }
    out.push_str(&input[last..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_down_the_stack_writes_to_top() {
        let mut s = VarStack::new();
        s.set("a", "1".into());
        s.push_frame();
        assert_eq!(s.get("a"), Some("1"), "the inner frame sees the outer one");
        s.set("b", "2".into());
        s.pop_frame(&[]).unwrap();
        assert_eq!(s.get("b"), None, "an unexported variable does not survive");
    }

    #[test]
    fn inner_frame_shadows_without_clobbering() {
        let mut s = VarStack::new();
        s.set("t", "outer".into());
        s.push_frame();
        s.set("t", "inner".into());
        assert_eq!(s.get("t"), Some("inner"));
        s.pop_frame(&[]).unwrap();
        assert_eq!(
            s.get("t"),
            Some("outer"),
            "a macro must not clobber the scenario's variable"
        );
    }

    #[test]
    fn exports_named_variables() {
        let mut s = VarStack::new();
        s.push_frame();
        s.set("companyId", "42".into());
        s.set("tmp", "garbage".into());
        s.pop_frame(&["companyId".to_string()]).unwrap();
        assert_eq!(s.get("companyId"), Some("42"));
        assert_eq!(s.get("tmp"), None);
    }

    #[test]
    fn exports_glob_pattern() {
        let mut s = VarStack::new();
        s.push_frame();
        s.set("last_insert_id_users", "7".into());
        s.set("last_insert_id_companies", "8".into());
        s.set("other", "x".into());
        s.pop_frame(&["last_insert_id_*".to_string()]).unwrap();
        assert_eq!(s.get("last_insert_id_users"), Some("7"));
        assert_eq!(s.get("last_insert_id_companies"), Some("8"));
        assert_eq!(s.get("other"), None);
    }

    #[test]
    fn missing_export_fails_loudly() {
        let mut s = VarStack::new();
        s.push_frame();
        let err = s.pop_frame(&["companyId".to_string()]).unwrap_err();
        assert!(
            err.contains("companyId"),
            "the error must name the variable: {err}"
        );
    }

    #[test]
    fn globals_survive_frame_pop() {
        let mut s = VarStack::new();
        s.push_frame();
        s.set_global("token", "abc".into());
        s.pop_frame(&[]).unwrap();
        assert_eq!(s.get("token"), Some("abc"));
    }

    #[test]
    fn frame_shadows_global() {
        let mut s = VarStack::new();
        s.set_global("x", "global".into());
        s.set("x", "framed".into());
        assert_eq!(s.get("x"), Some("framed"));
    }

    #[test]
    fn pop_frame_refuses_to_remove_root_frame() {
        let mut s = VarStack::new();
        s.set("a", "1".into());
        let err = s.pop_frame(&[]).unwrap_err();
        assert!(err.contains("root"), "the error must explain why: {err}");
        // the stack must stay untouched: data is in place, set() does not panic
        assert_eq!(
            s.get("a"),
            Some("1"),
            "the root frame must not be damaged by the refusal"
        );
        s.set("b", "2".into());
        assert_eq!(
            s.get("b"),
            Some("2"),
            "the stack must stay usable after the refusal"
        );
    }

    #[test]
    fn remove_clears_frame_and_global() {
        let mut s = VarStack::new();
        s.set("frameVar", "framed".into());
        s.set_global("globalVar", "global".into());
        s.remove("frameVar");
        s.remove("globalVar");
        assert_eq!(
            s.get("frameVar"),
            None,
            "remove must clear the frame variable"
        );
        assert_eq!(
            s.get("globalVar"),
            None,
            "remove must clear the global variable"
        );
    }

    fn generator() -> Generator {
        Generator::new()
    }

    #[test]
    fn substitutes_variable() {
        let mut s = VarStack::new();
        s.set("email", "a@b.net".into());
        assert_eq!(
            interpolate("to:<<email>>!", &s, &generator()).unwrap(),
            "to:a@b.net!"
        );
    }

    #[test]
    fn substitutes_several_slots_in_one_string() {
        let mut s = VarStack::new();
        s.set("a", "1".into());
        s.set("b", "2".into());
        assert_eq!(interpolate("<<a>>-<<b>>", &s, &generator()).unwrap(), "1-2");
    }

    #[test]
    fn leaves_text_without_slots_untouched() {
        let s = VarStack::new();
        assert_eq!(
            interpolate("plain text", &s, &generator()).unwrap(),
            "plain text"
        );
    }

    #[test]
    fn unknown_variable_is_an_error_naming_it() {
        let s = VarStack::new();
        let err = interpolate("<<missing>>", &s, &generator()).unwrap_err();
        assert!(err.contains("is not set"), "{err}");
    }

    #[test]
    fn unique_calls_differ_within_one_string() {
        let s = VarStack::new();
        let out = interpolate("<<unique(token)>>|<<unique(token)>>", &s, &generator()).unwrap();
        let (a, b) = out.split_once('|').unwrap();
        assert_ne!(a, b, "each call must produce a new value");
    }

    #[test]
    fn unique_without_argument_is_token() {
        let s = VarStack::new();
        let out = interpolate("<<unique()>>", &s, &generator()).unwrap();
        assert!(out.starts_with('u'), "{out}");
    }

    #[test]
    fn unknown_unique_kind_is_rejected() {
        let s = VarStack::new();
        assert!(interpolate("<<unique(banana)>>", &s, &generator()).is_err());
    }

    #[test]
    fn uuid_function_produces_v7() {
        let s = VarStack::new();
        let out = interpolate("<<uuid()>>", &s, &generator()).unwrap();
        let parsed = uuid::Uuid::parse_str(&out).unwrap();
        assert_eq!(parsed.get_version_num(), 7);
    }

    #[test]
    fn unknown_function_is_rejected() {
        let s = VarStack::new();
        let err = interpolate("<<microtime(true)>>", &s, &generator()).unwrap_err();
        assert!(err.contains("unknown function"), "{err}");
    }

    #[test]
    fn run_id_expands_to_the_prefix_every_unique_token_carries() {
        let s = VarStack::new();
        let g = generator();
        let out = interpolate("<<run_id>>", &s, &g).unwrap();
        assert_eq!(out, g.run_id());
        assert!(
            g.next(crate::unique::UniqueKind::Token)
                .starts_with(&format!("u{out}")),
            "a LIKE on the run_id must reach every unique token of the run"
        );
    }

    #[test]
    fn null_expands_to_sentinel() {
        let s = VarStack::new();
        assert_eq!(
            interpolate("<<null>>", &s, &generator()).unwrap(),
            NULL_SENTINEL
        );
    }
}
