//! Checking text against a JSON Schema, and saying where it breaks it.
//!
//! The contracts are TOML files that Athenaeum defines with JSON Schemas. This
//! is what every one of them is checked with, so that a violation is reported
//! the same way whichever contract it breaks: where in the file, which
//! keyword rejected it, and in words.

use std::fmt::Write as _;

use jsonschema::Validator;
use serde_json::Value;

/// One place where an instance breaks one rule of the schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Where in the instance the rule is broken, as `$.a.b[0]`.
    pub path: String,
    /// The JSON Schema keyword that rejects it.
    pub keyword: String,
    /// What is wrong, in words.
    pub message: String,
}

/// Why a manifest is not valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invalid {
    /// The text is not TOML, so there is nothing to check.
    NotToml(String),
    /// The manifest is TOML and breaks these rules of the schema.
    Violations(Vec<Violation>),
}

/// Compile the text of a schema.
///
/// # Panics
///
/// If the text is not a JSON Schema. That would be a defect in the pinned
/// Athenaeum release, and that release's own checks rule it out.
#[must_use]
pub fn compile(schema_text: &str) -> Validator {
    let schema: Value = serde_json::from_str(schema_text).expect("a pinned schema to be JSON");
    jsonschema::validator_for(&schema).expect("a pinned schema to be a JSON Schema")
}

/// Check the text of a TOML file against a compiled schema.
///
/// # Errors
///
/// Returns why the text is not valid: either it is not TOML, or it is and
/// breaks one or more rules, each reported with where and why.
pub fn validate(validator: &Validator, text: &str) -> Result<(), Invalid> {
    let instance: Value =
        toml::from_str(text).map_err(|error| Invalid::NotToml(error.to_string()))?;

    let violations: Vec<Violation> = validator
        .iter_errors(&instance)
        .map(|error| Violation {
            path: path_in(&instance, error.instance_path().as_str()),
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
