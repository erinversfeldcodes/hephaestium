//! The Hephaestium command-line tool.
//!
//! The version it reports is not read from the manifest of this crate: it is
//! compiled in from the environment when a release is built, so a released
//! binary names the tag it was built for and every other build says plainly
//! that it is a development build.

use std::process::ExitCode;

use heph::gate::{self, CouldNotRun};
use heph::manifest::{self, Invalid};

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
Usage: heph <COMMAND>
       heph [OPTION]

Commands:
  manifest check [PATH]  Check a manifest against the schema
                         [default: .athenaeum/manifest.toml]
  gate run NAME [--manifest PATH] [--changed PATH]...
                         Run a gate from the manifest and print its result as
                         one line of JSON. Run it from the repository root.
                         With --changed, a gate whose when_paths match none of
                         the files is skipped. Exits 0 if the gate passed or
                         was skipped, 1 if it failed or timed out, and 3 if it
                         could not be run.

Options:
  -V, --version  Print the version and exit
  -h, --help     Print this help and exit
";

/// The command ran and the answer was no: the manifest is not valid, or
/// could not be read.
const FAILED: u8 = 1;

/// The command line could not be understood, as distinct from a command that
/// ran and failed.
const USAGE_ERROR: u8 = 2;

/// A gate could not be run at all: the manifest is missing or invalid, the
/// gate is not declared, or its report could not be used. Distinct from a gate
/// that ran and failed, which is [`FAILED`].
const COULD_NOT_RUN: u8 = 3;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();

    match arguments.as_slice() {
        ["-V" | "--version"] => {
            println!("heph {VERSION}");
            ExitCode::SUCCESS
        }
        ["-h" | "--help"] => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        ["gate", "run", rest @ ..] => {
            GateRun::parse(rest).map_or_else(gate_usage_error, |request| run_gate(&request))
        }
        ["gate", ..] => gate_usage_error(),
        ["manifest", "check"] => check_manifest(manifest::DEFAULT_PATH),
        ["manifest", "check", path] => check_manifest(path),
        ["manifest", ..] => {
            eprintln!("heph: expected 'manifest check [PATH]'\n\n{USAGE}");
            ExitCode::from(USAGE_ERROR)
        }
        [unknown, ..] => {
            eprintln!("heph: unrecognised argument '{unknown}'\n\n{USAGE}");
            ExitCode::from(USAGE_ERROR)
        }
        [] => {
            eprint!("{USAGE}");
            ExitCode::from(USAGE_ERROR)
        }
    }
}

/// Check the manifest at `path`, reporting each problem on its own line,
/// prefixed with the file so the output can be read out of context.
fn check_manifest(path: &str) -> ExitCode {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("{path}: cannot be read: {error}");
            return ExitCode::from(FAILED);
        }
    };

    match manifest::check(&text) {
        Ok(()) => {
            println!("{path}: valid");
            ExitCode::SUCCESS
        }
        Err(Invalid::NotToml(reason)) => {
            eprintln!("{path}: is not TOML: {reason}");
            ExitCode::from(FAILED)
        }
        Err(Invalid::Violations(violations)) => {
            for violation in &violations {
                eprintln!("{path}: {}: {}", violation.path, violation.message);
            }
            ExitCode::from(FAILED)
        }
    }
}

/// The command line after `gate` could not be understood.
fn gate_usage_error() -> ExitCode {
    eprintln!("heph: expected 'gate run NAME [--manifest PATH] [--changed PATH]...'\n\n{USAGE}");
    ExitCode::from(USAGE_ERROR)
}

/// What `gate run` was asked to do.
struct GateRun {
    name: String,
    manifest: String,
    changed: Vec<String>,
}

impl GateRun {
    /// Read the arguments after `gate run`, or `None` if they are not a gate
    /// name followed by any of the two options.
    fn parse(arguments: &[&str]) -> Option<Self> {
        let mut name = None;
        let mut manifest = manifest::DEFAULT_PATH.to_owned();
        let mut changed = Vec::new();

        let mut remaining = arguments.iter();
        while let Some(argument) = remaining.next() {
            match *argument {
                "--manifest" => (*remaining.next()?).clone_into(&mut manifest),
                "--changed" => changed.push((*remaining.next()?).to_owned()),
                flag if flag.starts_with('-') => return None,
                positional if name.is_none() => name = Some(positional.to_owned()),
                _ => return None,
            }
        }

        Some(Self {
            name: name?,
            manifest,
            changed,
        })
    }
}

/// Run one gate and print its result on one line of standard output.
fn run_gate(request: &GateRun) -> ExitCode {
    let could_not_run = |message: String| {
        eprintln!("heph: {message}");
        ExitCode::from(COULD_NOT_RUN)
    };

    let text = match std::fs::read_to_string(&request.manifest) {
        Ok(text) => text,
        Err(error) => {
            return could_not_run(format!("{}: cannot be read: {error}", request.manifest));
        }
    };
    let manifest = match manifest::parse(&text) {
        Ok(manifest) => manifest,
        Err(Invalid::NotToml(reason)) => {
            return could_not_run(format!("{}: is not TOML: {reason}", request.manifest));
        }
        Err(Invalid::Violations(violations)) => {
            for violation in &violations {
                eprintln!(
                    "{}: {}: {}",
                    request.manifest, violation.path, violation.message
                );
            }
            return could_not_run(format!("{}: is not a valid manifest", request.manifest));
        }
    };

    let Some(gate) = manifest.gates.get(&request.name) else {
        let declared: Vec<&str> = manifest.gates.keys().map(String::as_str).collect();
        return could_not_run(format!(
            "{}: has no gate '{}'; it declares: {}",
            request.manifest,
            request.name,
            if declared.is_empty() {
                "none".to_owned()
            } else {
                declared.join(", ")
            }
        ));
    };

    let command = gate::command_for(&manifest, gate, |name| std::env::var(name).ok());

    let result = match gate::applies(gate, &request.changed) {
        Ok(true) => gate::run(&request.name, gate, command, |message| {
            eprintln!("heph: {message}");
        }),
        Ok(false) => Ok(gate::skipped(&request.name, gate, command)),
        Err(error) => Err(error),
    };

    match result {
        Ok(result) => {
            let line = serde_json::to_string(&result).expect("a gate result to serialise");
            println!("{line}");
            eprintln!(
                "heph: gate {} {} in {:.1}s",
                result.gate,
                result.outcome,
                f64::from(u32::try_from(result.duration_ms).unwrap_or(u32::MAX)) / 1000.0
            );
            ExitCode::from(result.exit_code_of_heph())
        }
        Err(
            error @ (CouldNotRun::BadGlob(_)
            | CouldNotRun::Wait(_)
            | CouldNotRun::UnusableReport(_)),
        ) => could_not_run(format!("gate {}: {error}", request.name)),
    }
}
