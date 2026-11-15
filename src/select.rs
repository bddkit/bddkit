//! Which feature files a `run` or a `list` takes, loaded and narrowed.

use crate::feature;
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// The features to run, loaded and narrowed. With `split`, an argument may end
/// in `:LINE` or `:LINE#ROW` and then runs only those scenarios of its file; a
/// file named more than once runs once (found by its real path, so `./a` and
/// `a` are one), with the lines of all its arguments, or whole when it, or a
/// directory holding it, is also named without a line. A line that selects
/// nothing is the inner `Err`, whether or not the file ends up whole.
pub(crate) fn select_features(
    paths: &[PathBuf],
    split: bool,
    filter: &feature::TagFilter,
) -> Result<Result<Vec<Arc<feature::LoadedFeature>>, String>> {
    struct Target {
        path: PathBuf,
        sels: Vec<feature::LineSel>,
        whole: bool,
    }
    let real = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let mut targets: BTreeMap<PathBuf, Target> = BTreeMap::new();
    let mut plain = Vec::new();
    for arg in paths {
        let (file, sels) = if split {
            feature::split_line_selection(arg)
        } else {
            (arg.clone(), Vec::new())
        };
        if sels.is_empty() {
            plain.push(file);
        } else {
            targets
                .entry(real(&file))
                .or_insert_with(|| Target {
                    path: file,
                    sels: Vec::new(),
                    whole: false,
                })
                .sels
                .extend(sels);
        }
    }
    for file in feature::discover(&plain)? {
        targets
            .entry(real(&file))
            .or_insert_with(|| Target {
                path: file,
                sels: Vec::new(),
                whole: false,
            })
            .whole = true;
    }
    let mut targets: Vec<Target> = targets.into_values().collect();
    targets.sort_by(|a, b| a.path.cmp(&b.path));
    let mut loaded = Vec::new();
    for target in targets {
        let mut lf = feature::load(&target.path)?;
        if !target.sels.is_empty() {
            if let Err(message) = lf.select_lines(&target.sels) {
                return Ok(Err(message));
            }
            if target.whole {
                lf.select_all();
            }
        }
        if lf.has_selected_scenario(filter) {
            loaded.push(Arc::new(lf));
        }
    }
    Ok(Ok(loaded))
}
