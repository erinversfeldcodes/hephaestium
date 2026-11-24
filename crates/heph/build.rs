//! Records what this build is made of, so a run can say which build produced it: the commit it
//! was built from and the Athenaeum release it is linked against. The version a binary reports
//! is the workspace's and names no build; a commit does.

use std::path::Path;
use std::process::Command;

fn main() {
    let root =
        Path::new(&std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets the manifest dir"))
            .join("../..");

    println!("cargo:rerun-if-env-changed=HEPHAESTIUM_COMMIT");
    let commit = std::env::var("HEPHAESTIUM_COMMIT")
        .ok()
        .filter(|c| !c.is_empty())
        .or_else(|| git_commit(&root))
        .unwrap_or_default();
    println!("cargo:rustc-env=HEPH_BUILD_COMMIT={commit}");
    watch_head(&root);

    // The lock holds the tag and the commit the tag named when it was resolved, which are the
    // release this build is linked against, so they are read from there and from nowhere else.
    let lock = root.join("Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock.display());
    let (tag, athenaeum_commit) = std::fs::read_to_string(&lock)
        .ok()
        .and_then(|text| athenaeum_source(&text))
        .unwrap_or_default();
    println!("cargo:rustc-env=HEPH_ATHENAEUM_TAG={tag}");
    println!("cargo:rustc-env=HEPH_ATHENAEUM_COMMIT={athenaeum_commit}");
}

fn git_commit(root: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (out.status.success() && text.len() == 40 && text.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| text.to_owned())
}

/// Rebuild when the checked-out commit changes.
fn watch_head(root: &Path) {
    let head = root.join(".git/HEAD");
    let Ok(text) = std::fs::read_to_string(&head) else {
        return;
    };
    println!("cargo:rerun-if-changed={}", head.display());
    if let Some(reference) = text.trim().strip_prefix("ref: ") {
        let reference = root.join(".git").join(reference);
        if reference.exists() {
            println!("cargo:rerun-if-changed={}", reference.display());
        }
    }
}

/// The tag and commit of the `athenaeum` package's source in a lock file's text.
fn athenaeum_source(lock: &str) -> Option<(String, String)> {
    let package = lock
        .split("[[package]]")
        .find(|p| p.lines().any(|l| l.trim() == "name = \"athenaeum\""))?;
    let source = package
        .lines()
        .find_map(|l| l.trim().strip_prefix("source = \""))?
        .trim_end_matches('"');
    let (url, commit) = source.split_once('#')?;
    let tag = url.split_once("?tag=")?.1;
    Some((tag.to_owned(), commit.to_owned()))
}
