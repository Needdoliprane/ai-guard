use agentsandbox::build_identity::{Component, early_exit};
use agentsandbox::egress::channel_helper::{self, HelperMode};

fn main() {
    if let Some(code) = early_exit(Component::Relay) {
        std::process::exit(code);
    }
    // Exact argv profiles only; an unknown mode fails before any runtime,
    // HELLO read or protocol output.
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let egress_requested = arguments
        .first()
        .is_some_and(|mode| channel_helper::is_egress_mode(mode));
    let mode = match channel_helper::parse_mode(arguments) {
        Ok(mode) => mode,
        // A relay or unknown argv keeps the historical relay diagnostic code.
        Err(error) if egress_requested => {
            channel_helper::emit_diagnostic(error);
            std::process::exit(126);
        }
        Err(_) => {
            agentsandbox::relay::channel::helper::emit_diagnostic(
                agentsandbox::relay::channel::helper::HelperError::Protocol,
            );
            std::process::exit(126);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            match mode {
                HelperMode::RelayV1 => agentsandbox::relay::channel::helper::emit_diagnostic(
                    agentsandbox::relay::channel::helper::HelperError::Unavailable,
                ),
                HelperMode::EgressV1(_) => channel_helper::emit_diagnostic(
                    channel_helper::CommonEgressHelperError::Unavailable,
                ),
            }
            std::process::exit(126);
        }
    };
    let failed = match mode {
        HelperMode::RelayV1 => {
            match runtime.block_on(agentsandbox::relay::channel::helper::serve_stdio()) {
                Ok(()) => false,
                Err(error) => {
                    agentsandbox::relay::channel::helper::emit_diagnostic(error);
                    true
                }
            }
        }
        HelperMode::EgressV1(config) => match runtime.block_on(channel_helper::serve_stdio(config))
        {
            Ok(()) => false,
            Err(error) => {
                channel_helper::emit_diagnostic(error);
                true
            }
        },
    };
    if failed {
        std::process::exit(126);
    }
}
