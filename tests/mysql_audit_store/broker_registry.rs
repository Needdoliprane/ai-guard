use super::{
    fixture_prefix,
    identity::{FixtureFiles, insert_event, lock_head, require_fresh, write_head},
};
use agentsandbox::{
    broker_registry::{
        BrokerAuthorityV1, BrokerGrantMutationV1, BrokerRegistryFailureV1, BrokerRevocationV1,
        BrokerRuntimeKindV1, BrokerStateV1, BrokerV1,
    },
    control::content_hash,
    model::Event,
    mysql_storage::audit::{AuditHead, MysqlAuditStore},
};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, SecondsFormat, TimeDelta};
use mysql_async::{Conn, Transaction, TxOpts, prelude::Queryable};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

const TENANT_A: &str = "tenant-qa-a";
const TENANT_B: &str = "tenant-qa-b";
const FIRST_BROKER: &str = "10000000-0000-4000-8000-000000000001";
const LAST_BROKER: &str = "20000000-0000-4000-8000-000000000002";
const HISTORICAL_AUTHORITY: &str = "ancien-hôte";
const DDL: &str = include_str!("../../schema/mysql/broker-registry-v1.sql");

#[derive(Serialize)]
struct Change<'a, T> {
    action: &'static str,
    mutation: &'a T,
}

fn open_writer(fixture: &FixtureFiles, tenant: &str) -> Result<MysqlAuditStore> {
    fixture.target.open(
        &fixture.target.maintenance_username,
        fixture.credentials(&fixture.maintenance_password)?,
        tenant,
        fixture.target.namespace,
        128,
    )
}

async fn prepare_genesis(conn: &mut Conn, tenant: &str, namespace: Uuid) -> Result<AuditHead> {
    let mut tx = conn.start_transaction(TxOpts::new()).await?;
    let mut head = lock_head(&mut tx).await?;
    let mut event = Event::new("control.broker_registry.genesis");
    event.data = json!({
        "version": 1,
        "tenant_id": tenant,
        "control_namespace_uuid": namespace,
        "previous_seq": head.seq,
        "previous_hash": head.hash,
    });
    let genesis = insert_event(&mut tx, &mut head, event).await?;
    tx.exec_drop(
        "INSERT INTO ai_guard_broker_registry_meta (singleton,genesis_seq,genesis_hash) VALUES (1,?,?)",
        (genesis.seq, &genesis.hash),
    )
    .await?;
    write_head(&mut tx, &head).await?;
    tx.commit().await?;
    Ok(head)
}

fn grant(id: &str, authority_ids: &[&str], at: &str) -> Result<BrokerGrantMutationV1> {
    Ok(BrokerGrantMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        broker_id: id.into(),
        expected_revision: None,
        credential_id: Uuid::new_v4().to_string(),
        incarnation_id: Uuid::new_v4().to_string(),
        authorities: authority_ids
            .iter()
            .map(|id| BrokerAuthorityV1 {
                authority_id: (*id).into(),
                runtime_kind: BrokerRuntimeKindV1::Docker,
            })
            .collect(),
        lease_seconds: 30,
        not_after: (DateTime::parse_from_rfc3339(at)? + TimeDelta::hours(1))
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    })
}

async fn append_change<T: Serialize>(
    tx: &mut Transaction<'_>,
    head: &mut AuditHead,
    at: &str,
    change: &Change<'_, T>,
) -> Result<(i64, String)> {
    let hash = content_hash("ai-guard:broker-registry-input:v1", change)?;
    let mut event = Event::new(&format!("control.broker_registry.{}", change.action));
    event.timestamp = at.into();
    event.data = json!({"version": 1, "input_hash": hash, "received_at": at, "change": change});
    let stored = insert_event(tx, head, event).await?;
    Ok((stored.seq, hash))
}

async fn write_projection(
    tx: &mut Transaction<'_>,
    broker: &BrokerV1,
    previous_revision: u64,
    operation_id: &str,
    input_hash: &str,
    seq: i64,
) -> Result<()> {
    let payload = serde_json::to_vec(broker)?;
    if previous_revision == 0 {
        tx.exec_drop(
            "INSERT INTO ai_guard_broker_registry_brokers
             (broker_id,grant_revision,heartbeat_revision,payload_json,event_seq) VALUES (?,?,?,?,?)",
            (&broker.broker_id, broker.grant_revision, broker.heartbeat_revision, &payload, seq),
        )
        .await?;
    } else {
        tx.exec_drop(
            "UPDATE ai_guard_broker_registry_brokers SET grant_revision=?,payload_json=?,event_seq=?
             WHERE broker_id=? AND grant_revision=?",
            (broker.grant_revision, &payload, seq, &broker.broker_id, previous_revision),
        )
        .await?;
    }
    ensure!(tx.affected_rows() == 1);
    tx.exec_drop(
        "INSERT INTO ai_guard_broker_registry_operations
         (operation_id,input_hash,event_seq,broker_id,result_json) VALUES (?,?,?,?,?)",
        (operation_id, input_hash, seq, &broker.broker_id, payload),
    )
    .await?;
    Ok(())
}

async fn prepare_grant(
    tx: &mut Transaction<'_>,
    head: &mut AuditHead,
    at: &str,
    mutation: &BrokerGrantMutationV1,
    new_authorities: &[&str],
) -> Result<BrokerV1> {
    mutation.validate()?;
    let change = Change {
        action: "grant",
        mutation,
    };
    let (seq, hash) = append_change(tx, head, at, &change).await?;
    let previous = mutation.expected_revision.unwrap_or(0);
    let broker = BrokerV1 {
        version: 1,
        broker_id: mutation.broker_id.clone(),
        grant_revision: previous + 1,
        heartbeat_revision: 0,
        credential_id: mutation.credential_id.clone(),
        incarnation_id: mutation.incarnation_id.clone(),
        authorities: mutation.authorities.clone(),
        lease_seconds: mutation.lease_seconds,
        not_after: mutation.not_after.clone(),
        revoked: false,
        granted_at: at.into(),
        revoked_at: None,
        last_heartbeat: None,
    };
    write_projection(tx, &broker, previous, &mutation.operation_id, &hash, seq).await?;
    for authority_id in new_authorities {
        tx.exec_drop(
            "INSERT INTO ai_guard_broker_registry_authorities (authority_id,broker_id,runtime_kind)
             VALUES (?,?,'docker')",
            (*authority_id, &broker.broker_id),
        )
        .await?;
    }
    tx.exec_drop(
        "INSERT INTO ai_guard_broker_registry_incarnations (incarnation_id,broker_id,event_seq)
         VALUES (?,?,?)",
        (&broker.incarnation_id, &broker.broker_id, seq),
    )
    .await?;
    Ok(broker)
}

#[tokio::test]
#[ignore = "requires two fresh prepared private MySQL 8.4 QA schemas and a SELECT-only reader"]
async fn prepared_broker_registry_requires_verified_administrative_history() -> Result<()> {
    let a = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let b = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
    ensure!(a.target.port == b.target.port && a.target.namespace != b.target.namespace);
    let mut conn_a = a.maintenance().await?;
    let mut conn_b = b.maintenance().await?;
    require_fresh(&mut conn_a).await?;
    require_fresh(&mut conn_b).await?;
    let writer = open_writer(&a, TENANT_A)?;
    writer.initialize().await?;
    let older = open_writer(&b, TENANT_B)?;
    let older_genesis = older.initialize().await?;
    assert_eq!(older.verify().await?.hash, older_genesis.hash);
    assert_eq!(
        older
            .brokers_page(None, 1)
            .await
            .unwrap_err()
            .downcast_ref::<BrokerRegistryFailureV1>(),
        Some(&BrokerRegistryFailureV1::MigrationRequired)
    );
    let mut ddl = DDL.split(';').map(str::trim).filter(|sql| !sql.is_empty());
    let first_ddl = ddl.next().context("broker registry DDL is empty")?;
    conn_b.query_drop(first_ddl).await?;
    assert!(older.verify().await.is_err());
    conn_a.query_drop(first_ddl).await?;
    for statement in ddl {
        conn_a.query_drop(statement).await?;
        conn_b.query_drop(statement).await?;
    }
    prepare_genesis(&mut conn_a, TENANT_A, a.target.namespace).await?;
    let head_b = prepare_genesis(&mut conn_b, TENANT_B, b.target.namespace).await?;
    let prefix_b = fixture_prefix(&mut conn_b, i64::MAX).await?;
    let reader_b = b.reader(TENANT_B, b.target.namespace)?;
    assert!(reader_b.brokers_page(None, 100).await?.is_empty());

    let mut tx = conn_a.start_transaction(TxOpts::new()).await?;
    let mut head = lock_head(&mut tx).await?;
    let at: String = tx
        .query_first("SELECT MAX(received_at) FROM ai_guard_audit_event")
        .await?
        .context("broker registry fixture time floor is missing")?;
    let last = grant(LAST_BROKER, &["dernier-hôte", HISTORICAL_AUTHORITY], &at)?;
    prepare_grant(
        &mut tx,
        &mut head,
        &at,
        &last,
        &["dernier-hôte", HISTORICAL_AUTHORITY],
    )
    .await?;
    let first = grant(FIRST_BROKER, &["premier-hôte"], &at)?;
    let first = prepare_grant(&mut tx, &mut head, &at, &first, &["premier-hôte"]).await?;
    let mut renewal = grant(LAST_BROKER, &["dernier-hôte"], &at)?;
    renewal.expected_revision = Some(1);
    let mut last = prepare_grant(&mut tx, &mut head, &at, &renewal, &[]).await?;
    let revoke = BrokerRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        broker_id: LAST_BROKER.into(),
        expected_revision: 2,
    };
    let change = Change {
        action: "revoke",
        mutation: &revoke,
    };
    let (seq, hash) = append_change(&mut tx, &mut head, &at, &change).await?;
    last.grant_revision = 3;
    last.revoked = true;
    last.revoked_at = Some(at.clone());
    write_projection(&mut tx, &last, 2, &revoke.operation_id, &hash, seq).await?;
    write_head(&mut tx, &head).await?;
    tx.commit().await?;

    let reader = a.reader(TENANT_A, a.target.namespace)?;
    let prefix_a = fixture_prefix(&mut conn_a, i64::MAX).await?;
    for (cursor, limit) in [(None, 0), (None, 101), (Some("not-a-uuid"), 1)] {
        assert_eq!(
            reader
                .brokers_page(cursor, limit)
                .await
                .unwrap_err()
                .downcast_ref::<BrokerRegistryFailureV1>(),
            Some(&BrokerRegistryFailureV1::InvalidRequest)
        );
    }
    let wrong_namespace = a.reader(TENANT_A, b.target.namespace)?;
    assert_eq!(
        wrong_namespace
            .brokers_page(None, 1)
            .await
            .unwrap_err()
            .to_string(),
        "MySQL audit binding mismatch"
    );
    let page = reader.brokers_page(None, 1).await?;
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].broker, first);
    assert_eq!(page[0].state, BrokerStateV1::Unseen);
    assert!(!page[0].runtime_inventory_complete);
    let page = reader.brokers_page(Some(FIRST_BROKER), 1).await?;
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].broker, last);
    assert_eq!(page[0].state, BrokerStateV1::Revoked);
    assert!(reader.brokers_page(Some(LAST_BROKER), 1).await?.is_empty());
    assert_eq!(reader.verify().await?, head);

    let fault =
        "UPDATE ai_guard_broker_registry_authorities SET runtime_kind=? WHERE authority_id=?";
    conn_a
        .exec_drop(fault, ("podman", HISTORICAL_AUTHORITY))
        .await?;
    assert_eq!(conn_a.affected_rows(), 1);
    let refused = reader.brokers_page(None, 1).await;
    let append_refused = writer
        .append(Event::new("qa.broker-registry-refused"))
        .await;
    conn_a
        .exec_drop(fault, ("docker", HISTORICAL_AUTHORITY))
        .await?;
    assert_eq!(conn_a.affected_rows(), 1);
    let expected =
        "MySQL broker registry projection differs from audit: broker_registry_authorities";
    assert_eq!(refused.unwrap_err().to_string(), expected);
    assert_eq!(append_refused.unwrap_err().to_string(), expected);
    assert_eq!(reader.verify().await?, head);
    assert_eq!(fixture_prefix(&mut conn_a, i64::MAX).await?, prefix_a);

    // No other broker may acquire an authority already assigned in the prefix.
    let mut tx = conn_a.start_transaction(TxOpts::new()).await?;
    let mut bad_head = lock_head(&mut tx).await?;
    let conflicting = grant(
        "30000000-0000-4000-8000-000000000003",
        &[HISTORICAL_AUTHORITY],
        &at,
    )?;
    append_change(
        &mut tx,
        &mut bad_head,
        &at,
        &Change {
            action: "grant",
            mutation: &conflicting,
        },
    )
    .await?;
    write_head(&mut tx, &bad_head).await?;
    tx.commit().await?;
    assert_eq!(
        reader
            .brokers_page(None, 1)
            .await
            .unwrap_err()
            .downcast_ref::<BrokerRegistryFailureV1>(),
        Some(&BrokerRegistryFailureV1::Conflict)
    );
    assert_eq!(fixture_prefix(&mut conn_a, head.seq).await?, prefix_a);
    assert!(reader_b.brokers_page(None, 100).await?.is_empty());
    assert_eq!(reader_b.verify().await?, head_b);
    assert_eq!(fixture_prefix(&mut conn_b, i64::MAX).await?, prefix_b);
    Ok(())
}
