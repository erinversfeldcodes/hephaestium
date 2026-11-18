//! What the executor does when it is interrupted: it stops what it is running,
//! and everything that started, ends the workload as cancelled, and still says
//! so in its events and its record.

mod common;

use std::os::unix::fs::PermissionsExt as _;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::{
    IDENTITY, Ran, Scratch, agent_script, command_for, finished, has_branch, kinds, manifest,
    source, workload,
};
use contracts::v1alpha1::finished::Outcome;
use serde_json::json;

fn alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .expect("kill")
        .success()
}

fn wait_for(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A command that says it has started, starts a process of its own that would
/// outlive it, records that process, and waits.
fn hang(scratch: &Scratch) -> String {
    format!(
        "touch {ready}; sleep 120 >/dev/null 2>&1 & echo $! > {pid}; wait",
        ready = scratch.path("ready").display(),
        pid = scratch.path("child.pid").display()
    )
}

/// Start the executor, wait for the command that hangs to have started, send it a
/// signal and read how it ended.
fn interrupted(
    scratch: Scratch,
    workload: &serde_json::Value,
    extra: &[&str],
    signal: &str,
    environment: &[(&str, &str)],
) -> (Ran, String) {
    let mut command = command_for(&scratch, workload, None, extra, environment);
    // Into files and not pipes: a process that outlives the executor would hold a
    // pipe open, and reading it would wait for that process.
    command
        .stdout(std::fs::File::create(scratch.path("stdout.txt")).expect("a file"))
        .stderr(std::fs::File::create(scratch.path("stderr.txt")).expect("a file"));
    let mut child: Child = command.spawn().expect("the executor to start");
    let ready = scratch.path("ready");
    wait_for("the command to start", || ready.exists());
    // The child it started, which must not outlive the signal.
    wait_for("the child's pid", || {
        std::fs::read_to_string(scratch.path("child.pid")).is_ok_and(|p| !p.trim().is_empty())
    });
    let pid = std::fs::read_to_string(scratch.path("child.pid"))
        .expect("a pid")
        .trim()
        .to_owned();
    assert!(
        alive(&pid),
        "the process the command started is not running to begin with"
    );

    Command::new("kill")
        .args([&format!("-{signal}"), &child.id().to_string()])
        .status()
        .expect("kill");
    let signalled = Instant::now();
    let deadline = signalled + Duration::from_secs(30);
    while child.try_wait().expect("to wait").is_none() {
        assert!(
            Instant::now() < deadline,
            "the executor did not stop within 30 seconds of {signal}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let status = child.wait().expect("to wait");
    let output = std::process::Output {
        status,
        stdout: std::fs::read(scratch.path("stdout.txt")).expect("its output"),
        stderr: std::fs::read(scratch.path("stderr.txt")).expect("its errors"),
    };

    // Whatever it started is gone, soon after the executor is.
    let gone = Instant::now() + Duration::from_secs(10);
    while alive(&pid) && Instant::now() < gone {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!alive(&pid), "the process {pid} outlived the executor");
    (Ran { output, scratch }, pid)
}

fn assert_cancelled(ran: &Ran, phase: &str) {
    assert_eq!(
        ran.code(),
        0,
        "{phase}: a workload that ended is exit 0: {}",
        ran.stderr()
    );
    let events = ran.events();
    let (outcome, reason) = finished(&events);
    assert_eq!(outcome, Outcome::Cancelled, "{phase}: {reason}");
    assert!(
        reason.to_lowercase().contains("cancel"),
        "{phase}: {reason}"
    );
    assert_eq!(ran.record().outcome, Outcome::Cancelled as i32, "{phase}");
    assert_eq!(
        kinds(&events).iter().filter(|k| **k == "finished").count(),
        1,
        "{phase}: finished is written once and last"
    );
}

#[test]
fn the_agent_is_stopped_with_what_it_started_and_the_workload_ends_cancelled_for_each_signal() {
    for signal in ["INT", "TERM", "HUP"] {
        let scratch = Scratch::new(&format!("cancel-agent-{signal}"));
        let (_, commit) = source(
            &scratch,
            &format!(
                "{}\n[gates.marker]\nrun = [\"sh\", \"-c\", \"touch gate-ran\"]\ntimeout_seconds = 30\n",
                manifest(&[])
            ),
        );
        let script = hang(&scratch);
        let arguments = [
            "--agent-command".to_owned(),
            json!(["sh", "-c", script]).to_string(),
        ];
        let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();

        let (ran, _) = interrupted(scratch, &workload(&commit), &refs, signal, &[]);

        assert_cancelled(&ran, signal);
        assert!(
            !kinds(&ran.events()).contains(&"gate_started"),
            "{signal}: gates ran after the signal"
        );
    }
}

#[test]
fn a_bootstrap_step_is_stopped_and_the_agent_is_never_started() {
    let scratch = Scratch::new("cancel-bootstrap");
    let step = hang(&scratch);
    let steps: [(&[&str], Option<&str>, u64); 1] = [(&["sh", "-c", &step], None, 300)];
    let (_, commit) = source(&scratch, &manifest(&steps));
    let agent = format!(
        "touch {}; echo '{}'",
        scratch.path("agent-ran").display(),
        json!({"agentMessage": {"text": "hi"}})
    );
    let arguments = [
        "--agent-command".to_owned(),
        json!(["sh", "-c", agent]).to_string(),
    ];
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();

    let (ran, _) = interrupted(scratch, &workload(&commit), &refs, "INT", &[]);

    assert_cancelled(&ran, "bootstrap");
    assert!(
        !ran.scratch.path("agent-ran").exists(),
        "the agent was started after the signal"
    );
}

#[test]
fn a_gate_is_stopped_and_is_not_reported_as_failed() {
    let scratch = Scratch::new("cancel-gate");
    let gate = hang(&scratch);
    let text = manifest(&[])
        + &format!("\n[gates.slow]\nrun = [\"sh\", \"-c\", {gate:?}]\ntimeout_seconds = 300\n");
    let (_, commit) = source(&scratch, &text);
    let agent = format!("echo '{}'", json!({"agentMessage": {"text": "done"}}));
    let arguments = [
        "--agent-command".to_owned(),
        json!(["sh", "-c", agent]).to_string(),
    ];
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();

    let (ran, _) = interrupted(scratch, &workload(&commit), &refs, "TERM", &[]);

    assert_cancelled(&ran, "gate");
    assert!(
        !kinds(&ran.events()).contains(&"gate_finished"),
        "a gate that was cancelled has no result"
    );
}

#[test]
fn the_pull_request_step_is_stopped_and_what_was_already_pushed_stays() {
    let scratch = Scratch::new("cancel-deliver");
    let (source_dir, commit) = source(&scratch, &manifest(&[]));
    // A gh that starts opening the pull request and takes its time.
    let gh = scratch.path("gh");
    let slow = format!(
        "#!/bin/sh\ntouch {ready}\nsleep 120 >/dev/null 2>&1 &\necho $! > {pid}\nwait\n",
        ready = scratch.path("ready").display(),
        pid = scratch.path("child.pid").display()
    );
    std::fs::write(&gh, slow).expect("a script");
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).expect("to chmod");
    let script = agent_script("");
    let arguments = [
        "--agent-command".to_owned(),
        json!(["sh", "-c", script]).to_string(),
        "--gh".to_owned(),
        gh.display().to_string(),
        "--deliver".to_owned(),
    ];
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();

    let (ran, _) = interrupted(scratch, &workload(&commit), &refs, "INT", &IDENTITY);

    assert_cancelled(&ran, "delivery");
    assert!(
        !kinds(&ran.events()).contains(&"pull_request_opened"),
        "a pull request was reported opened"
    );
    assert!(
        has_branch(&source_dir),
        "the push finished before the signal, so the branch stays"
    );
}
