//! What a workspace's tools are given: a home to keep packages in, and a test database of the
//! workspace's own that lives as long as the run and no longer.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use common::{Ran, Scratch, manifest, run_with_env, source, workload};
use serde_json::{Value, json};

/// A `psql` that records what it was asked to do, and when, in `calls`, and succeeds.
fn stand_in_psql(scratch: &Scratch) -> PathBuf {
    let directory = scratch.path("bin");
    let program = scratch.write(
        "bin/psql",
        &format!(
            "#!/bin/sh\necho \"$@\" >> {calls}\n",
            calls = scratch.path("calls").display()
        ),
    );
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    directory
}

/// Run a stub agent that writes the variables it was given, and then a mark, to `seen`.
fn run_agent(
    name: &str,
    id: &str,
    with_server: bool,
    steps: &[(&[&str], Option<&str>, u64)],
) -> Ran {
    let scratch = Scratch::new(name);
    let bin = stand_in_psql(&scratch);
    let (_, commit) = source(&scratch, &manifest(steps));
    let mut value: Value = workload(&commit);
    value["id"] = json!(id);
    let script = format!(
        "{{ echo HOME=$HOME; echo DATABASE_URL=$DATABASE_URL; echo GH_TOKEN=$GH_TOKEN; \
         echo psql_calls_so_far=$(cat {calls} 2>/dev/null | wc -l | tr -d ' '); }} > ../seen-{id}",
        calls = scratch.path("calls").display()
    );
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut environment = vec![("PATH", path.as_str())];
    if with_server {
        environment.push(("HEPH_TEST_DATABASE_SERVER", "postgres://me@db.invalid:5432"));
    }
    run_with_env(
        scratch,
        &value,
        None,
        &["--agent-command", &json!(["sh", "-c", script]).to_string()],
        &environment,
    )
}

fn seen(ran: &Ran, id: &str) -> String {
    std::fs::read_to_string(ran.scratch.path(&format!("work/seen-{id}")))
        .expect("what the agent saw")
}

fn calls(ran: &Ran) -> String {
    std::fs::read_to_string(ran.scratch.path("calls")).unwrap_or_default()
}

#[test]
fn the_agent_is_given_a_home_and_a_database_of_its_own() {
    let ran = run_agent("toolenv-agent", "wl-aaa", true, &[]);
    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let seen = seen(&ran, "wl-aaa");

    let home = ran.scratch.path("work/.tool-home");
    assert!(seen.contains(&format!("HOME={}", home.display())), "{seen}");
    assert!(
        seen.contains("DATABASE_URL=postgres://me@db.invalid:5432/heph_wl_aaa"),
        "{seen}"
    );
    assert!(
        !seen.contains("GH_TOKEN=ghp"),
        "no credential reaches the agent: {seen}"
    );
}

#[test]
fn two_workspaces_use_different_databases() {
    let one = run_agent("toolenv-one", "wl-one", true, &[]);
    let two = run_agent("toolenv-two", "wl-two", true, &[]);
    let url = |text: String| {
        text.lines()
            .find_map(|l| l.strip_prefix("DATABASE_URL=").map(str::to_owned))
            .expect("a database")
    };
    assert_ne!(url(seen(&one, "wl-one")), url(seen(&two, "wl-two")));
}

#[test]
fn a_workspace_with_no_server_is_given_no_database() {
    let ran = run_agent("toolenv-none", "wl-none", false, &[]);
    assert!(seen(&ran, "wl-none").contains("DATABASE_URL=\n"));
    assert_eq!(calls(&ran), "", "nothing was dropped that was not made");
}

#[test]
fn the_database_is_dropped_once_when_the_run_ends_and_not_while_the_agent_works() {
    let ran = run_agent("toolenv-drop", "wl-drop", true, &[]);
    assert_eq!(ran.code(), 0, "{}", ran.stderr());

    assert!(
        seen(&ran, "wl-drop").contains("psql_calls_so_far=0"),
        "the database is still there while the agent works"
    );
    let calls = calls(&ran);
    assert_eq!(calls.lines().count(), 1, "{calls}");
    assert!(
        calls.contains("drop database if exists \"heph_wl_drop\" with (force)"),
        "{calls}"
    );
    assert!(
        calls.contains("postgres://me@db.invalid:5432/postgres"),
        "{calls}"
    );
}

#[test]
fn the_database_is_dropped_when_a_bootstrap_step_ends_the_run() {
    let steps: [(&[&str], Option<&str>, u64); 1] = [(&["sh", "-c", "exit 1"], None, 30)];
    let ran = run_agent("toolenv-failed", "wl-fail", true, &steps);
    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert!(calls(&ran).contains("heph_wl_fail"), "{}", calls(&ran));
}

#[test]
fn bootstrap_steps_run_in_the_tools_home() {
    let steps: [(&[&str], Option<&str>, u64); 1] =
        [(&["sh", "-c", "echo $HOME > ../home-seen"], None, 30)];
    let ran = run_agent("toolenv-bootstrap", "wl-boot", false, &steps);
    let at = ran.scratch.path("work/home-seen");
    let home = std::fs::read_to_string(at).expect("a home");
    assert_eq!(
        PathBuf::from(home.trim()),
        ran.scratch.path("work/.tool-home")
    );
}

/// `mix`, `npm` and `npx` are found and run when the environment holds only what the agent is
/// given. A machine that has none of them proves nothing here, and says so.
#[test]
fn mix_npm_and_npx_run_inside_the_agents_environment() {
    let path = std::env::var("PATH").unwrap_or_default();
    let found = |program: &str| {
        std::env::split_paths(&path).any(|directory| directory.join(program).is_file())
    };
    if !["mix", "npm", "npx"].iter().all(|program| found(program)) {
        eprintln!("skipped: mix, npm and npx are not all installed on this machine");
        return;
    }
    let scratch = Scratch::new("toolenv-tools");
    let (_, commit) = source(&scratch, &manifest(&[]));
    let script = "mix --version > ../mix.out 2>&1; echo $? > ../mix.code; \
                  npm --version > ../npm.out 2>&1; echo $? > ../npm.code; \
                  npx --version > ../npx.out 2>&1; echo $? > ../npx.code";
    let ran = run_with_env(
        scratch,
        &workload(&commit),
        None,
        &[
            "--agent-command",
            &json!(["sh", "-c", script]).to_string(),
            "--agent-env",
            "PATH",
        ],
        &[],
    );
    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    for tool in ["mix", "npm", "npx"] {
        let code = std::fs::read_to_string(ran.scratch.path(&format!("work/{tool}.code")))
            .expect("an exit code");
        let said = std::fs::read_to_string(ran.scratch.path(&format!("work/{tool}.out")))
            .unwrap_or_default();
        assert_eq!(code.trim(), "0", "{tool} said: {said}");
    }
}
