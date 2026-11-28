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
//! `record.json` says. A run with no record may still be going, so its checkout is
//! removed only when nothing holds the run's lock and no process works under its
//! directory: then it ended without writing one.

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
    /// The disk space those checkouts held, in bytes.
    pub freed: u64,
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

/// The directories a build leaves in a checkout, named for what they hold.
pub const BUILD_OUTPUT: [&str; 5] = ["target", "_build", "deps", "node_modules", ".tool-home"];

/// Remove the build output from a run's checkout, wherever in it that sits, and say how
/// many directories went. The source, and what the run said beside the checkout, stay.
/// A link is removed and not followed, and `.git` is not entered. A directory with
/// anything git tracks in it is source, so it stays and is searched for build output.
///
/// # Errors
///
/// What the file system said, naming the directory.
pub fn discard_build_output(run: &Path) -> Result<usize, String> {
    discard_in(&run.join("work"), true, None)
}

/// Whether git tracks anything under `path`, which lies in the checkout at `repo`. A
/// directory that holds tracked files is source whatever it is called.
fn tracks_anything(repo: &Path, path: &Path) -> Result<bool, String> {
    let relative = path.strip_prefix(repo).unwrap_or(path);
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "-z", "--"])
        .arg(relative)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .output()
        .map_err(|error| format!("could not ask git about {}: {error}", path.display()))?;
    if !output.status.success() {
        return Err(format!(
            "git could not say whether it tracks {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(!output.stdout.is_empty())
}

fn discard_in(directory: &Path, top: bool, repo: Option<&Path>) -> Result<usize, String> {
    let repo = if directory.join(".git").exists() {
        Some(directory)
    } else {
        repo
    };
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        // A run with no checkout has nothing to discard.
        Err(error) if top && error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(format!("could not read {}: {error}", directory.display()));
        }
    };
    let mut removed = 0;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("could not read in {}: {error}", directory.display()))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let built = BUILD_OUTPUT.contains(&name.as_ref());
        if built && kind.is_symlink() {
            fs::remove_file(&path)
                .map_err(|error| format!("could not remove {}: {error}", path.display()))?;
            removed += 1;
        } else if built
            && kind.is_dir()
            && !repo.map_or(Ok(false), |r| tracks_anything(r, &path))?
        {
            fs::remove_dir_all(&path)
                .map_err(|error| format!("could not remove {}: {error}", path.display()))?;
            removed += 1;
        } else if kind.is_dir() && name != ".git" {
            removed += discard_in(&path, false, repo)?;
        }
    }
    Ok(removed)
}

/// Delete every checkout under `runs` that is due: those of runs that completed, and
/// those of runs that ended any other way more than `keep` ago.
///
/// Only a real directory named `<ticket>-<workload id>`, under a real directory of a
/// repository, with a record that says how the run ended, is touched. A link is never
/// entered. A run with no record is also removed when nothing holds it.
#[must_use]
pub fn sweep(runs: &Path, keep: Duration, now: SystemTime) -> Swept {
    let mut swept = Swept::default();
    for repository in directories(runs) {
        for run in directories(&repository) {
            let named = run
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_run_name);
            if !named {
                continue;
            }
            let abandoned = !has_record(&run) && !in_use(&run);
            if abandoned || due(&run, keep, now) {
                let size = disk_use(&run.join("work"));
                match delete_checkout(&run) {
                    Ok(true) => {
                        swept.deleted += 1;
                        swept.freed += size;
                        if abandoned {
                            eprintln!(
                                "heph: removed the checkout of {}, which ended without a record and is held by nothing, freeing {}",
                                run.display(),
                                show_size(size)
                            );
                        }
                    }
                    Ok(false) => {}
                    Err(warning) => swept.warnings.push(warning),
                }
            }
        }
    }
    swept
}

/// Whether the run has any `record.json` entry, valid or not.
fn has_record(run: &Path) -> bool {
    fs::symlink_metadata(run.join("record.json")).is_ok()
}

/// Whether the run may still be going: its lock is held, or a process works under it.
fn in_use(run: &Path) -> bool {
    crate::run_lock::is_held(run) || !crate::process::processes_under(run).is_empty()
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

/// The change an earlier attempt at the same ticket left on this worker, as a patch file.
#[derive(Debug, PartialEq, Eq)]
pub struct Continuation {
    /// The earlier workload's id.
    pub attempt: String,
    /// The patch, written in the new run's directory.
    pub patch: std::path::PathBuf,
}

/// Find the newest earlier run of the same ticket in this run's repository directory that
/// ended failed or timed out and still has its checkout, and write the change it left as
/// `continued.patch` in `run`. `Ok(None)` is no such run on this worker: another worker has
/// it, or the checkout is gone, and the run starts from the base. `Err` says why a run that
/// was found could not be continued.
///
/// Nothing from the old checkout is run or copied. The change is read with git's own
/// plumbing, with external diff and text conversion off, and only after the checkout's git
/// configuration is seen to hold nothing that names a program for git to run. It is read
/// through a temporary index, so the old checkout is not changed.
///
/// # Errors
///
/// Why the change of a found attempt could not be taken.
pub fn earlier_change(run: &Path, own_id: &str) -> Result<Option<Continuation>, String> {
    let (Some(repository), Some(name)) = (run.parent(), run.file_name().and_then(|n| n.to_str()))
    else {
        return Ok(None);
    };
    let Some((ticket, _)) = name.split_once('-') else {
        return Ok(None);
    };
    let Some((earlier, id)) = newest_unfinished(repository, ticket, own_id) else {
        return Ok(None);
    };
    let base_of = |dir: &Path| -> Option<String> {
        let text = fs::read_to_string(dir.join("workload.json")).ok()?;
        let value: Value = serde_json::from_str(&text).ok()?;
        value["repository"]["baseCommit"]
            .as_str()
            .or_else(|| value["repository"]["base_commit"].as_str())
            .map(str::to_owned)
    };
    let (Some(then), Some(now)) = (base_of(&earlier), base_of(run)) else {
        return Err(format!(
            "attempt {id} or this run does not name its base commit"
        ));
    };
    if then != now {
        return Err(format!(
            "the base has moved since attempt {id}, so its change may be stale"
        ));
    }
    let patch = run.join("continued.patch");
    let bytes = change_since(&earlier.join("work").join(&id), &then)
        .map_err(|why| format!("attempt {id}'s change could not be taken: {why}"))?;
    if bytes.is_empty() {
        return Err(format!("attempt {id} left no change"));
    }
    fs::write(&patch, bytes).map_err(|e| format!("could not write {}: {e}", patch.display()))?;
    Ok(Some(Continuation { attempt: id, patch }))
}

/// The newest earlier run of `ticket` in `repository` that ended failed or timed out and still
/// has its checkout, with its workload id.
fn newest_unfinished(
    repository: &Path,
    ticket: &str,
    own_id: &str,
) -> Option<(std::path::PathBuf, String)> {
    let mut found: Vec<(SystemTime, std::path::PathBuf, String)> = Vec::new();
    for earlier in directories(repository) {
        let Some(earlier_name) = earlier.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((earlier_ticket, id)) = earlier_name.split_once('-') else {
            continue;
        };
        if !is_run_name(earlier_name) || earlier_ticket != ticket || id == own_id {
            continue;
        }
        let record = earlier.join("record.json");
        let Ok(metadata) = fs::symlink_metadata(&record) else {
            continue;
        };
        let ended_unfinished = metadata.file_type().is_file()
            && fs::read_to_string(&record)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .and_then(|value| value["outcome"].as_str().map(str::to_owned))
                .is_some_and(|o| o == "OUTCOME_FAILED" || o == "OUTCOME_TIMED_OUT");
        let checkout = earlier.join("work").join(id);
        let is_checkout = fs::symlink_metadata(&checkout).is_ok_and(|m| m.is_dir());
        if ended_unfinished
            && is_checkout
            && let Ok(modified) = metadata.modified()
        {
            let id = id.to_owned();
            found.push((modified, earlier.clone(), id));
        }
    }
    found.sort();
    found.pop().map(|(_, earlier, id)| (earlier, id))
}

/// Makes the names of the temporary indexes one process uses differ.
static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// What a checkout holds beyond `base`: commits, edits and new files, as a binary patch.
fn change_since(checkout: &Path, base: &str) -> Result<Vec<u8>, String> {
    let git = |args: &[&str], index: Option<&Path>| {
        let mut command = std::process::Command::new("git");
        command
            .arg("-C")
            .arg(checkout)
            .args([
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0");
        if let Some(index) = index {
            command.env("GIT_INDEX_FILE", index);
        }
        let output = command
            .output()
            .map_err(|e| format!("could not run git: {e}"))?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
        }
    };
    let config = git(&["config", "--local", "--list"], None)?;
    for line in String::from_utf8_lossy(&config).lines() {
        let key = line.split('=').next().unwrap_or_default().to_lowercase();
        let runs_a_program = key.starts_with("filter.")
            || key.starts_with("diff.")
            || key.starts_with("include")
            || key.starts_with("core.fsmonitor")
            || key.starts_with("core.hookspath")
            || key.starts_with("core.attributesfile")
            || key.starts_with("core.pager")
            || key.starts_with("core.sshcommand")
            || key.starts_with("core.gitproxy");
        if runs_a_program {
            return Err(format!("its git configuration names a program ({key})"));
        }
    }
    let scratch = std::env::temp_dir().join(format!(
        "heph-index-{}-{}",
        std::process::id(),
        COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    // A scratch index left from an earlier process with this name would be read, not
    // overwritten, by add; a failed removal means it did not exist.
    let _ = fs::remove_file(&scratch);
    let result = git(&["read-tree", "HEAD"], Some(&scratch))
        .and_then(|_| git(&["add", "-A", "--", "."], Some(&scratch)))
        .and_then(|_| {
            git(
                &[
                    "diff",
                    "--cached",
                    "--binary",
                    "--no-ext-diff",
                    "--no-textconv",
                    base,
                ],
                Some(&scratch),
            )
        });
    // A scratch index left in the temporary directory costs nothing and is of no use to anyone.
    let _ = fs::remove_file(&scratch);
    result
}

/// A run's checkout that has gone unwritten for longer than a worker keeps one, and
/// that a control plane may still need.
#[derive(Debug, PartialEq, Eq)]
pub struct Old {
    /// The workload's id, from the run's directory name.
    pub id: String,
    /// The run's directory.
    pub run: std::path::PathBuf,
}

/// What a worker's clearing of old checkouts did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cleared {
    /// How many checkouts were removed.
    pub removed: usize,
    /// The disk space they held, in bytes.
    pub freed: u64,
    /// What could not be done, one line each.
    pub warnings: Vec<String>,
}

/// The runs under `runs` that have a checkout and whose newest file is older than `keep`.
/// Whether the run has ended is not said here: a worker's directory has no database, and
/// the control plane is the one that knows which workloads it still holds a lease on or
/// has a pull request open for. Only a real directory named `<ticket>-<workload id>`
/// under a real directory is looked at.
#[must_use]
pub fn old_checkouts(runs: &Path, keep: Duration, now: SystemTime) -> Vec<Old> {
    let mut found = Vec::new();
    for repository in directories(runs) {
        for run in directories(&repository) {
            let Some(name) = run.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !is_run_name(name) || fs::symlink_metadata(run.join("work")).is_err() {
                continue;
            }
            let last_written = ["record.json", "executor.log", "workload.json"]
                .iter()
                .filter_map(|file| fs::symlink_metadata(run.join(file)).ok()?.modified().ok())
                .max();
            let idle = last_written.and_then(|written| now.duration_since(written).ok());
            if idle.is_some_and(|idle| idle >= keep) {
                let id = name.split_once('-').map_or("", |(_, id)| id).to_owned();
                found.push(Old { id, run });
            }
        }
    }
    found
}

/// Remove the checkout of each of these runs, and say how many and how much space that
/// freed. What a run said (its record, events and logs) is left.
#[must_use]
pub fn clear(old: &[Old]) -> Cleared {
    let mut cleared = Cleared::default();
    for run in old {
        let size = disk_use(&run.run.join("work"));
        match delete_checkout(&run.run) {
            Ok(true) => {
                cleared.removed += 1;
                cleared.freed += size;
            }
            Ok(false) => {}
            Err(warning) => cleared.warnings.push(warning),
        }
    }
    cleared
}

/// The disk space a tree uses, counting a link and not what it points to.
fn disk_use(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    let own = metadata.blocks() * 512;
    if !metadata.is_dir() {
        return own;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return own;
    };
    own + entries
        .flatten()
        .map(|entry| disk_use(&entry.path()))
        .sum::<u64>()
}

/// A size for a person: bytes up to gigabytes, one decimal.
#[must_use]
pub fn show_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    #[expect(clippy::cast_precision_loss, reason = "a size shown to one decimal")]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A new empty directory of its own, under the system's temporary directory.
    pub(crate) struct Scratch(std::path::PathBuf);

    impl Scratch {
        pub(crate) fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("heph-workspaces-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("a directory");
            Self(path)
        }
        pub(crate) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    pub(crate) fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("a parent")).expect("directories");
        fs::write(path, text).expect("a file");
    }

    fn run_with_output(root: &Path) {
        for kept in ["record.json", "events.jsonl", "executor.log"] {
            write(&root.join(kept), "kept");
        }
        write(&root.join("work/repo/src/main.rs"), "source");
        // A real repository, because the discard asks git what is tracked.
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(root.join("work/repo"))
            .status()
            .expect("git");
        assert!(status.success(), "git init");
        for built in BUILD_OUTPUT {
            write(
                &root.join("work/repo").join(built).join("big/file"),
                "built",
            );
        }
        write(
            &root.join("work/repo/control-plane/node_modules/a/b.js"),
            "x",
        );
        write(&root.join("work/.tool-home/cache"), "x");
    }

    #[test]
    fn build_output_goes_and_source_and_record_stay() {
        let dir = Scratch::new("tree");
        let run = dir.path();
        run_with_output(run);

        let removed = discard_build_output(run).expect("to discard");

        assert_eq!(removed, 7);
        for built in BUILD_OUTPUT {
            assert!(!run.join("work/repo").join(built).exists(), "{built}");
        }
        assert!(!run.join("work/repo/control-plane/node_modules").exists());
        assert!(!run.join("work/.tool-home").exists());
        assert!(run.join("work/repo/src/main.rs").exists());
        assert!(run.join("work/repo/.git/HEAD").exists());
        for kept in ["record.json", "events.jsonl", "executor.log"] {
            assert!(run.join(kept).exists(), "{kept}");
        }
    }

    #[test]
    fn a_link_named_like_build_output_is_removed_and_not_followed() {
        let dir = Scratch::new("link-run");
        let outside = Scratch::new("link-outside");
        write(&outside.path().join("precious"), "x");
        fs::create_dir_all(dir.path().join("work/repo")).expect("directories");
        std::os::unix::fs::symlink(outside.path(), dir.path().join("work/repo/target"))
            .expect("a link");

        discard_build_output(dir.path()).expect("to discard");

        assert!(!dir.path().join("work/repo/target").exists());
        assert!(outside.path().join("precious").exists());
    }

    #[test]
    fn a_run_with_no_checkout_has_nothing_to_discard() {
        let dir = Scratch::new("empty");
        assert_eq!(discard_build_output(dir.path()), Ok(0));
    }

    #[test]
    fn a_subdirectory_that_cannot_be_read_is_reported() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = Scratch::new("unreadable");
        let locked = dir.path().join("work/repo/locked");
        fs::create_dir_all(&locked).expect("to create");
        fs::create_dir_all(dir.path().join("work/repo/target")).expect("to create");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("to lock");
        let result = discard_build_output(dir.path());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("to unlock");
        let error = result.expect_err("an unreadable directory to be reported");
        assert!(error.contains("locked"), "{error}");
    }

    const RUN: &str = "455-0b0e8f5e-5b8a-4c53-9a0e-3f1f2f0d9a11";

    /// A repository's runs directory with one run that has a checkout.
    fn runs_with_checkout(dir: &Scratch) -> std::path::PathBuf {
        let run = dir.path().join("owner-name").join(RUN);
        write(&run.join("work/repo/file"), "checkout");
        run
    }

    fn sweep_now(dir: &Scratch) -> Swept {
        sweep(dir.path(), Duration::from_secs(3600), SystemTime::now())
    }

    #[test]
    fn a_run_with_no_record_and_nothing_holding_it_loses_its_checkout() {
        let dir = Scratch::new("abandoned");
        let run = runs_with_checkout(&dir);
        write(&run.join("events.jsonl"), "said");

        let swept = sweep_now(&dir);

        assert_eq!(swept.deleted, 1);
        assert!(swept.freed > 0);
        assert!(!run.join("work").exists());
        assert!(run.join("events.jsonl").exists());
    }

    #[test]
    fn a_run_whose_lock_is_held_keeps_its_checkout() {
        let dir = Scratch::new("held");
        let run = runs_with_checkout(&dir);
        let _held = crate::run_lock::hold(&run).expect("to hold the run");

        let swept = sweep_now(&dir);

        assert_eq!(swept.deleted, 0);
        assert!(run.join("work/repo/file").exists());
    }

    #[test]
    fn a_run_with_a_process_under_it_keeps_its_checkout() {
        let dir = Scratch::new("process");
        let run = runs_with_checkout(&dir);
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .current_dir(run.join("work/repo"))
            .spawn()
            .expect("sleep");
        // The working directory is read from the system, so wait until it shows.
        let started = std::time::Instant::now();
        while crate::process::processes_under(&run).is_empty()
            && started.elapsed() < Duration::from_secs(10)
        {
            std::thread::sleep(Duration::from_millis(50));
        }

        let swept = sweep_now(&dir);
        let _ = child.kill();
        let _ = child.wait();

        assert_eq!(swept.deleted, 0);
        assert!(run.join("work/repo/file").exists());
    }

    #[test]
    fn a_run_with_a_record_is_left_to_the_retention_rule() {
        let dir = Scratch::new("recorded");
        let run = runs_with_checkout(&dir);
        write(&run.join("record.json"), r#"{"outcome":"OUTCOME_FAILED"}"#);

        let swept = sweep_now(&dir);

        assert_eq!(swept.deleted, 0);
        assert!(run.join("work/repo/file").exists());
    }

    pub(crate) const EARLIER: &str = "512-0b0e8f5e-5b8a-4c53-9a0e-3f1f2f0d9a11";
    pub(crate) const LATER: &str = "512-1c1f9a6f-6c9b-4d64-8b1f-4a2a3a1e0b22";

    fn git(directory: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .expect("git");
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// A repository's runs directory holding an earlier run of ticket 512 that ended as
    /// `outcome`, whose checkout holds a committed change and an untracked file, and a later
    /// run's directory. Returns the later run's directory, the source and the base commit.
    pub(crate) fn earlier_run(
        dir: &Scratch,
        outcome: &str,
    ) -> (std::path::PathBuf, std::path::PathBuf, String) {
        let source = dir.path().join("source");
        write(&source.join("README.md"), "base\n");
        git(dir.path(), &["init", "-q", source.to_str().expect("utf8")]);
        git(&source, &["add", "."]);
        git(&source, &["commit", "-q", "-m", "base"]);
        let base = git(&source, &["rev-parse", "HEAD"]);

        let repository = dir.path().join("runs/owner-name");
        let earlier = repository.join(EARLIER);
        let checkout = earlier.join("work").join(&EARLIER[4..]);
        git(
            dir.path(),
            &[
                "clone",
                "-q",
                source.to_str().expect("utf8"),
                checkout.to_str().expect("utf8"),
            ],
        );
        write(&checkout.join("committed.txt"), "committed\n");
        git(&checkout, &["add", "."]);
        git(&checkout, &["commit", "-q", "-m", "refused by a lens"]);
        write(&checkout.join("untracked.txt"), "untracked\n");
        write(
            &earlier.join("record.json"),
            &format!(r#"{{"outcome":"{outcome}"}}"#),
        );
        let workload = format!(r#"{{"repository":{{"baseCommit":"{base}"}}}}"#);
        write(&earlier.join("workload.json"), &workload);

        let later = repository.join(LATER);
        write(&later.join("workload.json"), &workload);
        (later, source, base)
    }

    #[test]
    fn a_second_workload_on_the_same_worker_after_a_refusal_carries_the_earlier_change() {
        let dir = Scratch::new("continue");
        let (later, source, base) = earlier_run(&dir, "OUTCOME_FAILED");

        let continued = earlier_change(&later, &LATER[4..])
            .expect("to find the change")
            .expect("an earlier attempt");

        assert_eq!(continued.attempt, EARLIER[4..]);
        // Applied to a fresh clone at the base, it brings both the commit and the new file.
        let fresh = dir.path().join("fresh");
        git(
            dir.path(),
            &[
                "clone",
                "-q",
                source.to_str().expect("utf8"),
                fresh.to_str().expect("utf8"),
            ],
        );
        git(&fresh, &["checkout", "-q", "--detach", &base]);
        git(&fresh, &["apply", continued.patch.to_str().expect("utf8")]);
        assert_eq!(
            fs::read_to_string(fresh.join("committed.txt")).expect("the commit"),
            "committed\n"
        );
        assert_eq!(
            fs::read_to_string(fresh.join("untracked.txt")).expect("the new file"),
            "untracked\n"
        );
    }

    #[test]
    fn a_retry_on_another_worker_finds_no_earlier_change_and_starts_fresh() {
        let dir = Scratch::new("elsewhere");
        let (_, _, _) = earlier_run(&dir, "OUTCOME_FAILED");
        // Another worker's runs directory holds only this run.
        let other = dir.path().join("other/owner-name").join(LATER);
        write(&other.join("workload.json"), "{}");

        assert_eq!(earlier_change(&other, &LATER[4..]), Ok(None));
        assert!(!other.join("continued.patch").exists());
    }

    #[test]
    fn a_completed_earlier_run_is_not_continued() {
        let dir = Scratch::new("completed");
        let (later, _, _) = earlier_run(&dir, "OUTCOME_COMPLETED");
        assert_eq!(earlier_change(&later, &LATER[4..]), Ok(None));
    }

    #[test]
    fn a_timed_out_earlier_run_is_continued() {
        let dir = Scratch::new("timed-out");
        let (later, _, _) = earlier_run(&dir, "OUTCOME_TIMED_OUT");
        let found = earlier_change(&later, &LATER[4..]);
        assert!(matches!(found, Ok(Some(_))), "{found:?}");
    }

    #[test]
    fn a_base_that_has_moved_starts_fresh_and_says_why() {
        let dir = Scratch::new("stale");
        let (later, _, _) = earlier_run(&dir, "OUTCOME_FAILED");
        write(
            &later.join("workload.json"),
            r#"{"repository":{"baseCommit":"0000000000000000000000000000000000000000"}}"#,
        );
        let error = earlier_change(&later, &LATER[4..]).expect_err("a stale base");
        assert!(error.contains("base has moved"), "{error}");
    }

    #[test]
    fn a_checkout_whose_git_configuration_names_a_program_is_not_read() {
        let dir = Scratch::new("filter");
        let (later, _, _) = earlier_run(&dir, "OUTCOME_FAILED");
        let checkout = dir
            .path()
            .join("runs/owner-name")
            .join(EARLIER)
            .join("work")
            .join(&EARLIER[4..]);
        git(&checkout, &["config", "filter.x.clean", "touch ran"]);
        let error = earlier_change(&later, &LATER[4..]).expect_err("a program to be refused");
        assert!(error.contains("names a program"), "{error}");
        assert!(!checkout.join("ran").exists());
    }
}
