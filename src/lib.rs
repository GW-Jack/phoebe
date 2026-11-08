//! Compatibility facade and static runtime archive for the Phoebe toolchain.
//! Phase implementations live in independently cacheable workspace crates.

pub use phoebe_abi as abi;
pub use phoebe_capability as capability_flow;
pub use phoebe_core as core;
pub use phoebe_frame as frame_layout;
pub use phoebe_generic_core as generic_core;
pub use phoebe_native as native;
pub use phoebe_package as package;
pub use phoebe_project as project;
pub use phoebe_runtime as runtime;
pub use phoebe_syntax as syntax;
pub use phoebe_syntax::{Diagnostic, Span};

mod project_session;
pub use project_session::{CompileReport, ProjectCompilerSession};
mod build_session;
pub use build_session::{BuildReport, ProjectBuildSession};
