//! In-process project rebuilds across edits, from source to standalone output.

use std::path::Path;

use crate::{
    ProjectCompilerSession, native,
    project_session::{CompileReport, CompiledProject},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildReport {
    pub compile: CompileReport,
    pub reused_native_objects: usize,
    pub linked: bool,
}

#[derive(Default)]
pub struct ProjectBuildSession {
    compiler: ProjectCompilerSession,
    native: native::NativeBuildSession,
}

impl ProjectBuildSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn build(&mut self, project: &Path, output: &Path) -> Result<BuildReport, String> {
        let (program, compile) = self.compiler.compile_for_build(project)?;
        let native = match program {
            CompiledProject::Concrete(program) => {
                self.native.build_executable_report(&program, output)?
            }
            CompiledProject::Linked(program) => self
                .native
                .build_executable_linked_report(&program, output)?,
        };
        Ok(BuildReport {
            compile,
            reused_native_objects: native.reused_objects,
            linked: native.linked,
        })
    }
}
