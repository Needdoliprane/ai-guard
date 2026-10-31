use std::process::Command;

#[test]
fn broker_help_lists_the_approved_service_commands() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["broker", "--help"])
        .output()
        .expect("run the real agentsandbox binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "broker --help must succeed; status={:?}; stderr={stderr}",
        output.status
    );
    for command in [
        "serve", "submit", "status", "list", "approve", "cancel", "export",
    ] {
        assert!(
            stdout.split_whitespace().any(|word| word == command),
            "broker --help omitted `{command}`; stdout={stdout}"
        );
    }
}
