//! In-process project compilation with checked function-body reuse.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use crate::{core, project, syntax};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileReport {
    pub load: project::LoadReport,
    pub reused_core_functions: usize,
}

struct FunctionSource {
    module: String,
    name: String,
    start: usize,
}

#[derive(Default)]
pub struct ProjectCompilerSession {
    project: project::ProjectSession,
    previous_ast: Option<Arc<syntax::syntax::Program>>,
    previous_core: Option<Arc<core::Program>>,
    previous_functions: Vec<FunctionSource>,
}

impl ProjectCompilerSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn compile(&mut self, path: &Path) -> Result<(Arc<core::Program>, CompileReport), String> {
        // Publish both caches only after the new combined Core passes all
        // verifiers. A failed edit cannot become a future cache hit.
        let mut next_project = self.project.clone();
        let (loaded, load) = next_project.load(path)?;
        let affected = load
            .affected
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let current = loaded
            .ast
            .functions
            .iter()
            .zip(&loaded.function_modules)
            .map(|(function, module)| FunctionSource {
                module: module.clone(),
                name: function.name.clone(),
                start: function.name_span.start,
            })
            .collect::<Vec<_>>();
        let (reusable, deltas): (Vec<_>, Vec<_>) = current
            .iter()
            .enumerate()
            .map(|(index, function)| {
                let Some(previous) = self.previous_functions.get(index) else {
                    return (false, 0);
                };
                if previous.module != function.module
                    || previous.name != function.name
                    || affected.contains(function.module.as_str())
                {
                    return (false, 0);
                }
                match (
                    isize::try_from(function.start),
                    isize::try_from(previous.start),
                ) {
                    (Ok(now), Ok(before)) => (true, now - before),
                    _ => (false, 0),
                }
            })
            .unzip();
        let (program, reused_core_functions) = if let (Some(previous_ast), Some(previous)) =
            (&self.previous_ast, &self.previous_core)
        {
            core::lower_reusing(&loaded.ast, previous_ast, previous, &reusable, &deltas)
                .map_err(|error| loaded.render(&error))?
        } else {
            (
                core::lower(&loaded.ast).map_err(|error| loaded.render(&error))?,
                0,
            )
        };
        let program = Arc::new(program);
        self.project = next_project;
        self.previous_ast = Some(Arc::new(loaded.ast));
        self.previous_core = Some(program.clone());
        self.previous_functions = current;
        Ok((
            program,
            CompileReport {
                load,
                reused_core_functions,
            },
        ))
    }
}
