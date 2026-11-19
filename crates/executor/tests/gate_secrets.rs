//! The secrets a repository's gates declare: given to the executor by a variable
//! or a file the maintainer keeps outside any repository, held out of reach of
//! everything the executor runs, and handed only to the gate that declares them.

mod common;

use std::os::unix::fs::PermissionsExt as _;

use common::{IDENTITY, Rig, Scratch, agent_script, finished, run_with_env, source, stand_in_gh};
use contracts::v1alpha1::finished::Outcome;
use serde_json::json;

const VALUE: &str = "a-pixabay-key";

/// A repository whose bootstrap step, a gate that declares the secret, a gate that
/// does not, and the agent each write their environment to `seen/`.
fn rig(name: &str) -> (Rig, String) {
    let scratch = Scratch::new(name);
    let out = scratch.path("seen");
    std::fs::create_dir_all(&out).expect("a directory");
    let text = format!(
        "schema_version = 0\n\n[components.app]\nroot = \".\"\npaths = [\"**\"]\n\n\
         [[bootstrap]]\nrun = [\"sh\", \"-c\", \"env > {out}/bootstrap-env.txt\"]\ntimeout_seconds = 30\n\n\
         [gates.needs]\nrun = [\"sh\", \"-c\", \"env > {out}/needs-env.txt\"]\ntimeout_seconds = 30\n\n\
         [gates.needs.requires]\nPIXABAY_API_KEY = \"secret\"\n\n\
         [gates.plain]\nrun = [\"sh\", \"-c\", \"env > {out}/plain-env.txt\"]\ntimeout_seconds = 30\n",
        out = out.display()
    );
    let (source_dir, commit) = source(&scratch, &text);
    let script = agent_script(&format!("env > {}/agent-env.txt;", out.display()));
    (
        Rig {
            scratch,
            commit,
            source: source_dir,
        },
        script,
    )
}

fn secret_file(scratch: &Scratch, content: &str, mode: u32) -> String {
    let path = scratch.write("secrets/pixabay", content);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("to chmod");
    path.display().to_string()
}

fn run(rig: Rig, script: &str, environment: &[(&str, &str)]) -> common::Ran {
    let gh = stand_in_gh(&rig.scratch, 0);
    let arguments = [
        "--agent-command".to_owned(),
        json!(["sh", "-c", script]).to_string(),
        "--gh".to_owned(),
        gh.display().to_string(),
    ];
    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let mut all: Vec<(&str, &str)> = IDENTITY
        .iter()
        .copied()
        .filter(|(name, _)| *name != "HEPH_GITHUB_TOKEN")
        .collect();
    all.extend_from_slice(environment);
    run_with_env(
        rig.scratch,
        &common::workload(&rig.commit),
        None,
        &refs,
        &all,
    )
}

fn seen(ran: &common::Ran, file: &str) -> String {
    std::fs::read_to_string(ran.scratch.path(&format!("seen/{file}")))
        .unwrap_or_else(|e| panic!("{file}: {e}"))
}

#[test]
fn a_gate_gets_the_secret_it_declares_from_a_variable_or_a_file_and_nothing_else_does() {
    for kind in ["variable", "file", "file with a trailing newline"] {
        let (rig, script) = rig(&format!("gate-secret-{}", kind.len()));
        let file = secret_file(
            &rig.scratch,
            &if kind == "file" {
                VALUE.to_owned()
            } else {
                format!("{VALUE}\n")
            },
            0o600,
        );
        let pair = match kind {
            "variable" => ("HEPH_GATE_SECRET_PIXABAY_API_KEY", VALUE),
            _ => ("HEPH_GATE_SECRET_PIXABAY_API_KEY_FILE", file.as_str()),
        };

        let ran = run(rig, &script, &[pair]);

        assert_eq!(ran.code(), 0, "{kind}: {}", ran.stderr());
        assert_eq!(
            finished(&ran.events()).0,
            Outcome::Completed,
            "{kind}: {}",
            ran.stderr()
        );
        let needs = seen(&ran, "needs-env.txt");
        assert!(
            needs.lines().any(|l| l == format!("PIXABAY_API_KEY={VALUE}")),
            "{kind}: the gate did not get it exactly:\n{needs}"
        );
        for file in ["bootstrap-env.txt", "plain-env.txt", "agent-env.txt"] {
            let env = seen(&ran, file);
            for forbidden in [VALUE, "PIXABAY_API_KEY", "HEPH_GATE_SECRET", "secrets/pixabay"] {
                assert!(
                    !env.contains(forbidden),
                    "{kind}: {file} shows {forbidden}:\n{env}"
                );
            }
        }
        assert!(
            !needs.contains("HEPH_GATE_SECRET") && !needs.contains("secrets/pixabay"),
            "{kind}: the gate learned how the secret was given:\n{needs}"
        );
    }
}

#[test]
fn a_secret_given_the_ordinary_way_still_meets_the_requirement() {
    let (rig, script) = rig("gate-secret-ordinary");

    let ran = run(rig, &script, &[("PIXABAY_API_KEY", VALUE)]);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert!(seen(&ran, "needs-env.txt").contains(&format!("PIXABAY_API_KEY={VALUE}")));
}

#[test]
fn a_secret_that_is_not_given_is_a_gate_that_could_not_be_run_and_names_the_variable() {
    let (rig, script) = rig("gate-secret-absent");

    let ran = run(rig, &script, &[]);

    let (outcome, reason) = finished(&ran.events());
    assert_eq!(outcome, Outcome::Failed, "{}", ran.stderr());
    assert!(
        reason.contains("gate needs could not be run")
            && reason.contains("PIXABAY_API_KEY (secret)"),
        "{reason}"
    );
    assert!(
        !ran.scratch.path("seen/needs-env.txt").exists(),
        "the gate ran without its secret"
    );
    assert!(
        ran.scratch.path("seen/plain-env.txt").exists(),
        "the other gate did not run"
    );
}

#[test]
fn the_value_is_in_no_event_no_record_and_no_output() {
    let (rig, script) = rig("gate-secret-nowhere");
    let file = secret_file(&rig.scratch, VALUE, 0o600);

    let ran = run(rig, &script, &[("HEPH_GATE_SECRET_PIXABAY_API_KEY_FILE", &file)]);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let record = std::fs::read_to_string(ran.scratch.path("record.json")).expect("a record");
    for (what, text) in [
        ("events", String::from_utf8_lossy(&ran.output.stdout).into_owned()),
        ("record", record),
        ("stderr", ran.stderr()),
    ] {
        assert!(!text.contains(VALUE), "{what} holds the secret:\n{text}");
    }
}

#[test]
fn a_variable_and_a_file_for_the_same_secret_are_refused_and_nothing_is_created() {
    let (rig, script) = rig("gate-secret-both");
    let file = secret_file(&rig.scratch, VALUE, 0o600);

    let ran = run(
        rig,
        &script,
        &[
            ("HEPH_GATE_SECRET_PIXABAY_API_KEY", "from-the-variable"),
            ("HEPH_GATE_SECRET_PIXABAY_API_KEY_FILE", &file),
        ],
    );

    assert_eq!(ran.code(), 1, "{}", ran.stderr());
    assert!(ran.stderr().contains("both set"), "{}", ran.stderr());
    assert!(!ran.stderr().contains("from-the-variable"));
    assert!(!ran.scratch.path("work").exists());
}

#[test]
fn a_file_others_can_read_or_one_that_is_missing_or_empty_is_refused_and_nothing_is_created() {
    for kind in ["0644", "0640", "0604", "missing", "empty"] {
        let (rig, script) = rig(&format!("gate-secret-bad-{kind}"));
        let path = match kind {
            "missing" => rig.scratch.path("secrets/nowhere").display().to_string(),
            "empty" => secret_file(&rig.scratch, " \n", 0o600),
            mode => secret_file(
                &rig.scratch,
                VALUE,
                u32::from_str_radix(mode, 8).expect("an octal mode"),
            ),
        };

        let ran = run(rig, &script, &[("HEPH_GATE_SECRET_PIXABAY_API_KEY_FILE", &path)]);

        assert_eq!(ran.code(), 1, "{kind}: {}", ran.stderr());
        assert!(
            ran.stderr().contains("HEPH_GATE_SECRET_PIXABAY_API_KEY_FILE"),
            "{kind}: {}",
            ran.stderr()
        );
        assert!(!ran.stderr().contains(VALUE), "{kind}: the secret was printed");
        assert!(!ran.scratch.path("work").exists(), "{kind}: a workspace was created");
    }
}

#[test]
fn a_name_that_cannot_be_a_variable_is_refused_and_nothing_is_created() {
    for name in [
        "HEPH_GATE_SECRET_",
        "HEPH_GATE_SECRET__FILE",
        "HEPH_GATE_SECRET_9LIVES",
        "HEPH_GATE_SECRET_BAD-NAME",
    ] {
        let (rig, script) = rig("gate-secret-name");

        let ran = run(rig, &script, &[(name, VALUE)]);

        assert_eq!(ran.code(), 1, "{name}: {}", ran.stderr());
        assert!(ran.stderr().contains(name), "{name}: {}", ran.stderr());
        assert!(!ran.stderr().contains(VALUE), "{name}: the secret was printed");
        assert!(!ran.scratch.path("work").exists(), "{name}: a workspace was created");
    }
}
