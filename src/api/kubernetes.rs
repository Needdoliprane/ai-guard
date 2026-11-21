//! Broker-only held-CREATE control operations; runtime credentials stay at the broker.
use super::workload::VerifiedBroker;
use super::*;
use crate::{broker_registry::parse_time, control::kubernetes_admission::*};
use axum::{body::to_bytes, extract::rejection::PathRejection};
use chrono::Utc;
use serde::{Serialize, de::DeserializeOwned};

const MAX_INPUT: usize = 64 * 1024;
const MAX_OUTPUT: usize = 128 * 1024;

pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/brokers/kubernetes/held-create-protocol", get(protocol))
        .route("/brokers/kubernetes/held-creates", post(reserve))
        .route(
            "/brokers/kubernetes/held-creates/{create_id}/claims",
            post(claim),
        )
        .route(
            "/brokers/kubernetes/held-creates/{create_id}/claims/{operation_id}",
            get(observe),
        )
        .route(
            "/brokers/kubernetes/held-creates/{create_id}/results",
            post(result),
        )
        .route(
            "/brokers/kubernetes/held-creates/{create_id}/cancellations",
            post(cancel),
        )
        .route(
            "/brokers/kubernetes/held-creates/{create_id}/recoveries",
            post(recover),
        )
        .route(
            "/brokers/kubernetes/held-creates/{create_id}/recoveries/{operation_id}",
            get(observe_recovery),
        )
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn(workload::request_guard))
}

fn unavailable_reply() -> (StatusCode, Json<Value>) {
    workload::response(
        StatusCode::SERVICE_UNAVAILABLE,
        "held_create_unavailable",
        "Held-create control is unavailable.",
    )
}

fn invalid() -> (StatusCode, Json<Value>) {
    workload::response(
        StatusCode::BAD_REQUEST,
        "invalid_held_create_request",
        "The request does not match the closed held-create contract.",
    )
}

fn failure(
    store: &impl crate::tenant_store::StoreHealth,
    error: anyhow::Error,
) -> (StatusCode, Json<Value>) {
    if !store.store_healthy() {
        return unavailable_reply();
    }
    use KubernetesCreateFailureV1 as F;
    let (status, code, message) = match error.downcast_ref::<F>() {
        Some(F::InvalidRequest) => return invalid(),
        Some(F::PermissionDenied) => (
            StatusCode::FORBIDDEN,
            "held_create_forbidden",
            "This broker scope or current tenant policy does not authorize the operation.",
        ),
        Some(F::NotFound) => (
            StatusCode::NOT_FOUND,
            "held_create_not_found",
            "The exact held-create operation was not found in the authorized scope.",
        ),
        Some(F::Conflict) => (
            StatusCode::CONFLICT,
            "held_create_conflict",
            "The operation conflicts with durable state.",
        ),
        Some(F::StaleEpoch) => (
            StatusCode::CONFLICT,
            "stale_epoch",
            "The result belongs to an earlier create epoch.",
        ),
        Some(F::StaleRevision) => (
            StatusCode::CONFLICT,
            "stale_revision",
            "The result belongs to an earlier revision.",
        ),
        Some(F::QuotaExceeded) => (
            StatusCode::TOO_MANY_REQUESTS,
            "held_create_quota_exceeded",
            "The current tenant quota cannot admit another allocation.",
        ),
        Some(F::MigrationRequired | F::SupervisionInactive | F::Unavailable) => {
            return unavailable_reply();
        }
        None => return workload::store_failure(store, error),
    };
    workload::response(status, code, message)
}

pub(super) fn decode_request<T: DeserializeOwned>(bytes: &[u8]) -> ApiResult<T> {
    if bytes.len() > MAX_INPUT {
        return Err(invalid());
    }
    let value = crate::json::decode_unique(bytes, 64).map_err(|_| invalid())?;
    serde_json::from_value(value).map_err(|_| invalid())
}

pub(super) fn encode_response<T: Serialize>(
    status: StatusCode,
    value: &T,
    redactor: &Redactor,
) -> ApiResult<Response> {
    let value = serde_json::to_value(value).map_err(|_| unavailable_reply())?;
    if redactor.value_unclipped(&value) != value {
        return Err(unavailable_reply());
    }
    let bytes = serde_json::to_vec(&value).map_err(|_| unavailable_reply())?;
    if bytes.len() > MAX_OUTPUT {
        return Err(unavailable_reply());
    }
    Ok((status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response())
}

fn screen_input<T: Serialize>(input: &T, redactor: &Redactor) -> ApiResult<()> {
    let value = serde_json::to_value(input).map_err(|_| invalid())?;
    if redactor.value_unclipped(&value) != value {
        return Err(invalid());
    }
    Ok(())
}

fn no_query(request: &Request) -> ApiResult<()> {
    if request.uri().query().is_some() {
        Err(invalid())
    } else {
        Ok(())
    }
}

async fn input<T: DeserializeOwned>(request: Request) -> ApiResult<T> {
    no_query(&request)?;
    let types = request.headers().get_all(header::CONTENT_TYPE);
    if types.iter().count() != 1
        || !types
            .iter()
            .next()
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(';')
                    .next()
                    .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
            })
    {
        return Err(invalid());
    }
    // No Store I/O slot is retained while an authenticated peer uploads a body.
    let bytes = tokio::time::timeout(
        Duration::from_secs(5),
        to_bytes(request.into_body(), MAX_INPUT),
    )
    .await
    .map_err(|_| invalid())?
    .map_err(|_| invalid())?;
    decode_request(&bytes)
}

fn path_id(path: Result<Path<String>, PathRejection>) -> ApiResult<String> {
    let Path(id) = path.map_err(|_| invalid())?;
    crate::control::uuid(&id).map_err(|_| invalid())?;
    Ok(id)
}

struct TenantAdmission<'a> {
    policy: &'a KubernetesHeldCreatePolicyV1,
    quota: &'a QuotaPolicyV1,
}
impl<'a> TenantAdmission<'a> {
    fn current(tenant: &'a TenantState) -> ApiResult<Self> {
        let policy = tenant.held_create_policy.as_deref().ok_or_else(|| {
            workload::response(
                StatusCode::FORBIDDEN,
                "held_create_forbidden",
                "No held-create admission policy is configured for this tenant.",
            )
        })?;
        Ok(Self {
            policy,
            quota: &tenant.worker_quota,
        })
    }
}

async fn protocol(broker: VerifiedBroker, request: Request) -> ApiResult<Response> {
    no_query(&request)?;
    broker
        .blocking(|tenant, identity, redactor| {
            let view = tenant
                .store
                .sqlite_api()?
                .broker_authenticated_identity(&identity)
                .map_err(|e| failure(&tenant.store, e))?;
            if ![9, 10, 11, 12, 13].contains(&view.schema_version) {
                return Err(unavailable_reply());
            }
            let mut operations = vec![
                KubernetesHeldCreateOperationV1::Reserve,
                KubernetesHeldCreateOperationV1::Claim,
                KubernetesHeldCreateOperationV1::ObserveClaim,
                KubernetesHeldCreateOperationV1::Result,
                KubernetesHeldCreateOperationV1::Cancel,
            ];
            if view.schema_version == 13 {
                operations.extend([
                    KubernetesHeldCreateOperationV1::Recover,
                    KubernetesHeldCreateOperationV1::ObserveRecovery,
                ]);
            }
            let body = KubernetesHeldCreateProtocolV1 {
                version: 1,
                schema_version: view.schema_version,
                tenant_id: view.tenant_id,
                control_namespace_uuid: view.control_namespace_uuid,
                broker_id: view.broker_id,
                incarnation_id: view.incarnation_id,
                binding_revision: view.binding_revision,
                grant_revision: view.grant_revision,
                admission_policy_configured: tenant.held_create_policy.is_some(),
                operations,
            };
            encode_response(StatusCode::OK, &body, &redactor)
        })
        .await
}

async fn reserve(broker: VerifiedBroker, request: Request) -> ApiResult<Response> {
    let input: KubernetesCreateIntentV1 = input(request).await?;
    broker
        .blocking(move |tenant, identity, redactor| {
            screen_input(&input, &redactor)?;
            input.validate().map_err(|_| invalid())?;
            let current = TenantAdmission::current(&tenant)?;
            // Include the current workload bearer for server-derived evidence too;
            // rejecting a changed audit record precedes persistence, never truncates it.
            let store = tenant
                .store
                .with_additional_redaction(&redactor)
                .map_err(|e| failure(&tenant.store, e))?;
            let authorization = store
                .sqlite_api()?
                .authorize_kubernetes_create_admission(
                    &identity,
                    &input,
                    current.policy,
                    current.quota,
                )
                .map_err(|e| failure(&store, e))?;
            let (receipt, snapshot, replayed) = store
                .sqlite_api()?
                .reserve_kubernetes_create(&identity, &authorization, &input)
                .map_err(|e| failure(&store, e))?;
            let body = KubernetesCreateMutationResponseV1 {
                version: 1,
                receipt,
                snapshot,
                replayed,
            };
            encode_response(
                if replayed {
                    StatusCode::OK
                } else {
                    StatusCode::CREATED
                },
                &body,
                &redactor,
            )
        })
        .await
}

fn live_window(reply: &KubernetesCreateClaimResponseV1) -> bool {
    let Some(window) = &reply.receipt.emission_window else {
        return false;
    };
    let (Ok(start), Ok(end)) = (parse_time(&window.issued_at), parse_time(&window.not_after))
    else {
        return false;
    };
    let now = Utc::now();
    start <= now && now < end && end - start <= chrono::Duration::seconds(8)
}

async fn claim(
    broker: VerifiedBroker,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> ApiResult<Response> {
    let create_id = path_id(path)?;
    let input: KubernetesCreateClaimV1 = input(request).await?;
    workload::match_path(&create_id, &input.create_id)?;
    broker
        .blocking(move |tenant, identity, redactor| {
            screen_input(&input, &redactor)?;
            input.validate().map_err(|_| invalid())?;
            // The request has only a hash: resolve the retained intent centrally.
            let store = tenant
                .store
                .with_additional_redaction(&redactor)
                .map_err(|e| failure(&tenant.store, e))?;
            let intent = store
                .sqlite_api()?
                .kubernetes_create_intent_for_admission(&identity, &create_id)
                .map_err(|e| failure(&store, e))?;
            let current = TenantAdmission::current(&tenant)?;
            let authorization = store
                .sqlite_api()?
                .authorize_kubernetes_create_admission(
                    &identity,
                    &intent,
                    current.policy,
                    current.quota,
                )
                .map_err(|e| failure(&store, e))?;
            let (mut body, fresh) = store
                .sqlite_api()?
                .claim_kubernetes_create(&identity, &authorization, &input)
                .map_err(|e| failure(&store, e))?;
            if let Some(proof) = fresh.as_ref() {
                if proof.claim() != &input
                    || proof.receipt() != &body.receipt
                    || proof.snapshot() != &body.snapshot
                {
                    return Err(unavailable_reply());
                }
            }
            let emitting = fresh.is_some()
                && body.disposition == KubernetesCreateDispositionV1::EmitOnce
                && live_window(&body);
            if !emitting {
                body.disposition = KubernetesCreateDispositionV1::ObserveOnly;
            }
            let response = encode_response(
                if emitting {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                },
                &body,
                &redactor,
            )?;
            if emitting && !live_window(&body) {
                body.disposition = KubernetesCreateDispositionV1::ObserveOnly;
                return encode_response(StatusCode::OK, &body, &redactor);
            }
            Ok(response)
        })
        .await
}

async fn observe(
    broker: VerifiedBroker,
    path: Result<Path<(String, String)>, PathRejection>,
    request: Request,
) -> ApiResult<Response> {
    no_query(&request)?;
    let Path((create_id, operation_id)) = path.map_err(|_| invalid())?;
    for id in [&create_id, &operation_id] {
        crate::control::uuid(id).map_err(|_| invalid())?;
    }
    broker
        .blocking(move |tenant, identity, redactor| {
            let store = tenant
                .store
                .with_additional_redaction(&redactor)
                .map_err(|e| failure(&tenant.store, e))?;
            let authorization = store
                .sqlite_api()?
                .authorize_kubernetes_create_reconciliation(
                    &identity,
                    &create_id,
                    &tenant.worker_quota,
                )
                .map_err(|e| failure(&store, e))?;
            let body = store
                .sqlite_api()?
                .observe_kubernetes_create_claim_operation(
                    &identity,
                    &authorization,
                    &create_id,
                    &operation_id,
                )
                .map_err(|e| failure(&store, e))?;
            encode_response(StatusCode::OK, &body, &redactor)
        })
        .await
}

async fn result(
    broker: VerifiedBroker,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> ApiResult<Response> {
    let create_id = path_id(path)?;
    let input: KubernetesCreateResultV1 = input(request).await?;
    workload::match_path(&create_id, &input.create_id)?;
    broker
        .blocking(move |tenant, identity, redactor| {
            screen_input(&input, &redactor)?;
            input.validate().map_err(|_| invalid())?;
            let store = tenant
                .store
                .with_additional_redaction(&redactor)
                .map_err(|e| failure(&tenant.store, e))?;
            let authorization = store
                .sqlite_api()?
                .authorize_kubernetes_create_reconciliation(
                    &identity,
                    &create_id,
                    &tenant.worker_quota,
                )
                .map_err(|e| failure(&store, e))?;
            let committed = store
                .sqlite_api()?
                .record_kubernetes_create_result(&identity, &authorization, &input)
                .map_err(|e| failure(&store, e))?;
            let body = KubernetesCreateResultResponseV1 {
                version: 1,
                receipt: committed.receipt().clone(),
                snapshot: committed.snapshot().clone(),
            };
            encode_response(StatusCode::OK, &body, &redactor)
        })
        .await
}

async fn cancel(
    broker: VerifiedBroker,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> ApiResult<Response> {
    let create_id = path_id(path)?;
    let input: KubernetesCreateCancelV1 = input(request).await?;
    workload::match_path(&create_id, &input.create_id)?;
    broker
        .blocking(move |tenant, identity, redactor| {
            screen_input(&input, &redactor)?;
            input.validate().map_err(|_| invalid())?;
            let store = tenant
                .store
                .with_additional_redaction(&redactor)
                .map_err(|e| failure(&tenant.store, e))?;
            let authorization = store
                .sqlite_api()?
                .authorize_kubernetes_create_reconciliation(
                    &identity,
                    &create_id,
                    &tenant.worker_quota,
                )
                .map_err(|e| failure(&store, e))?;
            let (receipt, snapshot, replayed) = store
                .sqlite_api()?
                .cancel_kubernetes_create(&identity, &authorization, &input)
                .map_err(|e| failure(&store, e))?;
            let body = KubernetesCreateMutationResponseV1 {
                version: 1,
                receipt,
                snapshot,
                replayed,
            };
            encode_response(StatusCode::OK, &body, &redactor)
        })
        .await
}

/// Close-only recovery has its own proof boundary. Reconcile remains bound to
/// the historical incarnation and is never a fallback for this operation.
async fn recover(
    broker: VerifiedBroker,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> ApiResult<Response> {
    if request.uri().path().contains('%') {
        return Err(invalid());
    }
    let create_id = path_id(path)?;
    let input: KubernetesCreateRecoveryV3 = input(request).await?;
    workload::match_path(&create_id, &input.create_id)?;
    broker
        .blocking(move |tenant, identity, redactor| {
            screen_input(&input, &redactor)?;
            input.validate().map_err(|_| invalid())?;
            let store = tenant
                .store
                .with_additional_redaction(&redactor)
                .map_err(|e| failure(&tenant.store, e))?;
            // Preparation checks the exact recorded operation before the
            // initial-state fence. An ACK lost after commit remains replayable.
            let prepared = store
                .sqlite_api()?
                .prepare_kubernetes_create_recovery(&identity, &input, &tenant.worker_quota)
                .map_err(|e| failure(&store, e))?;
            let body = match prepared {
                crate::audit::PreparedRecovery::Historical(proof) => store
                    .sqlite_api()?
                    .lookup_kubernetes_create_recovery(&identity, &proof),
                crate::audit::PreparedRecovery::New(proof) => store
                    .sqlite_api()?
                    .recover_kubernetes_create(&identity, &proof),
            }
            .map_err(|e| failure(&store, e))?;
            encode_response(
                if body.replayed {
                    StatusCode::OK
                } else {
                    StatusCode::CREATED
                },
                &body,
                &redactor,
            )
        })
        .await
}

async fn observe_recovery(
    broker: VerifiedBroker,
    path: Result<Path<(String, String)>, PathRejection>,
    request: Request,
) -> ApiResult<Response> {
    no_query(&request)?;
    if request.uri().path().contains('%') {
        return Err(invalid());
    }
    let Path((create_id, operation_id)) = path.map_err(|_| invalid())?;
    if create_id == operation_id {
        return Err(invalid());
    }
    for id in [&create_id, &operation_id] {
        let parsed = uuid::Uuid::parse_str(id).map_err(|_| invalid())?;
        if parsed.is_nil() || parsed.to_string() != *id {
            return Err(invalid());
        }
    }
    // GET is an exact bodyless receipt lookup, never a new recovery request.
    // Uploading data does not retain a Store I/O permit.
    let bytes = tokio::time::timeout(Duration::from_secs(5), to_bytes(request.into_body(), 1))
        .await
        .map_err(|_| invalid())?
        .map_err(|_| invalid())?;
    if !bytes.is_empty() {
        return Err(invalid());
    }
    broker
        .blocking(move |tenant, identity, redactor| {
            let store = tenant
                .store
                .with_additional_redaction(&redactor)
                .map_err(|e| failure(&tenant.store, e))?;
            let proof = store
                .sqlite_api()?
                .authorize_kubernetes_create_recovery_lookup(&identity, &create_id, &operation_id)
                .map_err(|e| failure(&store, e))?;
            let body = store
                .sqlite_api()?
                .lookup_kubernetes_create_recovery(&identity, &proof)
                .map_err(|e| failure(&store, e))?;
            encode_response(StatusCode::OK, &body, &redactor)
        })
        .await
}
