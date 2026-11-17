//! Reading and rewriting a lock.
//!
//! The schema is compiled in from the Athenaeum tag this crate pins, as the
//! manifest's is, and that tag's cases are this crate's tests.

use std::sync::LazyLock;

use jsonschema::Validator;
use serde::Deserialize;
use toml_edit::{DocumentMut, Value};

use crate::schema;
pub use crate::schema::{Invalid, Violation};

/// Where a managed repository keeps its lock, relative to its root.
pub const DEFAULT_PATH: &str = ".athenaeum/lock.toml";

/// An agent runtime that skills are linked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Harness {
    /// Claude Code looks for skills in `.claude/skills`.
    #[serde(rename = "claude-code")]
    ClaudeCode,
}

/// A lock that has passed the schema, read for what sync needs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Lock {
    /// Where Athenaeum is fetched from: a git URL.
    pub source: String,
    /// The release tag it is pinned at.
    pub rev: String,
    /// The commit that tag resolved to when the lock was written.
    pub commit: String,
    /// The skills taken, by name.
    pub skills: Vec<String>,
    /// The agent runtimes the skills are linked for.
    #[serde(default)]
    pub harnesses: Vec<Harness>,
}

/// The pinned schema, compiled once.
static VALIDATOR: LazyLock<Validator> = LazyLock::new(|| schema::compile(athenaeum::lock::SCHEMA));

/// Check the text of a lock against the schema.
///
/// # Errors
///
/// Returns why the lock is not valid: either the text is not TOML, or it is
/// and breaks one or more rules, each reported with where and why.
pub fn check(text: &str) -> Result<(), Invalid> {
    schema::validate(&VALIDATOR, text)
}

/// Check and read a lock.
///
/// # Errors
///
/// Returns why the lock is not valid.
///
/// # Panics
///
/// Only if the schema accepts a lock that [`Lock`] cannot read, which is a
/// defect in this crate and is ruled out by a test that reads every accepted
/// case.
pub fn parse(text: &str) -> Result<Lock, Invalid> {
    check(text)?;
    Ok(toml::from_str(text).expect("a lock the schema accepts to be readable as a Lock"))
}

/// The text of a lock with its pin moved to `rev` and `commit`.
///
/// Everything else in the text is kept as it was, comments and layout
/// included, because a person edits this file and reads its history.
///
/// # Panics
///
/// If the text is not a lock that has passed the schema, which always has both
/// keys: callers rewrite only a lock they have just checked.
#[must_use]
pub fn pinned_at(text: &str, rev: &str, commit: &str) -> String {
    let mut document: DocumentMut = text
        .parse()
        .expect("a lock that has passed the schema to be TOML");
    for (key, new) in [("rev", rev), ("commit", commit)] {
        let value = document[key]
            .as_value_mut()
            .expect("a lock that has passed the schema to have this key");
        let decor = value.decor().clone();
        *value = Value::from(new);
        *value.decor_mut() = decor;
    }
    document.to_string()
}
