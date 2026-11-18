//! Cancellation of a running command, process-wide.
//!
//! The flag is process-wide, because a signal handler can only set a flag, so
//! these tests are a binary of their own and run one at a time.

mod common;

use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use common::Scratch;
use heph::process::{Ended, Go, Streamed, cancel, execute, reset_cancellation, stream};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

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

fn gone_within_a_moment(pid: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    !alive(pid)
}

fn cancel_after(delay: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        cancel();
    });
}

const HANG: &str = "sleep 120 >/dev/null 2>&1 & echo $! > child.pid; wait";

#[test]
fn a_running_command_is_killed_with_what_it_started_when_cancellation_is_set() {
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    reset_cancellation();
    let scratch = Scratch::new("cancel-execute");
    cancel_after(Duration::from_millis(500));
    let started = Instant::now();
    let (ended, _) = execute(
        &words(&["sh", "-c", HANG]),
        &scratch.0,
        Duration::from_secs(120),
        false,
        |_| {},
    )
    .expect("to run");
    assert_eq!(ended, Ended::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "it was not stopped"
    );
    let pid = scratch.read("child.pid").trim().to_owned();
    assert!(
        gone_within_a_moment(&pid),
        "the grandchild {pid} outlived it"
    );

    reset_cancellation();
    let scratch = Scratch::new("cancel-stream");
    cancel_after(Duration::from_millis(500));
    let started = Instant::now();
    let ended = stream(
        &words(&["sh", "-c", HANG]),
        &scratch.0,
        &[],
        Duration::from_secs(120),
        Duration::from_secs(3600),
        |_| Go::On,
        || Go::On,
        |_| {},
    )
    .expect("to run");
    assert_eq!(ended, Streamed::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "it was not stopped"
    );
    let pid = scratch.read("child.pid").trim().to_owned();
    assert!(
        gone_within_a_moment(&pid),
        "the grandchild {pid} outlived it"
    );
    reset_cancellation();
}

#[test]
fn once_cancellation_is_set_a_command_is_not_started_at_all() {
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let scratch = Scratch::new("cancel-before");
    cancel();

    let (ended, _) = execute(
        &words(&["sh", "-c", "touch started"]),
        &scratch.0,
        Duration::from_secs(30),
        false,
        |_| {},
    )
    .expect("to run");
    let streamed = stream(
        &words(&["sh", "-c", "touch started-too"]),
        &scratch.0,
        &[],
        Duration::from_secs(30),
        Duration::from_secs(3600),
        |_| Go::On,
        || Go::On,
        |_| {},
    )
    .expect("to run");
    reset_cancellation();

    assert_eq!(ended, Ended::Cancelled);
    assert_eq!(streamed, Streamed::Cancelled);
    assert!(
        !scratch.exists("started"),
        "a command was started after cancellation"
    );
    assert!(
        !scratch.exists("started-too"),
        "a streamed command was started after cancellation"
    );
}

#[test]
fn a_command_that_would_not_have_started_is_cancelled_and_not_reported_as_not_found() {
    // A program that does not exist cannot be started whatever happens, so
    // whether it is reported as cancelled or as not found says whether anything
    // tried to start it.
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let scratch = Scratch::new("cancel-not-found");
    cancel();

    let (ended, _) = execute(
        &words(&["no-such-program-anywhere"]),
        &scratch.0,
        Duration::from_secs(30),
        false,
        |_| {},
    )
    .expect("to run");
    let streamed = stream(
        &words(&["no-such-program-anywhere"]),
        &scratch.0,
        &[],
        Duration::from_secs(30),
        Duration::from_secs(3600),
        |_| Go::On,
        || Go::On,
        |_| {},
    )
    .expect("to run");
    reset_cancellation();

    assert_eq!(ended, Ended::Cancelled);
    assert_eq!(streamed, Streamed::Cancelled);
}
