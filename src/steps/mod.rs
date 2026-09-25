pub mod api;
pub mod assert;
pub mod db;
pub mod debug;
pub mod help;
mod markup;
pub mod plugin;
pub mod srp;
pub mod vars;

use crate::macros::{MacroCatalog, MacroDef};
use crate::options::{OptionsLayer, PollingOptionsLayer};
use crate::polling::{AttemptError, AttemptResult};
use crate::world::World;
use regex::Regex;
use std::collections::{HashSet, VecDeque};
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepId {
    // request
    SetRequestHeader,
    AddRequestHeader,
    SetQueryParam,
    SetRequestBody,
    EmptyRequestBody,
    SetFormParams,
    RequestPath,
    RequestPathWithMethod,
    UseApi,
    UsePluginInstance,
    SignNextRequestWithHawk,
    ExpectEventually,
    ExpectWithin,
    ExpectWithinEvery,
    // response checks
    ResponseCode,
    ResponseBodyContainsJson,
    ResponseBodyEqualsJson,
    ResponseArrayLength,
    ResponseHeader,
    JsonNodeExists,
    JsonNodeNotExists,
    JsonNodeIsNull,
    JsonNodeNotNull,
    ResponseBodyNotContainsJson,
    JsonNodeNotContainsSubstring,
    ResponseBodyEmpty,
    ResponseBodyContains,
    ResponseBodyNotContains,
    ResponseBodyMatches,
    ResponseBodyNotMatches,
    ResponseBodyHasElement,
    ResponseBodyHasElementWithText,
    ResponseBodyNotHasElement,
    // variables
    SetVariable,
    SetVariableGlobal,
    ExtractFromJson,
    ExtractFromJsonGlobal,
    ExtractFromCookies,
    ExtractFromCookiesGlobal,
    ExtractFromMarkup,
    VariableEquals,
    VariableNotEquals,
    VariableContains,
    VariableNotContains,
    VariableMatches,
    VariableNotMatches,
    VariableEmpty,
    VariableContainsJson,
    VariableEqualsJson,
    VariableNotContainsJson,
    ExtractJsonFromVariable,
    ExtractJsonFromVariableGlobal,
    ExtractRegexFromVariable,
    ExtractRegexFromVariableGlobal,
    EncryptWithAes,
    // database
    UseConnection,
    DebugOn,
    DebugOff,
    HaveWith,
    HaveWhere,
    HaveMulti,
    Update,
    DeleteWhere,
    DeleteAll,
    ExtractFromDb,
    ShouldHaveWith,
    ShouldHaveTable,
    ShouldNotHaveWith,
    ShouldNotHaveTable,
    CallProcedure,
    CallFunction,
    GetSequence,
    Sleep,
    ShowAllVariables,
    ShowVariable,
    PrintResponseHeaders,
    PrintResponseBody,
    PrintResponseBodyAsPath,
    // SRP
    SrpVerifier,
    SrpVerifierWithSalt,
    SrpStartLogin,
    SrpCompleteLogin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionsSource {
    Global,
    Http,
    Db,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Action,
    Assertion(OptionsSource),
}

pub struct StepDef {
    pub id: StepId,
    /// The resource this step belongs to, and the heading `bddkit steps list`
    /// prints it under: `api`, `db`, `srp`, `vars`, `debug`, or `general` for
    /// the few steps that belong to no resource at all.
    pub group: &'static str,
    pub pattern: &'static str,
    /// One line, in English, stating the step's effect. English is the source
    /// language and it lives here, beside the pattern it describes;
    /// `locales/steps.<code>.yaml` carries translations of it.
    pub description: &'static str,
    pub kind: StepKind,
}

const fn action(
    id: StepId,
    group: &'static str,
    pattern: &'static str,
    description: &'static str,
) -> StepDef {
    StepDef {
        id,
        group,
        pattern,
        description,
        kind: StepKind::Action,
    }
}

const fn assertion(
    id: StepId,
    group: &'static str,
    pattern: &'static str,
    description: &'static str,
    source: OptionsSource,
) -> StepDef {
    StepDef {
        id,
        group,
        pattern,
        description,
        kind: StepKind::Assertion(source),
    }
}

/// Capture groups are named, and the name is the parameter's whole
/// documentation: `bddkit steps list` renders `(?P<path>[^"]*)` as `<path>`.
/// The `regex` crate keeps a named group addressable by index too, so every
/// dispatch arm below still captures by position.
pub const BUILTIN_STEPS: &[StepDef] = &[
    action(
        StepId::SetRequestHeader,
        "api",
        r#"^the "(?P<name>[^"]*)" request header is "(?P<value>[^"]*)"$"#,
        "sets a header on the request being built, replacing any previous value",
    ),
    action(
        StepId::AddRequestHeader,
        "api",
        r#"^I add "(?P<value>[^"]*)" to the "(?P<name>[^"]*)" request header$"#,
        "appends another value to a request header, keeping what is already there",
    ),
    action(
        StepId::SetQueryParam,
        "api",
        r#"^the query parameter "(?P<name>[^"]*)" is "(?P<value>[^"]*)"$"#,
        "sets a query string parameter on the request being built",
    ),
    action(
        StepId::SetRequestBody,
        "api",
        r#"^the request body is:$"#,
        "sets the request body from the docstring below the step",
    ),
    action(
        StepId::EmptyRequestBody,
        "api",
        r#"^the request body is empty$"#,
        "clears the request body built so far",
    ),
    action(
        StepId::SetFormParams,
        "api",
        r#"^the request form parameters are:$"#,
        "sends the table below as form-encoded parameters instead of a body",
    ),
    action(
        StepId::RequestPathWithMethod,
        "api",
        r#"^I request "(?P<path>[^"]*)" using HTTP (?P<method>GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)$"#,
        "performs the request against the current API with this method",
    ),
    action(
        StepId::RequestPath,
        "api",
        r#"^I request "(?P<path>[^"]*)"$"#,
        "performs a GET request against the current API",
    ),
    action(
        StepId::SignNextRequestWithHawk,
        "api",
        r#"^I sign the next request with Hawk id "(?P<id>[^"]*)" and key "(?P<key>[^"]*)"$"#,
        "signs the next request with these Hawk credentials, and every replay of it",
    ),
    action(
        StepId::ExpectEventually,
        "general",
        r#"^I expect the next assertion to pass eventually$"#,
        "lets the next assertion retry with the configured polling options",
    ),
    action(
        StepId::ExpectWithinEvery,
        "general",
        r#"^I expect the next assertion to pass within "(?P<seconds>\d+)" seconds, checking every "(?P<milliseconds>\d+)" milliseconds$"#,
        "lets the next assertion retry for this long, at this interval",
    ),
    action(
        StepId::ExpectWithin,
        "general",
        r#"^I expect the next assertion to pass within "(?P<seconds>\d+)" seconds$"#,
        "lets the next assertion retry for this long, at the configured interval",
    ),
    assertion(
        StepId::ResponseCode,
        "api",
        r#"^the response code is (?P<code>\d+)$"#,
        "asserts the HTTP status code of the last response",
        OptionsSource::Http,
    ),
    assertion(
        StepId::ResponseBodyContainsJson,
        "api",
        r#"^the response body contains JSON:$"#,
        "asserts the response body contains this JSON — object subsets pass, array order is ignored",
        OptionsSource::Http,
    ),
    assertion(
        StepId::ResponseBodyEqualsJson,
        "api",
        r#"^the response body equals JSON:$"#,
        "asserts the response body equals this JSON exactly, key for key and element for element",
        OptionsSource::Http,
    ),
    assertion(
        StepId::ResponseArrayLength,
        "api",
        r#"^the response body is a JSON array of length (?P<length>\d+)$"#,
        "asserts the response body is a JSON array holding this many elements",
        OptionsSource::Http,
    ),
    assertion(
        StepId::ResponseHeader,
        "api",
        r#"^the "(?P<name>[^"]*)" response header is "(?P<value>[^"]*)"$"#,
        "asserts a response header holds exactly this value",
        OptionsSource::Http,
    ),
    assertion(
        StepId::JsonNodeExists,
        "api",
        r#"^the JSON node "(?P<path>[^"]*)" should exist$"#,
        "asserts the response body has a node at this JSON path",
        OptionsSource::Http,
    ),
    assertion(
        StepId::JsonNodeNotExists,
        "api",
        r#"^the JSON node "(?P<path>[^"]*)" should not exist$"#,
        "asserts the response body has no node at this JSON path — a null value still counts as existing",
        OptionsSource::Http,
    ),
    assertion(
        StepId::JsonNodeIsNull,
        "api",
        r#"^the JSON node "(?P<path>[^"]*)" should be null$"#,
        "asserts the node at this JSON path exists and is JSON null",
        OptionsSource::Http,
    ),
    assertion(
        StepId::JsonNodeNotNull,
        "api",
        r#"^the JSON node "(?P<path>[^"]*)" should not be null$"#,
        "asserts the node at this JSON path exists and is not JSON null",
        OptionsSource::Http,
    ),
    assertion(
        StepId::ResponseBodyNotContainsJson,
        "api",
        r#"^the response body does not contain JSON:$"#,
        "asserts the response body does not contain this JSON — same subset/order-independent matching as `contains`, inverted",
        OptionsSource::Http,
    ),
    assertion(
        StepId::JsonNodeNotContainsSubstring,
        "api",
        r#"^the JSON node "(?P<path>[^"]*)" should not contain "(?P<substring>[^"]*)"$"#,
        "asserts a string node does not contain this substring",
        OptionsSource::Http,
    ),
    assertion(
        StepId::ResponseBodyEmpty,
        "api",
        r#"^the response body should be empty$"#,
        "asserts the response body is empty",
        OptionsSource::Http,
    ),
    assertion(
        StepId::ResponseBodyContains,
        "api",
        r#"^the response body contains "(?P<text>[^"]*)"$"#,
        "asserts the raw response body contains this text — works for any content type",
        OptionsSource::Http,
    ),
    assertion(
        StepId::ResponseBodyNotContains,
        "api",
        r#"^the response body does not contain "(?P<text>[^"]*)"$"#,
        "asserts the raw response body does not contain this text",
        OptionsSource::Http,
    ),
    assertion(
        StepId::ResponseBodyMatches,
        "api",
        r#"^the response body matches "(?P<pattern>[^"]*)"$"#,
        "asserts the raw response body matches this regular expression",
        OptionsSource::Http,
    ),
    assertion(
        StepId::ResponseBodyNotMatches,
        "api",
        r#"^the response body does not match "(?P<pattern>[^"]*)"$"#,
        "asserts the raw response body does not match this regular expression",
        OptionsSource::Http,
    ),
    assertion(
        StepId::ResponseBodyHasElement,
        "api",
        r#"^the response body has element "(?P<selector>[^"]*)"$"#,
        "asserts an HTML (CSS selector) or XML (XPath) element matches in the response body",
        OptionsSource::Http,
    ),
    assertion(
        StepId::ResponseBodyHasElementWithText,
        "api",
        r#"^the response body has element "(?P<selector>[^"]*)" with text "(?P<text>[^"]*)"$"#,
        "asserts the matched elements' text, newline-joined if more than one, equals this value exactly",
        OptionsSource::Http,
    ),
    assertion(
        StepId::ResponseBodyNotHasElement,
        "api",
        r#"^the response body does not have element "(?P<selector>[^"]*)"$"#,
        "asserts no element matches this selector in the response body",
        OptionsSource::Http,
    ),
    action(
        StepId::SetVariableGlobal,
        "vars",
        r#"^set variable "(?P<name>[^"]*)" to "(?P<value>[^"]*)" global$"#,
        "sets a variable every scenario of this feature file can read",
    ),
    action(
        StepId::SetVariable,
        "vars",
        r#"^set variable "(?P<name>[^"]*)" to "(?P<value>[^"]*)"$"#,
        "sets a variable for the current scenario",
    ),
    action(
        StepId::ExtractFromJsonGlobal,
        "vars",
        r#"^extract "(?P<path>[^"]*)" from JSON as "(?P<name>[^"]*)" global$"#,
        "reads a JSON path of the response into a variable the whole file can see",
    ),
    action(
        StepId::ExtractFromJson,
        "vars",
        r#"^extract "(?P<path>[^"]*)" from JSON as "(?P<name>[^"]*)"$"#,
        "reads a JSON path of the response into a variable",
    ),
    action(
        StepId::ExtractFromCookiesGlobal,
        "vars",
        r#"^extract "(?P<cookie>[^"]*)" from cookies as "(?P<name>[^"]*)" global$"#,
        "reads a response cookie into a variable the whole file can see",
    ),
    action(
        StepId::ExtractFromCookies,
        "vars",
        r#"^extract "(?P<cookie>[^"]*)" from cookies as "(?P<name>[^"]*)"$"#,
        "reads a response cookie into a variable",
    ),
    action(
        StepId::ExtractFromMarkup,
        "vars",
        r#"^extract "(?P<selector>[^"]*)" from response body as "(?P<name>[^"]*)"$"#,
        "reads the matched elements' text, newline-joined if more than one, into a variable",
    ),
    assertion(
        StepId::VariableNotEquals,
        "vars",
        r#"^variable "(?P<name>[^"]*)" should not be equal to "(?P<value>[^"]*)"$"#,
        "asserts a variable differs from this value",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableEquals,
        "vars",
        r#"^variable "(?P<name>[^"]*)" should be equal to "(?P<value>[^"]*)"$"#,
        "asserts a variable holds exactly this value",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableContains,
        "vars",
        r#"^variable "(?P<name>[^"]*)" should contain "(?P<text>[^"]*)"$"#,
        "asserts a variable's text contains this substring",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableNotContains,
        "vars",
        r#"^variable "(?P<name>[^"]*)" should not contain "(?P<text>[^"]*)"$"#,
        "asserts a variable's text does not contain this substring",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableMatches,
        "vars",
        r#"^variable "(?P<name>[^"]*)" should match "(?P<pattern>[^"]*)"$"#,
        "asserts a variable's text matches this regular expression, unanchored",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableNotMatches,
        "vars",
        r#"^variable "(?P<name>[^"]*)" should not match "(?P<pattern>[^"]*)"$"#,
        "asserts a variable's text does not match this regular expression",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableEmpty,
        "vars",
        r#"^variable "(?P<name>[^"]*)" should be empty$"#,
        "asserts a variable's text is empty or whitespace only",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableContainsJson,
        "vars",
        r#"^variable "(?P<name>[^"]*)" should contain JSON:$"#,
        "asserts a variable's text is JSON containing this JSON — same matching as `the response body contains JSON`",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableEqualsJson,
        "vars",
        r#"^variable "(?P<name>[^"]*)" should equal JSON:$"#,
        "asserts a variable's text is JSON exactly equal to this JSON",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableNotContainsJson,
        "vars",
        r#"^variable "(?P<name>[^"]*)" should not contain JSON:$"#,
        "asserts a variable's text is JSON that does not contain this JSON",
        OptionsSource::Global,
    ),
    action(
        StepId::ExtractJsonFromVariableGlobal,
        "vars",
        r#"^extract "(?P<path>[^"]*)" from variable "(?P<variable>[^"]*)" as JSON as "(?P<name>[^"]*)" global$"#,
        "reads a JSON path of a variable's text into a variable the whole file can see",
    ),
    action(
        StepId::ExtractJsonFromVariable,
        "vars",
        r#"^extract "(?P<path>[^"]*)" from variable "(?P<variable>[^"]*)" as JSON as "(?P<name>[^"]*)"$"#,
        "reads a JSON path of a variable's text into a variable",
    ),
    action(
        StepId::ExtractRegexFromVariableGlobal,
        "vars",
        r#"^extract "(?P<pattern>[^"]*)" from variable "(?P<variable>[^"]*)" as "(?P<name>[^"]*)" global$"#,
        "reads the first capture group of a regex over a variable's text into a variable the whole file can see",
    ),
    action(
        StepId::ExtractRegexFromVariable,
        "vars",
        r#"^extract "(?P<pattern>[^"]*)" from variable "(?P<variable>[^"]*)" as "(?P<name>[^"]*)"$"#,
        "reads the first capture group of a regex over a variable's text into a variable",
    ),
    action(
        StepId::EncryptWithAes,
        "vars",
        r#"^I encrypt "(?P<value>[^"]*)" with AES using key "(?P<key>[^"]*)" as "(?P<name>[^"]*)"$"#,
        "encrypts a value with an AES key and stores the result in a variable",
    ),
    action(
        StepId::UseApi,
        "api",
        r#"^I use "(?P<name>[^"]*)" api$"#,
        "switches to another API resource and its default headers, until the scenario ends",
    ),
    action(
        StepId::UseConnection,
        "db",
        r#"^I use "(?P<name>[^"]*)" connection$"#,
        "switches to another database connection, until the scenario ends",
    ),
    action(
        StepId::DebugOn,
        "debug",
        r#"^I am in debug mode$"#,
        "prints the generated SQL, its binds and plugin exchanges to stderr",
    ),
    action(
        StepId::DebugOff,
        "debug",
        r#"^I am not in debug mode$"#,
        "stops the debug output turned on earlier in the scenario",
    ),
    action(
        StepId::HaveWhere,
        "db",
        r#"^I have "(?P<table>[^"]*)" where:$"#,
        "inserts one row per line of the table below, its header naming the columns",
    ),
    action(
        StepId::HaveWith,
        "db",
        r#"^I have "(?P<table>[^"]*)" with "(?P<pairs>[^"]*)"$"#,
        "inserts one row, filling required columns the pairs omit from the table schema",
    ),
    action(
        StepId::HaveMulti,
        "db",
        r#"^I have:$"#,
        "inserts rows into several tables, the first column of the table below naming each one",
    ),
    action(
        StepId::Update,
        "db",
        r#"^I update "(?P<table>[^"]*)" with "(?P<pairs>[^"]*)" where "(?P<condition>[^"]*)"$"#,
        "updates matching rows and stores the affected count in updated_<table>",
    ),
    action(
        StepId::DeleteAll,
        "db",
        r#"^I delete all "(?P<table>[^"]*)"$"#,
        "wipes the whole table — no WHERE, and no awareness of files running in parallel",
    ),
    action(
        StepId::DeleteWhere,
        "db",
        r#"^I delete "(?P<table>[^"]*)" where "(?P<condition>[^"]*)"$"#,
        "deletes matching rows and stores the affected count in deleted_<table>",
    ),
    action(
        StepId::ExtractFromDb,
        "db",
        r#"^I extract "(?P<column>[^"]*)" from "(?P<table>[^"]*)" with "(?P<condition>[^"]*)" as "(?P<name>[^"]*)"$"#,
        "reads a column of the first matching row into a variable",
    ),
    assertion(
        StepId::ShouldNotHaveTable,
        "db",
        r#"^I should not have "(?P<table>[^"]*)" with:$"#,
        "asserts no row matches any line of the table below",
        OptionsSource::Db,
    ),
    assertion(
        StepId::ShouldNotHaveWith,
        "db",
        r#"^I should not have "(?P<table>[^"]*)" with "(?P<pairs>[^"]*)"$"#,
        "asserts no row matches these column values",
        OptionsSource::Db,
    ),
    assertion(
        StepId::ShouldHaveTable,
        "db",
        r#"^I should have "(?P<table>[^"]*)" with:$"#,
        "asserts a row matches each line of the table below",
        OptionsSource::Db,
    ),
    assertion(
        StepId::ShouldHaveWith,
        "db",
        r#"^I should have "(?P<table>[^"]*)" with "(?P<pairs>[^"]*)"$"#,
        "asserts at least one row matches these column values",
        OptionsSource::Db,
    ),
    action(
        StepId::CallProcedure,
        "db",
        r#"^I call procedure "(?P<name>[^"]*)" with "(?P<arguments>[^"]*)"$"#,
        "calls a stored procedure with these arguments",
    ),
    action(
        StepId::CallFunction,
        "db",
        r#"^I call function "(?P<name>[^"]*)" with "(?P<arguments>[^"]*)" as "(?P<variable>[^"]*)"$"#,
        "calls a function and stores its return value in a variable",
    ),
    action(
        StepId::GetSequence,
        "db",
        r#"^I get next value of sequence "(?P<name>[^"]*)" as "(?P<variable>[^"]*)"$"#,
        "advances a sequence and stores the new value in a variable",
    ),
    action(
        StepId::Sleep,
        "general",
        r#"^I sleep "(?P<seconds>\d+)" seconds$"#,
        "pauses the scenario — a last resort, an eventual assertion is usually what you want",
    ),
    action(
        StepId::ShowAllVariables,
        "debug",
        r#"^Show all variables$"#,
        "prints every variable in scope to stderr",
    ),
    action(
        StepId::ShowVariable,
        "debug",
        r#"^Show "(?P<name>[^"]*)" variable$"#,
        "prints one variable to stderr, failing if it is not set",
    ),
    action(
        StepId::PrintResponseHeaders,
        "debug",
        r#"^Print response headers$"#,
        "prints the headers of the last response to stderr",
    ),
    action(
        StepId::PrintResponseBody,
        "debug",
        r#"^Print response body$"#,
        "prints the body of the last response to stderr, highlighted",
    ),
    action(
        StepId::PrintResponseBodyAsPath,
        "debug",
        r#"^Print response body as "(?P<path>[^"]*)"$"#,
        "prints one JSON path, XPath (XML) or CSS selector (HTML) selection of the last response to stderr",
    ),
    action(
        StepId::SrpVerifierWithSalt,
        "srp",
        r#"^I generate an SRP verifier for "(?P<username>[^"]*)" with password "(?P<password>[^"]*)" and salt "(?P<salt>[^"]*)" as "(?P<prefix>[^"]*)"$"#,
        "computes an SRP verifier from this salt into <prefix>_salt and <prefix>_verifier",
    ),
    action(
        StepId::SrpVerifier,
        "srp",
        r#"^I generate an SRP verifier for "(?P<username>[^"]*)" with password "(?P<password>[^"]*)" as "(?P<prefix>[^"]*)"$"#,
        "computes an SRP verifier from a fresh salt into <prefix>_salt and <prefix>_verifier",
    ),
    action(
        StepId::SrpStartLogin,
        "srp",
        r#"^I start an SRP login as "(?P<prefix>[^"]*)"$"#,
        "starts an SRP login, storing the client values in <prefix>_a and <prefix>_A",
    ),
    action(
        StepId::SrpCompleteLogin,
        "srp",
        r#"^I complete SRP login "(?P<prefix>[^"]*)" for "(?P<username>[^"]*)" with password "(?P<password>[^"]*)" salt "(?P<salt>[^"]*)" and "(?P<server_public>[^"]*)"$"#,
        "answers the server challenge into <prefix>_M1, <prefix>_M2 and <prefix>_sessionKey",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepTarget {
    Builtin { id: StepId, kind: StepKind },
    Macro(usize),
    /// A step served over FFI. It never becomes a `StepId`, which is what
    /// keeps `dispatch`'s exhaustive match (invariant 4) meaningful.
    Plugin {
        lib: usize,
        step: usize,
        assertion: bool,
    },
}

impl PartialEq<StepId> for StepTarget {
    fn eq(&self, other: &StepId) -> bool {
        matches!(self, Self::Builtin { id, .. } if id == other)
    }
}

#[derive(Debug)]
pub struct Registry {
    entries: Vec<(StepTarget, Regex)>,
    macros: Vec<MacroDef>,
}

impl Registry {
    #[cfg(test)]
    pub fn new() -> Result<Self, String> {
        Self::with_macros(MacroCatalog::default())
    }

    /// Only tests build a registry without plugins now: `main` always goes
    /// through `with_macros_and_plugins`, which is a superset.
    #[cfg(test)]
    pub fn with_macros(catalog: MacroCatalog) -> Result<Self, String> {
        Self::with_macros_and_plugins(catalog, &[], &[])
    }

    /// Everything is registered before macros are validated, so a macro body may
    /// name a plugin step. Order inside `entries` is irrelevant — `find` collects
    /// every match and reports ambiguity — but a plugin pattern that is not in the
    /// registry yet is indistinguishable from a typo.
    pub fn with_macros_and_plugins(
        catalog: MacroCatalog,
        plugin_steps: &[(usize, usize, String, bool)],
        plugin_groups: &[String],
    ) -> Result<Self, String> {
        let mut entries = Vec::with_capacity(BUILTIN_STEPS.len());
        for def in BUILTIN_STEPS {
            let re = Regex::new(def.pattern)
                .map_err(|e| format!("invalid step pattern {:?}: {e}", def.pattern))?;
            entries.push((
                StepTarget::Builtin {
                    id: def.id,
                    kind: def.kind,
                },
                re,
            ));
        }
        for (index, definition) in catalog.definitions.iter().enumerate() {
            entries.push((StepTarget::Macro(index), definition.regex.clone()));
        }
        let mut registry = Self {
            entries,
            macros: catalog.definitions,
        };
        for (lib, step, pattern, assertion) in plugin_steps {
            registry.add_plugin_step(*lib, *step, pattern, *assertion)?;
        }
        registry.add_use_group_step(plugin_groups)?;
        registry.validate_macros()?;
        Ok(registry)
    }

    /// Returns `Ok(None)` if the step is unknown, and `Err` if it is ambiguous.
    /// Ambiguity is an error, not "first wins": a silently shadowed step
    /// is more expensive to debug than a failed start.
    pub fn find(&self, text: &str) -> Result<Option<(StepTarget, Vec<String>)>, String> {
        let mut hits: Vec<(StepTarget, Vec<String>)> = Vec::new();
        for (target, re) in &self.entries {
            if let Some(c) = re.captures(text) {
                let caps = c
                    .iter()
                    .skip(1)
                    .map(|g| g.map(|m| m.as_str().to_string()).unwrap_or_default())
                    .collect();
                hits.push((*target, caps));
            }
        }
        match hits.len() {
            0 => Ok(None),
            1 => Ok(Some(hits.remove(0))),
            _ => Err(format!(
                "step {text:?} matches several definitions: {:?}",
                hits.iter().map(|(target, _)| target).collect::<Vec<_>>()
            )),
        }
    }

    pub fn macro_def(&self, index: usize) -> &MacroDef {
        &self.macros[index]
    }

    /// Registers one pattern a plugin declared. Called at startup, before the
    /// first request, so `validate::check` still sees every step (invariant 1).
    ///
    /// ponytail: no static overlap analysis between two arbitrary plugin
    /// regexes — `find` reports ambiguity per step and `validate::check` runs
    /// it over every selected step, which catches every collision that can
    /// actually fire. Upgrade path if a startup-time check is ever wanted:
    /// extend the `patterns_overlap` token model in this file to raw regexes.
    pub fn add_plugin_step(
        &mut self,
        lib: usize,
        step: usize,
        pattern: &str,
        assertion: bool,
    ) -> Result<(), String> {
        let re = Regex::new(pattern)
            .map_err(|e| format!("invalid plugin step pattern {pattern:?}: {e}"))?;
        self.entries
            .push((StepTarget::Plugin { lib, step, assertion }, re));
        Ok(())
    }

    /// Registers `I use "<name>" <group>` over the groups the loaded plugins
    /// actually claim. The alternation is built from the real group names, so
    /// the pattern cannot overlap `I use "<name>" api` or
    /// `I use "<conn>" connection` and no existing .feature changes meaning.
    pub fn add_use_group_step(&mut self, groups: &[String]) -> Result<(), String> {
        if groups.is_empty() {
            return Ok(());
        }
        let alternation = groups
            .iter()
            .map(|g| regex::escape(g))
            .collect::<Vec<_>>()
            .join("|");
        let pattern = format!(r#"^I use "([^"]*)" ({alternation})$"#);
        let re = Regex::new(&pattern)
            .map_err(|e| format!("invalid group-switch pattern {pattern:?}: {e}"))?;
        self.entries.push((
            StepTarget::Builtin {
                id: StepId::UsePluginInstance,
                kind: StepKind::Action,
            },
            re,
        ));
        Ok(())
    }

    fn validate_macros(&self) -> Result<(), String> {
        for (left_index, left) in self.macros.iter().enumerate() {
            for builtin in BUILTIN_STEPS {
                if builtin_patterns(builtin.pattern)
                    .iter()
                    .any(|pattern| patterns_overlap(&macro_pattern(&left.step), pattern))
                {
                    return Err(format!(
                        "macro step {:?} from {}:{} conflicts with builtin step {:?}",
                        left.step,
                        left.source.display(),
                        left.line,
                        builtin.pattern
                    ));
                }
            }
            for right in self.macros.iter().skip(left_index + 1) {
                if patterns_overlap(&macro_pattern(&left.step), &macro_pattern(&right.step)) {
                    return Err(format!(
                        "macro step {:?} from {}:{} conflicts with {:?} from {}:{}",
                        left.step,
                        left.source.display(),
                        left.line,
                        right.step,
                        right.source.display(),
                        right.line,
                    ));
                }
            }
        }

        let mut graph = vec![Vec::new(); self.macros.len()];
        for (index, definition) in self.macros.iter().enumerate() {
            for step in &definition.body {
                match self.find(&step.text)? {
                    Some((StepTarget::Builtin { .. }, _)) => {}
                    Some((StepTarget::Plugin { .. }, _)) => {}
                    Some((StepTarget::Macro(_), _)) if step.docstring.is_some() => {
                        return Err(format!(
                            "macro call {:?} with a doc string inside macro {:?} from {}:{} is not supported",
                            step.text,
                            definition.step,
                            definition.source.display(),
                            definition.line,
                        ));
                    }
                    Some((StepTarget::Macro(target), _)) => graph[index].push(target),
                    None => {
                        return Err(format!(
                            "unknown step {:?} in macro {:?} from {}:{}",
                            step.text,
                            definition.step,
                            definition.source.display(),
                            definition.line,
                        ));
                    }
                }
            }
        }

        let mut visited = vec![false; self.macros.len()];
        let mut visiting = vec![false; self.macros.len()];
        let mut path = Vec::new();
        for index in 0..self.macros.len() {
            self.visit(index, &graph, &mut visited, &mut visiting, &mut path)?;
        }
        let mut depths = vec![None; self.macros.len()];
        for index in 0..self.macros.len() {
            if macro_depth(index, &graph, &mut depths) > 16 {
                return Err(format!(
                    "macro nesting exceeds 16 starting from step {:?}",
                    self.macros[index].step
                ));
            }
        }
        Ok(())
    }

    fn visit(
        &self,
        index: usize,
        graph: &[Vec<usize>],
        visited: &mut [bool],
        visiting: &mut [bool],
        path: &mut Vec<usize>,
    ) -> Result<(), String> {
        if visiting[index] {
            let start = path.iter().position(|item| *item == index).unwrap_or(0);
            let mut cycle: Vec<&str> = path[start..]
                .iter()
                .map(|item| self.macros[*item].step.as_str())
                .collect();
            cycle.push(self.macros[index].step.as_str());
            return Err(format!("cycle in macros: {}", cycle.join(" → ")));
        }
        if visited[index] {
            return Ok(());
        }
        visiting[index] = true;
        path.push(index);
        for child in &graph[index] {
            self.visit(*child, graph, visited, visiting, path)?;
        }
        path.pop();
        visiting[index] = false;
        visited[index] = true;
        Ok(())
    }
}

fn macro_depth(index: usize, graph: &[Vec<usize>], memo: &mut [Option<usize>]) -> usize {
    if let Some(depth) = memo[index] {
        return depth;
    }
    let depth = 1 + graph[index]
        .iter()
        .map(|child| macro_depth(*child, graph, memo))
        .max()
        .unwrap_or(0);
    memo[index] = Some(depth);
    depth
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CharClass {
    Any,
    NonQuote,
    Digit,
    Exact(char),
}

static DIGIT_CHAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d$").expect("constant digit regex"));

#[derive(Clone, Copy, Debug)]
enum PatternToken {
    One(CharClass),
    Star(CharClass),
}

static MACRO_PARAM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{[A-Za-z_][A-Za-z0-9_]*\}").expect("constant macro parameter regex")
});

fn macro_pattern(template: &str) -> Vec<PatternToken> {
    let mut tokens = Vec::new();
    let mut last = 0;
    for parameter in MACRO_PARAM.find_iter(template) {
        tokens.extend(
            template[last..parameter.start()]
                .chars()
                .map(|char_| PatternToken::One(CharClass::Exact(char_))),
        );
        tokens.push(PatternToken::Star(CharClass::Any));
        last = parameter.end();
    }
    tokens.extend(
        template[last..]
            .chars()
            .map(|char_| PatternToken::One(CharClass::Exact(char_))),
    );
    tokens
}

static GROUP_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\?P?<[A-Za-z_][A-Za-z0-9_]*>").expect("constant group-name regex")
});

fn builtin_patterns(pattern: &str) -> Vec<Vec<PatternToken>> {
    // A group name is display metadata for `bddkit steps list`, and the
    // tokenizer below recognizes `([^"]*)`, `(\d+)` and the method alternation
    // by their literal text. An unrecognized construct does not fail here — it
    // degrades into literal characters, which would quietly stop a macro from
    // conflicting with the builtin it shadows. Stripping the names first leaves
    // the token stream exactly as it was before the table was annotated.
    let pattern = &*GROUP_NAME.replace_all(pattern, "");
    const METHODS: &str = "(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)";
    let variants: Vec<String> = if pattern.contains(METHODS) {
        ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]
            .iter()
            .map(|method| pattern.replace(METHODS, method))
            .collect()
    } else {
        vec![pattern.to_string()]
    };
    variants
        .iter()
        .map(|variant| {
            let source = variant.trim_start_matches('^').trim_end_matches('$');
            let mut tokens = Vec::new();
            let mut rest = source;
            while !rest.is_empty() {
                if let Some(tail) = rest.strip_prefix(r#"([^"]*)"#) {
                    tokens.push(PatternToken::Star(CharClass::NonQuote));
                    rest = tail;
                } else if let Some(tail) = rest.strip_prefix(r"(\d+)") {
                    tokens.push(PatternToken::One(CharClass::Digit));
                    tokens.push(PatternToken::Star(CharClass::Digit));
                    rest = tail;
                } else {
                    let char_ = rest.chars().next().expect("string is not empty");
                    tokens.push(PatternToken::One(CharClass::Exact(char_)));
                    rest = &rest[char_.len_utf8()..];
                }
            }
            tokens
        })
        .collect()
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
        let Some(left_token) = left.get(left_pos) else {
            continue;
        };
        let Some(right_token) = right.get(right_pos) else {
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
    use CharClass::{Any, Digit, Exact, NonQuote};
    match (left, right) {
        (Any, _)
        | (_, Any)
        | (NonQuote, NonQuote)
        | (NonQuote, Digit)
        | (Digit, NonQuote)
        | (Digit, Digit) => true,
        (NonQuote, Exact(char_)) | (Exact(char_), NonQuote) => char_ != '"',
        (Digit, Exact(char_)) | (Exact(char_), Digit) => DIGIT_CHAR.is_match(&char_.to_string()),
        (Exact(left), Exact(right)) => left == right,
    }
}

/// Step arguments after interpolation.
pub struct Args {
    pub caps: Vec<String>,
    pub docstring: Option<String>,
    pub table: Option<Vec<Vec<String>>>,
}

impl Args {
    fn cap(&self, i: usize) -> &str {
        self.caps.get(i).map(String::as_str).unwrap_or("")
    }
}

/// One `match` instead of boxed futures and trait objects.
pub async fn dispatch(w: &mut World, id: StepId, a: &Args, attempt: u64) -> AttemptResult {
    let result: Result<(), String> = match id {
        StepId::SetRequestHeader => api::set_header(w, a.cap(0), a.cap(1)),
        // The step names the value before the name: `I add "V" to the "H" request header`.
        StepId::AddRequestHeader => api::add_header(w, a.cap(1), a.cap(0)),
        StepId::SetQueryParam => api::set_query(w, a.cap(0), a.cap(1)),
        StepId::SetRequestBody => api::set_body(w, a.docstring.as_ref()),
        StepId::EmptyRequestBody => api::clear_body(w),
        StepId::SetFormParams => api::set_form(w, a.table.as_ref()),
        StepId::SignNextRequestWithHawk => api::sign_next_request_with_hawk(w, a.cap(0), a.cap(1)),
        StepId::RequestPath => api::request(w, a.cap(0), "GET").await,
        StepId::RequestPathWithMethod => {
            let (p, m) = (a.cap(0).to_string(), a.cap(1).to_string());
            api::request(w, &p, &m).await
        }
        StepId::ExpectEventually => {
            w.arm_options(OptionsLayer::default());
            Ok(())
        }
        StepId::ExpectWithin => {
            let timeout_secs = parse_positive(a.cap(0))?;
            w.arm_options(OptionsLayer {
                polling: Some(PollingOptionsLayer {
                    timeout_secs: Some(timeout_secs),
                    interval_ms: None,
                }),
            });
            Ok(())
        }
        StepId::ExpectWithinEvery => {
            let timeout_secs = parse_positive(a.cap(0))?;
            let interval_ms = parse_positive(a.cap(1))?;
            if interval_ms > timeout_secs.saturating_mul(1000) {
                Err("polling interval_ms must not exceed timeout_secs".to_string())
            } else {
                w.arm_options(OptionsLayer {
                    polling: Some(PollingOptionsLayer {
                        timeout_secs: Some(timeout_secs),
                        interval_ms: Some(interval_ms),
                    }),
                });
                Ok(())
            }
        }
        StepId::ResponseCode => {
            assert::replay_response(w, attempt).await?;
            return assert::response_code(w, a.cap(0));
        }
        StepId::ResponseHeader => {
            assert::replay_response(w, attempt).await?;
            return assert::response_header(w, a.cap(0), a.cap(1));
        }
        StepId::ResponseBodyContainsJson => {
            assert::replay_response(w, attempt).await?;
            return assert::body_contains_json(w, a.docstring.as_ref());
        }
        StepId::ResponseBodyEqualsJson => {
            assert::replay_response(w, attempt).await?;
            return assert::body_equals_json(w, a.docstring.as_ref());
        }
        StepId::ResponseArrayLength => {
            assert::replay_response(w, attempt).await?;
            return assert::array_length(w, a.cap(0));
        }
        StepId::JsonNodeExists => {
            assert::replay_response(w, attempt).await?;
            return assert::json_node_exists(w, a.cap(0));
        }
        StepId::JsonNodeNotExists => {
            assert::replay_response(w, attempt).await?;
            return assert::json_node_not_exists(w, a.cap(0));
        }
        StepId::JsonNodeIsNull => {
            assert::replay_response(w, attempt).await?;
            return assert::json_node_is_null(w, a.cap(0));
        }
        StepId::JsonNodeNotNull => {
            assert::replay_response(w, attempt).await?;
            return assert::json_node_not_null(w, a.cap(0));
        }
        StepId::ResponseBodyNotContainsJson => {
            assert::replay_response(w, attempt).await?;
            return assert::body_not_contains_json(w, a.docstring.as_ref());
        }
        StepId::JsonNodeNotContainsSubstring => {
            assert::replay_response(w, attempt).await?;
            return assert::json_node_not_contains(w, a.cap(0), a.cap(1));
        }
        StepId::ResponseBodyEmpty => {
            assert::replay_response(w, attempt).await?;
            return assert::body_empty(w);
        }
        StepId::ResponseBodyContains => {
            assert::replay_response(w, attempt).await?;
            return assert::body_contains_text(w, a.cap(0));
        }
        StepId::ResponseBodyNotContains => {
            assert::replay_response(w, attempt).await?;
            return assert::body_not_contains_text(w, a.cap(0));
        }
        StepId::ResponseBodyMatches => {
            assert::replay_response(w, attempt).await?;
            return assert::body_matches(w, a.cap(0));
        }
        StepId::ResponseBodyNotMatches => {
            assert::replay_response(w, attempt).await?;
            return assert::body_not_matches(w, a.cap(0));
        }
        StepId::ResponseBodyHasElement => {
            assert::replay_response(w, attempt).await?;
            return assert::body_has_element(w, a.cap(0));
        }
        StepId::ResponseBodyHasElementWithText => {
            assert::replay_response(w, attempt).await?;
            return assert::body_has_element_with_text(w, a.cap(0), a.cap(1));
        }
        StepId::ResponseBodyNotHasElement => {
            assert::replay_response(w, attempt).await?;
            return assert::body_not_has_element(w, a.cap(0));
        }
        StepId::SetVariable => vars::set_variable(w, a.cap(0), a.cap(1), false),
        StepId::SetVariableGlobal => vars::set_variable(w, a.cap(0), a.cap(1), true),
        StepId::ExtractFromJson => vars::extract_from_json(w, a.cap(0), a.cap(1), false),
        StepId::ExtractFromJsonGlobal => vars::extract_from_json(w, a.cap(0), a.cap(1), true),
        StepId::ExtractFromCookies => vars::extract_from_cookies(w, a.cap(0), a.cap(1), false),
        StepId::ExtractFromCookiesGlobal => vars::extract_from_cookies(w, a.cap(0), a.cap(1), true),
        StepId::ExtractFromMarkup => vars::extract_from_markup(w, a.cap(0), a.cap(1)),
        StepId::VariableEquals => {
            return vars::variable_equals(w, a.cap(0), a.cap(1), false);
        }
        StepId::VariableNotEquals => {
            return vars::variable_equals(w, a.cap(0), a.cap(1), true);
        }
        StepId::VariableContains => return vars::variable_contains(w, a.cap(0), a.cap(1), false),
        StepId::VariableNotContains => return vars::variable_contains(w, a.cap(0), a.cap(1), true),
        StepId::VariableMatches => return vars::variable_matches(w, a.cap(0), a.cap(1), false),
        StepId::VariableNotMatches => return vars::variable_matches(w, a.cap(0), a.cap(1), true),
        StepId::VariableEmpty => return vars::variable_empty(w, a.cap(0)),
        StepId::VariableContainsJson => {
            return vars::variable_contains_json(w, a.cap(0), a.docstring.as_ref());
        }
        StepId::VariableEqualsJson => {
            return vars::variable_equals_json(w, a.cap(0), a.docstring.as_ref());
        }
        StepId::VariableNotContainsJson => {
            return vars::variable_not_contains_json(w, a.cap(0), a.docstring.as_ref());
        }
        StepId::ExtractJsonFromVariable => {
            vars::extract_json_from_variable(w, a.cap(0), a.cap(1), a.cap(2), false)
        }
        StepId::ExtractJsonFromVariableGlobal => {
            vars::extract_json_from_variable(w, a.cap(0), a.cap(1), a.cap(2), true)
        }
        StepId::ExtractRegexFromVariable => {
            vars::extract_regex_from_variable(w, a.cap(0), a.cap(1), a.cap(2), false)
        }
        StepId::ExtractRegexFromVariableGlobal => {
            vars::extract_regex_from_variable(w, a.cap(0), a.cap(1), a.cap(2), true)
        }
        StepId::EncryptWithAes => vars::encrypt_with_aes(w, a.cap(0), a.cap(1), a.cap(2)),
        StepId::UseApi => api::use_api(w, a.cap(0)),
        StepId::UsePluginInstance => plugin::use_instance(w, a.cap(1), a.cap(0)),
        StepId::UseConnection => db::use_connection(w, a.cap(0)),
        StepId::DebugOn => db::debug_on(w),
        StepId::DebugOff => db::debug_off(w),
        StepId::HaveWith => db::have_with(w, a.cap(0), a.cap(1)).await,
        StepId::HaveWhere => db::have_where(w, a.cap(0), a.table.as_ref()).await,
        StepId::HaveMulti => db::have_multi(w, a.table.as_ref()).await,
        StepId::Update => db::update(w, a.cap(0), a.cap(1), a.cap(2)).await,
        StepId::DeleteWhere => db::delete_where(w, a.cap(0), a.cap(1)).await,
        StepId::DeleteAll => db::delete_all(w, a.cap(0)).await,
        StepId::ExtractFromDb => {
            db::extract_from_db(w, a.cap(0), a.cap(1), a.cap(2), a.cap(3)).await
        }
        StepId::ShouldHaveWith => return db::should_have(w, a.cap(0), a.cap(1)).await,
        StepId::ShouldHaveTable => {
            return db::should_have_table(w, a.cap(0), a.table.as_ref()).await;
        }
        StepId::ShouldNotHaveWith => return db::should_not_have(w, a.cap(0), a.cap(1)).await,
        StepId::ShouldNotHaveTable => {
            return db::should_not_have_table(w, a.cap(0), a.table.as_ref()).await;
        }
        StepId::CallProcedure => db::call_procedure(w, a.cap(0), a.cap(1)).await,
        StepId::CallFunction => db::call_function(w, a.cap(0), a.cap(1), a.cap(2)).await,
        StepId::GetSequence => db::get_sequence(w, a.cap(0), a.cap(1)).await,
        StepId::Sleep => sleep(a.cap(0)).await,
        StepId::ShowAllVariables => show_all_variables(w),
        StepId::ShowVariable => show_variable(w, a.cap(0)),
        StepId::PrintResponseHeaders => debug::print_headers(w),
        StepId::PrintResponseBody => debug::print_body(w),
        StepId::PrintResponseBodyAsPath => debug::print_body_as(w, a.cap(0)),
        StepId::SrpVerifier => srp::generate_verifier(w, a.cap(0), a.cap(1), None, a.cap(2)),
        StepId::SrpVerifierWithSalt => {
            srp::generate_verifier(w, a.cap(0), a.cap(1), Some(a.cap(2)), a.cap(3))
        }
        StepId::SrpStartLogin => srp::start_login(w, a.cap(0)),
        StepId::SrpCompleteLogin => {
            srp::complete_login(w, a.cap(0), a.cap(1), a.cap(2), a.cap(3), a.cap(4))
        }
    };
    result.map_err(AttemptError::Fatal)
}

fn parse_positive(value: &str) -> Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("polling value must be a positive integer: {value:?}"))?;
    if parsed == 0 {
        Err("polling value must be positive".to_string())
    } else {
        Ok(parsed)
    }
}

async fn sleep(secs: &str) -> Result<(), String> {
    let n = secs
        .parse::<u64>()
        .map_err(|_| format!("not a number: {secs}"))?;
    tokio::time::sleep(std::time::Duration::from_secs(n)).await;
    Ok(())
}

fn show_all_variables(w: &World) -> Result<(), String> {
    eprintln!("=== All variables ===");
    for (name, value) in w.vars.all_vars() {
        eprintln!("{}: {}", name, value);
    }
    Ok(())
}

fn show_variable(w: &World, name: &str) -> Result<(), String> {
    match w.vars.get(name) {
        Some(v) => {
            eprintln!("{}: {}", name, v);
            Ok(())
        }
        None => Err(format!("variable not found: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> Registry {
        Registry::new().unwrap()
    }

    #[test]
    fn all_builtin_patterns_compile() {
        assert!(Registry::new().is_ok());
    }

    #[test]
    fn a_plugin_step_is_found_like_any_other() {
        let mut reg = Registry::new().expect("registry");
        reg.add_plugin_step(0, 1, r#"^I upload file "([^"]*)" to "([^"]*)"$"#, false)
            .expect("valid pattern");
        let (target, caps) = reg
            .find(r#"I upload file "report.pdf" to "backups""#)
            .expect("no ambiguity")
            .expect("matched");
        assert_eq!(
            target,
            StepTarget::Plugin { lib: 0, step: 1, assertion: false }
        );
        assert_eq!(caps, vec!["report.pdf".to_string(), "backups".to_string()]);
    }

    #[test]
    fn an_invalid_plugin_pattern_is_rejected_at_registration() {
        let mut reg = Registry::new().expect("registry");
        let error = reg
            .add_plugin_step(0, 0, "^I upload file \"([^\"]*$", false)
            .expect_err("unbalanced group");
        assert!(error.contains("plugin"), "{error}");
    }

    #[test]
    fn a_plugin_pattern_colliding_with_a_builtin_is_an_ambiguity_error() {
        // Cross-layer ambiguity needs no separate checker: `find` already reports
        // every match, and validate::check runs `find` over every selected step
        // before the first request.
        let mut reg = Registry::new().expect("registry");
        reg.add_plugin_step(0, 0, r#"^the response code is (\d+)$"#, true)
            .expect("valid pattern");
        let error = reg
            .find("the response code is 200")
            .expect_err("two definitions match");
        assert!(error.contains("several definitions"), "{error}");
    }

    #[test]
    fn the_group_switch_step_does_not_shadow_api_or_connection() {
        let reg = Registry::with_macros_and_plugins(
            MacroCatalog::default(),
            &[],
            &["widget".to_string(), "browser".to_string()],
        )
        .expect("registry");

        let (api_target, _) = reg
            .find(r#"I use "main" api"#)
            .expect("no ambiguity")
            .expect("matched");
        assert_eq!(api_target, StepTarget::Builtin { id: StepId::UseApi, kind: StepKind::Action });

        let (conn_target, _) = reg
            .find(r#"I use "pg" connection"#)
            .expect("no ambiguity")
            .expect("matched");
        assert_eq!(
            conn_target,
            StepTarget::Builtin { id: StepId::UseConnection, kind: StepKind::Action }
        );

        let (group_target, caps) = reg
            .find(r#"I use "bucket-a" widget"#)
            .expect("no ambiguity")
            .expect("matched");
        assert_eq!(
            group_target,
            StepTarget::Builtin { id: StepId::UsePluginInstance, kind: StepKind::Action }
        );
        assert_eq!(caps, vec!["bucket-a".to_string(), "widget".to_string()], "name before group");
    }

    #[test]
    fn a_group_name_with_regex_metacharacters_is_escaped_not_interpreted() {
        let reg = Registry::with_macros_and_plugins(
            MacroCatalog::default(),
            &[],
            &["widget.beta".to_string()],
        )
        .expect("registry");

        // If the dot were left as a regex metachar it would match any single
        // character here too, not just a literal dot.
        assert!(
            reg.find(r#"I use "x" widgetXbeta"#).expect("no ambiguity").is_none(),
            "an unescaped '.' would wrongly match any character"
        );
        assert!(
            reg.find(r#"I use "x" widget.beta"#).expect("no ambiguity").is_some(),
            "the literal group name must still match"
        );
    }

    #[test]
    fn every_builtin_declares_a_known_group_and_a_description() {
        // The group is what `bddkit steps list` prints and filters on: a typo
        // there hides a whole step from discovery without failing anything else.
        const GROUPS: &[&str] = &["api", "db", "srp", "vars", "debug", "general"];
        for def in BUILTIN_STEPS {
            assert!(
                GROUPS.contains(&def.group),
                "{:?}: unknown group {:?}",
                def.id,
                def.group
            );
            assert!(
                !def.description.is_empty(),
                "{:?} has no description",
                def.id
            );
        }
    }

    #[test]
    fn a_macro_conflicting_with_a_named_builtin_is_still_rejected() {
        // `builtin_patterns` recognizes `([^"]*)` by its literal text, and an
        // unrecognized construct degrades into literal characters rather than
        // failing. Naming the group without normalizing it first would silently
        // stop this collision from being seen at all.
        //
        // The macro is deliberately parameterless: a `{param}` becomes a
        // wildcard that swallows even unrecognized literals, so it would report
        // a conflict either way. Here the BUILTIN's capture is the side that has
        // to expand, which it can only do once it is recognized as a capture.
        let path = std::env::temp_dir().join(format!(
            "bddkit-steps-named-group-conflict-{}.yaml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "- step: I request \"/ping\"\n  do: [Show all variables]\n",
        )
        .expect("write macro file");
        let catalog = MacroCatalog::load(std::slice::from_ref(&path)).expect("macro loads");
        std::fs::remove_file(&path).ok();

        let error = Registry::with_macros(catalog).expect_err("the macro shadows a builtin");
        assert!(error.contains("conflicts with builtin"), "{error}");
    }

    #[test]
    fn a_macro_body_may_call_a_plugin_step() {
        // Macros exist to compose steps and plugins exist to add steps.
        // Validating macros before the plugin patterns are registered made a
        // plugin step in a macro body indistinguishable from a typo.
        let path = std::env::temp_dir().join(format!(
            "bddkit-steps-macro-calls-plugin-{}.yaml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "- step: I do the plugin thing\n  do: [I upload file \"a\" to \"b\"]\n",
        )
        .expect("write macro file");
        let catalog = MacroCatalog::load(std::slice::from_ref(&path)).expect("macro loads");
        std::fs::remove_file(&path).ok();

        let reg = Registry::with_macros_and_plugins(
            catalog,
            &[(0, 1, r#"^I upload file "([^"]*)" to "([^"]*)"$"#.to_string(), false)],
            &[],
        )
        .expect("a plugin step in a macro body is not a typo");

        let (target, _) = reg
            .find("I do the plugin thing")
            .expect("no ambiguity")
            .expect("matched");
        assert_eq!(target, StepTarget::Macro(0));
    }

    #[test]
    fn a_macro_body_calling_a_truly_unknown_step_still_fails() {
        let path = std::env::temp_dir().join(format!(
            "bddkit-steps-macro-calls-unknown-{}.yaml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "- step: I do the mystery thing\n  do: [I have never heard of this step]\n",
        )
        .expect("write macro file");
        let catalog = MacroCatalog::load(std::slice::from_ref(&path)).expect("macro loads");
        std::fs::remove_file(&path).ok();

        let error = Registry::with_macros_and_plugins(catalog, &[], &[])
            .expect_err("no builtin, macro, or plugin step matches");
        assert!(error.contains("unknown step"), "{error}");
    }

    #[test]
    fn eventual_modifiers_are_registered_as_actions() {
        let cases = [
            (
                "I expect the next assertion to pass eventually",
                StepId::ExpectEventually,
                vec![],
            ),
            (
                r#"I expect the next assertion to pass within "10" seconds"#,
                StepId::ExpectWithin,
                vec!["10".to_string()],
            ),
            (
                r#"I expect the next assertion to pass within "10" seconds, checking every "100" milliseconds"#,
                StepId::ExpectWithinEvery,
                vec!["10".to_string(), "100".to_string()],
            ),
        ];

        for (text, expected_id, expected_caps) in cases {
            let (target, caps) = reg().find(text).unwrap().expect("step is registered");
            let StepTarget::Builtin { id, kind } = target else {
                panic!("modifier resolved to a macro");
            };
            assert_eq!(id, expected_id);
            assert_eq!(kind, StepKind::Action);
            assert_eq!(caps, expected_caps);
        }
    }

    #[test]
    fn assertions_keep_their_option_source_in_registry_metadata() {
        let cases = [
            ("the response code is 200", OptionsSource::Http),
            (
                r#"variable "state" should be equal to "ready""#,
                OptionsSource::Global,
            ),
            (
                r#"I should have "users" with "state: ready""#,
                OptionsSource::Db,
            ),
        ];

        for (text, source) in cases {
            let (target, _) = reg().find(text).unwrap().expect("assertion is registered");
            let StepTarget::Builtin { kind, .. } = target else {
                panic!("assertion resolved to a macro");
            };
            assert_eq!(kind, StepKind::Assertion(source));
        }
    }

    #[test]
    fn no_two_builtin_steps_are_ambiguous() {
        // Every pattern is checked against all the others on concrete examples.
        let samples = [
            r#"the "Accept" request header is "application/json""#,
            r#"I add "b" to the "Accept" request header"#,
            r#"the query parameter "q" is "1""#,
            "the request body is:",
            "the request body is empty",
            "the request form parameters are:",
            r#"I request "/ping" using HTTP POST"#,
            r#"I request "/ping""#,
            "the response code is 200",
            "the response body contains JSON:",
            "the response body equals JSON:",
            "the response body is a JSON array of length 3",
            r#"the response body contains "hello""#,
            r#"the response body does not contain "hello""#,
            r#"the response body matches "h[0-9]""#,
            r#"the response body does not match "h[0-9]""#,
            r#"the response body has element "h1#title""#,
            r#"the response body has element "h1#title" with text "Hi""#,
            r#"the response body does not have element "h1#title""#,
            r#"the "X-Trace" response header is "abc""#,
            r#"the JSON node "data.id" should exist"#,
            r#"set variable "a" to "1""#,
            r#"set variable "a" to "1" global"#,
            r#"extract "id" from JSON as "userId""#,
            r#"extract "id" from JSON as "userId" global"#,
            r#"extract "jwt" from cookies as "t""#,
            r#"extract "jwt" from cookies as "t" global"#,
            r#"extract "h1#title" from response body as "pageTitle""#,
            r#"variable "a" should be equal to "1""#,
            r#"variable "a" should not be equal to "1""#,
            r#"variable "a" should contain "1""#,
            r#"variable "a" should not contain "1""#,
            r#"variable "a" should match "^\d+$""#,
            r#"variable "a" should not match "^\d+$""#,
            r#"variable "a" should be empty"#,
            "variable \"a\" should contain JSON:",
            "variable \"a\" should equal JSON:",
            "variable \"a\" should not contain JSON:",
            r#"extract "id" from variable "a" as JSON as "b""#,
            r#"extract "id" from variable "a" as JSON as "b" global"#,
            r#"extract "(\d+)" from variable "a" as "b""#,
            r#"extract "(\d+)" from variable "a" as "b" global"#,
            r#"I encrypt "555555" with AES using key "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f" as "otp""#,
            r#"I use "main" connection"#,
            r#"I use "billing" api"#,
            "I am in debug mode",
            "I am not in debug mode",
            r#"I have "users" where:"#,
            r#"I have "users" with "email: a@b.net""#,
            "I have:",
            r#"I update "companies" with "name: x" where "id: 1""#,
            r#"I delete "companies" where "slug: x""#,
            r#"I delete all "companies""#,
            r#"I extract "id" from "companies" with "slug: x" as "cid""#,
            r#"I call procedure "p" with "a: 1""#,
            r#"I call function "f" with "a: 1" as "r""#,
            r#"I get next value of sequence "s" as "n""#,
            r#"I sleep "5" seconds"#,
            "Show all variables",
            r#"Show "userId" variable"#,
            "Print response headers",
            "Print response body",
            r#"Print response body as "status""#,
            r#"I generate an SRP verifier for "u@example.test" with password "p" as "reg""#,
            r#"I generate an SRP verifier for "u@example.test" with password "p" and salt "ab" as "reg""#,
            r#"I start an SRP login as "srp""#,
            r#"I complete SRP login "srp" for "u@example.test" with password "p" salt "ab" and "cd""#,
            r#"I sign the next request with Hawk id "session-1" and key "abc""#,
        ];
        let r = reg();
        for s in samples {
            match r.find(s) {
                Ok(Some(_)) => {}
                Ok(None) => panic!("step not recognized: {s}"),
                Err(e) => panic!("ambiguity: {e}"),
            }
        }
    }

    #[test]
    fn the_use_api_step_resolves_to_its_builtin() {
        let (target, caps) = reg()
            .find(r#"I use "billing" api"#)
            .expect("pattern is not ambiguous")
            .expect("step is declared");
        assert!(target == StepId::UseApi, "the step must resolve to UseApi");
        assert_eq!(caps, vec!["billing".to_string()]);
    }

    #[test]
    fn the_aes_encryption_step_resolves_to_its_builtin() {
        let (target, caps) = reg()
            .find(
                r#"I encrypt "555555" with AES using key "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f" as "otp""#,
            )
            .expect("pattern is unambiguous")
            .expect("step is declared");

        assert!(target == StepId::EncryptWithAes);
        assert_eq!(
            caps,
            vec![
                "555555",
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
                "otp",
            ]
        );
    }

    #[test]
    fn the_sign_next_request_with_hawk_step_resolves_to_its_builtin() {
        let (target, caps) = reg()
            .find(r#"I sign the next request with Hawk id "session-1" and key "abc""#)
            .expect("pattern is not ambiguous")
            .expect("step is declared");
        assert!(
            target == StepId::SignNextRequestWithHawk,
            "the step must resolve to SignNextRequestWithHawk"
        );
        assert_eq!(caps, vec!["session-1".to_string(), "abc".to_string()]);
    }

    #[test]
    fn captures_path_and_method() {
        let (id, caps) = reg()
            .find(r#"I request "/api/v1/x" using HTTP PATCH"#)
            .unwrap()
            .unwrap();
        assert_eq!(id, StepId::RequestPathWithMethod);
        assert_eq!(caps, vec!["/api/v1/x".to_string(), "PATCH".to_string()]);
    }

    #[test]
    fn bare_request_defaults_to_get_variant() {
        let (id, caps) = reg().find(r#"I request "/ping""#).unwrap().unwrap();
        assert_eq!(id, StepId::RequestPath);
        assert_eq!(caps, vec!["/ping".to_string()]);
    }

    #[test]
    fn global_suffix_picks_the_global_variant() {
        let (id, _) = reg()
            .find(r#"set variable "a" to "1" global"#)
            .unwrap()
            .unwrap();
        assert_eq!(id, StepId::SetVariableGlobal);
        let (id, _) = reg().find(r#"set variable "a" to "1""#).unwrap().unwrap();
        assert_eq!(id, StepId::SetVariable);
    }

    #[test]
    fn unknown_method_is_not_a_known_step() {
        assert!(
            reg()
                .find(r#"I request "/ping" using HTTP POSTT"#)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unknown_step_returns_none() {
        assert!(reg().find("I refund the order").unwrap().is_none());
    }
}
