//! `bddkit init`: answers in, a suite out. `ask` is the only code that touches
//! `dialoguer`; `from_flags` and `apply` are what the tests drive.

use crate::{DirArgs, config, dirs, doctor, plugin, report, resource};
use anyhow::{Context as _, Result, bail};
use clap::{Args, ValueEnum};
use dialoguer::console::Term;
use dialoguer::{Input, MultiSelect, Select};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const FEATURES_DIR: &str = "features";
const GITIGNORE_LINES: [&str; 2] = [".bddkit/*.local.yaml", ".env.local"];

/// The question of `telemetry.md`, section 6, word for word.
const TELEMETRY_TEXT: &str =
    "bddkit can send anonymous usage counts to its maintainers after each run:
the bddkit version and OS, how long the run took, how many files, scenarios
and failures it had, and which kinds of resources the suite uses (APIs,
database engines, official plugins).
Never sent: names, step text, URLs, addresses, values, your config.
Your IP address reaches our server with the request and is dropped there;
it is never stored and never passed on.
bddkit is an open-source project; these counts are how we learn which
databases, plugins and suite sizes to work on, and whether a release broke
runs out there. The full list: https://bddkit.dev/telemetry
Change your answer any time: bddkit telemetry on|off
";

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Db {
    None,
    Postgres,
    Mysql,
    Mariadb,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Layer {
    /// For me on this machine
    User,
    /// For everyone with this repository, vendored into `.bddkit/`
    Project,
}

#[derive(Args)]
#[command(
    after_help = "Without a terminal nothing is asked: every answer comes from a flag, and
these defaults apply: no API, no database, no plugins, --parallel true, no
telemetry key.

Examples:
  bddkit init                                   ask a few questions
  bddkit init --api-url http://localhost:8080 --parallel false --telemetry false
  bddkit init --no-api --db postgres --dsn '${BDDKIT_DB_DSN}' --plugin browser --layer project"
)]
pub struct InitArgs {
    /// Where to write the config [default: $BDDKIT_CONFIG, else ./bddkit.yaml]
    #[arg(long)]
    config: Option<PathBuf>,
    /// Base URL of the API under test (question 1)
    #[arg(long, value_name = "URL", conflicts_with = "no_api")]
    api_url: Option<String>,
    /// The suite tests no HTTP service (question 1)
    #[arg(long)]
    no_api: bool,
    /// The database engine (question 2)
    #[arg(long, value_enum)]
    db: Option<Db>,
    /// The database DSN; an environment reference such as ${BDDKIT_DB_DSN} keeps the password out of the file
    #[arg(long)]
    dsn: Option<String>,
    /// A plugin to install, by index name or owner/repo (repeatable; questions 3)
    #[arg(long = "plugin", value_name = "NAME")]
    plugins: Vec<String>,
    /// Where the plugins go (question 4)
    #[arg(long, value_enum)]
    layer: Option<Layer>,
    /// Whether scenarios of different files may run at the same time (question 5)
    #[arg(long, action = clap::ArgAction::Set, value_name = "BOOL")]
    parallel: Option<bool>,
    /// Whether to allow anonymous usage counts (question 6)
    #[arg(long, action = clap::ArgAction::Set, value_name = "BOOL")]
    telemetry: Option<bool>,
    #[command(flatten)]
    dir: DirArgs,
}

pub struct Answers {
    api_url: Option<String>,
    db: Db,
    dsn: Option<String>,
    plugins: Vec<String>,
    layer: Layer,
    parallel: bool,
    telemetry: Option<bool>,
}

/// Mismatches that are wrong whether or not a terminal is there.
fn check_flags(args: &InitArgs) -> Result<()> {
    if matches!(args.db, None | Some(Db::None)) && args.dsn.is_some() {
        bail!("--dsn needs --db postgres, mysql or mariadb");
    }
    if args
        .api_url
        .as_deref()
        .is_some_and(|url| url.trim().is_empty())
    {
        bail!("--api-url is empty; --no-api says the suite has no API");
    }
    // `--bddkit-dir` / `$BDDKIT_DIR` replaces every layer, so there is no
    // layer to choose; `plugin install` refuses the same pair.
    if args.layer.is_some()
        && dirs::Env::from_process(args.dir.bddkit_dir.clone())
            .override_dir
            .is_some()
    {
        bail!(
            "--layer cannot be combined with --bddkit-dir / BDDKIT_DIR, which replaces every layer"
        );
    }
    Ok(())
}

/// Every answer from a flag, the spec's defaults for what is not given.
pub fn from_flags(args: &InitArgs) -> Result<Answers> {
    check_flags(args)?;
    if args.layer.is_some() && args.plugins.is_empty() {
        bail!("--layer needs at least one --plugin");
    }
    let db = args.db.unwrap_or(Db::None);
    let dsn = match db {
        Db::None => None,
        _ => Some(
            args.dsn
                .clone()
                .context("--db needs --dsn when nothing is asked")?,
        ),
    };
    Answers {
        api_url: args.api_url.clone(),
        db,
        dsn,
        plugins: args.plugins.clone(),
        layer: args.layer.unwrap_or(Layer::User),
        parallel: args.parallel.unwrap_or(true),
        telemetry: args.telemetry,
    }
    .checked()
}

impl Answers {
    /// The refusals that hold however the answers arrived: padding trimmed,
    /// an empty DSN, and a literal DSN of another engine than the one chosen.
    /// An environment reference is left alone — its value is not known here.
    fn checked(mut self) -> Result<Self> {
        self.api_url = self
            .api_url
            .map(|url| url.trim().to_string())
            .filter(|url| !url.is_empty());
        if let Some(dsn) = self.dsn.as_mut() {
            *dsn = dsn.trim().to_string();
            if dsn.is_empty() {
                bail!("the DSN is empty");
            }
            let schemes: &[&str] = match self.db {
                Db::Postgres => &["postgres", "postgresql"],
                Db::Mysql | Db::Mariadb => &["mysql", "mariadb"],
                Db::None => &[],
            };
            if let Some((scheme, _)) = dsn.split_once("://")
                && !scheme.contains('$')
                && !schemes.contains(&scheme)
            {
                bail!(
                    "the DSN starts with {scheme}://, which is not a {} DSN",
                    self.db.name()
                );
            }
        }
        Ok(self)
    }
}

impl Db {
    fn name(self) -> &'static str {
        match self {
            Db::None => "no",
            Db::Postgres => "PostgreSQL",
            Db::Mysql => "MySQL",
            Db::Mariadb => "MariaDB",
        }
    }
}

/// A cancelled prompt (Esc on a list; an interrupted read, where `console`
/// reports Ctrl-C that way instead of raising SIGINT) is `None`; any other
/// failure is an error.
fn answered<T>(result: dialoguer::Result<Option<T>>) -> Result<Option<T>> {
    match result {
        Ok(answer) => Ok(answer),
        Err(dialoguer::Error::IO(error)) if error.kind() == std::io::ErrorKind::Interrupted => {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

/// A `Select` built without `.default()`: nothing is highlighted, so Enter
/// does nothing until the person has moved.
fn select(term: &Term, prompt: &str, items: &[&str]) -> Result<Option<usize>> {
    answered(
        Select::new()
            .with_prompt(prompt)
            .items(items)
            .interact_on_opt(term),
    )
}

fn text(term: &Term, prompt: &str, initial: &str, allow_empty: bool) -> Result<Option<String>> {
    answered(
        Input::<String>::new()
            .with_prompt(prompt)
            .with_initial_text(initial)
            .allow_empty(allow_empty)
            .interact_text_on(term)
            .map(Some),
    )
}

/// The questions the flags did not answer. `None`: Esc or Ctrl-C.
async fn ask(args: &InitArgs) -> Result<Option<Answers>> {
    check_flags(args)?;
    let term = Term::stderr();

    let api_url = if args.no_api || args.api_url.is_some() {
        args.api_url.clone()
    } else {
        let Some(url) = text(
            &term,
            "Base URL of the API under test (empty: none)",
            "http://localhost:8080",
            true,
        )?
        else {
            return Ok(None);
        };
        Some(url.trim().to_string()).filter(|url| !url.is_empty())
    };

    // `--db` answers the engine only; the DSN is asked unless `--dsn` gave it.
    let db = match args.db {
        Some(db) => db,
        None => {
            let items = ["none", "PostgreSQL", "MySQL", "MariaDB"];
            let Some(engine) = select(&term, "A database?", &items)? else {
                return Ok(None);
            };
            [Db::None, Db::Postgres, Db::Mysql, Db::Mariadb][engine]
        }
    };
    let dsn = match (db, &args.dsn) {
        (Db::None, _) => None,
        (_, Some(dsn)) => Some(dsn.clone()),
        (_, None) => {
            let prompt = "DSN (an environment reference keeps the password out of the file)";
            let Some(dsn) = text(&term, prompt, "${BDDKIT_DB_DSN}", false)? else {
                return Ok(None);
            };
            Some(dsn)
        }
    };

    let plugins = if args.plugins.is_empty() {
        let Some(chosen) = ask_plugins(&term).await? else {
            return Ok(None);
        };
        chosen
    } else {
        args.plugins.clone()
    };
    if args.layer.is_some() && plugins.is_empty() {
        bail!("--layer needs at least one plugin");
    }

    let layer = match args.layer {
        Some(layer) => layer,
        // Under `--bddkit-dir` there is one directory and nothing to choose.
        None if plugins.is_empty()
            || dirs::Env::from_process(args.dir.bddkit_dir.clone())
                .override_dir
                .is_some() =>
        {
            Layer::User
        }
        None => {
            let items = [
                "for me on this machine (the user layer)",
                "for everyone with this repository (.bddkit/ in the project)",
            ];
            match select(&term, "Where should the plugins go?", &items)? {
                Some(0) => Layer::User,
                Some(_) => Layer::Project,
                None => return Ok(None),
            }
        }
    };

    let parallel = match args.parallel {
        Some(parallel) => parallel,
        None => {
            eprintln!(
                "Files run at the same time (concurrency 8) only when their data cannot collide: use <<unique()>> for it."
            );
            let items = ["yes (concurrency 8)", "no (concurrency 1)"];
            match select(
                &term,
                "Can scenarios of different files run at the same time?",
                &items,
            )? {
                Some(0) => true,
                Some(_) => false,
                None => return Ok(None),
            }
        }
    };

    let telemetry = match args.telemetry {
        Some(answer) => Some(answer),
        None => {
            eprintln!("{TELEMETRY_TEXT}");
            match select(&term, "Send anonymous usage counts?", &["Yes", "No"])? {
                Some(choice) => Some(choice == 0),
                None => return Ok(None),
            }
        }
    };

    Answers {
        api_url,
        db,
        dsn,
        plugins,
        layer,
        parallel,
        telemetry,
    }
    .checked()
    .map(Some)
}

/// Question 3: the official index as a checklist with nothing ticked. An index
/// that cannot be fetched skips the question with one line.
async fn ask_plugins(term: &Term) -> Result<Option<Vec<String>>> {
    let sources = plugin::install::Sources::from_env();
    eprintln!("Fetching the plugin index…");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let index = plugin::install::fetch_index(&client, &sources).await;
    let index = match index {
        Ok(index) if !index.is_empty() => index,
        Ok(_) => return Ok(Some(Vec::new())),
        Err(error) => {
            eprintln!(
                "The plugin index could not be fetched ({error:#}); install plugins later with `bddkit plugin install <name>`."
            );
            return Ok(Some(Vec::new()));
        }
    };
    let items: Vec<String> = index
        .iter()
        .map(|entry| format!("{} — {}", entry.name, entry.description))
        .collect();
    let chosen = answered(
        MultiSelect::new()
            .with_prompt("Plugins (arrows move, Space ticks, Enter accepts)")
            .items(&items)
            .interact_on_opt(term),
    )?;
    Ok(chosen.map(|chosen| chosen.into_iter().map(|i| index[i].name.clone()).collect()))
}

/// The config's text is `$`-expanded before it is parsed, so a `$` the person
/// typed as part of a password or a URL is written `$$`; `${NAME}` is the one
/// form kept, because it is what the prompt suggests.
fn literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        out.push_str(if rest[at + 1..].starts_with('{') {
            "$"
        } else {
            "$$"
        });
        rest = &rest[at + 1..];
    }
    out + rest
}

/// The config as text: one `key: value` block per top-level key, each under
/// a comment saying what it is.
fn config_text(answers: &Answers) -> Result<String> {
    use serde_json::{Map, Value, json};
    let mut out = String::from("# Written by `bddkit init`. The full reference: README.md.\n");
    let mut block = |comment: &str, key: &str, value: Value| -> Result<()> {
        out.push('\n');
        for line in comment.lines() {
            out.push_str(&format!("# {line}\n"));
        }
        let one = Map::from_iter([(key.to_string(), value)]);
        out.push_str(&serde_yaml_ng::to_string(&one)?);
        Ok(())
    };
    block(
        "Where the feature files are, resolved against the directory bddkit runs in.",
        "paths",
        json!([FEATURES_DIR]),
    )?;
    if !answers.parallel {
        block(
            "Files run one at a time. Raise it once every scenario brings its own data (<<unique()>>).",
            "concurrency",
            json!(1),
        )?;
    }
    // `api` is a required key of `resources`, empty when the suite has none.
    let mut resources = json!({"api": {}});
    if let Some(url) = &answers.api_url {
        resources["api"]["main"] = json!({"base_url": literal(url)});
    }
    if let Some(dsn) = &answers.dsn {
        resources["db"] = json!({"main": {"dsn": literal(dsn)}});
    }
    block(
        "The APIs and databases scenarios reach by name; `bddkit resource fields` lists what each takes.",
        "resources",
        resources,
    )?;
    if let Some(telemetry) = answers.telemetry {
        block(
            "Anonymous usage counts: the team's decision, followed by its CI.",
            "telemetry",
            json!(telemetry),
        )?;
    }
    Ok(out)
}

fn example_feature(answers: &Answers) -> String {
    let mut out = String::from(
        "Feature: Example

  Scenario: variables carry values from one step to the next
    Given set variable \"greeting\" to \"hello\"
    Then variable \"greeting\" should be equal to \"hello\"
",
    );
    if answers.api_url.is_some() {
        out.push_str(
            "    # A step in a comment is skipped, and the report shows it as disabled.
    # Remove the # once the API answers:
    # When I request \"/\"
    # Then the response code is 200
",
        );
    }
    out
}

/// A temporary name in the same directory, then a rename: a failure leaves no
/// half-written file.
fn write_new(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("bddkit-tmp");
    let written = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.with_context(|| format!("write {}", path.display()))
}

/// Inside a git work tree: `.git` is a directory, or a file in a linked
/// worktree or submodule.
fn in_git_work_tree() -> bool {
    std::env::current_dir().is_ok_and(|cwd| cwd.ancestors().any(|dir| dir.join(".git").exists()))
}

/// Appends the lines that are missing, nothing else; creates the file.
fn update_gitignore(path: &Path) -> Result<Vec<&'static str>> {
    // Only a missing file is empty: any other read error (a non-UTF-8 file
    // included) must leave the person's `.gitignore` as it is.
    let existing = match std::fs::read_to_string(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        read => read.with_context(|| format!("read {}", path.display()))?,
    };
    let missing: Vec<&str> = GITIGNORE_LINES
        .into_iter()
        .filter(|line| !existing.lines().any(|have| have.trim() == *line))
        .collect();
    if !missing.is_empty() {
        // Append only: nothing the person wrote can be lost half-way.
        let mut text = String::new();
        if !existing.is_empty() && !existing.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&missing.join("\n"));
        text.push('\n');
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, text.as_bytes()))
            .with_context(|| format!("write {}", path.display()))?;
    }
    Ok(missing)
}

/// The `resource add` command that would declare a plugin group, for a group
/// with required fields and no instance of its own.
fn hints(config_path: &Path, env: &dirs::Env) -> Result<Vec<String>> {
    let cfg = config::load(config_path, None)?;
    let Some(plugins) = crate::load_plugins(config_path, &cfg, env)? else {
        return Ok(Vec::new());
    };
    let flag = if config_path == Path::new("bddkit.yaml") {
        String::new()
    } else {
        format!(" --config {}", config_path.display())
    };
    Ok(resource::plugin_kinds(&plugins)
        .into_iter()
        .filter(|kind| !plugins.has_instance(&kind.kind))
        .filter_map(|kind| {
            let required: Vec<String> = kind
                .fields?
                .into_iter()
                .filter(|field| field.required)
                .map(|field| format!("--{0} <{0}>", field.name))
                .collect();
            (!required.is_empty()).then(|| {
                format!(
                    "bddkit resource add {} main{flag} {}",
                    kind.kind,
                    required.join(" ")
                )
            })
        })
        .collect())
}

fn write_example(answers: &Answers) -> Result<()> {
    let example = Path::new(FEATURES_DIR).join("example.feature");
    if example.exists() {
        println!("kept {}", example.display());
    } else {
        std::fs::create_dir_all(FEATURES_DIR)?;
        write_new(&example, &example_feature(answers))?;
        println!("wrote {}", example.display());
    }
    Ok(())
}

fn ignore_local_files() -> Result<()> {
    if in_git_work_tree() {
        let added = update_gitignore(Path::new(".gitignore"))?;
        if !added.is_empty() {
            println!(".gitignore gained {}", added.join(", "));
        }
    }
    Ok(())
}

/// Writes, installs and checks. 0 when everything is in place and `doctor` is
/// clean, 1 when a plugin failed to install or `doctor` found something.
pub async fn apply(
    answers: &Answers,
    config_path: &Path,
    explicit: Option<&Path>,
    env: dirs::Env,
) -> Result<i32> {
    // A questionnaire can take minutes: look again just before writing.
    refuse_existing(config_path)?;
    write_new(config_path, &config_text(answers)?)?;
    println!("wrote {}", config_path.display());

    // The config is on disk: from here a failure is a finding (exit 1) and
    // the rest still runs, never a refusal.
    let mut code = 0;
    let mut report = |error: anyhow::Error| {
        eprintln!(
            "error: {}",
            crate::privacy::mask_text(&format!("{error:#}"))
        );
        code = 1;
    };
    if let Err(error) = write_example(answers) {
        report(error);
    }
    if let Err(error) = ignore_local_files() {
        report(error);
    }
    if !answers.plugins.is_empty() {
        match plugin::manage::Context::new(env.clone(), crate::config_dir(config_path)) {
            Ok(ctx) => {
                // Never the auto-pick: with `/etc/bddkit` and no user directory
                // it would install machine-wide. One `--bddkit-dir` has no layers.
                let layer = match answers.layer {
                    Layer::Project => Some("project"),
                    Layer::User => env.override_dir.is_none().then_some("user"),
                };
                let mut installed = false;
                for name in &answers.plugins {
                    match ctx.install(name, layer).await {
                        Ok(_) => installed = true,
                        Err(error) => report(error.context(name.clone())),
                    }
                }
                if installed {
                    match hints(config_path, &env) {
                        Ok(lines) => lines.iter().for_each(|line| println!("next: {line}")),
                        Err(error) => eprintln!(
                            "note: plugin fields not listed: {}",
                            crate::privacy::mask_text(&format!("{error:#}"))
                        ),
                    }
                }
            }
            Err(error) => report(error),
        }
    }

    let checked = doctor::check(explicit, None, false, &env, &[], None, &Default::default()).await;
    println!();
    print!("{}", checked.render());
    Ok(code.max(checked.exit_code()))
}

/// `init` never overwrites a suite's config, and never makes the pair
/// `bddkit.yaml` + `bddkit.yml` that discovery refuses.
fn refuse_existing(path: &Path) -> Result<()> {
    let sibling = (path.file_stem().is_some_and(|stem| stem == "bddkit"))
        .then(|| [path.with_extension("yaml"), path.with_extension("yml")])
        .into_iter()
        .flatten()
        .find(|other| other.exists());
    match path.exists().then(|| path.to_path_buf()).or(sibling) {
        Some(found) => bail!(
            "{} already exists, and init never overwrites a suite's config; to extend the suite use `bddkit resource add` or `bddkit plugin install`",
            found.display()
        ),
        None => Ok(()),
    }
}

pub async fn command(args: InitArgs) -> Result<i32> {
    let config_path = match config::resolve_config_path(args.config.as_deref())? {
        Some(resolved) => resolved.path,
        None => PathBuf::from("bddkit.yaml"),
    };
    refuse_existing(&config_path)?;
    let answers = if report::progress::can_prompt() {
        exit_on_ctrl_c();
        match ask(&args).await? {
            Some(answers) => answers,
            None => cancel(),
        }
    } else {
        from_flags(&args)?
    };
    WRITING.store(true, Ordering::SeqCst);
    apply(
        &answers,
        &config_path,
        args.config.as_deref(),
        dirs::Env::from_process(args.dir.bddkit_dir),
    )
    .await
}

static WRITING: AtomicBool = AtomicBool::new(false);
static CANCELLED: AtomicBool = AtomicBool::new(false);

/// Leaves with 130, the cursor visible and one line saying so, whichever of
/// the two ways a Ctrl-C arrives (the signal handler, or the prompt's own
/// interrupted read) gets here first.
fn cancel() -> ! {
    let _ = Term::stderr().show_cursor();
    if !CANCELLED.swap(true, Ordering::SeqCst) {
        eprintln!(
            "{}",
            if WRITING.load(Ordering::SeqCst) {
                "interrupted"
            } else {
                "nothing written"
            }
        );
    }
    std::process::exit(130)
}

/// `console` hides the cursor for a prompt and, on Ctrl-C, raises SIGINT
/// without showing it again: the process would die with the terminal left
/// that way. Hold the signal here and leave through `cancel`.
fn exit_on_ctrl_c() {
    tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel();
        }
    });
}
