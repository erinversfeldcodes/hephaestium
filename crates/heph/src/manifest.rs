//! Checking a manifest against the schema.
//!
//! The schema is not written here. It is compiled in from the Athenaeum tag
//! this crate pins, and the same tag's cases are run as this crate's tests,
//! so the rules enforced are the rules published, and a disagreement between
//! the two fails the build.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use jsonschema::Validator;
use serde::Deserialize;

use crate::schema;
pub use crate::schema::{Invalid, Violation};

/// Where a managed repository keeps its manifest, relative to its root.
pub const DEFAULT_PATH: &str = ".athenaeum/manifest.toml";

/// A manifest that has passed the schema, read for what the commands need.
///
/// This holds only the parts something reads today. The schema is the
/// definition of a manifest, so a key this does not name is still valid.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    /// How to enter the repository's pinned toolchain.
    pub toolchain: Option<Toolchain>,
    /// The buildable parts of the repository, by name.
    pub components: BTreeMap<String, Component>,
    /// The gates, by name.
    #[serde(default)]
    pub gates: BTreeMap<String, Gate>,
}

/// One buildable part of the repository.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Component {
    /// The directory its commands run in.
    pub root: String,
    /// Globs for the files that belong to it.
    pub paths: Vec<String>,
    /// Its format and lint checks, each a command that changes nothing and
    /// fails on a finding.
    #[serde(default)]
    pub lint: Vec<Vec<String>>,
}

/// How to enter the repository's pinned toolchain.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Toolchain {
    /// Put in front of every command.
    pub wrapper: Vec<String>,
    /// When this variable is set, the caller is already inside the toolchain
    /// and the wrapper is left off.
    pub inside_env: Option<String>,
}

/// A command that decides whether a change may proceed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Gate {
    /// The program and its arguments.
    pub run: Vec<String>,
    /// The gate applies only to a change that touches a matching file.
    pub when_paths: Option<Vec<String>>,
    /// How long it may run before it is stopped.
    pub timeout_seconds: u64,
    /// How it reports.
    pub result: Option<ResultKind>,
    /// The environment variables it needs, by name.
    #[serde(default)]
    pub requires: BTreeMap<String, Requirement>,
}

/// What a gate needs from its environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Requirement {
    /// A variable that must be set and non-empty.
    #[serde(rename = "env")]
    Env,
    /// The same, and its value must never be printed or logged.
    #[serde(rename = "secret")]
    Secret,
}

/// How a gate reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ResultKind {
    /// Zero is a pass.
    #[serde(rename = "exit-code")]
    ExitCode,
    /// The command also writes a JSON object to its standard output.
    #[serde(rename = "json")]
    Json,
}

/// The pinned schema, compiled once.
static VALIDATOR: LazyLock<Validator> =
    LazyLock::new(|| schema::compile(athenaeum::manifest::SCHEMA));

/// Check the text of a manifest against the schema.
///
/// # Errors
///
/// Returns why the manifest is not valid: either the text is not TOML, or it
/// is and breaks one or more rules, each reported with where and why.
pub fn check(text: &str) -> Result<(), Invalid> {
    schema::validate(&VALIDATOR, text)
}

/// Check the text of a manifest against the schema and read it.
///
/// # Errors
///
/// Returns why the manifest is not valid.
///
/// # Panics
///
/// Only if the schema accepts a manifest that [`Manifest`] cannot read, which
/// is a defect in this crate and is ruled out by a test that reads every
/// accepted case.
pub fn parse(text: &str) -> Result<Manifest, Invalid> {
    check(text)?;
    Ok(toml::from_str(text).expect("a manifest the schema accepts to be readable as a Manifest"))
}
