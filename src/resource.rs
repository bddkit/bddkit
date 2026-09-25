//! `bddkit resource fields`: what a `resources.<kind>.<name>` body takes.
//!
//! The host's three kinds come from the hand-written tables in `config`; a
//! plugin group's come from the plugin's own manifest, which the host prints
//! and interprets in no other way. Both halves are one listing on purpose —
//! the reader's question is "what does this entry take", and which side of the
//! FFI boundary answers it is not part of the question.

use crate::config::{self, Scalar, host_fields};
use crate::plugin::Plugins;
use serde::Serialize;

/// The long flags the command owns. A field with one of these names would be
/// parsed by clap and never reach the assembled body, so it is refused rather
/// than silently dropped — and for `config` the damage would be worse than a
/// drop: it would retarget the file being edited.
const RESERVED_FLAGS: &[&str] = &["config", "env", "json", "no-check"];

/// What a plugin's field says its value is, defaulting to a string. The
/// `unwrap_or` is the load-time guarantee written down: `check_field_types`
/// has already refused any name `from_declared` does not know.
fn scalar_of(field: &crate::plugin::abi::ConfigField) -> Scalar {
    field
        .value_type
        .as_deref()
        .and_then(Scalar::from_declared)
        .unwrap_or(Scalar::Str)
}

/// One name a flag may set, and the YAML type its value becomes. A plugin's
/// field that declares no `type` is a string, which is what the host did for
/// every plugin field before a manifest could say otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDef {
    pub name: String,
    pub scalar: Scalar,
}

/// What is known about a kind's keys. `Undescribed` is a plugin whose manifest
/// declares no `fields` — not an error, and not the same as "takes no keys":
/// there is simply no list to check a name against, so `validate_config` is
/// what rejects a typo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fields {
    Known(Vec<FieldDef>),
    Undescribed,
}

impl Fields {
    /// `None` for anything the host does not serve itself — a plugin group or
    /// a typo, which only the loaded plugins can tell apart.
    pub fn host(kind: &str) -> Option<Self> {
        Some(Fields::Known(
            host_fields(kind)?
                .iter()
                .map(|(name, _, _, scalar)| FieldDef {
                    name: name.to_string(),
                    scalar: *scalar,
                })
                .collect(),
        ))
    }

    /// A field's declared `type`, or a string when the manifest declares none.
    /// An unknown name cannot reach here: `plugin::library` refuses it at load,
    /// so falling back to a string is the documented default and never the
    /// silent swallowing of a typo.
    pub fn plugin(declared: Option<&[crate::plugin::abi::ConfigField]>) -> Self {
        match declared {
            Some(fields) => Fields::Known(
                fields
                    .iter()
                    .map(|field| FieldDef {
                        name: field.name.clone(),
                        scalar: scalar_of(field),
                    })
                    .collect(),
            ),
            None => Fields::Undescribed,
        }
    }

    fn find(&self, name: &str) -> Option<&FieldDef> {
        match self {
            Fields::Known(fields) => fields.iter().find(|field| field.name == name),
            Fields::Undescribed => None,
        }
    }

    fn reserved_collision(&self) -> Option<&str> {
        match self {
            Fields::Known(fields) => fields
                .iter()
                .find(|field| RESERVED_FLAGS.contains(&field.name.as_str()))
                .map(|field| field.name.as_str()),
            Fields::Undescribed => None,
        }
    }
}

/// `--key value` and `--key=value`, in the order typed. Everything else is an
/// error naming what was read: a silently ignored argument is how a resource
/// gets written without the key its author thought they set.
pub fn parse_flags(rest: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut rest = rest.iter();
    while let Some(token) = rest.next() {
        let Some(flag) = token.strip_prefix("--") else {
            return Err(format!("expected a --field, got {token:?}"));
        };
        let (key, value) = match flag.split_once('=') {
            Some((key, value)) => (key.to_string(), value.to_string()),
            None => {
                let value = rest
                    .next()
                    .ok_or_else(|| format!("--{flag} has no value"))?;
                (flag.to_string(), value.clone())
            }
        };
        if out.iter().any(|(seen, _)| *seen == key) {
            return Err(format!("--{key} was given twice"));
        }
        out.push((key, value));
    }
    Ok(out)
}

/// The resource body: `--json` first, then the flags over it, key by key.
pub fn assemble_body(
    json: Option<&str>,
    flags: &[(String, String)],
    fields: &Fields,
) -> Result<serde_yaml_ng::Value, String> {
    if let Some(name) = fields.reserved_collision() {
        return Err(format!(
            "the field {name:?} has the same name as this command's own --{name}: set it through --json"
        ));
    }

    let mut body = serde_yaml_ng::Mapping::new();
    if let Some(text) = json {
        let parsed: serde_json::Value =
            serde_json::from_str(text).map_err(|error| format!("--json is not JSON: {error}"))?;
        let value: serde_yaml_ng::Value = serde_yaml_ng::to_value(parsed)
            .map_err(|error| format!("--json cannot be represented in YAML: {error}"))?;
        match value {
            serde_yaml_ng::Value::Mapping(mapping) => body = mapping,
            _ => return Err("--json must be a JSON object".to_string()),
        }
    }

    for (key, value) in flags {
        let converted = match fields.find(key) {
            Some(FieldDef {
                scalar: Scalar::Num,
                ..
            }) => serde_yaml_ng::Value::Number(
                // An integer first, so a whole number reaches YAML as one
                // rather than as `30.0`. The fraction is not decoration: a
                // plugin's key may be a duration or a ratio, and refusing one
                // would be the host inventing a rule about a config it does
                // not own.
                value
                    .parse::<i64>()
                    .map(serde_yaml_ng::Number::from)
                    .or_else(|_| value.parse::<f64>().map(serde_yaml_ng::Number::from))
                    .map_err(|_| format!("--{key} takes a number, got {value:?}"))?,
            ),
            Some(FieldDef {
                scalar: Scalar::Bool,
                ..
            }) => match value.as_str() {
                "true" => serde_yaml_ng::Value::Bool(true),
                "false" => serde_yaml_ng::Value::Bool(false),
                // Only the two YAML 1.2 spellings. `yes`/`on`/`1` are the
                // Norway problem in miniature, and guessing which of them a
                // plugin means is the inference this whole key replaces.
                other => {
                    return Err(format!("--{key} takes true or false, got {other:?}"));
                }
            },
            Some(FieldDef {
                scalar: Scalar::NonScalar,
                ..
            }) => {
                return Err(format!(
                    "--{key} is a map or a list, which no flag can express: set it through --json"
                ));
            }
            Some(_) => serde_yaml_ng::Value::String(value.clone()),
            None if RESERVED_FLAGS.contains(&key.as_str()) => {
                return Err(format!(
                    "--{key} is this command's own flag, not a resource field: put it before the --<field> values"
                ));
            }
            // A group whose plugin describes no fields has no list to check a
            // name against, so nothing here can reject one — that rejection
            // belongs to the plugin's own `validate_config`.
            None => match fields {
                Fields::Undescribed => serde_yaml_ng::Value::String(value.clone()),
                Fields::Known(known) => {
                    let names: Vec<&str> = known.iter().map(|f| f.name.as_str()).collect();
                    return Err(format!(
                        "no such field --{key} for this resource; it takes: {}",
                        names.join(", ")
                    ));
                }
            },
        };
        body.insert(serde_yaml_ng::Value::String(key.clone()), converted);
    }

    Ok(serde_yaml_ng::Value::Mapping(body))
}

/// One resource kind: a host kind or a plugin group.
#[derive(Debug, Serialize)]
pub struct Kind {
    pub kind: String,
    /// Who described it — the host itself, or the plugin serving the group.
    pub source: &'static str,
    /// `None` for a plugin whose manifest declares no `fields`. Not an error:
    /// the key is optional, so a plugin published before it existed still
    /// loads, and the host has nothing of its own to say about the group.
    pub fields: Option<Vec<Field>>,
}

#[derive(Debug, Serialize)]
pub struct Field {
    pub name: String,
    pub required: bool,
    /// The YAML type a `resource add` flag's value becomes for this key —
    /// `string`, `boolean`, `number`, or `nonscalar` for a key only `--json`
    /// can set. Never `None`: a plugin field that declares no type is a
    /// string, and saying so is the answer the reader came for.
    #[serde(rename = "type")]
    pub value_type: String,
    pub description: Option<String>,
    pub example: Option<String>,
}

/// The three kinds the host serves itself, always available: they need no
/// config, and describing them cannot fail.
pub fn host_kinds() -> Vec<Kind> {
    [
        ("api", crate::config::API_FIELDS),
        ("db", crate::config::DB_FIELDS),
        ("srp", crate::config::SRP_FIELDS),
    ]
    .into_iter()
    .map(|(kind, table)| Kind {
        kind: kind.to_string(),
        source: "host",
        fields: Some(
            table
                .iter()
                .map(|(name, required, description, scalar)| Field {
                    name: name.to_string(),
                    required: *required,
                    value_type: scalar.declared_name().to_string(),
                    description: Some(description.to_string()),
                    example: None,
                })
                .collect(),
        ),
    })
    .collect()
}

/// Every group the loaded plugins serve, described or not.
pub fn plugin_kinds(plugins: &Plugins) -> Vec<Kind> {
    plugins
        .group_names()
        .into_iter()
        .map(|group| Kind {
            fields: plugins.fields_for(&group).map(|fields| {
                fields
                    .iter()
                    .map(|field| Field {
                        name: field.name.clone(),
                        required: field.required,
                        value_type: scalar_of(field).declared_name().to_string(),
                        description: field.description.clone(),
                        example: field.example.clone(),
                    })
                    .collect()
            }),
            kind: group,
            source: "plugin",
        })
        .collect()
}

/// One kind per block, one field per line, `required` in its own column. A
/// plugin that describes nothing says so in place of its field list, because
/// an empty block reads as "this group takes no keys" — which is a different
/// and usually wrong answer.
pub fn render(kinds: &[Kind]) -> String {
    let mut out = String::new();
    for kind in kinds {
        out.push_str(&kind.kind);
        out.push_str(":\n");
        let Some(fields) = &kind.fields else {
            out.push_str("  the plugin serving this group does not describe its fields\n");
            continue;
        };
        for field in fields {
            let mark = if field.required { "required" } else { "" };
            let mut detail = field.description.clone().unwrap_or_default();
            if let Some(example) = &field.example {
                detail.push_str(&format!(" (e.g. {example})"));
            }
            out.push_str(&format!(
                "  {:<18}{:<10}{mark:<10}{}\n",
                field.name,
                field.value_type,
                detail.trim_start()
            ));
        }
    }
    out
}

/// The prospective file text, or the reason nothing can be written.
///
/// Insert only, and no existing line is ever rewritten: the block goes in after
/// an anchor line — the `<group>:` line when the group exists, the `resources:`
/// line when it does not. Comments, anchors, key order and formatting survive
/// because nothing touched them, which is a property of the edit rather than
/// something the code has to be careful about.
///
/// What is not structural is landing at the wrong index, and that is what the
/// re-parse at the end is for: the spliced text is parsed again and compared,
/// as a `Value`, against the original plus the new resource. `Mapping` is an
/// `IndexMap` underneath and its `PartialEq` ignores order, so the comparison
/// answers "does this file now mean what it should" and not "did the keys land
/// in the order I built them in".
pub fn splice(
    raw: &str,
    group: &str,
    name: &str,
    body: &serde_yaml_ng::Value,
) -> Result<String, String> {
    // A multi-document file fails here — `from_str` refuses more than one
    // document — which is one of the cases that must write nothing.
    let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(raw)
        .map_err(|error| format!("the config does not parse: {error}"))?;
    let resources = doc.get("resources").ok_or_else(|| {
        format!(
            "the config has no `resources:` key: add one as a block — a `resources:` line with `{group}:` under it — and the resource can be added there"
        )
    })?;
    if !resources.is_mapping() {
        return Err("`resources:` is not a mapping".to_string());
    }
    let group_exists = resources.get(group).is_some();
    if group_exists && resources[group].get(name).is_some() {
        return Err(format!(
            "resources.{group}.{name} already exists: `resource add` never rewrites a resource"
        ));
    }

    // The anchor: the group's own line if the group is there, else the
    // `resources:` line. `find_anchor` returns the byte range of the line and
    // its indent.
    let anchor = if group_exists {
        find_anchor(raw, Some(group))?
    } else {
        find_anchor(raw, None)?
    };
    let step = indent_step(raw, &anchor);
    let eol = if anchor.line.ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let block = if group_exists {
        render_block(None, name, body, anchor.indent + step, step, eol)
    } else {
        render_block(Some(group), name, body, anchor.indent + step, step, eol)
    };

    let mut out = String::with_capacity(raw.len() + block.len());
    out.push_str(&raw[..anchor.end]);
    // A file whose last line is the anchor and which ends without a newline
    // needs one before the block, or the block would continue that line.
    if !out.ends_with('\n') {
        out.push_str(eol);
    }
    out.push_str(&block);
    out.push_str(&raw[anchor.end..]);

    let reparsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&out)
        .map_err(|error| format!("the edit would not parse: {error}"))?;
    let mut expected = doc.clone();
    insert_resource(&mut expected, group, name, body)?;
    if reparsed != expected {
        return Err("the edit did not land where it meant to".to_string());
    }
    Ok(out)
}

/// One line of the file, by byte offset, plus its indent. `end` is the offset
/// just past the line's terminator, which is where the block goes.
struct Anchor {
    /// The line including its terminator, so the splice can reuse it.
    line: String,
    end: usize,
    indent: usize,
}

/// The `resources:` line, or a group's line inside it. Refuses a line whose
/// value is on the same line (`api: {}`): there is no block to splice into,
/// and rewriting that line is exactly what this command does not do.
fn find_anchor(raw: &str, group: Option<&str>) -> Result<Anchor, String> {
    let wanted = group.unwrap_or("resources");
    let mut offset = 0usize;
    let mut inside_resources = group.is_none();
    // The indent of `resources:`'s own direct children, learned from the first
    // one. A group key is only ever a direct child, so a deeper key of the same
    // name — `resources.s3.main.api` — must not out-anchor the real group.
    let mut child_indent: Option<usize> = None;
    for line in raw.split_inclusive('\n') {
        let end = offset + line.len();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let indent = trimmed.len() - trimmed.trim_start().len();
        let content = trimmed.trim_start();
        // A comment or a blank line carries no structure: it neither ends the
        // `resources:` block nor sets the indent its children are at.
        let structural = !content.is_empty() && !content.starts_with('#');
        if group.is_some() {
            if indent == 0 && content.starts_with("resources:") {
                inside_resources = true;
            } else if indent == 0 && structural {
                // Left the `resources:` block without finding the group.
                inside_resources = false;
            } else if inside_resources && structural {
                child_indent.get_or_insert(indent);
            }
        }
        // A match only counts at the level the key can legally be at: indent 0
        // for the top-level `resources:`, and the block's own child indent for
        // a group — otherwise a nested key that happens to share the name would
        // anchor first, and the splice would land inside another resource.
        let at_its_level = match group {
            None => indent == 0,
            Some(_) => child_indent == Some(indent),
        };
        let matches = at_its_level
            && content
                .strip_prefix(wanted)
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some();
        if matches && inside_resources {
            let rest = strip_comment(&trimmed[indent + wanted.len() + 1..]).trim();
            // `{}` is the natural "nothing here yet" scaffold, so it gets the
            // instruction rather than the diagnosis. Either way the line stays
            // as it is: never rewriting a line is what the whole edit rests on.
            if rest == "{}" {
                return Err(format!(
                    "`{wanted}:` holds an empty flow mapping, and rewriting that line is the one thing this command never does: delete the `{{}}` so `{wanted}:` opens a block, and the resource can be added under it"
                ));
            }
            if !rest.is_empty() {
                return Err(format!(
                    "`{wanted}:` has its value on the same line, so there is no block to splice into"
                ));
            }
            return Ok(Anchor {
                line: line.to_string(),
                end,
                indent,
            });
        }
        offset = end;
    }
    Err(format!("`{wanted}:` is not a line of this config"))
}

/// The line without its trailing `#` comment. YAML starts one at a `#` that
/// opens the line or follows a space, which is exactly enough to tell
/// `api:  # the HTTP APIs` (a block anchor) from `api: {}` (a value).
fn strip_comment(rest: &str) -> &str {
    let mut previous = ' ';
    for (at, ch) in rest.char_indices() {
        if ch == '#' && (previous == ' ' || previous == '\t') {
            return &rest[..at];
        }
        previous = ch;
    }
    rest
}

/// The file's own indent step, read from the first line after the anchor that
/// is indented deeper than it. Two when the file says nothing — a `resources:`
/// with no children, or a group whose block is empty.
///
/// The scan starts at `anchor.end`, the offset just past the anchor line, so
/// nothing here depends on identifying a line by anything but its position.
fn indent_step(raw: &str, anchor: &Anchor) -> usize {
    raw[anchor.end..]
        .split_inclusive('\n')
        .filter_map(|line| {
            let trimmed = line.trim_end_matches(['\r', '\n']);
            let content = trimmed.trim_start();
            if content.is_empty() || content.starts_with('#') {
                return None;
            }
            Some(trimmed.len() - content.len())
        })
        .next()
        .filter(|indent| *indent > anchor.indent)
        .map(|indent| indent - anchor.indent)
        .unwrap_or(2)
}

/// The block, indented and ready to insert. `group` is `Some` only when the
/// group is not in the file yet, in which case its key is emitted above the
/// resource's. `step` is the file's own indent step, so a config written with
/// four spaces gets four.
pub fn render_block(
    group: Option<&str>,
    name: &str,
    body: &serde_yaml_ng::Value,
    indent: usize,
    step: usize,
    eol: &str,
) -> String {
    let mut out = String::new();
    let mut at = indent;
    if let Some(group) = group {
        out.push_str(&format!("{:indent$}{group}:{eol}", "", indent = at));
        at += step;
    }
    out.push_str(&format!("{:indent$}{name}:{eol}", "", indent = at));
    // `to_string` on a mapping cannot fail; an empty body renders as `{}`,
    // which is a resource with no keys and a thing the kinds' own checks
    // reject far more clearly than this function could.
    let rendered = serde_yaml_ng::to_string(body).unwrap_or_default();
    for line in rendered.lines() {
        out.push_str(&format!("{:indent$}{line}{eol}", "", indent = at + step));
    }
    out
}

/// The same insertion, into a parsed `Value`: this is the expectation the
/// spliced text is compared against.
fn insert_resource(
    doc: &mut serde_yaml_ng::Value,
    group: &str,
    name: &str,
    body: &serde_yaml_ng::Value,
) -> Result<(), String> {
    let resources = doc
        .get_mut("resources")
        .and_then(serde_yaml_ng::Value::as_mapping_mut)
        .ok_or_else(|| "`resources:` is not a mapping".to_string())?;
    let key = serde_yaml_ng::Value::String(group.to_string());
    let entry = resources
        .entry(key)
        .or_insert_with(|| serde_yaml_ng::Value::Mapping(serde_yaml_ng::Mapping::new()));
    // An existing group with no instances parses as YAML null (`api:` with
    // nothing under it) rather than an empty mapping — a legal, plausible
    // shape (scaffolded, or emptied of its last instance). Treat it as the
    // empty mapping it is about to become; only a group holding something
    // that truly cannot take instances (a string, a sequence) is an error.
    if entry.is_null() {
        *entry = serde_yaml_ng::Value::Mapping(serde_yaml_ng::Mapping::new());
    }
    let instances = entry
        .as_mapping_mut()
        .ok_or_else(|| format!("`resources.{group}` is not a mapping"))?;
    instances.insert(serde_yaml_ng::Value::String(name.to_string()), body.clone());
    Ok(())
}

/// Write beside the config and rename over it: a crash mid-write must not
/// leave a truncated config behind. The temporary file is in the same
/// directory, which is what makes the rename atomic.
///
/// Two things the rename would otherwise take away from the file it replaces:
///
/// - Its permissions. `fs::write` creates at the umask, so a config held at
///   0600 would come back 0644 — and a `resources.db.dsn` routinely carries a
///   password, which makes that a quiet secret leak rather than a cosmetic
///   change. The mode is copied from the file being replaced.
/// - Its identity, when it is a symlink. Renaming over the link replaces the
///   link itself with a regular file and leaves the real config untouched,
///   while the command reports the resource as added. `canonicalize` resolves
///   it first, so the write lands on the target.
pub fn write_atomically(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let path = std::fs::canonicalize(path)?;
    let tmp = path.with_extension("bddkit-tmp");
    std::fs::write(&tmp, text)?;
    let permissions = std::fs::metadata(&path)?.permissions();
    std::fs::set_permissions(&tmp, permissions)?;
    std::fs::rename(&tmp, &path)
}

/// Everything `resource add` was given, with clap's types left in `main`.
pub struct AddInput<'a> {
    pub group: &'a str,
    pub name: &'a str,
    pub config: &'a std::path::Path,
    pub env: Option<&'a str>,
    pub json: Option<&'a str>,
    pub no_check: bool,
    pub flags: &'a [String],
}

/// 0 when the resource was written, 1 when nothing was. Never 2, for the same
/// reason `doctor` never exits 2: every outcome here is a report, and a
/// caller's rule stays "0, or fix something". So no failure below reaches
/// `main`'s error path — each one prints and returns 1, with the assembled
/// block whenever there is one, because composing it is the work the user does
/// not want to lose.
///
/// The one thing that still exits 2 is clap's own usage error, which happens
/// before this function is called at all.
pub async fn add(input: AddInput<'_>) -> anyhow::Result<i32> {
    /// Prints the reason, and the block when one was assembled, and answers 1.
    fn refuse(reason: &str, block: Option<&str>) -> anyhow::Result<i32> {
        println!("{reason}\n\nnothing was written.");
        if let Some(block) = block {
            println!("The block, for manual insertion:\n\n{block}");
        }
        Ok(1)
    }

    let config_dir = input
        .config
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let raw = match std::fs::read_to_string(input.config) {
        Ok(raw) => raw,
        Err(error) => {
            return refuse(
                &format!("failed to read config {}: {error}", input.config.display()),
                None,
            );
        }
    };

    // A host kind needs nothing loaded. A plugin group's field list lives
    // inside the plugin, so reaching it means loading the current config —
    // which must itself be sound before anything can be added to it.
    let fields = match Fields::host(input.group) {
        Some(fields) => fields,
        None => {
            let plugins = match config::load_str(&raw, config_dir, input.env) {
                Ok(cfg) => {
                    let generator = crate::unique::Generator::new();
                    match crate::load_plugins(
                        input.config,
                        &cfg,
                        &generator,
                        &crate::dirs::Env::from_process(None),
                    ) {
                        Ok(plugins) => plugins,
                        Err(error) => return refuse(&format!("{error:#}"), None),
                    }
                }
                Err(error) => return refuse(&format!("{error:#}"), None),
            };
            let serves = plugins
                .as_ref()
                .is_some_and(|plugins| plugins.group_names().iter().any(|g| g == input.group));
            if !serves {
                return refuse(
                    &format!(
                        "no such resource kind {:?}: bddkit serves api, db and srp, and no installed plugin serves that group",
                        input.group
                    ),
                    None,
                );
            }
            Fields::plugin(plugins.as_ref().and_then(|p| p.fields_for(input.group)))
        }
    };

    let flags = match parse_flags(input.flags) {
        Ok(flags) => flags,
        Err(error) => return refuse(&error, None),
    };
    let body = match assemble_body(input.json, &flags, &fields) {
        Ok(body) => body,
        Err(error) => return refuse(&error, None),
    };

    // From here the block exists, so every failure prints it. It carries the
    // group key unless the config already has the group — a block pasted under
    // `resources:` as `main:` alone would land as `resources.main`, and the
    // block is the whole value of the failure path.
    let has_group = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&raw)
        .ok()
        .and_then(|doc| {
            doc.get("resources")
                .and_then(|r| r.get(input.group))
                .cloned()
        })
        .is_some();
    let group_key = (!has_group).then_some(input.group);
    let block = render_block(group_key, input.name, &body, 0, 2, "\n");
    let prospective = match splice(&raw, input.group, input.name, &body) {
        Ok(text) => text,
        Err(error) => return refuse(&error, Some(&block)),
    };
    let cfg = match config::load_str(&prospective, config_dir, input.env) {
        Ok(cfg) => cfg,
        Err(error) => return refuse(&format!("{error:#}"), Some(&block)),
    };
    if let Err(error) = check_new_resource(&cfg, &input, input.config).await {
        return refuse(&error, Some(&block));
    }

    if let Err(error) = write_atomically(input.config, &prospective) {
        return refuse(
            &format!("failed to write {}: {error}", input.config.display()),
            Some(&block),
        );
    }
    println!(
        "added resources.{}.{} to {}",
        input.group,
        input.name,
        input.config.display()
    );
    Ok(0)
}

/// The just-added resource, read back out of the loaded config.
///
/// A miss is not "cannot happen", which is why this is a lookup and not an
/// index: the name is written to the file literally, while the loaded `Config`
/// holds the `${VAR}`-expanded one, so a name carrying a variable that is
/// actually set arrives here under a different key entirely. The resource is in
/// the prospective text and would work at run time; what cannot be done is
/// checking it under the name being written, and this command's contract is
/// exit 0 or 1 — never a panic.
fn under_the_name<'a, T>(
    map: &'a std::collections::BTreeMap<String, T>,
    input: &AddInput<'_>,
) -> Result<&'a T, String> {
    map.get(input.name).ok_or_else(|| {
        format!(
            "the name {:?} expands through ${{VAR}} to something else, so resources.{}.{} is not what the loaded config calls this resource and it cannot be checked under the name being written: name it literally",
            input.name, input.group, input.name
        )
    })
}

/// The new resource's own static check, and then its probe unless `--no-check`.
/// Deliberately not `doctor`'s whole stage list: a broken `.feature` elsewhere
/// in the suite is not a reason to refuse to add a resource, and `doctor` is
/// the command that answers that question.
async fn check_new_resource(
    cfg: &config::Config,
    input: &AddInput<'_>,
    config_path: &std::path::Path,
) -> Result<(), String> {
    match input.group {
        "api" => {
            let api = under_the_name(&cfg.resources.api, input)?;
            let (resource, _) = crate::doctor::check_api(api)?;
            if input.no_check {
                return Ok(());
            }
            println!(
                "{}",
                crate::doctor::probe_api(&resource, &api.base_url).await?
            );
        }
        "db" => {
            let connection = under_the_name(&cfg.resources.db, input)?;
            crate::db::check_dsn(connection)?;
            if input.no_check {
                return Ok(());
            }
            crate::doctor::probe_db(input.name, connection).await?;
        }
        "srp" => {
            crate::doctor::check_srp_resource(under_the_name(&cfg.resources.srp, input)?)?;
        }
        group => {
            // `Plugins::load` refuses a group nothing serves and runs
            // `validate_config` for every declared instance, so loading the
            // prospective config is the static check.
            let generator = crate::unique::Generator::new();
            let plugins = crate::load_plugins(
                config_path,
                cfg,
                &generator,
                &crate::dirs::Env::from_process(None),
            )
            .map_err(|error| format!("{error:#}"))?;
            let Some(plugins) = plugins else {
                return Err(format!("no installed plugin serves the group {group:?}"));
            };
            if input.no_check {
                return Ok(());
            }
            match plugins.probe_config(group, input.name) {
                Some(Ok(())) => println!("probed clean"),
                Some(Err(error)) => return Err(error),
                None => {
                    println!("the plugin exports no bddkit_probe_config, so nothing was probed")
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_host_kind_is_described_and_marks_its_required_keys() {
        let kinds = host_kinds();
        let out = render(&kinds);
        for kind in ["api:", "db:", "srp:"] {
            assert!(out.contains(kind), "{out}");
        }
        assert!(out.contains("base_url"), "{out}");
        assert!(out.contains("required"), "{out}");
        // Every host kind's config is fully described, or the table is stale.
        assert!(kinds.iter().all(|kind| kind.fields.is_some()));
    }

    /// `resource fields` is where someone learns what a key takes, and the
    /// type is what decides between `--timeout_secs 5` and `--json`. A field
    /// whose manifest declares none is shown as a string, because that is what
    /// the host will actually do with it — not left blank, which would read as
    /// "unknown" for a question that always has an answer.
    #[test]
    fn each_field_shows_the_type_its_flag_value_becomes() {
        let out = render(&host_kinds());
        let line = |key: &str| -> String {
            out.lines()
                .find(|line| line.trim_start().starts_with(key))
                .unwrap_or_else(|| panic!("{key} is listed:\n{out}"))
                .to_string()
        };
        assert!(
            line("timeout_secs").contains("number"),
            "{}",
            line("timeout_secs")
        );
        assert!(line("base_url").contains("string"), "{}", line("base_url"));
        assert!(
            line("default_headers").contains("nonscalar"),
            "{}",
            line("default_headers")
        );

        let out = render(&[Kind {
            kind: "s3".into(),
            source: "plugin",
            fields: Some(vec![
                Field {
                    name: "path_style".into(),
                    required: false,
                    value_type: "boolean".into(),
                    description: None,
                    example: None,
                },
                Field {
                    name: "bucket".into(),
                    required: true,
                    value_type: "string".into(),
                    description: None,
                    example: None,
                },
            ]),
        }]);
        assert!(out.contains("boolean"), "{out}");
        assert!(out.contains("string"), "{out}");
    }

    #[test]
    fn an_undescribed_group_says_so_instead_of_showing_an_empty_block() {
        let out = render(&[Kind {
            kind: "widget".into(),
            source: "plugin",
            fields: None,
        }]);
        assert!(out.contains("widget:"), "{out}");
        assert!(out.contains("does not describe"), "{out}");
    }

    #[test]
    fn an_example_is_shown_beside_the_description() {
        let out = render(&[Kind {
            kind: "s3".into(),
            source: "plugin",
            fields: Some(vec![Field {
                name: "bucket".into(),
                required: true,
                value_type: "string".into(),
                description: Some("bucket the steps read and write".into()),
                example: Some("acceptance".into()),
            }]),
        }]);
        assert!(out.contains("(e.g. acceptance)"), "{out}");
    }

    fn api_fields() -> Fields {
        Fields::host("api").expect("api is a host kind")
    }

    #[test]
    fn both_flag_spellings_are_read() {
        let flags = parse_flags(&[
            "--base_url".into(),
            "http://a.local".into(),
            "--timeout_secs=5".into(),
        ])
        .expect("both spellings parse");
        assert_eq!(
            flags,
            vec![
                ("base_url".to_string(), "http://a.local".to_string()),
                ("timeout_secs".to_string(), "5".to_string()),
            ]
        );
    }

    #[test]
    fn a_bare_word_a_valueless_flag_and_a_repeat_are_each_named() {
        for (argv, expected) in [
            (vec!["photos".to_string()], "photos"),
            (vec!["--base_url".to_string()], "base_url"),
            (
                vec![
                    "--base_url".to_string(),
                    "a".to_string(),
                    "--base_url".to_string(),
                    "b".to_string(),
                ],
                "base_url",
            ),
        ] {
            let error = parse_flags(&argv).expect_err("malformed");
            assert!(error.contains(expected), "{error}");
        }
    }

    /// The rule the whole feature rests on: a flag value is a string, and the
    /// kind's own table is what turns one into a number. Nothing is inferred
    /// from the shape of the value, so a bucket named `123` stays a string.
    #[test]
    fn a_flag_value_is_converted_by_the_table_not_by_its_shape() {
        let body = assemble_body(
            None,
            &[
                ("base_url".into(), "http://a.local".into()),
                ("timeout_secs".into(), "5".into()),
            ],
            &api_fields(),
        )
        .expect("both fields are known");
        assert_eq!(
            body["base_url"],
            serde_yaml_ng::Value::from("http://a.local")
        );
        assert_eq!(body["timeout_secs"], serde_yaml_ng::Value::from(5));

        let error = assemble_body(
            None,
            &[("timeout_secs".into(), "soon".into())],
            &api_fields(),
        )
        .expect_err("a number field takes a number");
        assert!(error.contains("timeout_secs"), "{error}");
    }

    #[test]
    fn an_unknown_field_is_rejected_and_a_non_scalar_one_names_json() {
        let error = assemble_body(None, &[("bukcet".into(), "photos".into())], &api_fields())
            .expect_err("api has no bukcet");
        assert!(error.contains("bukcet"), "{error}");
        assert!(
            error.contains("base_url"),
            "the known names are listed: {error}"
        );

        let error = assemble_body(
            None,
            &[("default_headers".into(), "Accept: text/plain".into())],
            &api_fields(),
        )
        .expect_err("a map cannot come from a flag");
        assert!(error.contains("--json"), "{error}");
    }

    /// clap's trailing-argument capture swallows every token after the first
    /// unrecognized one, so `--env` typed after a `--<field>` never reaches
    /// clap and lands here as an ordinary flag instead. The generic "no such
    /// field" message would blame a typo that is not there; the fix is to
    /// name the actual mistake — flag order — for the three flags that are
    /// not already rescued explicitly in `main.rs` (`--no-check` is).
    #[test]
    fn a_trailing_command_flag_names_itself_rather_than_a_typo() {
        for flag in ["config", "env", "json"] {
            let error = assemble_body(None, &[(flag.to_string(), "x".into())], &api_fields())
                .expect_err("a reserved flag is never a resource field");
            assert!(error.contains(&format!("--{flag}")), "{error}");
            assert!(
                error.contains("this command's own flag"),
                "must not read as a typo in the field name: {error}"
            );
        }
    }

    /// `--json` is applied first and flags override it, key by key.
    #[test]
    fn a_flag_overrides_the_same_key_from_json() {
        let body = assemble_body(
            Some(r#"{"base_url": "http://old.local", "default_headers": {"Accept": "application/json"}}"#),
            &[("base_url".into(), "http://new.local".into())],
            &api_fields(),
        )
        .expect("json plus one override");
        assert_eq!(
            body["base_url"],
            serde_yaml_ng::Value::from("http://new.local")
        );
        assert_eq!(
            body["default_headers"]["Accept"],
            serde_yaml_ng::Value::from("application/json")
        );
    }

    /// One field as a manifest declares it. `declared` is what the manifest's
    /// `type` said, and `None` is a manifest written before the key existed.
    fn plugin_field(name: &str, declared: Option<&str>) -> crate::plugin::abi::ConfigField {
        crate::plugin::abi::ConfigField {
            name: name.into(),
            required: false,
            description: None,
            example: None,
            value_type: declared.map(str::to_string),
        }
    }

    /// The failure this whole key exists for: a plugin whose value is a
    /// boolean got the string `'false'`, which its own `validate_config` then
    /// refused — correctly — on every input, forever.
    #[test]
    fn a_declared_boolean_becomes_a_yaml_boolean_not_the_word() {
        let declared = [plugin_field("path_style", Some("boolean"))];
        let fields = Fields::plugin(Some(&declared));
        let body = assemble_body(None, &[("path_style".into(), "false".into())], &fields)
            .expect("the manifest says this key is a boolean");
        assert_eq!(body["path_style"], serde_yaml_ng::Value::Bool(false));

        let error = assemble_body(None, &[("path_style".into(), "sometimes".into())], &fields)
            .expect_err("a boolean field takes a boolean");
        assert!(error.contains("path_style"), "{error}");
        assert!(
            error.contains("true"),
            "the two it takes are named: {error}"
        );
    }

    /// A plugin's `timeout_secs` must reach YAML the way the host's own does.
    /// The fraction is not decoration: a plugin key may be a duration or a
    /// ratio, and refusing it would be the host inventing a rule of its own.
    #[test]
    fn a_declared_number_takes_an_integer_or_a_fraction() {
        let declared = [plugin_field("timeout_secs", Some("number"))];
        let fields = Fields::plugin(Some(&declared));
        let body = assemble_body(None, &[("timeout_secs".into(), "30".into())], &fields)
            .expect("an integer");
        assert_eq!(body["timeout_secs"], serde_yaml_ng::Value::from(30));

        let body = assemble_body(None, &[("timeout_secs".into(), "1.5".into())], &fields)
            .expect("a fraction");
        assert_eq!(body["timeout_secs"], serde_yaml_ng::Value::from(1.5));

        let error = assemble_body(None, &[("timeout_secs".into(), "soon".into())], &fields)
            .expect_err("a number field takes a number");
        assert!(error.contains("timeout_secs"), "{error}");
    }

    /// The same courtesy the host's own `default_headers` gets: told it cannot
    /// come from a flag, rather than handed a string the plugin will reject.
    #[test]
    fn a_declared_nonscalar_field_names_json_instead_of_writing_a_string() {
        let declared = [plugin_field("tags", Some("nonscalar"))];
        let error = assemble_body(
            None,
            &[("tags".into(), "a,b".into())],
            &Fields::plugin(Some(&declared)),
        )
        .expect_err("no flag can express a map or a list");
        assert!(error.contains("--json"), "{error}");
    }

    /// A field whose manifest declares no type is a string, which is the whole
    /// backward-compatibility promise — and a plugin that describes nothing at
    /// all still accepts any name, because that rejection belongs to
    /// `validate_config` and not to a list the host does not have.
    #[test]
    fn plugin_fields_are_strings_and_an_undescribed_group_accepts_anything() {
        let declared = [plugin_field("bucket", None)];
        let fields = Fields::plugin(Some(&declared));
        let body = assemble_body(None, &[("bucket".into(), "42".into())], &fields)
            .expect("bucket is declared");
        assert_eq!(body["bucket"], serde_yaml_ng::Value::from("42"));
        assert!(
            assemble_body(None, &[("region".into(), "eu".into())], &fields).is_err(),
            "a name the manifest does not declare is a typo"
        );

        let body = assemble_body(
            None,
            &[("region".into(), "eu".into())],
            &Fields::plugin(None),
        )
        .expect("nothing describes this group, so nothing can reject a name");
        assert_eq!(body["region"], serde_yaml_ng::Value::from("eu"));
    }

    /// clap owns these four names, so a field called `config` would be eaten
    /// before the hand-rolled pass ever saw it — and `--config` would silently
    /// retarget the file being edited. Checked against the field list, so the
    /// failure does not depend on whether the user happened to type it.
    #[test]
    fn a_field_colliding_with_the_commands_own_flags_is_refused() {
        let declared = [plugin_field("config", None)];
        let error = assemble_body(None, &[], &Fields::plugin(Some(&declared)))
            .expect_err("config collides with --config");
        assert!(error.contains("--json"), "{error}");
        assert!(error.contains("config"), "{error}");
    }

    const COMMENTED: &str = "\
# the suite's resources
paths: [features]
resources:
  api:
    # the one the smoke tests hit
    stub:
      base_url: http://stub.local
# end of file
";

    fn body(pairs: &[(&str, &str)]) -> serde_yaml_ng::Value {
        let mut map = serde_yaml_ng::Mapping::new();
        for (key, value) in pairs {
            map.insert((*key).into(), (*value).into());
        }
        serde_yaml_ng::Value::Mapping(map)
    }

    /// The form that forbids every change except the insertion: reordered
    /// keys, a requoted scalar, a collapsed anchor or a swallowed final
    /// newline all fail here, where "the comments are still present" would
    /// pass through all of them.
    #[test]
    fn an_existing_group_gains_the_block_and_nothing_else_moves() {
        let after = splice(
            COMMENTED,
            "api",
            "staging",
            &body(&[("base_url", "http://staging.local")]),
        )
        .expect("api exists, staging does not");
        assert_eq!(
            after,
            COMMENTED.replace(
                "  api:\n",
                "  api:\n    staging:\n      base_url: http://staging.local\n"
            )
        );
    }

    #[test]
    fn a_new_group_lands_under_the_resources_anchor() {
        let after = splice(COMMENTED, "s3", "main", &body(&[("bucket", "photos")]))
            .expect("s3 is a new group");
        assert_eq!(
            after,
            COMMENTED.replace(
                "resources:\n",
                "resources:\n  s3:\n    main:\n      bucket: photos\n"
            )
        );
    }

    /// `str::lines` drops a trailing newline and hides `\r\n`, so the splice
    /// works on byte offsets. These two cases are what that is for.
    #[test]
    fn a_file_without_a_final_newline_keeps_its_shape() {
        let raw =
            "paths: [features]\nresources:\n  api:\n    stub:\n      base_url: http://stub.local";
        let after = splice(raw, "s3", "main", &body(&[("bucket", "photos")])).expect("splices");
        assert_eq!(
            after,
            raw.replace(
                "resources:\n",
                "resources:\n  s3:\n    main:\n      bucket: photos\n"
            )
        );
    }

    #[test]
    fn a_crlf_file_stays_crlf() {
        let raw = COMMENTED.replace('\n', "\r\n");
        let after = splice(&raw, "s3", "main", &body(&[("bucket", "photos")])).expect("splices");
        assert_eq!(
            after,
            raw.replace(
                "resources:\r\n",
                "resources:\r\n  s3:\r\n    main:\r\n      bucket: photos\r\n"
            )
        );
        assert!(
            !after.contains("\n\n"),
            "no bare LF was introduced: {after:?}"
        );
    }

    /// Insert only. Replacing a block means rewriting lines, and rewriting
    /// lines is where the comments inside it are lost.
    #[test]
    fn an_existing_resource_is_refused() {
        let error = splice(
            COMMENTED,
            "api",
            "stub",
            &body(&[("base_url", "http://other.local")]),
        )
        .expect_err("stub is already there");
        assert!(error.contains("api"), "{error}");
        assert!(error.contains("stub"), "{error}");
    }

    /// `{}` is what a scaffolded config says, so the refusal has to be an
    /// instruction. What it must never become is a rewrite of that line.
    #[test]
    fn an_empty_flow_mapping_is_refused_with_the_way_out() {
        for (raw, wanted) in [
            ("paths: [features]\nresources:\n  api: {}\n", "api"),
            ("paths: [features]\nresources: {}\n", "resources"),
        ] {
            let error = splice(raw, "api", "stub", &body(&[("base_url", "http://a.local")]))
                .expect_err("there is no block to splice into");
            assert!(error.contains(wanted), "{error}");
            assert!(error.contains("{}"), "the shape is named: {error}");
            assert!(
                error.contains("block"),
                "and so is what turns it into one: {error}"
            );
        }
    }

    /// A config with no `resources:` at all is a config this command cannot
    /// edit — but the reader still has to be told what to add.
    #[test]
    fn a_config_without_resources_says_what_to_add() {
        let error = splice(
            "paths: [features]\n",
            "api",
            "stub",
            &body(&[("base_url", "http://a.local")]),
        )
        .expect_err("nothing to splice into");
        assert!(error.contains("resources:"), "{error}");
        assert!(error.contains("api:"), "the group is named: {error}");
    }

    /// A comment after the key is not a value: `api:  # the HTTP APIs` opens a
    /// block like any other, and refusing it refuses the commented configs this
    /// command exists to serve.
    #[test]
    fn a_trailing_comment_on_the_anchor_is_not_a_value() {
        let raw = "paths: [features]\nresources:  # everything the suite talks to\n  api:  # the HTTP APIs\n    stub:\n      base_url: http://stub.local\n";
        let after = splice(
            raw,
            "api",
            "staging",
            &body(&[("base_url", "http://staging.local")]),
        )
        .expect("the comment is not a value");
        assert_eq!(
            after,
            raw.replace(
                "  api:  # the HTTP APIs\n",
                "  api:  # the HTTP APIs\n    staging:\n      base_url: http://staging.local\n"
            )
        );

        let after = splice(raw, "s3", "main", &body(&[("bucket", "photos")]))
            .expect("the same for the resources: line");
        assert_eq!(
            after,
            raw.replace(
                "resources:  # everything the suite talks to\n",
                "resources:  # everything the suite talks to\n  s3:\n    main:\n      bucket: photos\n"
            )
        );
    }

    /// A comment at column 0 is not the end of the `resources:` block — it
    /// belongs to whatever follows it, which here is the group being anchored.
    #[test]
    fn a_column_zero_comment_does_not_end_the_resources_block() {
        let raw = "paths: [features]\nresources:\n# the HTTP APIs\n  api:\n    stub:\n      base_url: http://stub.local\n";
        let after = splice(
            raw,
            "api",
            "staging",
            &body(&[("base_url", "http://staging.local")]),
        )
        .expect("the group is right there");
        assert_eq!(
            after,
            raw.replace(
                "  api:\n",
                "  api:\n    staging:\n      base_url: http://staging.local\n"
            )
        );
    }

    /// A key named like the group, nested deeper inside another resource, must
    /// not out-anchor the group itself — the block would land inside that
    /// resource's body.
    #[test]
    fn a_nested_key_named_like_the_group_does_not_shadow_it() {
        let raw = "paths: [features]\nresources:\n  s3:\n    main:\n      api:\n        version: 2\n  api:\n    stub:\n      base_url: http://stub.local\n";
        let after = splice(
            raw,
            "api",
            "staging",
            &body(&[("base_url", "http://staging.local")]),
        )
        .expect("the group's own line is the anchor");
        assert_eq!(
            after,
            raw.replace(
                "\n  api:\n",
                "\n  api:\n    staging:\n      base_url: http://staging.local\n"
            )
        );
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("bddkit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    /// A `resources.db.dsn` carries a password. Creating the temporary file at
    /// the umask and renaming it over a 0600 config publishes that password to
    /// everyone on the machine.
    #[cfg(unix)]
    #[test]
    fn the_written_config_keeps_its_own_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch("write-mode").join("c.yaml");
        std::fs::write(&path, "resources:\n").expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        write_atomically(&path, "resources:\n  api:\n").expect("write atomically");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the rename must not widen the config");
    }

    /// Renaming over a symlink replaces the link with a regular file and leaves
    /// the config it pointed at exactly as it was — while the command says the
    /// resource was added.
    #[cfg(unix)]
    #[test]
    fn writing_a_symlinked_config_updates_its_target() {
        let dir = scratch("write-symlink");
        let target = dir.join("real.yaml");
        let link = dir.join("cfg.yaml");
        std::fs::write(&target, "resources:\n").expect("write");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        write_atomically(&link, "resources:\n  api:\n").expect("write atomically");
        assert_eq!(
            std::fs::read_to_string(&target).expect("read"),
            "resources:\n  api:\n"
        );
        assert!(
            std::fs::symlink_metadata(&link)
                .expect("stat")
                .file_type()
                .is_symlink(),
            "the link itself is still a link"
        );
    }

    /// `resources:\n  api:\n` parses `api` as YAML null, not an empty mapping
    /// — a scaffolded or just-emptied group, and a legal shape. It must not
    /// be confused with a group holding something that cannot take instances.
    #[test]
    fn an_existing_but_empty_group_accepts_the_insertion() {
        let raw = "paths: [features]\nresources:\n  api:\n";
        let after = splice(raw, "api", "stub", &body(&[("base_url", "http://a.local")]))
            .expect("a null group is an empty group, not an error");
        assert_eq!(
            after,
            raw.replace(
                "  api:\n",
                "  api:\n    stub:\n      base_url: http://a.local\n"
            )
        );
    }

    /// A nested key that happens to be named `resources:` deeper in the
    /// document must not out-anchor the real top-level one.
    #[test]
    fn a_nested_key_named_resources_does_not_shadow_the_real_one() {
        let raw = "other:\n  resources:\n    fake: true\nresources:\n  api:\n    stub:\n      base_url: http://stub.local\n";
        let after = splice(raw, "s3", "main", &body(&[("bucket", "photos")]))
            .expect("the real top-level resources: line is found");
        assert_eq!(
            after,
            raw.replace(
                "\nresources:\n",
                "\nresources:\n  s3:\n    main:\n      bucket: photos\n"
            )
        );
    }
}
