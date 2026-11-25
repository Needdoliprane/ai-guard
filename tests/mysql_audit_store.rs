//! Real MySQL audit regression. The external fixture prepares a disposable schema.
use agentsandbox::{
    audit::{AuditCapacityFailureV1, EventQuery},
    control::QuotaPolicyV1,
    control_fs::PrivateFile,
    model::{Decision, Event, Outcome, StoredEvent},
    mysql_storage::{
        MysqlClientIdentity, MysqlCredentials, MysqlEndpoint, MysqlProbeError,
        audit::{AuditHead, MysqlAuditStore},
    },
    policy::{self, Policy},
    redaction::Redactor,
};
use anyhow::Context;
use mysql_async::{ClientIdentity, Conn, OptsBuilder, SslOpts, TxOpts, prelude::Queryable};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[path = "mysql_audit_store/broker_registry.rs"]
mod broker_registry;
#[path = "mysql_audit_store/capacity.rs"]
mod capacity;
#[path = "mysql_audit_store/decision.rs"]
mod decision;
#[path = "mysql_audit_store/entities.rs"]
mod entities;
#[path = "mysql_audit_store/events.rs"]
mod events;
#[path = "mysql_audit_store/identity.rs"]
mod identity;
#[path = "mysql_audit_store/integrity.rs"]
mod integrity;
#[path = "mysql_audit_store/overview.rs"]
mod overview;
#[path = "mysql_audit_store/owned_preparation.rs"]
mod owned_preparation;
#[path = "mysql_audit_store/prepared_admission.rs"]
mod prepared_admission;
#[path = "mysql_audit_store/supervised_write.rs"]
mod supervised_write;
#[path = "mysql_audit_store/supervision.rs"]
mod supervision;
#[path = "mysql_audit_store/worker.rs"]
mod worker;
#[path = "mysql_audit_store/worker_lifecycle.rs"]
mod worker_lifecycle;
#[path = "mysql_audit_store/workload.rs"]
mod workload;

const TENANT: &str = "tenant-qa";
const SECRET: &str = "synthetic-private-audit-value";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    host: String,
    port: u16,
    database: String,
    username: String,
    namespace: Uuid,
    password_file: PathBuf,
    ca_file: PathBuf,
    maintenance_username: String,
    maintenance_password_file: PathBuf,
    client_cert_file: Option<PathBuf>,
    client_key_file: Option<PathBuf>,
}

impl Fixture {
    fn endpoint<'a>(&'a self, username: &'a str) -> MysqlEndpoint<'a> {
        MysqlEndpoint {
            host: &self.host,
            port: self.port,
            database: &self.database,
            username,
        }
    }

    fn open(
        &self,
        username: &str,
        credentials: MysqlCredentials<'_>,
        tenant: &str,
        namespace: Uuid,
        max_events: i64,
    ) -> anyhow::Result<MysqlAuditStore> {
        MysqlAuditStore::new(
            self.endpoint(username),
            credentials,
            tenant,
            namespace,
            Redactor::new(vec![SECRET.into()]),
            max_events,
        )
    }
}

async fn maintenance_connection(
    endpoint: MysqlEndpoint<'_>,
    credentials: MysqlCredentials<'_>,
) -> anyhow::Result<Conn> {
    let identity = credentials.client_identity.map(|identity| {
        ClientIdentity::new(
            identity.certificate_chain_pem.to_vec().into(),
            identity.private_key_pem.to_vec().into(),
        )
    });
    let ssl = SslOpts::default()
        .with_disable_built_in_roots(true)
        .with_root_certs(vec![credentials.ca_pem.to_vec().into()])
        .with_client_identity(identity);
    let options = OptsBuilder::default()
        .ip_or_hostname(endpoint.host)
        .tcp_port(endpoint.port)
        .db_name(Some(endpoint.database))
        .user(Some(endpoint.username))
        .pass(Some(credentials.password))
        .prefer_socket(false)
        .socket(None::<String>)
        .ssl_opts(Some(ssl));
    tokio::time::timeout(std::time::Duration::from_secs(8), Conn::new(options))
        .await?
        .map_err(|_| anyhow::anyhow!("fixture maintenance connection failed"))
}

async fn restore_fixture_prefix(mut conn: Conn, prefix: &AuditHead) -> anyhow::Result<()> {
    let restore = async {
        let mut tx = conn.start_transaction(TxOpts::new()).await?;
        tx.exec_drop(
            "DELETE FROM ai_guard_audit_event WHERE seq > ?",
            (prefix.seq,),
        )
        .await?;
        tx.exec_drop(
            "UPDATE ai_guard_audit_state SET head_seq=?,head_hash=?,payload_bytes=? WHERE singleton=1",
            (prefix.seq, &prefix.hash, prefix.payload_bytes),
        )
        .await?;
        tx.commit().await
    };
    tokio::time::timeout(std::time::Duration::from_secs(8), restore)
        .await?
        .map_err(|_| anyhow::anyhow!("fixture restore transaction failed"))
}

async fn assert_missing_decision_index_refused(
    mut conn: Conn,
    store: &MysqlAuditStore,
) -> anyhow::Result<()> {
    let head = store.verify().await?;
    let remove_index = async {
        let decision_seq = conn
            .query_first::<i64, _>(
                "SELECT event_seq FROM ai_guard_audit_decision ORDER BY event_seq DESC LIMIT 1",
            )
            .await?;
        let decision_seq = decision_seq.expect("fixture has a durable decision");
        conn.exec_drop(
            "DELETE FROM ai_guard_audit_decision WHERE event_seq=?",
            (decision_seq,),
        )
        .await?;
        Ok::<_, mysql_async::Error>(conn.affected_rows())
    };
    let removed = tokio::time::timeout(std::time::Duration::from_secs(8), remove_index)
        .await?
        .map_err(|_| anyhow::anyhow!("fixture decision index removal failed"))?;
    assert_eq!(removed, 1);
    let missing_index = "MySQL decision event is missing its index receipt";
    let first_page = EventQuery {
        order: Some("asc".into()),
        limit: Some(1),
        ..Default::default()
    };
    for failure in [
        store.verify().await.err(),
        store.events(&first_page).await.err(),
        store.append(Event::new("qa.refused")).await.err(),
    ] {
        assert_eq!(
            failure.expect("missing index must fail closed").to_string(),
            missing_index
        );
    }
    let receipt = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        conn.query_first::<(i64, String, u64, i64), _>(
            "SELECT head_seq,head_hash,payload_bytes,(SELECT COUNT(*) FROM ai_guard_audit_event) \
             FROM ai_guard_audit_state WHERE singleton=1",
        ),
    )
    .await?
    .map_err(|_| anyhow::anyhow!("fixture state inspection failed"))?;
    assert_eq!(
        receipt,
        Some((head.seq, head.hash, head.payload_bytes, head.seq))
    );
    Ok(())
}

async fn assert_failed_index_rolls_back(
    mut conn: Conn,
    store: &MysqlAuditStore,
) -> anyhow::Result<()> {
    let head = store.verify().await?;
    let request = decision::request("strict", &["id"]);
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        conn.query_drop(
            "ALTER TABLE ai_guard_audit_decision \
             ADD CONSTRAINT ai_guard_qa_block_decision CHECK (event_seq < 0)",
        )
        .await
        .map_err(|_| anyhow::anyhow!("fixture decision fault setup failed"))?;
        let result = store
            .decision(
                &request,
                Some((
                    Decision::new(Outcome::Allow, "qa", "synthetic"),
                    json!(null),
                )),
            )
            .await;
        conn.query_drop(
            "ALTER TABLE ai_guard_audit_decision DROP CHECK ai_guard_qa_block_decision",
        )
        .await
        .map_err(|_| anyhow::anyhow!("fixture decision fault cleanup failed"))?;
        let error = result
            .err()
            .expect("decision index constraint must be enforced");
        assert_eq!(
            error.downcast_ref::<MysqlProbeError>(),
            Some(&MysqlProbeError::ServerRefused(3819)),
        );
        assert_eq!(store.verify().await?, head);
        assert!(store.decision(&request, None).await?.is_none());
        Ok::<_, anyhow::Error>(())
    })
    .await?
}

async fn assert_failed_worker_projection_rolls_back(
    mut conn: Conn,
    store: &MysqlAuditStore,
) -> anyhow::Result<()> {
    let policy = Policy::default();
    let quota = QuotaPolicyV1 {
        max_active_workers: 1,
        ..Default::default()
    };
    let (admission, _) = worker::admission(store, &policy).await?;
    let head = store.verify().await?;
    assert_eq!(store.worker_quota(&quota).await?.held_workers, 0);
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        conn.query_drop(
            "ALTER TABLE ai_guard_worker_attempt
             ADD CONSTRAINT ai_guard_qa_block_worker_attempt CHECK (OCTET_LENGTH(snapshot) = 0)",
        )
        .await
        .map_err(|_| anyhow::anyhow!("fixture worker projection fault setup failed"))?;
        let result = store
            .admit_worker(
                &admission,
                &policy,
                policy::evaluate(&policy, &admission.decision_request),
                json!(null),
                &quota,
            )
            .await;
        conn.query_drop(
            "ALTER TABLE ai_guard_worker_attempt DROP CHECK ai_guard_qa_block_worker_attempt",
        )
        .await
        .map_err(|_| anyhow::anyhow!("fixture worker projection fault cleanup failed"))?;
        let error = result.unwrap_err();
        assert_eq!(
            error.downcast_ref::<MysqlProbeError>(),
            Some(&MysqlProbeError::ServerRefused(3819))
        );
        assert_eq!(store.verify().await?, head);
        assert!(store.replay_admission(&admission, &quota).await?.is_none());
        assert_eq!(store.worker_quota(&quota).await?.held_workers, 0);
        Ok::<_, anyhow::Error>(())
    })
    .await?
}

async fn assert_worker_projection_mismatch_refused(
    mut conn: Conn,
    store: &MysqlAuditStore,
) -> anyhow::Result<()> {
    let head = store.verify().await?;
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let worker_id = conn
            .query_first::<String, _>(
                "SELECT worker_id FROM ai_guard_worker WHERE state='reserved' AND revision=1",
            )
            .await
            .map_err(|_| anyhow::anyhow!("fixture worker projection selection failed"))?
            .expect("fixture has a reserved worker");
        conn.exec_drop(
            "UPDATE ai_guard_worker SET revision=2 WHERE worker_id=? AND revision=1",
            (&worker_id,),
        )
        .await
        .map_err(|_| anyhow::anyhow!("fixture worker projection fault setup failed"))?;
        assert_eq!(conn.affected_rows(), 1);
        let verification = store.verify().await.unwrap_err();
        let page = store
            .events(&EventQuery {
                order: Some("asc".into()),
                limit: Some(1),
                ..Default::default()
            })
            .await
            .unwrap_err();
        let append = store
            .append(Event::new("qa.refused_after_worker_projection"))
            .await
            .unwrap_err();
        conn.exec_drop(
            "UPDATE ai_guard_worker SET revision=1 WHERE worker_id=? AND revision=2",
            (&worker_id,),
        )
        .await
        .map_err(|_| anyhow::anyhow!("fixture worker projection fault cleanup failed"))?;
        assert_eq!(conn.affected_rows(), 1);
        assert_eq!(
            verification.to_string(),
            "MySQL worker snapshot differs from its audit event"
        );
        assert_eq!(page.to_string(), verification.to_string());
        assert_eq!(append.to_string(), verification.to_string());
        assert_eq!(store.verify().await?, head);
        Ok::<_, anyhow::Error>(())
    })
    .await?
}

fn load(path: &Path, max_bytes: u64) -> anyhow::Result<PrivateFile> {
    PrivateFile::open(path, max_bytes).map_err(|_| anyhow::anyhow!("private QA file unavailable"))
}

fn expected_hash(row: &StoredEvent) -> anyhow::Result<String> {
    let mut hash = Sha256::new();
    hash.update(b"agentsandbox:audit:v1\0");
    hash.update(row.seq.to_be_bytes());
    let payload = serde_json::to_string(&row.event)?;
    for value in [&row.received_at, &row.prev_hash, &payload] {
        hash.update(value.as_bytes());
        hash.update([0]);
    }
    Ok(hex::encode(hash.finalize()))
}

type RawEventRow = (i64, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);

async fn fixture_prefix(conn: &mut Conn, last_seq: i64) -> anyhow::Result<Vec<RawEventRow>> {
    tokio::time::timeout(
        std::time::Duration::from_secs(8),
        conn.exec(
            "SELECT seq,event_id,received_at,prev_hash,hash,payload \
             FROM ai_guard_audit_event WHERE seq <= ? ORDER BY seq",
            (last_seq,),
        ),
    )
    .await?
    .map_err(|_| anyhow::anyhow!("fixture prefix inspection failed"))
}

#[tokio::test]
#[ignore = "requires the prepared private MySQL 8.4 QA schema"]
async fn prepared_target_bootstrap_append_replay_and_capacity() -> anyhow::Result<()> {
    let path = PathBuf::from(
        std::env::var_os("AI_GUARD_MYSQL_QA_CONFIG")
            .expect("private QA configuration path required"),
    );
    let held = load(&path, 8_192)?;
    let fixture: Fixture = serde_json::from_slice(held.bytes())
        .map_err(|_| anyhow::anyhow!("invalid private QA configuration"))?;
    assert_eq!(fixture.host, "localhost");
    assert_eq!(fixture.username, "qa_writer");
    assert_eq!(fixture.maintenance_username, "qa_probe");
    assert_eq!(
        fixture.database,
        format!("qa_audit_{}", fixture.namespace.simple())
    );
    let directory = path.parent().expect("QA configuration directory");
    let password = load(&directory.join(&fixture.password_file), 1_024)?;
    let maintenance_password = load(&directory.join(&fixture.maintenance_password_file), 1_024)?;
    let ca = load(&directory.join(&fixture.ca_file), 65_536)?;
    let client_identity = match (&fixture.client_cert_file, &fixture.client_key_file) {
        (Some(cert), Some(key)) => Some((
            load(&directory.join(cert), 65_536)?,
            load(&directory.join(key), 16_384)?,
        )),
        (None, None) => None,
        _ => anyhow::bail!("incomplete QA client identity"),
    };
    let password = std::str::from_utf8(password.bytes())?.trim_end_matches(['\r', '\n']);
    let maintenance_password =
        std::str::from_utf8(maintenance_password.bytes())?.trim_end_matches(['\r', '\n']);
    assert!(password != maintenance_password);
    let credentials = |password| MysqlCredentials {
        password,
        ca_pem: ca.bytes(),
        client_identity: client_identity
            .as_ref()
            .map(|(cert, key)| MysqlClientIdentity {
                certificate_chain_pem: cert.bytes(),
                private_key_pem: key.bytes(),
            }),
    };
    let make_store = |tenant: &str, namespace: Uuid| {
        fixture.open(
            &fixture.username,
            credentials(password),
            tenant,
            namespace,
            5,
        )
    };
    let maintenance = || {
        fixture.open(
            &fixture.maintenance_username,
            credentials(maintenance_password),
            TENANT,
            fixture.namespace,
            5,
        )
    };
    let maintenance_conn = || {
        maintenance_connection(
            fixture.endpoint(&fixture.maintenance_username),
            credentials(maintenance_password),
        )
    };
    let unprepared = make_store(TENANT, fixture.namespace)?;
    assert!(unprepared.verify().await.is_err());
    assert!(unprepared.append(Event::new("qa.event")).await.is_err());
    let error = unprepared
        .initialize()
        .await
        .err()
        .expect("writer cannot bind");
    assert_matches_privilege_denied(&error)?;
    drop(unprepared);

    let setup = maintenance()?;
    let other_setup = maintenance()?;
    let (first, retried) = tokio::join!(setup.initialize(), other_setup.initialize());
    let first = first?;
    let retried = retried?;
    assert_eq!(first.seq, 1);
    assert_eq!(retried.event.id, first.event.id);
    assert_eq!(retried.hash, first.hash);
    assert_eq!(first.prev_hash, "0".repeat(64));
    assert_eq!(first.hash, expected_hash(&first)?);
    let mut event = Event::new("qa.event");
    event.timestamp = "2026-10-30T12:10:11.999+02:00".into();
    event.data = json!({"note": SECRET});
    let written = setup.append(event.clone()).await?;
    assert_eq!(written.seq, 2);
    assert_eq!(written.prev_hash, first.hash);
    assert_eq!(written.event.timestamp, "2026-10-30T10:10:11.999Z");
    assert!(!serde_json::to_string(&written.event)?.contains(SECRET));
    assert_eq!(written.hash, expected_hash(&written)?);
    drop(setup);
    drop(other_setup);

    let mut inspector = maintenance_conn().await?;
    let original_rows = fixture_prefix(&mut inspector, written.seq).await?;
    assert_eq!(original_rows.len(), 2);
    let store = make_store(TENANT, fixture.namespace)?;
    assert_eq!(store.initialize().await?.hash, first.hash);
    let reopened = make_store(TENANT, fixture.namespace)?;
    let prefix = reopened.verify().await?;
    assert_eq!(prefix.hash, written.hash);
    for wrong_binding in [
        make_store("other-qa-tenant", fixture.namespace)?,
        make_store(TENANT, Uuid::new_v4())?,
    ] {
        let expected = "MySQL audit binding mismatch";
        assert_eq!(
            wrong_binding
                .events(&EventQuery::default())
                .await
                .unwrap_err()
                .to_string(),
            expected
        );
        assert_eq!(
            wrong_binding
                .append(Event::new("qa.wrong_binding"))
                .await
                .unwrap_err()
                .to_string(),
            expected
        );
    }
    assert!(store.append(Event::new("CoNtRoL.qa.event")).await.is_err());
    assert_eq!(reopened.append(event.clone()).await?.hash, written.hash);
    let mut conflict = event.clone();
    conflict.data = json!({"note": "different"});
    let error = reopened
        .append(conflict)
        .await
        .err()
        .expect("conflict should be refused");
    assert_eq!(error.to_string(), "event id conflict");

    let (a, b) = tokio::join!(
        store.append(Event::new("qa.one")),
        reopened.append(Event::new("qa.two"))
    );
    let mut last = [a?, b?];
    last.sort_by_key(|event| event.seq);
    assert_eq!([last[0].seq, last[1].seq], [3, 4]);
    assert_eq!(last[0].prev_hash, prefix.hash);
    assert_eq!(last[1].prev_hash, last[0].hash);
    assert_eq!(
        fixture_prefix(&mut inspector, written.seq).await?,
        original_rows
    );
    drop(inspector);
    let head = reopened.verify().await?;
    assert_eq!(head.seq, 4);
    assert_eq!(head.hash, last[1].hash);
    let exhausted = store.append(Event::new("qa.capacity")).await.unwrap_err();
    assert_eq!(
        exhausted.downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted),
    );
    assert_eq!(store.verify().await?, head);
    assert_eq!(store.append(event).await?.seq, 2);

    restore_fixture_prefix(maintenance_conn().await?, &prefix).await?;
    let fresh = make_store(TENANT, fixture.namespace)?;
    assert_eq!(fresh.verify().await?, prefix);
    assert_prefix_refused(store.verify().await.err())?;
    assert_prefix_refused(store.append(Event::new("qa.refused")).await.err())?;
    assert_prefix_refused(store.initialize().await.err())?;
    assert_eq!(fresh.verify().await?, prefix);

    let fork_first = fresh.append(Event::new("qa.fork.one")).await?;
    let fork_second = fresh.append(Event::new("qa.fork.two")).await?;
    assert_eq!(fork_first.prev_hash, prefix.hash);
    let fork = fresh.verify().await?;
    assert_eq!(fork.seq, head.seq);
    assert_eq!(fork.hash, fork_second.hash);
    assert_ne!(fork.hash, head.hash);
    assert_prefix_refused(store.verify().await.err())?;
    assert_prefix_refused(store.integrity().await.err())?;
    assert_prefix_refused(store.append(Event::new("qa.refused")).await.err())?;
    assert_eq!(fresh.verify().await?, fork);

    let decision_store = || {
        fixture.open(
            &fixture.username,
            credentials(password),
            TENANT,
            fixture.namespace,
            12,
        )
    };
    let first = decision_store()?;
    let reopened = decision_store()?;
    assert_failed_index_rolls_back(maintenance_conn().await?, &first).await?;
    decision::exercise(&first, &reopened).await?;
    capacity::exercise_events(&first).await?;
    let byte_capacity_store = fixture.open(
        &fixture.username,
        credentials(password),
        TENANT,
        fixture.namespace,
        128,
    )?;
    capacity::exercise_bytes(byte_capacity_store).await?;
    let worker_store = || {
        fixture.open(
            &fixture.username,
            credentials(password),
            TENANT,
            fixture.namespace,
            128,
        )
    };
    let workers = worker_store()?;
    let peer = worker_store()?;
    assert_failed_worker_projection_rolls_back(maintenance_conn().await?, &workers).await?;
    worker_lifecycle::exercise(&workers, &peer, maintenance_conn().await?, worker_store).await?;
    worker::exercise(&workers, &peer).await?;
    assert_worker_projection_mismatch_refused(maintenance_conn().await?, &workers).await?;
    events::exercise(&workers).await?;
    assert_missing_decision_index_refused(maintenance_conn().await?, &workers).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a fresh prepared private MySQL supervision QA schema"]
async fn prepared_supervision_origin_requires_fresh_atomic_bootstrap() -> anyhow::Result<()> {
    run_supervision_fixture(SupervisionCase::Bootstrap).await
}

#[tokio::test]
#[ignore = "requires a fresh prepared private MySQL supervision QA schema"]
async fn supervised_mysql_reservation_is_atomic_and_replayable() -> anyhow::Result<()> {
    run_supervision_fixture(SupervisionCase::Reservation).await
}

#[tokio::test]
#[ignore = "requires a fresh prepared private MySQL supervision QA schema"]
async fn prepared_control_delivery_preserves_reserves_and_replays() -> anyhow::Result<()> {
    run_supervision_fixture(SupervisionCase::ControlDelivery).await
}

#[tokio::test]
#[ignore = "requires a fresh prepared private MySQL supervision QA schema"]
async fn prepared_integrity_separates_chain_rupture_and_io_failure() -> anyhow::Result<()> {
    run_supervision_fixture(SupervisionCase::Integrity).await
}

#[tokio::test]
#[ignore = "requires a fresh prepared private MySQL supervision QA schema"]
async fn prepared_entities_match_sqlite_and_refuse_a_late_chain_error() -> anyhow::Result<()> {
    run_supervision_fixture(SupervisionCase::Entities).await
}

#[tokio::test]
#[ignore = "requires a fresh prepared private MySQL supervision QA schema"]
async fn prepared_overview_matches_sqlite_on_a_verified_prefix() -> anyhow::Result<()> {
    run_supervision_fixture(SupervisionCase::Overview).await
}

enum SupervisionCase {
    Bootstrap,
    Reservation,
    ControlDelivery,
    Integrity,
    Entities,
    Overview,
}

async fn run_supervision_fixture(case: SupervisionCase) -> anyhow::Result<()> {
    let path = PathBuf::from(
        std::env::var_os("AI_GUARD_MYSQL_SUPERVISION_QA_CONFIG")
            .expect("fresh private supervision QA configuration path required"),
    );
    let held = load(&path, 8_192)?;
    let fixture: Fixture = serde_json::from_slice(held.bytes())
        .map_err(|_| anyhow::anyhow!("invalid private supervision QA configuration"))?;
    assert_eq!(fixture.host, "localhost");
    assert_eq!(fixture.username, "qa_writer");
    assert_eq!(fixture.maintenance_username, "qa_probe");
    assert_eq!(
        fixture.database,
        format!("qa_audit_{}", fixture.namespace.simple())
    );
    let directory = path.parent().context("QA config directory unavailable")?;
    let password = load(&directory.join(&fixture.password_file), 1_024)?;
    let maintenance_password = load(&directory.join(&fixture.maintenance_password_file), 1_024)?;
    let ca = load(&directory.join(&fixture.ca_file), 65_536)?;
    let client_identity = match (&fixture.client_cert_file, &fixture.client_key_file) {
        (Some(cert), Some(key)) => Some((
            load(&directory.join(cert), 65_536)?,
            load(&directory.join(key), 16_384)?,
        )),
        (None, None) => None,
        _ => anyhow::bail!("incomplete supervision QA client identity"),
    };
    let password = std::str::from_utf8(password.bytes())?.trim_end_matches(['\r', '\n']);
    let maintenance_password =
        std::str::from_utf8(maintenance_password.bytes())?.trim_end_matches(['\r', '\n']);
    assert_ne!(password, maintenance_password);
    let credentials = |password| MysqlCredentials {
        password,
        ca_pem: ca.bytes(),
        client_identity: client_identity
            .as_ref()
            .map(|(cert, key)| MysqlClientIdentity {
                certificate_chain_pem: cert.bytes(),
                private_key_pem: key.bytes(),
            }),
    };
    let writer = fixture.open(
        &fixture.username,
        credentials(password),
        TENANT,
        fixture.namespace,
        128,
    )?;
    let bootstrapper = fixture.open(
        &fixture.maintenance_username,
        credentials(maintenance_password),
        TENANT,
        fixture.namespace,
        128,
    )?;
    let maintenance = maintenance_connection(
        fixture.endpoint(&fixture.maintenance_username),
        credentials(maintenance_password),
    )
    .await?;
    match case {
        SupervisionCase::Bootstrap => {
            let reopened = fixture.open(
                &fixture.maintenance_username,
                credentials(maintenance_password),
                TENANT,
                fixture.namespace,
                128,
            )?;
            Box::pin(supervision::exercise(
                &writer,
                &bootstrapper,
                &reopened,
                maintenance,
            ))
            .await
        }
        SupervisionCase::Reservation => {
            Box::pin(supervised_write::exercise(
                &bootstrapper,
                &writer,
                maintenance,
            ))
            .await
        }
        SupervisionCase::ControlDelivery => {
            let open = || {
                MysqlAuditStore::new(
                    fixture.endpoint(&fixture.username),
                    credentials(password),
                    TENANT,
                    fixture.namespace,
                    Redactor::new(vec![
                        SECRET.into(),
                        "a".repeat(64),
                        "command.requested".into(),
                    ]),
                    128,
                )
            };
            Box::pin(capacity::exercise_control_delivery(
                &bootstrapper,
                &writer,
                maintenance,
                open,
            ))
            .await
        }
        SupervisionCase::Integrity => {
            let transport_fixture = |port| {
                MysqlAuditStore::new(
                    MysqlEndpoint {
                        host: "127.0.0.1",
                        port,
                        ..fixture.endpoint(&fixture.username)
                    },
                    credentials(password),
                    TENANT,
                    fixture.namespace,
                    Redactor::new(vec![SECRET.into()]),
                    128,
                )
            };
            Box::pin(integrity::exercise(
                &bootstrapper,
                &writer,
                maintenance,
                transport_fixture,
            ))
            .await
        }
        SupervisionCase::Entities => {
            Box::pin(entities::exercise(&bootstrapper, &writer, maintenance)).await
        }
        SupervisionCase::Overview => {
            Box::pin(overview::exercise(&bootstrapper, &writer, maintenance)).await
        }
    }
}

fn assert_matches_privilege_denied(error: &anyhow::Error) -> anyhow::Result<()> {
    anyhow::ensure!(
        matches!(
            error.downcast_ref::<MysqlProbeError>(),
            Some(MysqlProbeError::ServerRefused(1142 | 1143))
        ),
        "writer bind failed for an unexpected reason: {error}"
    );
    Ok(())
}

fn assert_prefix_refused(error: Option<anyhow::Error>) -> anyhow::Result<()> {
    let error = error.expect("audit prefix change must be refused");
    anyhow::ensure!(
        error.to_string() == "MySQL audit does not extend its confirmed prefix",
        "unexpected error after changing the prefix: {error}"
    );
    Ok(())
}
