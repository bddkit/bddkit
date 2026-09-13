use crate::feature::{ExpandedStep, LoadedFeature, expand_outlines};
use crate::options::Options;
use crate::polling::{AttemptError, Polling};
use crate::plugin::abi::{DispatchRequest, OptionsJson, Status};
use crate::report::render_file;
use crate::report::{FileResult, ScenarioResult};
use crate::steps::{Args, OptionsSource, Registry, StepKind, StepTarget, dispatch};
use crate::unique::Generator;
use crate::vars::{VarStack, interpolate};
use crate::world::World;
use anyhow::Result;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

fn prepare(
    step: &ExpandedStep,
    caps: Vec<String>,
    vars: &VarStack,
    generator: &Generator,
) -> Result<Args, String> {
    // Substitution applies to arguments, the doc string, and table cells —
    // never to the whole step text.
    let caps = caps
        .iter()
        .map(|c| interpolate(c, vars, generator))
        .collect::<Result<Vec<_>, _>>()?;
    let docstring = step
        .docstring
        .as_ref()
        .map(|d| interpolate(d, vars, generator))
        .transpose()?;
    let table = step
        .table
        .as_ref()
        .map(|rows| {
            rows.iter()
                .map(|r| {
                    r.iter()
                        .map(|c| interpolate(c, vars, generator))
                        .collect::<Result<Vec<_>, _>>()
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    Ok(Args {
        caps,
        docstring,
        table,
    })
}

fn execute_step<'a>(
    world: &'a mut World,
    reg: &'a Registry,
    step: &'a ExpandedStep,
    generator: &'a Generator,
    depth: usize,
) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
    Box::pin(async move {
        let Some((target, caps)) = reg.find(&step.text)? else {
            return Err("unknown step".into());
        };
        match target {
            StepTarget::Builtin { id, kind } => {
                let args = prepare(step, caps, &world.vars, generator)?;
                match kind {
                    StepKind::Action => {
                        dispatch(world, id, &args, 0).await.map_err(attempt_message)
                    }
                    StepKind::Assertion(source) => {
                        let Some(layer) = world.take_options() else {
                            return dispatch(world, id, &args, 0).await.map_err(attempt_message);
                        };
                        let base = match source {
                            OptionsSource::Global => world.options.clone(),
                            OptionsSource::Http => world.http.options_for_last_response()?.clone(),
                            OptionsSource::Db => world.db.options()?.clone(),
                        };
                        let effective = base.apply(&layer)?;
                        let mut polling = Polling::new(&step.text, &effective.polling);
                        let mut attempt = 0;
                        loop {
                            match dispatch(world, id, &args, attempt).await {
                                Ok(()) => return Ok(()),
                                Err(AttemptError::Fatal(error)) => return Err(error),
                                Err(AttemptError::NotYet(error)) => {
                                    polling.after_not_yet(&error).await?;
                                    attempt += 1;
                                }
                            }
                        }
                    }
                }
            }
            StepTarget::Macro(index) => {
                if step.docstring.is_some() {
                    return Err("a macro call does not support a docstring".into());
                }
                if step.table.is_some() {
                    return Err("a macro call does not support a table".into());
                }
                if depth >= 16 {
                    return Err("macro nesting exceeds 16".into());
                }

                let definition = reg.macro_def(index);
                let args = prepare(step, caps, &world.vars, generator)?;
                world.vars.push_frame();
                for (name, value) in definition.params.iter().zip(args.caps) {
                    world.vars.set(name, value);
                }

                for body_step in &definition.body {
                    let expanded = ExpandedStep {
                        text: body_step.text.clone(),
                        line: step.line,
                        docstring: body_step.docstring.clone(),
                        table: None,
                    };
                    if let Err(error) =
                        execute_step(world, reg, &expanded, generator, depth + 1).await
                    {
                        world.vars.pop_frame(&[])?;
                        return Err(format!("  {}\n{error}", body_step.text));
                    }
                }
                world.vars.pop_frame(&definition.exports)
            }
            StepTarget::Plugin {
                lib,
                step: step_index,
                assertion,
            } => {
                // Arguments cross the boundary already interpolated: a plugin
                // never sees raw step text and never sees `<<variable>>`
                // syntax (invariant 1, restated at the FFI boundary).
                let args = prepare(step, caps, &world.vars, generator)?;
                let Some(plugins) = world.plugins.plugins().cloned() else {
                    return Err("this step is served by a plugin, but no plugin is loaded".into());
                };
                let group = plugins.group_of_step(lib, step_index).to_string();
                let instance = world.plugins.current(&group)?.to_string();
                let base = plugins.options_for(&group, &instance)?.clone();

                // Only an assertion consumes an armed eventual-assertion
                // modifier; an action leaves it for the assertion that follows.
                let layer = if assertion { world.take_options() } else { None };
                let effective = match &layer {
                    Some(layer) => base.apply(layer)?,
                    None => base,
                };
                let mut polling = layer
                    .as_ref()
                    .map(|_| Polling::new(&step.text, &effective.polling));
                // A `per_worker` instance belongs to this file, so this file
                // both creates and drops it; a `shared` one is only borrowed.
                let per_worker = plugins.is_per_worker(lib);

                // Resolved once per plugin step rather than per attempt: the
                // path is constant for the file, and `Workspace` caches the
                // `create_dir_all` after the first call.
                let workspace_dir = world.workspace_dir()?.display().to_string();

                loop {
                    let request = serde_json::to_string(&DispatchRequest {
                        args: &args.caps,
                        docstring: args.docstring.as_ref(),
                        table: args.table.as_ref(),
                        artifacts_dir: plugins.next_artifacts_dir(),
                        workspace_dir: workspace_dir.clone(),
                        debug: world.debug,
                        options: OptionsJson::from(&effective),
                    })
                    .map_err(|error| format!("failed to encode the plugin request: {error}"))?;

                    // The FFI call is synchronous and may block for seconds;
                    // spawn_blocking keeps it off the executor. Passing the
                    // host's tokio Handle across the boundary instead would
                    // pin plugin and host to one tokio version.
                    let plugins_for_call = plugins.clone();
                    let (call_group, call_instance) = (group.clone(), instance.clone());
                    // The handle this file already holds, if any. Passing it in
                    // keeps the dispatch to one blocking hop: resolving it out
                    // here and then dispatching would cost two on the first
                    // plugin step of every file.
                    // Filtered by library, not just group: a handle is unique
                    // within one plugin, never across two. `Plugins::load`
                    // refuses two plugins claiming one group, so this cannot
                    // differ today — the filter keeps it true locally.
                    let cached = world
                        .plugins
                        .handle_for(&group, &instance)
                        .filter(|(l, _)| *l == lib)
                        .map(|(_, h)| h);
                    let (handle, result) = tokio::task::spawn_blocking(move || {
                        plugins_for_call.call_step(
                            &call_group,
                            &call_instance,
                            lib,
                            step_index,
                            &request,
                            cached,
                        )
                    })
                    .await
                    .map_err(|error| format!("the plugin dispatch task failed: {error}"))??;
                    world
                        .plugins
                        .record(&group, &instance, lib, handle, per_worker);

                    match result.status {
                        Status::Passed => {
                            // Variables are published only on success: an
                            // intermediate observation must not leak into the
                            // scenario.
                            for (name, value) in result.vars {
                                world.vars.set(&name, value);
                            }
                            return Ok(());
                        }
                        Status::Fatal => return Err(result.render_failure()),
                        Status::NotYet if !assertion => {
                            return Err(format!(
                                "a plugin action answered not_yet, which only an assertion may do: {}",
                                result.render_failure()
                            ));
                        }
                        Status::NotYet => match &mut polling {
                            // Without an armed modifier there is no second
                            // attempt, so not_yet is simply a failure.
                            None => return Err(result.render_failure()),
                            Some(polling) => polling.after_not_yet(&result.render_failure()).await?,
                        },
                    }
                }
            }
        }
    })
}

fn attempt_message(error: AttemptError) -> String {
    match error {
        AttemptError::NotYet(error) | AttemptError::Fatal(error) => error,
    }
}

/// Everything shared across the whole run, behind one `Arc`: a worker clones
/// a single reference instead of six.
pub struct RunContext {
    pub reg: Registry,
    pub apis: Arc<crate::http::Apis>,
    pub generator: Arc<Generator>,
    pub filter: crate::feature::TagFilter,
    pub db: Option<Arc<crate::db::Db>>,
    pub default_db: String,
    pub srp: Option<Arc<crate::srp::SrpParams>>,
    pub plugins: Option<Arc<crate::plugin::Plugins>>,
    pub options: Options,
    fail_fast: bool,
    stop: std::sync::atomic::AtomicBool,
}

impl RunContext {
    // An aggregator for run-wide config: the parameter list grows with each
    // resource (db, srp, …), a separate builder for a single constructor is overkill.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        reg: Registry,
        apis: Arc<crate::http::Apis>,
        generator: Arc<Generator>,
        filter: crate::feature::TagFilter,
        db: Option<Arc<crate::db::Db>>,
        default_db: String,
        srp: Option<Arc<crate::srp::SrpParams>>,
        plugins: Option<Arc<crate::plugin::Plugins>>,
        options: Options,
        fail_fast: bool,
    ) -> Self {
        Self {
            reg,
            apis,
            generator,
            filter,
            db,
            default_db,
            srp,
            plugins,
            options,
            fail_fast,
            stop: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Arms the stop — only when `--fail-fast` is enabled. Without the flag a
    /// file's failure stops nothing: the run must reach the end and show all
    /// failures at once.
    pub fn request_stop(&self) {
        if self.fail_fast {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    /// The semantics of `--fail-fast` under parallelism are "don't start new
    /// work", not "abort everything immediately": an in-flight request is not
    /// recalled, and cutting it off mid-flight would leave the system under
    /// test in an unclear state.
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

/// Runs one feature file. The variable frame is shared for the file; HTTP
/// state is recreated for each scenario; the Background reruns before each
/// one. Arguments are owned (`Arc`), not borrowed: a file is one `tokio`
/// task, and `tokio::spawn` requires `'static`.
pub async fn run_file(lf: Arc<LoadedFeature>, ctx: Arc<RunContext>) -> FileResult {
    let mut world = World::new(
        ctx.apis.clone(),
        ctx.generator.clone(),
        crate::db::DbHandle::new(ctx.db.clone(), ctx.default_db.clone()),
        ctx.srp.clone(),
        ctx.plugins.clone(),
        ctx.options.clone(),
    );
    let mut scenarios = Vec::new();

    let background: Vec<ExpandedStep> = lf
        .feature
        .background
        .as_ref()
        .map(|bg| {
            bg.steps
                .iter()
                .map(|s| ExpandedStep {
                    text: s.value.clone(),
                    line: s.position.line,
                    docstring: s.docstring.clone(),
                    table: s.table.as_ref().map(|t| t.rows.clone()),
                })
                .collect()
        })
        .unwrap_or_default();

    // The generator handle is cloned once: inside the loop `&world.generator`
    // would conflict with the later `&mut world` in the dispatch call.
    let generator = world.generator.clone();

    for sc in &lf.feature.scenarios {
        if !ctx.filter.matches(&sc.tags) {
            continue;
        }
        for ex in expand_outlines(sc) {
            world.reset_scenario();
            // The FFI half of the scenario reset. It is here rather than in
            // World::reset_scenario because the call is blocking and this is
            // the async context that can hand it to spawn_blocking; runs with
            // no plugin loaded pay nothing.
            //
            // A reset that fails is this scenario's failure, exactly as a
            // failing Background step would be: running its steps against
            // state the plugin could not clear is how a suite goes green for
            // the wrong reason.
            let mut failure = match ctx.plugins.clone() {
                None => None,
                Some(plugins) => {
                    // Only what THIS file has touched. A file with no plugin
                    // steps has an empty list and does no work here at all.
                    let handles = world.plugins.to_reset();
                    if handles.is_empty() {
                        None
                    } else {
                        match tokio::task::spawn_blocking(move || plugins.reset_instances(&handles))
                            .await
                        {
                            Ok(result) => {
                                result.err().map(|error| format!("  scenario reset\n{error}"))
                            }
                            Err(error) => Some(format!(
                                "  scenario reset\nthe plugin reset task failed: {error}"
                            )),
                        }
                    }
                }
            };

            for step in background.iter().chain(ex.steps.iter()) {
                if failure.is_some() {
                    break;
                }
                if let Err(e) = execute_step(&mut world, &ctx.reg, step, &generator, 0).await {
                    let mut msg = format!("  {}\n{e}", step.text);
                    if let Some(ex) = world.http.last() {
                        msg.push_str(&format!("\n\n{ex}"));
                    }
                    failure = Some(msg);
                    break;
                }
            }
            scenarios.push(ScenarioResult {
                name: ex.name,
                line: ex.line,
                failure,
            });
        }
    }
    // Every exit path of this function, including a file whose scenarios all
    // failed. A panicking file cannot reach here — its handles stay in the
    // registry and `Plugins::shutdown` sweeps them after the pool drains.
    if let Some(plugins) = ctx.plugins.clone() {
        let handles = world.plugins.to_drop();
        if !handles.is_empty() {
            let _ = tokio::task::spawn_blocking(move || plugins.drop_instances(&handles)).await;
        }
    }

    FileResult {
        path: lf.path.clone(),
        scenarios,
    }
}

/// No more workers than units of work, and never fewer than one:
/// `concurrency: 0` in the config must not mean "run nothing".
fn worker_count(concurrency: usize, units: usize) -> usize {
    concurrency.clamp(1, units.max(1))
}

/// A panic inside a file does not kill the run: the file runs as a separate
/// task, and its failure turns into that file's failure (spec §10). The
/// panic text itself is printed to stderr by the standard panic hook.
fn panicked_file(path: std::path::PathBuf, error: &tokio::task::JoinError) -> FileResult {
    FileResult {
        path,
        scenarios: vec![ScenarioResult {
            name: "file run aborted".to_string(),
            line: 0,
            failure: Some(format!("panic while running the file: {error}")),
        }],
    }
}

/// A chain is the unit of scheduling. Its files run strictly one after
/// another on a single worker; different chains run in parallel. A file
/// without `@serial` forms a chain of one file, so "parallel by file" is a
/// special case of "parallel by chain".
///
/// Partitioning instead of a mutex on the group name is a deliberate choice:
/// a worker stuck on a busy group would sit idle holding a slot while
/// independent files wait in the queue.
#[derive(Debug)]
pub struct Chain {
    pub priority: i64,
    pub files: Vec<Arc<LoadedFeature>>,
}

/// Groups files into chains by the `@serial(name)` tag and orders the queue
/// by `@priority(N)`: higher goes earlier. Order at equal priority depends
/// only on paths — `discover` already returns them sorted — so it is
/// reproducible from run to run.
pub fn build_chains(files: Vec<Arc<LoadedFeature>>) -> Result<Vec<Chain>, String> {
    // (priority, file) — so tags aren't re-read on every comparison
    let mut named: BTreeMap<String, Vec<(i64, Arc<LoadedFeature>)>> = BTreeMap::new();
    let mut chains: Vec<Chain> = Vec::new();

    for lf in files {
        let priority = crate::feature::priority_of(&lf)?;
        match crate::feature::serial_of(&lf)? {
            Some(name) => named.entry(name).or_default().push((priority, lf)),
            None => chains.push(Chain {
                priority,
                files: vec![lf],
            }),
        }
    }
    for (_, mut members) in named {
        // The sort is stable, and `discover` returned the paths sorted:
        // at equal priorities, order within a chain is alphabetical.
        members.sort_by_key(|(p, _)| std::cmp::Reverse(*p));
        let priority = members.iter().map(|(p, _)| *p).max().unwrap_or(0);
        chains.push(Chain {
            priority,
            files: members.into_iter().map(|(_, lf)| lf).collect(),
        });
    }
    // Priority descending, ties broken by the first file's path. The second
    // key is required: without it, single files and named chains would sort
    // by container fill order, i.e. differently from run to run.
    chains.sort_by(|a, b| {
        b.priority.cmp(&a.priority).then_with(|| {
            a.files
                .first()
                .map(|f| &f.path)
                .cmp(&b.files.first().map(|f| &f.path))
        })
    });
    Ok(chains)
}

/// A worker pool over a shared queue of CHAINS. A chain's files run strictly
/// in order on one worker; scenarios within a file are always sequential,
/// because variables live for the file (invariant 2).
///
/// The nested `tokio::spawn` per file exists for exactly two properties: it
/// isolates panics (a worker survives a bad file and picks up the next one),
/// and the immediate `await` keeps tasks from outrunning the pool size.
pub async fn run_all(
    chains: Vec<Chain>,
    ctx: Arc<RunContext>,
    concurrency: usize,
) -> Vec<FileResult> {
    let workers = worker_count(concurrency, chains.len());
    let total: usize = chains.iter().map(|c| c.files.len()).sum();
    let chains = Arc::new(chains);
    let cursor = Arc::new(AtomicUsize::new(0));
    let results = Arc::new(Mutex::new(Vec::with_capacity(total)));

    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let (chains, cursor, results, ctx) =
            (chains.clone(), cursor.clone(), results.clone(), ctx.clone());
        handles.push(tokio::spawn(async move {
            loop {
                if ctx.stopped() {
                    break;
                }
                let index = cursor.fetch_add(1, Ordering::Relaxed);
                let Some(chain) = chains.get(index) else {
                    break;
                };
                for lf in &chain.files {
                    if ctx.stopped() {
                        break;
                    }
                    let path = lf.path.clone();
                    let task = tokio::spawn(run_file(lf.clone(), ctx.clone()));
                    let result = match task.await {
                        Ok(r) => r,
                        Err(error) => panicked_file(path, &error),
                    };
                    if result.failed() > 0 {
                        ctx.request_stop();
                    }
                    // Printing and collecting under one lock: a file's output
                    // lands in stdout as a whole, not interleaved with another
                    // worker's dump.
                    let mut collected = results.lock().expect("results mutex");
                    print!("{}", render_file(&result));
                    collected.push(result);
                }
            }
        }));
    }
    for handle in handles {
        // The worker itself cannot panic — a file's failure is already caught
        // above; but it still must not bring down the rest of the run.
        let _ = handle.await;
    }
    std::mem::take(&mut *results.lock().expect("results mutex"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::parse_str;
    use crate::macros::MacroCatalog;
    use crate::unique::UniqueKind;
    use std::path::PathBuf;

    fn apis() -> Arc<crate::http::Apis> {
        let mut by_name = std::collections::HashMap::new();
        by_name.insert(
            "default".to_string(),
            crate::http::ApiResource::new("http://example.test", 1, Vec::new(), Options::default())
                .unwrap(),
        );
        Arc::new(crate::http::Apis::new(by_name, Some("default".to_string())).unwrap())
    }

    fn context_with_generator(registry: Registry, generator: Arc<Generator>) -> Arc<RunContext> {
        Arc::new(RunContext::new(
            registry,
            apis(),
            generator,
            crate::feature::TagFilter::new(&[]),
            None,
            String::new(),
            None,
            None,
            Options::default(),
            false,
        ))
    }

    fn context(registry: Registry) -> Arc<RunContext> {
        context_with_generator(registry, Arc::new(Generator::new()))
    }

    /// The fixture plugin, with one declared instance "a" that is also the
    /// group default. Built by `plugin::tests` — the same helper its own unit
    /// tests use, so there is one place that knows how to build the cdylib.
    fn echo_plugins() -> crate::plugin::Plugins {
        let mut plugins = crate::plugin::Plugins::load(
            vec![crate::plugin::tests::entry()],
            &[crate::plugin::tests::instance("a", Some("p-"))],
            &["echo".to_string()],
            1,
            &crate::options::Options::default(),
        )
        .expect("the fixture plugin loads");
        plugins.add_defaults([("echo".to_string(), "a".to_string())].into_iter().collect());
        plugins
    }

    /// `force_action` overrides the kind the plugin declared for one step,
    /// which is how a plugin that answers `not_yet` from an ACTION is built:
    /// the registry, not the reply, is what says a step is an assertion.
    fn echo_registry(plugins: &crate::plugin::Plugins, force_action: Option<usize>) -> Registry {
        let steps: Vec<(usize, usize, String, bool)> = plugins
            .steps()
            .into_iter()
            .map(|(lib, index, pattern, assertion)| {
                (lib, index, pattern, assertion && force_action != Some(index))
            })
            .collect();
        Registry::with_macros_and_plugins(
            MacroCatalog {
                definitions: Vec::new(),
            },
            &steps,
            &["echo".to_string()],
        )
        .expect("the plugin registry builds")
    }

    /// The `Arc` comes back too: a test that asserts how many instances were
    /// created needs the same registry the `World` dispatches through.
    fn echo_world(plugins: crate::plugin::Plugins) -> (World, Arc<crate::plugin::Plugins>) {
        let plugins = Arc::new(plugins);
        let world = World::new(
            apis(),
            Arc::new(Generator::new()),
            crate::db::DbHandle::new(None, String::new()),
            None,
            Some(plugins.clone()),
            Options::default(),
        );
        (world, plugins)
    }

    async fn run_step(world: &mut World, reg: &Registry, text: &str) -> Result<(), String> {
        let step = ExpandedStep {
            text: text.to_string(),
            line: 1,
            docstring: None,
            table: None,
        };
        let generator = world.generator.clone();
        execute_step(world, reg, &step, &generator, 0).await
    }

    #[tokio::test]
    async fn a_plugin_step_records_its_handle_on_the_world() {
        // The second dispatch in a file must reuse the first one's handle
        // rather than resolving again — that is what makes a per-file instance
        // one instance rather than one per step.
        let plugins = echo_plugins();
        let reg = echo_registry(&plugins, None);
        let (mut world, plugins) = echo_world(plugins);
        run_step(&mut world, &reg, r#"I echo "x" as "first""#)
            .await
            .expect("first dispatch");
        let recorded = world
            .plugins
            .handle_for("echo", "a")
            .expect("the first dispatch recorded its handle");
        run_step(&mut world, &reg, r#"I echo "y" as "second""#)
            .await
            .expect("second dispatch");
        assert_eq!(
            world.plugins.handle_for("echo", "a"),
            Some(recorded),
            "the second dispatch reused the recorded handle"
        );
        assert_eq!(
            plugins.registered_count(),
            1,
            "and created no second instance"
        );
    }

    #[tokio::test]
    async fn a_plugin_step_gets_interpolated_arguments_and_publishes_its_vars() {
        // The plugin echoes back its first argument; seeing "world" there is
        // what proves interpolation happened on the host side of the boundary.
        let plugins = echo_plugins();
        let reg = echo_registry(&plugins, None);
        let (mut world, _) = echo_world(plugins);
        world.vars.set("who", "world".to_string());
        run_step(&mut world, &reg, r#"I echo "<<who>>" as "greeting""#)
            .await
            .expect("the action passes");
        assert_eq!(world.vars.get("greeting"), Some("p-world"));
    }

    #[tokio::test]
    async fn an_action_answering_not_yet_is_a_contract_error() {
        // `not_yet` means "poll me again", and only an assertion may be polled.
        let plugins = echo_plugins();
        let reg = echo_registry(&plugins, Some(1));
        let (mut world, _) = echo_world(plugins);
        let error = run_step(&mut world, &reg, "the echo counter should reach 3")
            .await
            .expect_err("an action may not answer not_yet");
        assert!(error.contains("only an assertion may do"), "{error}");
        assert!(error.contains("counter is 1 of 3"), "{error}");
    }

    #[tokio::test]
    async fn a_not_yet_without_an_armed_modifier_fails_and_discards_its_vars() {
        // No modifier armed means no second attempt, and the vars an
        // unfinished observation carries must not reach the scenario.
        let plugins = echo_plugins();
        let reg = echo_registry(&plugins, None);
        let (mut world, _) = echo_world(plugins);
        let error = run_step(&mut world, &reg, "the echo counter should reach 3")
            .await
            .expect_err("one attempt, and it did not pass");
        assert!(error.contains("counter is 1 of 3"), "{error}");
        assert_eq!(world.vars.get("echo_attempts"), None);
    }

    #[tokio::test]
    async fn an_armed_modifier_polls_the_plugin_until_it_passes() {
        // The retry loop belongs to the host: the plugin makes exactly one
        // fresh observation per dispatch and never sleeps.
        let plugins = echo_plugins();
        let reg = echo_registry(&plugins, None);
        let (mut world, _) = echo_world(plugins);
        world.arm_options(
            serde_yaml_ng::from_str("polling:\n  timeout_secs: 5\n  interval_ms: 1\n")
                .expect("the layer parses"),
        );
        run_step(&mut world, &reg, "the echo counter should reach 3")
            .await
            .expect("the third attempt passes");
        assert_eq!(
            world.vars.get("echo_attempts"),
            Some("3"),
            // Not "only the passing attempt publishes its vars" — the passing
            // attempt's value would overwrite a leaked one anyway, so this
            // assertion cannot see that rule. The discard test guards it.
            "the retry loop reached the third attempt"
        );
    }

    #[tokio::test]
    async fn a_plugin_action_leaves_an_armed_modifier_for_the_next_assertion() {
        // The other half of "only an assertion consumes the modifier": if the
        // action swallowed it, the assertion below would get a single attempt.
        let plugins = echo_plugins();
        let reg = echo_registry(&plugins, None);
        let (mut world, _) = echo_world(plugins);
        world.arm_options(
            serde_yaml_ng::from_str("polling:\n  timeout_secs: 5\n  interval_ms: 1\n")
                .expect("the layer parses"),
        );
        run_step(&mut world, &reg, r#"I echo "x" as "ignored""#)
            .await
            .expect("the action passes");
        run_step(&mut world, &reg, "the echo counter should reach 3")
            .await
            .expect("the modifier survived the action");
    }

    #[tokio::test]
    async fn a_fatal_reply_carries_its_message_and_its_diagnostics() {
        // Evidence reaches the user through the returned string, never
        // through a print that would land inside another worker's dump.
        let plugins = echo_plugins();
        let reg = echo_registry(&plugins, None);
        let (mut world, _) = echo_world(plugins);
        let error = run_step(&mut world, &reg, "the echo should fail")
            .await
            .expect_err("the step is asked to fail");
        assert!(error.contains("the echo step was asked to fail"), "{error}");
        assert!(error.contains("echo state"), "{error}");
        assert!(error.contains("prefix=p-"), "{error}");
    }

    fn registry(name: &str, source: &str) -> Registry {
        let dir =
            std::env::temp_dir().join(format!("bddkit-runner-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("macros.yaml");
        std::fs::write(&path, source).unwrap();
        Registry::with_macros(MacroCatalog::load(&[path]).unwrap()).unwrap()
    }

    async fn run(feature: &str, registry: Registry) -> FileResult {
        run_with_generator(feature, registry, Arc::new(Generator::new())).await
    }

    async fn run_with_generator(
        feature: &str,
        registry: Registry,
        generator: Arc<Generator>,
    ) -> FileResult {
        let loaded = LoadedFeature {
            path: PathBuf::from("macro.feature"),
            feature: parse_str(feature).unwrap(),
        };
        run_file(
            Arc::new(loaded),
            context_with_generator(registry, generator),
        )
        .await
    }

    /// `tokio::spawn` requires `Send + 'static`. This is checked by the
    /// compiler: if the file-run future stops being Send, the test won't build.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_file_run_can_be_spawned_onto_another_thread() {
        let loaded = LoadedFeature {
            path: PathBuf::from("spawnable.feature"),
            feature: parse_str(
                "Feature: f\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
            )
            .expect("gherkin parses"),
        };
        let ctx = context(Registry::new().expect("built-in patterns compile"));

        let result = tokio::spawn(run_file(Arc::new(loaded), ctx))
            .await
            .expect("the task must not panic");

        assert_eq!(result.failed(), 0, "{:?}", result.scenarios[0].failure);
    }

    #[tokio::test]
    async fn macro_interpolates_parameter_and_exports_result() {
        let registry = registry(
            "export",
            r#"
- step: 'I remember "{value}"'
  exports: [result]
  do:
    - set variable "private" to "<<value>>"
    - set variable "result" to "<<private>>"
"#,
        );
        let result = run(
            r#"
Feature: macro
  Scenario: export
    Given set variable "outer" to "saved"
    When I remember "<<outer>>"
    Then variable "result" should be equal to "saved"
"#,
            registry,
        )
        .await;

        assert!(
            result.scenarios[0].failure.is_none(),
            "{:?}",
            result.scenarios[0].failure
        );
    }

    #[tokio::test]
    async fn macro_drops_private_variables() {
        let registry = registry(
            "private",
            r#"
- step: I create private state
  do:
    - set variable "private" to "hidden"
"#,
        );
        let result = run(
            r#"
Feature: macro
  Scenario: private
    When I create private state
    Then variable "private" should be equal to "hidden"
"#,
            registry,
        )
        .await;

        let failure = result.scenarios[0].failure.as_deref().unwrap();
        assert!(
            failure.contains("private") && failure.contains("is not set"),
            "{failure}"
        );
    }

    #[tokio::test]
    async fn macro_exports_matching_glob() {
        let registry = registry(
            "glob",
            r#"
- step: I create a row
  exports: [last_insert_id_*]
  do:
    - set variable "last_insert_id_users" to "42"
"#,
        );
        let result = run(
            r#"
Feature: macro
  Scenario: glob
    When I create a row
    Then variable "last_insert_id_users" should be equal to "42"
"#,
            registry,
        )
        .await;

        assert!(
            result.scenarios[0].failure.is_none(),
            "{:?}",
            result.scenarios[0].failure
        );
    }

    #[tokio::test]
    async fn nested_macro_exports_through_each_frame() {
        let registry = registry(
            "nested-export",
            r#"
- step: 'I make inner "{value}"'
  exports: [inner]
  do:
    - set variable "inner" to "<<value>>"
- step: 'I make outer "{value}"'
  exports: [result]
  do:
    - I make inner "<<value>>"
    - set variable "result" to "<<inner>>"
"#,
        );
        let result = run(
            r#"
Feature: macro
  Scenario: nested
    When I make outer "done"
    Then variable "result" should be equal to "done"
"#,
            registry,
        )
        .await;

        assert!(
            result.scenarios[0].failure.is_none(),
            "{:?}",
            result.scenarios[0].failure
        );
    }

    #[tokio::test]
    async fn missing_declared_export_fails_scenario() {
        let registry = registry(
            "missing-export",
            r#"
- step: I forget the result
  exports: [result]
  do:
    - set variable "private" to "x"
"#,
        );
        let result = run(
            "Feature: macro\n  Scenario: missing\n    When I forget the result\n",
            registry,
        )
        .await;

        let failure = result.scenarios[0].failure.as_deref().unwrap();
        assert!(
            failure.contains("result") && failure.contains("is not set"),
            "{failure}"
        );
    }

    #[tokio::test]
    async fn macro_call_rejects_docstring_argument() {
        let registry = registry(
            "call-docstring",
            "- step: I do business\n  do: [the response code is 200]\n",
        );
        let result = run(
            r#"
Feature: macro
  Scenario: docstring
    When I do business
      """
      unsupported
      """
"#,
            registry,
        )
        .await;

        let failure = result.scenarios[0].failure.as_deref().unwrap();
        assert!(failure.contains("docstring"), "{failure}");
    }

    #[tokio::test]
    async fn macro_call_rejects_table_argument() {
        let registry = registry(
            "call-table",
            "- step: I do business\n  do: [the response code is 200]\n",
        );
        let result = run(
            r#"
Feature: macro
  Scenario: table
    When I do business
      | value |
      | x     |
"#,
            registry,
        )
        .await;

        let failure = result.scenarios[0].failure.as_deref().unwrap();
        assert!(failure.contains("table"), "{failure}");
    }

    mod eventual {
        use super::*;
        use tokio::time::Instant;

        async fn failure(feature: &str, registry: Registry) -> String {
            run(feature, registry).await.scenarios[0]
                .failure
                .clone()
                .expect("scenario should fail")
        }

        #[tokio::test(start_paused = true)]
        async fn variable_mismatch_times_out_with_the_last_observation() {
            let failure = failure(
                r#"
Feature: eventual assertion
  Scenario: timeout
    Given set variable "state" to "pending"
    And I expect the next assertion to pass within "1" seconds, checking every "100" milliseconds
    Then variable "state" should be equal to "ready"
"#,
                Registry::new().unwrap(),
            )
            .await;

            for expected in [
                "variable \"state\" should be equal to \"ready\"",
                "1s",
                "100ms",
                "11 attempts",
                "expected: ready",
                "actual:   pending",
            ] {
                assert!(
                    failure.contains(expected),
                    "missing {expected:?} in {failure}"
                );
            }
        }

        #[tokio::test(start_paused = true)]
        async fn a_retrying_assertion_prepares_unique_capture_once() {
            let generator = Arc::new(Generator::new());
            let before = generator.next(UniqueKind::Number).parse::<u64>().unwrap();
            let result = run_with_generator(
                r#"
Feature: eventual assertion
  Scenario: stable preparation
    Given set variable "state" to "pending"
    And I expect the next assertion to pass within "1" seconds, checking every "100" milliseconds
    Then variable "state" should be equal to "<<unique(number)>>"
"#,
                Registry::new().unwrap(),
                generator.clone(),
            )
            .await;
            let failure = result.scenarios[0].failure.as_deref().unwrap();
            assert!(failure.contains("11 attempts"), "{failure}");
            let after = generator.next(UniqueKind::Number).parse::<u64>().unwrap();

            assert_eq!(after - before, 2, "unique() must be evaluated only once");
        }

        #[tokio::test(start_paused = true)]
        async fn a_missing_variable_is_fatal_without_waiting() {
            let started = Instant::now();
            let failure = failure(
                r#"
Feature: eventual assertion
  Scenario: fatal
    Given I expect the next assertion to pass within "1" seconds, checking every "100" milliseconds
    Then variable "missing" should be equal to "ready"
"#,
                Registry::new().unwrap(),
            )
            .await;

            assert!(
                failure.contains("missing") && failure.contains("is not set"),
                "{failure}"
            );
            assert_eq!(Instant::now(), started);
        }

        #[tokio::test(start_paused = true)]
        async fn a_later_modifier_silently_replaces_the_first() {
            let failure = failure(
                r#"
Feature: eventual assertion
  Scenario: replacement
    Given set variable "state" to "pending"
    And I expect the next assertion to pass within "5" seconds
    And I expect the next assertion to pass within "1" seconds
    Then variable "state" should be equal to "ready"
"#,
                Registry::new().unwrap(),
            )
            .await;

            assert!(failure.contains("within 1s"), "{failure}");
            assert!(!failure.contains("within 5s"), "{failure}");
        }

        #[tokio::test(start_paused = true)]
        async fn a_modifier_survives_an_ordinary_action() {
            let failure = failure(
                r#"
Feature: eventual assertion
  Scenario: action
    Given I expect the next assertion to pass within "1" seconds
    And set variable "state" to "pending"
    Then variable "state" should be equal to "ready"
"#,
                Registry::new().unwrap(),
            )
            .await;

            assert!(failure.contains("did not pass within 1s"), "{failure}");
        }

        #[tokio::test(start_paused = true)]
        async fn a_modifier_survives_a_macro_boundary() {
            let registry = registry(
                "eventual",
                r#"
- step: I check state through a macro
  do:
    - set variable "macro_ran" to "yes"
    - variable "state" should be equal to "ready"
"#,
            );
            let failure = failure(
                r#"
Feature: eventual assertion
  Scenario: macro
    Given set variable "state" to "pending"
    And I expect the next assertion to pass within "1" seconds
    Then I check state through a macro
"#,
                registry,
            )
            .await;

            assert!(failure.contains("did not pass within 1s"), "{failure}");
        }

        #[tokio::test(start_paused = true)]
        async fn the_first_assertion_consumes_the_modifier() {
            let started = Instant::now();
            let failure = failure(
                r#"
Feature: eventual assertion
  Scenario: one shot
    Given set variable "state" to "ready"
    And I expect the next assertion to pass within "1" seconds
    Then variable "state" should be equal to "ready"
    And variable "state" should be equal to "later"
"#,
                Registry::new().unwrap(),
            )
            .await;

            assert!(!failure.contains("did not pass"), "{failure}");
            assert!(failure.contains("expected: later"), "{failure}");
            assert_eq!(Instant::now(), started);
        }

        #[tokio::test(start_paused = true)]
        async fn a_dangling_modifier_passes_silently() {
            let result = run(
                r#"
Feature: eventual assertion
  Scenario: dangling
    Given I expect the next assertion to pass eventually
"#,
                Registry::new().unwrap(),
            )
            .await;

            assert_eq!(result.failed(), 0, "{:?}", result.scenarios[0].failure);
        }

        #[tokio::test(start_paused = true)]
        async fn a_scenario_reset_discards_a_dangling_modifier() {
            let started = Instant::now();
            let result = run(
                r#"
Feature: eventual assertion
  Scenario: arm
    Given I expect the next assertion to pass within "1" seconds
  Scenario: assert
    Given set variable "state" to "pending"
    Then variable "state" should be equal to "ready"
"#,
                Registry::new().unwrap(),
            )
            .await;

            assert!(result.scenarios[0].failure.is_none());
            let failure = result.scenarios[1]
                .failure
                .as_deref()
                .expect("second scenario fails");
            assert!(!failure.contains("did not pass"), "{failure}");
            assert_eq!(Instant::now(), started);
        }

        #[tokio::test(start_paused = true)]
        async fn zero_polling_values_are_rejected_when_arming() {
            let failure = failure(
                r#"
Feature: eventual assertion
  Scenario: zero
    Given I expect the next assertion to pass within "0" seconds
"#,
                Registry::new().unwrap(),
            )
            .await;

            assert!(failure.contains("positive"), "{failure}");
        }

        #[tokio::test(start_paused = true)]
        async fn an_explicit_interval_cannot_exceed_the_explicit_timeout() {
            let failure = failure(
                r#"
Feature: eventual assertion
  Scenario: invalid interval
    Given I expect the next assertion to pass within "1" seconds, checking every "1001" milliseconds
"#,
                Registry::new().unwrap(),
            )
            .await;

            assert!(failure.contains("must not exceed"), "{failure}");
        }
    }

    #[test]
    fn a_zero_concurrency_config_still_runs_with_one_worker() {
        assert_eq!(worker_count(0, 5), 1);
    }

    #[test]
    fn workers_never_outnumber_the_work() {
        assert_eq!(worker_count(8, 3), 3);
    }

    #[test]
    fn a_concurrency_below_the_unit_count_is_respected() {
        assert_eq!(worker_count(4, 10), 4);
    }

    mod chains {
        use super::super::*;
        use crate::feature::parse_str;
        use std::path::PathBuf;
        use std::sync::Arc;

        fn file(path: &str, tags: &str) -> Arc<LoadedFeature> {
            let src =
                format!("{tags}Feature: f\n  Scenario: s\n    Then the response code is 200\n");
            Arc::new(LoadedFeature {
                path: PathBuf::from(path),
                feature: parse_str(&src).expect("gherkin parses"),
            })
        }

        #[test]
        fn untagged_files_become_one_chain_each() {
            let chains = build_chains(vec![file("a.feature", ""), file("b.feature", "")])
                .expect("no tags — no errors");
            assert_eq!(chains.len(), 2);
            assert!(chains.iter().all(|c| c.files.len() == 1));
        }

        #[test]
        fn files_sharing_a_name_end_up_in_one_chain() {
            let chains = build_chains(vec![
                file("a.feature", "@serial(x)\n"),
                file("b.feature", ""),
                file("c.feature", "@serial(x)\n"),
            ])
            .expect("tags are valid");
            assert_eq!(chains.len(), 2, "two chains: x and the lone b");
            let x = chains
                .iter()
                .find(|c| c.files.len() == 2)
                .expect("chain x exists");
            assert_eq!(x.files[0].path, PathBuf::from("a.feature"));
            assert_eq!(x.files[1].path, PathBuf::from("c.feature"));
        }

        #[test]
        fn different_names_stay_in_different_chains() {
            let chains = build_chains(vec![
                file("a.feature", "@serial(x)\n"),
                file("b.feature", "@serial(y)\n"),
            ])
            .expect("tags are valid");
            assert_eq!(chains.len(), 2);
        }

        #[test]
        fn chain_order_is_reproducible_across_runs() {
            // Queue order must depend only on paths, not on the order files
            // landed in a HashMap.
            let build = || {
                build_chains(vec![
                    file("b.feature", "@serial(zeta)\n"),
                    file("a.feature", ""),
                    file("c.feature", "@serial(alpha)\n"),
                ])
                .expect("tags are valid")
                .into_iter()
                .map(|c| c.files[0].path.clone())
                .collect::<Vec<_>>()
            };
            assert_eq!(build(), build());
        }

        #[test]
        fn a_broken_serial_tag_stops_chain_building() {
            let err = build_chains(vec![file("a.feature", "@serial()\n")])
                .expect_err("an empty chain name is an error");
            assert!(err.contains("a.feature"), "{err}");
        }

        #[test]
        fn higher_priority_chains_come_first() {
            let chains = build_chains(vec![
                file("a.feature", ""),
                file("b.feature", "@priority(-1)\n"),
                file("c.feature", "@priority(5)\n"),
            ])
            .expect("tags are valid");
            let order: Vec<_> = chains.iter().map(|c| c.files[0].path.clone()).collect();
            assert_eq!(
                order,
                vec![
                    PathBuf::from("c.feature"),
                    PathBuf::from("a.feature"),
                    PathBuf::from("b.feature"),
                ]
            );
        }

        #[test]
        fn a_chain_takes_the_highest_priority_of_its_files() {
            let chains = build_chains(vec![
                file("a.feature", "@serial(x)\n"),
                file("b.feature", "@serial(x)\n@priority(7)\n"),
                file("c.feature", "@priority(3)\n"),
            ])
            .expect("tags are valid");
            assert_eq!(chains[0].priority, 7, "chain x must go first");
            assert_eq!(chains[0].files.len(), 2);
        }

        #[test]
        fn files_inside_a_chain_are_ordered_by_priority_too() {
            let chains = build_chains(vec![
                file("a.feature", "@serial(x)\n"),
                file("b.feature", "@serial(x)\n@priority(7)\n"),
            ])
            .expect("tags are valid");
            assert_eq!(chains[0].files[0].path, PathBuf::from("b.feature"));
        }

        #[test]
        fn equal_priorities_keep_alphabetical_order() {
            let chains = build_chains(vec![
                file("b.feature", "@priority(1)\n"),
                file("a.feature", "@priority(1)\n"),
            ])
            .expect("tags are valid");
            assert_eq!(chains[0].files[0].path, PathBuf::from("a.feature"));
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_panicking_file_becomes_a_failed_result_instead_of_killing_the_run() {
        let error = tokio::spawn(async { panic!("deliberate") })
            .await
            .expect_err("the task must panic");
        let result = panicked_file(PathBuf::from("bad.feature"), &error);
        assert_eq!(result.failed(), 1);
        assert!(
            result.scenarios[0]
                .failure
                .as_deref()
                .is_some_and(|f| f.contains("panic")),
            "{:?}",
            result.scenarios[0].failure
        );
    }
}
