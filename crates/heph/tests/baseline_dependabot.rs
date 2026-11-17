//! `heph baseline check` and `heph baseline apply` for `dependabot.yml`.
//!
//! Each test builds a small repository in a scratch directory, runs the real
//! binary against it, and reads the result. Every result is also held to the
//! pinned Athenaeum gate result schema.

mod common;

use std::process::Output;

use common::{CLEAN_WORKFLOW, Scratch, compliant, conforms, heph_in, stderr, stdout};
use serde_json::Value;

const ANSWERS: &str = "schema_version = 0\n\
    licence = \"source-available\"\n\
    holder = \"Test Holder\"\n\
    contact = \"https://example.org/security-contact\"\n\
    required_checks = [\"ci\"]\n\
    default_branch_bypass = \"admin-pull-request\"\n";

const COVERAGE: &str = "dependencies-dependabot-coverage";

/// A repository that uses Cargo (a workspace at the root, whose member has no
/// lockfile of its own, and a standalone crate with one), npm (at the root and
/// in `e2e`, and a `scripts` package with no lockfile), Docker and GitHub
/// Actions, and nothing else the baseline names. Its `dependabot.yml` is set by
/// each test.
fn polyglot(name: &str, dependabot: Option<&str>) -> Scratch {
    let scratch = Scratch::new(name);
    scratch.write(".athenaeum/baseline.toml", ANSWERS);
    scratch.write("Cargo.toml", "[workspace]\n");
    scratch.write("Cargo.lock", "# lock\n");
    scratch.write("crates/web/Cargo.toml", "[package]\n");
    scratch.write("tools/x/Cargo.toml", "[package]\n");
    scratch.write("tools/x/Cargo.lock", "# lock\n");
    scratch.write("package.json", "{}\n");
    scratch.write("package-lock.json", "{}\n");
    scratch.write("e2e/package.json", "{}\n");
    scratch.write("e2e/package-lock.json", "{}\n");
    scratch.write("scripts/package.json", "{}\n");
    scratch.write("Dockerfile", "FROM scratch\n");
    scratch.write(".github/workflows/ci.yml", CLEAN_WORKFLOW);
    if let Some(text) = dependabot {
        scratch.write(".github/dependabot.yml", text);
    }
    scratch
}

fn entry(ecosystem: &str, directory: &str) -> String {
    format!(
        "  - package-ecosystem: {ecosystem}\n    directory: {directory}\n    schedule:\n      interval: weekly\n"
    )
}

fn file(entries: &[(&str, &str)]) -> String {
    let mut text = "version: 2\nupdates:\n".to_owned();
    for (ecosystem, directory) in entries {
        text.push_str(&entry(ecosystem, directory));
    }
    text
}

const ALL: [(&str, &str); 6] = [
    ("cargo", "/"),
    ("cargo", "/tools/x"),
    ("npm", "/"),
    ("npm", "/e2e"),
    ("docker", "/"),
    ("github-actions", "/"),
];

fn run(scratch: &Scratch, args: &[&str]) -> (Output, Value) {
    let output = heph_in(&scratch.0, args);
    if matches!(output.status.code(), Some(2 | 3)) {
        return (output, Value::Null);
    }
    let text = stdout(&output);
    let result: Value = serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!(
            "stdout should be JSON ({error}): {text:?}\nstderr: {}",
            stderr(&output)
        )
    });
    conforms(&result);
    (output, result)
}

fn check(scratch: &Scratch) -> (Output, Value) {
    run(scratch, &["baseline", "check"])
}

fn apply(scratch: &Scratch, extra: &[&str]) -> (Output, Value) {
    let mut args = vec!["baseline", "apply", "--repo", "owner/name"];
    args.extend_from_slice(extra);
    run(scratch, &args)
}

fn outcome<'a>(result: &'a Value, item: &str) -> &'a str {
    result["report"]["items"][item]["outcome"]
        .as_str()
        .unwrap_or_else(|| panic!("no outcome for {item}: {result}"))
}

fn detail(result: &Value, item: &str) -> String {
    result["report"]["items"][item]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn a_dependabot_file_that_covers_every_ecosystem_and_directory_satisfies_the_item() {
    let scratch = polyglot("dp-covered", Some(&file(&ALL)));

    let (_, result) = check(&scratch);

    assert_eq!(outcome(&result, COVERAGE), "satisfied", "{result}");
}

#[test]
fn each_missing_entry_is_named_with_its_directory() {
    let scratch = polyglot("dp-missing", Some(&file(&[("github-actions", "/")])));

    let (output, result) = check(&scratch);

    assert_eq!(output.status.code(), Some(1), "required, so it fails");
    assert_eq!(outcome(&result, COVERAGE), "unsatisfied");
    let message = detail(&result, COVERAGE);
    for missing in ["cargo /", "cargo /tools/x", "npm /", "npm /e2e", "docker /"] {
        assert!(
            message.contains(missing),
            "{missing} is not named: {message}"
        );
    }
    assert!(
        !message.contains("github-actions"),
        "a covered entry is named as missing: {message}"
    );
    for not_an_ecosystem in ["/crates/web", "/scripts"] {
        assert!(
            !message.contains(not_an_ecosystem),
            "{not_an_ecosystem} is not an ecosystem of its own: {message}"
        );
    }
}

#[test]
fn an_entry_for_the_wrong_directory_or_the_wrong_ecosystem_does_not_cover() {
    // npm is covered at the root only, so /e2e is missing; and the cargo entry
    // is for the wrong ecosystem name.
    let scratch = polyglot(
        "dp-wrong",
        Some(&file(&[
            ("npm", "/"),
            ("cargoo", "/"),
            ("cargo", "/tools/x"),
            ("docker", "/"),
            ("github-actions", "/"),
        ])),
    );

    let (_, result) = check(&scratch);

    let message = detail(&result, COVERAGE);
    assert!(message.contains("npm /e2e"), "{message}");
    assert!(message.contains("cargo /"), "{message}");
    assert!(!message.contains("cargo /tools/x"), "{message}");
}

#[test]
fn with_no_dependabot_file_the_item_is_unsatisfied_and_the_entries_are_named() {
    let scratch = polyglot("dp-none", None);

    let (_, result) = check(&scratch);

    assert_eq!(outcome(&result, COVERAGE), "unsatisfied");
    let message = detail(&result, COVERAGE);
    assert!(message.contains("no dependabot.yml"), "{message}");
    assert!(message.contains("npm /e2e"), "{message}");
}

#[test]
fn directories_yaml_extension_trailing_slash_and_patterns_all_cover() {
    let text = "version: 2
updates:
  - package-ecosystem: cargo
    directories: [\"/\", \"/tools/x\"]
    schedule:
      interval: weekly
  - package-ecosystem: npm
    directories:
      - /
      - /e2e/
    schedule:
      interval: weekly
  - package-ecosystem: docker
    directory: \"/*\"
    schedule:
      interval: weekly
  - package-ecosystem: github-actions
    directory: /
    schedule:
      interval: weekly
";
    let scratch = polyglot("dp-forms", None);
    scratch.write(".github/dependabot.yaml", text);

    let (_, result) = check(&scratch);

    assert_eq!(outcome(&result, COVERAGE), "satisfied", "{result}");
}

#[test]
fn a_manifest_in_a_subdirectory_is_a_subdirectory_entry() {
    let scratch = Scratch::new("dp-nested");
    scratch.write("services/api/deep/Cargo.toml", "[package]\n");
    scratch.write("services/api/deep/Cargo.lock", "# lock\n");
    scratch.write(
        ".github/dependabot.yml",
        &file(&[("cargo", "/services/api/deep")]),
    );

    let (_, covered) = check(&scratch);
    assert_eq!(outcome(&covered, COVERAGE), "satisfied", "{covered}");

    scratch.write(".github/dependabot.yml", &file(&[("cargo", "/")]));
    let (_, root_only) = check(&scratch);
    assert_eq!(outcome(&root_only, COVERAGE), "unsatisfied", "{root_only}");
    assert!(detail(&root_only, COVERAGE).contains("cargo /services/api/deep"));
}

#[test]
fn manifests_inside_directories_a_build_leaves_are_not_ecosystems() {
    let scratch = Scratch::new("dp-skipped");
    scratch.write("node_modules/pkg/package-lock.json", "{}\n");
    scratch.write("target/debug/build/x/Cargo.lock", "# lock\n");
    scratch.write(".git/hooks/Dockerfile", "FROM scratch\n");

    let (_, result) = check(&scratch);

    assert_eq!(outcome(&result, COVERAGE), "not_applicable", "{result}");
}

#[test]
fn a_workspace_member_or_a_package_without_a_lockfile_is_not_an_ecosystem_of_its_own() {
    let scratch = Scratch::new("dp-workspace");
    scratch.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\n",
    );
    scratch.write("Cargo.lock", "# lock\n");
    scratch.write("crates/a/Cargo.toml", "[package]\n");
    scratch.write("crates/b/Cargo.toml", "[package]\n");
    scratch.write("tools/package.json", "{}\n");
    scratch.write(".github/dependabot.yml", &file(&[("cargo", "/")]));

    let (_, result) = check(&scratch);

    assert_eq!(outcome(&result, COVERAGE), "satisfied", "{result}");
}

#[test]
fn a_repository_with_only_its_root_manifest_is_still_an_ecosystem() {
    let scratch = Scratch::new("dp-root-manifest");
    scratch.write("Cargo.toml", "[package]\n");
    scratch.write("package.json", "{}\n");

    let (_, result) = check(&scratch);

    assert_eq!(outcome(&result, COVERAGE), "unsatisfied", "{result}");
    let message = detail(&result, COVERAGE);
    assert!(
        message.contains("cargo /") && message.contains("npm /"),
        "{message}"
    );
}

#[test]
fn a_repository_that_uses_no_named_ecosystem_has_nothing_to_cover() {
    let scratch = Scratch::new("dp-none-used");
    scratch.write("README.md", "x\n");

    let (_, result) = check(&scratch);

    assert_eq!(outcome(&result, COVERAGE), "not_applicable");
}

#[test]
fn a_dependabot_file_that_is_not_yaml_is_an_error_and_names_the_file() {
    let scratch = polyglot("dp-bad-yaml", Some("updates: [unclosed\n  - : :\n"));

    let output = heph_in(&scratch.0, &["baseline", "check"]);

    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    assert!(
        stderr(&output).contains("dependabot.yml"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn applying_to_a_repository_with_no_dependabot_file_writes_one_that_satisfies_both_items() {
    let scratch = polyglot("dp-apply", None);
    // The pack's other files, so that only the dependabot items are in question.
    let (output, result) = apply(&scratch, &[]);

    assert!(
        result["report"]["written"]
            .as_array()
            .expect("written")
            .iter()
            .any(|w| w["destination"] == ".github/dependabot.yml"
                && w["item"] == "dependencies-dependabot-config"),
        "{result}\nstderr: {}",
        stderr(&output)
    );
    let (_, after) = check(&scratch);
    assert_eq!(outcome(&after, COVERAGE), "satisfied", "{after}");
    assert_eq!(
        outcome(&after, "dependencies-dependabot-config"),
        "satisfied"
    );
}

#[test]
fn the_generated_file_has_one_entry_for_each_ecosystem_and_directory_in_order() {
    let scratch = polyglot("dp-order", None);

    apply(&scratch, &[]);

    let text = scratch.read(".github/dependabot.yml");
    assert!(text.starts_with("version: 2\n\nupdates:\n"), "{text}");
    let found: Vec<(String, String)> = text
        .lines()
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|pair| pair[0].trim_start().starts_with("- package-ecosystem:"))
        .map(|pair| {
            (
                pair[0]
                    .trim_start()
                    .trim_start_matches("- package-ecosystem: ")
                    .to_owned(),
                pair[1]
                    .trim_start()
                    .trim_start_matches("directory: ")
                    .to_owned(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = ALL
        .iter()
        .map(|(e, d)| ((*e).to_owned(), (*d).to_owned()))
        .collect();
    let mut sorted = expected.clone();
    sorted.sort();
    assert_eq!(found, sorted, "{text}");
    assert!(!text.contains("{{"), "a variable was left in: {text}");
    assert!(
        text.contains("cooldown"),
        "the entry template's settings are in: {text}"
    );
}

#[test]
fn an_existing_dependabot_file_is_never_edited_and_what_it_lacks_is_reported() {
    let existing = file(&[("github-actions", "/")]);
    let scratch = polyglot("dp-existing", Some(&existing));

    let (_, result) = apply(&scratch, &[]);

    assert_eq!(
        scratch.read(".github/dependabot.yml"),
        existing,
        "the file was edited"
    );
    let remaining = result["report"]["remaining"].as_array().expect("remaining");
    let coverage = remaining
        .iter()
        .find(|entry| entry["item"] == COVERAGE)
        .unwrap_or_else(|| panic!("the coverage gap is not reported: {result}"));
    assert!(
        coverage["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("npm /e2e")
    );
}

#[test]
fn with_no_ecosystem_to_watch_nothing_is_generated_and_it_says_why() {
    let scratch = compliant("dp-nothing");
    std::fs::remove_file(scratch.0.join(".github/dependabot.yml")).expect("to remove");
    std::fs::remove_file(scratch.0.join(".github/workflows/ci.yml")).expect("to remove");
    scratch.write(".athenaeum/baseline.toml", ANSWERS);

    let (_, result) = apply(&scratch, &[]);

    assert!(!scratch.exists(".github/dependabot.yml"));
    let skipped = result["report"]["skipped"].as_array().expect("skipped");
    assert!(
        skipped
            .iter()
            .any(|entry| entry["destination"] == ".github/dependabot.yml"
                && entry["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("ecosystem")),
        "{result}"
    );
}

#[test]
fn a_dry_run_lists_the_generated_file_and_writes_nothing() {
    let scratch = polyglot("dp-dry", None);

    let (_, result) = apply(&scratch, &["--dry-run"]);

    assert!(!scratch.exists(".github/dependabot.yml"));
    let leftover: Vec<&str> = result["report"]["remaining"]
        .as_array()
        .expect("remaining")
        .iter()
        .filter_map(|entry| entry["item"].as_str())
        .collect();
    assert!(
        !leftover.contains(&COVERAGE) && !leftover.contains(&"dependencies-dependabot-config"),
        "a dry run should not leave over what the written file would satisfy: {leftover:?}"
    );
    assert!(
        result["report"]["written"]
            .as_array()
            .expect("written")
            .iter()
            .any(|w| w["destination"] == ".github/dependabot.yml"),
        "{result}"
    );
}
