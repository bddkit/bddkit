//! What counts as a secret, and the filter that hides it (spec §7).
//!
//! Masking is render-only: nothing here rewrites a stored exchange, variable
//! or plugin argument — callers hand text in and get masked text back.

use crate::vars::NULL_SENTINEL;
use anyhow::{Result, bail};
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// What a masked value is replaced with. Not configurable (spec §7.8).
pub const MASK: &str = "***";

pub const DEFAULT_WORDS: &[&str] = &["secret"];
pub const DEFAULT_FIELDS: &[&str] = &["password", "secret", "token"];
pub const DEFAULT_MIN_LENGTH: usize = 7;

/// A variable name is sensitive when a privacy word is one of its
/// `_`-separated words, at any position, compared without case. camelCase is
/// NOT split: the suite's author names variables and can write `session_secret`.
pub fn variable_is_sensitive<S: AsRef<str>>(name: &str, words: &[S]) -> bool {
    name.split('_').any(|part| contains_word(words, part))
}

fn contains_word<S: AsRef<str>>(words: &[S], candidate: &str) -> bool {
    words
        .iter()
        .any(|word| word.as_ref().eq_ignore_ascii_case(candidate))
}

/// A field name chosen by the system under test is sensitive when a field word
/// is one of its words. Split on `_`, `-` and lower-to-upper case boundaries,
/// since the suite's author cannot rename these.
pub fn field_is_sensitive<S: AsRef<str>>(name: &str, fields: &[S]) -> bool {
    let mut word = String::new();
    let mut previous_is_lower = false;
    for c in name.chars() {
        let boundary = c == '_' || c == '-' || (previous_is_lower && c.is_uppercase());
        if boundary && contains_word(fields, &std::mem::take(&mut word)) {
            return true;
        }
        if c != '_' && c != '-' {
            word.push(c);
        }
        previous_is_lower = c.is_lowercase();
    }
    contains_word(fields, &word)
}

/// Where an option's value came from, lowest precedence first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Default,
    Config,
    Env,
    Flag,
}

impl Layer {
    /// The exact spelling of a privacy option in this layer, so an error says
    /// what to edit: `privacy.fields`, `BDDKIT_PRIVACY_FIELDS`, `--privacy-fields`.
    pub fn spell(self, key: &str) -> String {
        match self {
            Layer::Default | Layer::Config => format!("privacy.{key}"),
            Layer::Env => format!("BDDKIT_PRIVACY_{}", key.to_ascii_uppercase()),
            Layer::Flag => format!("--privacy-{}", key.replace('_', "-")),
        }
    }
}

/// What one layer says about privacy, as the config's `privacy:` section or as
/// built from the environment or the command line. `None` is "nothing set":
/// an absent key and `~` alike, while `Some(vec![])` is an empty list.
#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivacyLayer {
    pub words: Option<Vec<String>>,
    pub words_only: Option<Vec<String>>,
    pub fields: Option<Vec<String>>,
    pub fields_only: Option<Vec<String>>,
    pub min_length: Option<usize>,
    pub reveal: Option<bool>,
}

/// The vocabulary in force after every layer has been folded in (spec §7.7).
#[derive(Debug, Clone, PartialEq)]
pub struct Privacy {
    pub words: Vec<String>,
    pub fields: Vec<String>,
    pub min_length: usize,
    pub reveal: bool,
    /// The layer that decided `reveal`, for the `doctor` row.
    pub reveal_from: Layer,
}

impl Default for Privacy {
    fn default() -> Self {
        Self {
            words: owned(DEFAULT_WORDS),
            fields: owned(DEFAULT_FIELDS),
            min_length: DEFAULT_MIN_LENGTH,
            reveal: false,
            reveal_from: Layer::Default,
        }
    }
}

/// A comma-separated list as a flag or a variable carries it: parts trimmed,
/// empty ones dropped, so `a, b,` is `[a, b]` and an empty value is an empty list.
pub fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

/// A boolean variable: `true`, `false`, `1` or `0`, and nothing else.
pub fn env_bool(name: &str, value: &str) -> Result<bool> {
    match value.trim() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => bail!("{name} is {value:?}, expected true, false, 1 or 0"),
    }
}

/// The environment layer. `get` returns a variable's value, and `None` for one
/// that is absent (an empty one is treated as absent): an empty variable is unset, as for `BDDKIT_EVENTS`,
/// so the environment cannot say "replace with nothing" — a flag or the config can.
pub fn env_layer(get: &dyn Fn(&str) -> Option<String>) -> Result<PrivacyLayer> {
    let get = |name: &str| get(name).filter(|value| !value.is_empty());
    let list = |key: &str| get(&format!("BDDKIT_PRIVACY_{key}")).map(|v| split_list(&v));
    let min_length = get("BDDKIT_PRIVACY_MIN_LENGTH")
        .map(|v| {
            v.trim().parse::<usize>().map_err(|_| {
                anyhow::anyhow!(
                    "BDDKIT_PRIVACY_MIN_LENGTH is {v:?}, expected a whole number of characters"
                )
            })
        })
        .transpose()?;
    let reveal = get("BDDKIT_PRIVACY_REVEAL")
        .map(|v| env_bool("BDDKIT_PRIVACY_REVEAL", &v))
        .transpose()?;
    Ok(PrivacyLayer {
        words: list("WORDS"),
        words_only: list("WORDS_ONLY"),
        fields: list("FIELDS"),
        fields_only: list("FIELDS_ONLY"),
        min_length,
        reveal,
    })
}

impl Privacy {
    /// The vocabulary a run or `doctor` works with: the config's section when
    /// there is one, then the environment, then the flags.
    pub fn from_sources(
        config: Option<&PrivacyLayer>,
        get: &dyn Fn(&str) -> Option<String>,
        flags: PrivacyLayer,
    ) -> Result<Self> {
        let mut layers = Vec::new();
        if let Some(config) = config {
            layers.push((Layer::Config, config.clone()));
        }
        layers.push((Layer::Env, env_layer(get)?));
        layers.push((Layer::Flag, flags));
        Self::resolve(&layers)
    }

    /// Where `reveal` was decided, spelled as the user wrote it: `--privacy-reveal`,
    /// `BDDKIT_PRIVACY_REVEAL`, `privacy.reveal`, or `default`.
    pub fn reveal_source(&self) -> String {
        match self.reveal_from {
            Layer::Default => "default".to_string(),
            layer => layer.spell("reveal"),
        }
    }

    /// The stderr line a revealed run prints.
    pub fn reveal_notice(&self) -> String {
        format!(
            "warning: masking is off (set by {}); secrets appear in clear text",
            self.reveal_source()
        )
    }
}

fn owned(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

impl Privacy {
    /// Folds layers given lowest precedence first. Per list, `*_only` resets
    /// what the layers below built and the extend form appends to it; for a
    /// scalar the last layer that sets it wins.
    pub fn resolve(layers: &[(Layer, PrivacyLayer)]) -> Result<Self> {
        let mut privacy = Self::default();
        for (layer, set) in layers {
            fold(
                &mut privacy.words,
                (&set.words, &set.words_only),
                ("words", "words_only"),
                *layer,
            )?;
            fold(
                &mut privacy.fields,
                (&set.fields, &set.fields_only),
                ("fields", "fields_only"),
                *layer,
            )?;
            if let Some(min_length) = set.min_length {
                privacy.min_length = min_length;
            }
            if let Some(reveal) = set.reveal {
                privacy.reveal = reveal;
                privacy.reveal_from = *layer;
            }
        }
        Ok(privacy)
    }
}

/// A privacy word: letters and digits, lowercased. The validator `fold_list`
/// takes returns `None` for a value that is not one.
fn word(value: &str) -> Option<String> {
    (!value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric()))
        .then(|| value.to_ascii_lowercase())
}

fn fold(
    list: &mut Vec<String>,
    (extend, only): (&Option<Vec<String>>, &Option<Vec<String>>),
    (extend_key, only_key): (&str, &str),
    layer: Layer,
) -> Result<()> {
    fold_list(
        list,
        (extend, only),
        (&layer.spell(extend_key), &layer.spell(only_key)),
        (&word, "a word of letters and digits"),
    )
}

/// Returns a value's normal form, or `None` when it is not acceptable.
pub type Normalise<'a> = &'a dyn Fn(&str) -> Option<String>;

/// One list option of one layer applied to what the layers below built:
/// `only` resets it, `extend` appends. `labels` spell the two keys the way the
/// user wrote them, and `valid` returns a value's normal form or `None` for one
/// that is not acceptable, which `shape` then describes.
pub fn fold_list(
    list: &mut Vec<String>,
    (extend, only): (&Option<Vec<String>>, &Option<Vec<String>>),
    (extend_label, only_label): (&str, &str),
    (valid, shape): (Normalise, &str),
) -> Result<()> {
    if extend.is_some() && only.is_some() {
        bail!(
            "{extend_label} and {only_label} are both set; use {only_label} to replace the list, {extend_label} to add to it"
        );
    }
    let normalised = |values: &[String], label: &str| -> Result<Vec<String>> {
        values
            .iter()
            .map(|value| {
                valid(value).ok_or_else(|| anyhow::anyhow!("{label}: {value:?} is not {shape}"))
            })
            .collect()
    };
    if let Some(only) = only {
        *list = normalised(only, only_label)?;
    }
    if let Some(extend) = extend {
        for value in normalised(extend, extend_label)? {
            if !list.contains(&value) {
                list.push(value);
            }
        }
    }
    Ok(())
}

/// A value found under a sensitive name: where it stands in the text, and what
/// it is when it is a string (a number is masked where it stands and never
/// joins the set).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub range: std::ops::Range<usize>,
    pub value: Option<String>,
}

/// Every string or number under a key with a field word in its name, at any
/// depth of a JSON document. `true`, `false` and `null` are left alone, an
/// object is descended into, and the scalars of an array take the sensitivity
/// of the key above it. A string's range is the text between its quotes, so
/// the rest of the document keeps its exact bytes. Text that is not JSON has
/// no hits.
pub fn json_hits<S: AsRef<str>>(text: &str, fields: &[S]) -> Vec<Hit> {
    // Validated first (and so depth-limited by serde_json): the scanner below
    // may then trust the shape of what it walks.
    if serde_json::from_str::<serde::de::IgnoredAny>(text).is_err() {
        return Vec::new();
    }
    let mut scan = Scan {
        text,
        at: 0,
        hits: Vec::new(),
    };
    scan.value(false, fields);
    scan.hits
}

struct Scan<'t> {
    text: &'t str,
    at: usize,
    hits: Vec<Hit>,
}

impl Scan<'_> {
    fn byte(&self) -> u8 {
        self.text.as_bytes()[self.at]
    }

    fn skip_space(&mut self) {
        while self
            .text
            .as_bytes()
            .get(self.at)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.at += 1;
        }
    }

    /// Past the closing quote; returns the range between the quotes.
    fn string(&mut self) -> std::ops::Range<usize> {
        let start = self.at + 1;
        self.at = start;
        while self.byte() != b'"' {
            self.at += if self.byte() == b'\\' { 2 } else { 1 };
        }
        let range = start..self.at;
        self.at += 1;
        range
    }

    fn decoded(&self, range: &std::ops::Range<usize>) -> Option<String> {
        serde_json::from_str(&self.text[range.start - 1..range.end + 1]).ok()
    }

    fn value<S: AsRef<str>>(&mut self, inherited: bool, fields: &[S]) {
        self.skip_space();
        match self.byte() {
            b'{' => {
                self.at += 1;
                loop {
                    self.skip_space();
                    match self.byte() {
                        b'}' => break,
                        b',' => self.at += 1,
                        _ => {
                            let key = self.string();
                            let sensitive = self
                                .decoded(&key)
                                .is_some_and(|name| field_is_sensitive(&name, fields));
                            self.skip_space();
                            self.at += 1; // ':'
                            self.value(sensitive, fields);
                        }
                    }
                }
                self.at += 1;
            }
            b'[' => {
                self.at += 1;
                loop {
                    self.skip_space();
                    match self.byte() {
                        b']' => break,
                        b',' => self.at += 1,
                        _ => self.value(inherited, fields),
                    }
                }
                self.at += 1;
            }
            b'"' => {
                let range = self.string();
                if inherited {
                    let value = self.decoded(&range);
                    self.hits.push(Hit { range, value });
                }
            }
            _ => {
                let start = self.at;
                while self
                    .text
                    .as_bytes()
                    .get(self.at)
                    .is_some_and(|b| !matches!(b, b',' | b']' | b'}') && !b.is_ascii_whitespace())
                {
                    self.at += 1;
                }
                let token = &self.text[start..self.at];
                if inherited && !matches!(token, "true" | "false" | "null") {
                    self.hits.push(Hit {
                        range: start..self.at,
                        value: None,
                    });
                }
            }
        }
    }
}

/// The values of `name=value` pairs joined by `&` — a form body or a query
/// string — whose name has a field word in it. `text` starts at the first pair;
/// `offset` is where it starts in the string the ranges are meant for. A name
/// is percent-decoded before it is compared, a value is returned decoded.
pub fn pair_hits<S: AsRef<str>>(text: &str, offset: usize, fields: &[S]) -> Vec<Hit> {
    let mut hits = Vec::new();
    let mut start = 0;
    for pair in text.split('&') {
        let from = start;
        start += pair.len() + 1;
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        let name = url::form_urlencoded::parse(name.as_bytes())
            .next()
            .map(|(name, _)| name.into_owned())
            .unwrap_or_default();
        if field_is_sensitive(&name, fields) {
            let begin = offset + from + (pair.len() - value.len());
            let decoded = url::form_urlencoded::parse(format!("v={value}").as_bytes())
                .next()
                .map(|(_, value)| value.into_owned());
            hits.push(Hit {
                range: begin..begin + value.len(),
                value: decoded,
            });
        }
    }
    hits
}

/// The hits of a URL's query string, as ranges into the whole URL.
pub fn query_hits<S: AsRef<str>>(url: &str, fields: &[S]) -> Vec<Hit> {
    let Some(question) = url.find('?') else {
        return Vec::new();
    };
    let query = &url[question + 1..];
    let end = query.find('#').unwrap_or(query.len());
    pair_hits(&query[..end], question + 1, fields)
}

/// `text` with each range replaced by `***`; ranges may be unordered and overlap.
pub fn mask_ranges(text: &str, ranges: &[std::ops::Range<usize>]) -> String {
    let mut ranges: Vec<_> = ranges.to_vec();
    ranges.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for range in ranges {
        if range.start < copied {
            copied = copied.max(range.end);
            continue;
        }
        out.push_str(&text[copied..range.start]);
        out.push_str(MASK);
        copied = range.end;
    }
    out.push_str(&text[copied..]);
    out
}

/// Words that make a name look sensitive although no privacy word is in it.
const HINT_WORDS: &[&str] = &["password", "token", "key"];

/// A text to scan for `<<name>>` slots: where it is written (`file:line`) and
/// the names a `secret`-typed parameter already marks inside it.
pub type Scanned<'a> = (String, &'a str, &'a [String]);

/// The `<<name>>` slots in `texts` whose name has a hint word in it (one of its
/// `_`-separated words), no privacy word and no `secret` parameter of that
/// name: they probably hold a secret nothing masks. Each name maps to the
/// places it was found. A hint only; it changes nothing that is masked.
pub fn unmarked_names<'a>(
    texts: impl IntoIterator<Item = Scanned<'a>>,
    words: &[String],
) -> BTreeMap<String, BTreeSet<String>> {
    static SLOT_NAME: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"<<([^\W\d]\w*)>>").expect("constant regex")
    });
    let mut names: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (place, text, marked) in texts {
        for captures in SLOT_NAME.captures_iter(text) {
            let name = &captures[1];
            if variable_is_sensitive(name, HINT_WORDS)
                && !variable_is_sensitive(name, words)
                && !marked.iter().any(|m| m == name)
            {
                names
                    .entry(name.to_string())
                    .or_default()
                    .insert(place.clone());
            }
        }
    }
    names
}

/// A variable that is set and not empty, as text. An empty value is unset.
pub fn process_env(name: &str) -> Option<String> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string_lossy().into_owned())
}

/// `%XX` decoded and nothing else: a URL's userinfo may hold a literal `+` or
/// `&`, which a form decoder would turn into a space or cut at.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = bytes.get(i + 1..i + 3).and_then(|hex| {
            let hex = std::str::from_utf8(hex).ok()?;
            u8::from_str_radix(hex, 16).ok()
        });
        match (bytes[i], escaped) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                i += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Every string in a parsed config that is a secret by the rules of spec §7.6:
/// a scalar under a key with a field word in its name, and the password of any
/// string that is a URL with credentials, as written and percent-decoded.
pub fn secrets_in(doc: &serde_yaml_ng::Value, fields: &[String]) -> Vec<String> {
    use serde_yaml_ng::Value;
    fn scalar(value: &Value) -> Option<String> {
        match value {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        }
    }
    // `inherited`: an array has no keys of its own, so its scalars take the
    // sensitivity of the key above (§7.5: an array is descended into).
    fn walk(value: &Value, fields: &[String], inherited: bool, found: &mut Vec<String>) {
        match value {
            Value::String(text) => {
                if inherited {
                    found.push(text.clone());
                }
                if let Some(password) = url::Url::parse(text)
                    .ok()
                    .and_then(|u| u.password().map(str::to_string))
                {
                    found.push(percent_decode(&password));
                    found.push(password);
                }
            }
            Value::Number(number) if inherited => found.push(number.to_string()),
            Value::Sequence(items) => items
                .iter()
                .for_each(|item| walk(item, fields, inherited, found)),
            Value::Mapping(map) => {
                for (key, item) in map {
                    let sensitive = key
                        .as_str()
                        .is_some_and(|name| field_is_sensitive(name, fields));
                    match (sensitive, scalar(item)) {
                        (true, Some(secret)) => found.push(secret),
                        _ => walk(item, fields, sensitive, found),
                    }
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk(doc, fields, false, &mut found);
    found
}

/// What this process knows about privacy before and outside any file's own
/// state: the flags `main` parsed, and the candidate secrets the configuration
/// holds. Text printed with no `World` around it — `error:` lines, `doctor`,
/// a driver's connection error — is masked from here.
struct Process {
    flags: PrivacyLayer,
    min_length: usize,
    reveal: bool,
    candidates: Vec<String>,
}

static PROCESS: Mutex<Process> = Mutex::new(Process {
    flags: PrivacyLayer {
        words: None,
        words_only: None,
        fields: None,
        fields_only: None,
        min_length: None,
        reveal: None,
    },
    min_length: DEFAULT_MIN_LENGTH,
    reveal: false,
    candidates: Vec::new(),
});

fn process() -> std::sync::MutexGuard<'static, Process> {
    // A panic elsewhere must not stop an error message from being masked.
    PROCESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `main` hands over the flag layer once, before any config is read. The
/// vocabulary the flags and the environment alone select applies from here, so
/// an error raised before the config is parsed is masked the way they say.
pub fn set_flags(flags: PrivacyLayer) {
    let early = Privacy::from_sources(None, &process_env, flags.clone());
    process().flags = flags;
    if let Ok(privacy) = early {
        record(&privacy, []);
    }
}

pub fn flags() -> PrivacyLayer {
    process().flags.clone()
}

/// Records what a loaded configuration decided: its resolved vocabulary, and
/// the values it holds that must never be printed.
pub fn record(privacy: &Privacy, values: impl IntoIterator<Item = String>) {
    let mut process = process();
    process.min_length = privacy.min_length;
    process.reveal = privacy.reveal;
    process.candidates.extend(values);
}

/// The secrets recorded so far, filtered by the `min_length` in force.
pub fn recorded() -> Secrets {
    let process = process();
    let mut secrets = Secrets::new(process.min_length);
    process.candidates.iter().for_each(|value| {
        secrets.add(value);
    });
    secrets
}

/// `text` with the recorded secrets masked, or as it is under `--reveal`.
pub fn mask_text(text: &str) -> String {
    if process().reveal {
        return text.to_string();
    }
    recorded().mask(text).into_owned()
}

/// A file's view of privacy: the vocabulary and the set of secrets, shared by
/// everything that file owns (its variables, its HTTP state, its output). A
/// clone is another handle on the SAME set; `fork` makes an independent copy.
/// Registering and masking are cheap and never held across an await.
#[derive(Clone)]
pub struct Redactor(Arc<Mutex<State>>);

#[derive(Clone)]
struct State {
    enabled: bool,
    secrets: Secrets,
    words: Vec<String>,
    fields: Vec<String>,
}

impl Redactor {
    pub fn new(privacy: &Privacy, secrets: Secrets) -> Self {
        Self(Arc::new(Mutex::new(State {
            enabled: !privacy.reveal,
            secrets,
            words: privacy.words.clone(),
            fields: privacy.fields.clone(),
        })))
    }

    /// Masks nothing and registers nothing: `--reveal`, and a `World` nobody
    /// gave a vocabulary to.
    pub fn disabled() -> Self {
        let privacy = Privacy {
            reveal: true,
            ..Privacy::default()
        };
        Self::new(&privacy, Secrets::new(DEFAULT_MIN_LENGTH))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// An independent copy: a file starts from the run-wide set and what it
    /// registers stays its own.
    pub fn fork(&self) -> Self {
        Self(Arc::new(Mutex::new(self.state().clone())))
    }

    pub fn enabled(&self) -> bool {
        self.state().enabled
    }

    /// Registers a variable's value when a privacy word is in its name.
    pub fn variable(&self, name: &str, value: &str) {
        let mut state = self.state();
        if state.enabled && variable_is_sensitive(name, &state.words) {
            state.secrets.add(value);
        }
    }

    /// A variable's value as debug output may show it: `***` when a privacy
    /// word is in its NAME, so even a value too short to join the set is hidden
    /// by position; otherwise the value, masked.
    pub fn shown(&self, name: &str, value: &str) -> String {
        let sensitive = variable_is_sensitive(name, &self.state().words);
        self.show(sensitive, value)
    }

    /// `***` for a value known to be sensitive by position, otherwise the value
    /// with the set applied; the value as it is when masking is off.
    pub fn show(&self, sensitive: bool, value: &str) -> String {
        if sensitive && self.enabled() {
            MASK.to_string()
        } else {
            self.mask(value)
        }
    }

    /// Registers a value that is secret whatever it is called.
    pub fn secret(&self, value: &str) {
        let mut state = self.state();
        if state.enabled {
            state.secrets.add(value);
        }
    }

    pub fn field_is_sensitive(&self, name: &str) -> bool {
        let state = self.state();
        state.enabled && field_is_sensitive(name, &state.fields)
    }

    /// Values under sensitive names, as ranges into `text`. All of these are
    /// empty while the redactor is disabled.
    pub fn json_hits(&self, text: &str) -> Vec<Hit> {
        let state = self.state();
        if state.enabled {
            json_hits(text, &state.fields)
        } else {
            Vec::new()
        }
    }

    pub fn pair_hits(&self, text: &str) -> Vec<Hit> {
        let state = self.state();
        if state.enabled {
            pair_hits(text, 0, &state.fields)
        } else {
            Vec::new()
        }
    }

    pub fn query_hits(&self, url: &str) -> Vec<Hit> {
        let state = self.state();
        if state.enabled {
            query_hits(url, &state.fields)
        } else {
            Vec::new()
        }
    }

    /// Adds the string values of `hits` to the set; a number never joins.
    pub fn register(&self, hits: &[Hit]) {
        let mut state = self.state();
        if state.enabled {
            hits.iter()
                .filter_map(|hit| hit.value.as_deref())
                .for_each(|v| {
                    state.secrets.add(v);
                });
        }
    }

    /// Text that may be a JSON document: values under sensitive names by
    /// position first, then the set. For JSON a step prints again after it was
    /// cut out of the exchange it came from.
    pub fn mask_json(&self, text: &str) -> String {
        let ranges: Vec<_> = self
            .json_hits(text)
            .into_iter()
            .map(|hit| hit.range)
            .collect();
        self.mask(&mask_ranges(text, &ranges))
    }

    pub fn mask(&self, text: &str) -> String {
        let state = self.state();
        if state.enabled {
            state.secrets.mask(text).into_owned()
        } else {
            text.to_string()
        }
    }
}

/// The set of secret values and the replacement over them.
#[derive(Clone)]
pub struct Secrets {
    min_length: usize,
    values: BTreeSet<String>,
}

impl Secrets {
    pub fn new(min_length: usize) -> Self {
        Self {
            min_length,
            values: BTreeSet::new(),
        }
    }

    /// Adds a value to the set, with the two encodings it takes in the dump: a
    /// query string or form field (`Hunter2!` travels as `Hunter2%21`) and a JSON
    /// string (`a"b` travels as `a\"b`). Returns whether it joined: an empty
    /// value, the SQL NULL sentinel and anything shorter than `min_length`
    /// characters never do — masking every `42` in the output would destroy it.
    /// Length is checked on the value as written, so a short value cannot get in
    /// through its longer encoding.
    // ponytail: percent-encoding with `%20` for a space (not `+`) is not registered.
    pub fn add(&mut self, value: &str) -> bool {
        if value.is_empty() || value == NULL_SENTINEL || value.chars().count() < self.min_length {
            return false;
        }
        let form: String = url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
        let json = serde_json::to_string(value).unwrap_or_default();
        let json = json.trim_matches('"');
        for form in [value, form.as_str(), json] {
            if !form.is_empty() {
                self.values.insert(form.to_string());
            }
        }
        true
    }

    /// Replaces every occurrence of every value with `***`. Occurrences that
    /// overlap or contain one another merge into one mask, so `1234567` and
    /// `4567890` hide all of `1234567890`.
    pub fn mask<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let spans: Vec<_> = self
            .values
            .iter()
            .flat_map(|v| text.match_indices(v.as_str()).map(|(i, m)| i..i + m.len()))
            .collect();
        if spans.is_empty() {
            Cow::Borrowed(text)
        } else {
            Cow::Owned(mask_ranges(text, &spans))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORDS: &[&str] = &["secret"];
    const FIELDS: &[&str] = &["password", "secret", "token"];

    #[test]
    fn a_privacy_word_matches_at_any_position_of_a_variable_name() {
        for name in [
            "access_token_secret",
            "secret_reg_verifier",
            "reg_secret_verifier",
            "SECRET",
            "buyer_token_secret",
        ] {
            assert!(variable_is_sensitive(name, WORDS), "{name}");
        }
    }

    #[test]
    fn a_name_that_only_contains_a_privacy_word_is_not_sensitive() {
        for name in ["secretary_id", "sessionSecret", "topsecret", "id"] {
            assert!(!variable_is_sensitive(name, WORDS), "{name}");
        }
    }

    #[test]
    fn a_field_word_matches_in_any_naming_style() {
        for name in [
            "password",
            "access_token",
            "accessToken",
            "refresh-token",
            "client_secret",
            "Token",
        ] {
            assert!(field_is_sensitive(name, FIELDS), "{name}");
        }
    }

    #[test]
    fn a_field_that_only_contains_a_field_word_is_not_sensitive() {
        for name in ["tokenizer", "key", "tokens", "passwords"] {
            assert!(!field_is_sensitive(name, FIELDS), "{name}");
        }
    }

    #[test]
    fn an_empty_word_list_matches_nothing() {
        assert!(!field_is_sensitive("password", &[] as &[&str]));
    }

    #[test]
    fn a_short_value_is_not_added_to_the_set() {
        let mut secrets = Secrets::new(7);
        assert!(!secrets.add("hunter"));
        assert_eq!(secrets.mask("hunter 42"), "hunter 42");
    }

    #[test]
    fn a_value_of_exactly_min_length_joins_and_masks() {
        let mut secrets = Secrets::new(7);
        assert!(secrets.add("hunter2"));
        assert_eq!(secrets.mask("pw=hunter2!"), "pw=***!");
    }

    #[test]
    fn empty_and_null_sentinel_values_never_join() {
        let mut secrets = Secrets::new(0);
        assert!(!secrets.add(""));
        assert!(!secrets.add(NULL_SENTINEL));
    }

    #[test]
    fn length_counts_characters_not_bytes() {
        let mut secrets = Secrets::new(4);
        assert!(!secrets.add("пас"));
        assert!(secrets.add("пароль"));
    }

    #[test]
    fn a_longer_secret_is_masked_before_one_it_contains() {
        let mut secrets = Secrets::new(4);
        secrets.add("abcd");
        secrets.add("abcdefgh");
        assert_eq!(secrets.mask("x abcdefgh y abcd"), "x *** y ***");
    }

    #[test]
    fn overlapping_secrets_merge_into_one_mask() {
        let mut secrets = Secrets::new(4);
        secrets.add("1234567");
        secrets.add("4567890");
        assert_eq!(secrets.mask("id 1234567890 end"), "id *** end");
    }

    #[test]
    fn the_form_and_json_encodings_of_a_secret_are_masked_too() {
        let mut secrets = Secrets::new(7);
        secrets.add("Hunter2!\"/x");
        let form = "pass=Hunter2%21%22%2Fx&a=1";
        let json = r#"{"pass":"Hunter2!\"/x"}"#;
        assert_eq!(secrets.mask(form), "pass=***&a=1");
        assert_eq!(secrets.mask(json), r#"{"pass":"***"}"#);
    }

    #[test]
    fn a_short_value_does_not_join_through_its_longer_encoding() {
        let mut secrets = Secrets::new(7);
        assert!(!secrets.add("a b!c"));
        assert_eq!(secrets.mask("a+b%21c"), "a+b%21c");
    }

    #[test]
    fn metacharacters_in_a_value_are_literal() {
        let mut secrets = Secrets::new(4);
        secrets.add("a.b*(c)");
        assert_eq!(secrets.mask("a.b*(c) axb"), "*** axb");
    }

    #[test]
    fn every_occurrence_is_masked_and_an_empty_set_leaves_text_borrowed() {
        let mut secrets = Secrets::new(4);
        assert!(matches!(secrets.mask("nothing"), Cow::Borrowed(_)));
        secrets.add("tok-123");
        assert_eq!(secrets.mask("tok-123 and tok-123"), "*** and ***");
    }

    fn layer(source: &str) -> PrivacyLayer {
        serde_yaml_ng::from_str(source).expect("a privacy layer")
    }

    fn resolve(layers: &[(Layer, &str)]) -> Result<Privacy> {
        let layers: Vec<_> = layers.iter().map(|(l, y)| (*l, layer(y))).collect();
        Privacy::resolve(&layers)
    }

    #[test]
    fn no_layer_gives_the_defaults() {
        assert_eq!(resolve(&[]).unwrap(), Privacy::default());
        assert_eq!(
            resolve(&[(Layer::Config, "{}")]).unwrap(),
            Privacy::default()
        );
    }

    #[test]
    fn an_extend_list_keeps_the_defaults() {
        let privacy = resolve(&[(Layer::Config, "fields: [otp]")]).unwrap();
        assert_eq!(privacy.fields, ["password", "secret", "token", "otp"]);
    }

    #[test]
    fn only_resets_the_default_words() {
        let privacy = resolve(&[(Layer::Config, "words_only: [gdpr]")]).unwrap();
        assert_eq!(privacy.words, ["gdpr"]);
    }

    #[test]
    fn an_empty_only_list_empties_the_rule() {
        let privacy = resolve(&[(Layer::Config, "fields_only: []")]).unwrap();
        assert!(privacy.fields.is_empty());
    }

    #[test]
    fn null_and_an_empty_extend_list_mean_the_default() {
        for source in ["fields: ~", "fields: []", "words: ~", "min_length: ~"] {
            assert_eq!(
                resolve(&[(Layer::Config, source)]).unwrap(),
                Privacy::default()
            );
        }
    }

    #[test]
    fn a_higher_only_discards_lower_layers_and_a_higher_extend_adds() {
        let privacy = resolve(&[
            (Layer::Config, "fields: [otp]"),
            (Layer::Env, "fields_only: [pin]"),
            (Layer::Flag, "fields: [cvv]"),
        ])
        .unwrap();
        assert_eq!(privacy.fields, ["pin", "cvv"]);
    }

    #[test]
    fn both_forms_in_one_layer_are_refused_naming_the_layer() {
        let error = resolve(&[(Layer::Env, "fields: [a]\nfields_only: [b]")])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("BDDKIT_PRIVACY_FIELDS_ONLY")
                && error.contains("BDDKIT_PRIVACY_FIELDS "),
            "{error}"
        );
    }

    #[test]
    fn a_word_in_a_different_case_is_not_added_twice() {
        let privacy = resolve(&[(Layer::Config, "fields: [Token, OTP]")]).unwrap();
        assert_eq!(privacy.fields, ["password", "secret", "token", "otp"]);
    }

    #[test]
    fn a_word_must_be_letters_and_digits() {
        for word in ["pass word", "a-b", "", "pa$s"] {
            let source = format!("words: [{word:?}]");
            assert!(resolve(&[(Layer::Config, &source)]).is_err(), "{word:?}");
        }
    }

    #[test]
    fn the_flag_beats_the_environment_beats_the_config_for_min_length() {
        let layers = [
            (Layer::Config, "min_length: 3"),
            (Layer::Env, "min_length: 5"),
            (Layer::Flag, "min_length: 9"),
        ];
        assert_eq!(resolve(&layers).unwrap().min_length, 9);
        assert_eq!(resolve(&layers[..2]).unwrap().min_length, 5);
        assert_eq!(resolve(&layers[..1]).unwrap().min_length, 3);
    }

    #[test]
    fn the_environment_overrides_a_committed_reveal_and_the_deciding_layer_is_kept() {
        let privacy = resolve(&[
            (Layer::Config, "reveal: true"),
            (Layer::Env, "reveal: false"),
        ])
        .unwrap();
        assert!(!privacy.reveal);
        assert_eq!(privacy.reveal_from, Layer::Env);
        let privacy = resolve(&[(Layer::Config, "reveal: true")]).unwrap();
        assert_eq!((privacy.reveal, privacy.reveal_from), (true, Layer::Config));
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn the_environment_layer_reads_lists_numbers_and_booleans() {
        let get = env(&[
            ("BDDKIT_PRIVACY_FIELDS", "otp, pin,"),
            ("BDDKIT_PRIVACY_WORDS_ONLY", "gdpr"),
            ("BDDKIT_PRIVACY_MIN_LENGTH", "5"),
            ("BDDKIT_PRIVACY_REVEAL", "1"),
        ]);
        let layer = env_layer(&get).unwrap();
        assert_eq!(
            layer.fields,
            Some(vec!["otp".to_string(), "pin".to_string()])
        );
        assert_eq!(layer.words_only, Some(vec!["gdpr".to_string()]));
        assert_eq!((layer.min_length, layer.reveal), (Some(5), Some(true)));
    }

    #[test]
    fn a_bad_boolean_or_number_in_the_environment_names_the_variable() {
        for (name, value) in [
            ("BDDKIT_PRIVACY_REVEAL", "yes"),
            ("BDDKIT_PRIVACY_MIN_LENGTH", "-1"),
        ] {
            let error = env_layer(&env(&[(name, value)])).unwrap_err().to_string();
            assert!(error.contains(name), "{error}");
        }
    }

    #[test]
    fn a_flag_layer_empty_list_replaces_but_the_environment_cannot_say_it() {
        let flags = PrivacyLayer {
            fields_only: Some(split_list("")),
            ..PrivacyLayer::default()
        };
        let privacy = Privacy::from_sources(None, &env(&[]), flags).unwrap();
        assert!(privacy.fields.is_empty());
        let layer = env_layer(&env(&[("BDDKIT_PRIVACY_FIELDS_ONLY", "")]));
        assert_eq!(layer.unwrap().fields_only, None);
    }

    fn walked(yaml: &str) -> Vec<String> {
        let doc = serde_yaml_ng::from_str(yaml).expect("yaml");
        let fields: Vec<String> = DEFAULT_FIELDS.iter().map(|s| s.to_string()).collect();
        secrets_in(&doc, &fields)
    }

    #[test]
    fn the_walk_finds_values_under_field_word_keys_at_any_depth() {
        let found = walked(
            "db:\n  main:\n    password: hunter22\n    secret_access_key: abcd1234\n    host: db\napi_token: 12345678\n",
        );
        assert_eq!(found, ["hunter22", "abcd1234", "12345678"]);
    }

    #[test]
    fn the_walk_ignores_a_key_that_only_contains_a_field_word() {
        assert!(walked("tokenizer: abcdefgh\nkey: abcdefgh\n").is_empty());
    }

    #[test]
    fn the_walk_takes_the_password_of_a_url_as_written_and_decoded() {
        let found =
            walked("dsn: postgres://app:p%40ss%21word@db/app\nplain: http://example.test/x\n");
        assert_eq!(found, ["p@ss!word", "p%40ss%21word"]);
    }

    #[test]
    fn a_literal_plus_and_ampersand_in_a_url_password_survive_decoding() {
        let found = walked("dsn: postgres://app:a+b&c%21@db/app\n");
        assert_eq!(found, ["a+b&c!", "a+b&c%21"]);
    }

    #[test]
    fn the_scalars_of_an_array_take_the_sensitivity_of_its_key() {
        let found = walked("client_secret: [abcdefgh, 12345678, {nested: x}]\nother: [abcdefgh]\n");
        assert_eq!(found, ["abcdefgh", "12345678"]);
    }

    #[test]
    fn the_walk_reaches_urls_inside_lists() {
        assert_eq!(
            walked("urls: [mysql://u:hunter22@h/d]\n"),
            ["hunter22", "hunter22"]
        );
    }

    #[test]
    fn a_redactor_registers_by_name_and_a_fork_is_independent() {
        let privacy = Privacy::default();
        let run_wide = Redactor::new(&privacy, Secrets::new(7));
        let file = run_wide.fork();
        file.variable("access_secret", "tok-abcdefg");
        file.variable("plain", "other-value1");
        assert_eq!(
            file.mask("a tok-abcdefg b other-value1"),
            "a *** b other-value1"
        );
        assert_eq!(run_wide.mask("tok-abcdefg"), "tok-abcdefg");
    }

    #[test]
    fn a_disabled_redactor_masks_and_registers_nothing() {
        let privacy = Privacy {
            reveal: true,
            ..Privacy::default()
        };
        let redactor = Redactor::new(&privacy, Secrets::new(7));
        redactor.secret("tok-abcdefg");
        assert_eq!(redactor.mask("tok-abcdefg"), "tok-abcdefg");
        assert!(!redactor.field_is_sensitive("password"));
    }

    fn hits(json: &str) -> Vec<(String, Option<String>)> {
        let fields: Vec<String> = DEFAULT_FIELDS.iter().map(|s| s.to_string()).collect();
        json_hits(json, &fields)
            .into_iter()
            .map(|hit| (json[hit.range].to_string(), hit.value))
            .collect()
    }

    #[test]
    fn a_sensitive_field_is_found_at_any_depth_and_in_any_naming_style() {
        let found = hits(
            r#"{"a": {"accessToken": "t1", "list": [{"refresh-token": "t2"}, {"client_secret": 12345}]}, "password": "p1"}"#,
        );
        assert_eq!(
            found,
            [
                ("t1".into(), Some("t1".into())),
                ("t2".into(), Some("t2".into())),
                ("12345".into(), None),
                ("p1".into(), Some("p1".into())),
            ]
        );
    }

    #[test]
    fn a_field_that_only_contains_a_field_word_is_not_a_hit() {
        assert!(hits(r#"{"tokenizer": "abc", "key": "def"}"#).is_empty());
    }

    #[test]
    fn booleans_and_null_are_left_alone_and_containers_are_descended_into() {
        let found =
            hits(r#"{"token": true, "secret": null, "password": {"inner": "x", "token": "y"}}"#);
        assert_eq!(found, [("y".into(), Some("y".into()))]);
    }

    #[test]
    fn a_json_array_takes_the_sensitivity_of_its_key() {
        let found = hits(r#"{"token": ["a1", 22, {"plain": "b"}], "other": ["c"]}"#);
        assert_eq!(
            found,
            [("a1".into(), Some("a1".into())), ("22".into(), None)]
        );
    }

    #[test]
    fn an_escaped_value_is_decoded_for_the_set_and_located_as_written() {
        let json = r#"{"password": "a\"b\\c\u0041"}"#;
        let found = hits(json);
        assert_eq!(
            found,
            [(r#"a\"b\\c\u0041"#.into(), Some("a\"b\\cA".into()))]
        );
    }

    #[test]
    fn text_that_is_not_json_has_no_hits() {
        assert!(hits("password=abc").is_empty());
        assert!(hits(r#"{"password": "abc""#).is_empty());
    }

    #[test]
    fn a_body_keeps_its_exact_text_around_a_masked_value() {
        let json = "{ \"user\" : \"ann\",\n  \"token\":   \"abc\" ,\"n\":1 }";
        let fields = ["token".to_string()];
        let ranges: Vec<_> = json_hits(json, &fields)
            .into_iter()
            .map(|h| h.range)
            .collect();
        assert_eq!(
            mask_ranges(json, &ranges),
            "{ \"user\" : \"ann\",\n  \"token\":   \"***\" ,\"n\":1 }"
        );
    }

    #[test]
    fn query_and_form_pairs_are_masked_by_name_with_their_encoding_kept() {
        let fields = ["token".to_string(), "password".to_string()];
        let url = "http://x/p?a=1&access_token=ab%21cd&Password=p+w#frag";
        let found = query_hits(url, &fields);
        let ranges: Vec<_> = found.iter().map(|h| h.range.clone()).collect();
        assert_eq!(
            mask_ranges(url, &ranges),
            "http://x/p?a=1&access_token=***&Password=***#frag"
        );
        assert_eq!(found[0].value.as_deref(), Some("ab!cd"));
        assert_eq!(found[1].value.as_deref(), Some("p w"));

        let form = "a=1&client_secret=x&token=short";
        let found = pair_hits(form, 0, &fields);
        let ranges: Vec<_> = found.iter().map(|h| h.range.clone()).collect();
        assert_eq!(mask_ranges(form, &ranges), "a=1&client_secret=x&token=***");
    }

    #[test]
    fn a_name_with_a_hint_word_and_no_privacy_word_is_listed() {
        let words = ["secret".to_string()];
        let texts = [
            "I use <<session_token>> and <<API_KEY>> and <<primary_key>>",
            "but not <<token_secret>>, <<monkey>>, <<unique()>> or <<name>>",
        ];
        let names = unmarked_names(
            texts.map(|t| ("a.feature:3".to_string(), t, &[][..])),
            &words,
        );
        assert_eq!(
            names.keys().collect::<Vec<_>>(),
            ["API_KEY", "primary_key", "session_token"]
        );
    }

    #[test]
    fn a_secret_typed_parameter_is_not_listed_and_a_text_one_is_found_where_it_is() {
        let words = ["secret".to_string()];
        let marked = ["password".to_string()];
        let body = r#"{"password": "<<password>>", "token": "<<token>>"}"#;
        let texts = [
            ("m.yaml:2".to_string(), body, &marked[..]),
            ("m.yaml:9".to_string(), "<<password>>", &[][..]),
            ("b.feature:5".to_string(), "<<token>> <<token>>", &[][..]),
        ];
        let names = unmarked_names(texts, &words);
        assert_eq!(names["password"].iter().collect::<Vec<_>>(), ["m.yaml:9"]);
        assert_eq!(
            names["token"].iter().collect::<Vec<_>>(),
            ["b.feature:5", "m.yaml:2"]
        );
    }

    #[test]
    fn registering_the_hits_of_a_json_document_hides_the_values_afterwards() {
        let redactor = Redactor::new(&Privacy::default(), Secrets::new(7));
        let json = r#"{"user": "ann", "token": "Zd1", "refresh_token": "diag-refresh-token-1"}"#;
        redactor.register(&redactor.json_hits(json));
        assert_eq!(redactor.mask("actual: diag-refresh-token-1"), "actual: ***");
        assert_eq!(redactor.mask("Zd1"), "Zd1", "too short to join the set");
    }

    #[test]
    fn deny_unknown_fields_rejects_a_misspelt_key() {
        let error = serde_yaml_ng::from_str::<PrivacyLayer>("wrods: [x]").unwrap_err();
        assert!(error.to_string().contains("wrods"), "{error}");
    }
}
