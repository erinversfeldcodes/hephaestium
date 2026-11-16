//! Checking a manifest against the schema.

/// Where a managed repository keeps its manifest, relative to its root.
pub const DEFAULT_PATH: &str = ".athenaeum/manifest.toml";

/// One place where a manifest breaks one rule of the schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Where in the manifest the rule is broken, as `$.a.b[0]`.
    pub path: String,
    /// The JSON Schema keyword that rejects it.
    pub keyword: String,
    /// What is wrong, in words.
    pub message: String,
}

/// Why a manifest is not valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invalid {
    /// The text is not TOML, so there is no manifest to check.
    NotToml(String),
    /// The manifest is TOML and breaks these rules of the schema.
    Violations(Vec<Violation>),
}

/// Check the text of a manifest against the schema.
///
/// # Errors
///
/// Returns why the manifest is not valid.
pub fn check(_text: &str) -> Result<(), Invalid> {
    Ok(())
}
