//! Pure admission rules over synthetic retained metadata. These fixtures use
//! no SQLite, no authenticated identity and no runtime.
use super::*;

const A1: &str = "a1a1a1a1-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const A2: &str = "a2a2a2a2-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const A3: &str = "a3a3a3a3-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const TASKS: &str = "11111111-1111-4111-8111-111111111111";
const RESULTS: &str = "22222222-2222-4222-8222-222222222222";
const OTHER: &str = "33333333-3333-4333-8333-333333333333";
const EARLY: &str = "2026-11-10T12:00:10.000Z";
const NOW: &str = "2026-11-10T12:00:20.000Z";
const LATE: &str = "2026-11-10T12:00:40.000Z";

fn endpoint(worker: &str) -> RelayEndpointV1 {
    RelayEndpointV1 {
        worker_id: worker.into(),
        attempt_id: "00000000-0000-4000-8000-000000000000".into(),
        generation: 1,
    }
}
fn body(source: &str, grant: &str, bytes: u64, expires_at: &str) -> RetainedBody {
    RetainedBody {
        expires: timestamp(expires_at).unwrap(),
        metadata: RelayMessageMetadataV1 {
            version: 1,
            message_id: uuid::Uuid::new_v4().to_string(),
            grant_id: grant.into(),
            source: endpoint(source),
            destination: endpoint(A3),
            channel: "tasks".into(),
            created_at: "2026-11-10T12:00:00.000Z".into(),
            expires_at: expires_at.into(),
            body_bytes: bytes,
            body_sha256: "0".repeat(64),
            acknowledged: false,
            body_pruned: false,
        },
    }
}
fn capacity(messages: u64, bytes: u64) -> RelayCapacityV1 {
    RelayCapacityV1 { messages, bytes }
}
fn policy(
    tenant: RelayCapacityV1,
    sender: RelayCapacityV1,
    grant: RelayCapacityV1,
) -> RelayRetentionPolicyV1 {
    let policy = RelayRetentionPolicyV1 {
        tenant,
        sender,
        grant,
        ..RelayRetentionPolicyV1::default()
    };
    policy.validate().unwrap();
    policy
}
fn decide(
    policy: &RelayRetentionPolicyV1,
    retained: &Retained,
    source: &str,
    grant: &str,
    bytes: u64,
) -> anyhow::Result<()> {
    admit(
        policy,
        retained,
        &endpoint(source),
        grant,
        bytes,
        timestamp(NOW).unwrap(),
    )
}
fn refusal(result: anyhow::Result<()>) -> RelayCapacityRefusalV1 {
    result
        .unwrap_err()
        .downcast::<RelayCapacityRefusalV1>()
        .expect("a typed own-scope refusal")
}

#[test]
fn sender_share_is_checked_before_the_grant_share_and_the_tenant_ceiling() {
    let policy = policy(
        capacity(8, 524_288),
        capacity(3, 196_608),
        capacity(2, 131_072),
    );
    let mut retained = Retained {
        bodies: vec![body(A1, TASKS, 10, LATE), body(A1, TASKS, 10, EARLY)],
    };
    // The grant is full; another grant of the same sender still has room.
    let grant_full = refusal(decide(&policy, &retained, A1, TASKS, 1));
    assert_eq!(grant_full.scope, RelayCapacityScopeV1::Grant);
    assert_eq!(grant_full.dimension, RelayCapacityDimensionV1::Messages);
    assert_eq!(grant_full.limit, capacity(2, 131_072));
    assert_eq!(grant_full.retained, capacity(2, 20));
    assert_eq!(grant_full.requested_bytes, 1);
    assert_eq!(grant_full.next_release_at.as_deref(), Some(EARLY));
    decide(&policy, &retained, A1, RESULTS, 1).unwrap();

    // Once the sender is full, the sender scope wins even on the full grant:
    // switching grants cannot help.
    retained.bodies.push(body(A1, RESULTS, 10, LATE));
    for grant in [TASKS, RESULTS, OTHER] {
        let sender_full = refusal(decide(&policy, &retained, A1, grant, 1));
        assert_eq!(sender_full.scope, RelayCapacityScopeV1::Sender);
        assert_eq!(sender_full.retained, capacity(3, 30));
        assert_eq!(sender_full.limit, capacity(3, 196_608));
    }
    // The neighbor is untouched by A1's saturation.
    decide(&policy, &retained, A2, OTHER, 65_536).unwrap();
}

#[test]
fn byte_dimension_is_exact_and_a_refusal_never_reports_a_neighbor() {
    let policy = policy(
        capacity(12, 393_216),
        capacity(6, 131_072),
        capacity(6, 131_072),
    );
    let retained = Retained {
        bodies: vec![
            body(A1, TASKS, 65_536, LATE),
            body(A1, TASKS, 65_535, LATE),
            // A neighbor's earlier expiry and bytes must not appear in A1's refusal.
            body(A2, OTHER, 65_536, EARLY),
        ],
    };
    decide(&policy, &retained, A1, TASKS, 1).unwrap();
    let over = refusal(decide(&policy, &retained, A1, TASKS, 2));
    assert_eq!(over.scope, RelayCapacityScopeV1::Sender);
    assert_eq!(over.dimension, RelayCapacityDimensionV1::Bytes);
    assert_eq!(over.retained, capacity(2, 131_071));
    assert_eq!(over.requested_bytes, 2);
    assert_eq!(over.next_release_at.as_deref(), Some(LATE));
    let encoded = serde_json::to_string(&over).unwrap();
    assert!(!encoded.contains(A2) && !encoded.contains(EARLY));
}

#[test]
fn tenant_ceiling_refusal_carries_the_retry_action_and_no_figure() {
    let policy = policy(
        capacity(2, 131_072),
        capacity(1, 65_536),
        capacity(1, 65_536),
    );
    let retained = Retained {
        bodies: vec![body(A1, TASKS, 1, EARLY), body(A2, RESULTS, 1, LATE)],
    };
    // A3 is under its own share. The tenant refusal carries only policy advice.
    for (automatic_prune_batch, retry_action) in [
        (8, RelayRetryActionV1::RetryAfterRelease),
        (0, RelayRetryActionV1::ContactOperator),
    ] {
        let configured = RelayRetentionPolicyV1 {
            automatic_prune_batch,
            ..policy.clone()
        };
        let error = decide(&configured, &retained, A3, OTHER, 1).unwrap_err();
        assert_eq!(
            error.downcast_ref::<RelayTenantCapacityRefusalV1>(),
            Some(&RelayTenantCapacityRefusalV1 { retry_action })
        );
        assert!(error.downcast_ref::<RelayCapacityRefusalV1>().is_none());
        assert!(!error.to_string().contains(EARLY));
    }

    // An empty scope has no deadline to report and nothing to acknowledge.
    let empty = Retained::default();
    assert_eq!(
        empty.usage(timestamp(NOW).unwrap(), |_| true),
        Usage {
            retained: capacity(0, 0),
            next_expiry: None,
            awaits_ack: false
        }
    );
    decide(&policy, &empty, A3, OTHER, 65_536).unwrap();
}

#[test]
fn tenant_advice_without_automatic_pruning_does_not_reveal_neighbor_acknowledgement_state() {
    let policy = RelayRetentionPolicyV1 {
        automatic_prune_batch: 0,
        release_on_ack: true,
        ..policy(
            capacity(2, 131_072),
            capacity(1, 65_536),
            capacity(1, 65_536),
        )
    };
    for (expiry, acknowledged) in [(LATE, false), (EARLY, false), (LATE, true)] {
        let mut neighbor = body(A2, RESULTS, 1, expiry);
        neighbor.metadata.acknowledged = acknowledged;
        let retained = Retained {
            bodies: vec![body(A1, TASKS, 1, EARLY), neighbor],
        };
        let error = decide(&policy, &retained, A3, OTHER, 1).unwrap_err();
        assert_eq!(
            error.downcast_ref::<RelayTenantCapacityRefusalV1>(),
            Some(&RelayTenantCapacityRefusalV1 {
                retry_action: RelayRetryActionV1::ContactOperator,
            }),
            "tenant advice exposed neighbor state: expiry={expiry}, acknowledged={acknowledged}"
        );
    }
}

#[test]
fn retry_is_announced_only_while_sends_or_an_acknowledgement_can_free_the_refused_scope() {
    let now = timestamp(NOW).unwrap();
    let shares = || {
        policy(
            capacity(4, 262_144),
            capacity(2, 131_072),
            capacity(2, 131_072),
        )
    };
    let with = |automatic_prune_batch, release_on_ack| RelayRetentionPolicyV1 {
        automatic_prune_batch,
        release_on_ack,
        ..shares()
    };
    let awaiting = |awaits_ack| Usage {
        awaits_ack,
        ..Usage::default()
    };
    use RelayRetryActionV1::{ContactOperator, RetryAfterRelease};
    for (batch, release_on_ack, awaits_ack, expected) in [
        // A positive batch allows a later send to release expired bodies.
        (8, false, false, RetryAfterRelease),
        (8, false, true, RetryAfterRelease),
        (8, true, false, RetryAfterRelease),
        // Nothing releases automatically.
        (0, false, true, ContactOperator),
        (0, false, false, ContactOperator),
        // Acknowledgement is the only release: it needs a body it can release.
        (0, true, true, RetryAfterRelease),
        (0, true, false, ContactOperator),
    ] {
        assert_eq!(
            own_retry_action(&with(batch, release_on_ack), &awaiting(awaits_ack)),
            expected,
            "batch {batch}, release_on_ack {release_on_ack}, awaits_ack {awaits_ack}"
        );
    }

    // What an acknowledgement can release: a body neither expired nor
    // acknowledged. Expiry is inclusive, as in the acknowledgement itself.
    let mut acknowledged = body(A1, TASKS, 1, LATE);
    acknowledged.metadata.acknowledged = true;
    let retained = Retained {
        bodies: vec![
            body(A1, TASKS, 1, EARLY),
            body(A1, TASKS, 1, NOW),
            acknowledged,
        ],
    };
    assert!(!retained.usage(now, |_| true).awaits_ack);
    assert!(
        retained
            .usage(timestamp(NOW).unwrap() - Duration::milliseconds(1), |_| {
                true
            })
            .awaits_ack,
        "a body expiring one millisecond later was already counted as expired"
    );
}

#[test]
fn acknowledgement_advice_depends_on_the_refused_own_scope() {
    use RelayRetryActionV1::{ContactOperator, RetryAfterRelease};

    let policy = RelayRetentionPolicyV1 {
        automatic_prune_batch: 0,
        release_on_ack: true,
        ..policy(
            capacity(8, 524_288),
            capacity(4, 262_144),
            capacity(2, 131_072),
        )
    };
    for neighbor_expiry in [EARLY, LATE] {
        let mut retained = Retained {
            bodies: vec![
                body(A1, TASKS, 1, EARLY),
                body(A1, TASKS, 1, NOW),
                body(A1, RESULTS, 1, LATE),
                body(A2, OTHER, 1, neighbor_expiry),
            ],
        };
        let grant = refusal(decide(&policy, &retained, A1, TASKS, 1));
        assert_eq!(grant.scope, RelayCapacityScopeV1::Grant);
        assert_eq!(grant.retry_action, ContactOperator);

        retained.bodies.push(body(A1, RESULTS, 1, EARLY));
        let sender = refusal(decide(&policy, &retained, A1, TASKS, 1));
        assert_eq!(sender.scope, RelayCapacityScopeV1::Sender);
        assert_eq!(sender.retry_action, RetryAfterRelease);
    }
}

#[test]
fn release_order_takes_expired_own_bodies_first_then_the_oldest_of_the_tenant() {
    let now = timestamp("2026-11-10T12:00:20.000Z").unwrap();
    let neighbor_oldest = body(A2, OTHER, 1, "2026-11-10T12:00:05.000Z");
    let own_early = body(A1, TASKS, 1, EARLY);
    let own_live = body(A1, TASKS, 1, LATE);
    let neighbor_later = body(A2, OTHER, 1, "2026-11-10T12:00:15.000Z");
    // Expiry is inclusive: a body expiring exactly now is releasable.
    let own_now = body(A1, RESULTS, 1, "2026-11-10T12:00:20.000Z");
    let id = |body: &RetainedBody| body.metadata.message_id.clone();
    let (oldest, early, later, at_now) = (
        id(&neighbor_oldest),
        id(&own_early),
        id(&neighbor_later),
        id(&own_now),
    );
    let retained = Retained {
        bodies: vec![
            neighbor_oldest,
            own_early,
            own_live,
            neighbor_later,
            own_now,
        ],
    };
    assert_eq!(
        retained.release_order(&endpoint(A1), now, 8),
        [early.clone(), at_now.clone(), oldest.clone(), later.clone()]
    );
    assert_eq!(
        retained.release_order(&endpoint(A1), now, 1),
        [early.as_str()]
    );
    assert!(retained.release_order(&endpoint(A1), now, 0).is_empty());
    // A sender without expired bodies drains the oldest bodies of the tenant.
    assert_eq!(
        retained.release_order(&endpoint(A3), now, 3),
        [oldest, early, later]
    );
    assert!(
        retained
            .release_order(
                &endpoint(A1),
                timestamp(EARLY).unwrap() - Duration::milliseconds(6000),
                8
            )
            .is_empty(),
        "a live body was selected for release"
    );
}

#[test]
fn status_orders_largest_first_then_by_identifier_and_keeps_truncation_visible() {
    let now = timestamp("2026-11-10T12:00:20.000Z").unwrap();
    let expired = body(A1, TASKS, 10, EARLY);
    let expired_id = expired.metadata.message_id.clone();
    let mut acknowledged = body(A2, OTHER, 20, LATE);
    acknowledged.metadata.acknowledged = true;
    let retained = Retained {
        bodies: vec![body(A1, RESULTS, 10, LATE), acknowledged, expired],
    };
    let policy = RelayRetentionPolicyV1::default();
    let status = retained.status(&policy, now, 8).unwrap();
    assert_eq!(status.version, 1);
    assert_eq!(status.observed_at, "2026-11-10T12:00:20.000Z");
    assert_eq!(status.policy_fingerprint, policy.fingerprint().unwrap());
    assert_eq!(status.tenant.limit, policy.tenant);
    assert_eq!(status.tenant.retained, capacity(3, 40));
    assert_eq!(status.tenant.expired, capacity(1, 10));
    assert_eq!(status.tenant.acknowledged, capacity(1, 20));
    assert_eq!(status.tenant.next_expiry_at.as_deref(), Some(EARLY));
    // Equal bytes: the identifier decides, so the listing is stable.
    let senders: Vec<_> = status
        .senders
        .iter()
        .map(|sender| (sender.worker_id.as_str(), sender.retained, sender.expired))
        .collect();
    assert_eq!(
        senders,
        [
            (A1, capacity(2, 20), capacity(1, 10)),
            (A2, capacity(1, 20), capacity(0, 0))
        ]
    );
    let grants: Vec<_> = status
        .grants
        .iter()
        .map(|grant| (grant.grant_id.as_str(), grant.retained.bytes))
        .collect();
    assert_eq!(grants, [(OTHER, 20), (TASKS, 10), (RESULTS, 10)]);
    assert_eq!(status.grants[0].source_worker_id, A2);
    assert_eq!(status.grants[0].limit, policy.grant);
    assert_eq!(status.grants[1].next_expiry_at.as_deref(), Some(EARLY));
    assert_eq!(status.prunable.len(), 1);
    assert_eq!(status.prunable[0].message_id, expired_id);
    assert_eq!(status.prunable[0].expires_at, EARLY);
    assert_eq!(
        (
            status.senders_total,
            status.grants_total,
            status.prunable_total
        ),
        (2, 3, 1)
    );

    let truncated = retained.status(&policy, now, 1).unwrap();
    assert_eq!(
        (
            truncated.senders.len(),
            truncated.grants.len(),
            truncated.prunable.len()
        ),
        (1, 1, 1)
    );
    assert_eq!(
        (
            truncated.senders_total,
            truncated.grants_total,
            truncated.prunable_total
        ),
        (2, 3, 1)
    );
    assert_eq!(truncated.tenant, status.tenant);

    let empty = Retained::default().status(&policy, now, 8).unwrap();
    assert_eq!(empty.tenant.retained, capacity(0, 0));
    assert_eq!(empty.tenant.next_expiry_at, None);
    assert!(empty.senders.is_empty() && empty.grants.is_empty() && empty.prunable.is_empty());
}

#[cfg(unix)]
#[test]
fn retention_policy_file_is_private_bounded_and_closed() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().join("relay-retention.json");
    let shipped = RelayRetentionPolicyV1::default();
    std::fs::write(&path, serde_json::to_vec(&shipped).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(Store::load_relay_retention_policy(&path).unwrap(), shipped);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        Store::load_relay_retention_policy(&path).is_err(),
        "a policy file readable by others was accepted"
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let alias = directory.path().join("alias.json");
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    assert!(Store::load_relay_retention_policy(&alias).is_err());
    let mut unknown = serde_json::to_value(&shipped).unwrap();
    unknown["burst"] = serde_json::json!(1);
    std::fs::write(&path, unknown.to_string()).unwrap();
    assert!(Store::load_relay_retention_policy(&path).is_err());
    std::fs::write(&path, vec![b' '; 16_385]).unwrap();
    assert!(Store::load_relay_retention_policy(&path).is_err());
}
