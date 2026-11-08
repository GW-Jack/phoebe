//! In-process project compilation with checked function-body reuse.

use std::collections::HashMap;
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

pub(crate) enum CompiledProject {
    Concrete(Arc<core::Program>),
    Linked(Arc<core::CheckedLinkedProgram>),
}

#[derive(Default)]
pub struct ProjectCompilerSession {
    project: project::ProjectSession,
    previous_ast: Option<Arc<syntax::syntax::Program>>,
    previous_core: Option<Arc<core::Program>>,
    previous_linked: Option<Arc<core::CheckedLinkedProgram>>,
    previous_functions: Vec<FunctionSource>,
}

impl ProjectCompilerSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn compile(&mut self, path: &Path) -> Result<(Arc<core::Program>, CompileReport), String> {
        // Publish both caches only after the new combined Core passes all
        // verifiers. A failed edit cannot become a future cache hit.
        let prepared = self.prepare(path)?;
        self.compile_prepared(prepared)
    }

    fn compile_prepared(
        &mut self,
        prepared: PreparedProject,
    ) -> Result<(Arc<core::Program>, CompileReport), String> {
        let PreparedProject {
            project: next_project,
            loaded,
            load,
            functions: current,
            reusable,
            deltas,
        } = prepared;
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
        self.previous_linked = None;
        self.previous_functions = current;
        Ok((
            program,
            CompileReport {
                load,
                reused_core_functions,
            },
        ))
    }

    pub fn compile_linked(
        &mut self,
        path: &Path,
    ) -> Result<(Arc<core::CheckedLinkedProgram>, CompileReport), String> {
        let prepared = self.prepare(path)?;
        self.compile_linked_prepared(prepared)
    }

    fn compile_linked_prepared(
        &mut self,
        prepared: PreparedProject,
    ) -> Result<(Arc<core::CheckedLinkedProgram>, CompileReport), String> {
        let PreparedProject {
            project: next_project,
            loaded,
            load,
            functions: current,
            reusable,
            deltas,
        } = prepared;
        let (program, reused_core_functions) = if let (Some(previous_ast), Some(previous)) =
            (&self.previous_ast, &self.previous_linked)
        {
            core::lower_linked_reusing(&loaded.ast, previous_ast, previous, &reusable, &deltas)
                .map_err(|error| loaded.render(&error))?
        } else {
            (
                core::lower_linked_checked(&loaded.ast).map_err(|error| loaded.render(&error))?,
                0,
            )
        };
        let program = Arc::new(program);
        self.project = next_project;
        self.previous_ast = Some(Arc::new(loaded.ast));
        self.previous_core = None;
        self.previous_linked = Some(program.clone());
        self.previous_functions = current;
        Ok((
            program,
            CompileReport {
                load,
                reused_core_functions,
            },
        ))
    }

    pub(crate) fn compile_for_build(
        &mut self,
        path: &Path,
    ) -> Result<(CompiledProject, CompileReport), String> {
        let prepared = self.prepare(path)?;
        if prepared
            .loaded
            .ast
            .functions
            .iter()
            .any(|function| !function.type_params.is_empty())
        {
            let (program, report) = self.compile_linked_prepared(prepared)?;
            Ok((CompiledProject::Linked(program), report))
        } else {
            let (program, report) = self.compile_prepared(prepared)?;
            Ok((CompiledProject::Concrete(program), report))
        }
    }

    fn prepare(&self, path: &Path) -> Result<PreparedProject, String> {
        let mut next_project = self.project.clone();
        let (loaded, load) = next_project.load(path)?;
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
        let previous = self
            .previous_functions
            .iter()
            .map(|function| {
                (
                    (function.module.as_str(), function.name.as_str()),
                    function.start,
                )
            })
            .collect::<HashMap<_, _>>();
        let (reusable, deltas): (Vec<_>, Vec<_>) = current
            .iter()
            .map(|function| {
                let Some(&start) =
                    previous.get(&(function.module.as_str(), function.name.as_str()))
                else {
                    return (false, 0);
                };
                match (isize::try_from(function.start), isize::try_from(start)) {
                    (Ok(now), Ok(before)) => (true, now - before),
                    _ => (false, 0),
                }
            })
            .unzip();
        Ok(PreparedProject {
            project: next_project,
            loaded,
            load,
            functions: current,
            reusable,
            deltas,
        })
    }
}

struct PreparedProject {
    project: project::ProjectSession,
    loaded: project::Loaded,
    load: project::LoadReport,
    functions: Vec<FunctionSource>,
    reusable: Vec<bool>,
    deltas: Vec<isize>,
}
