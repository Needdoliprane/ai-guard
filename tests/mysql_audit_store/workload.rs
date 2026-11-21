use super::{
    expected_hash, fixture_prefix,
    identity::{
        FixtureFiles, insert_event, lock_head, prepare_identity, require_fresh, revoke_identity,
        write_head,
    },
    worker,
};
use agentsandbox::{
    audit::{AuditCapacityFailureV1, EventQuery},
    control::{QuotaPolicyV1, WorkerStateV1, content_hash},
    model::{Event, StoredEvent},
    mysql_storage::{
        MysqlProbeError,
        audit::{AuditHead, ControlCollision, KnownControlCollision, MysqlAuditStore},
    },
    policy::{self, Policy},
    redaction::Redactor,
    workload::{
        WorkloadBindingIssueV1, WorkloadBindingRevocationV1, WorkloadDigestOwnerV1,
        WorkloadFailureV1, WorkloadSubjectV1,
    },
};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, SecondsFormat, TimeDelta};
use mysql_async::{Conn, Transaction, TxOpts, prelude::Queryable};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

const TENANT_A: &str = "tenant-qa-a";
const TENANT_B: &str = "tenant-qa-b";
const ORIGINAL: [u8; 32] = [0x61; 32];
const ROTATED: [u8; 32] = [0x62; 32];
const ABSENT: [u8; 32] = [0x63; 32];
// Canonical, non-nil and outside the v4 UUIDs generated for normal fixture operations.
const REFUSED_REVOCATION: &str = "00000000-0000-0000-0000-000000000001";
const WORKLOAD_DDL: &str = include_str!("../../schema/mysql/workload-v1.sql");

// Both the input hash and the projected JSON bind the serialized field order.
#[derive(Serialize)]
struct Change<'a, T> {
    action: &'static str,
    input: &'a T,
}

#[derive(Serialize)]
struct Binding {
    version: u32,
    credential_id: String,
    revision: u64,
    subject: WorkloadSubjectV1,
    sha256_digest: String,
    expires_at: String,
    revoked: bool,
    updated_at: String,
}

pub(super) async fn prepare_capability(
    conn: &mut Conn,
    tenant: &str,
    namespace: Uuid,
    expected_head: (i64, &str),
) -> Result<AuditHead> {
    prepare_capability_from_ddl(conn, tenant, namespace, expected_head, WORKLOAD_DDL).await
}

async fn prepare_capability_from_ddl(
    conn: &mut Conn,
    tenant: &str,
    namespace: Uuid,
    expected_head: (i64, &str),
    ddl: &str,
) -> Result<AuditHead> {
    for statement in ddl.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        conn.query_drop(statement).await?;
    }
    let mut tx = conn.start_transaction(TxOpts::new()).await?;
    let mut head = lock_head(&mut tx).await?;
    ensure!((head.seq, head.hash.as_str()) == expected_head);
    let mut event = Event::new("control.workload.genesis");
    event.data = json!({
        "version": 1,
        "tenant_id": tenant,
        "control_namespace_uuid": namespace,
        "previous_seq": head.seq,
        "previous_hash": head.hash,
    });
    let genesis = insert_event(&mut tx, &mut head, event).await?;
    tx.exec_drop(
        "INSERT INTO ai_guard_workload_meta (singleton,genesis_seq,genesis_hash) VALUES (1,?,?)",
        (genesis.seq, &genesis.hash),
    )
    .await?;
    write_head(&mut tx, &head).await?;
    tx.commit().await?;
    Ok(head)
}

async fn append_issue(
    tx: &mut Transaction<'_>,
    head: &mut AuditHead,
    input: &WorkloadBindingIssueV1,
    at: &str,
) -> Result<(StoredEvent, Binding)> {
    input.validate()?;
    let change = Change {
        action: "issue",
        input,
    };
    let input_hash = content_hash("ai-guard:workload-binding-input:v1", &change)?;
    let mut event = Event::new("control.workload.issue");
    event.timestamp = at.into();
    event.data = json!({
        "version": 1,
        "input_hash": input_hash,
        "received_at": at,
        "change": change,
    });
    let stored = insert_event(tx, head, event).await?;
    let binding = Binding {
        version: 1,
        credential_id: input.credential_id.clone(),
        revision: input.expected_revision + 1,
        subject: input.subject.clone(),
        sha256_digest: input.sha256_digest.clone(),
        expires_at: input.expires_at.clone(),
        revoked: false,
        updated_at: at.into(),
    };
    let payload = serde_json::to_vec(&binding)?;
    if input.expected_revision == 0 {
        let subject_key = content_hash("ai-guard:workload-subject:v1", &binding.subject)?;
        tx.exec_drop(
            "INSERT INTO ai_guard_workload_bindings
             (credential_id,revision,sha256_digest,subject_key,payload_json,event_seq)
             VALUES (?,1,?,?,?,?)",
            (
                &binding.credential_id,
                &binding.sha256_digest,
                subject_key,
                &payload,
                stored.seq,
            ),
        )
        .await?;
    } else {
        tx.exec_drop(
            "UPDATE ai_guard_workload_bindings
             SET revision=?,sha256_digest=?,payload_json=?,event_seq=?
             WHERE credential_id=? AND revision=?",
            (
                binding.revision,
                &binding.sha256_digest,
                &payload,
                stored.seq,
                &binding.credential_id,
                input.expected_revision,
            ),
        )
        .await?;
        ensure!(
            tx.affected_rows() == 1,
            "workload QA rotation did not update one binding"
        );
    }
    tx.exec_drop(
        "INSERT INTO ai_guard_workload_binding_history
         (credential_id,revision,payload_json,event_seq) VALUES (?,?,?,?)",
        (
            &binding.credential_id,
            binding.revision,
            payload,
            stored.seq,
        ),
    )
    .await?;
    tx.exec_drop(
        "INSERT INTO ai_guard_workload_digests (sha256_digest,credential_id) VALUES (?,?)",
        (&binding.sha256_digest, &binding.credential_id),
    )
    .await?;
    tx.exec_drop(
        "INSERT INTO ai_guard_workload_operations
         (operation_id,input_hash,event_seq,credential_id,revision) VALUES (?,?,?,?,?)",
        (
            &input.operation_id,
            input_hash,
            stored.seq,
            &binding.credential_id,
            binding.revision,
        ),
    )
    .await?;
    Ok((stored, binding))
}

async fn prepare_rotation(
    conn: &mut Conn,
    reservation_seq: i64,
    worker_id: &str,
    attempt_id: &str,
) -> Result<(AuditHead, StoredEvent, Binding, Binding)> {
    let mut tx = conn.start_transaction(TxOpts::new()).await?;
    let mut head = lock_head(&mut tx).await?;
    ensure!(
        head.seq == reservation_seq,
        "workload QA reservation must be the last event"
    );
    let at: String = tx
        .query_first("SELECT MAX(received_at) FROM ai_guard_audit_event")
        .await?
        .context("workload QA audit time floor is missing")?;
    let expiry = (DateTime::parse_from_rfc3339(&at)? + TimeDelta::hours(1))
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    let first = WorkloadBindingIssueV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: Uuid::new_v4().to_string(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Worker {
            worker_id: worker_id.into(),
            attempt_id: attempt_id.into(),
            generation: 1,
        },
        sha256_digest: hex::encode(ORIGINAL),
        expires_at: expiry,
    };
    let (issued, binding) = append_issue(&mut tx, &mut head, &first, &at).await?;
    tx.exec_drop(
        "INSERT INTO ai_guard_workload_worker_endpoints
         (worker_id,attempt_id,generation,event_seq) VALUES (?,?,1,?)",
        (worker_id, attempt_id, issued.seq),
    )
    .await?;
    let rotation = WorkloadBindingIssueV1 {
        operation_id: Uuid::new_v4().to_string(),
        expected_revision: 1,
        sha256_digest: hex::encode(ROTATED),
        ..first
    };
    let (_, current) = append_issue(&mut tx, &mut head, &rotation, &at).await?;
    write_head(&mut tx, &head).await?;
    tx.commit().await?;
    Ok((head, issued, binding, current))
}

async fn exercise_revocation(
    conn: &mut Conn,
    fixture: &FixtureFiles,
    reader: &MysqlAuditStore,
    expected_head: &AuditHead,
    binding: &Binding,
) -> Result<AuditHead> {
    let open = || {
        fixture.target.open(
            &fixture.target.maintenance_username,
            fixture.credentials(&fixture.maintenance_password)?,
            TENANT_A,
            fixture.target.namespace,
            expected_head.seq + 1,
        )
    };
    let writer = open()?;
    let input = WorkloadBindingRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: binding.credential_id.clone(),
        expected_revision: binding.revision,
    };
    let stale = WorkloadBindingRevocationV1 {
        operation_id: Uuid::new_v4().to_string(),
        expected_revision: 1,
        ..input.clone()
    };
    let prior = fixture_prefix(conn, i64::MAX).await?;
    let redacting = MysqlAuditStore::new(
        fixture
            .target
            .endpoint(&fixture.target.maintenance_username),
        fixture.credentials(&fixture.maintenance_password)?,
        TENANT_A,
        fixture.target.namespace,
        Redactor::new(vec![input.credential_id.clone()]),
        expected_head.seq + 1,
    )?;
    assert_eq!(
        redacting
            .revoke_workload_binding(&input)
            .await
            .unwrap_err()
            .downcast_ref::<WorkloadFailureV1>(),
        Some(&WorkloadFailureV1::InvalidRequest)
    );
    assert_eq!(
        writer
            .revoke_workload_binding(&stale)
            .await
            .unwrap_err()
            .downcast_ref::<WorkloadFailureV1>(),
        Some(&WorkloadFailureV1::Conflict)
    );
    assert_eq!(
        writer
            .append_control_event(Event::new("command.completed"))
            .await
            .unwrap_err()
            .downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted)
    );
    assert_projection_refused(conn, reader, &writer, &input).await?;
    assert_eq!(&writer.verify().await?, expected_head);
    assert_eq!(fixture_prefix(conn, i64::MAX).await?, prior);

    assert_revocation_rollback(conn, &writer, expected_head, &input).await?;

    let receipt = writer.revoke_workload_binding(&input).await?;
    assert_eq!(receipt.credential_id, input.credential_id);
    assert_eq!(receipt.revision, binding.revision + 1);
    assert_eq!(receipt.audit_seq, expected_head.seq + 1);
    assert!(!receipt.replayed);

    let reopened = open()?;
    let approved = reopened.verify().await?;
    assert_eq!(approved.seq, receipt.audit_seq);
    assert_eq!(approved.hash, receipt.audit_hash);
    let events = reopened
        .events(&EventQuery {
            after: Some(expected_head.seq),
            limit: Some(1),
            ..Default::default()
        })
        .await?;
    let stored = events
        .first()
        .context("revocation is absent from the audit")?;
    assert_eq!(stored.event.kind, "control.workload.revoke");
    assert_eq!(stored.event.id, receipt.audit_event_id);
    assert_eq!(stored.hash, receipt.audit_hash);
    assert_eq!(stored.prev_hash, expected_head.hash);

    let different = WorkloadBindingRevocationV1 {
        expected_revision: 1,
        ..input.clone()
    };
    assert_eq!(
        reopened
            .revoke_workload_binding(&different)
            .await
            .unwrap_err()
            .downcast_ref::<WorkloadFailureV1>(),
        Some(&WorkloadFailureV1::Conflict)
    );
    let replay = reopened.revoke_workload_binding(&input).await?;
    assert_eq!(
        replay,
        agentsandbox::workload::WorkloadBindingReceiptV1 {
            replayed: true,
            ..receipt
        }
    );
    assert_eq!(reopened.verify().await?, approved);
    assert_eq!(fixture_prefix(conn, expected_head.seq).await?, prior);
    Ok(approved)
}

type BindingRow = (String, i64, String, String, Vec<u8>, i64);
type HistoryRow = (String, i64, Vec<u8>, i64);
type OperationRow = (String, String, i64, String, i64);

#[derive(Debug, PartialEq, Eq)]
struct RevocationRows {
    bindings: Vec<BindingRow>,
    history: Vec<HistoryRow>,
    operations: Vec<OperationRow>,
}

async fn revocation_rows(conn: &mut Conn) -> Result<RevocationRows> {
    Ok(RevocationRows {
        bindings: conn
            .query(
                "SELECT credential_id,revision,sha256_digest,subject_key,payload_json,event_seq
                 FROM ai_guard_workload_bindings ORDER BY credential_id",
            )
            .await?,
        history: conn
            .query(
                "SELECT credential_id,revision,payload_json,event_seq
                 FROM ai_guard_workload_binding_history ORDER BY credential_id,revision",
            )
            .await?,
        operations: conn
            .query(
                "SELECT operation_id,input_hash,event_seq,credential_id,revision
                 FROM ai_guard_workload_operations ORDER BY operation_id",
            )
            .await?,
    })
}

async fn assert_revocation_rollback(
    conn: &mut Conn,
    writer: &MysqlAuditStore,
    expected_head: &AuditHead,
    accepted: &WorkloadBindingRevocationV1,
) -> Result<()> {
    let refused = WorkloadBindingRevocationV1 {
        operation_id: REFUSED_REVOCATION.into(),
        ..accepted.clone()
    };
    let journal = fixture_prefix(conn, i64::MAX).await?;
    let projection = revocation_rows(conn).await?;
    assert!(
        !projection
            .operations
            .iter()
            .any(|row| row.0 == REFUSED_REVOCATION)
    );

    // The receipt is inserted after the audit event, current binding and history.
    let error = writer
        .revoke_workload_binding(&refused)
        .await
        .expect_err("receipt constraint must abort the revocation");
    assert_eq!(
        error.downcast_ref::<MysqlProbeError>(),
        Some(&MysqlProbeError::ServerRefused(3819))
    );
    assert_eq!(fixture_prefix(conn, i64::MAX).await?, journal);
    assert_eq!(revocation_rows(conn).await?, projection);
    assert_eq!(&writer.verify().await?, expected_head);
    Ok(())
}

async fn assert_projection_refused(
    conn: &mut Conn,
    reader: &MysqlAuditStore,
    writer: &MysqlAuditStore,
    revocation: &WorkloadBindingRevocationV1,
) -> Result<()> {
    let change_digest =
        "UPDATE ai_guard_workload_digests SET sha256_digest=? WHERE sha256_digest=?";
    conn.exec_drop(change_digest, (hex::encode(ABSENT), hex::encode(ORIGINAL)))
        .await?;
    ensure!(
        conn.affected_rows() == 1,
        "workload QA changed the wrong projection"
    );
    let refused = reader.has_workload_credential_binding(&ORIGINAL).await;
    let owner_refused = reader.workload_digest_binding(&ORIGINAL).await;
    let other_owner_refused = reader.workload_digest_binding(&ROTATED).await;
    let unknown_owner_refused = reader.workload_digest_binding(&ABSENT).await;
    let revocation_refused = writer.revoke_workload_binding(revocation).await;
    conn.exec_drop(change_digest, (hex::encode(ORIGINAL), hex::encode(ABSENT)))
        .await?;
    ensure!(
        conn.affected_rows() == 1,
        "workload QA failed to restore the projection"
    );
    let error = refused
        .err()
        .context("divergent workload projection was accepted")?;
    let expected = "MySQL workload projection differs from audit: workload_digests";
    ensure!(error.to_string() == expected);
    ensure!(owner_refused.unwrap_err().to_string() == expected);
    ensure!(other_owner_refused.unwrap_err().to_string() == expected);
    ensure!(unknown_owner_refused.unwrap_err().to_string() == expected);
    ensure!(revocation_refused.unwrap_err().to_string() == expected);
    Ok(())
}

async fn prepare_issue_before_worker(
    conn: &mut Conn,
    approved_head: &AuditHead,
    slot: &StoredEvent,
    issued: &StoredEvent,
    mut binding: Binding,
) -> Result<()> {
    let mut tx = conn.start_transaction(TxOpts::new()).await?;
    let mut head = lock_head(&mut tx).await?;
    ensure!(
        &head == approved_head,
        "workload QA approved history changed"
    );
    // The decision precedes this suffix. Preserve its receipt and the order and
    // projections of the real worker reservation while moving the first Issue.
    let tail: Vec<(i64, String, String, String)> = tx
        .exec(
            "SELECT seq,received_at,hash,payload FROM ai_guard_audit_event WHERE seq>=? ORDER BY seq",
            (slot.seq,),
        )
        .await?;
    ensure!(
        tail.iter().map(|row| row.0).eq(slot.seq..=head.seq)
            && issued.seq == slot.seq + 2
            && head.seq == issued.seq + 2,
        "workload QA needs the slot, reservation, issue, rotation and revocation in that order"
    );
    let prior_max: String = tx
        .exec_first(
            "SELECT MAX(received_at) FROM ai_guard_audit_event WHERE seq<?",
            (slot.seq,),
        )
        .await?
        .context("workload QA early issue time floor is missing")?;
    let early_time = slot.received_at.clone().max(prior_max);
    binding.updated_at.clone_from(&early_time);
    head.hash.clone_from(&slot.prev_hash);
    for (seq, mut received_at, original_hash, original_payload) in tail {
        let mut event: Event = serde_json::from_str(&original_payload)?;
        if seq == slot.seq {
            event.kind.clone_from(&issued.event.kind);
            event.data.clone_from(&issued.event.data);
            *event
                .data
                .get_mut("received_at")
                .context("workload QA issue has no control time")? = json!(early_time);
            received_at.clone_from(&early_time);
        } else if seq == issued.seq {
            event.kind.clone_from(&slot.event.kind);
            event.data.clone_from(&slot.event.data);
        }
        let mut updated = StoredEvent {
            seq,
            received_at,
            prev_hash: head.hash.clone(),
            hash: String::new(),
            event,
        };
        updated.hash = expected_hash(&updated)?;
        let payload = serde_json::to_string(&updated.event)?;
        tx.exec_drop(
            "UPDATE ai_guard_audit_event SET received_at=?,prev_hash=?,hash=?,payload=?
             WHERE seq=? AND hash=?",
            (
                &updated.received_at,
                &updated.prev_hash,
                &updated.hash,
                &payload,
                seq,
                original_hash,
            ),
        )
        .await?;
        ensure!(
            tx.affected_rows() == 1,
            "workload QA suffix was not recomposed"
        );
        head.hash = updated.hash;
        head.payload_bytes = head
            .payload_bytes
            .checked_sub(original_payload.len() as u64)
            .and_then(|remaining| remaining.checked_add(payload.len() as u64))
            .context("workload QA recomposed payload byte count is invalid")?;
    }
    tx.exec_drop(
        "UPDATE ai_guard_workload_binding_history SET event_seq=?,payload_json=?
         WHERE event_seq=? AND revision=1",
        (slot.seq, serde_json::to_vec(&binding)?, issued.seq),
    )
    .await?;
    ensure!(
        tx.affected_rows() == 1,
        "workload QA early binding history was not updated"
    );
    for statement in [
        "UPDATE ai_guard_workload_operations SET event_seq=? WHERE event_seq=? AND revision=1",
        "UPDATE ai_guard_workload_worker_endpoints SET event_seq=? WHERE event_seq=?",
    ] {
        tx.exec_drop(statement, (slot.seq, issued.seq)).await?;
        ensure!(
            tx.affected_rows() == 1,
            "workload QA early projection was not updated"
        );
    }
    write_head(&mut tx, &head).await?;
    tx.commit().await?;
    Ok(())
}

fn maintenance_store(fixture: &FixtureFiles, tenant: &str) -> Result<MysqlAuditStore> {
    fixture.target.open(
        &fixture.target.maintenance_username,
        fixture.credentials(&fixture.maintenance_password)?,
        tenant,
        fixture.target.namespace,
        128,
    )
}

#[tokio::test]
#[ignore = "requires two fresh prepared private MySQL 8.4 QA schemas and a SELECT-only reader"]
async fn prepared_workload_binding_requires_verified_history() -> Result<()> {
    let a = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let b = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
    ensure!(
        a.target.port == b.target.port && a.target.namespace != b.target.namespace,
        "workload QA requires separate schemas on one loopback instance"
    );
    let mut conn_a = a.maintenance().await?;
    let mut conn_b = b.maintenance().await?;
    require_fresh(&mut conn_a).await?;
    require_fresh(&mut conn_b).await?;
    let writer = maintenance_store(&a, TENANT_A)?;
    let initialized_a = writer.initialize().await?;
    let initialized_b = maintenance_store(&b, TENANT_B)?.initialize().await?;
    let reader_a = a.reader(TENANT_A, a.target.namespace)?;
    let reader_b = b.reader(TENANT_B, b.target.namespace)?;
    let missing = reader_a.has_workload_credential_binding(&ORIGINAL).await;
    ensure!(
        missing
            .err()
            .and_then(|err| err.downcast::<WorkloadFailureV1>().ok())
            == Some(WorkloadFailureV1::MigrationRequired),
        "an unprepared workload schema cannot prove absence"
    );
    assert_eq!(
        reader_a
            .workload_digest_binding(&ORIGINAL)
            .await
            .unwrap_err()
            .downcast_ref::<WorkloadFailureV1>(),
        Some(&WorkloadFailureV1::MigrationRequired)
    );
    // Add the fault only to A while creating its private fixture schema.
    // The DDL and existing grants of every other schema remain unchanged.
    let operation_column = "CREATE TABLE ai_guard_workload_operations (\n    operation_id BINARY(36) NOT NULL PRIMARY KEY,";
    ensure!(WORKLOAD_DDL.matches(operation_column).count() == 1);
    let qa_workload_ddl = WORKLOAD_DDL.replacen(
        operation_column,
        &format!(
            "{operation_column}\n    CONSTRAINT ai_guard_qa_refuse_revocation_receipt CHECK (operation_id <> X'{}'),",
            hex::encode(REFUSED_REVOCATION),
        ),
        1,
    );
    prepare_capability_from_ddl(
        &mut conn_a,
        TENANT_A,
        a.target.namespace,
        (initialized_a.seq, &initialized_a.hash),
        &qa_workload_ddl,
    )
    .await?;
    let head_b = prepare_capability(
        &mut conn_b,
        TENANT_B,
        b.target.namespace,
        (initialized_b.seq, &initialized_b.hash),
    )
    .await?;
    let prefix_b = fixture_prefix(&mut conn_b, i64::MAX).await?;
    ensure!(!reader_b.has_workload_credential_binding(&ORIGINAL).await?);

    let policy = Policy::default();
    let quota = QuotaPolicyV1 {
        max_active_workers: 1,
        ..Default::default()
    };
    let (admission, decision) = worker::admission(&writer, &policy).await?;
    let slot = writer
        .append(Event::new("qa.workload.before-reservation"))
        .await?;
    let admitted = writer
        .admit_worker(
            &admission,
            &policy,
            policy::evaluate(&policy, &admission.decision_request),
            json!(null),
            &quota,
        )
        .await?;
    let worker = admitted
        .worker
        .context("workload QA reservation was refused")?;
    ensure!(
        worker.state == WorkerStateV1::Reserved
            && worker.quota_held
            && decision.seq + 1 == slot.seq
            && slot.seq + 1 == admitted.receipt.audit_seq
    );
    let (rotated_head, issued, binding, current) = prepare_rotation(
        &mut conn_a,
        admitted.receipt.audit_seq,
        &worker.worker_id,
        &worker.attempt.attempt_id,
    )
    .await?;
    let reopened = a.reader(TENANT_A, a.target.namespace)?;
    let owner = WorkloadDigestOwnerV1 {
        credential_id: binding.credential_id.clone(),
        subject: WorkloadSubjectV1::Worker {
            worker_id: worker.worker_id,
            attempt_id: worker.attempt.attempt_id,
            generation: 1,
        },
    };
    ensure!(reopened.has_workload_credential_binding(&ORIGINAL).await?);
    ensure!(reopened.has_workload_credential_binding(&ROTATED).await?);
    ensure!(!reopened.has_workload_credential_binding(&ABSENT).await?);
    assert_eq!(
        reopened.workload_digest_binding(&ROTATED).await?,
        Some(owner.clone())
    );
    assert_eq!(
        reopened.workload_digest_binding(&ORIGINAL).await?,
        Some(owner.clone())
    );
    assert!(reopened.workload_digest_binding(&ABSENT).await?.is_none());
    ensure!(reopened.verify().await? == rotated_head);

    let approved = exercise_revocation(&mut conn_a, &a, &reopened, &rotated_head, &current).await?;
    let reopened = a.reader(TENANT_A, a.target.namespace)?;
    assert_eq!(
        reopened.workload_digest_binding(&ROTATED).await?,
        Some(owner.clone())
    );
    assert_eq!(
        reopened.workload_digest_binding(&ORIGINAL).await?,
        Some(owner)
    );
    assert!(reopened.has_workload_credential_binding(&ROTATED).await?);
    ensure!(reopened.verify().await? == approved);
    ensure!(reopened.has_workload_credential_binding(&ORIGINAL).await?);

    prepare_issue_before_worker(&mut conn_a, &approved, &slot, &issued, binding).await?;
    let cold = a.reader(TENANT_A, a.target.namespace)?;
    let refused = cold.has_workload_credential_binding(&ORIGINAL).await;
    ensure!(
        refused
            .err()
            .and_then(|err| err.downcast::<WorkloadFailureV1>().ok())
            == Some(WorkloadFailureV1::Unauthorized),
        "a worker reservation later in the verified journal cannot authorize an earlier issue"
    );
    ensure!(!reader_b.has_workload_credential_binding(&ORIGINAL).await?);
    ensure!(!reader_b.has_workload_credential_binding(&ROTATED).await?);
    assert!(reader_b.workload_digest_binding(&ORIGINAL).await?.is_none());
    assert!(reader_b.workload_digest_binding(&ROTATED).await?.is_none());
    ensure!(reader_b.verify().await? == head_b);
    ensure!(fixture_prefix(&mut conn_b, i64::MAX).await? == prefix_b);
    Ok(())
}

#[tokio::test]
#[ignore = "requires two fresh prepared private MySQL 8.4 QA schemas and a SELECT-only reader"]
async fn prepared_workload_issue_preserves_history_and_replays() -> Result<()> {
    let a = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let b = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
    ensure!(a.target.port == b.target.port && a.target.namespace != b.target.namespace);
    let mut conn_a = a.maintenance().await?;
    let mut conn_b = b.maintenance().await?;
    require_fresh(&mut conn_a).await?;
    require_fresh(&mut conn_b).await?;
    let writer = maintenance_store(&a, TENANT_A)?;
    let genesis_a = writer.initialize().await?;
    let genesis_b = maintenance_store(&b, TENANT_B)?.initialize().await?;
    prepare_capability(
        &mut conn_a,
        TENANT_A,
        a.target.namespace,
        (genesis_a.seq, &genesis_a.hash),
    )
    .await?;
    let head_b = prepare_capability(
        &mut conn_b,
        TENANT_B,
        b.target.namespace,
        (genesis_b.seq, &genesis_b.hash),
    )
    .await?;
    let prefix_b = fixture_prefix(&mut conn_b, i64::MAX).await?;

    let policy = Policy::default();
    let (admission, _) = worker::admission(&writer, &policy).await?;
    let admitted = writer
        .admit_worker(
            &admission,
            &policy,
            policy::evaluate(&policy, &admission.decision_request),
            json!(null),
            &QuotaPolicyV1::default(),
        )
        .await?;
    let worker = admitted
        .worker
        .context("workload issue QA reservation was refused")?;
    let before = writer.verify().await?;
    let prefix_a = fixture_prefix(&mut conn_a, i64::MAX).await?;
    let expires_at = chrono::Utc::now() + TimeDelta::hours(1);
    let issue = WorkloadBindingIssueV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: Uuid::new_v4().to_string(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Worker {
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            generation: 1,
        },
        sha256_digest: hex::encode(ORIGINAL),
        expires_at: expires_at.to_rfc3339_opts(SecondsFormat::Millis, true),
    };
    let wrong_attempt = WorkloadBindingIssueV1 {
        subject: WorkloadSubjectV1::Worker {
            worker_id: worker.worker_id.clone(),
            attempt_id: Uuid::new_v4().to_string(),
            generation: 1,
        },
        ..issue.clone()
    };
    assert_eq!(
        writer
            .issue_workload_binding(&wrong_attempt)
            .await
            .unwrap_err()
            .downcast_ref::<WorkloadFailureV1>(),
        Some(&WorkloadFailureV1::Unauthorized)
    );
    assert_eq!(writer.verify().await?, before);

    let rotate = WorkloadBindingIssueV1 {
        operation_id: Uuid::new_v4().to_string(),
        expected_revision: 1,
        sha256_digest: hex::encode(ROTATED),
        ..issue.clone()
    };
    let renew = WorkloadBindingIssueV1 {
        operation_id: Uuid::new_v4().to_string(),
        expected_revision: 2,
        expires_at: (expires_at + TimeDelta::hours(1)).to_rfc3339_opts(SecondsFormat::Millis, true),
        ..rotate.clone()
    };
    let first = writer.issue_workload_binding(&issue).await?;
    let rotated = writer.issue_workload_binding(&rotate).await?;
    let renewed = writer.issue_workload_binding(&renew).await?;
    let reopened = maintenance_store(&a, TENANT_A)?;
    let events = reopened
        .events(&EventQuery {
            after: Some(before.seq),
            limit: Some(3),
            order: Some("asc".into()),
            ..Default::default()
        })
        .await?;
    assert_eq!(events.len(), 3);
    let mut previous_hash = &before.hash;
    for (index, (stored, receipt)) in events.iter().zip([&first, &rotated, &renewed]).enumerate() {
        assert_eq!(stored.event.kind, "control.workload.issue");
        assert_eq!(stored.seq, before.seq + index as i64 + 1);
        assert_eq!(&stored.prev_hash, previous_hash);
        assert_eq!(stored.hash, receipt.audit_hash);
        assert_eq!(stored.event.id, receipt.audit_event_id);
        assert_eq!(stored.seq, receipt.audit_seq);
        assert_eq!(receipt.credential_id, issue.credential_id);
        assert_eq!(receipt.revision, index as u64 + 1);
        assert!(!receipt.replayed);
        previous_hash = &stored.hash;
    }
    let head = reopened.verify().await?;
    assert_eq!(
        (head.seq, &head.hash),
        (renewed.audit_seq, &renewed.audit_hash)
    );
    let owner = WorkloadDigestOwnerV1 {
        credential_id: issue.credential_id.clone(),
        subject: issue.subject.clone(),
    };
    assert_eq!(
        reopened.workload_digest_binding(&ORIGINAL).await?,
        Some(owner.clone())
    );
    assert_eq!(
        reopened.workload_digest_binding(&ROTATED).await?,
        Some(owner)
    );
    let endpoints: Vec<(String, String, i64, i64)> = conn_a.query(
        "SELECT worker_id,attempt_id,generation,event_seq FROM ai_guard_workload_worker_endpoints",
    ).await?;
    assert_eq!(
        endpoints,
        [(
            worker.worker_id,
            worker.attempt.attempt_id,
            1,
            first.audit_seq
        )]
    );
    assert_eq!(
        conn_a
            .query_first::<u64, _>("SELECT COUNT(*) FROM ai_guard_workload_digests")
            .await?,
        Some(2)
    );
    let replay = reopened.issue_workload_binding(&issue).await?;
    assert_eq!(
        replay,
        agentsandbox::workload::WorkloadBindingReceiptV1 {
            replayed: true,
            ..first
        }
    );

    let stale = WorkloadBindingIssueV1 {
        operation_id: Uuid::new_v4().to_string(),
        ..renew
    };
    let divergent = WorkloadBindingIssueV1 {
        expected_revision: renewed.revision,
        sha256_digest: hex::encode(ABSENT),
        ..issue.clone()
    };
    let no_capacity = WorkloadBindingIssueV1 {
        operation_id: Uuid::new_v4().to_string(),
        ..divergent.clone()
    };
    let historical = WorkloadBindingIssueV1 {
        operation_id: Uuid::new_v4().to_string(),
        expected_revision: renewed.revision,
        ..issue
    };
    for request in [stale, historical, divergent] {
        assert_eq!(
            reopened
                .issue_workload_binding(&request)
                .await
                .unwrap_err()
                .downcast_ref::<WorkloadFailureV1>(),
            Some(&WorkloadFailureV1::Conflict)
        );
    }
    let limited = a.target.open(
        &a.target.maintenance_username,
        a.credentials(&a.maintenance_password)?,
        TENANT_A,
        a.target.namespace,
        head.seq + 1,
    )?;
    assert_eq!(
        limited
            .issue_workload_binding(&no_capacity)
            .await
            .unwrap_err()
            .downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted)
    );
    assert_eq!(reopened.verify().await?, head);
    assert_eq!(fixture_prefix(&mut conn_a, before.seq).await?, prefix_a);
    let reader_b = b.reader(TENANT_B, b.target.namespace)?;
    assert!(reader_b.workload_digest_binding(&ORIGINAL).await?.is_none());
    assert!(reader_b.workload_digest_binding(&ROTATED).await?.is_none());
    assert_eq!(reader_b.verify().await?, head_b);
    assert_eq!(fixture_prefix(&mut conn_b, i64::MAX).await?, prefix_b);
    Ok(())
}

#[tokio::test]
#[ignore = "requires two fresh prepared private MySQL 8.4 QA schemas and a SELECT-only reader"]
async fn prepared_workload_broker_issue_preserves_subject_and_replays() -> Result<()> {
    let a = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let b = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
    ensure!(a.target.port == b.target.port && a.target.namespace != b.target.namespace);
    let mut conn_a = a.maintenance().await?;
    let mut conn_b = b.maintenance().await?;
    require_fresh(&mut conn_a).await?;
    require_fresh(&mut conn_b).await?;
    let writer = maintenance_store(&a, TENANT_A)?;
    let genesis_a = writer.initialize().await?;
    let genesis_b = maintenance_store(&b, TENANT_B)?.initialize().await?;
    let head_a = prepare_capability(
        &mut conn_a,
        TENANT_A,
        a.target.namespace,
        (genesis_a.seq, &genesis_a.hash),
    )
    .await?;
    let head_b = prepare_capability(
        &mut conn_b,
        TENANT_B,
        b.target.namespace,
        (genesis_b.seq, &genesis_b.hash),
    )
    .await?;
    let prefix_a = fixture_prefix(&mut conn_a, i64::MAX).await?;
    let prefix_b = fixture_prefix(&mut conn_b, i64::MAX).await?;
    let rows_b = revocation_rows(&mut conn_b).await?;

    let broker_id = Uuid::new_v4().to_string();
    let issue = WorkloadBindingIssueV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: Uuid::new_v4().to_string(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Broker {
            broker_id: broker_id.clone(),
            incarnation_id: Uuid::new_v4().to_string(),
        },
        sha256_digest: hex::encode(ORIGINAL),
        expires_at: (chrono::Utc::now() + TimeDelta::hours(1))
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    };
    let issued = writer.issue_workload_binding(&issue).await?;
    assert_eq!(issued.credential_id, issue.credential_id);
    assert_eq!(issued.revision, 1);
    assert_eq!(issued.audit_seq, head_a.seq + 1);
    assert!(!issued.replayed);

    let reader = a.reader(TENANT_A, a.target.namespace)?;
    let events = reader
        .events(&EventQuery {
            after: Some(head_a.seq),
            limit: Some(2),
            ..Default::default()
        })
        .await?;
    assert_eq!(events.len(), 1);
    let stored = &events[0];
    assert_eq!(stored.event.kind, "control.workload.issue");
    assert_eq!(stored.event.id, issued.audit_event_id);
    assert_eq!(stored.seq, issued.audit_seq);
    assert_eq!(stored.hash, issued.audit_hash);
    assert_eq!(stored.prev_hash, head_a.hash);
    assert_eq!(
        stored.event.data["change"]["input"],
        serde_json::to_value(&issue)?
    );
    let head = reader.verify().await?;
    assert_eq!(
        (head.seq, &head.hash),
        (issued.audit_seq, &issued.audit_hash)
    );
    assert_eq!(
        reader.workload_digest_binding(&ORIGINAL).await?,
        Some(WorkloadDigestOwnerV1 {
            credential_id: issue.credential_id.clone(),
            subject: issue.subject.clone(),
        })
    );

    let payload = serde_json::to_vec(&Binding {
        version: 1,
        credential_id: issue.credential_id.clone(),
        revision: 1,
        subject: issue.subject.clone(),
        sha256_digest: issue.sha256_digest.clone(),
        expires_at: issue.expires_at.clone(),
        revoked: false,
        updated_at: stored.event.timestamp.clone(),
    })?;
    let subject_key = content_hash("ai-guard:workload-subject:v1", &issue.subject)?;
    let input_hash = content_hash(
        "ai-guard:workload-binding-input:v1",
        &Change {
            action: "issue",
            input: &issue,
        },
    )?;
    let rows_a = revocation_rows(&mut conn_a).await?;
    assert_eq!(
        rows_a.bindings,
        [(
            issue.credential_id.clone(),
            1,
            issue.sha256_digest.clone(),
            subject_key,
            payload.clone(),
            issued.audit_seq
        )]
    );
    assert_eq!(
        rows_a.history,
        [(issue.credential_id.clone(), 1, payload, issued.audit_seq)]
    );
    assert_eq!(
        rows_a.operations,
        [(
            issue.operation_id.clone(),
            input_hash,
            issued.audit_seq,
            issue.credential_id.clone(),
            1
        )]
    );
    assert_eq!(
        conn_a
            .query_first::<(u64, u64), _>(
                "SELECT (SELECT COUNT(*) FROM ai_guard_worker),
                    (SELECT COUNT(*) FROM ai_guard_workload_worker_endpoints)",
            )
            .await?,
        Some((0, 0))
    );
    let digests: Vec<(String, String)> = conn_a
        .query("SELECT sha256_digest,credential_id FROM ai_guard_workload_digests")
        .await?;
    assert_eq!(
        digests,
        [(issue.sha256_digest.clone(), issue.credential_id.clone())]
    );

    // These requests pass the shape validation, but try to change durable ownership.
    let other_incarnation = WorkloadBindingIssueV1 {
        operation_id: Uuid::new_v4().to_string(),
        expected_revision: 1,
        subject: WorkloadSubjectV1::Broker {
            broker_id,
            incarnation_id: Uuid::new_v4().to_string(),
        },
        sha256_digest: hex::encode(ROTATED),
        ..issue.clone()
    };
    let other_credential = WorkloadBindingIssueV1 {
        operation_id: Uuid::new_v4().to_string(),
        credential_id: Uuid::new_v4().to_string(),
        ..issue.clone()
    };
    let reopened = maintenance_store(&a, TENANT_A)?;
    for conflicting in [other_incarnation, other_credential] {
        assert_eq!(
            reopened
                .issue_workload_binding(&conflicting)
                .await
                .unwrap_err()
                .downcast_ref::<WorkloadFailureV1>(),
            Some(&WorkloadFailureV1::Conflict)
        );
    }
    assert_eq!(
        reopened.issue_workload_binding(&issue).await?,
        agentsandbox::workload::WorkloadBindingReceiptV1 {
            replayed: true,
            ..issued
        }
    );
    assert_eq!(reopened.verify().await?, head);
    assert_eq!(fixture_prefix(&mut conn_a, head_a.seq).await?, prefix_a);
    assert_eq!(revocation_rows(&mut conn_a).await?, rows_a);

    let reader_b = b.reader(TENANT_B, b.target.namespace)?;
    assert!(reader_b.workload_digest_binding(&ORIGINAL).await?.is_none());
    assert_eq!(reader_b.verify().await?, head_b);
    assert_eq!(fixture_prefix(&mut conn_b, i64::MAX).await?, prefix_b);
    assert_eq!(revocation_rows(&mut conn_b).await?, rows_b);
    Ok(())
}

#[tokio::test]
#[ignore = "requires two fresh prepared private MySQL 8.4 QA schemas and a SELECT-only reader"]
async fn checked_credential_presence_retains_known_collisions_during_projection_failure()
-> Result<()> {
    const HUMAN_A: [u8; 32] = [0x64; 32];
    const HUMAN_B: [u8; 32] = [0x65; 32];
    let candidates = [HUMAN_B, ORIGINAL, ABSENT];
    let a = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let b = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
    ensure!(a.target.namespace != b.target.namespace);
    let mut conn_a = a.maintenance().await?;
    let mut conn_b = b.maintenance().await?;
    require_fresh(&mut conn_a).await?;
    require_fresh(&mut conn_b).await?;

    let initial_a = maintenance_store(&a, TENANT_A)?.initialize().await?;
    let initial_b = maintenance_store(&b, TENANT_B)?.initialize().await?;
    let oversized_candidates = [[0u8; 32]; 513];
    ensure!(
        a.reader(TENANT_A, a.target.namespace)?
            .with_control_collision_candidates(&oversized_candidates)
            .is_err()
    );
    let reader_a = a
        .reader(TENANT_A, a.target.namespace)?
        .with_control_collision_candidates(&candidates)?;
    let reader_b = b
        .reader(TENANT_B, b.target.namespace)?
        .with_control_collision_candidates(&candidates)?;

    let (_, identity_a) =
        prepare_identity(&mut conn_a, a.target.namespace, &initial_a, &HUMAN_A).await?;
    ensure!(
        reader_a
            .verify_control_credential_presence(&HUMAN_A)
            .await
            .err()
            .and_then(|error| error.downcast::<WorkloadFailureV1>().ok())
            == Some(WorkloadFailureV1::MigrationRequired)
    );
    let head_a = prepare_capability(
        &mut conn_a,
        TENANT_A,
        a.target.namespace,
        (identity_a.seq, &identity_a.hash),
    )
    .await?;
    let (human_b, identity_b) =
        prepare_identity(&mut conn_b, b.target.namespace, &initial_b, &HUMAN_B).await?;
    let revoked_b = revoke_identity(&mut conn_b, &identity_b, &human_b).await?;
    prepare_capability(
        &mut conn_b,
        TENANT_B,
        b.target.namespace,
        (revoked_b.seq, &revoked_b.hash),
    )
    .await?;

    let mut tx = conn_b.start_transaction(TxOpts::new()).await?;
    let mut head_b = lock_head(&mut tx).await?;
    let at: String = tx
        .query_first("SELECT MAX(received_at) FROM ai_guard_audit_event")
        .await?
        .context("workload QA audit time floor is missing")?;
    let input = WorkloadBindingIssueV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: Uuid::new_v4().to_string(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Broker {
            broker_id: Uuid::new_v4().to_string(),
            incarnation_id: Uuid::new_v4().to_string(),
        },
        sha256_digest: hex::encode(ORIGINAL),
        expires_at: (DateTime::parse_from_rfc3339(&at)? + TimeDelta::hours(1))
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    };
    append_issue(&mut tx, &mut head_b, &input, &at).await?;
    append_issue(
        &mut tx,
        &mut head_b,
        &WorkloadBindingIssueV1 {
            operation_id: Uuid::new_v4().to_string(),
            expected_revision: 1,
            sha256_digest: hex::encode(ROTATED),
            ..input
        },
        &at,
    )
    .await?;
    write_head(&mut tx, &head_b).await?;
    tx.commit().await?;

    let prefix_a = fixture_prefix(&mut conn_a, i64::MAX).await?;
    let prefix_b = fixture_prefix(&mut conn_b, i64::MAX).await?;
    // Preflight verifies B without a lookup for any particular digest.
    ensure!(reader_b.known_control_collision(&HUMAN_B) == KnownControlCollision::Unknown);
    ensure!(reader_b.verify().await? == head_b);
    ensure!(
        reader_b.known_control_collision(&HUMAN_B)
            == KnownControlCollision::Known(ControlCollision::Human)
    );
    ensure!(
        reader_b.known_control_collision(&ORIGINAL)
            == KnownControlCollision::Known(ControlCollision::Workload)
    );
    ensure!(reader_b.known_control_collision(&ROTATED) == KnownControlCollision::Unknown);

    let change_digest =
        "UPDATE ai_guard_workload_digests SET sha256_digest=? WHERE sha256_digest=?";
    conn_b
        .exec_drop(change_digest, (hex::encode(ABSENT), hex::encode(ORIGINAL)))
        .await?;
    ensure!(conn_b.affected_rows() == 1);
    let during_failure = async {
        let error = reader_b
            .verify_control_credential_presence(&HUMAN_B)
            .await
            .err()
            .context("fresh presence must refuse a divergent workload projection")?;
        ensure!(
            error.to_string() == "MySQL workload projection differs from audit: workload_digests"
        );
        ensure!(
            reader_b.known_control_collision(&HUMAN_B)
                == KnownControlCollision::Known(ControlCollision::Human)
        );
        ensure!(
            reader_b.known_control_collision(&ORIGINAL)
                == KnownControlCollision::Known(ControlCollision::Workload)
        );
        ensure!(reader_b.known_control_collision(&ABSENT) == KnownControlCollision::Unknown);
        let cold_b = b
            .reader(TENANT_B, b.target.namespace)?
            .with_control_collision_candidates(&candidates)?;
        ensure!(
            cold_b
                .verify_control_credential_presence(&ABSENT)
                .await
                .is_err()
        );
        for digest in &candidates {
            ensure!(cold_b.known_control_collision(digest) == KnownControlCollision::Unknown);
        }
        ensure!(
            reader_a
                .verify_control_credential_presence(&ORIGINAL)
                .await?
                .is_none()
        );
        ensure!(reader_a.known_control_collision(&ORIGINAL) == KnownControlCollision::Unknown);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    conn_b
        .exec_drop(change_digest, (hex::encode(ORIGINAL), hex::encode(ABSENT)))
        .await?;
    ensure!(conn_b.affected_rows() == 1);
    during_failure?;

    ensure!(
        reader_b
            .verify_control_credential_presence(&HUMAN_B)
            .await?
            == Some(ControlCollision::Human)
    );
    ensure!(
        reader_b
            .verify_control_credential_presence(&ORIGINAL)
            .await?
            == Some(ControlCollision::Workload)
    );
    ensure!(
        reader_b
            .verify_control_credential_presence(&ABSENT)
            .await?
            .is_none()
    );
    ensure!(reader_a.verify().await? == head_a && reader_b.verify().await? == head_b);
    ensure!(fixture_prefix(&mut conn_a, i64::MAX).await? == prefix_a);
    ensure!(fixture_prefix(&mut conn_b, i64::MAX).await? == prefix_b);
    Ok(())
}
