//! Historical-record fixtures only. These receipts are parser test data and
//! cannot mint FreshRelayExecClaim or qualify live Core/workload authority.
use super::*;
use crate::control::{ControlProtocolV1, SupervisionProtocolV1};
use crate::workload_cli::{
    manifest::{Control, Manifest},
    storage::RecordedReceipt,
};
use uuid::Uuid;

pub(crate) fn issued_operation(
    parent: &crate::control_fs::Directory,
    leaf: &str,
    scope: &JournalScopeV1,
    endpoint: &RelayEndpointV1,
) -> std::path::PathBuf {
    let control_name = format!("control-{leaf}.json");
    let control_file = parent.persistent_create(&control_name, &serde_json::to_vec(&serde_json::json!({
        "version":1,"gateway":scope.gateway_origin(),"expected_tenant_id":scope.tenant_id(),
        "control_namespace_uuid":scope.control_namespace_uuid(),"runner_token_file":parent.path().join("unused-fixture-runner"),"ca_bundle_file":null
    })).unwrap()).unwrap();
    control_file.persistent_sync().unwrap();
    parent.sync().unwrap();
    let control = Control::load(control_file.path()).unwrap();
    let manifest = Manifest::Issue {
        version: 1,
        expected_revision: 0,
        subject: WorkloadSubjectV1::Worker {
            worker_id: endpoint.worker_id.clone(),
            attempt_id: endpoint.attempt_id.clone(),
            generation: endpoint.generation,
        },
        expires_at: "2099-01-01T00:00:00Z".into(),
    };
    let path = parent.path().join(leaf);
    let operation = Operation::prepare(
        manifest,
        &control,
        &path,
        "public-parser-runner-fixture-00000000000000",
    )
    .unwrap();
    operation
        .confirm_scope(&ControlProtocolV1 {
            version: 1,
            schema_version: 10,
            control_namespace_uuid: Some(scope.control_namespace_uuid().to_string()),
            tenant_id: scope.tenant_id().into(),
            role: "runner".into(),
            protocols: vec![SupervisionProtocolV1::BrokerSupervisionV1],
            active: true,
        })
        .unwrap();
    operation
        .save_receipt(&RecordedReceipt::Binding(WorkloadBindingReceiptV1 {
            version: 1,
            credential_id: operation.recovery.request.id().into(),
            revision: 1,
            audit_event_id: Uuid::new_v4().to_string(),
            audit_seq: 1,
            audit_hash: "a".repeat(64),
            replayed: false,
        }))
        .unwrap();
    drop(operation);
    path
}
