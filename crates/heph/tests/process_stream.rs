//! What running a command and reading its output as it runs promises.

mod common;

use std::cell::RefCell;
use std::process::Command;
use std::time::{Duration, Instant};

use common::Scratch;
use heph::process::{Go, Streamed, stream};

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

const LONG: Duration = Duration::from_secs(30);
const NEVER: Duration = Duration::from_secs(3600);

#[test]
fn lines_arrive_in_order_as_they_are_written_before_the_command_ends() {
    let scratch = Scratch::new("stream-lines");
    let seen: RefCell<Vec<(String, Instant)>> = RefCell::new(Vec::new());

    let ended = stream(
        &words(&["sh", "-c", "echo first; sleep 1; echo second"]),
        &scratch.0,
        &[],
        LONG,
        NEVER,
        |line| {
            seen.borrow_mut().push((line.to_owned(), Instant::now()));
            Go::On
        },
        || Go::On,
        |_| {},
    )
    .expect("to run");

    assert_eq!(ended, Streamed::Exited(0));
    let seen = seen.into_inner();
    assert_eq!(
        seen.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert!(
        seen[1].1 - seen[0].1 >= Duration::from_millis(700),
        "the first line was held back until the command ended"
    );
}

#[test]
fn the_environment_is_exactly_what_was_given() {
    let scratch = Scratch::new("stream-env");
    let seen = RefCell::new(Vec::new());

    // HOME is in this test's environment and is not given, so it must not be
    // in the command's.
    let ended = stream(
        &words(&["sh", "-c", "echo \"${HOME:-none} ${GIVEN:-none}\""]),
        &scratch.0,
        &[("GIVEN".to_owned(), "yes".to_owned())],
        LONG,
        NEVER,
        |line| {
            seen.borrow_mut().push(line.to_owned());
            Go::On
        },
        || Go::On,
        |_| {},
    )
    .expect("to run");

    assert_eq!(ended, Streamed::Exited(0));
    assert_eq!(seen.into_inner(), ["none yes"]);
}

#[test]
fn a_caller_that_says_stop_stops_the_command_and_everything_it_started() {
    let scratch = Scratch::new("stream-stop");

    let started = Instant::now();
    let ended = stream(
        &words(&[
            "sh",
            "-c",
            "sleep 60 >/dev/null 2>&1 & echo $! > child.pid; echo go; wait",
        ]),
        &scratch.0,
        &[],
        LONG,
        NEVER,
        |_| Go::Stop,
        || Go::On,
        |_| {},
    )
    .expect("to run");

    assert_eq!(ended, Streamed::Stopped);
    assert!(started.elapsed() < Duration::from_secs(20));
    let pid = scratch.read("child.pid").trim().to_owned();
    assert!(
        gone_within_a_moment(&pid),
        "the grandchild {pid} outlived it"
    );
}

#[test]
fn a_command_past_its_deadline_is_stopped_with_everything_it_started() {
    let scratch = Scratch::new("stream-deadline");

    let started = Instant::now();
    let ended = stream(
        &words(&[
            "sh",
            "-c",
            "sleep 60 >/dev/null 2>&1 & echo $! > child.pid; wait",
        ]),
        &scratch.0,
        &[],
        Duration::from_secs(1),
        NEVER,
        |_| Go::On,
        || Go::On,
        |_| {},
    )
    .expect("to run");

    assert_eq!(ended, Streamed::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(20));
    let pid = scratch.read("child.pid").trim().to_owned();
    assert!(
        gone_within_a_moment(&pid),
        "the grandchild {pid} outlived it"
    );
}

#[test]
fn ticks_arrive_while_the_command_is_quiet() {
    let scratch = Scratch::new("stream-ticks");
    let ticks = RefCell::new(0_u32);

    let ended = stream(
        &words(&["sleep", "2"]),
        &scratch.0,
        &[("PATH".to_owned(), std::env::var("PATH").unwrap_or_default())],
        LONG,
        Duration::from_millis(250),
        |_| Go::On,
        || {
            *ticks.borrow_mut() += 1;
            Go::On
        },
        |_| {},
    )
    .expect("to run");

    assert_eq!(ended, Streamed::Exited(0));
    assert!(*ticks.borrow() >= 4, "ticks: {}", ticks.borrow());
}

#[test]
fn an_exit_code_is_reported_after_the_lines() {
    let scratch = Scratch::new("stream-exit");
    let seen = RefCell::new(Vec::new());

    let ended = stream(
        &words(&["sh", "-c", "echo a; echo b; exit 3"]),
        &scratch.0,
        &[],
        LONG,
        NEVER,
        |line| {
            seen.borrow_mut().push(line.to_owned());
            Go::On
        },
        || Go::On,
        |_| {},
    )
    .expect("to run");

    assert_eq!(ended, Streamed::Exited(3));
    assert_eq!(seen.into_inner(), ["a", "b"]);
}

#[test]
fn a_command_that_exits_and_leaves_a_child_holding_its_output_is_not_waited_for() {
    let scratch = Scratch::new("stream-straggler");
    let seen = RefCell::new(Vec::new());

    // The background child inherits standard output, so the pipe stays open
    // after the command itself has gone.
    let started = Instant::now();
    let ended = stream(
        &words(&["sh", "-c", "sleep 60 & echo $! > child.pid; echo done"]),
        &scratch.0,
        &[("PATH".to_owned(), std::env::var("PATH").unwrap_or_default())],
        LONG,
        NEVER,
        |line| {
            seen.borrow_mut().push(line.to_owned());
            Go::On
        },
        || Go::On,
        |_| {},
    )
    .expect("to run");

    assert_eq!(ended, Streamed::Exited(0));
    assert_eq!(seen.into_inner(), ["done"]);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "it waited for the child"
    );
    let pid = scratch.read("child.pid").trim().to_owned();
    assert!(gone_within_a_moment(&pid), "the child {pid} outlived it");
}

#[test]
fn a_program_that_does_not_exist_is_not_run_and_gets_the_code_a_shell_would() {
    let scratch = Scratch::new("stream-missing");

    let ended = stream(
        &words(&["no-such-program-anywhere"]),
        &scratch.0,
        &[],
        LONG,
        NEVER,
        |_| Go::On,
        || Go::On,
        |_| {},
    )
    .expect("to run");

    assert_eq!(ended, Streamed::NotRun(127));
}

#[test]
fn a_command_that_exits_and_leaves_a_child_that_escaped_its_group_is_not_waited_for_beyond_the_deadline()
 {
    let scratch = Scratch::new("stream-escaped");

    // The child starts a session of its own, so it is outside the command's
    // process group and holds standard output open after the command is gone.
    let started = Instant::now();
    let ended = stream(
        &words(&[
            "perl",
            "-MPOSIX",
            "-e",
            "my $p = fork; if ($p == 0) { POSIX::setsid(); exec 'sleep', '60' } \
             open F, '>child.pid'; print F $p; close F; print \"done\\n\";",
        ]),
        &scratch.0,
        &[("PATH".to_owned(), std::env::var("PATH").unwrap_or_default())],
        Duration::from_secs(2),
        NEVER,
        |_| Go::On,
        || Go::On,
        |_| {},
    )
    .expect("to run");

    let pid = scratch.read("child.pid").trim().to_owned();
    let _ = Command::new("kill").arg(&pid).status();
    assert_eq!(ended, Streamed::Exited(0));
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "it waited for a process it could not reach"
    );
}
