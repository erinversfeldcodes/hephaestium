//! The executor's secrets: where they come from, and where they are held.
//!
//! A secret is the variable of its name or the contents of the file the
//! variable `<NAME>_FILE` names, a file the maintainer keeps outside any
//! repository with the mode a private key has. Both are taken once, at the
//! start, and removed from this process's environment, so that a bootstrap step,
//! a gate or the agent, which run the repository's code and the agent's changes
//! to it, can neither read a secret nor learn where it is kept. A secret reaches
//! only the step that needs it.

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt as _;

/// The name of the variable the token for delivery is given in.
pub const GITHUB_TOKEN: &str = "HEPH_GITHUB_TOKEN";

/// Every secret the executor knows. An adapter's plan names the ones its agent
/// may have.
pub const KNOWN: [&str; 2] = [GITHUB_TOKEN, "CLAUDE_CODE_OAUTH_TOKEN"];

/// The secrets taken from the environment.
#[derive(Debug, Default)]
pub struct Secrets(BTreeMap<String, String>);

impl Secrets {
    /// What a secret is, if it was given.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    /// Take each named secret from the environment, from the variable or from
    /// the file its `_FILE` variable names, and remove both from this process's
    /// environment.
    ///
    /// # Errors
    ///
    /// Returns what is wrong with a secret: both a variable and a file are
    /// given, or the file is missing, not a regular file, readable by the group
    /// or by others, or empty. A secret's value is never part of the message.
    ///
    /// # Safety
    ///
    /// Must be called before any other thread has been started, because it
    /// removes variables from the environment, which is not safe while another
    /// thread may be reading it.
    pub unsafe fn take(names: &[&str]) -> Result<Self, String> {
        let mut secrets = BTreeMap::new();
        for name in names {
            let pointer = format!("{name}_FILE");
            let variable = std::env::var(name).ok().filter(|value| !value.is_empty());
            let file = std::env::var(&pointer)
                .ok()
                .filter(|value| !value.is_empty());
            // SAFETY: the caller guarantees there is no other thread.
            unsafe {
                std::env::remove_var(name);
                std::env::remove_var(&pointer);
            }
            let value = match (variable, file) {
                (Some(_), Some(_)) => {
                    return Err(format!("{name} and {pointer} are both set: give one"));
                }
                (Some(value), None) => value,
                (None, Some(path)) => read(&pointer, &path)?,
                (None, None) => continue,
            };
            secrets.insert((*name).to_owned(), value);
        }
        Ok(Self(secrets))
    }
}

/// Read a secret from a file that only its owner can read.
fn read(pointer: &str, path: &str) -> Result<String, String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("{pointer}: {path}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{pointer}: {path} is not a regular file"));
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(format!(
            "{pointer}: {path} can be read by others (mode {:03o}); run: chmod 600 {path}",
            metadata.mode() & 0o777
        ));
    }
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{pointer}: {path}: {error}"))?;
    let value = text.trim();
    if value.is_empty() {
        return Err(format!("{pointer}: {path} is empty"));
    }
    Ok(value.to_owned())
}
