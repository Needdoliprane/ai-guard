//! Worker-side provider activity channel. Two fixed modes only:
//! `hook-v1 <client> <event>` (run by the provider's managed hooks) and
//! `serve-v1` (started by the broker's fixed exec). Nothing is read from the
//! environment and nothing is written to stdout outside the broker channel.
use agentsandbox::{
    activity_channel::{helper, hook},
    build_identity::{Component, early_exit},
};

fn main() {
    // Static `--version`/`-V` identity only; hook and helper arguments never
    // carry these flags.
    if let Some(code) = early_exit(Component::Activity) {
        std::process::exit(code);
    }
    let arguments: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let code = match arguments.first().and_then(|mode| mode.to_str()) {
        Some("serve-v1") if arguments.len() == 1 => helper::main_v1(),
        Some("hook-v1") => hook::main_v1(&arguments[1..]),
        _ => hook::EXIT_USAGE,
    };
    std::process::exit(code);
}
