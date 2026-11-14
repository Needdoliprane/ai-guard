use agentsandbox::build_identity::{Component, early_exit};

fn main() {
    if let Some(code) = early_exit(Component::Relay) {
        std::process::exit(code);
    }
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 1 || arguments[0] != "serve-v1" {
        agentsandbox::relay::channel::helper::emit_diagnostic(
            agentsandbox::relay::channel::helper::HelperError::Protocol,
        );
        std::process::exit(126);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let result = match runtime {
        Ok(runtime) => runtime.block_on(agentsandbox::relay::channel::helper::serve_stdio()),
        Err(_) => Err(agentsandbox::relay::channel::helper::HelperError::Unavailable),
    };
    if let Err(error) = result {
        agentsandbox::relay::channel::helper::emit_diagnostic(error);
        std::process::exit(126);
    }
}
