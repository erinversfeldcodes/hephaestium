//! What the tools a workspace runs need, which the agent's own environment does not hold: a
//! home for `mix`, `npm` and `npx` to keep their packages in, and a database of the
//! workspace's own to test against.
//!
//! The home is shared by every workspace on the machine, so a package fetched once is not
//! fetched again. The database is one for each workspace, named for it, so two runs on one
//! machine never see each other's rows. It is made by the tests' own `mix test` and dropped
//! here when the run ends.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use contracts::v1alpha1::Workload;

/// Where the Postgres server that test databases are made on is, as `postgres://user:password@host:port`
/// with no database. Without it no workspace is given a database.
pub const SERVER: &str = "HEPH_TEST_DATABASE_SERVER";

/// The variable the control plane's tests read their connection from.
pub const DATABASE_URL: &str = "DATABASE_URL";

/// The longest a database name may be.
const NAME_LIMIT: usize = 63;

/// The home the tools of every workspace in `workspace`'s work directory share.
#[must_use]
pub fn home(workspace: &Path) -> PathBuf {
    workspace.parent().unwrap_or(workspace).join(".tool-home")
}

/// The name of the database a workspace tests against: the workload's own, so that no two
/// workspaces share one.
#[must_use]
pub fn database_name(workspace: &Path) -> String {
    let id = workspace
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let safe: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("heph_{safe}").chars().take(NAME_LIMIT).collect()
}

/// The connection to a workspace's database, when the executor has a server to make one on.
#[must_use]
pub fn database_url(server: Option<&str>, workspace: &Path) -> Option<String> {
    let server = server?.trim_end_matches('/');
    if server.is_empty() {
        return None;
    }
    Some(format!("{server}/{}", database_name(workspace)))
}

/// The variables a command run in the workspace is given besides what it names for itself.
///
/// `lookup` reads the executor's environment. The home is the shared one and not the
/// maintainer's, so that a workspace's tools see neither their credentials nor their
/// configuration. A review or an evaluation runs no test against a database, so it is not given one.
#[must_use]
pub fn environment(
    workload: &Workload,
    workspace: &Path,
    lookup: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let mut environment = vec![("HOME".to_owned(), home(workspace).display().to_string())];
    for name in ["USER", "LANG", "LC_ALL", "TERM", "TMPDIR"] {
        if let Some(value) = lookup(name) {
            environment.push((name.to_owned(), value));
        }
    }
    if !reads_only(workload)
        && let Some(url) = database_url(lookup(SERVER).as_deref(), workspace)
    {
        environment.push((DATABASE_URL.to_owned(), url));
    }
    environment
}

fn reads_only(workload: &Workload) -> bool {
    workload.review.is_some() || workload.completeness.is_some() || workload.gap.is_some()
}

/// What the agent is told it can run in this workspace, and how. Whether it has a database
/// changes what the control plane's tests can do, so the text says which it has.
#[must_use]
pub fn prompt_section(has_database: bool) -> String {
    let database = if has_database {
        "`DATABASE_URL` names a database of this workspace's own, which `mix test` creates and migrates and the runtime drops when the run ends, so no other run shares its rows."
    } else {
        "No database was given to this workspace, so a control plane test that needs one cannot be run here: say so in the description, and leave it to CI."
    };
    format!(
        "The dependencies of the control plane and of the dashboard are already fetched, and `mix`, `npm` and `npx` run in your environment. Run only the tests of what you change, and leave the whole suites to CI: `cd control-plane && mix test test/path/to_test.exs` for the control plane, and `cd dashboard && npx vitest run src/name.test.ts` for the dashboard. {database} Quote the last lines of what you ran in the description, as the proof of it.\n\n"
    )
}

/// A database that lives for the whole run and is dropped when the run ends, whichever way it
/// ends, and not after each script the agent runs.
pub struct Owned {
    server: String,
    name: String,
    dropped: std::cell::Cell<bool>,
}

impl Owned {
    /// The database the workspace is given, if it is given one.
    #[must_use]
    pub fn of(
        workload: &Workload,
        workspace: &Path,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Option<Self> {
        if reads_only(workload) {
            return None;
        }
        let server = lookup(SERVER).filter(|server| !server.trim().is_empty())?;
        Some(Self {
            server: server.trim_end_matches('/').to_owned(),
            name: database_name(workspace),
            dropped: std::cell::Cell::new(false),
        })
    }

    /// The name of the database.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Drop the database, with whoever is connected to it. A database that is not there is
    /// already what is wanted. Says why it could not, so a run can report it.
    ///
    /// # Errors
    ///
    /// Returns what `psql` said, or why it could not be started.
    pub fn drop_now(&self) -> Result<(), String> {
        if self.dropped.replace(true) {
            return Ok(());
        }
        let output = Command::new("psql")
            .args(["--no-psqlrc", "--quiet", "--set=ON_ERROR_STOP=1"])
            .arg(format!("{}/postgres", self.server))
            .arg("--command")
            .arg(format!(
                "drop database if exists \"{}\" with (force)",
                self.name
            ))
            .env("PGCONNECT_TIMEOUT", "10")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("psql could not be started: {e}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // A run that ended by an error still leaves nothing on the server; the explicit drop
        // is for the run that wants to say it could not.
        let _ = self.drop_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_names_each_kind_of_command_and_says_whether_there_is_a_database() {
        let with = prompt_section(true);
        let without = prompt_section(false);
        for text in [&with, &without] {
            assert!(text.contains("mix test"), "{text}");
            assert!(text.contains("npx vitest run"), "{text}");
        }
        assert!(with.contains("DATABASE_URL"));
        assert!(without.contains("No database was given"));
    }

    #[test]
    fn two_workspaces_are_given_two_databases() {
        let one = database_name(Path::new("/work/Abc-123"));
        let two = database_name(Path::new("/work/abc-124"));
        assert_eq!(one, "heph_abc_123");
        assert_ne!(one, two);
    }

    #[test]
    fn a_name_is_cut_to_what_postgres_allows() {
        let long = format!("/work/{}", "x".repeat(200));
        assert_eq!(database_name(Path::new(&long)).len(), NAME_LIMIT);
    }

    #[test]
    fn no_server_means_no_database() {
        assert_eq!(database_url(None, Path::new("/work/a")), None);
        assert_eq!(database_url(Some(""), Path::new("/work/a")), None);
        assert_eq!(
            database_url(Some("postgres://me@localhost/"), Path::new("/work/a")).as_deref(),
            Some("postgres://me@localhost/heph_a")
        );
    }
}
