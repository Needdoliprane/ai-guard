fn main() {
    let raw: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if let Some(code) =
        agentsandbox::runtime::kubernetes::session_worker::dispatch_waiting_mode(&raw)
    {
        std::process::exit(code);
    }
    let arguments: Result<Vec<String>, _> =
        raw.into_iter().map(|value| value.into_string()).collect();
    let Ok(arguments) = arguments else {
        eprintln!("invalid provider bootstrap argument encoding");
        std::process::exit(126);
    };
    match agentsandbox::provider::bootstrap::run_from_environment(&arguments) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!(
                "{}",
                agentsandbox::provider::bootstrap::public_error(&error)
            );
            std::process::exit(126);
        }
    }
}
