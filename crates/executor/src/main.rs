//! The executor binary. The version it reports is compiled in from the
//! environment when a release is built, as `heph`'s is.

use std::process::ExitCode;

use executor::cli;
use executor::run::{keep_out_of_the_log, run};
use executor::secrets::{self, Secrets};

/// What a build reports when no version was compiled in.
const DEVELOPMENT_VERSION: &str = "0.0.0-dev";

const VERSION: &str = match option_env!("HEPHAESTIUM_VERSION") {
    Some(version) if !version.is_empty() => version,
    _ => DEVELOPMENT_VERSION,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Alone, as `heph` takes it: a release proves each binary it ships reports its version.
    if matches!(args.as_slice(), [flag] if flag == "--version" || flag == "-V") {
        println!("executor {VERSION}");
        return ExitCode::SUCCESS;
    }
    let options = match cli::parse(args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("executor: {message}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    // SAFETY: nothing has started a thread yet.
    let mut secrets = match unsafe { Secrets::take(&secrets::KNOWN) } {
        Ok(secrets) => secrets,
        Err(message) => {
            eprintln!("executor: {message}");
            return ExitCode::from(1);
        }
    };
    // SAFETY: nothing has started a thread yet.
    if let Err(message) = unsafe { secrets.take_gate() } {
        eprintln!("executor: {message}");
        return ExitCode::from(1);
    }
    // From here a signal asks the executor to stop what it is running, and does
    // not end it: the agent and everything else it started would be left behind.
    if let Err(error) = heph::process::install_cancellation() {
        eprintln!("executor: could not install the signal handlers: {error}");
        return ExitCode::from(1);
    }
    if options.deliver && secrets.get(secrets::GITHUB_TOKEN).is_none() {
        eprintln!(
            "executor: --deliver needs {0} or {0}_FILE in the environment",
            secrets::GITHUB_TOKEN
        );
        return ExitCode::from(1);
    }
    keep_out_of_the_log(&secrets);
    match run(&options, VERSION, &secrets, &mut std::io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(refused) => {
            eprintln!(
                "executor: {}",
                executor::run::for_the_log(&refused.to_string())
            );
            ExitCode::from(1)
        }
    }
}
