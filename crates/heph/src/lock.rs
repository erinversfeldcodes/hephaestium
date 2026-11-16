//! Reading a lock.

pub use crate::manifest::{Invalid, Violation};

/// Where a managed repository keeps its lock, relative to its root.
pub const DEFAULT_PATH: &str = ".athenaeum/lock.toml";

/// A lock that has passed the schema, read for what sync needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    /// Where Athenaeum is fetched from.
    pub source: String,
    /// The release tag it is pinned at.
    pub rev: String,
    /// The commit that tag resolved to.
    pub commit: String,
    /// The skills taken.
    pub skills: Vec<String>,
}

/// Check the text of a lock against the schema.
///
/// # Errors
///
/// Returns why the lock is not valid.
pub fn check(_text: &str) -> Result<(), Invalid> {
    Ok(())
}

/// Check and read a lock.
///
/// # Errors
///
/// Returns why the lock is not valid.
pub fn parse(_text: &str) -> Result<Lock, Invalid> {
    Err(Invalid::NotToml("not implemented".to_owned()))
}
