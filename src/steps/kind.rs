//! What a built-in step is: its declaration, and whether it acts or asserts.

use super::StepId;
use crate::plugin::abi::StepSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionsSource {
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Action,
    /// `source` is where a built-in's polling options come from. It has no
    /// meaning on a plugin step, which polls with its instance's options: the
    /// value there is never read. `negative`: the assertion passes when
    /// something is absent or does not match. A check of a value, such as
    /// `status should be 404`, is not negative.
    Assertion {
        source: OptionsSource,
        negative: bool,
    },
}

impl From<&StepSpec> for StepKind {
    fn from(spec: &StepSpec) -> Self {
        if spec.is_assertion() {
            Self::Assertion {
                source: OptionsSource::Global,
                negative: spec.negative,
            }
        } else {
            Self::Action
        }
    }
}

pub struct StepDef {
    pub id: StepId,
    /// The resource this step belongs to, and the heading `bddkit steps list`
    /// prints it under: `vars`, `debug`, or `general` for
    /// the few steps that belong to no resource at all.
    pub group: &'static str,
    /// A Cucumber Expression with bddkit's parameter extension: `{name}` is a
    /// `text` parameter (`[^"]*`; a bare one is matched inside quotes the host
    /// adds), `{name:type}` picks a type from `expression::TYPES`, which also
    /// decides whether a bare parameter takes quotes. The name is what `bddkit steps
    /// list` prints as `<name>`; captures still reach `dispatch` by position.
    pub expression: &'static str,
    /// One line, in English, stating the step's effect. English is the source
    /// language and it lives here, beside the pattern it describes;
    /// `locales/steps.<code>.yaml` carries translations of it.
    pub description: &'static str,
    pub kind: StepKind,
}

pub(super) const fn action(
    id: StepId,
    group: &'static str,
    expression: &'static str,
    description: &'static str,
) -> StepDef {
    StepDef {
        id,
        group,
        expression,
        description,
        kind: StepKind::Action,
    }
}

const fn declare_assertion(
    id: StepId,
    group: &'static str,
    expression: &'static str,
    description: &'static str,
    source: OptionsSource,
    negative: bool,
) -> StepDef {
    StepDef {
        id,
        group,
        expression,
        description,
        kind: StepKind::Assertion { source, negative },
    }
}

pub(super) const fn assertion(
    id: StepId,
    group: &'static str,
    expression: &'static str,
    description: &'static str,
    source: OptionsSource,
) -> StepDef {
    declare_assertion(id, group, expression, description, source, false)
}

pub(super) const fn negative_assertion(
    id: StepId,
    group: &'static str,
    expression: &'static str,
    description: &'static str,
    source: OptionsSource,
) -> StepDef {
    declare_assertion(id, group, expression, description, source, true)
}

#[cfg(test)]
mod tests {
    use crate::steps::{BUILTIN_STEPS, Registry, StepId, StepKind, StepTarget};

    #[test]
    fn the_four_negative_variable_steps_are_marked() {
        let negative = |kind| matches!(kind, StepKind::Assertion { negative: true, .. });
        let marked: Vec<StepId> = BUILTIN_STEPS
            .iter()
            .filter(|def| negative(def.kind))
            .map(|def| def.id)
            .collect();
        assert_eq!(
            marked,
            [
                StepId::VariableNotEquals,
                StepId::VariableNotContains,
                StepId::VariableNotMatches,
                StepId::VariableNotContainsJson,
            ]
        );
        // The resolved step carries the flag.
        let registry = Registry::new().expect("registry");
        for (text, expected) in [
            (r#"variable "a" should not be equal to "1""#, true),
            (r#"variable "a" should not contain "1""#, true),
            (r#"variable "a" should not match "1""#, true),
            (r#"variable "a" should not contain JSON:"#, true),
            (r#"variable "a" should be equal to "1""#, false),
            ("I am not in debug mode", false),
            ("I am not collecting evidence", false),
        ] {
            let (target, _) = registry.find(text).expect("one match").expect("found");
            let StepTarget::Builtin { kind, .. } = target else {
                panic!("{text}: {target:?}");
            };
            assert_eq!(negative(kind), expected, "{text}");
        }
    }
}
