//! The Hephaestium command-line tool.
//!
//! The version it reports is not read from the manifest of this crate: it is
//! compiled in from the environment when a release is built, so a released
//! binary names the tag it was built for and every other build says plainly
//! that it is a development build.

use std::process::ExitCode;

use heph::gate;
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

  lint [--manifest PATH] [--changed PATH]... [--timeout-seconds N]
                         Run the lint commands of the components a change
                         touches, each from its component's root, and print
                         each result as one line of JSON. Every command runs
                         even if an earlier one fails. Each is stopped after
                         900 seconds unless --timeout-seconds says otherwise.
                         Exits 0 if all passed, 1 if any failed or timed out,
                         and 3 if linting could not be run.

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
        ["lint", rest @ ..] => {
            Lint::parse(rest).map_or_else(lint_usage_error, |request| run_lint(&request))
        }
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
    let manifest = match load_manifest(&request.manifest) {
        Ok(manifest) => manifest,
        Err(code) => return code,
    };

    let Some(gate) = manifest.gates.get(&request.name) else {
        let declared: Vec<&str> = manifest.gates.keys().map(String::as_str).collect();
        return could_not_run(&format!(
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
        Ok(true) => gate::run(
            &request.name,
            gate,
            command,
            std::path::Path::new("."),
            |message| {
                eprintln!("heph: {message}");
            },
        ),
        Ok(false) => Ok(gate::skipped(&request.name, gate, command)),
        Err(error) => Err(error),
    };

    match result {
        Ok(result) => {
            print_result(&result);
            ExitCode::from(result.exit_code_of_heph())
        }
        Err(error) => could_not_run(&format!("gate {}: {error}", request.name)),
    }
}

/// The command line after `lint` could not be understood.
fn lint_usage_error() -> ExitCode {
    eprintln!(
        "heph: expected 'lint [--manifest PATH] [--changed PATH]... [--timeout-seconds N]'\n\n{USAGE}"
    );
    ExitCode::from(USAGE_ERROR)
}

/// How long a component's command may run when nothing says otherwise. The
/// manifest gives a gate a timeout and gives a component's commands none, so
/// this is the limit until the schema has somewhere to put one.
const DEFAULT_LINT_TIMEOUT_SECONDS: u64 = 900;

/// What `lint` was asked to do.
struct Lint {
    manifest: String,
    changed: Vec<String>,
    timeout_seconds: u64,
}

impl Lint {
    /// Read the arguments after `lint`, or `None` if they are not any of the
    /// three options with their values.
    fn parse(arguments: &[&str]) -> Option<Self> {
        let mut manifest = manifest::DEFAULT_PATH.to_owned();
        let mut changed = Vec::new();
        let mut timeout_seconds = DEFAULT_LINT_TIMEOUT_SECONDS;

        let mut remaining = arguments.iter();
        while let Some(argument) = remaining.next() {
            match *argument {
                "--manifest" => (*remaining.next()?).clone_into(&mut manifest),
                "--changed" => changed.push((*remaining.next()?).to_owned()),
                "--timeout-seconds" => {
                    timeout_seconds = remaining
                        .next()?
                        .parse()
                        .ok()
                        .filter(|seconds| *seconds > 0)?;
                }
                _ => return None,
            }
        }

        Some(Self {
            manifest,
            changed,
            timeout_seconds,
        })
    }
}

/// Run the lint commands of the components the change touches, and print each
/// result on a line of standard output.
fn run_lint(request: &Lint) -> ExitCode {
    let manifest = match load_manifest(&request.manifest) {
        Ok(manifest) => manifest,
        Err(code) => return code,
    };

    let mut jobs = Vec::new();
    for (name, component) in &manifest.components {
        let touched = request.changed.is_empty()
            || match gate::matches_any(&component.paths, &request.changed) {
                Ok(touched) => touched,
                Err(error) => return could_not_run(&format!("component {name}: {error}")),
            };
        if !touched || component.lint.is_empty() {
            continue;
        }
        if !std::path::Path::new(&component.root).is_dir() {
            return could_not_run(&format!(
                "component {name}: its root '{}' is not a directory",
                component.root
            ));
        }
        for (index, command) in component.lint.iter().enumerate() {
            jobs.push((format!("lint:{name}:{}", index + 1), component, command));
        }
    }

    let mut exit = 0;
    for (name, component, command) in jobs {
        let as_gate = manifest::Gate {
            run: command.clone(),
            when_paths: None,
            timeout_seconds: request.timeout_seconds,
            result: None,
        };
        let wrapped =
            gate::command_for(&manifest, &as_gate, |variable| std::env::var(variable).ok());
        let directory = std::path::Path::new(&component.root);

        match gate::run(&name, &as_gate, wrapped, directory, |message| {
            eprintln!("heph: {message}");
        }) {
            Ok(result) => {
                print_result(&result);
                exit = exit.max(result.exit_code_of_heph());
            }
            Err(error) => return could_not_run(&format!("{name}: {error}")),
        }
    }

    ExitCode::from(exit)
}

/// Say that something could not be run, and end with the code for it.
fn could_not_run(message: &str) -> ExitCode {
    eprintln!("heph: {message}");
    ExitCode::from(COULD_NOT_RUN)
}

/// Print a result as one line of standard output, and a summary of it as one
/// line of standard error.
fn print_result(result: &gate::GateResult) {
    let line = serde_json::to_string(result).expect("a gate result to serialise");
    println!("{line}");
    eprintln!(
        "heph: gate {} {} in {:.1}s",
        result.gate,
        result.outcome,
        f64::from(u32::try_from(result.duration_ms).unwrap_or(u32::MAX)) / 1000.0
    );
}

/// Read and check the manifest, or print why it cannot be used and give the
/// code to end with.
fn load_manifest(path: &str) -> Result<manifest::Manifest, ExitCode> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => return Err(could_not_run(&format!("{path}: cannot be read: {error}"))),
    };
    match manifest::parse(&text) {
        Ok(manifest) => Ok(manifest),
        Err(Invalid::NotToml(reason)) => {
            Err(could_not_run(&format!("{path}: is not TOML: {reason}")))
        }
        Err(Invalid::Violations(violations)) => {
            for violation in &violations {
                eprintln!("{path}: {}: {}", violation.path, violation.message);
            }
            Err(could_not_run(&format!("{path}: is not a valid manifest")))
        }
    }
}
