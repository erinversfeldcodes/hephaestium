//! What running a command for at most a given time promises, tested directly
//! and not only through the gate runner that uses it.

mod common;

use std::process::Command;
use std::time::{Duration, Instant};

use common::Scratch;
use heph::process::{Ended, execute};

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

fn alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .expect("kill to run")
        .success()
}

#[test]
fn an_exit_code_is_reported() {
    let scratch = Scratch::new("process-exit");
    let (ended, captured) = execute(
        &words(&["sh", "-c", "exit 3"]),
        &scratch.0,
        Duration::from_secs(30),
        false,
        |_| {},
    )
    .expect("to run");
    assert_eq!(ended, Ended::Exited(3));
    assert_eq!(captured, None);
}

#[test]
fn a_program_that_does_not_exist_is_not_run_and_gets_the_code_a_shell_would() {
    let scratch = Scratch::new("process-missing");
    let (ended, _) = execute(
        &words(&["no-such-program-anywhere"]),
        &scratch.0,
        Duration::from_secs(30),
        false,
        |_| {},
    )
    .expect("to run");
    assert_eq!(ended, Ended::NotRun(127));
}

#[test]
fn standard_output_is_captured_when_asked() {
    let scratch = Scratch::new("process-capture");
    let (ended, captured) = execute(
        &words(&["echo", "hello"]),
        &scratch.0,
        Duration::from_secs(30),
        true,
        |_| {},
    )
    .expect("to run");
    assert_eq!(ended, Ended::Exited(0));
    assert_eq!(captured.as_deref(), Some(&b"hello\n"[..]));
}

#[test]
fn a_command_past_its_deadline_is_stopped_with_everything_it_started() {
    let scratch = Scratch::new("process-deadline");
    let started = Instant::now();
    let (ended, _) = execute(
        &words(&[
            "sh",
            "-c",
            "sleep 60 >/dev/null 2>&1 & echo $! > child.pid; wait",
        ]),
        &scratch.0,
        Duration::from_secs(1),
        false,
        |_| {},
    )
    .expect("to run");

    assert_eq!(ended, Ended::TimedOut);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "it was not stopped at its deadline"
    );
    let pid = scratch.read("child.pid").trim().to_owned();
    // The kill is delivered asynchronously, so allow it a moment to land.
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(&pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(&pid), "the grandchild {pid} outlived its parent");
}
