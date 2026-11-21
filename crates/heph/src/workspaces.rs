//! The checkouts that `heph run` leaves behind.
//!
//! A run's directory holds what the run said (`workload.json`, `events.jsonl`,
//! `executor.log` and `record.json`, all small) and its `work/` directory, a full
//! checkout that can be gigabytes. Nothing reads the checkout once the run has
//! ended, so a run that completed deletes it, and a run that ended any other way
//! keeps it for a window, in which someone can look at what the agent left, and
//! is cleaned up by the next `heph run`.
//!
//! There is no database here to ask whether a run has ended, so the run's own
//! `record.json` says: a run with no record may still be going, and is never touched.

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::Value;

/// How long a run that did not complete keeps its checkout, in hours.
pub const DEFAULT_KEEP_HOURS: u64 = 72;

/// What a sweep did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Swept {
    /// How many checkouts were deleted.
    pub deleted: usize,
    /// What could not be done, one line each.
    pub warnings: Vec<String>,
}

/// Delete the checkout of one run's directory, if it has one. A link is removed and
/// not followed. An error is a line to show, not a reason to stop.
///
/// # Errors
///
/// What the file system said, naming the checkout.
pub fn delete_checkout(run: &Path) -> Result<bool, String> {
    let work = run.join("work");
    let Ok(metadata) = fs::symlink_metadata(&work) else {
        return Ok(false);
    };
    let result = if metadata.is_dir() {
        fs::remove_dir_all(&work)
    } else if metadata.file_type().is_symlink() {
        fs::remove_file(&work)
    } else {
        return Ok(false);
    };
    result
        .map(|()| true)
        .map_err(|error| format!("could not delete the checkout {}: {error}", work.display()))
}

/// Delete every checkout under `runs` that is due: those of runs that completed, and
/// those of runs that ended any other way more than `keep` ago.
///
/// Only a real directory named `<ticket>-<workload id>`, under a real directory of a
/// repository, with a record that says how the run ended, is touched. A link is never
/// entered.
#[must_use]
pub fn sweep(runs: &Path, keep: Duration, now: SystemTime) -> Swept {
    let mut swept = Swept::default();
    for repository in directories(runs) {
        for run in directories(&repository) {
            let named = run
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_run_name);
            if named && due(&run, keep, now) {
                match delete_checkout(&run) {
                    Ok(true) => swept.deleted += 1,
                    Ok(false) => {}
                    Err(warning) => swept.warnings.push(warning),
                }
            }
        }
    }
    swept
}

/// The real directories in a directory. A link is not one, and a directory that is
/// not there has none.
fn directories(path: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    let mut found: Vec<_> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

/// Whether the run's checkout is due, from its record.
fn due(run: &Path, keep: Duration, now: SystemTime) -> bool {
    let record = run.join("record.json");
    let Ok(metadata) = fs::symlink_metadata(&record) else {
        return false;
    };
    if !metadata.file_type().is_file() {
        return false;
    }
    let Some(outcome) = fs::read_to_string(&record)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value["outcome"].as_str().map(str::to_owned))
    else {
        return false;
    };
    match outcome.as_str() {
        // Said nothing about how it ended.
        "OUTCOME_UNSPECIFIED" => false,
        "OUTCOME_COMPLETED" => true,
        _ => metadata
            .modified()
            .ok()
            .and_then(|ended| now.duration_since(ended).ok())
            .is_some_and(|age| age >= keep),
    }
}

/// `<ticket>-<workload id>`: digits, a hyphen, and a UUID.
fn is_run_name(name: &str) -> bool {
    let Some((ticket, id)) = name.split_once('-') else {
        return false;
    };
    !ticket.is_empty()
        && ticket.bytes().all(|b| b.is_ascii_digit())
        && id.len() == 36
        && id.bytes().enumerate().all(|(at, b)| {
            if matches!(at, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
