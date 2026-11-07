use crate::failure::StepError;
use crate::world::World;

/// `I use "<name>" <word>`. The word is a group or a plugin's deprecated word
/// for one. A group-scoped handle the file already holds is told of the
/// switch; with no handle yet the first dispatch carries the instance.
pub async fn use_instance(w: &mut World, word: &str, name: &str) -> Result<(), StepError> {
    let plugins = w.plugins.plugins().cloned();
    let group = plugins
        .as_ref()
        .map_or(word, |plugins| plugins.resolve_group(word))
        .to_string();
    let previous = w.plugins.current(&group).ok().map(str::to_string);
    w.plugins
        .use_instance(&group, name)
        .map_err(StepError::input)?;
    let Some(plugins) = plugins else {
        return Ok(());
    };
    let Some(lib) = plugins.wants_selection(&group) else {
        return Ok(());
    };
    let Some((_, handle)) = w
        .plugins
        .handle_for(&group, name)
        .filter(|(l, _)| *l == lib)
    else {
        return Ok(());
    };
    let name = name.to_string();
    let told = {
        let group = group.clone();
        tokio::task::spawn_blocking(move || plugins.select_instance(lib, handle, &group, &name))
            .await
            .unwrap_or_else(|error| Err(format!("the plugin dispatch task failed: {error}")))
    };
    if let Err(error) = told {
        // The plugin was not told, so the host must not act as if it were.
        if let Some(previous) = previous {
            let _ = w.plugins.use_instance(&group, &previous);
        }
        return Err(StepError::plugin(error));
    }
    Ok(())
}
