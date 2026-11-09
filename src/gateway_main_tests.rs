//! Main's real policy handoff, prepared before the assembly change.
use super::*;
use std::{
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
};

fn local_tenant_fixture() -> (tempfile::TempDir, gateway_config::ConfiguredTenant) {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let runner = directory.path().join("runner");
    let viewer = directory.path().join("viewer");
    for (path, token) in [
        (&runner, "fixture-main-session-policy-runner-20261108"),
        (&viewer, "fixture-main-session-policy-viewer-20261108"),
    ] {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        file.write_all(token.as_bytes()).unwrap();
    }
    let database = directory.path().join("local.sqlite");
    let cli = Cli::try_parse_from([
        "agentsandbox",
        "gateway",
        "--data",
        database.to_str().unwrap(),
        "--runner-token-file",
        runner.to_str().unwrap(),
        "--viewer-token-file",
        viewer.to_str().unwrap(),
    ])
    .unwrap();
    let Command::Gateway(arguments) = cli.command else {
        unreachable!()
    };
    (directory, configured_local_tenant(&arguments).unwrap())
}

fn state(directory: &tempfile::TempDir) -> GatewayState {
    GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "local".into(),
            store: Arc::new(
                Store::open_for_tenant(
                    &directory.path().join("store.sqlite"),
                    "local",
                    Redactor::new(vec![]),
                    1000,
                )
                .unwrap(),
            ),
            runner_token: "fixture-main-session-policy-runner-20261108".into(),
            viewer_token: "fixture-main-session-policy-viewer-20261108".into(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap()
}

#[test]
fn local_main_configuration_keeps_all_three_session_policies_disabled() {
    let (directory, tenant) = local_tenant_fixture();
    assert!(tenant.session_reservation_policy.is_none());
    assert!(tenant.session_recovery_policy.is_none());
    assert!(tenant.session_control_recovery_policy.is_none());
    let policies = GatewaySessionPolicies::from_tenants(&[tenant]);
    assert!(policies.reservation.is_empty());
    assert!(policies.recovery.is_empty());
    assert!(policies.control_recovery.is_empty());
    assert!(!directory.path().join("local.sqlite").exists());
    assert!(policies.apply(state(&directory)).is_ok());
}

#[test]
fn main_passes_each_configured_session_policy_to_its_real_checked_builder() {
    // Omitting either collection or the corresponding builder admits the
    // deliberately unknown tenant below instead of returning the real error.
    for selected in 0..3 {
        let (directory, mut tenant) = local_tenant_fixture();
        tenant.tenant_id = "not-in-gateway".into();
        let empty =
            serde_json::json!({"version":2,"decision_ttl_seconds":30,"allowed_broker_ids":[]});
        match selected {
            0 => {
                tenant.session_reservation_policy = Some(
                    serde_json::from_value(
                        serde_json::json!({"version":2,"decision_ttl_seconds":30,"approvals":[]}),
                    )
                    .unwrap(),
                )
            }
            1 => tenant.session_recovery_policy = Some(serde_json::from_value(empty).unwrap()),
            2 => {
                tenant.session_control_recovery_policy =
                    Some(serde_json::from_value(empty).unwrap())
            }
            _ => unreachable!(),
        }
        let policies = GatewaySessionPolicies::from_tenants(&[tenant]);
        assert_eq!(policies.reservation.len(), usize::from(selected == 0));
        assert_eq!(policies.recovery.len(), usize::from(selected == 1));
        assert_eq!(policies.control_recovery.len(), usize::from(selected == 2));
        assert!(policies.apply(state(&directory)).is_err());
    }
}
