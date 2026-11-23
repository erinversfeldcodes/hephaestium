//! The clone reaches a private repository the way the push does: through `gh`'s credential
//! helper with the run's token, and never by prompting. The remote here answers 401 to a
//! request that carries no credential, as a private repository's host does.

mod common;

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{Scratch, finished, workload};
use contracts::v1alpha1::finished::Outcome;

const TOKEN: &str = "a-token-for-the-helper";

/// What the remote saw: the `Authorization` header of each request, empty when it had none.
type Seen = Arc<Mutex<Vec<String>>>;

/// A remote that answers 401 to a request with no credential and 404 to one with, so a clone
/// of it can never succeed but shows what it sent.
fn remote() -> (String, Seen) {
    serve(None, None)
}

/// As [`remote`], but a request that carries a credential is answered from the files of the
/// bare repository `root` (git's dumb protocol) until the file `revoked` exists, after which
/// the credential is refused too. That lets a clone succeed and the fetch after it be the
/// request that is turned away.
fn serve(root: Option<PathBuf>, revoked: Option<PathBuf>) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let url = format!(
        "http://{}/owner/name.git",
        listener.local_addr().expect("an address")
    );
    let seen: Seen = Arc::default();
    let record = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut authorization = String::new();
            let mut line = String::new();
            let mut target = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line.trim() != "" {
                if target.is_empty() {
                    line.split(' ')
                        .nth(1)
                        .unwrap_or_default()
                        .clone_into(&mut target);
                }
                if line.to_ascii_lowercase().starts_with("authorization:") {
                    line["authorization:".len()..]
                        .trim()
                        .clone_into(&mut authorization);
                }
                line.clear();
            }
            record.lock().expect("a lock").push(authorization.clone());
            let turned_away =
                authorization.is_empty() || revoked.as_ref().is_some_and(|f| f.exists());
            let file = root
                .as_ref()
                .map(|root| {
                    let path = target.split('?').next().unwrap_or_default();
                    root.join(path.trim_start_matches("/owner/name.git/"))
                })
                .and_then(|path| std::fs::read(path).ok());
            let reply: Vec<u8> = if turned_away {
                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"remote\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
            } else if let Some(body) = file {
                let mut reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                reply.extend(body);
                reply
            } else {
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
            };
            let _ = reader.get_mut().write_all(&reply);
        }
    });
    (url, seen)
}

/// A `gh` that is a credential helper: it gives `GH_TOKEN` as the password when it has one,
/// and records that it was asked.
fn helper(scratch: &Scratch) -> std::path::PathBuf {
    let asked = scratch.path("helper-asked");
    let script = format!(
        "#!/bin/sh\n\
         echo \"$@\" >> {asked}\n\
         [ \"$1 $2 $3\" = 'auth git-credential get' ] || exit 0\n\
         [ -n \"$GH_TOKEN\" ] || exit 1\n\
         printf 'username=x-access-token\\npassword=%s\\n' \"$GH_TOKEN\"\n",
        asked = asked.display()
    );
    let gh = scratch.path("gh");
    std::fs::write(&gh, script).expect("a script");
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).expect("to chmod");
    gh
}

struct Cloned {
    scratch: Scratch,
    seen: Seen,
    outcome: Outcome,
    reason: String,
    took: Duration,
}

fn clone_from_private(name: &str, token: Option<&str>) -> Cloned {
    let scratch = Scratch::new(name);
    let (url, seen) = remote();
    let gh = helper(&scratch);
    let workload = workload("0123456789abcdef0123456789abcdef01234567");
    scratch.write("workload.json", &workload.to_string());
    let mut command = Command::new(env!("CARGO_BIN_EXE_executor"));
    command
        .args(["run", "--workload"])
        .arg(scratch.path("workload.json"))
        .arg("--work-dir")
        .arg(scratch.path("work"))
        .arg("--record")
        .arg(scratch.path("record.json"))
        .args(["--source", &url, "--gh"])
        .arg(&gh)
        .args(["--agent-command", r#"["true"]"#])
        // A prompt would read the terminal, so a run that prompts hangs and this fails.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env_remove("HEPH_GITHUB_TOKEN")
        .env_remove("HEPH_GITHUB_READ_TOKEN")
        .stdin(std::process::Stdio::null());
    if let Some(token) = token {
        // Delivery without a token is refused up front, so only a run that holds one delivers.
        command.arg("--deliver").env("HEPH_GITHUB_TOKEN", token);
    }
    let started = Instant::now();
    let output = command.output().expect("the executor to run");
    let took = started.elapsed();
    let ran = common::Ran { output, scratch };
    let (outcome, reason) = finished(&ran.events());
    Cloned {
        scratch: ran.scratch,
        seen,
        outcome,
        reason,
        took,
    }
}

#[test]
fn the_clone_gives_the_token_through_the_helper() {
    let ran = clone_from_private("clone-token", Some(TOKEN));
    let asked = std::fs::read_to_string(ran.scratch.path("helper-asked")).unwrap_or_default();
    assert!(asked.contains("auth git-credential get"), "{asked}");
    let seen = ran.seen.lock().expect("a lock").clone();
    // user:password as the helper gave them, base64.
    let expected = format!("Basic {}", base64_of(&format!("x-access-token:{TOKEN}")));
    assert!(seen.contains(&expected), "{seen:?}");
    assert_eq!(ran.outcome, Outcome::Failed, "{}", ran.reason);
    assert!(ran.reason.contains("could not clone"), "{}", ran.reason);
    assert!(!ran.reason.contains(TOKEN), "{}", ran.reason);
}

#[test]
fn a_missing_credential_fails_the_clone_at_once_and_says_so() {
    let ran = clone_from_private("clone-no-token", None);
    assert_eq!(ran.outcome, Outcome::Failed, "{}", ran.reason);
    assert!(ran.took < Duration::from_secs(30), "took {:?}", ran.took);
    assert!(ran.reason.contains("could not clone"), "{}", ran.reason);
    assert!(ran.reason.contains("token"), "{}", ran.reason);
}

/// A run that clones, does work and delivers, against a remote that serves its repository to a
/// credential until the agent has run and refuses everyone after. The fetch of the base branch
/// is the request that is refused.
#[test]
fn the_fetch_of_the_base_branch_gives_the_token_through_the_helper() {
    let rig = common::rig("fetch-token", &common::manifest(&[]));
    let scratch = &rig.scratch;
    let served = scratch.path("served.git");
    common::git(
        scratch.0.as_path(),
        &[
            "clone",
            "-q",
            "--bare",
            rig.source.to_str().expect("UTF-8"),
            served.to_str().expect("UTF-8"),
        ],
    );
    common::git(&served, &["update-server-info"]);
    let revoked = scratch.path("agent-ran");
    let (url, seen) = serve(Some(served), Some(revoked.clone()));
    // The dumb protocol cannot take a push, so pushes to the remote go to a repository on disk
    // that takes them; only the clone and the fetch are what this test is about.
    let pushed = scratch.path("pushed.git");
    common::git(
        scratch.0.as_path(),
        &[
            "clone",
            "-q",
            "--bare",
            rig.source.to_str().expect("UTF-8"),
            pushed.to_str().expect("UTF-8"),
        ],
    );
    let config = scratch.write(
        "gitconfig",
        &format!("[url \"{}\"]\n\tpushInsteadOf = {url}\n", pushed.display()),
    );
    let gh = helper(scratch);
    let agent = common::agent_script(&format!("touch {};", revoked.display()));
    let workload = workload(&rig.commit);
    scratch.write("workload.json", &workload.to_string());
    let mut command = Command::new(env!("CARGO_BIN_EXE_executor"));
    command
        .args(["run", "--workload"])
        .arg(scratch.path("workload.json"))
        .arg("--work-dir")
        .arg(scratch.path("work"))
        .arg("--record")
        .arg(scratch.path("record.json"))
        .args(["--source", &url, "--gh"])
        .arg(&gh)
        .args([
            "--agent-command",
            &serde_json::json!(["sh", "-c", agent]).to_string(),
        ])
        .arg("--deliver")
        .envs(common::IDENTITY)
        .env("HEPH_GITHUB_TOKEN", TOKEN)
        .env("GIT_CONFIG_GLOBAL", &config)
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env_remove("HEPH_GITHUB_READ_TOKEN")
        .stdin(std::process::Stdio::null());
    let started = Instant::now();
    let output = command.output().expect("the executor to run");
    let took = started.elapsed();
    let ran = common::Ran {
        output,
        scratch: rig.scratch,
    };
    let (outcome, reason) = finished(&ran.events());
    assert!(revoked.exists(), "the agent never ran: {reason}");
    assert_eq!(outcome, Outcome::Failed, "{reason}");
    assert!(
        reason.contains("could not fetch the base branch"),
        "{reason}"
    );
    assert!(!reason.contains(TOKEN), "{reason}");
    assert!(took < Duration::from_secs(30), "took {took:?}");
    // The requests the remote saw after the agent ran are the fetch's; each of them carried
    // the token the helper gave, and the clone's before it did too.
    let expected = format!("Basic {}", base64_of(&format!("x-access-token:{TOKEN}")));
    let seen = seen.lock().expect("a lock").clone();
    assert_eq!(seen.last(), Some(&expected), "{seen:?}");
    let asked = std::fs::read_to_string(ran.scratch.path("helper-asked")).unwrap_or_default();
    assert!(
        asked.matches("auth git-credential get").count() >= 2,
        "the clone and the fetch each ask the helper: {asked}"
    );
}

fn base64_of(text: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in text.as_bytes().chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(TABLE[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}
