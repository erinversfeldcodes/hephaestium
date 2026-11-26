//! Reading a local commit under one lens, before anything is pushed.
//!
//! The reader is a fresh read-only agent, here a shell script that writes whatever
//! verdict a test wants, so each test says what the executor does with one particular
//! answer. Nothing here has a pull request: the commit is in the workspace and nowhere
//! else.

mod common;

use std::cell::Cell;
use std::path::PathBuf;
use std::time::Duration;

use common::{Scratch, git, manifest, source, workload};
use contracts::v1alpha1::{Lens, Workload};
use executor::adapter::Stub;
use executor::prepush::{self, Reading};
use executor::secrets::Secrets;
use executor::stream::Stream;
use serde_json::{Value, json};

const LENS_TEXT: &str = "---\nname: rust\n---\nLook at ownership and error handling.";

fn lens(pack: &str) -> Lens {
    Lens {
        pack: pack.to_owned(),
        document: LENS_TEXT.replace("rust", pack),
    }
}

/// A workspace with one commit past `main`, its id, and the workload that made it.
struct Local {
    scratch: Scratch,
    workspace: PathBuf,
    commit: String,
    workload: Workload,
}

fn local(name: &str) -> Local {
    let scratch = Scratch::new(name);
    let (workspace, base) = source(&scratch, &manifest(&[]));
    let workload =
        executor::workload::read(&workload(&base).to_string()).expect("the contract's own case");
    // A clone keeps the verdict file out of the workspace's changes; this stands in for one.
    std::fs::write(workspace.join(".git/info/exclude"), ".heph-verdict.json\n").expect("exclude");
    scratch.write("source/web/change.txt", "the change\n");
    git(&workspace, &["add", "."]);
    git(&workspace, &["commit", "-q", "-m", "feat: the change"]);
    let commit = git(&workspace, &["rev-parse", "HEAD"]);
    Local {
        scratch,
        workspace,
        commit,
        workload,
    }
}

/// A verdict that passes the contract, for this commit and lens.
fn verdict(commit: &str, pack: &str) -> Value {
    json!({
        "schema_version": 0,
        "lens": pack,
        "commit": commit,
        "verdict": "approved",
        "findings": [{
            "severity": "P3",
            "blocking": false,
            "title": "A name that says what it holds",
            "evidence": "web/change.txt:1 is called `the change`."
        }],
        "notes": ["The change is small."]
    })
}

fn says() -> String {
    format!("echo '{}'", json!({"agentMessage": {"text": "read it"}}))
}

/// An agent that writes `contents` as its verdict.
fn writes(contents: &Value) -> String {
    format!("printf '%s' '{contents}' > .heph-verdict.json; {}", says())
}

/// Read the local commit under `pack` with an agent that runs `script`. Returns what the
/// reading came to, and what the workload had spent when it was done.
fn read(local: &Local, pack: &str, script: &str, spent: f64) -> (Result<Value, String>, f64) {
    let adapter = Stub {
        command: vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
        env_names: Vec::new(),
    };
    let secrets = Secrets::default();
    let cell = Cell::new(spent);
    let mut events: Vec<u8> = Vec::new();
    let mut stream = Stream::new(&mut events, &local.workload.id);
    let reading = Reading {
        adapter: &adapter,
        heartbeat: Duration::from_secs(60),
        secrets: &secrets,
        spent: &cell,
    };
    let said = prepush::read(
        &reading,
        &local.workload,
        &lens(pack),
        &local.workspace,
        &local.commit,
        &mut stream,
    )
    .expect("the events could be written");
    (said, cell.get())
}

fn refused(said: &Result<Value, String>, words: &[&str]) {
    let Err(reason) = said else {
        panic!("a verdict was kept: {said:?}");
    };
    for word in words {
        assert!(reason.contains(word), "{word:?} is not in: {reason}");
    }
}

#[test]
fn a_verdict_that_passes_the_contract_is_kept_and_does_not_outlive_the_reading() {
    let local = local("prepush-ok");
    let said = verdict(&local.commit, "rust");

    let (kept, _) = read(&local, "rust", &writes(&said), 0.0);

    assert_eq!(kept.expect("a verdict"), said);
    assert!(
        !local.workspace.join(".heph-verdict.json").exists(),
        "the next reader must not find this one's verdict"
    );
}

#[test]
fn an_approval_that_names_a_blocking_finding_is_no_verdict() {
    let local = local("prepush-contradiction");
    let mut said = verdict(&local.commit, "rust");
    said["findings"][0]["blocking"] = json!(true);

    let (kept, _) = read(&local, "rust", &writes(&said), 0.0);

    refused(&kept, &["contract"]);
}

#[test]
fn a_verdict_for_another_lens_or_another_commit_is_no_verdict() {
    let local = local("prepush-names");

    let (kept, _) = read(&local, "rust", &writes(&verdict(&local.commit, "css")), 0.0);
    refused(&kept, &["lens", "css"]);

    let other = "0123456789abcdef0123456789abcdef01234567";
    let (kept, _) = read(&local, "rust", &writes(&verdict(other, "rust")), 0.0);
    refused(&kept, &["commit", other]);
}

#[test]
fn an_agent_that_leaves_no_verdict_gives_none() {
    let local = local("prepush-none");

    let (kept, _) = read(&local, "rust", &says(), 0.0);

    refused(&kept, &["no verdict"]);
}

#[test]
fn an_agent_that_changes_a_file_gives_none_whatever_it_wrote() {
    let local = local("prepush-edits");
    let script = format!(
        "echo edited >> web/change.txt; {}",
        writes(&verdict(&local.commit, "rust"))
    );

    let (kept, _) = read(&local, "rust", &script, 0.0);

    refused(&kept, &["changed the workspace"]);
}

#[test]
fn an_agent_that_only_lets_gh_write_its_own_state_still_gives_its_verdict() {
    let local = local("prepush-gh-state");
    let script = format!(
        "mkdir -p .local/state/gh .config/gh; echo id > .local/state/gh/device-id; echo c > .config/gh/hosts.yml; {}",
        writes(&verdict(&local.commit, "rust"))
    );

    let (kept, _) = read(&local, "rust", &script, 0.0);

    assert!(kept.is_ok(), "{kept:?}");
}

#[test]
fn an_agent_that_edits_a_source_file_beside_gh_state_is_still_caught() {
    let local = local("prepush-gh-and-source");
    let script = format!(
        "mkdir -p .local/state/gh src; echo id > .local/state/gh/device-id; echo 'fn x() {{}}' > src/lib.rs; {}",
        writes(&verdict(&local.commit, "rust"))
    );

    let (kept, _) = read(&local, "rust", &script, 0.0);

    refused(&kept, &["changed the workspace", "src/lib.rs"]);
}

#[test]
fn an_agent_that_fails_gives_none() {
    let local = local("prepush-fails");

    let (kept, _) = read(&local, "rust", "exit 3", 0.0);

    assert!(kept.is_err(), "{kept:?}");
}

#[test]
fn a_reading_does_not_start_when_the_budget_is_already_spent() {
    let local = local("prepush-budget");
    let marker = local.scratch.path("started");
    let script = format!("touch {}; {}", marker.display(), says());

    // The contract's own case has a budget of a few dollars; this is more than that.
    let (kept, spent) = read(&local, "rust", &script, 1_000.0);

    refused(&kept, &["budget"]);
    assert!(
        !marker.exists(),
        "the reader was started with nothing to spend"
    );
    assert!((spent - 1_000.0).abs() < f64::EPSILON, "{spent}");
}

#[test]
fn what_a_reader_spends_goes_on_the_runs_one_account() {
    let local = local("prepush-spend");
    let cost = format!(
        "echo '{}'",
        json!({"cost": {"unit": "COST_UNIT_USD", "amount": 0.25, "source": "SOURCE_REPORTED_BY_RUNTIME"}})
    );
    let script = format!("{cost}; {}", writes(&verdict(&local.commit, "rust")));

    let (kept, spent) = read(&local, "rust", &script, 0.5);

    assert!(kept.is_ok(), "{kept:?}");
    assert!((spent - 0.75).abs() < 1e-9, "spent {spent}");
}

#[test]
fn a_reader_is_not_told_of_a_pull_request_or_of_anyone_elses_verdict() {
    let local = local("prepush-prompt");

    let asked = prepush::as_review(&local.workload, &lens("rust"), &local.commit);
    let review = asked.review.as_ref().expect("a read is a review");
    let prompt = executor::review::prompt(&asked, review, None);

    // What the executor writes, before the contract's own text, which has words of its own.
    let ours = prompt
        .split("must satisfy this JSON Schema")
        .next()
        .expect("a prompt");
    assert!(!ours.contains("pull request"), "{ours}");
    assert!(!ours.contains("http"), "{ours}");
    assert!(prompt.contains(&local.commit), "the commit it reads");
    assert!(prompt.contains("`rust`"), "the lens");
    assert!(
        prompt.contains("Look at ownership and error handling."),
        "the lens text"
    );
    assert!(
        prompt.contains("git diff origin/"),
        "how to read the change"
    );
    assert!(
        prompt.contains(".heph-verdict.json"),
        "where to put the verdict"
    );
    assert!(asked.revision.is_none(), "a reader is never a revision");
    assert!(
        asked.lenses.is_empty(),
        "nor does it read under lenses itself"
    );
    assert_eq!(asked.prepush_rounds, 0);
}
