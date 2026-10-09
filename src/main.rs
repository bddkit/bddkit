mod config;
mod db;
mod dirs;
mod doctor;
mod events;
mod feature;
mod hawk;
mod http;
mod include;
mod json;
mod macros;
mod options;
mod plugin;
mod polling;
mod report;
mod resource;
mod runner;
mod srp;
mod steps;
mod unique;
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
    /// Check a suite's config, and with --live probe what it talks to
    Doctor(DoctorArgs),
    /// Show what a resource's config takes
    Resource(ResourceArgs),
    /// Find, install, update and remove plugins
    Plugin(PluginArgs),
    /// Print the version, and where to look next
    Version,
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
    /// Show installed plugins, or one plugin in detail
    Show { name: Option<String> },
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
    let outcome: Result<i32> = async {
        // Located, never parsed: only the project layer's anchor is needed.
        let config_dir = match config::resolve_config_path(args.config.as_deref())? {
            Some(resolved) => config_dir(&resolved.path).to_path_buf(),
            None => PathBuf::from("."),
        };
        let ctx =
            plugin::manage::Context::new(dirs::Env::from_process(args.bddkit_dir), &config_dir)?;
        match command {
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
            PluginCommand::Show { name } => ctx.show(name.as_deref()).await,
        }
    }
    .await;
    Ok(outcome.unwrap_or_else(|error| {
        eprintln!("error: {error:#}");
        1
    }))
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
                    println!("{error:#}\n\nnothing was written.");
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
        let generator = unique::Generator::new();
        if let Some(plugins) = load_plugins(path, &cfg, &generator, &dirs::Env::from_process(None))?
        {
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
}

/// Unlike `run`, every outcome here is a report: a config that cannot be
/// parsed is the most ordinary thing `doctor` has to say, not a reason to
/// answer in a different currency. Hence 0/1 and no `?`.
async fn doctor_command(args: DoctorArgs) -> Result<i32> {
    let dir_env = dirs::Env::from_process(args.dir.bddkit_dir.clone());
    let report = doctor::check(
        args.config.as_deref(),
        args.env.as_deref(),
        args.live,
        &dir_env,
        &args.reports.paths(),
        args.events.as_deref(),
    )
    .await;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render());
    }
    Ok(report.exit_code())
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
    #[command(flatten)]
    dir: DirArgs,
    #[command(flatten)]
    reports: ReportArgs,
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
        let generator = unique::Generator::new();
        let env = dirs::Env::from_process(args.dir.bddkit_dir.clone());
        if let Some(plugins) = load_plugins(path, &cfg, &generator, &env)? {
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
    let cli = Cli::parse();
    // Each command names what did not happen: "run not started" is a lie when
    // the user only asked for a listing.
    let (result, nothing_happened) = match cli.command {
        Command::Run(args) => (run(args).await, "run not started"),
        Command::Steps(args) => (steps_command(args), "nothing listed"),
        Command::Doctor(args) => (doctor_command(args).await, "nothing checked"),
        Command::Resource(args) => (resource_command(args).await, "nothing listed"),
        Command::Plugin(args) => (plugin_command(args).await, "nothing changed"),
        Command::Version => {
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
            eprintln!("error: {error:#}\n\n{nothing_happened}");
            std::process::exit(2);
        }
    }
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
    generator: &unique::Generator,
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
    plugins.set_artifacts_root(std::env::temp_dir().join(format!("bddkit-{}", generator.run_id())));
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

async fn run(cli: RunArgs) -> Result<i32> {
    cli.reports
        .paths()
        .into_iter()
        .try_for_each(report::prepare)?;
    // Opened once, here, and held: see `events::open`. Nothing is written to
    // it until the run has started, so every refusal below leaves it empty.
    let events_file = cli
        .events
        .clone()
        // An empty value is unset: a templated `BDDKIT_EVENTS=` means "off".
        .or_else(|| {
            std::env::var_os("BDDKIT_EVENTS")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        })
        .map(|path| events::open(&path))
        .transpose()?;
    let config_path = config::resolve_config_path(cli.config.as_deref())?
        .ok_or_else(|| anyhow::anyhow!("{}", config::NO_CONFIG_FOUND))?
        .path;
    let cfg = config::load(&config_path, cli.env.as_deref())?;
    // Before the plugins: the artifact root is derived from the run id.
    let generator = Arc::new(unique::Generator::new());
    let env = dirs::Env::from_process(cli.dir.bddkit_dir.clone());
    let plugins = load_plugins(&config_path, &cfg, &generator, &env)?;

    let reg = match build_registry(&cfg, plugins.as_ref()) {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("error: 1 problem, run not started\n\n  {error}");
            std::process::exit(2);
        }
    };

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
        std::process::exit(2);
    }

    let for_check: Vec<&feature::LoadedFeature> = loaded.iter().map(Arc::as_ref).collect();
    let problems = validate::check(&for_check, &reg, &filter);
    if !problems.is_empty() {
        eprintln!("error: {} problem(s), run not started\n", problems.len());
        for p in &problems {
            eprintln!("{p}");
        }
        std::process::exit(2);
    }

    let chains = runner::build_chains(loaded).map_err(anyhow::Error::msg)?;

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
            )?,
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
    let default_db = cfg.resolve_default_db()?.unwrap_or_default();
    let srp = match cfg.resolve_default_srp()? {
        Some(name) => Some(Arc::new(cfg.resources.srp[&name].to_params()?)),
        None => None,
    };

    // Before anything is announced, and before the handler task is first
    // polled: see `Interrupts`.
    let interrupts = Interrupts::install();
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
                "concurrency": cfg.concurrency,
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
        cli.fail_fast,
    );
    ctx.events = events;
    let ctx = Arc::new(ctx);

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

    let mut code = report::print_summary(&results, generator.run_id());
    // The one deliberate exception to "2 = before the first request": a
    // report silently lost behind a green code is the worse outcome.
    if let Err(error) = cli.reports.write(&results) {
        eprintln!("error: {error:#}");
        code = 2;
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
            eprintln!("error: cannot write the events file: {error}");
            code = 2;
        }
    }
    Ok(code)
}
