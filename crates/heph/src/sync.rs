//! Making a repository's vendored skills match its lock.
//!
//! The lock pins an Athenaeum release. Syncing fetches that release with
//! `git`, copies the skills the lock names into `.athenaeum/skills/`, where a
//! person can review them, and links each for the agent runtimes the lock
//! names, where that runtime looks.
//!
//! Nothing is written until everything has been checked. A repository that
//! cannot be synced is left exactly as it was.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::lock::{Harness, Lock};

/// Where the skills a lock names are vendored, relative to the repository.
pub const VENDORED: &str = ".athenaeum/skills";

/// Where Claude Code looks for skills, relative to the repository.
const CLAUDE_CODE_SKILLS: &str = ".claude/skills";

/// Where a link in `.claude/skills` points, relative to the link itself. It is
/// relative so that the repository means the same thing wherever it is cloned.
const LINKS_INTO: &str = "../../.athenaeum/skills";

/// Why a repository could not be synced.
#[derive(Debug)]
pub enum SyncError {
    /// `git` could not be run, or failed.
    Git(String),
    /// The tag the lock names now points at a different commit.
    PinMoved {
        /// The tag.
        rev: String,
        /// The commit the lock recorded.
        locked: String,
        /// The commit the tag points at now.
        found: String,
    },
    /// The lock names a skill the pinned release does not have.
    NotInSource(String),
    /// The source has no release tags to move to.
    NoReleases(String),
    /// Something would be overwritten that sync did not create.
    WouldOverwrite(Vec<PathBuf>),
    /// Reading or writing a file failed.
    Io(String),
}

impl fmt::Display for SyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(message) | Self::Io(message) => formatter.write_str(message),
            Self::PinMoved { rev, locked, found } => write!(
                formatter,
                "the tag {rev} now points at {found}, but the lock records {locked}; \
                 a tag that has moved is not a pin, so nothing was changed"
            ),
            Self::NotInSource(skill) => write!(
                formatter,
                "the lock names the skill '{skill}', which the pinned release does not have"
            ),
            Self::NoReleases(source) => {
                write!(formatter, "{source} has no release tags of the form vX.Y.Z")
            }
            Self::WouldOverwrite(paths) => {
                let listed: Vec<String> = paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect();
                write!(
                    formatter,
                    "would overwrite what sync did not create: {}; move it away or remove the \
                     skill from the lock, and nothing was changed",
                    listed.join(", ")
                )
            }
        }
    }
}

fn io_error(path: &Path, error: &io::Error) -> SyncError {
    SyncError::Io(format!("{}: {error}", path.display()))
}

/// A fetched copy of Athenaeum at one release, removed when dropped.
pub struct Upstream {
    directory: PathBuf,
    /// The commit the release resolved to.
    pub commit: String,
}

impl Upstream {
    fn skill(&self, name: &str) -> PathBuf {
        self.directory.join("skills").join(name)
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn git(arguments: &[&str], directory: Option<&Path>) -> Result<String, SyncError> {
    let mut command = Command::new("git");
    command.args(arguments).env("GIT_TERMINAL_PROMPT", "0");
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    let output = command
        .output()
        .map_err(|error| SyncError::Git(format!("git could not be run: {error}")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(SyncError::Git(format!(
            "git {} failed: {}",
            arguments.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Fetch the source at a release tag, and the commit it resolves to.
///
/// # Errors
///
/// Returns why the release could not be fetched.
pub fn fetch(source: &str, rev: &str) -> Result<Upstream, SyncError> {
    let nanoseconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let directory =
        std::env::temp_dir().join(format!("heph-sync-{}-{nanoseconds}", std::process::id()));

    let target = directory.to_string_lossy().into_owned();
    let cloned = git(
        &[
            "-c",
            "advice.detachedHead=false",
            "clone",
            "--quiet",
            "--depth",
            "1",
            "--branch",
            rev,
            "--",
            source,
            &target,
        ],
        None,
    );
    let mut upstream = Upstream {
        directory,
        commit: String::new(),
    };
    cloned?;

    upstream.commit = git(&["rev-parse", "HEAD"], Some(&upstream.directory))?;
    Ok(upstream)
}

/// The latest release tag of the source, by version number and not by name.
///
/// # Errors
///
/// Returns why the tags could not be listed, or that there are none.
pub fn latest_release(source: &str) -> Result<String, SyncError> {
    let listing = git(&["ls-remote", "--tags", "--refs", "--", source], None)?;
    listing
        .lines()
        .filter_map(|line| line.split('\t').nth(1))
        .filter_map(|reference| reference.strip_prefix("refs/tags/"))
        .filter_map(|tag| version_of(tag).map(|version| (version, tag)))
        .max_by_key(|(version, _)| *version)
        .map(|(_, tag)| tag.to_owned())
        .ok_or_else(|| SyncError::NoReleases(source.to_owned()))
}

/// The numbers in a tag of the form `vX.Y.Z`.
fn version_of(tag: &str) -> Option<(u64, u64, u64)> {
    let mut parts = tag.strip_prefix('v')?.split('.');
    let version = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(version)
}

/// Refuse a release whose tag no longer points where the lock says it did.
///
/// # Errors
///
/// Returns [`SyncError::PinMoved`] if the commits differ.
pub fn verify_pin(lock: &Lock, upstream: &Upstream) -> Result<(), SyncError> {
    if upstream.commit == lock.commit {
        Ok(())
    } else {
        Err(SyncError::PinMoved {
            rev: lock.rev.clone(),
            locked: lock.commit.clone(),
            found: upstream.commit.clone(),
        })
    }
}

/// One way the repository differs from what the lock says it should be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Difference {
    /// A skill the lock names is not vendored.
    Missing(String),
    /// A vendored skill differs from the release.
    Modified(String),
    /// A vendored skill the lock does not name.
    Extra(String),
    /// A skill is not linked for a runtime that the lock names.
    NotLinked(String),
    /// A link into the vendored skills that the lock no longer wants.
    StaleLink(String),
    /// Something sync did not create is where a link must go.
    Blocked(PathBuf),
}

impl fmt::Display for Difference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(skill) => write!(formatter, "missing: {skill}"),
            Self::Modified(skill) => write!(formatter, "modified: {skill}"),
            Self::Extra(skill) => write!(formatter, "extra: {skill}"),
            Self::NotLinked(skill) => write!(formatter, "not linked: {skill}"),
            Self::StaleLink(skill) => write!(formatter, "stale link: {skill}"),
            Self::Blocked(path) => write!(
                formatter,
                "blocked: {} exists and sync did not create it",
                path.display()
            ),
        }
    }
}

fn link_to(skill: &str) -> String {
    format!("{LINKS_INTO}/{skill}")
}

fn link_for(harness: Harness, skill: &str) -> PathBuf {
    match harness {
        Harness::ClaudeCode => Path::new(CLAUDE_CODE_SKILLS).join(skill),
    }
}

/// What is at the place a link belongs.
enum LinkState {
    Right,
    Absent,
    /// Something is there that is not the link sync would make.
    Foreign,
}

fn link_state(link: &Path, expected: &str) -> Result<LinkState, SyncError> {
    match fs::symlink_metadata(link) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(LinkState::Absent),
        Err(error) => Err(io_error(link, &error)),
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = fs::read_link(link).map_err(|error| io_error(link, &error))?;
            Ok(if target == Path::new(expected) {
                LinkState::Right
            } else {
                LinkState::Foreign
            })
        }
        Ok(_) => Ok(LinkState::Foreign),
    }
}

/// The entries of a directory, by name, with whether each is a directory.
fn entries(directory: &Path) -> Result<Vec<(OsString, bool)>, SyncError> {
    let mut found = Vec::new();
    for entry in fs::read_dir(directory).map_err(|error| io_error(directory, &error))? {
        let entry = entry.map_err(|error| io_error(directory, &error))?;
        let is_directory = entry
            .file_type()
            .map_err(|error| io_error(&entry.path(), &error))?
            .is_dir();
        found.push((entry.file_name(), is_directory));
    }
    found.sort();
    Ok(found)
}

/// Whether two directories hold the same files with the same contents.
fn same_tree(left: &Path, right: &Path) -> Result<bool, SyncError> {
    let (in_left, in_right) = (entries(left)?, entries(right)?);
    if in_left != in_right {
        return Ok(false);
    }
    for (name, is_directory) in in_left {
        let (a, b) = (left.join(&name), right.join(&name));
        let same = if is_directory {
            same_tree(&a, &b)?
        } else {
            fs::read(&a).map_err(|error| io_error(&a, &error))?
                == fs::read(&b).map_err(|error| io_error(&b, &error))?
        };
        if !same {
            return Ok(false);
        }
    }
    Ok(true)
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), SyncError> {
    fs::create_dir_all(to).map_err(|error| io_error(to, &error))?;
    for entry in fs::read_dir(from).map_err(|error| io_error(from, &error))? {
        let entry = entry.map_err(|error| io_error(from, &error))?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        let kind = entry
            .file_type()
            .map_err(|error| io_error(&source, &error))?;
        if kind.is_dir() {
            copy_tree(&source, &target)?;
        } else if kind.is_file() {
            fs::copy(&source, &target).map_err(|error| io_error(&source, &error))?;
        } else {
            return Err(SyncError::Io(format!(
                "{}: is a symbolic link, and a skill holds only files",
                source.display()
            )));
        }
    }
    Ok(())
}

/// How the repository at `root` differs from what the lock says, against the
/// release it pins. Changes nothing.
///
/// # Errors
///
/// Returns an error if the lock names a skill the release does not have, or a
/// file cannot be read.
pub fn differences(
    lock: &Lock,
    upstream: &Upstream,
    root: &Path,
) -> Result<Vec<Difference>, SyncError> {
    let vendored_root = root.join(VENDORED);
    let mut found = Vec::new();

    for skill in &lock.skills {
        let source = upstream.skill(skill);
        if !source.is_dir() {
            return Err(SyncError::NotInSource(skill.clone()));
        }

        let vendored = vendored_root.join(skill);
        if !vendored.exists() {
            found.push(Difference::Missing(skill.clone()));
        } else if !same_tree(&source, &vendored)? {
            found.push(Difference::Modified(skill.clone()));
        }

        for harness in &lock.harnesses {
            let link = link_for(*harness, skill);
            match link_state(&root.join(&link), &link_to(skill))? {
                LinkState::Right => {}
                LinkState::Absent => found.push(Difference::NotLinked(skill.clone())),
                LinkState::Foreign => found.push(Difference::Blocked(link)),
            }
        }
    }

    if vendored_root.is_dir() {
        for (name, is_directory) in entries(&vendored_root)? {
            let name = name.to_string_lossy().into_owned();
            if is_directory && !lock.skills.contains(&name) {
                found.push(Difference::Extra(name));
            }
        }
    }

    let links = root.join(CLAUDE_CODE_SKILLS);
    if links.is_dir() {
        for (name, _) in entries(&links)? {
            let name = name.to_string_lossy().into_owned();
            let path = links.join(&name);
            let ours =
                fs::read_link(&path).is_ok_and(|target| target == Path::new(&link_to(&name)));
            let wanted =
                lock.harnesses.contains(&Harness::ClaudeCode) && lock.skills.contains(&name);
            if ours && !wanted {
                found.push(Difference::StaleLink(name));
            }
        }
    }

    Ok(found)
}

/// Make the repository at `root` match the lock, reporting each change made
/// through `notify`.
///
/// # Errors
///
/// Returns [`SyncError::WouldOverwrite`] before changing anything if a place
/// a link belongs holds something sync did not create, and otherwise an error
/// if a file cannot be written.
pub fn apply(
    differences: &[Difference],
    upstream: &Upstream,
    root: &Path,
    mut notify: impl FnMut(&str),
) -> Result<(), SyncError> {
    let blocked: Vec<PathBuf> = differences
        .iter()
        .filter_map(|difference| match difference {
            Difference::Blocked(path) => Some(path.clone()),
            _ => None,
        })
        .collect();
    if !blocked.is_empty() {
        return Err(SyncError::WouldOverwrite(blocked));
    }

    let vendored_root = root.join(VENDORED);
    for difference in differences {
        match difference {
            Difference::Missing(skill) | Difference::Modified(skill) => {
                let target = vendored_root.join(skill);
                if target.exists() {
                    fs::remove_dir_all(&target).map_err(|error| io_error(&target, &error))?;
                }
                copy_tree(&upstream.skill(skill), &target)?;
                let verb = if matches!(difference, Difference::Missing(_)) {
                    "installed"
                } else {
                    "updated"
                };
                notify(&format!("{verb} {skill}"));
            }
            Difference::Extra(skill) => {
                let target = vendored_root.join(skill);
                fs::remove_dir_all(&target).map_err(|error| io_error(&target, &error))?;
                notify(&format!("removed {skill}"));
            }
            Difference::NotLinked(skill) => {
                let link = root.join(link_for(Harness::ClaudeCode, skill));
                if let Some(parent) = link.parent() {
                    fs::create_dir_all(parent).map_err(|error| io_error(parent, &error))?;
                }
                symlink(link_to(skill), &link).map_err(|error| io_error(&link, &error))?;
                notify(&format!("linked {skill}"));
            }
            Difference::StaleLink(skill) => {
                let link = root.join(link_for(Harness::ClaudeCode, skill));
                fs::remove_file(&link).map_err(|error| io_error(&link, &error))?;
                notify(&format!("unlinked {skill}"));
            }
            Difference::Blocked(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::version_of;

    #[test]
    fn a_release_tag_is_three_numbers_after_a_v() {
        assert_eq!(version_of("v0.10.2"), Some((0, 10, 2)));
        assert_eq!(version_of("v10.0.0"), Some((10, 0, 0)));
    }

    #[test]
    fn anything_else_is_not_a_release_tag() {
        for tag in [
            "0.1.0",
            "v1.2",
            "v1.2.3.4",
            "v1.2.x",
            "main",
            "v-1.2.3",
            "v1.2.3-rc1",
        ] {
            assert_eq!(version_of(tag), None, "{tag}");
        }
    }
}
