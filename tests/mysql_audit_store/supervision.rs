use agentsandbox::{
    audit::EventQuery,
    control::{ControlFailure, QuotaPolicyV1},
    mysql_storage::{MysqlProbeError, audit::MysqlAuditStore},
    policy::{self, Policy},
};
use anyhow::Context;
use mysql_async::{Conn, prelude::Queryable};
use sha2::{Digest, Sha256};

const TABLES: [&str; 10] = [
    "ai_guard_audit_state",
    "ai_guard_audit_event",
    "ai_guard_audit_decision",
    "ai_guard_worker",
    "ai_guard_worker_attempt",
    "ai_guard_worker_mutation",
    "ai_guard_worker_observation",
    "ai_guard_worker_allocation",
    "ai_guard_worker_supervision",
    "ai_guard_supervision_meta",
];

pub(super) async fn exercise(
    writer: &MysqlAuditStore,
    bootstrapper: &MysqlAuditStore,
    reopened: &MysqlAuditStore,
    mut maintenance: Conn,
) -> anyhow::Result<()> {
    let initial = fingerprint(&mut maintenance).await?;
    assert_empty(&mut maintenance).await?;

    // The ordinary writer can read the origin projection but cannot establish it.
    let writer_failure = writer
        .initialize_supervised()
        .await
        .expect_err("qa_writer must not bootstrap the supervision boundary");
    super::assert_matches_privilege_denied(&writer_failure)?;
    anyhow::ensure!(fingerprint(&mut maintenance).await? == initial);

    maintenance
        .query_drop(
            "ALTER TABLE ai_guard_supervision_meta \
             ADD CONSTRAINT ai_guard_qa_block_supervision_origin CHECK (genesis_seq < 0)",
        )
        .await?;
    let failed = bootstrapper.initialize_supervised().await;
    maintenance
        .query_drop(
            "ALTER TABLE ai_guard_supervision_meta \
             DROP CHECK ai_guard_qa_block_supervision_origin",
        )
        .await?;
    let error = failed.expect_err("supervision projection CHECK must abort both genesis events");
    anyhow::ensure!(
        error.downcast_ref::<MysqlProbeError>() == Some(&MysqlProbeError::ServerRefused(3819)),
        "bootstrap fault returned unexpected error: {error}"
    );
    anyhow::ensure!(fingerprint(&mut maintenance).await? == initial);
    assert_empty(&mut maintenance).await?;

    let marker = bootstrapper.initialize_supervised().await?;
    anyhow::ensure!(
        marker.seq == 2,
        "supervision origin marker must be sequence two"
    );
    let origin = bootstrapper.supervision_origin().await?;
    anyhow::ensure!(
        origin.fresh_origin_retained,
        "fresh bootstrap witness was not retained"
    );
    let boundary = origin
        .boundary
        .as_ref()
        .context("supervision origin marker is missing")?;
    anyhow::ensure!(
        boundary.seq == marker.seq
            && boundary.hash == marker.hash
            && boundary.event.id == marker.event.id,
        "origin read differs from the committed bootstrap marker"
    );
    let successful = fingerprint(&mut maintenance).await?;
    assert_counts(&mut maintenance, &[1, 2, 0, 0, 0, 0, 0, 0, 0, 1]).await?;

    let repeated = bootstrapper
        .initialize_supervised()
        .await
        .expect_err("the original handle cannot establish a second boundary");
    anyhow::ensure!(
        repeated.downcast_ref::<ControlFailure>() == Some(&ControlFailure::Conflict)
            && bootstrapper
                .supervision_origin()
                .await?
                .fresh_origin_retained,
        "an explicit bootstrap conflict changed the confirmed origin"
    );

    let reinit = reopened
        .initialize_supervised()
        .await
        .expect_err("a new handle must not reinitialize a persisted origin");
    anyhow::ensure!(
        reinit.downcast_ref::<ControlFailure>() == Some(&ControlFailure::Conflict),
        "reinitialization returned an unexpected failure: {reinit}"
    );
    let reopened_origin = reopened.supervision_origin().await?;
    anyhow::ensure!(
        !reopened_origin.fresh_origin_retained
            && reopened_origin
                .boundary
                .as_ref()
                .is_some_and(|stored| stored.hash == marker.hash && stored.seq == marker.seq),
        "new handle recovered the in-memory fresh-origin authority"
    );
    anyhow::ensure!(fingerprint(&mut maintenance).await? == successful);

    let corrupt_hash = "f".repeat(64);
    maintenance
        .exec_drop(
            "UPDATE ai_guard_supervision_meta SET genesis_hash=? WHERE singleton=1",
            (&corrupt_hash,),
        )
        .await?;
    anyhow::ensure!(maintenance.affected_rows() == 1);
    anyhow::ensure!(
        bootstrapper.supervision_origin().await.is_err(),
        "corrupt origin projection must fail its verified read"
    );
    maintenance
        .exec_drop(
            "UPDATE ai_guard_supervision_meta SET genesis_hash=? WHERE singleton=1",
            (marker.hash.as_bytes(),),
        )
        .await?;
    anyhow::ensure!(maintenance.affected_rows() == 1);
    anyhow::ensure!(fingerprint(&mut maintenance).await? == successful);
    anyhow::ensure!(
        !bootstrapper
            .supervision_origin()
            .await?
            .fresh_origin_retained,
        "restoring exact projection bytes revived the invalidated Store witness"
    );

    let verified = bootstrapper.verify().await?;
    anyhow::ensure!(
        verified.seq == 2,
        "audit chain is not readable after bootstrap"
    );
    let events = bootstrapper
        .events(&EventQuery {
            order: Some("asc".into()),
            limit: Some(10),
            ..Default::default()
        })
        .await?;
    anyhow::ensure!(
        events.len() == 2
            && events[1].seq == marker.seq
            && events[1].hash == marker.hash
            && events[1].event.id == marker.event.id,
        "the committed supervision marker is not readable from audit history"
    );

    // An explicit V2 admission attempt is refused after the boundary, before a
    // worker projection or quota reservation is written.
    let policy = Policy::default();
    let (admission, _) = super::worker::admission(bootstrapper, &policy).await?;
    let before_admission = bootstrapper.verify().await?;
    let refusal = bootstrapper
        .admit_worker(
            &admission,
            &policy,
            policy::evaluate(&policy, &admission.decision_request),
            serde_json::Value::Null,
            &QuotaPolicyV1::default(),
        )
        .await
        .expect_err("legacy V2 worker admission must remain disabled after the boundary");
    anyhow::ensure!(
        refusal.downcast_ref::<ControlFailure>() == Some(&ControlFailure::SupervisionInactive),
        "post-boundary admission returned an unexpected refusal: {refusal}"
    );
    anyhow::ensure!(bootstrapper.verify().await? == before_admission);
    anyhow::ensure!(
        bootstrapper
            .worker_quota(&QuotaPolicyV1::default())
            .await?
            .held_workers
            == 0,
        "refused admission reserved worker quota"
    );
    let namespace = marker.event.data["namespace_uuid"]
        .as_str()
        .context("supervision namespace is missing")?;
    super::prepared_admission::exercise(writer, maintenance, namespace).await
}

async fn assert_empty(conn: &mut Conn) -> anyhow::Result<()> {
    assert_counts(conn, &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0]).await?;
    let binding = conn
        .query_first::<(Option<Vec<u8>>, Option<Vec<u8>>, i64, Vec<u8>), _>(
            "SELECT tenant_id,namespace_id,head_seq,head_hash FROM ai_guard_audit_state WHERE singleton=1",
        )
        .await?;
    anyhow::ensure!(
        binding.is_some_and(|(tenant, namespace, seq, hash)| {
            tenant.is_none() && namespace.is_none() && seq == 0 && hash == b"0".repeat(64)
        }),
        "failed bootstrap changed the audit binding or empty head"
    );
    Ok(())
}

async fn assert_counts(conn: &mut Conn, expected: &[u64; 10]) -> anyhow::Result<()> {
    for (table, expected) in TABLES.into_iter().zip(expected) {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        let actual = conn
            .query_first::<u64, _>(sql)
            .await?
            .context("QA projection table is missing")?;
        anyhow::ensure!(
            actual == *expected,
            "unexpected row count in {table}: {actual}"
        );
    }
    Ok(())
}

type EventRow = (i64, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);
type AuditState = (u16, Option<Vec<u8>>, Option<Vec<u8>>, i64, Vec<u8>, u64);
type SupervisionMeta = (Vec<u8>, i64, Vec<u8>);

async fn fingerprint(conn: &mut Conn) -> anyhow::Result<String> {
    let state = conn
        .query_first::<AuditState, _>(
            "SELECT schema_version,tenant_id,namespace_id,head_seq,head_hash,payload_bytes \
             FROM ai_guard_audit_state WHERE singleton=1",
        )
        .await?
        .context("audit state singleton is missing")?;
    let events: Vec<EventRow> = conn
        .query(
            "SELECT seq,event_id,received_at,prev_hash,hash,payload \
             FROM ai_guard_audit_event ORDER BY seq",
        )
        .await?;
    let meta = conn
        .query_first::<SupervisionMeta, _>(
            "SELECT protocol,genesis_seq,genesis_hash FROM ai_guard_supervision_meta WHERE singleton=1",
        )
        .await?;
    let mut counts = Vec::with_capacity(TABLES.len());
    for table in TABLES {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        let count = conn
            .query_first::<u64, _>(sql)
            .await?
            .context("QA projection table is missing")?;
        counts.push((table, count));
    }
    let bytes = serde_json::to_vec(&(state, events, meta, counts))?;
    let mut digest = Sha256::new();
    digest.update(b"ai-guard:mysql-supervision-bootstrap-qa:v1\0");
    digest.update(bytes);
    Ok(hex::encode(digest.finalize()))
}
