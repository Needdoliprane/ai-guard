//! Narrow registry metadata from one already checked Business transaction.
//! No policy body, human authority, audit append or execution permit is exposed.
use super::*;
use crate::control::egress_policy::{
    EgressPolicyExpectedScopeV1, EgressPolicyInspectionRequestV1, EgressPolicyInspectionV1,
    EgressPolicyObservedCeilingV1 as CeilingObservation,
    EgressPolicyObservedHeadV1 as HeadObservation,
    EgressPolicyObservedSessionResourceV1 as ResourceObservation,
};

pub(in crate::audit) fn require_inspection_format(tx: &CheckedTransaction<'_>) -> anyhow::Result<()> {
    tx.require_business_access()?;
    require15(tx)
}

impl Store {
    pub(in crate::audit) fn egress_policy_inspection_checked(
        &self,
        tx: &CheckedTransaction<'_>,
        request: &EgressPolicyInspectionRequestV1,
        clock: &workload::AuthorizedControlTimeV1,
    ) -> anyhow::Result<EgressPolicyInspectionV1> {
        tx.require_business_access()?;
        require15(tx)?;
        request.validate().map_err(|_| EgressStoreFailure::Invalid)?;
        let projection = replay(tx)?;
        let selector = request.selector();
        let head = match projection.heads.get(selector) {
            Some(head) => HeadObservation::Present {
                revision: head.policy.revision(),
                epoch: head.epoch,
                state: head.state,
            },
            None => HeadObservation::Absent,
        };
        let (ceiling, resource) = match selector {
            EgressPolicySelectorV1::Tenant => (
                CeilingObservation::NotApplicable,
                ResourceObservation::NotApplicable,
            ),
            EgressPolicySelectorV1::Session(id) => {
                let ceiling = match projection.heads.get(&EgressPolicySelectorV1::Tenant) {
                    None => CeilingObservation::Absent,
                    Some(head) if head.state == EgressPolicyStateV1::Active => {
                        CeilingObservation::Active {
                            revision: head.policy.revision(),
                            epoch: head.epoch,
                            digest: head.policy.digest().into(),
                        }
                    }
                    Some(head) => CeilingObservation::Inactive {
                        revision: head.policy.revision(),
                        epoch: head.epoch,
                    },
                };
                let resource = crate::identity::ResourceRefV1 {
                    kind: crate::identity::ResourceKindV1::Session,
                    resource_id: id.clone(),
                };
                let presence = match current_session_grant(tx, &resource)? {
                    None => ResourceObservation::Absent,
                    Some(grant) => {
                        grant.validate()?;
                        ensure(grant.tenant_id == self.tenant_id() && grant.resource == resource)?;
                        if grant.active {
                            ResourceObservation::Active
                        } else {
                            ResourceObservation::Inactive
                        }
                    }
                };
                (ceiling, presence)
            }
        };
        let namespace = crate::audit::control::supervision::namespace(tx)?;
        let scope = EgressPolicyExpectedScopeV1::new(self.tenant_id(), &namespace)
            .map_err(|_| EgressStoreFailure::Integrity)?;
        let observation = EgressPolicyInspectionV1::observed(
            scope,
            selector.clone(),
            head,
            ceiling,
            resource,
            crate::broker_registry::format_time(clock.at()),
        )
        .map_err(|_| EgressStoreFailure::Integrity)?;
        // A collision in the configured dictionary refuses the exact report;
        // alteration would break its canonical scope/CAS and historical value.
        let value = serde_json::to_value(&observation)?;
        ensure(self.redactor.value(&value) == value)?;
        tx.require_business_access()?;
        Ok(observation)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use axum::{body::{Body, to_bytes}, http::{Request, StatusCode, header}};
    use tower::ServiceExt;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn configured_metadata_redaction_refuses_the_whole_observation_without_a_write() {
        let mut files = Store::owned_control_fixture_for_test(15);
        let (mut bundle, credentials, tokens) = files.take_startup();
        let source = bundle.owner.closure().sources.iter()
            .find(|source| source.tenant_id.as_deref() == Some("A")).unwrap();
        let source_id = source.source_id;
        let database = source.storage_binding.path();
        let store = bundle.stores.get_mut(&source_id).unwrap();
        // Configure a literal collision before the retained consumer is
        // published. No live source, epoch or authorization guard is changed.
        store.redactor = store.redactor.union(&Redactor::new(vec!["policy_registry_metadata".into()]));
        let state = crate::api::GatewayState::for_owned_human_sources(
            bundle, credentials, tokens, crate::policy::Policy::default(), None, Redactor::new(vec![]),
        ).unwrap();
        let retirement = state.human_retirement().unwrap();
        let router = crate::api::gateway_router(state);
        let request = |method: &'static str, path: &'static str, body: &'static str| Request::builder().method(method).uri(path)
            .header(header::AUTHORIZATION, "Bearer synthetic-A-runner")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body)).unwrap();
        let protocol = router.clone().oneshot(request("GET", "/control/api/v1/control/protocol", "")).await.unwrap();
        assert_eq!(protocol.status(), StatusCode::OK, "real source/binding admission must be healthy");
        drop(protocol);
        let db = Connection::open_with_flags(&database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let counts = || db.query_row("SELECT count(*),coalesce(max(seq),0) FROM events", [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))).unwrap();
        let before = counts();
        let before_data_version: i64 = db.pragma_query_value(None, "data_version", |row| row.get(0)).unwrap();
        let response = router.oneshot(request("POST", "/control/api/v1/egress/policies/inspect",
            r#"{"version":1,"selector":{"kind":"tenant"}}"#)).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        for field in ["scope", "head", "tenant_ceiling", "session_resource", "observed_at", "coverage"] {
            assert!(body.get(field).is_none(), "redaction must not publish a usable partial observation");
        }
        assert_eq!(counts(), before, "technical redaction refusal wrote an audit Event");
        let after_data_version: i64 = db.pragma_query_value(None, "data_version", |row| row.get(0)).unwrap();
        assert_eq!(after_data_version, before_data_version, "redaction refusal changed the actual database");
        drop(db);
        retirement.retire();
        tokio::time::timeout(std::time::Duration::from_secs(30), retirement.drain()).await.unwrap().unwrap();
        drop(files);
    }
}
