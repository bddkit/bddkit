use crate::events::{Events, Origin, Trace};
use crate::failure::{FailureKind, StepError};
use crate::feature::{ExpandedStep, LoadedFeature, ScenarioRef, expand_outlines};
use crate::options::Options;
use crate::plugin::abi::{DispatchRequest, OptionsJson, Status};
use crate::polling::{AttemptError, Polling};
use crate::report::render_file;
use crate::report::{FileResult, ReportTag, ScenarioResult, StepResult, StepStatus};
use crate::steps::{Args, OptionsSource, Registry, StepId, StepKind, StepTarget, dispatch};
use crate::unique::Generator;
use crate::vars::{VarStack, interpolate, interpolate_with};
use crate::world::World;
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// What a step's slots resolved to, as `step_started.vars` shows them:
/// `[expression, value]`. A value is `***` in a `secret` parameter or under a
/// sensitive name — by position, so a short one is hidden too — and the SQL
/// NULL is the JSON `null`; the rest goes through the file's set.
fn shown_slots(
    slots: Vec<(String, String)>,
    secret: bool,
    vars: &VarStack,
    redactor: &crate::privacy::Redactor,
    out: &mut Vec<Value>,
) {
    for (expression, value) in slots {
        let name = expression.split('(').next().unwrap_or_default();
        let shown = if secret || vars.is_secret(name) {
            redactor.show(true, &value).into()
        } else if value == crate::vars::NULL_SENTINEL {
            Value::Null
        } else {
            redactor.shown(name, &value).into()
        };
        out.push(json!([expression, shown]));
    }
}

/// The interpolated arguments of a step and, beside them, the slots that
/// produced them — from the same pass, because `<<unique()>>` would draw a new
/// value on a second one.
fn prepare(
    step: &ExpandedStep,
    caps: &[String],
    params: &[crate::steps::expression::Param],
    vars: &VarStack,
    redactor: &crate::privacy::Redactor,
    generator: &Generator,
    raw_docstring: bool,
) -> Result<(Args, Vec<Value>), String> {
    let mut shown = Vec::new();
    // Substitution applies to arguments, the doc string, and table cells —
    // never to the whole step text.
    let mut resolved = Vec::with_capacity(caps.len());
    let mut cap_slots = Vec::with_capacity(caps.len());
    for c in caps {
        let mut slots = Vec::new();
        resolved.push(interpolate_with(c, vars, generator, &mut slots)?);
        cap_slots.push(slots);
    }
    let caps = resolved;
    // A typed parameter matched either a literal of its type or a whole
    // `<<…>>` slot. Only a slot's value can still be the wrong type, and this
    // is the first point where that value exists (invariant 1).
    for (param, value) in params.iter().zip(&caps) {
        param.check(value)?;
        if param.ty.secret {
            redactor.secret(value);
        }
    }
    // After every secret parameter is registered: a value given to one
    // parameter and quoted by another is hidden in both.
    for (i, slots) in cap_slots.into_iter().enumerate() {
        let secret = params.get(i).is_some_and(|param| param.ty.secret);
        shown_slots(slots, secret, vars, redactor, &mut shown);
    }
    let docstring = if raw_docstring {
        step.docstring.clone()
    } else {
        let mut slots = Vec::new();
        let doc = step
            .docstring
            .as_ref()
            .map(|d| interpolate_with(d, vars, generator, &mut slots))
            .transpose()?;
        shown_slots(slots, false, vars, redactor, &mut shown);
        doc
    };
    let mut slots = Vec::new();
    let table = step
        .table
        .as_ref()
        .map(|rows| {
            rows.iter()
                .map(|r| {
                    r.iter()
                        .map(|c| interpolate_with(c, vars, generator, &mut slots))
                        .collect::<Result<Vec<_>, _>>()
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    shown_slots(slots, false, vars, redactor, &mut shown);
    Ok((
        Args {
            caps,
            docstring,
            table,
        },
        shown,
    ))
}

/// A step matched against the registry, and its arguments interpolated — done
/// before `step_started` so the event can carry the slots. `Include` is the
/// one step that interpolates nothing here: it reads its raw captures.
struct Resolved {
    target: StepTarget,
    caps: Vec<String>,
    args: Option<Args>,
    vars: Vec<Value>,
}

fn resolve(
    world: &World,
    reg: &Registry,
    step: &ExpandedStep,
    generator: &Generator,
    depth: usize,
) -> Result<Resolved, String> {
    let Some((target, caps, params)) = reg.find_with_params(&step.text)? else {
        return Err("unknown step".into());
    };
    if matches!(target, StepTarget::Macro(_)) {
        if step.docstring.is_some() {
            return Err("a macro call does not support a docstring".into());
        }
        if step.table.is_some() {
            return Err("a macro call does not support a table".into());
        }
        if depth >= 16 {
            return Err("macro nesting exceeds 16".into());
        }
    }
    let is_include = matches!(
        target,
        StepTarget::Builtin {
            id: StepId::Include | StepId::IncludeScenario,
            ..
        }
    );
    // A plugin step may ask for its doc string untouched: a script is code,
    // and a value spliced into code can change what the code is.
    let raw_docstring = matches!(
        target,
        StepTarget::Plugin { lib, step, .. }
            if reg.plugin_step(lib, step).is_some_and(|spec| spec.raw_docstring)
    );
    let interpolated = (!is_include)
        .then(|| {
            prepare(
                step,
                &caps,
                params,
                &world.vars,
                &world.redactor,
                generator,
                raw_docstring,
            )
        })
        .transpose()?;
    let (args, vars) = interpolated.unzip();
    Ok(Resolved {
        target,
        caps,
        args,
        vars: vars.unwrap_or_default(),
    })
}

/// Every step at every depth goes through here, so the event stream sees the
/// ones inside a macro or an include too. The reports do not: they show the
/// caller as one step.
fn execute_step<'a>(
    world: &'a mut World,
    reg: &'a Registry,
    step: &'a ExpandedStep,
    source: &'a std::path::Path,
    generator: &'a Generator,
    depth: usize,
    origin: Origin<'a>,
) -> Pin<Box<dyn Future<Output = Result<(), StepError>> + Send + 'a>> {
    Box::pin(async move {
        let started = Instant::now();
        let warned = world.warnings.len();
        let mut resolved = resolve(world, reg, step, generator, depth);
        let vars = resolved
            .as_mut()
            .map(|r| std::mem::take(&mut r.vars))
            .unwrap_or_default();
        // A macro call and an include are not counted: the steps they run are.
        let leaf_group: Option<std::borrow::Cow<'static, str>> =
            resolved.as_ref().ok().and_then(|r| match r.target {
                StepTarget::Macro(_)
                | StepTarget::Builtin {
                    id: StepId::Include | StepId::IncludeScenario,
                    ..
                } => None,
                StepTarget::Builtin { id, .. } => Some(crate::steps::group_of(id).into()),
                StepTarget::Plugin { lib, step, .. } => {
                    Some(world.plugins.usage_group(lib, step).to_string().into())
                }
            });
        world.trace.step_started(
            &step.keyword,
            &reg.display_text(&step.text, &world.redactor),
            origin,
            source,
            vars,
        );
        let result = match resolved {
            Ok(resolved) => run_step(world, reg, step, source, generator, depth, resolved).await,
            // Everything `resolve` refuses is the scenario's own text: an
            // unknown step, a variable that is not set, a value of the wrong type.
            Err(error) => Err(StepError::input(error)),
        };
        if let (Some(group), Some(counters)) = (&leaf_group, &world.counters) {
            counters.leaf(group, started.elapsed());
        }
        // Still the innermost step in flight: this is whose exchanges they are.
        world.flush_evidence();
        world.trace.step_finished(
            result.as_ref().err().map(|error| error.kind),
            started.elapsed(),
            &world.warnings[warned..],
        );
        result
    })
}

async fn run_step(
    world: &mut World,
    reg: &Registry,
    step: &ExpandedStep,
    source: &std::path::Path,
    generator: &Generator,
    depth: usize,
    resolved: Resolved,
) -> Result<(), StepError> {
    let Resolved {
        target, caps, args, ..
    } = resolved;
    match target {
        StepTarget::Builtin { id, kind } => {
            if matches!(id, StepId::Include | StepId::IncludeScenario) {
                return run_include(world, reg, step, id, caps, source, generator, depth).await;
            }
            let args = args.expect("resolve interpolates every step but an include");
            if id == StepId::AttachFile {
                // Needs the directory of the file the step is written in,
                // which `dispatch` does not carry — same reason as Include.
                let base_dir = source.parent().unwrap_or(std::path::Path::new("."));
                return crate::steps::api::attach_file(world, base_dir, args.cap(0), args.cap(1));
            }
            match kind {
                StepKind::Action => dispatch(world, id, &args, 0)
                    .await
                    .map_err(AttemptError::into_error),
                StepKind::Assertion(source_kind) => {
                    let Some(layer) = world.take_options() else {
                        return dispatch(world, id, &args, 0)
                            .await
                            .map_err(AttemptError::into_error);
                    };
                    let base = match source_kind {
                        OptionsSource::Global => world.options.clone(),
                        OptionsSource::Http => world
                            .http
                            .options_for_last_response()
                            .map_err(StepError::input)?
                            .clone(),
                        OptionsSource::Db => world.db.options().map_err(StepError::input)?.clone(),
                    };
                    let effective = base.apply(&layer).map_err(StepError::input)?;
                    let shown = reg.display_text(&step.text, &world.redactor);
                    let mut polling = Polling::new(&shown, &effective.polling);
                    let mut attempt = 0;
                    loop {
                        let outcome = dispatch(world, id, &args, attempt).await;
                        // A replay stored an exchange: it is evidence before
                        // the attempt it belongs to is reported.
                        world.flush_evidence();
                        match outcome {
                            Ok(()) => return Ok(()),
                            Err(AttemptError::Fatal(error)) => return Err(error),
                            Err(AttemptError::NotYet(error)) => {
                                world.trace.attempt(attempt + 1, &error.message);
                                polling.after_not_yet(&error).await?;
                                attempt += 1;
                            }
                        }
                    }
                }
            }
        }
        StepTarget::Macro(index) => {
            let definition = reg.macro_def(index);
            let args = args.expect("resolve interpolates every step but an include");
            world.vars.push_frame();
            for (param, value) in definition.compiled.params.iter().zip(args.caps) {
                world.vars.set(&param.name, value);
                if param.ty.secret {
                    world.vars.mark_secret(&param.name);
                }
            }
            if world.debug {
                debug_eprintln!(
                    world,
                    "macro {:?}",
                    reg.display_text(&step.text, &world.redactor)
                );
            }

            // A macro body resolves ITS OWN includes relative to the
            // macro's own source file, never the caller's — the same
            // rule the spec states for `I include`.
            let macro_source = definition.source.clone();
            for (index, body_step) in definition.body.iter().enumerate() {
                let expanded = ExpandedStep {
                    keyword: String::new(),
                    text: body_step.text.clone(),
                    line: step.line,
                    docstring: body_step.docstring.clone(),
                    table: None,
                };
                // The definition's line and the step's position in the body:
                // `MacroStep` has no line of its own.
                let origin = Origin {
                    line: definition.line,
                    index: Some(index),
                    comment: None,
                };
                if let Err(error) = execute_step(
                    world,
                    reg,
                    &expanded,
                    &macro_source,
                    generator,
                    depth + 1,
                    origin,
                )
                .await
                {
                    world.vars.pop_frame(&[]).map_err(StepError::input)?;
                    // The inner step's class stands; the macro adds context.
                    return Err(error.wrap(|message| {
                        format!(
                            "  {}\n{message}",
                            reg.display_text(&body_step.text, &world.redactor)
                        )
                    }));
                }
            }
            let exported = world
                .vars
                .pop_frame(&definition.exports)
                .map_err(StepError::input)?;
            if world.debug {
                for (k, v) in &exported {
                    debug_eprintln!(
                        world,
                        "  export {k} = {}",
                        world.redactor.shown(k, debug_display(v))
                    );
                }
            }
            Ok(())
        }
        StepTarget::Plugin {
            lib,
            step: step_index,
            assertion,
        } => {
            // Arguments cross the boundary already interpolated: a plugin
            // never sees raw step text and never sees `<<variable>>`
            // syntax (invariant 1, restated at the FFI boundary).
            let args = args.expect("resolve interpolates every step but an include");
            let Some(plugins) = world.plugins.plugins().cloned() else {
                return Err(StepError::plugin(
                    "this step is served by a plugin, but no plugin is loaded",
                ));
            };
            let group = plugins.group_of_step(lib, step_index).to_string();
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
            let layer = if assertion {
                world.take_options()
            } else {
                None
            };
            let effective = match &layer {
                Some(layer) => base.apply(layer).map_err(StepError::input)?,
                None => base,
            };
            let shown = reg.display_text(&step.text, &world.redactor);
            let mut polling = layer
                .as_ref()
                .map(|_| Polling::new(&shown, &effective.polling));
            // A `per_worker` instance belongs to this file, so this file
            // both creates and drops it; a `shared` one is only borrowed.
            let per_worker = plugins.is_per_worker(lib);

            // Resolved once per plugin step rather than per attempt: the
            // path is constant for the file, and `Workspace` caches the
            // `create_dir_all` after the first call.
            let workspace_dir = world
                .workspace_dir()
                .map_err(StepError::plugin)?
                .display()
                .to_string();

            let wants_vars = reg
                .plugin_step(lib, step_index)
                .is_some_and(|spec| spec.wants_vars);

            let mut attempts = 0;
            loop {
                let dispatch_dir = world.artifacts.next_dir();
                let request = serde_json::to_string(&DispatchRequest {
                    args: &args.caps,
                    docstring: args.docstring.as_ref(),
                    table: args.table.as_ref(),
                    artifacts_dir: dispatch_dir.display().to_string(),
                    workspace_dir: workspace_dir.clone(),
                    debug: world.debug,
                    options: OptionsJson::from(&effective),
                    evidence: world.trace.evidence_on(),
                    vars: if wants_vars {
                        crate::plugin::request_vars(&world.vars, &world.redactor)
                    } else {
                        Default::default()
                    },
                })
                .map_err(|error| {
                    StepError::plugin(format!("failed to encode the plugin request: {error}"))
                })?;

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
                .map_err(|error| {
                    StepError::plugin(format!("the plugin dispatch task failed: {error}"))
                })?
                .map_err(StepError::plugin)?;
                world
                    .plugins
                    .record(&group, &instance, lib, handle, per_worker);
                // Every secret this reply carries is registered before
                // anything else is read from it, on any status: the error,
                // a diagnostic or a `not_yet` message may quote one.
                let mut result = result;
                for secret in &result.secrets {
                    world.redactor.secret(secret);
                }
                // A diagnostic may also quote a value another one, or a
                // published variable, marks as secret.
                if matches!(result.status, Status::Passed) {
                    // Variables are published only on success: an
                    // intermediate observation must not leak into the
                    // scenario.
                    for (name, value) in std::mem::take(&mut result.vars) {
                        world.vars.set(&name, value);
                    }
                }
                for diagnostic in &result.diagnostics {
                    if diagnostic.kind == "json"
                        && let Some(content) = &diagnostic.content
                    {
                        world.redactor.register(&world.redactor.json_hits(content));
                    }
                }
                // Whatever the status: the console shows diagnostics only of
                // a failure, the stream of every reply.
                for diagnostic in &result.diagnostics {
                    let notes =
                        world
                            .trace
                            .diagnostic(&group, &instance, &dispatch_dir, diagnostic);
                    world.warnings.extend(notes);
                }

                match result.status {
                    Status::Passed => return Ok(()),
                    Status::Fatal => {
                        return Err(StepError::plugin(result.render_failure(&world.redactor)));
                    }
                    Status::NotYet if !assertion => {
                        return Err(StepError::plugin(format!(
                            "a plugin action answered not_yet, which only an assertion may do: {}",
                            result.render_failure(&world.redactor)
                        )));
                    }
                    Status::NotYet => match &mut polling {
                        // Without an armed modifier there is no second
                        // attempt, so not_yet is simply a failure.
                        None => {
                            return Err(StepError::assertion(
                                result.render_failure(&world.redactor),
                            ));
                        }
                        Some(polling) => {
                            let error =
                                StepError::assertion(result.render_failure(&world.redactor));
                            attempts += 1;
                            world.trace.attempt(attempts, &error.message);
                            polling.after_not_yet(&error).await?
                        }
                    },
                }
            }
        }
    }
}

/// Runs `I include "<file>"` / `I include "<file>" scenario "<name>"`. The
/// caller's `World` is reused in place — HTTP state, the current API
/// resource, the DB connection, debug mode and every plugin instance carry
/// over in both directions, because `reset_scenario` is never called here.
/// Only `world.vars` is swapped for a fresh, isolated `VarStack`: the spec
/// requires the caller's globals to be invisible inside the include, which a
/// pushed frame (as macros use) would not give — `get` still falls back to
/// `globals` from any frame, so isolation needs the whole stack replaced,
/// not just a new top frame.
#[allow(clippy::too_many_arguments)]
fn run_include<'a>(
    world: &'a mut World,
    reg: &'a Registry,
    caller_step: &'a ExpandedStep,
    id: StepId,
    caps: Vec<String>,
    source: &'a std::path::Path,
    generator: &'a Generator,
    depth: usize,
) -> Pin<Box<dyn Future<Output = Result<(), StepError>> + Send + 'a>> {
    Box::pin(async move {
        if caller_step.docstring.is_some() {
            return Err(StepError::input("I include does not support a docstring"));
        }
        if depth >= 16 {
            return Err(StepError::input("include nesting exceeds 16"));
        }

        let path_literal = &caps[0];
        // The prefix is the last capture of the `with prefix "…"` twin of each
        // include step and absent from the other one (an optional may not hold
        // a parameter, so the two are separate declarations).
        let (scenario_name, prefix_raw) = if id == StepId::IncludeScenario {
            (
                Some(caps[1].as_str()),
                caps.get(2).map_or("", String::as_str),
            )
        } else {
            (None, caps.get(1).map_or("", String::as_str))
        };
        let prefix = (!prefix_raw.is_empty()).then_some(prefix_raw);

        let base_dir = source
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("."));

        crate::include::check_literal(path_literal).map_err(StepError::input)?;
        let resolved =
            crate::include::resolve(path_literal, &base_dir).map_err(StepError::input)?;
        let included =
            crate::feature::load(&resolved).map_err(|e| StepError::input(e.to_string()))?;
        let ScenarioRef { scenario, rule } =
            crate::include::select_scenario(&included, scenario_name).map_err(StepError::input)?;
        let exports = crate::feature::exports_of(&included, scenario).map_err(StepError::input)?;

        // build_scenario validates the with: table's shape (exactly one data row) —
        // do this BEFORE extracting values, so there is one authoritative shape
        // check, not two independently-written ones with different error messages.
        let expanded = crate::include::build_scenario(scenario, caller_step.table.as_deref())
            .map_err(StepError::input)?;

        // `with:` cells are interpolated in the CALLER's scope, before the swap.
        // The shape (exactly one data row) is already guaranteed by build_scenario above.
        let with_values: Vec<(String, String)> = match &caller_step.table {
            Some(table) => {
                let [header, row] = table.as_slice() else {
                    unreachable!("build_scenario already validated exactly one data row")
                };
                let mut out = Vec::with_capacity(header.len());
                for (name, raw) in header.iter().zip(row.iter()) {
                    let value =
                        interpolate(raw, &world.vars, generator).map_err(StepError::input)?;
                    out.push((name.clone(), value));
                }
                out
            }
            None => Vec::new(),
        };

        if world.debug {
            debug_eprintln!(
                world,
                "include {} › {:?}",
                crate::feature::display_path(&resolved),
                expanded.name
            );
            for (k, v) in &with_values {
                debug_eprintln!(
                    world,
                    "  with {k} = {}",
                    world.redactor.shown(k, debug_display(v))
                );
            }
        }

        let background = included.background_steps(rule);

        let mut fresh = VarStack::new();
        fresh.set_redactor(world.vars.redactor());
        let saved = std::mem::replace(&mut world.vars, fresh);
        for (name, value) in &with_values {
            world.vars.set(name, value.clone());
        }

        let annotations =
            crate::annotate::scenario(&included, ScenarioRef { scenario, rule }, &expanded, reg);
        let mut run_result: Result<(), StepError> = Ok(());
        for item in annotations.execution(background.iter().chain(expanded.steps.iter())) {
            let (step, comment) = match item {
                crate::annotate::ExecutionItem::Step { step, comment } => (step, comment),
                crate::annotate::ExecutionItem::Annotation(item) => {
                    world.trace.annotation(item, &resolved, reg);
                    continue;
                }
            };
            if run_result.is_err() {
                world.trace.step_skipped(
                    &step.keyword,
                    &reg.display_text(&step.text, &world.redactor),
                    step.line,
                    &resolved,
                    comment,
                );
                continue;
            }
            let origin = Origin {
                line: step.line,
                index: None,
                comment,
            };
            if let Err(e) =
                execute_step(world, reg, step, &resolved, generator, depth + 1, origin).await
            {
                run_result = Err(e.wrap(|message| {
                    format!(
                        "{}:{} → {}:{}\n  {}\n{message}",
                        crate::feature::display_path(source),
                        caller_step.line,
                        crate::feature::display_path(&resolved),
                        step.line,
                        reg.display_text(&step.text, &world.redactor)
                    )
                }));
            }
        }

        // The call-site `with prefix "<p>"` renames every exported variable to
        // `<p>_<name>` as it crosses back into the caller's scope — this is
        // what lets two includes of the same flow (e.g. registering a buyer
        // and a seller) coexist without the second overwriting the first.
        let apply_prefix = |name: String| match prefix {
            Some(p) => format!("{p}_{name}"),
            None => name,
        };
        let export_result = if run_result.is_ok() {
            let mut resolved_exports = Vec::new();
            let mut missing = None;
            for name in &exports {
                if let Some(glob_prefix) = name.strip_suffix('*') {
                    for (k, v) in world.vars.all_vars() {
                        if k.starts_with(glob_prefix) {
                            resolved_exports.push((apply_prefix(k), v));
                        }
                    }
                } else if let Some(v) = world.vars.get(name) {
                    resolved_exports.push((apply_prefix(name.clone()), v.to_string()));
                } else {
                    missing = Some(StepError::input(format!(
                        "I include {path_literal:?}: declared export {name:?}, but the variable is not set"
                    )));
                    break;
                }
            }
            match missing {
                Some(e) => Err(e),
                None => Ok(resolved_exports),
            }
        } else {
            Ok(Vec::new())
        };

        if world.debug
            && let Ok(exported) = &export_result
        {
            for (k, v) in exported {
                debug_eprintln!(
                    world,
                    "  export {k} = {}",
                    world.redactor.shown(k, debug_display(v))
                );
            }
        }

        world.vars = saved;

        run_result?;
        let exported = export_result?;
        for (name, value) in exported {
            world.vars.set(&name, value);
        }
        Ok(())
    })
}

/// The SQL-NULL sentinel prints as `<<null>>` in debug output, never the raw
/// NUL bytes — the same rule `steps/db.rs`'s debug lines already follow.
fn debug_display(value: &str) -> &str {
    if value == crate::vars::NULL_SENTINEL {
        "<<null>>"
    } else {
        value
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
    pub artifacts: Arc<crate::artifacts::Artifacts>,
    /// `--events`; `None` costs each emit site one branch.
    pub events: Option<Events>,
    /// `--evidence`: the stream also carries what steps resolved, sent and read.
    pub evidence: crate::events::Sources,
    /// `$BDDKIT_EVIDENCE_BUFFER`: bytes of long deferred text a scenario holds
    /// in memory before it goes to disk.
    pub evidence_buffer: usize,
    /// The run-wide privacy vocabulary and the secrets known before the first
    /// request; each file forks it. `main` sets it; masks nothing until then.
    pub redactor: crate::privacy::Redactor,
    /// Leaf steps per group and outline rows, for the telemetry payload.
    pub counters: Arc<crate::telemetry::counters::Counters>,
    fail_fast: bool,
    stop: std::sync::atomic::AtomicBool,
    ending: std::sync::atomic::AtomicBool,
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
        artifacts: Arc<crate::artifacts::Artifacts>,
        fail_fast: bool,
    ) -> Self {
        let official = plugins.iter().flat_map(|p| p.official_groups());
        let counters = Arc::new(crate::telemetry::counters::Counters::new(
            crate::telemetry::builtin_groups().chain(official),
        ));
        Self {
            counters,
            reg,
            apis,
            generator,
            filter,
            db,
            default_db,
            srp,
            plugins,
            options,
            artifacts,
            events: None,
            evidence: crate::events::Sources::default(),
            evidence_buffer: crate::events::EVIDENCE_BUFFER,
            redactor: crate::privacy::Redactor::disabled(),
            fail_fast,
            stop: std::sync::atomic::AtomicBool::new(false),
            ending: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// First caller wins, once per run. The interrupt handler takes it when its
    /// signal arrives and `run` takes it when the pool has drained; whoever
    /// loses leaves the exit — the exit code, the plugin shutdown and the last
    /// line of the event stream — to the winner, so the three always agree.
    pub fn claim_end(&self) -> bool {
        !self.ending.swap(true, Ordering::SeqCst)
    }

    /// The end is claimed while a file still runs only by the interrupt
    /// handler: `run` claims it after the pool has drained.
    pub fn interrupted(&self) -> bool {
        self.ending.load(Ordering::SeqCst)
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

    /// Arms the stop unconditionally, ignoring `--fail-fast`. Used by the
    /// interrupt handler: an operator's Ctrl-C must stop new work regardless
    /// of that flag, unlike a scenario failure's `request_stop`.
    pub fn force_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
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
        ctx.artifacts.clone(),
    );
    let redactor = ctx.redactor.fork();
    world.trace = Trace::new(ctx.events.clone(), &lf, redactor.clone());
    world.set_redactor(redactor);
    world.collect_evidence(ctx.evidence, ctx.evidence_buffer);
    world.counters = Some(ctx.counters.clone());
    let mut scenarios = Vec::new();

    // The generator handle is cloned once: inside the loop `&world.generator`
    // would conflict with the later `&mut world` in the dispatch call.
    let generator = world.generator.clone();

    for scenario_ref in lf.scenarios() {
        let ScenarioRef { scenario: sc, rule } = scenario_ref;
        let background = lf.background_steps(rule);
        for ex in expand_outlines(sc) {
            if !ctx.filter.selects_row(sc, ex.examples_line) {
                continue;
            }
            if ex.example.is_some() {
                ctx.counters.example();
            }
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
            let mut failure: Option<(String, FailureKind)> = match ctx.plugins.clone() {
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
                            Ok(result) => result.err().map(|error| {
                                let text = format!("  scenario reset\n{error}");
                                (world.redactor.mask(&text), FailureKind::Reset)
                            }),
                            Err(error) => {
                                let text = format!(
                                    "  scenario reset\nthe plugin reset task failed: {error}"
                                );
                                Some((world.redactor.mask(&text), FailureKind::Reset))
                            }
                        }
                    }
                }
            };

            world.trace.scenario_started(&lf, scenario_ref, &ex);
            let started = Instant::now();
            let mut steps = Vec::with_capacity(background.len() + ex.steps.len());
            let annotations = crate::annotate::scenario(&lf, scenario_ref, &ex, &ctx.reg);
            for item in annotations.execution(background.iter().chain(ex.steps.iter())) {
                let (step, comment) = match item {
                    crate::annotate::ExecutionItem::Step { step, comment } => (step, comment),
                    crate::annotate::ExecutionItem::Annotation(item) => {
                        world.trace.annotation(item, &lf.path, &ctx.reg);
                        continue;
                    }
                };
                let step_started = Instant::now();
                // Everything after a failure is skipped, not run: the report
                // lists every step of the scenario either way.
                let origin = Origin {
                    line: step.line,
                    index: None,
                    comment,
                };
                let status = if failure.is_some() {
                    world.trace.step_skipped(
                        &step.keyword,
                        &ctx.reg.display_text(&step.text, &world.redactor),
                        step.line,
                        &lf.path,
                        comment,
                    );
                    StepStatus::Skipped
                } else if let Err(e) =
                    execute_step(&mut world, &ctx.reg, step, &lf.path, &generator, 0, origin).await
                {
                    let mut msg = world.redactor.mask(&format!(
                        "  {}\n{}",
                        ctx.reg.display_text(&step.text, &world.redactor),
                        e.message
                    ));
                    if let Some(ex) = world.http.last() {
                        msg.push_str(&format!("\n\n{}", ex.render(&world.redactor)));
                    }
                    failure = Some((msg, e.kind));
                    StepStatus::Failed
                } else {
                    StepStatus::Passed
                };
                steps.push(StepResult {
                    keyword: step.keyword.clone(),
                    text: ctx
                        .reg
                        .display_text(&step.text, &world.redactor)
                        .into_owned(),
                    line: step.line,
                    status,
                    duration: step_started.elapsed(),
                    warnings: std::mem::take(&mut world.warnings)
                        .iter()
                        .map(|warning| world.redactor.mask(warning))
                        .collect(),
                });
            }
            let duration = started.elapsed();
            world.trace.scenario_finished(
                failure.as_ref().map(|(text, kind)| (text.as_str(), *kind)),
                duration,
                ctx.interrupted(),
            );
            let (failure, failure_kind) = match failure {
                Some((text, kind)) => (Some(text), Some(kind)),
                None => (None, None),
            };
            scenarios.push(ScenarioResult {
                name: world.redactor.mask(&ex.name),
                line: ex.line,
                description: world.redactor.mask(lf.description(ex.line).unwrap_or("")),
                tags: report_tags(
                    &world.redactor,
                    lf.effective_tags(scenario_ref, ex.examples_line),
                ),
                failure,
                failure_kind,
                steps,
                duration,
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

    // A value can join the set AFTER the step that mentions it was recorded (a
    // header set in one step, sent in the next), so the texts are masked once
    // more with the file's final set.
    for scenario in &mut scenarios {
        scenario.name = world.redactor.mask(&scenario.name);
        scenario.description = world.redactor.mask(&scenario.description);
        for tag in &mut scenario.tags {
            tag.name = world.redactor.mask(&tag.name);
        }
        scenario.failure = scenario
            .failure
            .take()
            .map(|text| world.redactor.mask(&text));
        for step in &mut scenario.steps {
            step.text = world.redactor.mask(&step.text);
            step.warnings = step
                .warnings
                .iter()
                .map(|w| world.redactor.mask(w))
                .collect();
        }
    }

    file_result(&lf, &world.redactor, scenarios)
}

/// `(name, line)` pairs as a report shows them: with the `@`, and masked.
fn report_tags<'a>(
    redactor: &crate::privacy::Redactor,
    tags: impl IntoIterator<Item = (&'a str, usize)>,
) -> Vec<ReportTag> {
    tags.into_iter()
        .map(|(name, line)| ReportTag {
            name: redactor.mask(&format!("@{name}")),
            line,
        })
        .collect()
}

/// The file's result, with the feature's own description and tags masked by
/// `redactor` as it stands now.
fn file_result(
    lf: &LoadedFeature,
    redactor: &crate::privacy::Redactor,
    scenarios: Vec<ScenarioResult>,
) -> FileResult {
    let line = lf.feature.position.line;
    FileResult {
        path: lf.path.clone(),
        name: lf.feature.name.clone(),
        line,
        description: redactor.mask(lf.description(line).unwrap_or("")),
        tags: report_tags(
            redactor,
            lf.own_tags(line)
                .iter()
                .map(|(name, line)| (name.as_str(), *line)),
        ),
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
fn panicked_file(
    lf: &LoadedFeature,
    error: &tokio::task::JoinError,
    redactor: &crate::privacy::Redactor,
) -> FileResult {
    file_result(
        lf,
        redactor,
        vec![ScenarioResult {
            name: "file run aborted".to_string(),
            line: 0,
            description: String::new(),
            tags: Vec::new(),
            // The file's own set is gone with its `World`: the run-wide one is
            // what is left to mask a payload with.
            failure: Some(redactor.mask(&format!("panic while running the file: {error}"))),
            failure_kind: Some(FailureKind::Panic),
            steps: Vec::new(),
            duration: std::time::Duration::ZERO,
        }],
    )
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
                    let task = tokio::spawn(run_file(lf.clone(), ctx.clone()));
                    let (result, panicked) = match task.await {
                        Ok(r) => (r, false),
                        Err(error) => (panicked_file(lf, &error, &ctx.redactor), true),
                    };
                    // Here, not in `run_file`, so a file that panicked is closed
                    // in the stream too — and any scenario or step it left open
                    // with it.
                    if let Some(events) = &ctx.events {
                        events.emit(
                            "file_finished",
                            serde_json::json!({
                                "file": crate::feature::display_path(&result.path),
                                "scenarios": result.scenarios.len(),
                                "failed": result.failed(),
                                "panicked": panicked,
                            }),
                        );
                    }
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
    use crate::macros::MacroCatalog;
    use crate::unique::UniqueKind;
    use std::path::PathBuf;

    fn apis() -> Arc<crate::http::Apis> {
        apis_at("http://example.test")
    }

    fn apis_at(base: &str) -> Arc<crate::http::Apis> {
        let mut by_name = std::collections::HashMap::new();
        by_name.insert(
            "default".to_string(),
            crate::http::ApiResource::new(base, 1, Vec::new(), Options::default()).unwrap(),
        );
        Arc::new(crate::http::Apis::new(by_name, Some("default".to_string())).unwrap())
    }

    fn context_with(
        registry: Registry,
        apis: Arc<crate::http::Apis>,
        generator: Arc<Generator>,
        plugins: Option<Arc<crate::plugin::Plugins>>,
    ) -> Arc<RunContext> {
        Arc::new(RunContext::new(
            registry,
            apis,
            generator,
            crate::feature::TagFilter::new(&[]),
            None,
            String::new(),
            None,
            plugins,
            Options::default(),
            crate::artifacts::Artifacts::for_test(),
            false,
        ))
    }

    fn context(registry: Registry) -> Arc<RunContext> {
        context_with(registry, apis(), Arc::new(Generator::new()), None)
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
        plugins.add_defaults(
            [("echo".to_string(), "a".to_string())]
                .into_iter()
                .collect(),
        );
        plugins
    }

    /// `force_action` overrides the kind the plugin declared for one step,
    /// which is how a plugin that answers `not_yet` from an ACTION is built:
    /// the registry, not the reply, is what says a step is an assertion.
    fn echo_registry(plugins: &crate::plugin::Plugins, force_action: Option<usize>) -> Registry {
        let steps: Vec<_> = plugins
            .steps()
            .into_iter()
            .map(|(lib, index, pattern, assertion, secret_groups)| {
                (
                    lib,
                    index,
                    pattern,
                    assertion && force_action != Some(index),
                    secret_groups,
                )
            })
            .collect();
        Registry::with_macros_and_plugins(
            MacroCatalog::default(),
            &steps,
            &["echo".to_string()],
            None,
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
            crate::artifacts::Artifacts::for_test(),
        );
        (world, plugins)
    }

    async fn run_step(world: &mut World, reg: &Registry, text: &str) -> Result<(), String> {
        let step = ExpandedStep {
            keyword: String::new(),
            text: text.to_string(),
            line: 1,
            docstring: None,
            table: None,
        };
        let generator = world.generator.clone();
        let source = std::path::Path::new("test.feature");
        let origin = Origin {
            line: 1,
            index: None,
            comment: None,
        };
        execute_step(world, reg, &step, source, &generator, 0, origin)
            .await
            .map_err(|error| error.message)
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

    fn macro_catalog(name: &str, source: &str) -> MacroCatalog {
        let dir = std::env::temp_dir().join(format!("bddkit-runner-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("macros.yaml");
        std::fs::write(&path, source).unwrap();
        MacroCatalog::load(&[path]).unwrap()
    }

    fn registry(name: &str, source: &str) -> Registry {
        Registry::with_macros(macro_catalog(name, source)).unwrap()
    }

    async fn run(feature: &str, registry: Registry) -> FileResult {
        run_in(feature, context(registry)).await
    }

    async fn run_in(feature: &str, context: Arc<RunContext>) -> FileResult {
        let loaded = LoadedFeature::from_text(PathBuf::from("macro.feature"), feature)
            .expect("gherkin parses");
        run_file(Arc::new(loaded), context).await
    }

    #[tokio::test]
    async fn a_missing_annotation_scan_never_skips_a_parsed_step() {
        let mut loaded = LoadedFeature::from_text(
            PathBuf::from("without-annotations.feature"),
            "Feature: f\n  Background:\n    Given set variable \"x\" to \"1\"\n  Scenario: s\n    Then variable \"x\" should be equal to \"2\"\n",
        ).unwrap();
        loaded.annotations = crate::annotate::Source::default();
        let result = run_file(Arc::new(loaded), context(Registry::new().unwrap())).await;
        assert_eq!(
            result.failed(),
            1,
            "an annotation scan cannot decide whether parsed steps execute"
        );
        assert_eq!(result.scenarios[0].steps.len(), 2);
    }

    /// `tokio::spawn` requires `Send + 'static`. This is checked by the
    /// compiler: if the file-run future stops being Send, the test won't build.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_file_run_can_be_spawned_onto_another_thread() {
        let loaded = LoadedFeature::from_text(
            PathBuf::from("spawnable.feature"),
            "Feature: f\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
        )
        .expect("gherkin parses");
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

        /// A built-in variable assertion is final on its first attempt (issue
        /// #39), so the modifier's plumbing is driven through the echo
        /// plugin's counter instead: a real `not_yet` that only a retry can
        /// change, with no network under the paused clock.
        async fn run_with_echo(feature: &str, macros: MacroCatalog) -> FileResult {
            let plugins = echo_plugins();
            let registry = Registry::with_macros_and_plugins(
                macros,
                &plugins.steps(),
                &["echo".to_string()],
                None,
            )
            .expect("the plugin registry builds");
            let context = context_with(
                registry,
                apis(),
                Arc::new(Generator::new()),
                Some(Arc::new(plugins)),
            );
            run_in(feature, context).await
        }

        async fn echo_failure(feature: &str, macros: MacroCatalog) -> String {
            run_with_echo(feature, macros).await.scenarios[0]
                .failure
                .clone()
                .expect("scenario should fail")
        }

        #[tokio::test(start_paused = true)]
        async fn a_counter_that_never_arrives_times_out_with_the_last_observation() {
            let failure = echo_failure(
                r#"
Feature: eventual assertion
  Scenario: timeout
    Given I expect the next assertion to pass within "1" seconds, checking every "100" milliseconds
    Then the echo counter should reach 1000
"#,
                MacroCatalog::default(),
            )
            .await;

            for expected in [
                "the echo counter should reach 1000",
                "1s",
                "100ms",
                "11 attempts",
                "counter is 11 of 1000",
            ] {
                assert!(
                    failure.contains(expected),
                    "missing {expected:?} in {failure}"
                );
            }
        }

        #[tokio::test(start_paused = true)]
        async fn a_variable_mismatch_under_a_modifier_fails_at_once() {
            // Nothing runs between two attempts, so the variable cannot
            // change: polling it would burn the timeout to repeat itself.
            let started = Instant::now();
            let failure = failure(
                r#"
Feature: eventual assertion
  Scenario: final
    Given set variable "state" to "pending"
    And I expect the next assertion to pass within "1" seconds, checking every "100" milliseconds
    Then variable "state" should be equal to "ready"
"#,
                Registry::new().unwrap(),
            )
            .await;

            assert!(!failure.contains("did not pass"), "{failure}");
            assert!(failure.contains("expected: ready"), "{failure}");
            assert!(failure.contains("actual:   pending"), "{failure}");
            assert_eq!(Instant::now(), started);
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_retrying_assertion_prepares_unique_capture_once() {
            // Real time and a real stub: the built-in path is the one under
            // test, and a built-in retry needs a response it can re-request.
            let app =
                axum::Router::new().route("/state", axum::routing::get(|| async { "pending" }));
            let (base, _server) = crate::http::tests::spawn_app(app).await;
            let generator = Arc::new(Generator::new());
            let context = context_with(
                Registry::new().unwrap(),
                apis_at(&base),
                generator.clone(),
                None,
            );
            let before = generator.next(UniqueKind::Number).parse::<u64>().unwrap();
            let result = run_in(
                r#"
Feature: eventual assertion
  Scenario: stable preparation
    When I request "/state"
    And I expect the next assertion to pass within "1" seconds, checking every "100" milliseconds
    Then the response body contains "<<unique(number)>>"
"#,
                context,
            )
            .await;
            let failure = result.scenarios[0].failure.as_deref().unwrap();
            let attempts: u64 = regex::Regex::new(r"(\d+) attempts")
                .unwrap()
                .captures(failure)
                .and_then(|caps| caps[1].parse().ok())
                .unwrap_or_else(|| panic!("no attempt count in {failure}"));
            assert!(attempts > 1, "the assertion was never retried: {failure}");
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
            let failure = echo_failure(
                r#"
Feature: eventual assertion
  Scenario: replacement
    Given I expect the next assertion to pass within "5" seconds
    And I expect the next assertion to pass within "1" seconds
    Then the echo counter should reach 1000
"#,
                MacroCatalog::default(),
            )
            .await;

            assert!(failure.contains("within 1s"), "{failure}");
            assert!(!failure.contains("within 5s"), "{failure}");
        }

        #[tokio::test(start_paused = true)]
        async fn a_modifier_survives_an_ordinary_action() {
            let failure = echo_failure(
                r#"
Feature: eventual assertion
  Scenario: action
    Given I expect the next assertion to pass within "1" seconds
    And set variable "state" to "pending"
    Then the echo counter should reach 1000
"#,
                MacroCatalog::default(),
            )
            .await;

            assert!(failure.contains("did not pass within 1s"), "{failure}");
        }

        #[tokio::test(start_paused = true)]
        async fn a_modifier_survives_a_macro_boundary() {
            let macros = macro_catalog(
                "eventual",
                r#"
- step: I check the counter through a macro
  do:
    - set variable "macro_ran" to "yes"
    - the echo counter should reach 1000
"#,
            );
            let failure = echo_failure(
                r#"
Feature: eventual assertion
  Scenario: macro
    Given I expect the next assertion to pass within "1" seconds
    Then I check the counter through a macro
"#,
                macros,
            )
            .await;

            assert!(failure.contains("did not pass within 1s"), "{failure}");
        }

        #[tokio::test(start_paused = true)]
        async fn the_first_assertion_consumes_the_modifier() {
            // The first assertion is a built-in one on purpose: the built-in
            // and the plugin paths take the modifier in separate code, and
            // only a leak from the built-in take would reach the echo step.
            let started = Instant::now();
            let failure = echo_failure(
                r#"
Feature: eventual assertion
  Scenario: one shot
    Given set variable "state" to "ready"
    And I expect the next assertion to pass within "1" seconds
    Then variable "state" should be equal to "ready"
    And the echo counter should reach 1000
"#,
                MacroCatalog::default(),
            )
            .await;

            assert!(!failure.contains("did not pass"), "{failure}");
            assert!(failure.contains("counter is 1 of 1000"), "{failure}");
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
            let result = run_with_echo(
                r#"
Feature: eventual assertion
  Scenario: arm
    Given I expect the next assertion to pass within "1" seconds
  Scenario: assert
    Then the echo counter should reach 1000
"#,
                MacroCatalog::default(),
            )
            .await;

            assert!(result.scenarios[0].failure.is_none());
            let failure = result.scenarios[1]
                .failure
                .as_deref()
                .expect("second scenario fails");
            assert!(!failure.contains("did not pass"), "{failure}");
            assert!(failure.contains("counter is 1 of 1000"), "{failure}");
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
        use std::path::PathBuf;
        use std::sync::Arc;

        pub(super) fn file(path: &str, tags: &str) -> Arc<LoadedFeature> {
            let src =
                format!("{tags}Feature: f\n  Scenario: s\n    Then the response code is 200\n");
            Arc::new(LoadedFeature::from_text(PathBuf::from(path), &src).expect("gherkin parses"))
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
    async fn a_panicked_file_is_a_panic_failure() {
        let error = tokio::spawn(async { panic!("deliberate") })
            .await
            .expect_err("the task must panic");
        let result = panicked_file(
            &chains::file("bad.feature", ""),
            &error,
            &crate::privacy::Redactor::disabled(),
        );
        assert_eq!(result.scenarios[0].failure_kind, Some(FailureKind::Panic));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_panicking_file_becomes_a_failed_result_instead_of_killing_the_run() {
        let error = tokio::spawn(async { panic!("deliberate") })
            .await
            .expect_err("the task must panic");
        let result = panicked_file(
            &chains::file("bad.feature", ""),
            &error,
            &crate::privacy::Redactor::disabled(),
        );
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

    /// The subprocess tests have no way to panic a file — the fixtures guard
    /// every export and no host step panics on input — so this drives
    /// `run_all` directly. A row shorter than its header is what the parser
    /// guarantees never to produce and what `substitute` indexes without a
    /// check, which makes `expand_outlines` panic inside `run_file`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_panicking_file_is_still_closed_in_the_stream() {
        let path = std::env::temp_dir().join(format!("bddkit-panic-stream-{}", std::process::id()));
        let events = Events::start(crate::events::open(&path).expect("open"));
        let mut feature = crate::feature::parse_str(
            "Feature: f\n  Scenario Outline: o\n    Then the response code is <a>\n    Examples:\n      | a |\n      | 200 |\n",
        )
        .expect("gherkin parses");
        feature.scenarios[0].examples[0]
            .table
            .as_mut()
            .expect("an examples table")
            .rows
            .push(Vec::new());
        let lf = Arc::new(LoadedFeature::without_source(
            PathBuf::from("bad.feature"),
            feature,
        ));
        let mut ctx = Arc::into_inner(context(Registry::new().expect("builtin steps register")))
            .expect("one owner");
        ctx.events = Some(events.clone());
        let ctx = Arc::new(ctx);

        let results = run_all(build_chains(vec![lf]).expect("no tags"), ctx, 1).await;
        events
            .finish(serde_json::json!({"exit": 1}))
            .expect("finish");

        assert_eq!(results[0].failed(), 1);
        let text = std::fs::read_to_string(&path).expect("the stream is written");
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
            .collect();
        let types: Vec<_> = lines
            .iter()
            .map(|l| l["type"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(types, ["file_started", "file_finished", "run_finished"]);
        assert_eq!(lines[1]["panicked"], true);
        assert_eq!(
            (&lines[1]["scenarios"], &lines[1]["failed"]),
            (&1.into(), &1.into())
        );
        assert_eq!(lines[2]["files"], 1, "{text}");
    }

    #[test]
    fn force_stop_arms_the_stop_even_without_fail_fast() {
        // request_stop is a no-op without --fail-fast; force_stop (the
        // interrupt handler's tool) must not have that gate.
        let ctx = context(Registry::new().expect("builtin steps register"));
        assert!(!ctx.stopped());
        ctx.request_stop();
        assert!(!ctx.stopped(), "request_stop must stay gated by fail_fast");
        ctx.force_stop();
        assert!(ctx.stopped());
    }

    #[test]
    fn a_run_without_plugins_can_still_allocate_an_artifact_path() {
        let ctx = context(Registry::new().expect("built-in patterns compile"));
        assert!(ctx.plugins.is_none());
        let (first, second) = (ctx.artifacts.next_dir(), ctx.artifacts.next_dir());
        assert_ne!(first, second);
        assert!(first.starts_with(ctx.artifacts.root()));
    }
}
