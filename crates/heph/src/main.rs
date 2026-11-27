//! The Hephaestium command-line tool.
//!
//! The version it reports is not read from the manifest of this crate: it is
//! compiled in from the environment when a release is built, so a released
//! binary names the tag it was built for and every other build says plainly
//! that it is a development build.

use std::process::ExitCode;

use heph::apply;
use heph::apply_settings;
use heph::baseline;
use heph::gate;
use heph::github;
use heph::lenses;
use heph::lock;
use heph::manifest::{self, Invalid};
use heph::run;
use heph::sync;
use heph::worker;
use heph::workload;

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
                         could not be run, which includes a variable or secret
                         the gate declares in requires being unset or empty.

  lint [--manifest PATH] [--changed PATH]... [--timeout-seconds N]
                         Run the lint commands of the components a change
                         touches, each from its component's root, and print
                         each result as one line of JSON. Every command runs
                         even if an earlier one fails. Each is stopped after
                         900 seconds unless --timeout-seconds says otherwise.
                         Exits 0 if all passed, 1 if any failed or timed out,
                         and 3 if linting could not be run.

  lenses [--manifest PATH] --changed PATH [--changed PATH]...
                         Say which review lenses a change needs, as one line of
                         JSON: the packs of the components the files belong to,
                         each with the components that named it, then the
                         touched components that name no pack and the files no
                         component owns, which no lens will look at. At least
                         one --changed is required. Exits 0 when it says, 2 on
                         a usage error and 3 if it could not be read.

  run TICKET [--repo OWNER/NAME] [--root PATH] [--model NAME]
             [--timeout-seconds N] [--budget-usd X] [--runs-dir PATH]
             [--executor PATH] [--keep-hours N]
                         Run a ticket: build its workload as `workload` does,
                         run the executor on it with delivery, show what
                         happens as it does and end with the outcome, the cost
                         and the pull request. The run is kept in its own
                         directory outside your project, under --runs-dir
                         [default: $XDG_STATE_HOME/hephaestium/runs, or
                         ~/.local/state/hephaestium/runs]. A run that completed
                         deletes its checkout; one that did not keeps it for
                         --keep-hours [default: HEPH_WORKSPACE_KEEP_HOURS, or
                         72], and the next run deletes it then. The GitHub token is
                         HEPH_GITHUB_TOKEN, or HEPH_GITHUB_TOKEN_FILE, or the
                         file ~/.config/hephaestium/github-token; the Claude
                         Code token is found the same way at claude-token.
                         Exits 0 if the workload completed, 1 if it ended any
                         other way, 2 on a usage error and 3 if it could not
                         be run.

  worker --control-plane URL [--runs-dir PATH] [--executor PATH]
         [--workloads N] [--keep-runs-for DAYS] [--min-free-gb GB]
                         Do the work a hosted control plane gives this machine:
                         ask it for a workload, run the executor on it as `run`
                         does, send the events and the run record back, and ask
                         again, until stopped by an interrupt or a termination,
                         which stops the executor and sends how it ended. It
                         opens every connection and listens on none, and tries
                         again with growing waits when it cannot connect. The
                         device token, which a signed-in person makes in the
                         control plane, is HEPH_WORKER_TOKEN, or
                         HEPH_WORKER_TOKEN_FILE, or the file
                         ~/.config/hephaestium/worker-token (mode 600); it is
                         never an argument, and never sent in anything but the
                         Authorization header. The executor's own secrets stay
                         on this machine, found as `run` finds them. Plain http
                         is accepted only for this machine. --workloads N (or
                         HEPH_WORKER_WORKLOADS) is how many workloads it runs
                         at once, each in a workspace and under a lease of its
                         own: 1 to 64, default 1. Each holds a workspace on
                         disk and a model session. A run's
                         workspace (its checkout and build output, not its
                         record or logs) is removed once the run was last
                         written more than --keep-runs-for days ago [default:
                         HEPH_WORKER_KEEP_RUNS, or 7], unless the control plane
                         still holds a lease on its workload or has a pull
                         request open for it; what was removed and the space
                         freed is logged. Below --min-free-gb gigabytes free
                         under the runs directory [default:
                         HEPH_WORKER_MIN_FREE_GB, or 5] it takes no new claim,
                         says so with the free space and the floor, and goes
                         on when there is room; 0 turns this off. Exits 0 when
                         stopped, 2 on a usage error and 3 if it could not go
                         on.

  workload TICKET [--repo OWNER/NAME] [--root PATH] [--model NAME]
                   [--timeout-seconds N] [--budget-usd X]
                   [--approver LOGIN] [--approved-at TIME]
                         Build the workload for a ticket and print it as JSON,
                         which is what `executor run` takes. The repository is
                         --repo, or the origin of the checkout at --root
                         [default: .]. The run starts from the tip of the
                         default branch on GitHub, not from your checkout, and
                         a checkout at another commit is warned about. The
                         ticket must be an open issue. You are the approver:
                         the signed-in GitHub user, now, unless --approver
                         names a login and --approved-at an RFC 3339 time. Defaults: Claude Code
                         on the Anthropic subscription, model sonnet, 1800
                         seconds, 5 dollars. Nothing is run or changed. Exits 0,
                         2 on a usage error, and 3 if no workload could be
                         built.

  baseline check [--root PATH] [--answers PATH] [--settings [--repo OWNER/NAME]]
                         Check the repository at PATH [default: .] against the
                         baseline: the files and workflow rules every repository
                         must have. Prints a gate result named baseline, and a
                         table of what it found. Items that need GitHub are
                         reported as not checked unless --settings is given,
                         which reads the repository's settings and rulesets
                         from GitHub, read-only, by running gh. The repository
                         is --repo, or the github.com origin remote of PATH. The repository's answers and
                         exceptions are read from --answers [default:
                         .athenaeum/baseline.toml under the root], if it exists.
                         Exits 0 if no required item is unsatisfied, 1 if one
                         is, and 3 if the check could not be made.

  baseline apply [--root PATH] [--answers PATH] [--repo OWNER/NAME] [--dry-run]
                         Write the files the repository is missing from the
                         baseline pack: for each file item that is unsatisfied,
                         not excepted and has a template, the template with the
                         repository's values filled in. A file that exists is
                         never written over, and nothing is written through a
                         link. Nothing is written unless everything can be. The
                         values come from --repo or the github.com origin remote
                         and from the answers file, which is required. Prints a
                         gate result named baseline-apply. Exits 0 if no
                         required item is still unsatisfied, 1 if one is, and 3
                         if nothing could be applied. --dry-run writes nothing.

  baseline apply --settings [--root PATH] [--answers PATH] [--repo OWNER/NAME] [--yes]
                         Change the repository's GitHub settings, and create its
                         default-branch ruleset, so that the baseline's setting
                         and ruleset items are satisfied. This changes GitHub,
                         not the working tree, and only with --yes: without it
                         the requests that would be made are printed and none is.
                         An existing ruleset is never modified. Prints a gate
                         result named baseline-apply-settings. Exits 0 if no
                         request failed, 1 if one did, and 3 if nothing could be
                         applied.

  sync [--lock PATH] [--check | --update [TAG]]
                         Make the skills vendored in .athenaeum/skills match the
                         lock, which pins an Athenaeum release, and link them
                         for the agent runtimes it names. Run it from the
                         repository root. --check changes nothing and exits 1 if
                         anything differs. --update moves the pin to TAG, or to
                         the latest release, and then syncs. Exits 3 if the
                         repository could not be synced, and then it is left
                         as it was.

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
        ["baseline", "check", rest @ ..] => BaselineCheck::parse(rest)
            .map_or_else(baseline_usage_error, |request| run_baseline_check(&request)),
        ["baseline", "apply", rest @ ..] => BaselineApply::parse(rest)
            .map_or_else(baseline_apply_usage_error, |request| {
                run_baseline_apply(&request)
            }),
        ["baseline", ..] => baseline_usage_error(),
        ["workload", rest @ ..] => WorkloadRequest::parse(rest, false)
            .map_or_else(workload_usage_error, |request| run_workload(&request)),
        ["run", rest @ ..] => WorkloadRequest::parse(rest, true)
            .map_or_else(run_usage_error, |request| run_ticket(&request)),
        ["worker", rest @ ..] => {
            Worker::parse(rest).map_or_else(worker_usage_error, |request| run_worker(&request))
        }
        ["sync", rest @ ..] => {
            Sync::parse(rest).map_or_else(sync_usage_error, |request| run_sync(&request))
        }
        ["lint", rest @ ..] => {
            Lint::parse(rest).map_or_else(lint_usage_error, |request| run_lint(&request))
        }
        ["lenses", rest @ ..] => {
            Lenses::parse(rest).map_or_else(lenses_usage_error, |request| run_lenses(&request))
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
    /// The files given with `--changed`, or `None` when none were given and what
    /// changed is not known.
    changed: Option<Vec<String>>,
}

impl GateRun {
    /// Read the arguments after `gate run`, or `None` if they are not a gate
    /// name followed by any of the two options.
    fn parse(arguments: &[&str]) -> Option<Self> {
        let mut name = None;
        let mut manifest = manifest::DEFAULT_PATH.to_owned();
        let mut changed: Option<Vec<String>> = None;

        let mut remaining = arguments.iter();
        while let Some(argument) = remaining.next() {
            match *argument {
                "--manifest" => (*remaining.next()?).clone_into(&mut manifest),
                "--changed" => changed
                    .get_or_insert_with(Vec::new)
                    .push((*remaining.next()?).to_owned()),
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

    let result = gate::run_named(
        &manifest,
        &request.name,
        request.changed.as_deref(),
        std::path::Path::new("."),
        |name| std::env::var(name).ok(),
        |message| {
            eprintln!("heph: {message}");
        },
    );

    match result {
        Ok(result) => {
            print_result(&result);
            ExitCode::from(result.exit_code_of_heph())
        }
        Err(error @ gate::NotRun::NoSuchGate { .. }) => {
            could_not_run(&format!("{}: {error}", request.manifest))
        }
        Err(error) => could_not_run(&format!("gate {}: {error}", request.name)),
    }
}

/// What `baseline check` was asked.
struct BaselineCheck {
    root: String,
    answers: Option<String>,
    settings: bool,
    repo: Option<String>,
}

impl BaselineCheck {
    fn parse(arguments: &[&str]) -> Option<Self> {
        let mut root = ".".to_owned();
        let mut answers = None;
        let mut settings = false;
        let mut repo = None;
        let mut rest = arguments.iter();
        while let Some(&flag) = rest.next() {
            match flag {
                "--root" => (*rest.next()?).clone_into(&mut root),
                "--answers" => answers = Some((*rest.next()?).to_owned()),
                "--repo" => repo = Some((*rest.next()?).to_owned()),
                "--settings" => settings = true,
                _ => return None,
            }
        }
        Some(Self {
            root,
            answers,
            settings,
            repo,
        })
    }
}

/// Check a repository against the baseline and print the result on one line of
/// standard output, and what it found as a table on standard error.
fn run_baseline_check(request: &BaselineCheck) -> ExitCode {
    let root = std::path::Path::new(&request.root);
    let answers = request.answers.as_ref().map_or_else(
        || root.join(baseline::DEFAULT_ANSWERS),
        std::path::PathBuf::from,
    );

    let started = std::time::Instant::now();
    let github = if request.settings {
        let read = github::repository_of(root, request.repo.as_deref())
            .and_then(|repository| github::read(&repository));
        match read {
            Ok(state) => Some(state),
            Err(error) => return could_not_run(&format!("baseline: {error}")),
        }
    } else {
        None
    };
    let checked = match baseline::check(root, &answers, github.as_ref()) {
        Ok(checked) => checked,
        Err(error) => return could_not_run(&format!("baseline: {error}")),
    };
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    let result = gate::reported(
        "baseline",
        checked.failed,
        vec!["heph".to_owned(), "baseline".to_owned(), "check".to_owned()],
        duration_ms,
        checked.report,
    );
    eprint!("{}", checked.table);
    print_result(&result);
    ExitCode::from(result.exit_code_of_heph())
}

/// What `baseline apply` was asked.
struct BaselineApply {
    root: String,
    answers: Option<String>,
    repo: Option<String>,
    dry_run: bool,
    /// Change GitHub, and not the working tree.
    settings: bool,
    /// Make the changes, and not only say what they would be.
    yes: bool,
}

impl BaselineApply {
    fn parse(arguments: &[&str]) -> Option<Self> {
        let mut root = ".".to_owned();
        let mut answers = None;
        let mut repo = None;
        let mut dry_run = false;
        let mut settings = false;
        let mut yes = false;
        let mut rest = arguments.iter();
        while let Some(&flag) = rest.next() {
            match flag {
                "--root" => (*rest.next()?).clone_into(&mut root),
                "--answers" => answers = Some((*rest.next()?).to_owned()),
                "--repo" => repo = Some((*rest.next()?).to_owned()),
                "--dry-run" => dry_run = true,
                "--settings" => settings = true,
                "--yes" => yes = true,
                _ => return None,
            }
        }
        // Changing GitHub is a different thing from writing files, and --yes
        // means the first, so it means nothing without --settings.
        if (yes && !settings) || (settings && dry_run) {
            return None;
        }
        Some(Self {
            root,
            answers,
            repo,
            dry_run,
            settings,
            yes,
        })
    }
}

/// Write the files a repository is missing from the baseline pack, and print
/// what was done as a gate result on one line of standard output, and as a
/// table on standard error.
fn run_baseline_apply(request: &BaselineApply) -> ExitCode {
    if request.settings {
        return run_baseline_apply_settings(request);
    }
    let root = std::path::Path::new(&request.root);
    let answers = request.answers.as_ref().map_or_else(
        || root.join(baseline::DEFAULT_ANSWERS),
        std::path::PathBuf::from,
    );
    let repository = match github::repository_of(root, request.repo.as_deref()) {
        Ok(repository) => repository,
        Err(error) => return could_not_run(&format!("baseline apply: {error}")),
    };

    let started = std::time::Instant::now();
    let applied = match apply::apply(
        root,
        &answers,
        &repository,
        apply::current_year(),
        request.dry_run,
    ) {
        Ok(applied) => applied,
        Err(error) => return could_not_run(&format!("baseline apply: {error}")),
    };
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    let result = gate::reported(
        "baseline-apply",
        applied.failed,
        vec!["heph".to_owned(), "baseline".to_owned(), "apply".to_owned()],
        duration_ms,
        applied.report,
    );
    eprint!("{}", applied.table);
    print_result(&result);
    ExitCode::from(result.exit_code_of_heph())
}

/// Change a repository's GitHub settings, and create its ruleset, if told to,
/// and print what was done, or would be, as a gate result on one line of
/// standard output and a table on standard error.
fn run_baseline_apply_settings(request: &BaselineApply) -> ExitCode {
    let root = std::path::Path::new(&request.root);
    let answers = request.answers.as_ref().map_or_else(
        || root.join(baseline::DEFAULT_ANSWERS),
        std::path::PathBuf::from,
    );
    let repository = match github::repository_of(root, request.repo.as_deref()) {
        Ok(repository) => repository,
        Err(error) => return could_not_run(&format!("baseline apply: {error}")),
    };

    let started = std::time::Instant::now();
    let applied = match apply_settings::apply(root, &answers, &repository, request.yes) {
        Ok(applied) => applied,
        Err(error) => return could_not_run(&format!("baseline apply: {error}")),
    };
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    let result = gate::reported(
        "baseline-apply-settings",
        applied.failed,
        vec![
            "heph".to_owned(),
            "baseline".to_owned(),
            "apply".to_owned(),
            "--settings".to_owned(),
        ],
        duration_ms,
        applied.report,
    );
    eprint!("{}", applied.table);
    print_result(&result);
    ExitCode::from(result.exit_code_of_heph())
}

/// The command line after `baseline apply` could not be understood.
fn baseline_apply_usage_error() -> ExitCode {
    eprintln!(
        "heph: expected 'baseline apply [--root PATH] [--answers PATH] [--repo OWNER/NAME] [--dry-run]' or 'baseline apply --settings [--repo OWNER/NAME] [--yes]'\n\n{USAGE}"
    );
    ExitCode::from(USAGE_ERROR)
}

/// The command line after `baseline` could not be understood.
fn baseline_usage_error() -> ExitCode {
    eprintln!(
        "heph: expected 'baseline check [--root PATH] [--answers PATH] [--settings [--repo OWNER/NAME]]'\n\n{USAGE}"
    );
    ExitCode::from(USAGE_ERROR)
}

/// The command line after `lint` could not be understood.
fn lint_usage_error() -> ExitCode {
    eprintln!(
        "heph: expected 'lint [--manifest PATH] [--changed PATH]... [--timeout-seconds N]'\n\n{USAGE}"
    );
    ExitCode::from(USAGE_ERROR)
}

/// The command line after `lenses` could not be understood.
fn lenses_usage_error() -> ExitCode {
    eprintln!(
        "heph: expected 'lenses [--manifest PATH] --changed PATH [--changed PATH]...'\n\n{USAGE}"
    );
    ExitCode::from(USAGE_ERROR)
}

/// What `lenses` was asked to do.
struct Lenses {
    manifest: String,
    changed: Vec<String>,
}

impl Lenses {
    /// Read the arguments after `lenses`, or `None` if they are not the two
    /// options with their values, or name no changed file: a change with no
    /// files is not a change that needs every lens.
    fn parse(arguments: &[&str]) -> Option<Self> {
        let mut manifest = manifest::DEFAULT_PATH.to_owned();
        let mut changed = Vec::new();
        let mut remaining = arguments.iter();
        while let Some(argument) = remaining.next() {
            match *argument {
                "--manifest" => (*remaining.next()?).clone_into(&mut manifest),
                "--changed" => changed.push((*remaining.next()?).to_owned()),
                _ => return None,
            }
        }
        (!changed.is_empty()).then_some(Self { manifest, changed })
    }
}

/// Say which lenses the change needs, on one line of standard output.
fn run_lenses(request: &Lenses) -> ExitCode {
    let manifest = match load_manifest(&request.manifest) {
        Ok(manifest) => manifest,
        Err(code) => return code,
    };
    match lenses::plan(&manifest, &request.changed) {
        Ok(plan) => {
            println!(
                "{}",
                serde_json::to_string(&plan).expect("a plan to serialise")
            );
            ExitCode::SUCCESS
        }
        Err(error) => could_not_run(&error.to_string()),
    }
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
            requires: std::collections::BTreeMap::new(),
            ci_check: None,
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

/// The command line after `sync` could not be understood.
fn sync_usage_error() -> ExitCode {
    eprintln!("heph: expected 'sync [--lock PATH] [--check | --update [TAG]]'\n\n{USAGE}");
    ExitCode::from(USAGE_ERROR)
}

/// What `sync` was asked to do about the repository.
enum Mode {
    /// Make the repository match the lock.
    Sync,
    /// Change nothing, and say whether the repository matches the lock.
    Check,
    /// Move the pin to a release, the latest if none is named, and then sync.
    Update(Option<String>),
}

/// What `sync` was asked to do.
struct Sync {
    lock: String,
    mode: Mode,
}

impl Sync {
    /// Read the arguments after `sync`, or `None` if they are not the options
    /// with their values, or if `--check` and `--update` are asked for together.
    fn parse(arguments: &[&str]) -> Option<Self> {
        let mut lock = lock::DEFAULT_PATH.to_owned();
        let mut check = false;
        let mut update = false;
        let mut tag = None;

        let mut remaining = arguments.iter().peekable();
        while let Some(argument) = remaining.next() {
            match *argument {
                "--lock" => (*remaining.next()?).clone_into(&mut lock),
                "--check" => check = true,
                "--update" if !update => {
                    update = true;
                    tag = remaining
                        .next_if(|next| !next.starts_with('-'))
                        .map(|tag| (*tag).to_owned());
                }
                _ => return None,
            }
        }

        let mode = match (check, update) {
            (true, true) => return None,
            (true, false) => Mode::Check,
            (false, true) => Mode::Update(tag),
            (false, false) => Mode::Sync,
        };
        Some(Self { lock, mode })
    }
}

/// Sync the repository in the current directory with its lock.
fn run_sync(request: &Sync) -> ExitCode {
    match sync_repository(request) {
        Ok(code) => code,
        Err(message) => could_not_run(&message),
    }
}

/// Everything `sync` does. An error is the message to end with, and the code
/// for a repository that could not be synced.
fn sync_repository(request: &Sync) -> Result<ExitCode, String> {
    let text = std::fs::read_to_string(&request.lock)
        .map_err(|error| format!("{}: cannot be read: {error}", request.lock))?;
    let current = match lock::parse(&text) {
        Ok(current) => current,
        Err(Invalid::NotToml(reason)) => {
            return Err(format!("{}: is not TOML: {reason}", request.lock));
        }
        Err(Invalid::Violations(violations)) => {
            for violation in &violations {
                eprintln!(
                    "{}: {}: {}",
                    request.lock, violation.path, violation.message
                );
            }
            return Err(format!("{}: is not a valid lock", request.lock));
        }
    };

    let root = std::path::Path::new(".");
    let message = |error: sync::SyncError| error.to_string();

    let (wanted, upstream, new_text) = if let Mode::Update(tag) = &request.mode {
        let rev = match tag {
            Some(tag) => tag.clone(),
            None => sync::latest_release(&current.source).map_err(message)?,
        };
        let upstream = sync::fetch(&current.source, &rev).map_err(message)?;
        let new_text = lock::pinned_at(&text, &rev, &upstream.commit);
        let wanted = lock::Lock {
            rev,
            commit: upstream.commit.clone(),
            ..current
        };
        (wanted, upstream, Some(new_text))
    } else {
        let upstream = sync::fetch(&current.source, &current.rev).map_err(message)?;
        sync::verify_pin(&current, &upstream).map_err(message)?;
        (current, upstream, None)
    };

    let differences = sync::differences(&wanted, &upstream, root).map_err(message)?;

    if matches!(request.mode, Mode::Check) {
        if differences.is_empty() {
            println!("up to date");
            return Ok(ExitCode::SUCCESS);
        }
        for difference in &differences {
            eprintln!("heph: {difference}");
        }
        return Ok(ExitCode::from(FAILED));
    }

    let mut changed = false;
    sync::apply(&differences, &upstream, root, |line| {
        changed = true;
        println!("{line}");
    })
    .map_err(message)?;

    if let Some(new_text) = new_text {
        std::fs::write(&request.lock, new_text)
            .map_err(|error| format!("{}: cannot be written: {error}", request.lock))?;
        println!("pinned {} ({})", wanted.rev, wanted.commit);
        changed = true;
    }
    if !changed {
        println!("up to date");
    }
    Ok(ExitCode::SUCCESS)
}

/// What `workload` was asked to do.
struct WorkloadRequest {
    ticket: i64,
    repo: Option<String>,
    root: String,
    model: String,
    timeout_seconds: u64,
    budget_usd: f64,
    /// Only `run` takes these two.
    runs_dir: Option<String>,
    executor: Option<String>,
    keep_hours: Option<u64>,
    /// Only `workload` takes these two.
    approver: Option<String>,
    approved_at: Option<pbjson_types::Timestamp>,
}

impl WorkloadRequest {
    /// Read the arguments after `workload`: a ticket number, with or without a
    /// leading `#`, and any of the options. `None` if they are not that.
    fn parse(arguments: &[&str], for_run: bool) -> Option<Self> {
        let mut ticket = None;
        let mut repo = None;
        let mut runs_dir = None;
        let mut executor = None;
        let mut keep_hours = None;
        let mut approver = None;
        let mut approved_at = None;
        let mut root = ".".to_owned();
        let mut model = workload::DEFAULT_MODEL.to_owned();
        let mut timeout_seconds = workload::DEFAULT_TIMEOUT_SECONDS;
        let mut budget_usd = workload::DEFAULT_BUDGET_USD;

        let mut remaining = arguments.iter();
        while let Some(argument) = remaining.next() {
            match *argument {
                "--repo" => repo = Some((*remaining.next()?).to_owned()),
                "--root" => (*remaining.next()?).clone_into(&mut root),
                "--runs-dir" if for_run => runs_dir = Some((*remaining.next()?).to_owned()),
                "--executor" if for_run => executor = Some((*remaining.next()?).to_owned()),
                "--keep-hours" if for_run => keep_hours = Some(remaining.next()?.parse().ok()?),
                "--approver" if !for_run => approver = Some(login(remaining.next()?)?),
                "--approved-at" if !for_run => approved_at = Some(timestamp(remaining.next()?)?),
                "--model" => (*remaining.next()?).clone_into(&mut model),
                "--timeout-seconds" => {
                    timeout_seconds = remaining.next()?.parse().ok().filter(|n| *n > 0)?;
                }
                "--budget-usd" => {
                    budget_usd = remaining
                        .next()?
                        .parse()
                        .ok()
                        .filter(|amount: &f64| amount.is_finite() && *amount > 0.0)?;
                }
                flag if flag.starts_with("--") => return None,
                positional if ticket.is_none() => {
                    ticket = Some(
                        positional
                            .trim_start_matches('#')
                            .parse::<i64>()
                            .ok()
                            .filter(|n| *n > 0)?,
                    );
                }
                _ => return None,
            }
        }

        Some(Self {
            ticket: ticket?,
            repo,
            root,
            model,
            timeout_seconds,
            budget_usd,
            runs_dir,
            executor,
            keep_hours,
            approver,
            approved_at,
        })
    }
}

/// A GitHub login: letters, digits and single hyphens, not starting or ending with one.
fn login(text: &str) -> Option<String> {
    let plain = !text.is_empty()
        && text.len() <= 39
        && !text.starts_with('-')
        && !text.ends_with('-')
        && !text.contains("--")
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    plain.then(|| text.to_owned())
}

/// A time written as RFC 3339, with its offset.
fn timestamp(text: &str) -> Option<pbjson_types::Timestamp> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
}

fn workload_usage_error() -> ExitCode {
    eprintln!(
        "heph: expected 'workload TICKET [--repo OWNER/NAME] [--root PATH] [--model NAME] [--timeout-seconds N] [--budget-usd X]'\n\n{USAGE}"
    );
    ExitCode::from(USAGE_ERROR)
}

/// Build the workload for a ticket and print it.
fn run_workload(request: &WorkloadRequest) -> ExitCode {
    let root = std::path::Path::new(&request.root);
    let repository = match github::repository_of(root, request.repo.as_deref()) {
        Ok(repository) => repository,
        Err(error) => return could_not_run(&error.to_string()),
    };
    let local_head = workload::head_of(root);
    let built = workload::build(
        &workload::Request {
            repository: &repository,
            ticket: request.ticket,
            model: &request.model,
            timeout_seconds: request.timeout_seconds,
            budget_usd: request.budget_usd,
            approver: request.approver.as_deref(),
            approved_at: request.approved_at,
        },
        local_head.as_deref(),
    );
    match built {
        Ok(built) => {
            for warning in &built.warnings {
                eprintln!("heph: warning: {warning}");
            }
            match serde_json::to_string_pretty(&built.workload) {
                Ok(text) => {
                    println!("{text}");
                    ExitCode::SUCCESS
                }
                Err(error) => could_not_run(&format!("the workload could not be written: {error}")),
            }
        }
        Err(error) => could_not_run(&error.to_string()),
    }
}

fn run_usage_error() -> ExitCode {
    eprintln!(
        "heph: expected 'run TICKET [--repo OWNER/NAME] [--root PATH] [--model NAME] [--timeout-seconds N] [--budget-usd X] [--runs-dir PATH] [--executor PATH] [--keep-hours N]'\n\n{USAGE}"
    );
    ExitCode::from(USAGE_ERROR)
}

/// Run a ticket with the executor and say how it ended.
fn run_ticket(request: &WorkloadRequest) -> ExitCode {
    let root = std::path::Path::new(&request.root);
    let repository = match github::repository_of(root, request.repo.as_deref()) {
        Ok(repository) => repository,
        Err(error) => return could_not_run(&error.to_string()),
    };
    // An interrupt or a termination is passed on to the executor and waited for,
    // and does not end this program before it has said how the run ended.
    if let Err(error) = heph::process::install_cancellation() {
        return could_not_run(&format!("could not install the signal handlers: {error}"));
    }
    let options = run::Options {
        repository,
        ticket: request.ticket,
        model: request.model.clone(),
        timeout_seconds: request.timeout_seconds,
        budget_usd: request.budget_usd,
        runs_dir: request.runs_dir.as_ref().map(std::path::PathBuf::from),
        executor: request.executor.as_ref().map(std::path::PathBuf::from),
        keep_hours: request.keep_hours,
        local_head: workload::head_of(root),
    };
    match run::run(&options) {
        Ok(run::Ended::Completed) => ExitCode::SUCCESS,
        Ok(run::Ended::Otherwise) => ExitCode::from(FAILED),
        Err(message) => could_not_run(&message),
    }
}

/// What `worker` was asked to do.
struct Worker {
    control_plane: String,
    runs_dir: Option<String>,
    executor: Option<String>,
    workloads: Option<String>,
    keep_runs_for: Option<String>,
    min_free_gb: Option<String>,
}

impl Worker {
    /// Read the arguments after `worker`, or `None` if they are not the options with their
    /// values or there is no address.
    fn parse(arguments: &[&str]) -> Option<Self> {
        let mut control_plane = None;
        let mut runs_dir = None;
        let mut executor = None;
        let mut workloads = None;
        let mut keep_runs_for = None;
        let mut min_free_gb = None;
        let mut remaining = arguments.iter();
        while let Some(argument) = remaining.next() {
            let value = (*remaining.next()?).to_owned();
            match *argument {
                "--min-free-gb" => min_free_gb = Some(value),
                "--control-plane" => control_plane = Some(value),
                "--runs-dir" => runs_dir = Some(value),
                "--executor" => executor = Some(value),
                "--workloads" => workloads = Some(value),
                "--keep-runs-for" => keep_runs_for = Some(value),
                _ => return None,
            }
        }
        Some(Self {
            control_plane: control_plane?,
            runs_dir,
            executor,
            workloads,
            keep_runs_for,
            min_free_gb,
        })
    }
}

fn worker_usage_error() -> ExitCode {
    eprintln!(
        "heph: expected 'worker --control-plane URL [--runs-dir PATH] [--executor PATH] [--workloads N] [--keep-runs-for DAYS] [--min-free-gb GB]'\n\n{USAGE}"
    );
    ExitCode::from(USAGE_ERROR)
}

/// Do the work a control plane gives this machine, until told to stop.
fn run_worker(request: &Worker) -> ExitCode {
    let token = match worker::token() {
        Ok(token) => token,
        Err(message) => return could_not_run(&message),
    };
    let workloads = match worker::workloads(request.workloads.as_deref()) {
        Ok(workloads) => workloads,
        Err(message) => return could_not_run(&message),
    };
    let keep_runs_for = match worker::keep_runs_for(request.keep_runs_for.as_deref()) {
        Ok(keep) => keep,
        Err(message) => return could_not_run(&message),
    };
    let min_free = match worker::min_free(request.min_free_gb.as_deref()) {
        Ok(floor) => floor,
        Err(message) => return could_not_run(&message),
    };
    // A stop is passed on to the executor and waited for, and does not end this program
    // before the run has said how it ended.
    if let Err(error) = heph::process::install_cancellation() {
        return could_not_run(&format!("could not install the signal handlers: {error}"));
    }
    let options = worker::Options {
        min_free,
        keep_runs_for,
        control_plane: request.control_plane.clone(),
        runs_dir: request.runs_dir.as_ref().map(std::path::PathBuf::from),
        executor: request.executor.as_ref().map(std::path::PathBuf::from),
        workloads,
    };
    match worker::run(&options, token, &heph::process::cancelled) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => could_not_run(&message),
    }
}
