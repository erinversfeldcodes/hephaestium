//! What the command-line tests share: a directory of their own to run in, and
//! the binary run from it.

#![allow(
    dead_code,
    reason = "each test binary uses a different part of what the tests share"
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
