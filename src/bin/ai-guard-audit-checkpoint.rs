//! Offline administration of signed audit checkpoints. The logic lives in
//! `agentsandbox::audit::signed_checkpoint`; this binary only relays the
//! command line, one JSON document and the exit status.

use std::io::Write;

fn main() -> std::process::ExitCode {
    let completed = agentsandbox::audit::signed_checkpoint::cli::run(std::env::args_os());
    let mut stdout = std::io::stdout().lock();
    if writeln!(stdout, "{}", completed.stdout)
        .and_then(|()| stdout.flush())
        .is_err()
    {
        // The result could not be delivered: never report success.
        return std::process::ExitCode::from(1);
    }
    std::process::ExitCode::from(completed.exit_code)
}
