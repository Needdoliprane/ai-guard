fn main() {
    let arguments: Result<Vec<String>, _> = std::env::args_os()
        .skip(1)
        .map(|value| value.into_string())
        .collect();
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
