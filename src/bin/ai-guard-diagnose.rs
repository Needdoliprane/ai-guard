//! Offline, read-only inspection of an ai-guard installation.
//!
//! Exit codes: 0 report without error or unknown, 3 report with an error,
//! 4 report with an unknown and no error, 2 usage error, 1 refusal without
//! any report.

#[derive(clap::Parser)]
#[command(
    name = "ai-guard-diagnose",
    version,
    about = "Offline, read-only inspection of an ai-guard installation",
    long_about = "Offline, read-only inspection of an ai-guard installation.\n\nThe tool starts nothing and writes nothing. It opens no file that the inspected files name as a key, a token, a credential or a store; a file given by option is opened as what that option says it is. It contacts nothing unless the option --active names an active check. It inspects only the paths given below and reports what it could not examine as not checked or not available.\n\nExit codes: 0 report without error or unknown; 3 report with at least one error; 4 report with an unknown and no error; 2 usage error; 1 refusal, no report."
)]
struct Cli {
    #[command(flatten)]
    args: agentsandbox::diagnostics::DiagnoseArgs,
}

// A current-thread runtime, as the offline OIDC client build expects a Tokio
// context. The only thing ever awaited is an active check the operator named.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    agentsandbox::diagnostics::install_panic_hook();
    let cli = match <Cli as clap::Parser>::try_parse() {
        Ok(cli) => cli,
        Err(error) => std::process::exit(agentsandbox::diagnostics::usage_exit(&error)),
    };
    match agentsandbox::diagnostics::execute(cli.args).await {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(agentsandbox::diagnostics::EXIT_REFUSED)
        }
    }
}
