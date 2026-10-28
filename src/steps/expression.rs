//! The one parser of step declarations.
//!
//! A builtin step and a macro template are both a Cucumber Expression
//! (optional `(text)`, alternation `a/b`, escapes `\(` `\{` `\/` `\\`) whose
//! parameters follow a bddkit extension: `{name}` or `{name:type}`. Cucumber
//! itself has no parameter names — only types — so the name is written into the
//! type slot and split off here. [`compile`] parses an expression ONCE and
//! derives from that single AST everything the host needs from it: the
//! matching regex, the conflict-detection token streams, the `steps list`
//! template and each parameter's type. Nothing else in the host parses a
//! declaration, which is what keeps the three from drifting apart (#66).

use cucumber_expressions::{Alternative, Expression, SingleExpression, Spanned};
use regex::Regex;
use std::collections::{HashMap, HashSet, VecDeque};

/// The shape of a whole `<<…>>` slot as the step regex sees it. Deliberately
/// wider than `vars::PLACEHOLDER`, which a typed parameter checks after
/// interpolation anyway: the crate expands `\w` in `PLACEHOLDER` into ~11 KB of
/// unicode ranges per use (0.5 s in a debug build, and a bare `optional`
/// parameter uses it twice), so the exact grammar cannot sit in every pattern.
const SLOT_SHAPE: &str = r#"<<[^<>"\s]*(?:\([^)]*\))?>>"#;

#[derive(Debug)]
pub struct ParamType {
    pub name: &'static str,
    regex: &'static str,
    /// A typed parameter narrows the literal text a step may carry, so it also
    /// accepts a whole `<<…>>` slot and is checked again after interpolation.
    /// `text` and `any` accept a slot already and need no check.
    typed: bool,
    /// The value is a secret: the runner registers it when the step is
    /// prepared and a literal one is written `***` in the step text it shows.
    pub secret: bool,
    /// Whether a bare `{x}` of this type is written in quotes in `.feature` text.
    quoting: Quoting,
    shape: Shape,
}

/// What a type says about the quotes around a BARE parameter, one without
/// quotes of its own in the declaration — a `"{x}"` is always literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quoting {
    /// Only `"…"`; the host adds the quotes. A bare `{x}` that matched unquoted
    /// text would let `the response body contains {text}` match `… contains JSON:`.
    Required,
    /// `5` and `"5"` alike. Allowed only for a type whose values hold neither
    /// whitespace nor `"` — `"5"` would be ambiguous otherwise, and a value with
    /// spaces would swallow the step's next word.
    Optional,
    /// Exactly what the template says, nothing added.
    AsWritten,
}

/// What a type can match, for conflict detection. It may be WIDER than the
/// regex, never narrower: a false conflict is a loud startup error, a missed
/// one is a silently shadowed step.
#[derive(Debug)]
enum Shape {
    Star(CharClass),
    Plus(CharClass),
    SignedInt,
    Words(&'static [&'static str]),
}

pub static TYPES: &[ParamType] = &[
    ParamType {
        name: "text",
        regex: r#"[^"]*"#,
        typed: false,
        secret: false,
        quoting: Quoting::Required,
        shape: Shape::Star(CharClass::NonQuote),
    },
    ParamType {
        name: "secret",
        regex: r#"[^"]*"#,
        typed: false,
        secret: true,
        quoting: Quoting::Required,
        shape: Shape::Star(CharClass::NonQuote),
    },
    ParamType {
        name: "any",
        regex: ".*?",
        typed: false,
        secret: false,
        quoting: Quoting::AsWritten,
        shape: Shape::Star(CharClass::Any),
    },
    ParamType {
        name: "uint",
        regex: "[0-9]+",
        typed: true,
        secret: false,
        quoting: Quoting::Optional,
        shape: Shape::Plus(CharClass::Digit),
    },
    ParamType {
        name: "int",
        regex: "-?[0-9]+",
        typed: true,
        secret: false,
        quoting: Quoting::Optional,
        shape: Shape::SignedInt,
    },
    ParamType {
        name: "float",
        regex: r"[+-]?(?:inf|NaN|(?:[0-9]+|[0-9]+\.[0-9]*|[0-9]*\.[0-9]+)(?:[eE][+-]?[0-9]+)?)",
        typed: true,
        secret: false,
        quoting: Quoting::Optional,
        shape: Shape::Star(CharClass::Any),
    },
    ParamType {
        name: "word",
        regex: r#"[^\s"]+"#,
        typed: true,
        secret: false,
        quoting: Quoting::Optional,
        shape: Shape::Plus(CharClass::NonSpace),
    },
    ParamType {
        name: "method",
        regex: "GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS",
        typed: true,
        secret: false,
        quoting: Quoting::Optional,
        shape: Shape::Words(&["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]),
    },
];

impl ParamType {
    fn value(&self) -> String {
        if self.typed {
            format!("{}|{SLOT_SHAPE}", self.regex)
        } else {
            self.regex.to_string()
        }
    }

    /// The regex for the parameter's slot in the step. The crate wraps one that
    /// holds no group in a group of its own and renames the groups of one that
    /// does (`__{id}_0`, `__{id}_1`), so the quotes of a bare parameter sit
    /// outside what it captures.
    fn pattern(&self, quoting: Quoting) -> String {
        self.wrap(quoting, self.value())
    }

    /// `pattern` with a typed parameter's value widened to anything: the same
    /// quotes and groups, so it matches a literal the strict regex refused.
    fn loose_pattern(&self, quoting: Quoting) -> String {
        self.wrap(
            quoting,
            if self.typed {
                ".*?".into()
            } else {
                self.value()
            },
        )
    }

    fn wrap(&self, quoting: Quoting, value: String) -> String {
        match quoting {
            Quoting::AsWritten => value,
            Quoting::Required => format!(r#""({value})""#),
            Quoting::Optional => format!(r#""({value})"|({value})"#),
        }
    }

    fn tokens(&self, quoting: Quoting) -> Vec<Vec<PatternToken>> {
        let bare = self.unquoted_tokens();
        let quoted = |value: &Vec<PatternToken>| [exact("\""), value.clone(), exact("\"")].concat();
        match quoting {
            Quoting::AsWritten => bare,
            Quoting::Required => bare.iter().map(quoted).collect(),
            Quoting::Optional => bare
                .iter()
                .map(quoted)
                .chain(bare.iter().cloned())
                .collect(),
        }
    }

    fn unquoted_tokens(&self) -> Vec<Vec<PatternToken>> {
        use PatternToken::{One, Star};
        let mut alternatives = match self.shape {
            Shape::Star(class) => vec![vec![Star(class)]],
            Shape::Plus(class) => vec![vec![One(class), Star(class)]],
            Shape::SignedInt => vec![
                vec![One(CharClass::Digit), Star(CharClass::Digit)],
                vec![
                    One(CharClass::Exact('-')),
                    One(CharClass::Digit),
                    Star(CharClass::Digit),
                ],
            ],
            Shape::Words(words) => words.iter().map(|word| exact(word)).collect(),
        };
        if self.typed {
            let mut slot = exact("<<");
            slot.push(Star(CharClass::Any));
            slot.extend(exact(">>"));
            alternatives.push(slot);
        }
        alternatives
    }
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: &'static ParamType,
    /// The capture groups of the step regex that can hold this parameter's
    /// value, 1-based: one, or two for a bare `optional` one (quoted, unquoted).
    /// Exactly one of them matched.
    groups: Vec<usize>,
    check: Option<Regex>,
}

/// One argument per parameter, as `dispatch` and the macros read them: the
/// first group of each parameter that matched. Groups beyond the last
/// parameter's — a plugin's regex has more groups than `plugin_params` names —
/// follow as they are.
pub fn arguments(params: &[Param], captures: &regex::Captures<'_>) -> Vec<String> {
    let text = |group| captures.get(group).map(|m| m.as_str().to_string());
    let covered = params
        .last()
        .and_then(|p| p.groups.last())
        .copied()
        .unwrap_or(0);
    params
        .iter()
        .map(|p| p.groups.iter().find_map(|&g| text(g)).unwrap_or_default())
        .chain((covered + 1..captures.len()).map(|g| text(g).unwrap_or_default()))
        .collect()
}

impl Param {
    /// A typed parameter matched a `<<…>>` shape wider than a real slot (see
    /// `SLOT_SHAPE`); `false` means the step does not match, so `validate::check`
    /// refuses it before the first request instead of `prepare` failing mid-run.
    pub fn slot_is_whole(&self, value: &str) -> bool {
        !self.ty.typed || !value.starts_with("<<") || crate::vars::is_whole_slot(value)
    }

    /// Where the value sits in the step text, `None` if no group matched.
    pub fn span<'t>(&self, captures: &regex::Captures<'t>) -> Option<regex::Match<'t>> {
        self.groups.iter().find_map(|&g| captures.get(g))
    }
}

/// The parameters of a plugin step, whose regex has unnamed groups: one per
/// group up to the last secret one, `secret` where the manifest says so and
/// `text` elsewhere. Nothing is type-checked (`check` is `None`); the list
/// exists so the runner and `Registry::display_text` treat a plugin's secret
/// group exactly as a `{name:secret}` of a builtin. `secret_groups` is 1-based.
pub fn plugin_params(secret_groups: &[usize]) -> Vec<Param> {
    let ty = |name| {
        TYPES
            .iter()
            .find(|ty| ty.name == name)
            .expect("a known type")
    };
    (1..=secret_groups.iter().copied().max().unwrap_or(0))
        .map(|group| Param {
            name: format!("group {group}"),
            groups: vec![group],
            ty: ty(if secret_groups.contains(&group) {
                "secret"
            } else {
                "text"
            }),
            check: None,
        })
        .collect()
}

impl Param {
    /// The post-interpolation half of a typed parameter: the step matched a
    /// literal of the type or a whole `<<…>>` slot, and only the slot's value
    /// can still be wrong here.
    pub fn check(&self, value: &str) -> Result<(), String> {
        let Some(re) = &self.check else {
            return Ok(());
        };
        // `<<null>>` is a SQL NULL, meant for `text` positions only.
        if value == crate::vars::NULL_SENTINEL {
            return Err(format!(
                "parameter <{}> expects {}, got <<null>>",
                self.name, self.ty.name
            ));
        }
        if re.is_match(value) {
            Ok(())
        } else {
            Err(format!(
                "parameter <{}> expects {}, got {value:?}",
                self.name, self.ty.name
            ))
        }
    }
}

#[derive(Debug, Clone)]
pub struct Compiled {
    pub regex: Regex,
    /// In declaration order; each knows its own capture groups.
    pub params: Vec<Param>,
    pub variants: Vec<Vec<PatternToken>>,
    /// `{name:type}` → `<name>`, everything else verbatim.
    pub template: String,
    /// The same expression with every typed parameter widened to `.*?`: it
    /// matches a literal the strict regex refused, so the refusal can be
    /// explained. `None` when no parameter is typed.
    pub loose: Option<Regex>,
}

pub fn compile(expr: &str, default_type: &str) -> Result<Compiled, String> {
    let ast = Expression::parse(expr)
        .map_err(|error| format!("invalid step expression {expr:?}: {error}"))?;
    let mut params: Vec<Param> = Vec::new();
    let mut types: HashMap<String, String> = HashMap::new();
    let mut loose_types: HashMap<String, String> = HashMap::new();
    let mut variants: Vec<Vec<PatternToken>> = vec![Vec::new()];
    let mut template = String::with_capacity(expr.len());
    let mut last = 0;
    for item in &ast.0 {
        let alternatives = match item {
            SingleExpression::Text(text) | SingleExpression::Whitespaces(text) => {
                vec![exact(&unescape(text))]
            }
            SingleExpression::Optional(optional) => optional_variants(&optional.0),
            SingleExpression::Alternation(alternation) => alternation
                .0
                .iter()
                .flat_map(|single| {
                    single.iter().fold(vec![Vec::new()], |acc, piece| {
                        cross(
                            acc,
                            &match piece {
                                Alternative::Text(text) => vec![exact(&unescape(text))],
                                Alternative::Optional(optional) => optional_variants(&optional.0),
                            },
                        )
                    })
                })
                .collect(),
            SingleExpression::Parameter(parameter) => {
                let raw: &str = parameter.input.fragment();
                let mut param = param(raw, default_type, expr)?;
                if params.iter().any(|seen| seen.name == param.name) {
                    return Err(format!(
                        "parameter {:?} is declared more than once in {expr:?}",
                        param.name
                    ));
                }
                // `location_offset` is the byte offset of the name, just past `{`.
                let start = parameter.input.location_offset() - 1;
                let end = start + raw.len() + 2;
                // Bare unless the parameter sits inside a quoted literal of the
                // template: an odd number of `"` before it (Cucumber has no `\"`).
                let bare = expr[..start].matches('"').count().is_multiple_of(2);
                let quoting = if bare {
                    param.ty.quoting
                } else {
                    Quoting::AsWritten
                };
                template.push_str(&expr[last..start]);
                template.push_str(&match quoting {
                    Quoting::Required => format!("\"<{}>\"", param.name),
                    Quoting::Optional | Quoting::AsWritten => format!("<{}>", param.name),
                });
                last = end;
                types.insert(raw.to_string(), param.ty.pattern(quoting));
                loose_types.insert(raw.to_string(), param.ty.loose_pattern(quoting));
                let first = params.iter().map(|p| p.groups.len()).sum::<usize>() + 1;
                let count = if quoting == Quoting::Optional { 2 } else { 1 };
                param.groups = (first..first + count).collect();
                let alternatives = param.ty.tokens(quoting);
                params.push(param);
                alternatives
            }
        };
        variants = cross(variants, &alternatives);
    }
    template.push_str(&expr[last..]);
    let regex = Expression::regex_with_parameters(expr, &types)
        .map_err(|error| format!("invalid step expression {expr:?}: {error}"))?;
    let loose = if params.iter().any(|param| param.ty.typed) {
        Some(
            Expression::regex_with_parameters(expr, &loose_types)
                .map_err(|error| format!("invalid step expression {expr:?}: {error}"))?,
        )
    } else {
        None
    };
    Ok(Compiled {
        regex,
        params,
        variants,
        template,
        loose,
    })
}

fn param(raw: &str, default_type: &str, expr: &str) -> Result<Param, String> {
    let (name, type_name) = raw.split_once(':').unwrap_or((raw, default_type));
    let mut chars = name.chars();
    let identifier = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !identifier {
        return Err(format!(
            "parameter {{{raw}}} in {expr:?} needs a name: letters, digits and '_', not starting with a digit"
        ));
    }
    let ty = TYPES
        .iter()
        .find(|ty| ty.name == type_name)
        .ok_or_else(|| format!("parameter {{{raw}}} in {expr:?} has unknown type {type_name:?}"))?;
    let check = ty
        .typed
        .then(|| Regex::new(&format!("^(?:{})$", ty.regex)).expect("type regexes are constants"));
    Ok(Param {
        name: name.to_string(),
        ty,
        groups: Vec::new(),
        check,
    })
}

fn unescape(text: &Spanned<'_>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        out.push(if c == '\\' {
            chars.next().unwrap_or('\\')
        } else {
            c
        });
    }
    out
}

/// An optional part is either there, in full, or absent.
fn optional_variants(text: &Spanned<'_>) -> Vec<Vec<PatternToken>> {
    vec![exact(&unescape(text)), Vec::new()]
}

fn exact(text: &str) -> Vec<PatternToken> {
    text.chars()
        .map(|c| PatternToken::One(CharClass::Exact(c)))
        .collect()
}

// ponytail: each optional doubles the variants and each alternation multiplies
// them, a typed parameter by 2-8 and a bare `optional` one by twice that (the
// quoted and the unquoted form), so a template with ~20 of them means ~10^6
// variants at startup; cap the count with a load error if real templates ever
// approach that.
fn cross(
    prefixes: Vec<Vec<PatternToken>>,
    suffixes: &[Vec<PatternToken>],
) -> Vec<Vec<PatternToken>> {
    prefixes
        .iter()
        .flat_map(|prefix| {
            suffixes
                .iter()
                .map(move |suffix| [prefix.as_slice(), suffix].concat())
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharClass {
    Any,
    NonQuote,
    Digit,
    NonSpace,
    Exact(char),
}

#[derive(Clone, Copy, Debug)]
pub enum PatternToken {
    One(CharClass),
    Star(CharClass),
}

/// Whether some step text is matched by a variant of each side.
pub fn conflicts(left: &[Vec<PatternToken>], right: &[Vec<PatternToken>]) -> bool {
    left.iter()
        .any(|l| right.iter().any(|r| patterns_overlap(l, r)))
}

fn patterns_overlap(left: &[PatternToken], right: &[PatternToken]) -> bool {
    let mut queue = VecDeque::from([(0usize, 0usize)]);
    let mut visited = HashSet::new();
    while let Some((left_pos, right_pos)) = queue.pop_front() {
        if !visited.insert((left_pos, right_pos)) {
            continue;
        }
        if left_pos == left.len() && right_pos == right.len() {
            return true;
        }
        if matches!(left.get(left_pos), Some(PatternToken::Star(_))) {
            queue.push_back((left_pos + 1, right_pos));
        }
        if matches!(right.get(right_pos), Some(PatternToken::Star(_))) {
            queue.push_back((left_pos, right_pos + 1));
        }
        let (Some(left_token), Some(right_token)) = (left.get(left_pos), right.get(right_pos))
        else {
            continue;
        };
        let (left_class, left_next) = consumed(*left_token, left_pos);
        let (right_class, right_next) = consumed(*right_token, right_pos);
        if classes_overlap(left_class, right_class) {
            queue.push_back((left_next, right_next));
        }
    }
    false
}

fn consumed(token: PatternToken, position: usize) -> (CharClass, usize) {
    match token {
        PatternToken::One(class) => (class, position + 1),
        PatternToken::Star(class) => (class, position),
    }
}

fn classes_overlap(left: CharClass, right: CharClass) -> bool {
    use CharClass::{Any, Exact};
    match (left, right) {
        (Any, _) | (_, Any) => true,
        (Exact(left), Exact(right)) => left == right,
        (class, Exact(char_)) | (Exact(char_), class) => contains(class, char_),
        // Every pair of the remaining classes shares a character (an ASCII
        // digit is in all three).
        _ => true,
    }
}

fn contains(class: CharClass, char_: char) -> bool {
    match class {
        CharClass::Any => true,
        CharClass::NonQuote => char_ != '"',
        CharClass::Digit => char_.is_numeric(),
        CharClass::NonSpace => !char_.is_whitespace(),
        CharClass::Exact(expected) => char_ == expected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin(expr: &str) -> Compiled {
        compile(expr, "text").expect("valid expression")
    }

    #[test]
    fn a_quoted_parameter_compiles_to_the_regex_it_replaced() {
        let compiled = builtin(r#"the "{name}" request header is "{value}""#);
        assert_eq!(
            compiled.regex.as_str(),
            r#"^the "([^"]*)" request header is "([^"]*)"$"#
        );
        assert_eq!(
            compiled.template,
            r#"the "<name>" request header is "<value>""#
        );
        let names: Vec<_> = compiled
            .params
            .iter()
            .map(|p| (p.name.as_str(), p.ty.name))
            .collect();
        assert_eq!(names, [("name", "text"), ("value", "text")]);
    }

    #[test]
    fn a_typed_parameter_accepts_a_literal_or_a_whole_slot() {
        let compiled = builtin("the response code is {code:uint}");
        assert_eq!(compiled.template, "the response code is <code>");
        for text in [
            "the response code is 200",
            "the response code is <<code>>",
            "the response code is <<unique(n)>>",
        ] {
            assert!(compiled.regex.is_match(text), "{text}");
        }
        for text in [
            "the response code is abc",
            "the response code is -5",
            "the response code is <<code>>0",
        ] {
            assert!(!compiled.regex.is_match(text), "{text}");
        }
    }

    #[test]
    fn a_typed_parameter_checks_the_interpolated_value() {
        let compiled = builtin("the response code is {code:uint}");
        assert!(compiled.params[0].check("201").is_ok());
        let error = compiled.params[0].check("abc").unwrap_err();
        assert_eq!(error, r#"parameter <code> expects uint, got "abc""#);
        assert!(builtin(r#"x "{v}""#).params[0].check("anything").is_ok());
    }

    #[test]
    fn null_fails_every_typed_check_and_passes_an_untyped_one() {
        let null = crate::vars::NULL_SENTINEL;
        for ty in ["word", "uint"] {
            let compiled = builtin(&format!("x {{v:{ty}}}"));
            assert_eq!(
                compiled.params[0].check(null).unwrap_err(),
                format!("parameter <v> expects {ty}, got <<null>>")
            );
        }
        assert!(builtin(r#"x "{v}""#).params[0].check(null).is_ok());
    }

    #[test]
    fn an_optional_and_an_alternation_compile() {
        let compiled = builtin(r#"I include "{file}"( with:)"#);
        assert_eq!(
            compiled.regex.as_str(),
            r#"^I include "([^"]*)"(?: with:)?$"#
        );
        assert_eq!(compiled.template, r#"I include "<file>"( with:)"#);
        let compiled = compile("I have a cat/dog", "any").unwrap();
        assert!(compiled.regex.is_match("I have a dog"));
        assert_eq!(compiled.variants.len(), 2);
    }

    #[test]
    fn a_method_parameter_is_a_closed_set() {
        let compiled = builtin(r#"I request "{path}" using HTTP {method:method}"#);
        assert!(
            compiled
                .regex
                .is_match(r#"I request "/a" using HTTP PATCH"#)
        );
        assert!(
            !compiled
                .regex
                .is_match(r#"I request "/a" using HTTP FETCH"#)
        );
    }

    #[test]
    fn declaration_errors_are_refused() {
        for (expr, needle) in [
            ("a {}", "needs a name"),
            ("a {n:nope}", "unknown type"),
            ("a {v} and {v}", "more than once"),
            ("a {1x}", "needs a name"),
            ("a (b {c})", "optional may not contain a parameter"),
            ("a {b", "does not have a matching"),
            ("a \\d", "Only the characters"),
        ] {
            let error = compile(expr, "text").unwrap_err();
            assert!(error.contains(needle), "{expr}: {error}");
        }
    }

    #[test]
    fn an_optional_parameter_owns_two_groups_and_every_other_one() {
        // Two groups for a bare `optional` parameter (quoted, unquoted), one for
        // everything else: the `groups` map is what `arguments` reads.
        for ty in TYPES {
            for expr in [
                format!("x {{v:{}}}", ty.name),
                format!(r#"x "{{v:{}}}""#, ty.name),
            ] {
                let compiled = compile(&expr, "text").unwrap();
                let groups = &compiled.params[0].groups;
                assert_eq!(compiled.regex.captures_len(), groups.len() + 1, "{expr}");
                let optional = ty.quoting == Quoting::Optional && !expr.contains('"');
                assert_eq!(groups.len(), if optional { 2 } else { 1 }, "{expr}");
            }
        }
    }

    #[test]
    fn an_optional_type_matches_with_or_without_quotes() {
        let compiled = builtin("I sleep {n:uint} seconds");
        assert_eq!(compiled.template, "I sleep <n> seconds");
        for text in [
            "I sleep 5 seconds",
            r#"I sleep "5" seconds"#,
            "I sleep <<n>> seconds",
            r#"I sleep "<<n>>" seconds"#,
        ] {
            assert!(compiled.regex.is_match(text), "{text}");
        }
        for text in [
            r#"I sleep "5 seconds"#,
            r#"I sleep 5" seconds"#,
            "I sleep  seconds",
        ] {
            assert!(!compiled.regex.is_match(text), "{text}");
        }
        for text in ["I sleep 5 seconds", r#"I sleep "5" seconds"#] {
            let captures = compiled.regex.captures(text).unwrap();
            assert_eq!(arguments(&compiled.params, &captures), ["5"], "{text}");
        }
    }

    #[test]
    fn quotes_written_in_a_declaration_stay_mandatory() {
        let compiled = builtin(r#"I sleep "{n:uint}" seconds"#);
        assert_eq!(compiled.template, r#"I sleep "<n>" seconds"#);
        assert!(compiled.regex.is_match(r#"I sleep "5" seconds"#));
        assert!(!compiled.regex.is_match("I sleep 5 seconds"));
        assert_eq!(compiled.params[0].groups.len(), 1);
    }

    #[test]
    fn a_bare_text_parameter_requires_the_quotes_and_an_any_parameter_adds_none() {
        let text = builtin("the body contains {v}");
        assert_eq!(text.template, r#"the body contains "<v>""#);
        assert!(text.regex.is_match(r#"the body contains "JSON:""#));
        assert!(!text.regex.is_match("the body contains JSON:"));
        let any = compile("I compare {a} with {b}", "any").unwrap();
        assert_eq!(any.template, "I compare <a> with <b>");
        assert!(any.regex.is_match("I compare x with y z"));
    }

    #[test]
    fn a_parameter_inside_a_quoted_literal_is_written_not_bare() {
        // An odd number of `"` before it: the quotes are the template's own.
        let compiled = builtin(r#"I send the header "Bearer {token:secret}""#);
        assert!(
            compiled
                .regex
                .is_match(r#"I send the header "Bearer abcdefghij""#)
        );
        assert_eq!(compiled.template, r#"I send the header "Bearer <token>""#);
        let compiled = builtin(r#"I say "{a:text} {b:text}""#);
        assert!(compiled.regex.is_match(r#"I say "x y""#));
        let compiled = builtin(r#"I say "{a:uint}-{b:uint}""#);
        assert!(compiled.regex.is_match(r#"I say "1-2""#));
        assert!(!compiled.regex.is_match(r#"I say ""1"-"2"""#));
    }

    #[test]
    fn two_optional_parameters_each_yield_the_group_that_matched() {
        let compiled = builtin("a {x:uint} b {y:word} c");
        for (text, want) in [
            ("a 1 b two c", ["1", "two"]),
            (r#"a "1" b "two" c"#, ["1", "two"]),
            (r#"a 1 b "two" c"#, ["1", "two"]),
            (r#"a "1" b two c"#, ["1", "two"]),
        ] {
            let captures = compiled.regex.captures(text).unwrap();
            assert_eq!(arguments(&compiled.params, &captures), want, "{text}");
        }
    }

    #[test]
    fn an_optional_type_cannot_hold_whitespace_or_a_quote() {
        // The rule that keeps a bare parameter unambiguous. `pattern` is what
        // the step regex uses, so the `<<…>>` slot is covered too (its
        // arguments, `[^)]*`, are the one place a space can ride along; an
        // existing limit of the slot grammar, bounded by the closing `)>>`).
        for ty in TYPES.iter().filter(|ty| ty.quoting == Quoting::Optional) {
            let re = Regex::new(&format!("^(?:{})$", ty.value())).unwrap();
            let sample = match ty.name {
                "method" => "GET",
                "float" => "1.5",
                "word" => "abc",
                _ => "5",
            };
            for probe in [
                " ".to_string(),
                "\t".to_string(),
                "\"".to_string(),
                format!("{sample} {sample}"),
                format!("\"{sample}"),
                format!("{sample}\""),
                "<<a b>>".to_string(),
                "<<a\"b>>".to_string(),
            ] {
                assert!(!re.is_match(&probe), "{} matches {probe:?}", ty.name);
            }
            assert!(re.is_match(sample), "{} rejects {sample:?}", ty.name);
        }
    }

    #[test]
    fn conflicts_follow_every_variant() {
        let code = builtin("the response code is {code:uint}").variants;
        let conflict =
            |macro_step: &str| conflicts(&compile(macro_step, "any").unwrap().variants, &code);
        assert!(conflict("the response code is {n}"));
        assert!(conflict("the response code is {n:word}"));
        assert!(conflict("the response code is 2{tail}"));
        assert!(conflict("the response code is ٢{tail}"));
        assert!(conflict("the response code is <<x>>"));
        assert!(conflict("the response code is 200/none"));
        assert!(!conflict("the response code is none"));
        assert!(!conflict("the response code is -5"));
        let include = builtin(r#"I include "{file}"( with:)"#).variants;
        assert!(conflicts(
            &compile(r#"I include "setup.feature""#, "any")
                .unwrap()
                .variants,
            &include
        ));
        assert!(conflicts(
            &compile(r#"I include "setup.feature" with:"#, "any")
                .unwrap()
                .variants,
            &include
        ));
    }

    #[test]
    fn secret_matches_exactly_what_text_matches() {
        let secret = compile("x {v:secret}", "text").expect("valid");
        let text = compile("x {v:text}", "text").expect("valid");
        assert_eq!(secret.regex.as_str(), text.regex.as_str());
        assert!(secret.params[0].ty.secret && !text.params[0].ty.secret);
    }
}
