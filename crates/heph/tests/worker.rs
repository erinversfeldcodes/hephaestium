//! `heph worker`: claiming workloads from a control plane, running the executor on them,
//! sending what it says, and stopping when told to.
//!
//! The control plane is stood in for by a small server that keeps every byte it is sent, and
//! the executor by a script that prints a recorded run, so this proves what the worker does and
//! not what either of them does. The worker was run against the real control plane by hand.

mod common;

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::Scratch;
use common::link::{Answer, Request, Server};
use rustix::process::{Pid, Signal, kill_process};
use serde_json::{Value, json};

const TOKEN: &str = "hdt_the-device-token-0123456789";
const GITHUB_SECRET: &str = "ghs_a-github-secret-aaaaaaaa";
const CLAUDE_SECRET: &str = "sk-ant-a-model-key-bbbbbbbb";
const FIRST: &str = "0d7a1c3e-6f2b-4b8a-9a31-2f6c9e1d4b55";
const SECOND: &str = "7e1b2c3d-1111-4222-8333-444455556666";

fn workload(id: &str) -> Value {
    json!({
        "id": id,
        "ticket": {"number": "12", "title": "t", "body": "b", "url": "https://github.com/acme/widgets/issues/12"},
        "repository": {"owner": "acme", "name": "widgets", "baseBranch": "main", "baseCommit": "0123456789abcdef0123456789abcdef01234567"},
    })
}

/// The answer to a claim that gives a workload.
fn given(id: &str, next_sequence: u64, lease_seconds: u64) -> Answer {
    Answer::Json(
        200,
        json!({"workload": workload(id), "lease": {"seconds": lease_seconds}, "nextSequence": next_sequence})
            .to_string(),
    )
}

fn ok() -> Answer {
    Answer::Json(200, "{}".to_owned())
}

fn is_claim(request: &Request) -> bool {
    request.path == "/worker/claim"
}

fn events_of(request: &Request) -> Vec<Value> {
    let body: Value = serde_json::from_str(&request.body).expect("a JSON body");
    body["events"].as_array().expect("events").clone()
}

/// What the executor stand-in does.
enum Executor {
    /// Three events and a record that says it completed.
    Completes,
    /// One event, then nothing until it is stopped.
    Hangs,
    /// As `Hangs`, with the process that waits working in its run's directory, where a sweep
    /// for leftovers looks.
    HangsInItsRunDirectory,
    /// The first workload hangs as `Hangs` does; any other completes as `Completes` does.
    HangsFirstCompletesOthers,
    /// One event, then nothing until it is told to stop, and then it says it was cancelled.
    SaysItWasCancelled,
}

/// One test's own home, where the worker is run from and where its run directories are made.
struct Rig {
    home: Scratch,
    /// Where run directories are made: the home's own, unless another worker's is shared.
    runs: PathBuf,
}

impl Rig {
    fn new(name: &str) -> Self {
        let home = Scratch::new(&format!("worker-{name}"));
        let runs = home.0.join("runs");
        let rig = Self { home, runs };
        std::fs::create_dir_all(rig.seen()).expect("a directory");
        rig
    }

    /// A rig with a home of its own that makes its run directories where `other` does, as a
    /// second worker on the same machine does.
    fn sharing_runs_with(name: &str, other: &Self) -> Self {
        let mut rig = Self::new(name);
        rig.runs.clone_from(&other.runs);
        rig
    }

    fn seen(&self) -> PathBuf {
        self.home.0.join("executor/seen")
    }

    fn pid(&self, id: &str) -> Option<String> {
        std::fs::read_to_string(self.seen().join(format!("pid-{id}")))
            .ok()
            .map(|text| text.trim().to_owned())
    }

    fn ran(&self) -> Vec<String> {
        let mut ids: Vec<String> = std::fs::read_dir(self.seen())
            .expect("a directory")
            .flatten()
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .strip_prefix("pid-")
                    .map(str::to_owned)
            })
            .collect();
        ids.sort();
        ids
    }

    fn executor(&self, mode: &Executor) -> PathBuf {
        let events = r#"
ev() { printf '{"workloadId":"%s","sequence":"%s","time":"2026-11-22T10:00:0%sZ",%s}\n' "$id" "$1" "$1" "$2"; }
record() { printf '{"workloadId":"%s","outcome":"%s","eventCount":"%s"}' "$id" "$1" "$2" > "$rec"; }
"#;
        let body = match mode {
            Executor::Completes => {
                r#"ev 1 '"started":{"executorVersion":"0.0.0"}'
ev 2 '"agentMessage":{"text":"working"}'
ev 3 '"finished":{"outcome":"OUTCOME_COMPLETED"}'
record OUTCOME_COMPLETED 3"#
            }
            Executor::Hangs => {
                r#"ev 1 '"started":{"executorVersion":"0.0.0"}'
exec sleep 300"#
            }
            Executor::HangsInItsRunDirectory => {
                r#"ev 1 '"started":{"executorVersion":"0.0.0"}'
cd "$(dirname "$wl")"
exec sleep 300"#
            }
            Executor::HangsFirstCompletesOthers => {
                r#"if [ "$id" = 0d7a1c3e-6f2b-4b8a-9a31-2f6c9e1d4b55 ]; then
  ev 1 '"started":{"executorVersion":"0.0.0"}'
  exec sleep 300
fi
ev 1 '"started":{"executorVersion":"0.0.0"}'
ev 2 '"agentMessage":{"text":"working"}'
ev 3 '"finished":{"outcome":"OUTCOME_COMPLETED"}'
record OUTCOME_COMPLETED 3"#
            }
            Executor::SaysItWasCancelled => {
                r#"ev 1 '"started":{"executorVersion":"0.0.0"}'
trap 'ev 2 "\"finished\":{\"outcome\":\"OUTCOME_CANCELLED\"}"; record OUTCOME_CANCELLED 2; kill $sleeper; exit 0' TERM
sleep 300 &
sleeper=$!
wait"#
            }
        };
        let script = format!(
            r#"#!/bin/sh
d={seen}
wl=''; rec=''; prev=''
for a in "$@"; do
  [ "$prev" = --workload ] && wl="$a"
  [ "$prev" = --record ] && rec="$a"
  prev="$a"
done
id=$(sed -n 's/^  "id": "\([^"]*\)".*/\1/p' "$wl" | head -1)
printf '%s\n' "$@" > "$d/args-$id"
env > "$d/env-$id"
cp "$wl" "$d/workload-$id"
{events}
echo "$$" > "$d/pid-$id"
{body}
"#,
            seen = self.seen().display()
        );
        let path = self.home.0.join("executor/executor");
        std::fs::write(&path, script).expect("a script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    fn secret_file(&self) -> PathBuf {
        let path = self.home.write("config/claude-secret", CLAUDE_SECRET);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        path
    }

    /// The worker, with a token and the executor's secrets in the places it looks, and its
    /// output kept in files.
    fn start(&self, address: &str, mode: &Executor) -> Worker {
        self.start_with(address, mode, &[])
    }

    /// As `start`, with more arguments to `heph worker`.
    fn start_with(&self, address: &str, mode: &Executor, extra_args: &[&str]) -> Worker {
        let executor = self.executor(mode);
        let claude = self.secret_file();
        let out = self.home.0.join("worker.out");
        let err = self.home.0.join("worker.err");
        let child = Command::new(env!("CARGO_BIN_EXE_heph"))
            .current_dir(&self.home.0)
            .args(["worker", "--control-plane", address, "--executor"])
            .arg(executor)
            .arg("--runs-dir")
            .arg(&self.runs)
            .args(extra_args)
            .env("HOME", &self.home.0)
            .env("XDG_CONFIG_HOME", self.home.0.join("config"))
            .env("HEPH_WORKER_TOKEN", TOKEN)
            .env_remove("HEPH_WORKER_TOKEN_FILE")
            .env("HEPH_GITHUB_TOKEN", GITHUB_SECRET)
            .env_remove("HEPH_GITHUB_TOKEN_FILE")
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
            .env("CLAUDE_CODE_OAUTH_TOKEN_FILE", claude)
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&out).expect("a file"))
            .stderr(std::fs::File::create(&err).expect("a file"))
            .spawn()
            .expect("the heph binary to run");
        Worker { child, out, err }
    }
}

struct Worker {
    child: Child,
    out: PathBuf,
    err: PathBuf,
}

impl Worker {
    fn signal(&self, signal: Signal) {
        let pid = Pid::from_raw(i32::try_from(self.child.id()).expect("a pid")).expect("a pid");
        kill_process(pid, signal).expect("the signal to be sent");
    }

    /// Wait for the worker to end, for a while.
    fn ends(&mut self) -> std::process::ExitStatus {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().expect("to wait") {
                return status;
            }
            assert!(Instant::now() < until, "the worker did not end");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Stop it as a person does, and say how it ended.
    fn stops(&mut self) -> std::process::ExitStatus {
        self.signal(Signal::TERM);
        self.ends()
    }

    fn output(&self) -> String {
        format!(
            "{}{}",
            std::fs::read_to_string(&self.out).unwrap_or_default(),
            std::fs::read_to_string(&self.err).unwrap_or_default()
        )
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .expect("kill to run")
        .success()
}

/// Gone sooner than the worker's grace before it kills what does not stop when told to.
fn gone_within_a_moment(pid: &str) -> bool {
    let until = Instant::now() + Duration::from_secs(5);
    while alive(pid) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(25));
    }
    !alive(pid)
}

/// A control plane that gives `id` once, to the first claim, and has none after.
fn gives_once(id: &'static str, next_sequence: u64) -> Server {
    let given_already = Arc::new(Mutex::new(false));
    Server::start(move |request| {
        if is_claim(request) {
            let mut done = given_already.lock().expect("a lock");
            if *done {
                return Answer::NoContent;
            }
            *done = true;
            return given(id, next_sequence, 120);
        }
        ok()
    })
}

fn sequences(server: &Server) -> Vec<String> {
    server
        .to(&format!("/worker/workloads/{FIRST}/events"))
        .iter()
        .flat_map(events_of)
        .map(|event| event["sequence"].as_str().expect("a sequence").to_owned())
        .collect()
}

#[test]
fn a_workload_is_claimed_run_and_its_events_and_record_sent_in_order() {
    let rig = Rig::new("flow");
    let server = gives_once(FIRST, 1);
    let mut worker = rig.start(&server.address, &Executor::Completes);

    server.wait_for("the record", |s| {
        !s.to(&format!("/worker/workloads/{FIRST}/record"))
            .is_empty()
    });
    assert_eq!(sequences(&server), ["1", "2", "3"], "gapless and in order");
    let record = &server.to(&format!("/worker/workloads/{FIRST}/record"))[0];
    let record: Value = serde_json::from_str(&record.body).expect("a record");
    assert_eq!(record["outcome"], "OUTCOME_COMPLETED");

    // The executor was given the workload the control plane gave, and asked to deliver.
    let given: Value = serde_json::from_str(
        &std::fs::read_to_string(rig.seen().join(format!("workload-{FIRST}"))).expect("a file"),
    )
    .expect("JSON");
    assert_eq!(given, workload(FIRST));
    let args = std::fs::read_to_string(rig.seen().join(format!("args-{FIRST}"))).expect("a file");
    assert!(args.lines().any(|a| a == "--deliver"), "{args}");

    // Every request is the worker's, with the token, and it goes on asking afterwards.
    for request in server.requests() {
        assert_eq!(
            request.authorization.as_deref(),
            Some(format!("Bearer {TOKEN}").as_str())
        );
    }
    server.wait_for("a claim after the run", |s| {
        s.to("/worker/claim").len() >= 2
    });
    assert!(worker.stops().success(), "{}", worker.output());
    assert_eq!(rig.ran(), [FIRST], "it ran once");
}

#[test]
fn no_secret_of_this_machine_is_on_the_wire_and_the_token_is_in_nothing_but_the_header() {
    let rig = Rig::new("secrets");
    let server = gives_once(FIRST, 1);
    let mut worker = rig.start(&server.address, &Executor::Completes);
    server.wait_for("the record", |s| {
        !s.to(&format!("/worker/workloads/{FIRST}/record"))
            .is_empty()
    });
    assert!(worker.stops().success(), "{}", worker.output());

    // The secrets were on this machine, and the executor had them.
    let env = std::fs::read_to_string(rig.seen().join(format!("env-{FIRST}"))).expect("a file");
    assert!(
        env.contains(GITHUB_SECRET),
        "the executor was not given the GitHub token"
    );
    assert!(env.contains("CLAUDE_CODE_OAUTH_TOKEN_FILE="), "{env}");
    // But not the token that speaks for this machine to the control plane.
    assert!(
        !env.contains(TOKEN) && !env.contains("HEPH_WORKER_TOKEN"),
        "{env}"
    );

    let traffic = String::from_utf8_lossy(&server.traffic()).into_owned();
    assert!(traffic.contains("/worker/claim"), "nothing was captured");
    for secret in [GITHUB_SECRET, CLAUDE_SECRET] {
        assert!(!traffic.contains(secret), "{secret} was sent");
    }
    // The token is the header, for it is what the control plane knows the request by, and
    // nowhere else.
    let header = format!("Bearer {TOKEN}");
    assert!(traffic.contains(&header), "no request carried the token");
    for request in server.requests() {
        assert!(
            !request.body.contains(TOKEN),
            "the token was in a body: {}",
            request.path
        );
        assert!(!request.path.contains(TOKEN));
    }
    assert_eq!(traffic.replace(&header, "").matches(TOKEN).count(), 0);

    for secret in [TOKEN, GITHUB_SECRET, CLAUDE_SECRET] {
        assert!(!worker.output().contains(secret), "{secret} was printed");
    }
}

#[test]
fn a_claim_with_nothing_is_asked_again_and_not_in_a_tight_loop() {
    let rig = Rig::new("empty");
    let claims = Arc::new(Mutex::new(0));
    let counted = Arc::clone(&claims);
    let server = Server::start(move |request| {
        if !is_claim(request) {
            return ok();
        }
        let mut count = counted.lock().expect("a lock");
        *count += 1;
        if *count <= 3 {
            Answer::NoContent
        } else {
            given(FIRST, 1, 120)
        }
    });
    let mut worker = rig.start(&server.address, &Executor::Completes);

    server.wait_for("the record", |s| {
        !s.to(&format!("/worker/workloads/{FIRST}/record"))
            .is_empty()
    });
    let asked: Vec<Instant> = server.to("/worker/claim").iter().map(|r| r.at).collect();
    assert!(asked.len() >= 4, "{}", asked.len());
    for pair in asked[..4].windows(2) {
        assert!(
            pair[1] - pair[0] >= Duration::from_millis(900),
            "an answer that came at once was asked again at once"
        );
    }
    assert!(worker.stops().success(), "{}", worker.output());
}

#[test]
fn a_claim_whose_answer_is_lost_runs_nothing_and_is_asked_again_after_a_wait() {
    let rig = Rig::new("lost-claim");
    let claims = Arc::new(Mutex::new(0));
    let counted = Arc::clone(&claims);
    let server = Server::start(move |request| {
        if !is_claim(request) {
            return ok();
        }
        let mut count = counted.lock().expect("a lock");
        *count += 1;
        match *count {
            // The control plane took the claim, and the answer never arrived.
            1 => Answer::Drop,
            2 => given(SECOND, 1, 120),
            _ => Answer::NoContent,
        }
    });
    let mut worker = rig.start(&server.address, &Executor::Completes);

    server.wait_for("the record", |s| {
        !s.to(&format!("/worker/workloads/{SECOND}/record"))
            .is_empty()
    });
    let asked: Vec<Instant> = server.to("/worker/claim").iter().map(|r| r.at).collect();
    assert!(
        asked[1] - asked[0] >= Duration::from_millis(450),
        "it came back at once, with no wait: {:?}",
        asked[1] - asked[0]
    );
    assert!(worker.stops().success(), "{}", worker.output());
    assert_eq!(
        rig.ran(),
        [SECOND],
        "only the workload it was given ran, and once"
    );
}

#[test]
fn events_whose_answer_is_lost_are_sent_again_unchanged_and_the_run_is_not_started_again() {
    let rig = Rig::new("lost-events");
    let dropped = Arc::new(Mutex::new(false));
    let claimed = Arc::new(Mutex::new(false));
    let server = Server::start(move |request| {
        if is_claim(request) {
            let mut claimed = claimed.lock().expect("a lock");
            if std::mem::replace(&mut *claimed, true) {
                return Answer::NoContent;
            }
            return given(FIRST, 1, 120);
        }
        if request.path.ends_with("/events")
            && !std::mem::replace(&mut *dropped.lock().expect("a lock"), true)
        {
            return Answer::Drop;
        }
        ok()
    });
    let mut worker = rig.start(&server.address, &Executor::Completes);

    server.wait_for("the record", |s| {
        !s.to(&format!("/worker/workloads/{FIRST}/record"))
            .is_empty()
    });
    // What was sent twice is the same, and what the control plane would keep has no gap.
    let mut kept = std::collections::BTreeMap::new();
    for event in server
        .to(&format!("/worker/workloads/{FIRST}/events"))
        .iter()
        .flat_map(events_of)
    {
        let sequence: u64 = event["sequence"]
            .as_str()
            .expect("a sequence")
            .parse()
            .expect("a number");
        if let Some(earlier) = kept.insert(sequence, event.clone()) {
            assert_eq!(earlier, event, "a different event at a kept number");
        }
    }
    assert_eq!(kept.keys().copied().collect::<Vec<_>>(), [1, 2, 3]);
    assert!(worker.stops().success(), "{}", worker.output());
    assert_eq!(rig.ran(), [FIRST]);
}

#[test]
fn a_workload_the_control_plane_takes_back_stops_its_executor_at_once_and_the_worker_goes_on() {
    let rig = Rig::new("taken-back");
    let claims = Arc::new(Mutex::new(0));
    let counted = Arc::clone(&claims);
    let server = Server::start(move |request| {
        if is_claim(request) {
            let mut count = counted.lock().expect("a lock");
            *count += 1;
            return match *count {
                1 => given(FIRST, 1, 120),
                2 => given(SECOND, 1, 120),
                _ => Answer::NoContent,
            };
        }
        // The lease lapsed: nothing about the first workload is taken any more.
        if request.path.contains(FIRST) {
            return Answer::Json(
                409,
                r#"{"error":"this token does not hold the workload"}"#.to_owned(),
            );
        }
        ok()
    });
    // The first workload's executor never ends by itself.
    let mut worker = rig.start(&server.address, &Executor::Hangs);

    server.wait_for("the first workload's events", |s| {
        !s.to(&format!("/worker/workloads/{FIRST}/events"))
            .is_empty()
    });
    let pid = {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(pid) = rig.pid(FIRST) {
                break pid;
            }
            assert!(Instant::now() < until, "no executor");
            std::thread::sleep(Duration::from_millis(25));
        }
    };
    assert!(
        gone_within_a_moment(&pid),
        "the executor of a workload that was taken back went on"
    );
    // The worker is not stuck on it: it asks again and runs what it is given.
    server.wait_for("the second workload", |s| {
        !s.to(&format!("/worker/workloads/{SECOND}/events"))
            .is_empty()
    });
    assert!(
        server
            .to(&format!("/worker/workloads/{FIRST}/record"))
            .is_empty(),
        "a record was sent for a workload that was taken back"
    );
    worker.stops();
}

#[test]
fn a_stop_during_a_run_stops_the_executor_gives_the_workload_back_and_exits() {
    let rig = Rig::new("stop");
    let server = gives_once(FIRST, 1);
    let mut worker = rig.start(&server.address, &Executor::SaysItWasCancelled);
    server.wait_for("the first event", |s| {
        !s.to(&format!("/worker/workloads/{FIRST}/events"))
            .is_empty()
    });
    // The executor has set up how it answers a stop.
    let until = Instant::now() + Duration::from_secs(10);
    while rig.pid(FIRST).is_none() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(25));
    }
    std::thread::sleep(Duration::from_millis(300));
    let claims = server.to("/worker/claim").len();

    let status = worker.stops();

    assert!(status.success(), "{}", worker.output());
    let pid = rig.pid(FIRST).expect("a pid");
    assert!(!alive(&pid), "the executor outlived the worker");
    // What the executor said when it was stopped (that it was cancelled) is not sent, because
    // the control plane would end the workload for good on it. The workload is given back.
    assert_eq!(sequences(&server), ["1"], "the cancelled event was sent");
    assert!(
        server
            .to(&format!("/worker/workloads/{FIRST}/record"))
            .is_empty(),
        "the cancelled record was sent on an operator's stop"
    );
    assert_eq!(
        server
            .to(&format!("/worker/workloads/{FIRST}/release"))
            .len(),
        1,
        "the workload was not given back"
    );
    assert_eq!(
        server.to("/worker/claim").len(),
        claims,
        "it claimed again after the stop"
    );
}

#[test]
fn a_workload_put_back_goes_on_from_the_number_the_control_plane_gives() {
    let rig = Rig::new("next-sequence");
    let server = gives_once(FIRST, 5);
    let mut worker = rig.start(&server.address, &Executor::Completes);

    server.wait_for("the record", |s| {
        !s.to(&format!("/worker/workloads/{FIRST}/record"))
            .is_empty()
    });
    assert_eq!(sequences(&server), ["5", "6", "7"]);
    assert!(worker.stops().success(), "{}", worker.output());
}

#[test]
fn a_run_that_says_nothing_is_kept_alive_by_a_heartbeat_well_inside_its_lease() {
    let rig = Rig::new("heartbeat");
    let claimed = Arc::new(Mutex::new(false));
    let server = Server::start(move |request| {
        if is_claim(request) {
            let mut claimed = claimed.lock().expect("a lock");
            if std::mem::replace(&mut *claimed, true) {
                return Answer::NoContent;
            }
            return given(FIRST, 1, 20);
        }
        ok()
    });
    let mut worker = rig.start(&server.address, &Executor::Hangs);

    let route = format!("/worker/workloads/{FIRST}/heartbeat");
    server.wait_for("two heartbeats", |s| s.to(&route).len() >= 2);
    let beats: Vec<Instant> = server.to(&route).iter().map(|r| r.at).collect();
    assert!(
        beats[1] - beats[0] <= Duration::from_secs(20 / 2),
        "the beats are spaced well inside half of a twenty second lease, with room for a loaded machine: {:?}",
        beats[1] - beats[0]
    );
    worker.stops();
}

#[test]
fn a_token_the_control_plane_refuses_ends_the_worker_without_showing_it() {
    let rig = Rig::new("refused");
    let server = Server::start(|_| Answer::Json(401, r#"{"error":"not authorized"}"#.to_owned()));
    let mut worker = rig.start(&server.address, &Executor::Completes);

    let status = worker.ends();

    assert_eq!(status.code(), Some(3), "{}", worker.output());
    assert!(
        worker.output().contains("does not accept the device token"),
        "{}",
        worker.output()
    );
    assert!(!worker.output().contains(TOKEN));
}

#[test]
fn a_control_plane_that_cannot_be_reached_is_tried_again_and_not_given_up_on() {
    let rig = Rig::new("unreachable");
    // Nothing listens here.
    let free = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    let address = format!("http://{}", free.local_addr().expect("an address"));
    drop(free);
    let mut worker = rig.start(&address, &Executor::Completes);
    std::thread::sleep(Duration::from_millis(2500));

    assert!(
        worker.child.try_wait().expect("to wait").is_none(),
        "it gave up"
    );
    let status = worker.stops();
    assert!(status.success(), "{}", worker.output());
    assert!(
        worker.output().contains("trying again"),
        "{}",
        worker.output()
    );
    assert!(!worker.output().contains(TOKEN));
}

#[test]
fn what_cannot_be_used_is_refused_before_anything_is_claimed() {
    let rig = Rig::new("usage");
    let run = |args: &[&str], token: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_heph"));
        command
            .current_dir(&rig.home.0)
            .arg("worker")
            .args(args)
            .env("HOME", &rig.home.0)
            .env("XDG_CONFIG_HOME", rig.home.0.join("config"))
            .env_remove("HEPH_WORKER_TOKEN_FILE")
            .env("HEPH_GITHUB_TOKEN", GITHUB_SECRET);
        match token {
            Some(token) => command.env("HEPH_WORKER_TOKEN", token),
            None => command.env_remove("HEPH_WORKER_TOKEN"),
        };
        command.output().expect("heph to run")
    };

    let no_address = run(&[], Some(TOKEN));
    assert_eq!(no_address.status.code(), Some(2));

    let no_token = run(&["--control-plane", "https://control.example.com"], None);
    assert_eq!(no_token.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&no_token.stderr).contains("HEPH_WORKER_TOKEN"));

    let in_the_clear = run(
        &["--control-plane", "http://control.example.com"],
        Some(TOKEN),
    );
    assert_eq!(in_the_clear.status.code(), Some(3));
    let said = String::from_utf8_lossy(&in_the_clear.stderr).into_owned();
    assert!(said.contains("https"), "{said}");
    assert!(!said.contains(TOKEN));
}

/// A control plane that gives `FIRST` to the first claim and `SECOND` to the second, and has
/// none after.
fn gives_two() -> Server {
    let claims = Arc::new(Mutex::new(0));
    Server::start(move |request| {
        if is_claim(request) {
            let mut count = claims.lock().expect("a lock");
            *count += 1;
            return match *count {
                1 => given(FIRST, 1, 120),
                2 => given(SECOND, 1, 120),
                _ => Answer::NoContent,
            };
        }
        ok()
    })
}

fn waits_for_pid(rig: &Rig, id: &str) -> String {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(pid) = rig.pid(id) {
            return pid;
        }
        assert!(Instant::now() < until, "no executor for {id}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn two_workloads_run_at_once_on_one_worker_and_finish_independently() {
    let rig = Rig::new("two-at-once");
    let server = gives_two();
    let mut worker = rig.start_with(
        &server.address,
        &Executor::HangsFirstCompletesOthers,
        &["--workloads", "2"],
    );

    // The second finishes while the first is still running: neither waited for the other.
    server.wait_for("the second workload's record", |s| {
        !s.to(&format!("/worker/workloads/{SECOND}/record"))
            .is_empty()
    });
    let first = waits_for_pid(&rig, FIRST);
    assert!(
        alive(&first),
        "the first workload's executor ended with the second's"
    );
    assert!(
        server
            .to(&format!("/worker/workloads/{FIRST}/record"))
            .is_empty(),
        "the first workload was ended by the second finishing"
    );
    assert!(
        server
            .to(&format!("/worker/workloads/{FIRST}/release"))
            .is_empty(),
        "the first workload was given back when the second finished"
    );

    // Each has a workspace of its own.
    let directories: Vec<_> = std::fs::read_dir(rig.home.0.join("runs/acme-widgets"))
        .expect("a runs directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        directories.iter().any(|d| d.ends_with(FIRST)),
        "{directories:?}"
    );
    assert!(
        directories.iter().any(|d| d.ends_with(SECOND)),
        "{directories:?}"
    );

    // Every claim says how many the worker may run at once.
    for claim in server.to("/worker/claim") {
        let body: Value = serde_json::from_str(&claim.body).expect("a JSON body");
        assert_eq!(body["slots"], 2);
    }
    // The finished slot goes on claiming while the other is still running.
    server.wait_for("a claim after the second finished", |s| {
        s.to("/worker/claim").len() >= 3
    });
    assert!(alive(&first));
    worker.stops();
}

#[test]
fn an_interrupt_gives_back_every_workload_the_worker_holds() {
    let rig = Rig::new("stop-both");
    let server = gives_two();
    let mut worker = rig.start_with(&server.address, &Executor::Hangs, &["--workloads", "2"]);
    server.wait_for("both workloads' events", |s| {
        !s.to(&format!("/worker/workloads/{FIRST}/events"))
            .is_empty()
            && !s
                .to(&format!("/worker/workloads/{SECOND}/events"))
                .is_empty()
    });
    let pids = [waits_for_pid(&rig, FIRST), waits_for_pid(&rig, SECOND)];
    assert!(
        pids.iter().all(|pid| alive(pid)),
        "both run before the stop"
    );

    let status = worker.stops();

    assert!(status.success(), "{}", worker.output());
    for (id, pid) in [FIRST, SECOND].iter().zip(&pids) {
        assert!(!alive(pid), "the executor of {id} outlived the worker");
        assert_eq!(
            server.to(&format!("/worker/workloads/{id}/release")).len(),
            1,
            "{id} was not given back"
        );
        assert!(
            server
                .to(&format!("/worker/workloads/{id}/record"))
                .is_empty()
        );
    }
}

#[test]
fn a_worker_runs_one_workload_at_a_time_unless_asked_for_more() {
    let rig = Rig::new("one-slot");
    let server = gives_two();
    let mut worker = rig.start(&server.address, &Executor::HangsFirstCompletesOthers);
    server.wait_for("the first workload", |s| {
        !s.to(&format!("/worker/workloads/{FIRST}/events"))
            .is_empty()
    });
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        server.to("/worker/claim").len(),
        1,
        "it claimed a second while busy"
    );
    let claim: Value = serde_json::from_str(&server.to("/worker/claim")[0].body).expect("JSON");
    assert_eq!(claim["slots"], 1);
    worker.stops();
}

#[test]
fn a_number_of_workloads_that_is_zero_or_above_the_ceiling_is_refused_before_anything_is_claimed() {
    let rig = Rig::new("slots-refused");
    let server = gives_two();
    for (number, said) in [
        ("0", "at least one"),
        ("65", "above the most"),
        ("many", "whole number"),
    ] {
        let mut worker = rig.start_with(
            &server.address,
            &Executor::Completes,
            &["--workloads", number],
        );
        let status = worker.ends();
        assert_eq!(status.code(), Some(3), "{number}: {}", worker.output());
        let output = worker.output();
        assert!(
            output.contains(said) && output.contains("--workloads"),
            "{number}: {output}"
        );
    }
    assert!(server.requests().is_empty(), "something was claimed");
}

#[test]
fn a_second_worker_on_the_same_runs_directory_does_not_stop_the_first_ones_live_run() {
    let first_rig = Rig::new("sweep-first");
    let first_server = gives_once(FIRST, 1);
    let mut first = first_rig.start(&first_server.address, &Executor::HangsInItsRunDirectory);
    let pid = waits_for_pid(&first_rig, FIRST);

    // A second worker, a process of its own, sweeps the same runs directory before each claim.
    let second_rig = Rig::sharing_runs_with("sweep-second", &first_rig);
    let second_server = Server::start(|request| {
        if is_claim(request) {
            Answer::NoContent
        } else {
            ok()
        }
    });
    let mut second = second_rig.start(&second_server.address, &Executor::Completes);
    second_server.wait_for("the second worker's claim, which follows its sweep", |s| {
        !s.to("/worker/claim").is_empty()
    });
    let live = alive(&pid);
    let said = second.output();
    second.stops();
    first.stops();

    assert!(
        live,
        "the second worker stopped the first one's live run: {said}"
    );
    assert!(
        !said.contains("stopping process"),
        "the second worker took the live run for a finished one: {said}"
    );
}
