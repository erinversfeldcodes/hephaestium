//! Where the executor's secrets come from, and where they are held: a variable
//! or a file the maintainer keeps outside any repository, taken once at the
//! start and out of reach of everything the executor runs.

mod common;

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use common::{
    IDENTITY, Rig, Scratch, agent_script, finished, manifest, run_with_env, source, stand_in_gh,
};
use contracts::v1alpha1::finished::Outcome;
use serde_json::json;

const GITHUB: &str = "a-github-token";
const CLAUDE: &str = "a-claude-token";

/// A repository whose bootstrap step and gate each write their environment to
/// `seen/`, and an agent that does the same.
fn capturing_rig(name: &str) -> (Rig, String) {
    let scratch = Scratch::new(name);
    let out = scratch.path("seen");
    std::fs::create_dir_all(&out).expect("a directory");
    let text = manifest(&[(
        &[
            "sh",
            "-c",
            &format!("env > {}/bootstrap-env.txt", out.display()),
        ],
        None,
        30,
    )]) + &format!(
        "\n[gates.env]\nrun = [\"sh\", \"-c\", \"env > {}/gate-env.txt\"]\ntimeout_seconds = 30\n",
        out.display()
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

/// A secret file with this content and mode, and its path.
fn secret_file(scratch: &Scratch, name: &str, content: &str, mode: u32) -> String {
    let path = scratch.write(&format!("secrets/{name}"), content);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("to chmod");
    path.display().to_string()
}

fn run(rig: Rig, script: &str, deliver: bool, environment: &[(&str, &str)]) -> common::Ran {
    let gh = stand_in_gh(&rig.scratch, 0);
    let mut arguments = vec![
        "--agent-command".to_owned(),
        json!(["sh", "-c", script]).to_string(),
        "--agent-env".to_owned(),
        "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
        "--gh".to_owned(),
        gh.display().to_string(),
    ];
    if deliver {
        arguments.push("--deliver".to_owned());
    }
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
fn a_github_token_in_a_file_is_used_for_delivery_and_a_trailing_newline_is_trimmed() {
    let (rig, script) = capturing_rig("secrets-github-file");
    let file = secret_file(&rig.scratch, "github", &format!("{GITHUB}\n"), 0o600);

    let ran = run(rig, &script, true, &[("HEPH_GITHUB_TOKEN_FILE", &file)]);

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(
        finished(&ran.events()).0,
        Outcome::Completed,
        "{}",
        ran.stderr()
    );
    // gh's stand-in prints the token and a newline, so a newline kept from the
    // file would show as a second one.
    assert_eq!(seen(&ran, "gh-token.txt"), format!("{GITHUB}\n"));
}

#[test]
fn a_claude_token_from_a_file_or_the_environment_reaches_the_agent_and_not_a_bootstrap_step_or_a_gate()
 {
    for source_kind in ["variable", "file"] {
        let (rig, script) = capturing_rig(&format!("secrets-claude-{source_kind}"));
        let file = secret_file(&rig.scratch, "claude", CLAUDE, 0o600);
        let pair = match source_kind {
            "variable" => ("CLAUDE_CODE_OAUTH_TOKEN", CLAUDE),
            _ => ("CLAUDE_CODE_OAUTH_TOKEN_FILE", file.as_str()),
        };

        let ran = run(rig, &script, false, &[pair]);

        assert_eq!(ran.code(), 0, "{source_kind}: {}", ran.stderr());
        assert!(
            seen(&ran, "agent-env.txt").contains(&format!("CLAUDE_CODE_OAUTH_TOKEN={CLAUDE}")),
            "{source_kind}: the agent did not get its token"
        );
        for file in ["bootstrap-env.txt", "gate-env.txt"] {
            let env = seen(&ran, file);
            assert!(
                !env.contains(CLAUDE),
                "{source_kind}: {file} holds the token:\n{env}"
            );
            assert!(
                !env.contains("CLAUDE_CODE_OAUTH_TOKEN"),
                "{source_kind}: {file} names the token variable:\n{env}"
            );
        }
    }
}

#[test]
fn no_secret_variable_or_pointer_is_in_what_a_bootstrap_step_a_gate_or_the_agent_sees() {
    let (rig, script) = capturing_rig("secrets-pointers");
    let github = secret_file(&rig.scratch, "github", GITHUB, 0o600);
    let claude = secret_file(&rig.scratch, "claude", CLAUDE, 0o600);

    let ran = run(
        rig,
        &script,
        true,
        &[
            ("HEPH_GITHUB_TOKEN_FILE", &github),
            ("CLAUDE_CODE_OAUTH_TOKEN_FILE", &claude),
        ],
    );

    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    for file in ["bootstrap-env.txt", "gate-env.txt", "agent-env.txt"] {
        let env = seen(&ran, file);
        for forbidden in [
            "HEPH_GITHUB_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN_FILE",
            "secrets/github",
            "secrets/claude",
            GITHUB,
        ] {
            assert!(!env.contains(forbidden), "{file} shows {forbidden}:\n{env}");
        }
    }
}

#[test]
fn a_secret_file_the_group_or_others_can_read_is_refused_and_nothing_is_created() {
    for (name, mode) in [("0644", 0o644), ("0640", 0o640), ("0604", 0o604)] {
        for (variable, deliver) in [
            ("HEPH_GITHUB_TOKEN_FILE", true),
            ("CLAUDE_CODE_OAUTH_TOKEN_FILE", false),
        ] {
            let (rig, script) = capturing_rig(&format!("secrets-mode-{name}-{}", variable.len()));
            let file = secret_file(&rig.scratch, "secret", "a-secret", mode);

            let ran = run(rig, &script, deliver, &[(variable, &file)]);

            assert_eq!(ran.code(), 1, "{variable} {name}: {}", ran.stderr());
            assert!(
                ran.stderr().contains("chmod 600"),
                "{variable} {name}: {}",
                ran.stderr()
            );
            assert!(!ran.stderr().contains("a-secret"), "the secret was printed");
            assert_eq!(ran.output.stdout, b"");
            assert!(
                !ran.scratch.path("work").exists(),
                "{variable} {name}: a workspace was created"
            );
        }
    }
}

#[test]
fn a_variable_and_a_file_for_the_same_secret_are_refused() {
    let (rig, script) = capturing_rig("secrets-both");
    let file = secret_file(&rig.scratch, "github", GITHUB, 0o600);

    let ran = run(
        rig,
        &script,
        true,
        &[
            ("HEPH_GITHUB_TOKEN", "from-the-variable"),
            ("HEPH_GITHUB_TOKEN_FILE", &file),
        ],
    );

    assert_eq!(ran.code(), 1, "{}", ran.stderr());
    assert!(
        ran.stderr().contains("both set"),
        "it must say they are both set, not that one is missing: {}",
        ran.stderr()
    );
    assert!(!ran.scratch.path("work").exists());
}

#[test]
fn a_missing_empty_or_non_regular_secret_file_is_refused() {
    for kind in ["missing", "empty", "blank", "directory"] {
        let (rig, script) = capturing_rig(&format!("secrets-bad-{kind}"));
        let path: String = match kind {
            "missing" => rig.scratch.path("secrets/nowhere").display().to_string(),
            "empty" => secret_file(&rig.scratch, "empty", "", 0o600),
            "blank" => secret_file(&rig.scratch, "blank", "  \n\n", 0o600),
            _ => {
                let directory: PathBuf = rig.scratch.path("secrets/dir");
                std::fs::create_dir_all(&directory).expect("a directory");
                std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                    .expect("to chmod");
                directory.display().to_string()
            }
        };

        let ran = run(rig, &script, true, &[("HEPH_GITHUB_TOKEN_FILE", &path)]);

        assert_eq!(ran.code(), 1, "{kind}: {}", ran.stderr());
        assert!(
            ran.stderr().contains("HEPH_GITHUB_TOKEN_FILE"),
            "{kind}: {}",
            ran.stderr()
        );
        assert!(
            !ran.scratch.path("work").exists(),
            "{kind}: a workspace was created"
        );
    }
}
