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
    Ok(select(paths, split, filter, false)?.map(|all| {
        all.into_iter()
            .filter_map(|selected| match selected {
                Selected::File(file) => Some(file),
                Selected::Unparsed(_) => None,
            })
            .collect()
    }))
}

/// A file that is broken in itself (it does not parse, or is not UTF-8), with
/// the line the parser stopped at when it says one.
pub(crate) struct Unparsed {
    pub path: PathBuf,
    pub message: String,
    pub line: Option<usize>,
}

/// What [`select_listed`] found, in path order.
pub(crate) enum Selected {
    File(Arc<feature::LoadedFeature>),
    Unparsed(Unparsed),
}

/// [`select_features`] for `list`, which shows a file that does not parse
/// instead of stopping at it: `run` must not start over one, a tree of the rest
/// can still be drawn. Its tags are unknown, so a tag filter cannot drop it, and
/// a line argument cannot narrow it. A path that does not exist, or a file that
/// cannot be read at all, is still the outer `Err`.
pub(crate) fn select_listed(
    paths: &[PathBuf],
    split: bool,
    filter: &feature::TagFilter,
) -> Result<Result<Vec<Selected>, String>> {
    select(paths, split, filter, true)
}

/// `error` as a file that is broken in itself, when it is one: its text does not
/// parse, or is not UTF-8. A path that does not exist and a file that cannot be
/// read at all are not: they stay errors. `message` is the whole chain, the
/// words `run` says for the same file.
fn unparsed(path: &std::path::Path, error: &anyhow::Error) -> Option<Unparsed> {
    let line = if let Some(syntax) = error.downcast_ref::<crate::feature::source::SyntaxError>() {
        Some(syntax.line)
    } else if error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|read| read.kind() == std::io::ErrorKind::InvalidData)
    {
        None
    } else {
        return None;
    };
    Some(Unparsed {
        path: path.to_path_buf(),
        message: format!("{error:#}"),
        line,
    })
}

fn select(
    paths: &[PathBuf],
    split: bool,
    filter: &feature::TagFilter,
    lenient: bool,
) -> Result<Result<Vec<Selected>, String>> {
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
        let mut lf = match feature::load(&target.path) {
            Ok(lf) => lf,
            Err(error) if lenient => match unparsed(&target.path, &error) {
                Some(broken) => {
                    loaded.push(Selected::Unparsed(broken));
                    continue;
                }
                None => return Err(error),
            },
            Err(error) => return Err(error),
        };
        if !target.sels.is_empty() {
            if let Err(message) = lf.select_lines(&target.sels) {
                return Ok(Err(message));
            }
            if target.whole {
                lf.select_all();
            }
        }
        if lf.has_selected_scenario(filter) {
            loaded.push(Selected::File(Arc::new(lf)));
        }
    }
    Ok(Ok(loaded))
}
