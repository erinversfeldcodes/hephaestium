//! `heph baseline apply` as a person or a workflow meets it.
//!
//! Each test builds a small repository in a scratch directory, runs the real
//! binary against it, and reads what it wrote. Every result is also held to the
//! pinned Athenaeum gate result schema.

mod common;

use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Output};

use athenaeum::baseline::TEMPLATES;
use common::{Scratch, conforms, heph_in, stderr, stdout};
use serde_json::Value;

const ANSWERS: &str = "schema_version = 0\n\
    licence = \"source-available\"\n\
    holder = \"Test Holder\"\n\
    contact = \"https://example.org/security-contact\"\n\
    required_checks = [\"ci\"]\n\
    default_branch_bypass = \"admin-pull-request\"\n";

/// Every file the pack writes for a repository that has none of them.
const WRITTEN: [&str; 10] = [
    "README.md",
    "LICENSE",
    "SECURITY.md",
    "CONTRIBUTING.md",
    "CODE_OF_CONDUCT.md",
    ".github/ISSUE_TEMPLATE/bug.yml",
    ".github/ISSUE_TEMPLATE/idea.yml",
    ".github/ISSUE_TEMPLATE/config.yml",
    ".github/pull_request_template.md",
    ".github/CODEOWNERS",
];

/// A repository with its answers and a `dependabot.yml`, which the pack has no
/// template for, and none of the files the pack writes.
fn bare(name: &str) -> Scratch {
    let scratch = Scratch::new(name);
    scratch.write(".athenaeum/baseline.toml", ANSWERS);
    scratch.write(".github/dependabot.yml", "version: 2\nupdates: []\n");
    scratch
}

fn apply(scratch: &Scratch, extra: &[&str]) -> (Output, Value) {
    let mut args = vec!["baseline", "apply", "--repo", "owner/name"];
    args.extend_from_slice(extra);
    let output = heph_in(&scratch.0, &args);
    if output.status.code() == Some(3) || output.status.code() == Some(2) {
        return (output, Value::Null);
    }
    let text = stdout(&output);
    let result: Value = serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!(
            "stdout should be one line of JSON ({error}): {text:?}\nstderr: {}",
            stderr(&output)
        )
    });
    conforms(&result);
    (output, result)
}

fn written(result: &Value) -> Vec<String> {
    let mut found: Vec<String> = result["report"]["written"]
        .as_array()
        .expect("written")
        .iter()
        .map(|entry| {
            entry["destination"]
                .as_str()
                .expect("a destination")
                .to_owned()
        })
        .collect();
    found.sort();
    found
}

fn expected(files: &[&str]) -> Vec<String> {
    let mut list: Vec<String> = files.iter().map(|file| (*file).to_owned()).collect();
    list.sort();
    list
}

fn year() -> String {
    let output = Command::new("date")
        .args(["-u", "+%Y"])
        .output()
        .expect("date to run");
    String::from_utf8(output.stdout)
        .expect("UTF-8")
        .trim()
        .to_owned()
}

fn tree(root: &Path) -> Vec<String> {
    fn walk(directory: &Path, root: &Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(directory).expect("a readable directory") {
            let path = entry.expect("an entry").path();
            found.push(
                path.strip_prefix(root)
                    .expect("under root")
                    .display()
                    .to_string(),
            );
            if path.is_dir() && !path.is_symlink() {
                walk(&path, root, found);
            }
        }
    }
    let mut found = Vec::new();
    walk(root, root, &mut found);
    found.sort();
    found
}

#[test]
fn applying_to_a_bare_repository_writes_every_file_and_each_satisfies_its_item() {
    let scratch = bare("ba-bare");

    let (output, result) = apply(&scratch, &[]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(result["gate"], "baseline-apply");
    assert_eq!(written(&result), expected(&WRITTEN));
    for file in WRITTEN {
        assert!(scratch.exists(file), "{file} was not written");
    }

    // The proof that a template is a good one: the check finds nothing wrong.
    let check = heph_in(&scratch.0, &["baseline", "check"]);
    assert_eq!(check.status.code(), Some(0), "stderr: {}", stderr(&check));
    let report: Value = serde_json::from_str(&stdout(&check)).expect("JSON");
    for item in [
        "community-readme",
        "community-licence",
        "community-security-policy",
        "community-contributing",
        "community-code-of-conduct",
        "community-issue-templates",
        "community-pull-request-template",
        "community-code-owners",
    ] {
        assert_eq!(
            report["report"]["items"][item]["outcome"], "satisfied",
            "{item}: {report}"
        );
    }
}

#[test]
fn the_variables_are_filled_in_and_none_is_left() {
    let scratch = bare("ba-variables");

    apply(&scratch, &[]);

    assert!(
        scratch.read("README.md").starts_with("# name\n"),
        "the repository name"
    );
    let licence = scratch.read("LICENSE");
    assert!(
        licence.starts_with(&format!("Copyright (c) {} Test Holder.", year())),
        "the year and the holder: {}",
        licence.lines().next().unwrap_or_default()
    );
    assert_eq!(scratch.read(".github/CODEOWNERS"), "* @owner\n");
    assert!(
        scratch
            .read("SECURITY.md")
            .contains("https://example.org/security-contact"),
        "the contact"
    );
    assert!(
        scratch
            .read(".github/ISSUE_TEMPLATE/config.yml")
            .contains("https://github.com/owner/name/security/advisories/new")
    );
    for file in WRITTEN {
        let text = scratch.read(file);
        assert!(
            !text.contains("{{") && !text.contains("}}"),
            "{file} still has a variable in it"
        );
    }
}

#[test]
fn a_rendered_file_is_its_template_with_the_variables_replaced() {
    let scratch = bare("ba-faithful");
    apply(&scratch, &[]);

    let template = TEMPLATES
        .iter()
        .find(|t| t.path == "licence/source-available/LICENSE")
        .expect("the licence template");
    let expected = template
        .text
        .replace("{{year}}", &year())
        .replace("{{holder}}", "Test Holder")
        .replace("{{owner}}", "owner")
        .replace("{{repository}}", "name");

    assert_eq!(scratch.read("LICENSE"), expected);
}

#[test]
fn a_destination_that_already_exists_is_never_written_over() {
    // A directory where a file belongs: the item is unsatisfied, since a
    // directory is not a file, and the destination exists.
    let scratch = bare("ba-exists");
    std::fs::create_dir_all(scratch.0.join(".github/CODEOWNERS")).expect("a directory");

    let (output, result) = apply(&scratch, &[]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(
        scratch.0.join(".github/CODEOWNERS").is_dir(),
        "the directory was replaced"
    );
    let skipped = result["report"]["skipped"].as_array().expect("skipped");
    assert!(
        skipped
            .iter()
            .any(|entry| entry["destination"] == ".github/CODEOWNERS"),
        "{result}"
    );
    assert!(!written(&result).contains(&".github/CODEOWNERS".to_owned()));
}

#[test]
fn a_link_in_the_way_is_never_written_through() {
    let scratch = bare("ba-link");
    let elsewhere = Scratch::new("ba-link-target");
    elsewhere.write("precious.txt", "do not touch\n");
    symlink(
        elsewhere.0.join("precious.txt"),
        scratch.0.join("README.md"),
    )
    .expect("a link");

    let (_, result) = apply(&scratch, &[]);

    assert_eq!(elsewhere.read("precious.txt"), "do not touch\n");
    assert!(scratch.is_present("README.md"));
    assert!(
        !written(&result).contains(&"README.md".to_owned()),
        "{result}"
    );
}

#[test]
fn a_path_through_a_link_that_leaves_the_repository_is_refused() {
    let scratch = bare("ba-escape");
    let outside = Scratch::new("ba-escape-outside");
    std::fs::remove_dir_all(scratch.0.join(".github")).expect("to remove .github");
    symlink(&outside.0, scratch.0.join(".github")).expect("a link");

    let output = heph_in(&scratch.0, &["baseline", "apply", "--repo", "owner/name"]);

    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    assert_eq!(
        tree(&outside.0),
        Vec::<String>::new(),
        "something was written outside"
    );
    assert!(
        !scratch.is_present("README.md"),
        "nothing is written when it cannot all be"
    );
}

#[test]
fn items_that_are_satisfied_excepted_or_without_a_template_write_nothing() {
    let scratch = bare("ba-nothing");
    scratch.write("README.md", "mine\n");
    scratch.write(
        ".athenaeum/baseline.toml",
        &format!(
            "{ANSWERS}\n[exceptions.community-security-policy]\nreason = \"Reported by email.\"\n"
        ),
    );

    let (_, result) = apply(&scratch, &[]);

    assert_eq!(
        scratch.read("README.md"),
        "mine\n",
        "a satisfied item is left alone"
    );
    assert!(
        !scratch.exists("SECURITY.md"),
        "an excepted item is not written"
    );
    assert!(
        !scratch
            .0
            .join(".github/dependabot.yml")
            .to_string_lossy()
            .is_empty()
            && scratch.read(".github/dependabot.yml") == "version: 2\nupdates: []\n",
        "an item with no template is left alone"
    );
    assert!(!written(&result).contains(&"README.md".to_owned()));
}

#[test]
fn a_required_item_that_cannot_be_written_is_reported_and_fails() {
    let scratch = bare("ba-remaining");
    std::fs::remove_file(scratch.0.join(".github/dependabot.yml")).expect("to remove");

    let (output, result) = apply(&scratch, &[]);

    assert_eq!(output.status.code(), Some(1), "stderr: {}", stderr(&output));
    assert_eq!(result["outcome"], "failed");
    let remaining = result["report"]["remaining"].as_array().expect("remaining");
    assert!(
        remaining
            .iter()
            .any(|entry| entry["item"] == "dependencies-dependabot-config"),
        "{result}"
    );
}

#[test]
fn a_second_run_writes_nothing() {
    let scratch = bare("ba-idempotent");
    apply(&scratch, &[]);
    let before = tree(&scratch.0);

    let (output, result) = apply(&scratch, &[]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(written(&result), Vec::<String>::new());
    assert_eq!(tree(&scratch.0), before);
}

#[test]
fn nothing_is_written_when_a_value_is_missing_or_a_licence_has_no_templates() {
    let without_holder = ANSWERS.replace("holder = \"Test Holder\"\n", "");
    let unknown_licence = ANSWERS.replace("source-available", "mit");
    for (label, answers, mentions) in [
        ("no holder", without_holder.as_str(), "holder"),
        ("unknown licence", unknown_licence.as_str(), "mit"),
    ] {
        let scratch = bare("ba-atomic");
        scratch.write(".athenaeum/baseline.toml", answers);
        let before = tree(&scratch.0);

        let output = heph_in(&scratch.0, &["baseline", "apply", "--repo", "owner/name"]);

        assert_eq!(
            output.status.code(),
            Some(3),
            "{label}: stderr: {}",
            stderr(&output)
        );
        assert_eq!(stdout(&output), "", "{label}");
        assert!(
            stderr(&output).contains(mentions),
            "{label}: stderr: {}",
            stderr(&output)
        );
        assert_eq!(tree(&scratch.0), before, "{label}: something was written");
    }
}

#[test]
fn without_an_answers_file_it_could_not_be_applied() {
    let scratch = bare("ba-no-answers");
    std::fs::remove_file(scratch.0.join(".athenaeum/baseline.toml")).expect("to remove");
    let before = tree(&scratch.0);

    let output = heph_in(&scratch.0, &["baseline", "apply", "--repo", "owner/name"]);

    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    assert!(
        stderr(&output).contains("answers"),
        "stderr: {}",
        stderr(&output)
    );
    assert_eq!(tree(&scratch.0), before);
}

#[test]
fn the_repository_is_taken_from_the_origin_remote_or_it_could_not_be_applied() {
    let scratch = bare("ba-remote");
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(&scratch.0)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("git");
        assert!(output.status.success());
    };
    git(&["init", "-q"]);

    let without = heph_in(&scratch.0, &["baseline", "apply"]);
    assert_eq!(
        without.status.code(),
        Some(3),
        "no remote: {}",
        stderr(&without)
    );
    assert!(!scratch.exists("README.md"));

    git(&[
        "remote",
        "add",
        "origin",
        "git@github.com:someone/thing.git",
    ]);
    let with = heph_in(&scratch.0, &["baseline", "apply"]);

    assert_eq!(with.status.code(), Some(0), "stderr: {}", stderr(&with));
    assert_eq!(scratch.read(".github/CODEOWNERS"), "* @someone\n");
    assert!(scratch.read("README.md").starts_with("# thing\n"));
}

#[test]
fn a_dry_run_writes_nothing_and_says_what_it_would() {
    let scratch = bare("ba-dry");
    let before = tree(&scratch.0);

    let (output, result) = apply(&scratch, &["--dry-run"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(tree(&scratch.0), before, "a dry run wrote something");
    assert_eq!(result["report"]["dry_run"], true);
    assert_eq!(written(&result), expected(&WRITTEN));
}

#[test]
fn root_chooses_the_repository_and_an_unknown_flag_is_a_usage_error() {
    let scratch = bare("ba-root");
    let elsewhere = Scratch::new("ba-root-cwd");

    let output = heph_in(
        &elsewhere.0,
        &[
            "baseline",
            "apply",
            "--repo",
            "owner/name",
            "--root",
            &scratch.0.to_string_lossy(),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(scratch.exists("README.md"));
    assert!(!elsewhere.exists("README.md"));

    let usage = heph_in(&scratch.0, &["baseline", "apply", "--nope"]);
    assert_eq!(usage.status.code(), Some(2));
}
