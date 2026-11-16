//! Checking a manifest against the schema.
//!
//! The schema is not written here. It is compiled in from the Athenaeum tag
//! this crate pins, and the same tag's cases are run as this crate's tests,
//! so the rules enforced are the rules published, and a disagreement between
//! the two fails the build.

use std::fmt::Write as _;
use std::sync::LazyLock;

use jsonschema::Validator;
use serde_json::Value;

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

/// The pinned schema, compiled once.
///
/// Both failures here would be defects in the pinned Athenaeum release, not
/// in anything a user wrote, and that release's own checks rule them out.
static VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    let schema: Value = serde_json::from_str(athenaeum::manifest::SCHEMA)
        .expect("the pinned manifest schema to be JSON");
    jsonschema::validator_for(&schema).expect("the pinned manifest schema to be a JSON Schema")
});

/// Check the text of a manifest against the schema.
///
/// # Errors
///
/// Returns why the manifest is not valid: either the text is not TOML, or it
/// is and breaks one or more rules, each reported with where and why.
pub fn check(text: &str) -> Result<(), Invalid> {
    let manifest: Value =
        toml::from_str(text).map_err(|error| Invalid::NotToml(error.to_string()))?;

    let violations: Vec<Violation> = VALIDATOR
        .iter_errors(&manifest)
        .map(|error| Violation {
            path: path_in(&manifest, error.instance_path().as_str()),
            keyword: error.kind().keyword().to_owned(),
            message: error.to_string(),
        })
        .collect();

    if violations.is_empty() {
        Ok(())
    } else {
        Err(Invalid::Violations(violations))
    }
}

/// Rewrite a JSON Pointer into the form the cases use, `$.a.b[0]`.
///
/// A pointer does not say whether `0` is an index or a key, so the manifest
/// is walked alongside it: a segment is an index only where the value it
/// steps into is an array.
fn path_in(manifest: &Value, pointer: &str) -> String {
    let mut path = String::from("$");
    let mut here = Some(manifest);

    for segment in pointer.split('/').skip(1) {
        let key = segment.replace("~1", "/").replace("~0", "~");
        let index = match here {
            Some(Value::Array(_)) => key.parse::<usize>().ok(),
            _ => None,
        };

        if let Some(index) = index {
            let _ = write!(path, "[{index}]");
            here = here.and_then(|value| value.get(index));
        } else {
            path.push('.');
            path.push_str(&key);
            here = here.and_then(|value| value.get(&key));
        }
    }

    path
}
