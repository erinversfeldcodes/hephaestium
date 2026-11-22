//! What running a command for at most a given time promises, tested directly
//! and not only through the gate runner that uses it.

mod common;

use std::process::Command;
use std::time::{Duration, Instant};

use common::Scratch;
use heph::process::{Ended, Tail, execute, execute_with_tail};

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

/// Run a script with a tail. Each caller names its own scratch directory, because tests run in
/// parallel and one that shared a name would delete another's directory under it.
fn tailed(
    name: &str,
    script: &str,
    capture_stdout: bool,
    most: usize,
) -> (Ended, Option<Vec<u8>>, Tail) {
    let scratch = Scratch::new(name);
    let tail = Tail::new(most);
    let (ended, captured) = execute_with_tail(
        &words(&["sh", "-c", script]),
        &scratch.0,
        Duration::from_secs(30),
        capture_stdout,
        &[],
        |_| {},
        Some(&tail),
    )
    .expect("to run");
    (ended, captured, tail)
}

#[test]
fn a_tail_keeps_the_last_lines_a_command_printed_on_either_stream() {
    let (ended, _, tail) = tailed(
        "process-tail-lines",
        "for i in 1 2 3 4 5; do echo out$i; echo err$i >&2; done; exit 1",
        false,
        4,
    );

    assert_eq!(ended, Ended::Exited(1), "how it ended is as it always was");
    let text = tail.text();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "only the last four are kept: {text}");
    assert!(
        text.contains('5'),
        "the last of what it printed is there: {text}"
    );
    assert!(
        !text.contains("out1") && !text.contains("err1"),
        "the first is not: {text}"
    );
}

#[test]
fn a_tail_leaves_the_captured_standard_output_whole_and_keeps_only_what_went_to_standard_error() {
    let (ended, captured, tail) = tailed(
        "process-tail-captured",
        "echo '{\"a\": 1}'; echo warning >&2",
        true,
        10,
    );

    assert_eq!(ended, Ended::Exited(0));
    assert_eq!(
        String::from_utf8(captured.expect("captured"))
            .expect("UTF-8")
            .trim(),
        "{\"a\": 1}"
    );
    assert_eq!(tail.text(), "warning");
}

#[test]
fn a_tail_of_a_command_that_prints_nothing_is_empty_and_text_that_is_not_utf8_does_not_end_it() {
    let (_, _, quiet) = tailed("process-tail-quiet", "exit 0", false, 5);
    assert_eq!(quiet.text(), "");

    let (ended, _, odd) = tailed("process-tail-odd", "printf 'caf\\351\\n' >&2", false, 5);
    assert_eq!(ended, Ended::Exited(0));
    assert!(odd.text().starts_with("caf"), "{}", odd.text());
}

#[test]
fn a_command_that_runs_past_its_deadline_is_stopped_and_its_tail_is_what_it_printed_so_far() {
    let scratch = Scratch::new("process-tail-timeout");
    let tail = Tail::new(5);
    let (ended, _) = execute_with_tail(
        &words(&["sh", "-c", "echo started >&2; sleep 30"]),
        &scratch.0,
        Duration::from_millis(500),
        false,
        &[],
        |_| {},
        Some(&tail),
    )
    .expect("to run");

    assert_eq!(ended, Ended::TimedOut);
    assert_eq!(tail.text(), "started");
}
