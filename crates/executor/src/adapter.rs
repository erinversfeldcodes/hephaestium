//! An agent runtime behind one interface.
//!
//! An adapter does two things: from a workload and its workspace it decides how
//! to start the runtime, and from each line the runtime writes it says what
//! events that means. Everything else in the executor deals in events, so none
//! of it reads a runtime's own format.

use std::path::Path;

use contracts::v1alpha1::event::Kind;
use contracts::v1alpha1::{LaunchPlan, Workload};

use crate::transcript;

pub trait Adapter {
    /// Make whatever the runtime needs to exist before it starts, in or beside
    /// the workspace. Nothing by default.
    ///
    /// # Errors
    ///
    /// Returns the I/O error if something could not be made.
    fn prepare(&self, _workload: &Workload, _workspace: &Path) -> std::io::Result<()> {
        Ok(())
    }

    /// How to start the runtime for this workload in this workspace.
    fn plan(&self, workload: &Workload, workspace: &Path) -> LaunchPlan;

    /// The events one line of the runtime's output means.
    ///
    /// # Errors
    ///
    /// Returns why the line cannot be understood.
    fn translate(&self, line: &str) -> Result<Vec<Kind>, String>;
}

/// An adapter for development and tests: it runs the command it was given, with
/// the variables it was told to pass, and reads each line of output as an event.
pub struct Stub {
    pub command: Vec<String>,
    pub env_names: Vec<String>,
}

impl Adapter for Stub {
    fn plan(&self, _workload: &Workload, workspace: &Path) -> LaunchPlan {
        LaunchPlan {
            command: self.command.clone(),
            env_names: self.env_names.clone(),
            working_directory: workspace.display().to_string(),
            stdin: String::new(),
        }
    }

    fn translate(&self, line: &str) -> Result<Vec<Kind>, String> {
        transcript::kind_of(line).map(|kind| vec![kind])
    }
}
