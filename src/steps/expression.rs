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

use crate::vars::PLACEHOLDER;
use cucumber_expressions::{Alternative, Expression, SingleExpression, Spanned};
use regex::Regex;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug)]
pub struct ParamType {
    pub name: &'static str,
    regex: &'static str,
    /// A typed parameter narrows the literal text a step may carry, so it also
    /// accepts a whole `<<…>>` slot and is checked again after interpolation.
    /// `text` and `any` accept a slot already and need no check.
    typed: bool,
    shape: Shape,
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
        shape: Shape::Star(CharClass::NonQuote),
    },
    ParamType {
        name: "any",
        regex: ".*?",
        typed: false,
        shape: Shape::Star(CharClass::Any),
    },
    ParamType {
        name: "uint",
        regex: r"\d+",
        typed: true,
        shape: Shape::Plus(CharClass::Digit),
    },
    ParamType {
        name: "int",
        regex: r"-?\d+",
        typed: true,
        shape: Shape::SignedInt,
    },
    ParamType {
        name: "float",
        regex: r"[+-]?(?:inf|NaN|(?:\d+|\d+\.\d*|\d*\.\d+)(?:[eE][+-]?\d+)?)",
        typed: true,
        shape: Shape::Star(CharClass::Any),
    },
    ParamType {
        name: "word",
        regex: r"[^\s]+",
        typed: true,
        shape: Shape::Plus(CharClass::NonSpace),
    },
    ParamType {
        name: "method",
        regex: "GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS",
        typed: true,
        shape: Shape::Words(&["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]),
    },
];

impl ParamType {
    fn pattern(&self) -> String {
        if self.typed {
            format!("{}|{PLACEHOLDER}", self.regex)
        } else {
            self.regex.to_string()
        }
    }

    fn tokens(&self) -> Vec<Vec<PatternToken>> {
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "read by Task 4 of #66: runner::prepare type check"
        )
    )]
    check: Option<Regex>,
}

impl Param {
    /// The post-interpolation half of a typed parameter: the step matched a
    /// literal of the type or a whole `<<…>>` slot, and only the slot's value
    /// can still be wrong here.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "read by Task 4 of #66: runner::prepare type check"
        )
    )]
    pub fn check(&self, value: &str) -> Result<(), String> {
        match &self.check {
            Some(re) if !re.is_match(value) => Err(format!(
                "parameter <{}> expects {}, got {value:?}",
                self.name, self.ty.name
            )),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Compiled {
    pub regex: Regex,
    /// In capture order: `params[i]` describes capture group `i + 1`.
    pub params: Vec<Param>,
    pub variants: Vec<Vec<PatternToken>>,
    /// `{name:type}` → `<name>`, everything else verbatim.
    pub template: String,
}

pub fn compile(expr: &str, default_type: &str) -> Result<Compiled, String> {
    let ast = Expression::parse(expr)
        .map_err(|error| format!("invalid step expression {expr:?}: {error}"))?;
    let mut params: Vec<Param> = Vec::new();
    let mut types: HashMap<String, String> = HashMap::new();
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
                let param = param(raw, default_type, expr)?;
                if params.iter().any(|seen| seen.name == param.name) {
                    return Err(format!(
                        "parameter {:?} is declared more than once in {expr:?}",
                        param.name
                    ));
                }
                // `location_offset` is the byte offset of the name, just past `{`.
                let start = parameter.input.location_offset() - 1;
                template.push_str(&expr[last..start]);
                template.push_str(&format!("<{}>", param.name));
                last = start + raw.len() + 2;
                types.insert(raw.to_string(), param.ty.pattern());
                let alternatives = param.ty.tokens();
                params.push(param);
                alternatives
            }
        };
        variants = cross(variants, &alternatives);
    }
    template.push_str(&expr[last..]);
    let regex = Expression::regex_with_parameters(expr, &types)
        .map_err(|error| format!("invalid step expression {expr:?}: {error}"))?;
    Ok(Compiled {
        regex,
        params,
        variants,
        template,
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
    fn no_type_regex_adds_a_capture_group() {
        for ty in TYPES {
            let compiled = compile(&format!("x {{v:{}}}", ty.name), "text").unwrap();
            assert_eq!(compiled.regex.captures_len(), 2, "type {}", ty.name);
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
}
