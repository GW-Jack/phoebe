//! In-process project rebuilds across edits, from source to standalone output.

use std::path::Path;

use crate::{ProjectCompilerSession, native, project_session::CompileReport};

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
        let (program, compile) = self.compiler.compile(project)?;
        let native = self.native.build_executable_report(&program, output)?;
        Ok(BuildReport {
            compile,
            reused_native_objects: native.reused_objects,
            linked: native.linked,
        })
    }
}
