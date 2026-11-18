//! The executor binary. The version it reports is compiled in from the
//! environment when a release is built, as `heph`'s is.

use std::process::ExitCode;

use executor::cli;
use executor::run::run;

/// What a build reports when no version was compiled in.
const DEVELOPMENT_VERSION: &str = "0.0.0-dev";

const VERSION: &str = match option_env!("HEPHAESTIUM_VERSION") {
    Some(version) if !version.is_empty() => version,
    _ => DEVELOPMENT_VERSION,
};

fn main() -> ExitCode {
    let options = match cli::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("executor: {message}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    let token = if options.deliver {
        match take_token() {
            Ok(token) => Some(token),
            Err(message) => {
                eprintln!("executor: {message}");
                return ExitCode::from(1);
            }
        }
    } else {
        None
    };
    match run(
        &options,
        VERSION,
        token.as_deref(),
        &mut std::io::stdout().lock(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(refused) => {
            eprintln!("executor: {refused}");
            ExitCode::from(1)
        }
    }
}

/// The name of the variable the token for delivery is given in.
const TOKEN_VARIABLE: &str = "HEPH_GITHUB_TOKEN";

/// Read the token for delivery and remove it from this process's environment,
/// so that the bootstrap steps, the gates and the agent, which run the
/// repository's code and the agent's changes to it, never inherit it. Only
/// delivery holds it.
fn take_token() -> Result<String, String> {
    let token = std::env::var(TOKEN_VARIABLE)
        .ok()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| format!("--deliver needs {TOKEN_VARIABLE} in the environment"))?;
    // SAFETY: this is called from `main` before any thread has been started, so
    // nothing else can be reading the environment while it changes.
    unsafe { std::env::remove_var(TOKEN_VARIABLE) };
    Ok(token)
}
