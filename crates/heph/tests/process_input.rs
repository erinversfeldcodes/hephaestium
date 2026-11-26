//! What giving a command its input on standard input promises, and why the agent is given its
//! prompt that way: a command line is matched by `pkill -f` in every other run on the machine.

mod common;

use std::cell::RefCell;
use std::process::{Command, Stdio};
use std::time::Duration;

use common::Scratch;
use heph::process::{Go, Streamed, stream_with_input};

const LONG: Duration = Duration::from_secs(30);
const NEVER: Duration = Duration::from_secs(3600);

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// A pattern no other process on the machine has, made by the test and never taken from a
/// prompt, a ticket or a tool name.
fn a_marker(name: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock")
        .as_nanos();
    format!("marker-{name}-{}-{nanos}", std::process::id())
}

/// Run `argv` with `input`, and say what it printed and how it ended. `after` is called with
/// each line so a test can act while the command runs.
fn run(
    scratch: &Scratch,
    argv: &[&str],
    input: &str,
    mut after: impl FnMut(&str),
) -> (Vec<String>, Streamed) {
    let seen: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let ended = stream_with_input(
        &words(argv),
        &scratch.0,
        &[("PATH".to_owned(), std::env::var("PATH").unwrap_or_default())],
        input,
        LONG,
        NEVER,
        |line| {
            after(line);
            seen.borrow_mut().push(line.to_owned());
            Go::On
        },
        || Go::On,
        |_| {},
    )
    .expect("to run");
    (seen.into_inner(), ended)
}

#[test]
fn the_input_reaches_the_command_and_none_of_it_is_on_its_command_line() {
    let scratch = Scratch::new("input-argv");
    let marker = a_marker("argv");

    let (lines, ended) = run(
        &scratch,
        &["sh", "-c", "ps -o args= -p $$; cat"],
        &format!("{marker}\nsecond\n"),
        |_| {},
    );

    assert_eq!(ended, Streamed::Exited(0));
    assert_eq!(&lines[lines.len() - 2..], [marker.as_str(), "second"]);
    assert!(
        !lines[0].contains(&marker),
        "the started command line held the input: {}",
        lines[0]
    );
}

#[test]
fn input_larger_than_a_pipe_holds_is_delivered_whole() {
    let scratch = Scratch::new("input-large");
    let input = "x".repeat(1_000_000);

    let (lines, ended) = run(&scratch, &["wc", "-c"], &input, |_| {});

    assert_eq!(ended, Streamed::Exited(0));
    assert_eq!(lines[0].trim(), "1000000");
}

#[test]
fn input_that_could_not_be_written_fails_the_run_with_the_reason() {
    let scratch = Scratch::new("input-failed");

    // The command closes its standard input and stays, so the write meets a closed pipe.
    let (_, ended) = run(
        &scratch,
        &["sh", "-c", "exec 0<&-; sleep 5"],
        &"x".repeat(1_000_000),
        |_| {},
    );

    assert!(
        matches!(&ended, Streamed::InputFailed(reason) if !reason.is_empty()),
        "{ended:?}"
    );
}

/// Whether `pkill -f marker` leaves the started command alive. The command prints `ready` once
/// it has started, and `survived` if nothing stopped it.
fn survives_a_kill_by_pattern(scratch: &Scratch, argv: &[&str], input: &str, marker: &str) -> bool {
    let (lines, _) = run(scratch, argv, input, |line| {
        if line == "ready" {
            Command::new("pkill")
                .args(["-f", marker])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("pkill to run");
        }
    });
    lines.iter().any(|line| line == "survived")
}

#[test]
fn a_kill_by_pattern_does_not_reach_an_agent_that_was_given_the_pattern_as_input() {
    let marker = a_marker("kill-input");
    let script = "read first; echo ready; sleep 2; echo survived";

    // The control: the same pattern on the command line is found, so the check can fail.
    let on_command_line = survives_a_kill_by_pattern(
        &Scratch::new("input-kill-control"),
        &["sh", "-c", script, &marker],
        "go\n",
        &marker,
    );
    assert!(
        !on_command_line,
        "the pattern on the command line did not match, so this test cannot show anything"
    );

    let on_input = survives_a_kill_by_pattern(
        &Scratch::new("input-kill"),
        &["sh", "-c", script],
        &format!("{marker}\n"),
        &marker,
    );
    assert!(on_input, "a kill by pattern reached an agent run on input");
}
