//! The Hephaestium command-line tool.
//!
//! It reports its version and nothing else yet. The version is not read from
//! the manifest of this crate: it is compiled in from the environment when a
//! release is built, so a released binary names the tag it was built for and
//! every other build says plainly that it is a development build.

use std::process::ExitCode;

/// What a build reports when no version was compiled in.
const DEVELOPMENT_VERSION: &str = "0.0.0-dev";

/// The version this binary reports.
///
/// The release workflow exports `HEPHAESTIUM_VERSION` before it builds. An
/// empty value counts as absent, because that is what a workflow passes when
/// it has no version to give.
const VERSION: &str = match option_env!("HEPHAESTIUM_VERSION") {
    Some(version) if !version.is_empty() => version,
    _ => DEVELOPMENT_VERSION,
};

const USAGE: &str = "\
Usage: heph [OPTION]

Options:
  -V, --version  Print the version and exit
  -h, --help     Print this help and exit
";

/// The exit code for a command line that could not be understood, as distinct
/// from a command that ran and failed.
const USAGE_ERROR: u8 = 2;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    match arguments.first().map(String::as_str) {
        Some("-V" | "--version") => {
            println!("heph {VERSION}");
            ExitCode::SUCCESS
        }
        Some("-h" | "--help") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(unknown) => {
            eprintln!("heph: unrecognised argument '{unknown}'\n\n{USAGE}");
            ExitCode::from(USAGE_ERROR)
        }
        None => {
            eprint!("{USAGE}");
            ExitCode::from(USAGE_ERROR)
        }
    }
}
