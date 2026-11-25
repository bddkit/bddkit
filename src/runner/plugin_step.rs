//! A step served by a plugin: one dispatch, or several while an armed
//! eventual-assertion modifier keeps `not_yet` answers retrying.

use super::{Args, DispatchRequest, Options, OptionsJson, Polling, Status};
use crate::failure::{FailureKind, StepError};
use crate::feature::ExpandedStep;
use crate::steps::Registry;
use crate::world::World;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Which step of which plugin library a plugin step is.
#[derive(Clone, Copy)]
pub(super) struct Target {
    pub(super) lib: usize,
    pub(super) step_index: usize,
    pub(super) assertion: bool,
}

/// What stays the same across the attempts of one plugin step.
struct PluginCall<'a> {
    plugins: Arc<crate::plugin::Plugins>,
    target: Target,
    args: &'a Args,
    group: String,
    instance: String,
    /// A `per_worker` instance belongs to this file, so this file both creates
    /// and drops it; a `shared` one is only borrowed.
    per_worker: bool,
    /// Resolved once per plugin step rather than per attempt: the path is
    /// constant for the file, and `Workspace` caches the `create_dir_all`
    /// after the first call.
    workspace_dir: String,
    source_dir: String,
    wants_vars: bool,
}

/// What changes across the attempts: the options in force, the layer they came
/// from, the polling that an armed modifier started and the attempts so far.
struct Retry<'a> {
    effective: Options,
    layer: Option<crate::options::OptionsLayer>,
    polling: Option<Polling<'a>>,
    attempts: u64,
}

/// One plugin reply, read once: everything it revealed is registered and its
/// output printed.
struct Reply {
    result: crate::plugin::abi::DispatchResult,
    shown: Vec<crate::plugin::reply::Shown>,
    blamed: Option<(usize, u64)>,
    class: Option<FailureKind>,
}

impl Reply {
    /// The failing reply's diagnostics ride on the error, apart from its text.
    fn failure(&self, world: &World, error: StepError) -> StepError {
        error
            .with_diagnostics(crate::plugin::reply::dumps(&self.shown, &world.redactor))
            .with_handle(self.blamed)
    }
}

/// A step served by a plugin, dispatched once, or again for as long as an
/// armed eventual-assertion modifier keeps its `not_yet` answers retrying.
pub(super) async fn run(
    world: &mut World,
    reg: &Registry,
    step: &ExpandedStep,
    source: &std::path::Path,
    target: Target,
    args: &Args,
) -> Result<(), StepError> {
    let Some(plugins) = world.plugins.plugins().cloned() else {
        return Err(StepError::plugin(
            "this step is served by a plugin, but no plugin is loaded",
        ));
    };
    let group = plugins
        .group_of_step(target.lib, target.step_index)
        .to_string();
    let instance = world
        .plugins
        .current(&group)
        .map_err(StepError::input)?
        .to_string();
    let base = plugins
        .options_for(&group, &instance)
        .map_err(StepError::input)?
        .clone();

    // Only an assertion consumes an armed eventual-assertion
    // modifier; an action leaves it for the assertion that follows.
    let layer = if target.assertion {
        world.take_options()
    } else {
        None
    };
    let effective = match &layer {
        Some(layer) => base.apply(layer).map_err(StepError::input)?,
        None => base,
    };
    let shown = reg.display_text(&step.text, &world.redactor);
    let polling = layer
        .as_ref()
        .map(|_| Polling::new(&shown, &effective.polling));
    let call = PluginCall {
        per_worker: plugins.is_per_worker(target.lib),
        workspace_dir: world
            .workspace_dir()
            .map_err(StepError::plugin)?
            .display()
            .to_string(),
        wants_vars: reg
            .plugin_step(target.lib, target.step_index)
            .is_some_and(|spec| spec.wants_vars),
        source_dir: crate::validate::source_dir(
            source.parent().unwrap_or(std::path::Path::new("")),
        ),
        plugins,
        target,
        args,
        group,
        instance,
    };
    let mut retry = Retry {
        effective,
        layer,
        polling,
        attempts: 0,
    };
    loop {
        let reply = plugin_attempt(world, &call, &retry).await?;
        if settle(world, &call, &reply, &mut retry).await? {
            return Ok(());
        }
    }
}

/// One dispatch of a plugin step and the reading of its reply.
async fn plugin_attempt(
    world: &mut World,
    call: &PluginCall<'_>,
    retry: &Retry<'_>,
) -> Result<Reply, StepError> {
    let Target {
        lib, step_index, ..
    } = call.target;
    let attempt = retry.attempts + 1;
    world
        .trace
        .dispatching(retry.polling.is_some().then_some(attempt));
    let dispatch_dir = world.artifacts.next_dir();
    let request = serde_json::to_string(&DispatchRequest {
        instance: &call.instance,
        args: &call.args.caps,
        docstring: call.args.docstring.as_ref(),
        table: call.args.table.as_ref(),
        artifacts_dir: dispatch_dir.display().to_string(),
        workspace_dir: call.workspace_dir.clone(),
        debug: world.debug,
        options: OptionsJson::from(&retry.effective),
        evidence: world.trace.evidence_on(),
        vars: if call.wants_vars {
            crate::plugin::request_vars(&world.vars, &world.redactor)
        } else {
            BTreeMap::default()
        },
        attempt,
        source_dir: call.source_dir.clone(),
    })
    .map_err(|error| StepError::plugin(format!("failed to encode the plugin request: {error}")))?;

    // The FFI call is synchronous and may block for seconds;
    // spawn_blocking keeps it off the executor. Passing the
    // host's tokio Handle across the boundary instead would
    // pin plugin and host to one tokio version.
    let plugins_for_call = Arc::clone(&call.plugins);
    let (call_group, call_instance) = (call.group.clone(), call.instance.clone());
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
        .handle_for(&call.group, &call.instance)
        .filter(|(l, _)| *l == lib)
        .map(|(_, h)| h);
    let (handle, mut result) = tokio::task::spawn_blocking(move || {
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
    .map_err(|error| StepError::plugin(format!("the plugin dispatch task failed: {error}")))?
    .map_err(StepError::plugin)?;
    world
        .plugins
        .record(&call.group, &call.instance, lib, handle, call.per_worker);
    // Everything this reply reveals is registered before anything
    // else is read from it, on any status and in a fixed order
    // (`plugin::reply::absorb`): the error, a diagnostic or a
    // `not_yet` message may quote it.
    let blamed = Some((lib, handle));
    let shown = crate::plugin::reply::absorb(&result, &world.redactor, &call.instance)
        .map_err(|why| StepError::plugin(why).with_handle(blamed))?;
    // Read before anything is written, so a reply whose `output` is
    // refused changes nothing and its diagnostics still show.
    let output = crate::plugin::reply::outputs(&result).map_err(|why| {
        StepError::plugin(why)
            .with_diagnostics(crate::plugin::reply::dumps(&shown, &world.redactor))
            .with_handle(blamed)
    })?;
    if matches!(result.status, Status::Passed) {
        // Variables are published only on success: an
        // intermediate observation must not leak into the
        // scenario.
        crate::plugin::reply::publish_vars(world, &mut result);
    }
    // Printed once per reply, after everything it revealed is
    // registered, its variables included: a passing reply may
    // quote a value it just stored under a sensitive name. A
    // polled step prints once per attempt.
    crate::plugin::reply::print_output(&output, &world.redactor);
    let class = if matches!(result.status, Status::Passed) {
        None
    } else {
        let (class, note) = crate::plugin::reply::class(&result, &call.plugins);
        world.warnings.extend(note);
        class
    };
    // Whatever the status: the console shows diagnostics only of
    // a failure, the stream of every reply.
    for diagnostic in &shown {
        let notes = diagnostic.emit(&world.trace, &call.group, &call.instance, &dispatch_dir);
        world.warnings.extend(notes);
    }
    Ok(Reply {
        result,
        shown,
        blamed,
        class,
    })
}

/// What a reply means for the step: `Ok(true)` it passed, `Err` it failed and
/// `Ok(false)` it answered `not_yet` and the polling waited for another attempt.
async fn settle(
    world: &mut World,
    call: &PluginCall<'_>,
    reply: &Reply,
    retry: &mut Retry<'_>,
) -> Result<bool, StepError> {
    let class = reply.class;
    let message = reply.result.message();
    match reply.result.status {
        Status::Passed => Ok(true),
        Status::Fatal => Err(reply.failure(
            world,
            StepError::new(class.unwrap_or(FailureKind::Plugin), message),
        )),
        Status::NotYet if !call.target.assertion => Err(reply.failure(
            world,
            StepError::new(
                class.unwrap_or(FailureKind::Plugin),
                format!(
                    "a plugin action answered not_yet, which only an assertion may do: {message}"
                ),
            ),
        )),
        Status::NotYet => {
            let error = reply.failure(
                world,
                StepError::new(class.unwrap_or(FailureKind::Assertion), message),
            );
            // Without an armed modifier there is no second
            // attempt, so not_yet is simply a failure.
            if retry.polling.is_none() {
                return Err(error);
            }
            // `options_instance` is read from the first `not_yet` only.
            if retry.attempts == 0
                && let Some(name) = &reply.result.options_instance
            {
                // Before the first wait, so it already sleeps
                // this instance's interval.
                let base = call
                    .plugins
                    .options_for(&call.group, name)
                    .map_err(|e| reply.failure(world, StepError::plugin(e)))?;
                retry.effective = match &retry.layer {
                    Some(layer) => base.apply(layer),
                    None => Ok(base.clone()),
                }
                .map_err(|e| reply.failure(world, StepError::plugin(e)))?;
                if let Some(polling) = &mut retry.polling {
                    polling.retarget(&retry.effective.polling);
                }
            }
            retry.attempts += 1;
            world.trace.attempt(retry.attempts, &error.message);
            if let Some(polling) = &mut retry.polling {
                polling.after_not_yet(&error).await?;
            }
            Ok(false)
        }
    }
}
