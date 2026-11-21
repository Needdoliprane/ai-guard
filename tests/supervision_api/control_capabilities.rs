//! The private, runner-only capabilities contract on the real SQLite gateway.
use super::*;
use agentsandbox::control::{ControlCapabilitiesV1, ControlOperationV1, ControlStorageBackendV1};

const PRIVATE: &str = "/control/api/v1/control/capabilities";
const PUBLIC: &str = "/api/v1/control/capabilities";

#[tokio::test]
async fn capabilities_are_private_runner_only_typed_bounded_and_tenant_bound() {
    let fixture = Fixture::new();
    let before = fixture.heads();

    let response = send(&fixture.router, Method::GET, PRIVATE, Some(&fixture.runner_a), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 65_536).await.unwrap();
    assert!(bytes.len() <= ControlCapabilitiesV1::MAX_BYTES);
    let a: ControlCapabilitiesV1 = serde_json::from_slice(&bytes).unwrap();
    a.validate().unwrap();
    assert_eq!(a.role, "runner");
    assert_eq!(a.tenant_id, "tenant-a");
    assert_eq!(a.storage, ControlStorageBackendV1::Sqlite);
    assert_eq!(
        a.operations,
        [
            ControlOperationV1::RecoveryInspect,
            ControlOperationV1::StopReconcile,
            ControlOperationV1::SupervisedAdmission,
        ]
    );
    let protocol = send(
        &fixture.router,
        Method::GET,
        "/control/api/v1/control/protocol",
        Some(&fixture.runner_a),
        None,
    )
    .await;
    let protocol: Value = response_json(protocol).await;
    assert_eq!(
        protocol["control_namespace_uuid"].as_str(),
        a.control_namespace_uuid.as_deref(),
        "the capabilities name the same verified namespace as the protocol"
    );

    // The identity, never a request field, selects the tenant.
    let b = send(&fixture.router, Method::GET, PRIVATE, Some(&fixture.runner_b), None).await;
    let b: ControlCapabilitiesV1 = response_json(b).await;
    assert_eq!(b.tenant_id, "tenant-b");
    assert_ne!(b.control_namespace_uuid, a.control_namespace_uuid);

    // A viewer, a missing credential and the public transport are refused.
    for (path, token, status) in [
        (PRIVATE, Some(fixture.viewer_a.as_str()), StatusCode::FORBIDDEN),
        (PRIVATE, None, StatusCode::UNAUTHORIZED),
        (PUBLIC, Some(fixture.runner_a.as_str()), StatusCode::NOT_FOUND),
        (PUBLIC, None, StatusCode::NOT_FOUND),
    ] {
        let response = send(&fixture.router, Method::GET, path, token, None).await;
        assert_eq!(response.status(), status, "{path} {token:?}");
    }
    // A schema2 tenant announces no operation and no namespace.
    let legacy = send(&fixture.router, Method::GET, PRIVATE, Some(&fixture.runner_v2), None).await;
    let legacy: ControlCapabilitiesV1 = response_json(legacy).await;
    assert!(legacy.operations.is_empty() && legacy.control_namespace_uuid.is_none());
    assert_eq!(fixture.heads(), before, "capabilities never write an audit row");
}
