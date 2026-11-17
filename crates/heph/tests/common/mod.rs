//! What the command-line tests share: a directory of their own to run in, and
//! the binary run from it.

#![allow(
    dead_code,
    reason = "each test binary uses a different part of what the tests share"
)]

pub mod github;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use athenaeum::gate_result::SCHEMA;
use serde_json::Value;

/// A directory of one test's own, removed when the test ends.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let directory = std::env::temp_dir().join(format!("heph-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a scratch directory");
        Self(directory)
    }

    pub fn write(&self, relative: &str, text: &str) -> PathBuf {
        let file = self.0.join(relative);
        std::fs::create_dir_all(file.parent().expect("a parent directory"))
            .expect("the file's directory");
        std::fs::write(&file, text).expect("the file to be written");
        file
    }

    pub fn exists(&self, relative: &str) -> bool {
        self.0.join(relative).exists()
    }

    /// Whether anything is at the path, including a link that points nowhere.
    pub fn is_present(&self, relative: &str) -> bool {
        self.0.join(relative).symlink_metadata().is_ok()
    }

    pub fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.0.join(relative))
            .unwrap_or_else(|error| panic!("{relative} should be readable: {error}"))
    }

    /// Where a symbolic link points, exactly as written.
    pub fn link_target(&self, relative: &str) -> String {
        std::fs::read_link(self.0.join(relative))
            .unwrap_or_else(|error| panic!("{relative} should be a symbolic link: {error}"))
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn heph_in(directory: &Path, args: &[&str]) -> Output {
    heph_with_env(directory, args, &[])
}

pub fn heph_with_env(directory: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_heph"))
        .current_dir(directory)
        .args(args)
        .envs(env.iter().copied())
        .output()
        .expect("the heph binary to run")
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout to be UTF-8")
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr to be UTF-8")
}

/// Hold a result to the gate result schema of the Athenaeum tag this tool
/// pins, so that a result the tool writes is one the contract accepts.
pub fn conforms(result: &Value) {
    let schema: Value = serde_json::from_str(SCHEMA).expect("the schema to be JSON");
    let validator = jsonschema::validator_for(&schema).expect("the schema to compile");
    let problems: Vec<String> = validator
        .iter_errors(result)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect();
    assert!(
        problems.is_empty(),
        "the result does not conform to the gate result schema:\n{}\nresult: {result}",
        problems.join("\n")
    );
}

/// A clean workflow: every action pinned, permissions declared, a timeout.
pub const CLEAN_WORKFLOW: &str = "name: ci
on: [push]
permissions:
  contents: read
jobs:
  test:
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@v4
      - run: echo ok
";

/// A repository that satisfies every item a working tree can answer.
pub fn compliant(name: &str) -> Scratch {
    let scratch = Scratch::new(name);
    for file in [
        "README.md",
        "LICENSE",
        "SECURITY.md",
        "CONTRIBUTING.md",
        "CODE_OF_CONDUCT.md",
        ".github/ISSUE_TEMPLATE/bug.yml",
        ".github/pull_request_template.md",
        ".github/CODEOWNERS",
    ] {
        scratch.write(file, "x\n");
    }
    scratch.write(".github/dependabot.yml", COVERS_ACTIONS);
    scratch.write(".github/workflows/ci.yml", CLEAN_WORKFLOW);
    scratch
}

/// A `dependabot.yml` that covers GitHub Actions, the only ecosystem a
/// repository with just a workflow uses.
pub const COVERS_ACTIONS: &str = "version: 2
updates:
  - package-ecosystem: github-actions
    directory: /
    schedule:
      interval: weekly
";
