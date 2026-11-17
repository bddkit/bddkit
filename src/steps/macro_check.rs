//! The static check of the macro catalog, run while the registry is built:
//! conflicts, every body step, cycles and nesting depth.

use super::{BUILTIN_STEPS, COMPILED_BUILTINS, Registry, StepTarget, expression};
use crate::macros::{MacroDef, MacroStep};

impl Registry {
    pub(super) fn validate_macros(&self) -> Result<(), String> {
        self.macro_conflicts()?;
        let graph = self.macro_graph()?;
        let mut visited = vec![false; self.macros.len()];
        let mut visiting = vec![false; self.macros.len()];
        let mut path = Vec::new();
        for index in 0..self.macros.len() {
            self.visit(index, &graph, &mut visited, &mut visiting, &mut path)?;
        }
        let mut depths = vec![None; self.macros.len()];
        for index in 0..self.macros.len() {
            if macro_depth(index, &graph, &mut depths) > 16 {
                return Err(format!(
                    "macro nesting exceeds 16 starting from step {:?}",
                    self.macros[index].step
                ));
            }
        }
        Ok(())
    }

    /// No macro step may be matched by a builtin or by a later macro.
    fn macro_conflicts(&self) -> Result<(), String> {
        for (left_index, left) in self.macros.iter().enumerate() {
            for (def, builtin) in BUILTIN_STEPS.iter().zip(COMPILED_BUILTINS.iter()) {
                if expression::conflicts(&left.compiled.variants, &builtin.variants) {
                    return Err(format!(
                        "macro step {:?} from {}:{} conflicts with builtin step {:?}",
                        left.step,
                        left.source.display(),
                        left.line,
                        def.expression
                    ));
                }
            }
            for right in self.macros.iter().skip(left_index + 1) {
                if expression::conflicts(&left.compiled.variants, &right.compiled.variants) {
                    return Err(format!(
                        "macro step {:?} from {}:{} conflicts with {:?} from {}:{}",
                        left.step,
                        left.source.display(),
                        left.line,
                        right.step,
                        right.source.display(),
                        right.line,
                    ));
                }
            }
        }
        Ok(())
    }

    /// Checks every body step and returns, per macro, the macros it calls.
    fn macro_graph(&self) -> Result<Vec<Vec<usize>>, String> {
        let mut graph = vec![Vec::new(); self.macros.len()];
        for (index, definition) in self.macros.iter().enumerate() {
            for step in &definition.body {
                if let Some(target) = self.check_body_step(definition, step)? {
                    graph[index].push(target);
                }
            }
        }
        Ok(graph)
    }

    /// One macro body step: `Some(index)` when it calls another macro.
    fn check_body_step(
        &self,
        definition: &MacroDef,
        step: &MacroStep,
    ) -> Result<Option<usize>, String> {
        let at = || {
            format!(
                "{:?} from {}:{}",
                definition.step,
                definition.source.display(),
                definition.line
            )
        };
        match self.find(&step.text)? {
            Some((StepTarget::Builtin { .. }, _)) => Ok(None),
            Some((target @ StepTarget::Plugin { .. }, caps)) => {
                // ponytail: a file included from a macro body is not
                // walked, so its plugin steps are checked only at run time.
                crate::validate::check_plugin_step(
                    self,
                    &target,
                    &caps,
                    step.docstring.as_deref(),
                    // A macro body step has no table.
                    None,
                    definition
                        .source
                        .parent()
                        .unwrap_or(std::path::Path::new(".")),
                )
                .map_err(|e| format!("{e}, in macro {}", at()))?;
                Ok(None)
            }
            Some((StepTarget::Macro(_), _)) if step.docstring.is_some() => Err(format!(
                "macro call {:?} with a doc string inside macro {} is not supported",
                step.text,
                at()
            )),
            Some((StepTarget::Macro(target), _)) => Ok(Some(target)),
            None => Err(format!(
                "unknown step {:?} in macro {}{}",
                step.text,
                at(),
                self.unknown_step_detail(&step.text),
            )),
        }
    }

    fn visit(
        &self,
        index: usize,
        graph: &[Vec<usize>],
        visited: &mut [bool],
        visiting: &mut [bool],
        path: &mut Vec<usize>,
    ) -> Result<(), String> {
        if visiting[index] {
            let start = path.iter().position(|item| *item == index).unwrap_or(0);
            let mut cycle: Vec<&str> = path[start..]
                .iter()
                .map(|item| self.macros[*item].step.as_str())
                .collect();
            cycle.push(self.macros[index].step.as_str());
            return Err(format!("cycle in macros: {}", cycle.join(" → ")));
        }
        if visited[index] {
            return Ok(());
        }
        visiting[index] = true;
        path.push(index);
        for child in &graph[index] {
            self.visit(*child, graph, visited, visiting, path)?;
        }
        path.pop();
        visiting[index] = false;
        visited[index] = true;
        Ok(())
    }
}

fn macro_depth(index: usize, graph: &[Vec<usize>], memo: &mut [Option<usize>]) -> usize {
    if let Some(depth) = memo[index] {
        return depth;
    }
    let depth = 1 + graph[index]
        .iter()
        .map(|child| macro_depth(*child, graph, memo))
        .max()
        .unwrap_or(0);
    memo[index] = Some(depth);
    depth
}
