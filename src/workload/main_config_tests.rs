use super::*;

#[test]
fn workload_policy_file_is_local_and_migration6_is_an_explicit_cli_target() {
    assert!(
        Cli::try_parse_from([
            "agentsandbox",
            "gateway",
            "--workload-audit-file",
            "workload.json"
        ])
        .is_ok()
    );
    assert!(
        Cli::try_parse_from([
            "agentsandbox",
            "gateway",
            "--workload-audit-file",
            "workload.json",
            "--tenants-file",
            "tenants.json"
        ])
        .is_err()
    );
    let parsed = Cli::try_parse_from([
        "agentsandbox",
        "migrate-control",
        "--data",
        "audit.sqlite",
        "--tenant-id",
        "tenant-a",
        "--backup",
        "backup.sqlite",
        "--target-version",
        "6",
        "--confirm-control-stopped",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Command::MigrateControl {
            target_version: 6,
            ..
        }
    ));
    assert!(
        Cli::try_parse_from([
            "agentsandbox",
            "migrate-control",
            "--data",
            "audit.sqlite",
            "--tenant-id",
            "tenant-a",
            "--backup",
            "backup.sqlite",
            "--target-version",
            "14",
            "--confirm-control-stopped"
        ])
        .is_err()
    );
}
