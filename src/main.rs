/// `eprintln!` for the host's own debug output: masked with the file's set, as
/// the failure text is. Textually first, so every module below sees it.
macro_rules! debug_eprintln {
    ($world:expr, $($arg:tt)*) => {
        eprintln!("{}", $world.redactor.mask(&format!($($arg)*)))
    };
}

mod annotate;
mod artifacts;
mod config;
mod db;
mod dirs;
mod doctor;
mod events;
mod failure;
mod feature;
mod hawk;
mod http;
mod include;
mod init;
mod json;
mod macros;
mod options;
mod plugin;
mod polling;
mod privacy;
mod report;
mod resource;
mod runner;
mod srp;
mod steps;
mod telemetry;
mod unique;
mod upgrade;
mod validate;
mod vars;
mod world;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;

/// What `--version` and `bddkit version` answer. `-V` keeps the bare
/// `version` — the one line a script greps — so the two signposts below are
/// the long form only. clap prefixes both with the binary name.
const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\nGherkin acceptance testing for backend services.\n",
    "\n  bddkit steps list                    every step this binary understands",
    "\n  bddkit doctor --config suite.yaml    check a suite before it runs",
);

#[derive(Parser)]
#[command(
    name = "bddkit",
    version,
    long_version = LONG_VERSION,
    about = "Run Gherkin scenarios against an HTTP API"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the feature files the config selects
    Run(RunArgs),
    /// Show the steps this binary understands
    Steps(StepsArgs),
    /// Set up a new suite: answer a few questions, or pass flags
    Init(init::InitArgs),
    /// Check a suite's config, and with --live probe what it talks to
    Doctor(DoctorArgs),
    /// Render an event stream (`run --events`) as a report
    Report(RenderArgs),
    /// Show what a resource's config takes
    Resource(ResourceArgs),
    /// Find, install, update and remove plugins
    Plugin(PluginArgs),
    /// Replace bddkit and its plugins with another release, with a way back
    Upgrade(UpgradeArgs),
    /// Anonymous usage counts: what is decided, what was recorded, turn them on or off
    Telemetry(TelemetryCommandArgs),
    /// Print the version, and where to look next
    Version {
        /// Machine-readable: version, target and plugin ABI
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
#[command(after_help = "Examples:
  bddkit report run.ndjson                 run.html beside the stream
  bddkit report run.ndjson -o out/r.html   a named file
  bddkit report run.ndjson --embed-artifacts   one file that opens anywhere
  bddkit report run.ndjson -o report.md    Markdown for an agent: failures only
  bddkit report run.ndjson -o report.md --detail all   every scenario in full")]
struct RenderArgs {
    /// The stream a run wrote with --events
    events: PathBuf,
    /// Where to write the report; the format follows the extension (.html, .htm, .md or .markdown). Repeatable [default: the stream's name with .html]
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    outputs: Vec<PathBuf>,
    /// What a Markdown report shows in full: failed scenarios, or every scenario [default: failed]
    #[arg(long, value_enum)]
    detail: Option<report::markdown::Detail>,
    /// Where the run's artifacts are, when they are not where `run_started` says
    #[arg(long, value_name = "DIR")]
    artifacts: Option<PathBuf>,
    /// Carry the files the report links to inside the page, so it can be mailed
    #[arg(long)]
    embed_artifacts: bool,
    /// The most on-disk bytes to embed, smallest files first: a whole number and B, KB, MB or GB [default: 20MB]
    #[arg(long, value_name = "SIZE", value_parser = report::embed::parse_size, requires = "embed_artifacts")]
    embed_limit: Option<u64>,
    /// What to do to a PNG on its way in; the file on disk is never touched [default: lossless]
    #[arg(long, value_enum, requires = "embed_artifacts")]
    embed_images: Option<report::embed::Images>,
    /// WebP quality for --embed-images lossy [default: 80]
    #[arg(long, value_name = "1-100", value_parser = clap::value_parser!(u8).range(1..=100), requires = "embed_artifacts")]
    embed_quality: Option<u8>,
}

#[derive(Args)]
#[command(after_help = "Examples:
  bddkit upgrade --check          what would change, changing nothing
  bddkit upgrade                  to the latest release
  bddkit upgrade --version 0.4.1  to a given release, older ones included
  bddkit upgrade --rollback       back to the version the last upgrade replaced")]
struct UpgradeArgs {
    /// Show the plan, change nothing
    #[arg(long, conflicts_with = "rollback")]
    check: bool,
    /// Upgrade to this release instead of the latest
    #[arg(long = "version", value_name = "VERSION", conflicts_with = "rollback")]
    target_version: Option<String>,
    /// Restore the binary and lock files the last upgrade replaced
    #[arg(long)]
    rollback: bool,
    /// Upgrade even when a plugin has no release for the new ABI, leaving it as it is
    #[arg(long)]
    force: bool,
    /// Do not ask: required where there is no terminal
    #[arg(long, short = 'y')]
    yes: bool,
    /// Only used to find the project's `.bddkit/` [default: $BDDKIT_CONFIG, else ./bddkit.yaml or ./bddkit.yml]
    #[arg(long)]
    config: Option<PathBuf>,
    /// Use this directory only, skipping the shared, user and project layers
    /// (overrides $BDDKIT_DIR)
    #[arg(long = "bddkit-dir")]
    bddkit_dir: Option<PathBuf>,
}

const LAYERS: [&str; 6] = [
    "shared",
    "shared.local",
    "user",
    "user.local",
    "project",
    "project.local",
];

#[derive(Args)]
#[command(after_help = "Examples:
  bddkit plugin list                        every plugin in the index
  bddkit plugin list mail                   only those whose name or description matches
  bddkit plugin install exec                the latest release, into the user layer
  bddkit plugin install exec@0.1.0 --layer project.local
  bddkit plugin install someone/bddkit-widget
  bddkit plugin update --dry-run            what would move, without moving it
  bddkit plugin show exec                   one plugin in detail")]
struct PluginArgs {
    #[command(subcommand)]
    command: Option<PluginCommand>,
    /// Only used to find the project's `.bddkit/` [default: $BDDKIT_CONFIG, else ./bddkit.yaml or ./bddkit.yml]
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Use this directory only, skipping the shared, user and project layers
    /// (overrides $BDDKIT_DIR)
    #[arg(long = "bddkit-dir", global = true)]
    bddkit_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum PluginCommand {
    /// Browse the plugin index
    List {
        /// Case-insensitive substring of a name or description
        query: Option<String>,
    },
    /// Download a plugin release and register it in a lock file
    Install {
        /// An index name or owner/repo, optionally @<version>
        plugin: String,
        /// The lock file to write [default: user, or shared when only that directory exists]
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(LAYERS))]
        layer: Option<String>,
    },
    /// Move installed plugins to their latest release
    Update {
        /// Only these plugins [default: every one installed by bddkit]
        names: Vec<String>,
        /// Only this lock file
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(LAYERS))]
        layer: Option<String>,
        /// Show what would change, write nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Unregister a plugin and delete the files install put there
    Remove {
        name: String,
        /// The lock file to remove it from [default: the only one declaring it]
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(LAYERS))]
        layer: Option<String>,
    },
    /// Delete plugin versions that no lock file points into any more
    Prune {
        /// List what would be deleted and its size, delete nothing
        #[arg(long)]
        dry_run: bool,
        /// Delete without asking (required when there is no terminal)
        #[arg(long)]
        yes: bool,
    },
    /// Show installed plugins, or one plugin in detail
    Show { name: Option<String> },
    /// Load a library and print its plugin name (what `upgrade` asks the new binary)
    #[command(hide = true)]
    Check { path: PathBuf },
}

/// Every failure here is exit 1: nothing ran, but there is no run whose
/// start a 2 would date — the same currency as `resource add`.
async fn plugin_command(args: PluginArgs) -> Result<i32> {
    let Some(command) = args.command else {
        use clap::CommandFactory;
        let mut cli = Cli::command();
        cli.build();
        cli.find_subcommand_mut("plugin")
            .expect("the plugin subcommand is declared")
            .print_help()?;
        println!();
        return Ok(0);
    };
    let started = std::time::Instant::now();
    let telemetry_command = match &command {
        PluginCommand::Install { .. } => Some(telemetry::payload::Command::PluginInstall),
        PluginCommand::Update { dry_run: false, .. } => {
            Some(telemetry::payload::Command::PluginUpdate)
        }
        PluginCommand::Remove { .. } => Some(telemetry::payload::Command::PluginRemove),
        _ => None,
    };
    let setting =
        telemetry::command::Setting::discover(args.config.as_deref(), args.bddkit_dir.clone()).ok();
    let acted = std::sync::Mutex::new(Vec::new());
    let outcome: Result<i32> = async {
        if let PluginCommand::Check { path } = &command {
            // `ManuallyDrop` for the reason `Library::load` gives: never unloaded.
            let lib = std::mem::ManuallyDrop::new(plugin::library::Library::load(
                &path.file_name().unwrap_or_default().to_string_lossy(),
                path,
            )?);
            println!("{}", lib.manifest.name);
            return Ok(0);
        }
        // Located, never parsed: only the project layer's anchor is needed.
        let config_dir = match config::resolve_config_path(args.config.as_deref())? {
            Some(resolved) => config_dir(&resolved.path).to_path_buf(),
            None => PathBuf::from("."),
        };
        let ctx =
            plugin::manage::Context::new(dirs::Env::from_process(args.bddkit_dir), &config_dir)?;
        let outcome = match command {
            PluginCommand::List { query } => ctx.list(query.as_deref()).await,
            PluginCommand::Install { plugin, layer } => {
                ctx.install(&plugin, layer.as_deref()).await
            }
            PluginCommand::Update {
                names,
                layer,
                dry_run,
            } => ctx.update(&names, layer.as_deref(), dry_run).await,
            PluginCommand::Remove { name, layer } => ctx.remove(&name, layer.as_deref()).await,
            PluginCommand::Prune { dry_run, yes } => ctx.prune(dry_run, yes),
            PluginCommand::Show { name } => ctx.show(name.as_deref()).await,
            PluginCommand::Check { .. } => unreachable!("answered before the layers are read"),
        };
        *acted.lock().expect("acted is never poisoned") = ctx.take_acted();
        outcome
    }
    .await;
    let code = outcome.unwrap_or_else(|error| {
        eprintln!("error: {}", privacy::mask_text(&format!("{error:#}")));
        1
    });
    if let Some(command) = telemetry_command {
        let mut payload = telemetry::payload::Payload::new(command, code, started.elapsed());
        payload.plugin = Some(acted.into_inner().expect("acted is never poisoned"));
        telemetry::command::record_if_on(setting.as_ref(), &payload).await;
    }
    Ok(code)
}

#[derive(Args)]
#[command(after_help = "Examples:
  bddkit telemetry status               on or off, and what decided
  bddkit telemetry show                 the last payload recorded on this machine
  bddkit telemetry off                  for this user, on this machine
  bddkit telemetry on --layer shared    for every user of this machine (/etc/bddkit)")]
struct TelemetryCommandArgs {
    #[command(subcommand)]
    command: TelemetryCommand,
    /// Only used to find the project's `.bddkit/` and the suite's `telemetry` key [default: $BDDKIT_CONFIG, else ./bddkit.yaml or ./bddkit.yml]
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Use this directory only, skipping the shared, user and project layers
    /// (overrides $BDDKIT_DIR)
    #[arg(long = "bddkit-dir", global = true)]
    bddkit_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum TelemetryCommand {
    /// Whether it is on, and which source decided
    Status,
    /// Print the last payload recorded on this machine
    Show,
    /// Allow anonymous usage counts
    On {
        /// The layer to write [default: user]
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(LAYERS))]
        layer: Option<String>,
    },
    /// Refuse anonymous usage counts
    Off {
        /// The layer to write [default: user]
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(LAYERS))]
        layer: Option<String>,
    },
}

/// Exit 1 on any failure, like `plugin`: nothing ran that a 2 would date.
fn telemetry_command(args: TelemetryCommandArgs) -> Result<i32> {
    let outcome: Result<String> = (|| {
        let setting =
            telemetry::command::Setting::discover(args.config.as_deref(), args.bddkit_dir)?;
        match args.command {
            TelemetryCommand::Status => Ok(telemetry::command::status(&setting)),
            TelemetryCommand::Show => telemetry::command::show(&setting),
            TelemetryCommand::On { layer } => {
                telemetry::command::set(&setting, true, layer.as_deref())
            }
            TelemetryCommand::Off { layer } => {
                telemetry::command::set(&setting, false, layer.as_deref())
            }
        }
    })();
    Ok(match outcome {
        Ok(text) => {
            print!("{text}");
            0
        }
        Err(error) => {
            eprintln!("error: {}", privacy::mask_text(&format!("{error:#}")));
            1
        }
    })
}

/// Every failure is exit 1, like `plugin`: nothing ran that a 2 would date.
async fn upgrade_command(args: UpgradeArgs) -> Result<i32> {
    let started = std::time::Instant::now();
    let setting =
        telemetry::command::Setting::discover(args.config.as_deref(), args.bddkit_dir.clone()).ok();
    let mut note = telemetry::payload::UpgradeInfo::default();
    let outcome: Result<i32> = async {
        let config_dir = match config::resolve_config_path(args.config.as_deref())? {
            Some(resolved) => config_dir(&resolved.path).to_path_buf(),
            None => PathBuf::from("."),
        };
        let ctx =
            plugin::manage::Context::new(dirs::Env::from_process(args.bddkit_dir), &config_dir)?;
        upgrade::run(
            &ctx,
            &upgrade::Options {
                check: args.check,
                version: args.target_version,
                rollback: args.rollback,
                force: args.force,
                yes: args.yes,
            },
            &mut note,
        )
        .await
    }
    .await;
    let code = outcome.unwrap_or_else(|error| {
        eprintln!("error: {}", privacy::mask_text(&format!("{error:#}")));
        1
    });
    let mut payload = telemetry::payload::Payload::new(
        telemetry::payload::Command::Upgrade,
        code,
        started.elapsed(),
    );
    payload.upgrade = Some(note);
    telemetry::command::record_if_on(setting.as_ref(), &payload).await;
    Ok(code)
}

#[derive(Args)]
#[command(after_help = "Examples:
  bddkit resource fields                      every resource kind bddkit serves itself
  bddkit resource fields db                   only the database connection keys
  bddkit resource fields --config suite.yaml  also the groups this suite's plugins serve
  bddkit resource fields --json               the same listing, machine-readable
  bddkit resource add api staging --config suite.yaml --base_url http://staging.local
  bddkit resource add db reporting --config suite.yaml --no-check --dsn postgres://...
  # --config/--env/--json/--no-check must come before any --<field> value")]
struct ResourceArgs {
    #[command(subcommand)]
    command: Option<ResourceCommand>,
}

#[derive(Subcommand)]
enum ResourceCommand {
    /// List the keys each resource kind's config takes
    Fields(FieldsArgs),
    /// Write a validated resource into the config
    Add(AddArgs),
}

#[derive(Args)]
struct AddArgs {
    /// api, db, srp, or a plugin group
    group: String,
    /// The name the resource is reachable by
    name: String,
    /// Path to the YAML config to edit [default: $BDDKIT_CONFIG, else ./bddkit.yaml or ./bddkit.yml]
    #[arg(long)]
    config: Option<PathBuf>,
    /// Override APP_ENV: selects .env.<name> / .env.<name>.local
    #[arg(long = "env")]
    env: Option<String>,
    /// The whole body as JSON; flags override it, key by key
    #[arg(long)]
    json: Option<String>,
    /// Skip the live probe. The shape is validated either way
    #[arg(long = "no-check")]
    no_check: bool,
    /// --<field> <value> pairs, one per key of the resource
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    fields: Vec<String>,
}

#[derive(Args)]
struct FieldsArgs {
    /// Only this kind: api, db, srp, or a plugin group
    kind: Option<String>,
    /// Also describe the groups the plugins this config loads serve [default: $BDDKIT_CONFIG, else ./bddkit.yaml or ./bddkit.yml]
    #[arg(long)]
    config: Option<PathBuf>,
    /// Builtins only, even when a config would otherwise be picked up
    #[arg(long, conflicts_with = "config")]
    no_config: bool,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
}

/// Bare `bddkit resource` is a signpost, exactly as bare `bddkit steps` is.
async fn resource_command(args: ResourceArgs) -> Result<i32> {
    match args.command {
        Some(ResourceCommand::Fields(args)) => list_fields(args),
        Some(ResourceCommand::Add(mut args)) => {
            // ponytail: `trailing_var_arg` starts swallowing every remaining
            // token, known flags included, the moment it meets the first one
            // clap does not recognize — and a resource field like
            // `--base_url` is never in `AddArgs`'s own flag set, so a
            // `--no-check` typed after it lands in `fields`, not in
            // `args.no_check`. Recovering it here (rather than teaching
            // `resource::add` about clap's parsing order) keeps the field
            // parser answering only "what fields did the user set".
            if let Some(pos) = args.fields.iter().position(|f| f == "--no-check") {
                args.fields.remove(pos);
                args.no_check = true;
            }
            let config_path = match config::resolve_config_path(args.config.as_deref()) {
                Ok(Some(resolved)) => resolved.path,
                Ok(None) => {
                    println!("{}\n\nnothing was written.", config::NO_CONFIG_FOUND);
                    return Ok(1);
                }
                Err(error) => {
                    println!(
                        "{}\n\nnothing was written.",
                        privacy::mask_text(&format!("{error:#}"))
                    );
                    return Ok(1);
                }
            };
            resource::add(resource::AddInput {
                group: &args.group,
                name: &args.name,
                config: &config_path,
                env: args.env.as_deref(),
                json: args.json.as_deref(),
                no_check: args.no_check,
                flags: &args.fields,
            })
            .await
        }
        None => {
            use clap::CommandFactory;
            let mut cli = Cli::command();
            cli.build();
            cli.find_subcommand_mut("resource")
                .expect("the resource subcommand is declared")
                .print_help()?;
            println!();
            Ok(0)
        }
    }
}

fn list_fields(args: FieldsArgs) -> Result<i32> {
    let mut kinds = resource::host_kinds();

    // A plugin's field list lives inside its `cdylib`, so reading it means
    // loading the plugin — which is why `--config` is optional here, exactly
    // as it is for `steps list`: the common question, "what does an api entry
    // take", must cost nothing. It is also the only way to reach the lock
    // file, which is anchored at the config's parent directory. `--no-config`
    // is the same escape hatch `steps list` has.
    if !args.no_config
        && let Some(resolved) = config::resolve_config_path(args.config.as_deref())?
    {
        let path = &resolved.path;
        let cfg = config::load(path, None)?;
        if let Some(plugins) = load_plugins(path, &cfg, &dirs::Env::from_process(None))? {
            kinds.extend(resource::plugin_kinds(&plugins));
        }
    }

    if let Some(kind) = &args.kind {
        // Checked before filtering, so a typo is named rather than silently
        // producing an empty listing — as `steps list` does with its resource.
        if !kinds.iter().any(|k| &k.kind == kind) {
            anyhow::bail!("no such resource: {kind:?}");
        }
        kinds.retain(|k| &k.kind == kind);
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&kinds)?);
    } else {
        print!("{}", resource::render(&kinds));
    }
    Ok(0)
}

#[derive(Args)]
#[command(after_help = "Examples:
  bddkit doctor --config suite.yaml          every static check, no socket opened
  bddkit doctor --config suite.yaml --live   also probe every API and database
  bddkit doctor --config suite.yaml --json   the same report, machine-readable
  bddkit doctor --config suite.yaml --junit reports/junit.xml
                                             also check that run's report path can be written")]
struct DoctorArgs {
    /// Path to the YAML config [default: $BDDKIT_CONFIG, else ./bddkit.yaml or ./bddkit.yml]
    #[arg(long)]
    config: Option<PathBuf>,
    /// Override APP_ENV: selects .env.<name> / .env.<name>.local
    #[arg(long = "env")]
    env: Option<String>,
    /// Also open a socket to every API and database the config names
    #[arg(long)]
    live: bool,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
    /// Check that a `run --events` path can be written; a path that exists and
    /// is not a regular file (a FIFO, /dev/fd/N) is not opened
    #[arg(long)]
    events: Option<PathBuf>,
    #[command(flatten)]
    dir: DirArgs,
    #[command(flatten)]
    reports: ReportArgs,
    #[command(flatten)]
    privacy: PrivacyArgs,
    #[command(flatten)]
    telemetry: TelemetryArgs,
}

/// Unlike `run`, every outcome here is a report: a config that cannot be
/// parsed is the most ordinary thing `doctor` has to say, not a reason to
/// answer in a different currency. Hence 0/1 and no `?`.
async fn doctor_command(args: DoctorArgs) -> Result<i32> {
    let started = std::time::Instant::now();
    privacy::set_flags(args.privacy.layer());
    let dir_env = dirs::Env::from_process(args.dir.bddkit_dir.clone());
    let report = doctor::check(
        args.config.as_deref(),
        args.env.as_deref(),
        args.live,
        &dir_env,
        &args.reports.paths(),
        args.events.as_deref(),
        &args.privacy.layer(),
        args.telemetry.enabled,
    )
    .await;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render());
    }
    // After the report, like `run`'s question after its summary.
    telemetry::doctor::finish(&report, started.elapsed()).await;
    Ok(report.exit_code())
}

/// The flag layer of the privacy options (spec §7.7); `run` and `doctor` flatten
/// the same struct so they cannot disagree about what a flag means.
#[derive(Args, Default)]
struct PrivacyArgs {
    /// Show real values instead of `***` in everything printed or written
    #[arg(long = "privacy-reveal", visible_alias = "reveal")]
    reveal: bool,
    /// Shortest value, in characters, that is masked wherever it appears [default: 7]
    #[arg(long = "privacy-min-length")]
    min_length: Option<usize>,
    /// Extra words that make a variable name sensitive (comma-separated, repeatable)
    #[arg(long = "privacy-words", value_delimiter = ',', action = clap::ArgAction::Append)]
    words: Option<Vec<String>>,
    /// Replace the words that make a variable name sensitive
    #[arg(long = "privacy-words-only", value_delimiter = ',', action = clap::ArgAction::Append)]
    words_only: Option<Vec<String>>,
    /// Extra field words that make a JSON key, form field or query parameter sensitive
    #[arg(long = "privacy-fields", value_delimiter = ',', action = clap::ArgAction::Append)]
    fields: Option<Vec<String>>,
    /// Replace the field words that make a key, form field or query parameter sensitive
    #[arg(long = "privacy-fields-only", value_delimiter = ',', action = clap::ArgAction::Append)]
    fields_only: Option<Vec<String>>,
}

impl PrivacyArgs {
    fn layer(&self) -> privacy::PrivacyLayer {
        // `--privacy-fields-only ''` arrives as one empty part; the same
        // trimming as a variable's turns it into the empty list it means.
        let list = |list: &Option<Vec<String>>| {
            list.as_ref()
                .map(|parts| privacy::split_list(&parts.join(",")))
        };
        privacy::PrivacyLayer {
            words: list(&self.words),
            words_only: list(&self.words_only),
            fields: list(&self.fields),
            fields_only: list(&self.fields_only),
            min_length: self.min_length,
            reveal: self.reveal.then_some(true),
        }
    }
}

/// A variable that is set and not empty. An empty value is unset: a templated
/// `BDDKIT_EVENTS=` means "off".
fn env_var(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

#[derive(Args)]
struct DirArgs {
    /// Read `.bddkit/` files from this directory only, skipping the shared,
    /// user and project layers (overrides $BDDKIT_DIR)
    #[arg(long = "bddkit-dir")]
    bddkit_dir: Option<PathBuf>,
}

#[derive(Args)]
struct RunArgs {
    /// Path to the YAML config [default: $BDDKIT_CONFIG, else ./bddkit.yaml or ./bddkit.yml]
    #[arg(long)]
    config: Option<PathBuf>,
    /// Run only these directories or .feature files instead of the config's `paths`
    paths: Vec<PathBuf>,
    /// Run only scenarios with one of these tags (repeatable)
    #[arg(long = "tag")]
    tags: Vec<String>,
    /// Override APP_ENV: selects .env.<name> / .env.<name>.local
    #[arg(long = "env")]
    env: Option<String>,
    /// Stop dispatching new files after the first failure
    #[arg(long = "fail-fast")]
    fail_fast: bool,
    /// Write one JSON object per line here while the run happens: a file, a
    /// FIFO or /dev/fd/N [default: $BDDKIT_EVENTS]
    #[arg(long)]
    events: Option<PathBuf>,
    /// Add evidence to the event stream: resolved values, HTTP exchanges, SQL
    /// and plugin diagnostics, from these comma-separated sources (written
    /// `--evidence=marked,failures`): `marked`
    /// (steps the scenario marks), `failures` (every step of a failed
    /// scenario), `all` (the default without a value). Off by default because
    /// it puts bodies and query parameters on disk. Requires --events
    /// [default: $BDDKIT_EVIDENCE, else the config's `evidence`]
    // `require_equals`: `--evidence features/a.feature` keeps meaning a bare
    // flag and a path, as it did when the flag took no value.
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "all", value_parser = events::Sources::parse)]
    evidence: Option<events::Sources>,
    /// Root for the evidence files plugins write [default: $BDDKIT_ARTIFACTS_DIR,
    /// else <temp dir>/bddkit-<run id>]
    #[arg(long = "artifacts-dir")]
    artifacts_dir: Option<PathBuf>,
    #[command(flatten)]
    dir: DirArgs,
    #[command(flatten)]
    reports: ReportArgs,
    #[command(flatten)]
    privacy: PrivacyArgs,
    #[command(flatten)]
    telemetry: TelemetryArgs,
}

/// The one flag of the telemetry decision (`telemetry::decide`), shared by
/// the commands that ask the question.
#[derive(Args)]
struct TelemetryArgs {
    /// Allow or refuse anonymous usage counts for this command; beats every other source
    #[arg(long = "telemetry", action = clap::ArgAction::Set, value_name = "BOOL")]
    enabled: Option<bool>,
}

/// A report is a property of the run, not of the suite, so these live on the
/// command line and the config never learns them.
#[derive(Args)]
struct ReportArgs {
    /// Write a JUnit XML report here (one testsuite per feature file)
    #[arg(long)]
    junit: Option<PathBuf>,
    /// Write a Cucumber JSON report here
    #[arg(long = "cucumber-json")]
    cucumber_json: Option<PathBuf>,
}

impl ReportArgs {
    fn paths(&self) -> Vec<&std::path::Path> {
        [&self.junit, &self.cucumber_json]
            .into_iter()
            .flatten()
            .map(PathBuf::as_path)
            .collect()
    }

    fn write(&self, results: &[report::FileResult]) -> Result<()> {
        if let Some(path) = &self.junit {
            report::write_junit(results, path)?;
        }
        if let Some(path) = &self.cucumber_json {
            report::write_cucumber_json(results, path)?;
        }
        Ok(())
    }
}

#[derive(Args)]
#[command(after_help = "Examples:
  bddkit steps list                          every builtin step, grouped by resource
  bddkit steps list db                       only the database steps
  bddkit steps list --filter response -v     narrow, and describe what is left
  bddkit steps list --json                   the same listing, machine-readable
  bddkit steps list --config suite.yaml      also the steps of that suite's plugins")]
struct StepsArgs {
    #[command(subcommand)]
    command: Option<StepsCommand>,
}

#[derive(Subcommand)]
enum StepsCommand {
    /// List the available steps, grouped by resource
    List(ListArgs),
}

#[derive(Args)]
struct ListArgs {
    /// Only this resource: api, db, srp, vars, debug, general, or a plugin group
    resource: Option<String>,
    /// Case-insensitive substring match on the step template
    #[arg(long)]
    filter: Option<String>,
    /// Add a one-line description under each step
    #[arg(short, long)]
    verbose: bool,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
    /// Also list the steps of the plugins this config loads [default: $BDDKIT_CONFIG, else ./bddkit.yaml or ./bddkit.yml]
    #[arg(long)]
    config: Option<PathBuf>,
    /// Builtins only, even when a config would otherwise be picked up
    #[arg(long, conflicts_with = "config")]
    no_config: bool,
    /// Description language (default: $BDDKIT_LANG, else en)
    #[arg(long)]
    lang: Option<String>,
    #[command(flatten)]
    dir: DirArgs,
}

/// Bare `bddkit steps` is a signpost, not an error: it prints what the family
/// can do and how, which is the question someone typing it is asking.
fn steps_command(args: StepsArgs) -> Result<i32> {
    let Some(StepsCommand::List(args)) = args.command else {
        use clap::CommandFactory;
        let mut cli = Cli::command();
        // `build` first: an unbuilt `Command` has not propagated `bin_name` to
        // its subcommands, so the help would print `Usage: steps [COMMAND]` —
        // a line the reader cannot type.
        cli.build();
        cli.find_subcommand_mut("steps")
            .expect("the steps subcommand is declared")
            .print_help()?;
        println!();
        return Ok(0);
    };
    list_steps(args)
}

fn list_steps(args: ListArgs) -> Result<i32> {
    let overlay = steps::help::translations(&steps::help::language(args.lang.as_deref()));
    let mut rows = steps::help::builtin_rows(&overlay);

    // A plugin's vocabulary lives inside its `cdylib`, so listing it means
    // loading it — which is why `--config` is optional here and required by
    // `run`: the common question, "what steps exist", must cost nothing.
    // `--no-config` is the escape hatch back to builtins-only, both for a
    // deliberate lookup and for a broken `bddkit.yaml` that would otherwise
    // block this simple vocabulary lookup.
    if !args.no_config
        && let Some(resolved) = config::resolve_config_path(args.config.as_deref())?
    {
        let path = &resolved.path;
        let cfg = config::load(path, None)?;
        let env = dirs::Env::from_process(args.dir.bddkit_dir.clone());
        if let Some(plugins) = load_plugins(path, &cfg, &env)? {
            rows.extend(steps::help::plugin_rows(
                plugins.described_steps(),
                &plugins.group_names(),
                &overlay,
            ));
        }
    }

    if let Some(resource) = &args.resource {
        // Checked before filtering, so a typo is named rather than silently
        // producing the same empty output an over-narrow filter does.
        if !rows.iter().any(|row| &row.group == resource) {
            anyhow::bail!("no such resource: {resource:?}");
        }
        rows.retain(|row| &row.group == resource);
    }
    if let Some(filter) = &args.filter {
        // `--json` always emits the description, so there it is searchable
        // whether or not `-v` was passed — `-v` has no other effect on JSON.
        let searches_descriptions = args.verbose || args.json;
        rows.retain(|row| steps::help::matches_filter(row, filter, searches_descriptions));
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        print!("{}", steps::help::render(&rows, args.verbose));
    }
    Ok(0)
}

/// Everything that fails before the first request must exit with code 2 (invariant 6):
/// config loading, path traversal, building API resources and DB pools, parsing
/// scheduling tags — this is a "nothing ran" failure, while 1 is reserved for
/// a failed scenario.
#[tokio::main]
async fn main() {
    // Every path that can build a pool runs through here first — `AnyPool`
    // panics rather than erroring if a driver is not installed.
    sqlx::any::install_default_drivers();
    // The default hook prints the payload as it is, and a payload can hold a
    // value (an `unwrap` on a parsed secret). Same text, masked.
    std::panic::set_hook(Box::new(|info| {
        // The backtrace the default hook would print, when RUST_BACKTRACE asks.
        let backtrace = std::backtrace::Backtrace::capture();
        let trace = if backtrace.status() == std::backtrace::BacktraceStatus::Captured {
            format!("\n{backtrace}")
        } else {
            String::new()
        };
        eprintln!("{}", privacy::mask_text(&format!("panic: {info}{trace}")));
    }));
    let cli = Cli::parse();
    // Each command names what did not happen: "run not started" is a lie when
    // the user only asked for a listing.
    let (result, nothing_happened) = match cli.command {
        Command::Run(args) => (run(args).await, "run not started"),
        Command::Steps(args) => (steps_command(args), "nothing listed"),
        Command::Init(args) => (init::command(args).await, "nothing written"),
        Command::Doctor(args) => (doctor_command(args).await, "nothing checked"),
        Command::Report(args) => (report_command(args).await, "nothing written"),
        Command::Resource(args) => (resource_command(args).await, "nothing listed"),
        Command::Plugin(args) => (plugin_command(args).await, "nothing changed"),
        Command::Upgrade(args) => (upgrade_command(args).await, "nothing changed"),
        Command::Telemetry(args) => (telemetry_command(args), "nothing changed"),
        Command::Version { json: true } => {
            let target = plugin::install::host_target().unwrap_or("unknown");
            println!(
                "{}",
                serde_json::json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "target": target,
                    "plugin_abi": plugin::abi::ABI_VERSION,
                })
            );
            (Ok(0), "nothing printed")
        }
        Command::Version { json: false } => {
            use clap::CommandFactory;
            // Rendered, never re-formatted from `LONG_VERSION` by hand: the
            // subcommand and `--version` cannot drift apart if they cannot be
            // written apart.
            print!("{}", Cli::command().render_long_version());
            (Ok(0), "nothing printed")
        }
    };
    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            print_refusal(&error, nothing_happened);
            std::process::exit(2);
        }
    }
}

/// The refusal a command ends in. `run` and `report` print it themselves,
/// before telemetry's request, so the person is not left waiting on the
/// network for an error that is already known.
fn print_refusal(error: &anyhow::Error, nothing_happened: &str) {
    eprintln!(
        "error: {}\n\n{nothing_happened}",
        privacy::mask_text(&format!("{error:#}"))
    );
}

/// Reads the lock file, loads what it names, and resolves each group's
/// default. Every failure here is a "nothing ran" failure: the caller's `?`
/// carries it to `main`, which exits 2.
///
/// `None` means no plugin was installed at all — the path every existing suite
/// takes, and the one that must cost nothing.
fn load_plugins(
    config_path: &std::path::Path,
    cfg: &config::Config,
    env: &dirs::Env,
) -> Result<Option<Arc<plugin::Plugins>>> {
    let groups_in_config: Vec<String> = cfg.group_names().cloned().collect();
    // The same anchor `config::load` uses for the `.env` layers: the lock
    // belongs to the suite, not to whatever directory the run started in.
    let layers = dirs::layers(dirs::Os::current(), env, config_dir(config_path))?;
    let mut plugins = plugin::Plugins::load(
        plugin::lock::load(&dirs::candidates(&layers, "plugins"))?,
        &cfg.plugin_instances,
        &groups_in_config,
        cfg.concurrency,
        &cfg.effective_options,
    )?;
    // Only meaningful once a plugin is loaded. With none there are no resource
    // groups at all, so every top-level `default_*` key is the unknown key
    // `Config` has always tolerated — it has never had `deny_unknown_fields`,
    // and a suite written before plugins existed must keep running unchanged.
    if !plugins.is_empty() {
        cfg.check_group_defaults()?;
    }
    let mut defaults = std::collections::BTreeMap::new();
    for group in &groups_in_config {
        if let Some(name) = cfg.resolve_default_group(group)? {
            defaults.insert(group.clone(), name);
        }
    }
    plugins.add_defaults(defaults);
    if plugins.is_empty() {
        return Ok(None);
    }

    let plugins = Arc::new(plugins);
    // The libraries are deliberately never unloaded. If a plugin registered a
    // thread-local destructor or an `atexit` handler, running it after
    // `dlclose` executes code in an unmapped page; the process is exiting
    // anyway, so leaking one mapping is cheaper than a segfault in someone's
    // CI. Leaking a reference here rather than at the end of the run is what
    // makes that true on every path out of `run` — an early `?`, a
    // `process::exit(2)`, and the happy path alike.
    //
    // This leaks the mapping only. The instances a plugin created are still
    // dropped through FFI by `Plugins::shutdown`, once the worker pool drains.
    std::mem::forget(plugins.clone());
    Ok(Some(plugins))
}

/// `--config cfg.yaml` has the parent `""`, which `std::path::absolute` rejects.
pub(crate) fn config_dir(config_path: &std::path::Path) -> &std::path::Path {
    match config_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => std::path::Path::new("."),
    }
}

/// The one piece of startup `run` and `doctor` genuinely share.
///
/// `with_macros_and_plugins`, not `with_macros` plus a registration loop:
/// macros are validated after everything is registered, so a macro body may
/// name a plugin step. Both callers build it before checking any step, which
/// is what keeps invariant 1 — every step of every selected scenario is
/// matched before the first request.
///
/// Deliberately not a `build_context`: `doctor` needs neither `Apis` nor
/// `RunContext`, and a shared constructor that builds both for a caller that
/// wants neither is how an aggregator grows a builder.
fn build_registry(
    cfg: &config::Config,
    plugins: Option<&Arc<plugin::Plugins>>,
) -> std::result::Result<steps::Registry, String> {
    let plugin_steps = plugins.map(|p| p.steps()).unwrap_or_default();
    let plugin_groups = plugins.map(|p| p.group_names()).unwrap_or_default();
    macros::MacroCatalog::load(&cfg.macro_paths).and_then(|catalog| {
        steps::Registry::with_macros_and_plugins(catalog, &plugin_steps, &plugin_groups)
    })
}

/// The operator's "please stop" signals — Ctrl-C, a CI system's polite kill or
/// a closed terminal on Unix, Ctrl-C or Ctrl-Break on Windows. Registered by
/// `install`, which is synchronous on purpose: until it has run the OS default
/// applies and a signal kills the process outright, with no cleanup and no
/// `run_finished`. A signal that arrives after `install` is queued and comes
/// out of the next `wait`, however late the task that waits gets to run.
#[cfg(unix)]
struct Interrupts {
    sigint: tokio::signal::unix::Signal,
    sigterm: tokio::signal::unix::Signal,
    sighup: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Interrupts {
    fn install() -> Self {
        use tokio::signal::unix::{SignalKind, signal};
        Self {
            sigint: signal(SignalKind::interrupt()).expect("install a SIGINT handler"),
            sigterm: signal(SignalKind::terminate()).expect("install a SIGTERM handler"),
            sighup: signal(SignalKind::hangup()).expect("install a SIGHUP handler"),
        }
    }

    /// The signal's name and the exit code it maps to: 128 plus the signal
    /// number, the shell convention, so a cancelled CI job (SIGTERM, 143) is
    /// not read as an operator's Ctrl-C (SIGINT, 130).
    async fn wait(&mut self) -> (&'static str, i32) {
        tokio::select! {
            _ = self.sigint.recv() => ("SIGINT", 130),
            _ = self.sigterm.recv() => ("SIGTERM", 143),
            _ = self.sighup.recv() => ("SIGHUP", 129),
        }
    }
}

#[cfg(windows)]
struct Interrupts {
    ctrl_c: tokio::signal::windows::CtrlC,
    ctrl_break: tokio::signal::windows::CtrlBreak,
}

#[cfg(windows)]
impl Interrupts {
    fn install() -> Self {
        use tokio::signal::windows::{ctrl_break, ctrl_c};
        Self {
            ctrl_c: ctrl_c().expect("install a Ctrl-C handler"),
            ctrl_break: ctrl_break().expect("install a Ctrl-Break handler"),
        }
    }

    async fn wait(&mut self) -> (&'static str, i32) {
        tokio::select! {
            _ = self.ctrl_c.recv() => ("Ctrl-C", 130),
            _ = self.ctrl_break.recv() => ("Ctrl-Break", 130),
        }
    }
}

/// Runs for the lifetime of the process, spawned alongside `run_all`. On the
/// first interrupt it takes the run's ending (`RunContext::claim_end`), stops
/// it from starting new work and asks every plugin to release what it holds
/// (`Plugins::shutdown` — the same sweep a normal run does after the pool
/// drains), then ends the event stream with the signal and exits with that
/// signal's code. A second interrupt during that tail exits immediately instead
/// of waiting for it — "stop now" as promised in issue #62 — still with the
/// FIRST signal's code, since that is the one that ended the run. If the run
/// finished first, the claim is already gone and the signal is left alone: the
/// run exits with its own code and its own last line. If no interrupt ever
/// arrives, `run` exits normally first and this task is simply dropped.
async fn handle_interrupt(
    ctx: Arc<runner::RunContext>,
    plugins: Option<Arc<plugin::Plugins>>,
    mut interrupts: Interrupts,
) {
    let (name, code) = interrupts.wait().await;
    if !ctx.claim_end() {
        return;
    }
    eprintln!("\ninterrupted by {name}: stopping new work, cleaning up plugins...");
    ctx.force_stop();

    let cleanup = async {
        if let Some(plugins) = plugins {
            // Plugins::shutdown makes blocking FFI calls; off the async
            // thread so a second interrupt can still race it and win.
            let _ = tokio::task::spawn_blocking(move || plugins.shutdown()).await;
        }
        eprintln!("cleanup finished");
        if let Some(events) = ctx.events.clone() {
            // Files still in flight keep emitting; the writer drops whatever
            // arrives after this line. A write error cannot change the code.
            let signal = name.to_uppercase().replace('-', "_");
            let _ = tokio::task::spawn_blocking(move || {
                events.finish(serde_json::json!({"exit": code, "signal": signal}))
            })
            .await;
        }
    };
    tokio::select! {
        () = cleanup => {},
        (second, _) = interrupts.wait() => {
            eprintln!("\nsecond interrupt ({second}): exiting now, without waiting for cleanup");
        }
    }
    std::process::exit(code);
}

/// Runs the render off the async thread and races it against an interrupt:
/// the target is never partial, so all a signal can leave is the temporary
/// files beside the outputs, and those are removed here.
async fn report_command(args: RenderArgs) -> Result<i32> {
    let started = std::time::Instant::now();
    let setting = telemetry::command::Setting::discover(None, None).ok();
    let embed = args
        .embed_artifacts
        .then(|| {
            report::embed::Options::from_flags(
                args.embed_limit,
                args.embed_images,
                args.embed_quality,
            )
        })
        .transpose()?;
    let mut interrupts = Interrupts::install();
    let outputs = report::command::outputs_for(&args.events, &args.outputs);
    let formats = report::command::format_names(&outputs);
    let mut info = telemetry::payload::ReportInfo {
        detail: formats.contains(&"md").then(|| {
            args.detail
                .unwrap_or(report::markdown::Detail::Failed)
                .name()
        }),
        embed: formats.contains(&"html").then_some(match &embed {
            None => "none",
            Some(options) => match options.images {
                report::embed::Images::Original => "original",
                report::embed::Images::Lossless => "lossless",
                report::embed::Images::Lossy => "lossy",
            },
        }),
        formats,
        artifacts: args.artifacts.is_some(),
        result: "error",
        ..Default::default()
    };
    let progress = Arc::new(report::progress::Progress::new(
        report::progress::interactive(),
        1 + usize::from(embed.is_some()),
    ));
    let work = {
        let progress = Arc::clone(&progress);
        tokio::task::spawn_blocking(move || {
            let result = report::command::run(
                &args.events,
                &args.outputs,
                args.artifacts.as_deref(),
                embed.as_ref(),
                args.detail,
                &progress,
                &mut info,
            );
            (result, info)
        })
    };
    tokio::select! {
        done = work => {
            // Every way out, an error included, leaves the line clean.
            progress.clear();
            let (result, mut info) = done.context("the report task failed")?;
            let code = result.as_ref().map_or(2, |code| *code);
            let mut payload = telemetry::payload::Payload::new(
                telemetry::payload::Command::Report,
                code,
                started.elapsed(),
            );
            info.result = if result.is_ok() { "ok" } else { "error" };
            payload.report = Some(info);
            if let Err(error) = &result {
                print_refusal(error, "nothing written");
            }
            telemetry::command::record_if_on(setting.as_ref(), &payload).await;
            if result.is_err() {
                std::process::exit(2);
            }
            result
        },
        (_, code) = interrupts.wait() => {
            report::command::cancel();
            progress.clear();
            report::command::remove_temporaries(&outputs);
            std::process::exit(code);
        }
    }
}

/// What `run` knows once the config is read: from here on a refusal or a
/// finished run has a payload to report. Before it, nothing does.
fn note_config(
    probe: &mut telemetry::RunProbe,
    cli: &RunArgs,
    cfg: &config::Config,
    config_path: &std::path::Path,
    events: bool,
    evidence: events::Sources,
) {
    let env = dirs::Env::from_process(cli.dir.bddkit_dir.clone());
    // A missing override directory fails the run at its plugins; here it only
    // means the layers have nothing to say.
    probe.layers =
        dirs::layers(dirs::Os::current(), &env, config_dir(config_path)).unwrap_or_default();
    probe.decision = Some(telemetry::decide_here(
        cli.telemetry.enabled,
        (&cfg.telemetry).into(),
        &probe.layers,
    ));
    probe.concurrency = cfg.concurrency as u64;
    probe.fail_fast = cli.fail_fast;
    probe.features = telemetry::payload::Features {
        junit: cli.reports.junit.is_some(),
        cucumber_json: cli.reports.cucumber_json.is_some(),
        events,
        evidence: evidence.names(),
        artifacts_dir: cli.artifacts_dir.is_some() || env_var("BDDKIT_ARTIFACTS_DIR").is_some(),
        tags: !cli.tags.is_empty(),
        paths: !cli.paths.is_empty(),
        env: cli.env.is_some(),
    };
    probe.resources = telemetry::run::resources_of(cfg);
    probe.read = true;
}

/// `run_suite`, plus what telemetry makes of how it ended. Nothing in here may
/// change the code: the probe only reads what `run_suite` noted on its way.
async fn run(cli: RunArgs) -> Result<i32> {
    let started = std::time::Instant::now();
    let mut probe = telemetry::RunProbe::default();
    let outcome = run_suite(cli, &mut probe).await;
    let code = outcome.as_ref().map_or(2, |code| *code);
    if let Err(error) = &outcome {
        print_refusal(error, "run not started");
    }
    probe.finish(code, started.elapsed(), outcome.is_ok()).await;
    if outcome.is_err() {
        std::process::exit(2);
    }
    outcome
}

async fn run_suite(cli: RunArgs, probe: &mut telemetry::RunProbe) -> Result<i32> {
    privacy::set_flags(cli.privacy.layer());
    cli.reports
        .paths()
        .into_iter()
        .try_for_each(report::prepare)?;
    // Opened once, here, and held: see `events::open`. Nothing is written to
    // it until the run has started, so every refusal below leaves it empty.
    let events_path = cli
        .events
        .clone()
        .or_else(|| env_var("BDDKIT_EVENTS").map(PathBuf::from));
    let events_file = events_path
        .as_ref()
        .map(|path| events::open(path))
        .transpose()?;
    let evidence = match cli.evidence {
        Some(sources) => Some(("--evidence", sources)),
        None => events::sources_from_env()?.map(|sources| ("$BDDKIT_EVIDENCE", sources)),
    };
    // `$BDDKIT_EVENTS` counts, so this is not clap's `requires`.
    if let (Some((source, _)), None) = (evidence, &events_file) {
        anyhow::bail!("{source} requires --events (or $BDDKIT_EVENTS)");
    }
    let resolved = config::resolve_config_path(cli.config.as_deref())?
        .ok_or_else(|| anyhow::anyhow!("{}", config::NO_CONFIG_FOUND))?;
    let config_path = resolved.path;
    let cfg = config::load(&config_path, cli.env.as_deref())?;
    // The config key is the suite's default: a run without a stream ignores it.
    let evidence = match evidence {
        Some((_, sources)) => sources,
        None if events_file.is_some() => cfg.evidence.unwrap_or_default(),
        None => events::Sources::default(),
    };
    note_config(
        probe,
        &cli,
        &cfg,
        &config_path,
        events_file.is_some(),
        evidence,
    );
    // Only a run that defers anything has a use for it.
    let evidence_buffer = if evidence.any() {
        events::buffer_from_env()?
    } else {
        events::EVIDENCE_BUFFER
    };
    let privacy = privacy::Privacy::from_sources(
        Some(&cfg.privacy),
        &privacy::process_env,
        cli.privacy.layer(),
    )?;
    probe.privacy = Some(telemetry::payload::PrivacyFlags::of(&privacy));
    if privacy.reveal {
        eprintln!("{}", privacy.reveal_notice());
    }
    let generator = Arc::new(unique::Generator::new());
    let artifacts = Arc::new(artifacts::Artifacts::new(
        cli.artifacts_dir
            .clone()
            .or_else(|| env_var("BDDKIT_ARTIFACTS_DIR").map(PathBuf::from)),
        generator.run_id(),
    )?);
    let env = dirs::Env::from_process(cli.dir.bddkit_dir.clone());
    probe.stage = Some(telemetry::payload::Stage::Plugins);
    let plugins = load_plugins(&config_path, &cfg, &env)?;
    if let Some(plugins) = &plugins {
        (probe.resources.plugins, probe.resources.plugins_other) = plugins.instance_counts();
    }

    probe.stage = Some(telemetry::payload::Stage::Steps);
    let reg = match build_registry(&cfg, plugins.as_ref()) {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!(
                "error: 1 problem, run not started\n\n  {}",
                privacy::mask_text(&error.to_string())
            );
            return Ok(2);
        }
    };

    probe.stage = Some(telemetry::payload::Stage::Selection);
    let filter = feature::TagFilter::new(&cli.tags);
    let paths = if cli.paths.is_empty() {
        cfg.paths.as_slice()
    } else {
        cli.paths.as_slice()
    };

    let mut loaded = Vec::new();
    for path in feature::discover(paths)? {
        let lf = feature::load(&path)?;
        if lf.has_selected_scenario(&filter) {
            loaded.push(Arc::new(lf));
        }
    }
    if loaded.is_empty() {
        eprintln!("error: no scenario selected, run not started");
        return Ok(2);
    }

    probe.stage = Some(telemetry::payload::Stage::Validation);
    let for_check: Vec<&feature::LoadedFeature> = loaded.iter().map(Arc::as_ref).collect();
    let problems = validate::check(&for_check, &reg, &filter);
    if !problems.is_empty() {
        eprintln!("error: {} problem(s), run not started\n", problems.len());
        for p in &problems {
            eprintln!("{}", privacy::mask_text(&p.to_string()));
        }
        return Ok(2);
    }

    probe.stage = Some(telemetry::payload::Stage::Scheduling);
    let chains = runner::build_chains(loaded).map_err(anyhow::Error::msg)?;
    // What `run_all` will start: at least one, never more than the chains.
    probe.concurrency = cfg.concurrency.clamp(1, chains.len().max(1)) as u64;

    probe.stage = Some(telemetry::payload::Stage::Resources);
    let mut by_name = std::collections::HashMap::new();
    for (name, api) in &cfg.resources.api {
        let headers = api
            .default_headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        by_name.insert(
            name.clone(),
            http::ApiResource::new(
                &api.base_url,
                api.timeout_secs,
                headers,
                api.effective_options.clone(),
            )?
            .with_sensitive_headers(api.effective_sensitive_headers.clone()),
        );
    }
    let apis = Arc::new(http::Apis::new(by_name, cfg.resolve_default_api()?)?);

    // Every declared SRP resource, not only the default: a malformed
    // `variant:` in a second block must not sit there until someone points
    // `default_srp` at it. This is also what keeps `doctor` — which reports on
    // every declared resource — from being stricter than the run it predicts.
    //
    // Before the pools: this costs microseconds, and connecting can cost
    // thirty seconds against a database that is down. A suite with both faults
    // should learn about both on the first attempt, not one per attempt.
    for (name, srp) in &cfg.resources.srp {
        srp.to_params()
            .with_context(|| format!("resources.srp.{name}"))?;
    }

    // Pools are created once per run, sized to the worker pool's own
    // concurrency so every worker can hold a connection at once.
    let db = if cfg.resources.db.is_empty() {
        None
    } else {
        Some(Arc::new(
            db::Db::connect(&cfg.resources.db, cfg.concurrency as u32)
                .await
                .map_err(anyhow::Error::msg)?,
        ))
    };
    if let Some(db) = &db {
        probe.resources.db = db.engines().fold(Default::default(), |mut engines, name| {
            telemetry::payload::Engines::add(&mut engines, name);
            engines
        });
    }
    let default_db = cfg.resolve_default_db()?.unwrap_or_default();
    let srp = match cfg.resolve_default_srp()? {
        Some(name) => Some(Arc::new(cfg.resources.srp[&name].to_params()?)),
        None => None,
    };

    // Before anything is announced, and before the handler task is first
    // polled: see `Interrupts`.
    let interrupts = Interrupts::install();
    probe.stage = None;
    probe.started = true;
    println!("run {}", generator.run_id());
    let events = events_file.map(events::Events::start);
    if let Some(events) = &events {
        events.emit(
            "run_started",
            serde_json::json!({
                "schema": 1,
                "bddkit": env!("CARGO_PKG_VERSION"),
                "run_id": generator.run_id(),
                "started_at_unix_ms": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(0)),
                "artifacts_dir": artifacts.root().display().to_string(),
                "config": feature::display_path(&config_path),
                "config_source": resolved.source,
                "selection": {
                    "tags": cli.tags,
                    "paths": cli.paths.iter().map(|p| feature::display_path(p)).collect::<Vec<_>>(),
                },
                "evidence": evidence.any(),
                "evidence_sources": evidence.names(),
                "concurrency": cfg.concurrency,
                "privacy": {
                    "masking": !privacy.reveal,
                    "words": privacy.words,
                    "fields": privacy.fields,
                    "min_length": privacy.min_length,
                },
                "files": chains.iter().map(|chain| chain.files.len()).sum::<usize>(),
            }),
        );
    }
    let mut ctx = runner::RunContext::new(
        reg,
        apis,
        generator.clone(),
        filter,
        db,
        default_db,
        srp,
        plugins.clone(),
        cfg.effective_options.clone(),
        artifacts,
        cli.fail_fast,
    );
    ctx.events = events;
    ctx.evidence = evidence;
    ctx.evidence_buffer = evidence_buffer;
    ctx.redactor = privacy::Redactor::new(&privacy, privacy::recorded());
    let ctx = Arc::new(ctx);
    probe.counters = Some(ctx.counters.clone());

    let interrupt = tokio::spawn(handle_interrupt(ctx.clone(), plugins.clone(), interrupts));
    let results = runner::run_all(chains, ctx.clone(), cfg.concurrency).await;
    // A normal finish races a signal that arrives in the gap before the
    // shutdown below. The claim settles it: a handler that already took it
    // owns the plugin shutdown, the last line of the stream and the exit, and
    // this task waits for it to end the process; otherwise the handler is
    // aborted, so a late signal can neither flip this run's exit code to
    // 128+N nor call Plugins::shutdown a second time concurrently with the
    // one below.
    // For the tail below, registered before the handler can be aborted so that
    // no signal falls between the two.
    let mut tail_interrupts = Interrupts::install();
    if ctx.claim_end() {
        interrupt.abort();
    } else {
        let _ = interrupt.await;
    }

    // After the pool has drained, including a failed or --fail-fast run: an
    // instance that outlives the run is a bug the host must not permit. The
    // libraries themselves stay mapped — see `load_plugins`.
    if let Some(plugins) = &plugins {
        plugins.shutdown();
    }

    probe.tally(&results);
    let mut code = report::print_summary(&results, generator.run_id());
    // The one deliberate exception to "2 = before the first request": a
    // report silently lost behind a green code is the worse outcome.
    if let Err(error) = cli.reports.write(&results) {
        eprintln!("error: {}", privacy::mask_text(&format!("{error:#}")));
        code = 2;
        probe.stage = Some(telemetry::payload::Stage::Reports);
    }
    // After the reports, so `exit` is the code the process really returns —
    // and the same exception covers a stream that could not be written.
    if let Some(events) = ctx.events.clone() {
        let mut flush =
            tokio::task::spawn_blocking(move || events.finish(serde_json::json!({"exit": code})));
        // The same two-signal rule as the handler's: the run is over and its
        // code is settled, so a first signal only says «still writing» and the
        // queued lines are not thrown away; a second one exits at once, with
        // the first one's code and the stream cut short. Without the second
        // way out a reader that stopped reading would make the process
        // unkillable but for SIGKILL.
        let stream = tokio::select! {
            stream = &mut flush => stream,
            (name, first) = tail_interrupts.wait() => {
                eprintln!("\n{name}: still writing the event stream; signal again to exit now");
                tokio::select! {
                    stream = flush => stream,
                    _ = tail_interrupts.wait() => std::process::exit(first),
                }
            }
        };
        if let Ok(Err(error)) = stream {
            eprintln!(
                "error: cannot write the events file: {}",
                privacy::mask_text(&error.to_string())
            );
            code = 2;
            probe.stage = Some(telemetry::payload::Stage::Events);
        }
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn privacy_layer(args: &[&str]) -> privacy::PrivacyLayer {
        let argv = ["bddkit", "run"].into_iter().chain(args.iter().copied());
        match Cli::try_parse_from(argv).expect("flags parse").command {
            Command::Run(run) => run.privacy.layer(),
            _ => unreachable!("parsed `run`"),
        }
    }

    #[test]
    fn an_empty_list_flag_is_an_empty_list_and_an_absent_one_is_nothing_set() {
        let layer = privacy_layer(&["--privacy-fields-only", ""]);
        assert_eq!(layer.fields_only, Some(vec![]));
        assert_eq!(layer.fields, None);
        assert_eq!(privacy_layer(&[]), privacy::PrivacyLayer::default());
    }

    #[test]
    fn a_list_flag_splits_on_commas_and_repeats_append() {
        let layer = privacy_layer(&["--privacy-fields", "otp, pin", "--privacy-fields", "cvv"]);
        assert_eq!(
            layer.fields,
            Some(vec!["otp".into(), "pin".into(), "cvv".into()])
        );
    }

    #[test]
    fn reveal_has_two_spellings_and_only_ever_sets_true() {
        for flag in ["--reveal", "--privacy-reveal"] {
            assert_eq!(privacy_layer(&[flag]).reveal, Some(true), "{flag}");
        }
        assert_eq!(privacy_layer(&[]).reveal, None);
    }
}
