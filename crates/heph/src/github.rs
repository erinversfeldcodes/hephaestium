//! Reading a repository's settings and rulesets from GitHub.
//!
//! Everything here is a read. GitHub is reached by running `gh api` with an
//! endpoint and nothing else, so the credentials are the ones `gh` already
//! has and this tool never holds one, and a request cannot be anything but a
//! `GET`.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// What GitHub said to one read.
#[derive(Debug, Clone, PartialEq)]
pub enum Fetched {
    /// It answered. A response with no body is `Null`.
    Found(Value),
    /// It said there is no such thing, which for some settings means "off".
    NotFound,
    /// It refused or failed, with what `gh` said. The setting is unknown.
    Failed(String),
}

/// What is known of the repository's rulesets.
#[derive(Debug, Clone, PartialEq)]
pub enum Rulesets {
    /// The active branch rulesets, each as GitHub returns it by id.
    Known(Vec<Value>),
    /// They could not be read, with why.
    Unknown(String),
}

/// Everything read from GitHub for one repository.
#[derive(Debug)]
pub struct State {
    /// The repository itself.
    pub repository: Value,
    /// Whether Dependabot alerts are on: a success means on.
    pub alerts: Fetched,
    /// Dependabot security updates.
    pub security_fixes: Fetched,
    /// Code scanning's default setup.
    pub default_setup: Fetched,
    /// At least one code scanning analysis, as a list of at most one.
    pub analyses: Fetched,
    /// The active branch rulesets.
    pub rulesets: Rulesets,
}

/// Why GitHub could not be read at all.
#[derive(Debug)]
pub struct CouldNotRead(pub String);

impl std::fmt::Display for CouldNotRead {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Read one endpoint.
fn get(endpoint: &str) -> Result<Fetched, CouldNotRead> {
    let output = match Command::new("gh").args(["api", endpoint]).output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CouldNotRead(
                "the GitHub command-line tool, gh, is not installed or not on the path".to_owned(),
            ));
        }
        Err(error) => return Err(CouldNotRead(format!("gh could not be run: {error}"))),
    };
    let said = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if output.status.success() {
        let body = String::from_utf8_lossy(&output.stdout);
        if body.trim().is_empty() {
            return Ok(Fetched::Found(Value::Null));
        }
        return Ok(serde_json::from_str(&body).map_or_else(
            |error| Fetched::Failed(format!("{endpoint}: not JSON: {error}")),
            Fetched::Found,
        ));
    }
    if said.contains("HTTP 404") {
        return Ok(Fetched::NotFound);
    }
    Ok(Fetched::Failed(if said.is_empty() {
        format!("{endpoint}: gh failed")
    } else {
        said
    }))
}

/// Read what the baseline's settings and ruleset items need.
///
/// # Errors
///
/// Returns why GitHub could not be read at all: `gh` is not there, or the
/// repository itself could not be read. A setting that cannot be read is not an
/// error here: it is [`Fetched::Failed`], and the item is reported as not
/// checked.
pub fn read(repository: &str) -> Result<State, CouldNotRead> {
    let base = format!("repos/{repository}");

    let repo = match get(&base)? {
        Fetched::Found(value) => value,
        Fetched::NotFound => {
            return Err(CouldNotRead(format!(
                "{repository}: GitHub says there is no such repository, or it is not visible to gh"
            )));
        }
        Fetched::Failed(said) => return Err(CouldNotRead(format!("{repository}: {said}"))),
    };

    let rulesets = read_rulesets(&base)?;

    Ok(State {
        repository: repo,
        alerts: get(&format!("{base}/vulnerability-alerts"))?,
        security_fixes: get(&format!("{base}/automated-security-fixes"))?,
        default_setup: get(&format!("{base}/code-scanning/default-setup"))?,
        analyses: get(&format!("{base}/code-scanning/analyses?per_page=1"))?,
        rulesets,
    })
}

fn read_rulesets(base: &str) -> Result<Rulesets, CouldNotRead> {
    let listed = match get(&format!("{base}/rulesets"))? {
        Fetched::Found(Value::Array(listed)) => listed,
        Fetched::Found(_) => {
            return Ok(Rulesets::Unknown(
                "GitHub did not list the rulesets".to_owned(),
            ));
        }
        Fetched::NotFound => {
            return Ok(Rulesets::Unknown(
                "GitHub says the rulesets are not found, which it also says when they are not visible to gh"
                    .to_owned(),
            ));
        }
        Fetched::Failed(said) => return Ok(Rulesets::Unknown(said)),
    };

    let mut active = Vec::new();
    for summary in listed {
        if summary["enforcement"] != "active" || summary["target"] != "branch" {
            continue;
        }
        let Some(id) = summary["id"].as_u64() else {
            continue;
        };
        match get(&format!("{base}/rulesets/{id}"))? {
            Fetched::Found(detail) => active.push(detail),
            Fetched::NotFound => {
                return Ok(Rulesets::Unknown(format!("ruleset {id} could not be read")));
            }
            Fetched::Failed(said) => return Ok(Rulesets::Unknown(said)),
        }
    }
    Ok(Rulesets::Known(active))
}

/// The repository, as `OWNER/NAME`, from `--repo` if given and otherwise from
/// the `origin` remote of the repository at `root`, if that is on github.com.
///
/// # Errors
///
/// Returns why the repository could not be named.
pub fn repository_of(root: &Path, flag: Option<&str>) -> Result<String, CouldNotRead> {
    if let Some(flag) = flag {
        return valid(flag)
            .ok_or_else(|| CouldNotRead(format!("--repo '{flag}' is not OWNER/NAME")));
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["remote", "get-url", "origin"])
        .output()
        .map_err(|error| {
            CouldNotRead(format!("git could not be run to find the remote: {error}"))
        })?;
    let url = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || url.is_empty() {
        return Err(CouldNotRead(
            "the repository has no origin remote; name it with --repo OWNER/NAME".to_owned(),
        ));
    }
    from_remote(&url).ok_or_else(|| {
        CouldNotRead(format!(
            "the origin remote '{url}' is not on github.com; name the repository with --repo OWNER/NAME"
        ))
    })
}

/// `owner/name` if that is what the text is.
fn valid(text: &str) -> Option<String> {
    let (owner, name) = text.split_once('/')?;
    let plain = |part: &str| {
        !part.is_empty()
            && part.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b'.'
            })
    };
    (plain(owner) && plain(name)).then(|| format!("{owner}/{name}"))
}

/// The repository a github.com remote URL names, as https, scp-style ssh or
/// `ssh://`.
fn from_remote(url: &str) -> Option<String> {
    let path = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    valid(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_github_remote_names_its_repository() {
        for url in [
            "https://github.com/owner/name.git",
            "https://github.com/owner/name",
            "https://github.com/owner/name/",
            "git@github.com:owner/name.git",
            "ssh://git@github.com/owner/name.git",
        ] {
            assert_eq!(from_remote(url).as_deref(), Some("owner/name"), "{url}");
        }
        for url in [
            "https://example.org/owner/name.git",
            "https://github.com/owner",
            "https://github.com/owner/name/extra",
            "git@gitlab.com:owner/name.git",
            "",
        ] {
            assert_eq!(from_remote(url), None, "{url}");
        }
    }
}
