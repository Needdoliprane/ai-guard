use super::{Fixture, RawEventRow, expected_hash, fixture_prefix, load, maintenance_connection};
use agentsandbox::{
    control_fs::{PrivateFile, TrustedExecutable},
    identity::{
        HumanAccountV1, IdentityChangeV1, IdentityFailureV1, IdentityMutationV1,
        LegacyControlRoleV1, LocalCredentialV1, MembershipV1, PrincipalRefV1, ResourceGrantV1,
        ResourceKindV1, ResourceRefV1,
    },
    model::{Event, StoredEvent},
    mysql_storage::{
        MysqlClientIdentity, MysqlCredentials,
        audit::{AuditHead, MysqlAuditStore},
    },
    runtime::capture_stream,
    workload::WorkloadFailureV1,
};
use anyhow::{Context, Result, ensure};
use futures_util::FutureExt;
use mysql_async::{Conn, Transaction, TxOpts, prelude::Queryable};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::Permissions,
    io::Write,
    os::unix::fs::PermissionsExt,
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::Duration,
};
use tokio::process::Command;
use uuid::Uuid;

#[path = "gateway.rs"]
mod gateway;

const TENANT_A: &str = "tenant-qa-a";
const TENANT_B: &str = "tenant-qa-b";
const DIGEST_A: [u8; 32] = [0x42; 32];
const DIGEST_B: [u8; 32] = [0x43; 32];
const IDENTITY_DDL: &str = include_str!("../../schema/mysql/identity-v1.sql");

pub(super) struct FixtureFiles {
    pub(super) target: Fixture,
    reader_password: PrivateFile,
    pub(super) maintenance_password: PrivateFile,
    ca: PrivateFile,
    client_identity: Option<(PrivateFile, PrivateFile)>,
}

impl FixtureFiles {
    pub(super) fn open(config_env: &str) -> Result<Self> {
        Self::open_checked(config_env, |target| {
            target.username == "qa_reader" && target.maintenance_username == "qa_probe"
        })
    }

    pub(super) fn open_gateway(config_env: &str) -> Result<Self> {
        Self::open_checked(config_env, gateway_accounts_match_namespace)
    }

    fn open_checked(
        config_env: &str,
        accounts_match: impl FnOnce(&Fixture) -> bool,
    ) -> Result<Self> {
        let path = PathBuf::from(
            std::env::var_os(config_env)
                .context("private identity QA configuration is required")?,
        );
        let config = load(&path, 8_192)?;
        let target: Fixture = serde_json::from_slice(config.bytes())
            .map_err(|_| anyhow::anyhow!("invalid private identity QA configuration"))?;
        ensure!(
            target.host == "localhost"
                && target.port != 0
                && !target.namespace.is_nil()
                && target.database == format!("qa_audit_{}", target.namespace.simple())
                && accounts_match(&target),
            "identity QA requires a dedicated loopback database and QA accounts"
        );
        let directory = path.parent().context("identity QA directory is missing")?;
        let reader_password = load(&directory.join(&target.password_file), 1_024)?;
        let maintenance_password = load(&directory.join(&target.maintenance_password_file), 1_024)?;
        let ca = load(&directory.join(&target.ca_file), 65_536)?;
        let client_identity = match (&target.client_cert_file, &target.client_key_file) {
            (Some(cert), Some(key)) => Some((
                load(&directory.join(cert), 65_536)?,
                load(&directory.join(key), 16_384)?,
            )),
            (None, None) => None,
            _ => anyhow::bail!("incomplete identity QA client identity"),
        };
        let fixture = Self {
            target,
            reader_password,
            maintenance_password,
            ca,
            client_identity,
        };
        ensure!(
            fixture.credentials(&fixture.reader_password)?.password
                != fixture.credentials(&fixture.maintenance_password)?.password,
            "identity QA requires distinct reader and maintenance passwords"
        );
        Ok(fixture)
    }

    pub(super) fn credentials<'a>(
        &'a self,
        password: &'a PrivateFile,
    ) -> Result<MysqlCredentials<'a>> {
        let password = std::str::from_utf8(password.bytes())?.trim_end_matches(['\r', '\n']);
        Ok(MysqlCredentials {
            password,
            ca_pem: self.ca.bytes(),
            client_identity: self
                .client_identity
                .as_ref()
                .map(|(cert, key)| MysqlClientIdentity {
                    certificate_chain_pem: cert.bytes(),
                    private_key_pem: key.bytes(),
                }),
        })
    }

    pub(super) fn reader(&self, tenant: &str, namespace: Uuid) -> Result<MysqlAuditStore> {
        self.target.open(
            &self.target.username,
            self.credentials(&self.reader_password)?,
            tenant,
            namespace,
            128,
        )
    }

    pub(super) async fn maintenance(&self) -> Result<Conn> {
        maintenance_connection(
            self.target.endpoint(&self.target.maintenance_username),
            self.credentials(&self.maintenance_password)?,
        )
        .await
    }

    pub(super) async fn initialize(&self, tenant: &str) -> Result<StoredEvent> {
        self.target
            .open(
                &self.target.maintenance_username,
                self.credentials(&self.maintenance_password)?,
                tenant,
                self.target.namespace,
                128,
            )?
            .initialize()
            .await
    }
}

fn gateway_accounts_match_namespace(target: &Fixture) -> bool {
    let namespace = target.namespace.simple().to_string();
    let account_suffix = &namespace[..24];
    target.username == format!("qa_s_{account_suffix}")
        && target.maintenance_username == format!("qa_m_{account_suffix}")
}

#[test]
fn gateway_fixture_accounts_are_named_for_their_namespace_and_role() -> Result<()> {
    let mut target: Fixture = serde_json::from_value(json!({
        "host": "localhost",
        "port": 13306,
        "database": "qa_audit_0123456789abcdef0123456789abcdef",
        "namespace": "01234567-89ab-cdef-0123-456789abcdef",
        "username": "qa_s_0123456789abcdef01234567",
        "maintenance_username": "qa_m_0123456789abcdef01234567",
        "password_file": "service.password",
        "maintenance_password_file": "maintenance.password",
        "ca_file": "ca.pem",
    }))?;
    assert!(gateway_accounts_match_namespace(&target));
    target.username = "qa_reader".into();
    assert!(!gateway_accounts_match_namespace(&target));
    target.username = "qa_s_0123456789abcdef01234568".into();
    assert!(!gateway_accounts_match_namespace(&target));
    Ok(())
}

type AuditState = (u16, Option<String>, Option<String>, i64, String, u64);
type AuditSnapshot = (AuditState, Vec<RawEventRow>);

async fn audit_state(conn: &mut Conn) -> Result<AuditState> {
    conn.query_first(
        "SELECT schema_version,tenant_id,namespace_id,head_seq,head_hash,payload_bytes
             FROM ai_guard_audit_state WHERE singleton=1",
    )
    .await?
    .context("identity QA audit state is missing")
}

pub(super) async fn audit_snapshot(conn: &mut Conn) -> Result<AuditSnapshot> {
    let state = audit_state(conn).await?;
    let events = fixture_prefix(conn, i64::MAX).await?;
    Ok((state, events))
}

pub(super) async fn require_fresh(conn: &mut Conn) -> Result<()> {
    let state = audit_state(conn).await?;
    ensure!(
        state == (1, None, None, 0, "0".repeat(64), 0),
        "identity QA requires a fresh prepared audit schema"
    );
    let has_event = conn
        .query_first::<u8, _>("SELECT EXISTS(SELECT 1 FROM ai_guard_audit_event)")
        .await?;
    ensure!(
        has_event == Some(0),
        "identity QA audit journal is not empty"
    );
    Ok(())
}

pub(super) async fn lock_head(tx: &mut Transaction<'_>) -> Result<AuditHead> {
    let (seq, hash, payload_bytes) = tx
        .query_first(
            "SELECT head_seq,head_hash,payload_bytes
             FROM ai_guard_audit_state WHERE singleton=1 FOR UPDATE",
        )
        .await?
        .context("identity QA audit head is missing")?;
    Ok(AuditHead {
        seq,
        hash,
        payload_bytes,
    })
}

pub(super) async fn insert_event(
    tx: &mut Transaction<'_>,
    head: &mut AuditHead,
    event: Event,
) -> Result<StoredEvent> {
    event.validate()?;
    let payload = serde_json::to_string(&event)?;
    let mut stored = StoredEvent {
        seq: head
            .seq
            .checked_add(1)
            .context("identity QA sequence overflow")?,
        received_at: event.timestamp.clone(),
        prev_hash: head.hash.clone(),
        hash: String::new(),
        event,
    };
    stored.hash = expected_hash(&stored)?;
    tx.exec_drop(
        "INSERT INTO ai_guard_audit_event (seq,event_id,received_at,prev_hash,hash,payload)
         VALUES (?,?,?,?,?,?)",
        (
            stored.seq,
            &stored.event.id,
            &stored.received_at,
            &stored.prev_hash,
            &stored.hash,
            &payload,
        ),
    )
    .await?;
    head.seq = stored.seq;
    head.hash.clone_from(&stored.hash);
    head.payload_bytes += payload.len() as u64;
    Ok(stored)
}

async fn insert_mutation(
    tx: &mut Transaction<'_>,
    head: &mut AuditHead,
    revision: u64,
    change: IdentityChangeV1,
) -> Result<StoredEvent> {
    let mutation = IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "identity-qa-operator".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    };
    let input_hash = mutation.input_hash()?;
    let mut event = Event::new("control.identity.mutation");
    event.data = json!({
        "version": 1,
        "authorization_revision": revision,
        "input_hash": input_hash,
        "mutation": mutation,
    });
    let stored = insert_event(tx, head, event).await?;
    tx.exec_drop(
        "INSERT INTO ai_guard_identity_operations (operation_id,input_hash,event_seq) VALUES (?,?,?)",
        (&mutation.operation_id, &input_hash, stored.seq),
    )
    .await?;
    Ok(stored)
}

pub(super) async fn write_head(tx: &mut Transaction<'_>, head: &AuditHead) -> Result<()> {
    tx.exec_drop(
        "UPDATE ai_guard_audit_state SET head_seq=?,head_hash=?,payload_bytes=? WHERE singleton=1",
        (head.seq, &head.hash, head.payload_bytes),
    )
    .await?;
    Ok(())
}

async fn set_identity_revision(tx: &mut Transaction<'_>, expected: u64, next: u64) -> Result<()> {
    tx.exec_drop(
        "UPDATE ai_guard_identity_meta SET authorization_revision=?
         WHERE singleton=1 AND authorization_revision=?",
        (next, expected),
    )
    .await?;
    ensure!(
        tx.affected_rows() == 1,
        "identity QA authorization revision changed"
    );
    Ok(())
}

pub(super) async fn prepare_identity(
    conn: &mut Conn,
    namespace: Uuid,
    audit_genesis: &StoredEvent,
    digest: &[u8; 32],
) -> Result<(LocalCredentialV1, AuditHead)> {
    for statement in IDENTITY_DDL
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        conn.query_drop(statement).await?;
    }
    let mut tx = conn.start_transaction(TxOpts::new()).await?;
    let mut head = lock_head(&mut tx).await?;
    ensure!(
        head.seq == audit_genesis.seq && head.hash == audit_genesis.hash,
        "identity QA initialization receipt does not match the audit head"
    );
    let mut event = Event::new("control.identity.genesis");
    event.data = json!({
        "version": 4,
        "control_namespace_uuid": namespace,
        "previous_seq": head.seq,
        "previous_hash": head.hash,
    });
    let identity_genesis = insert_event(&mut tx, &mut head, event).await?;
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: namespace.to_string(),
        account_id: Uuid::new_v4().to_string(),
    };
    let principal_key = principal.key()?;
    let account = HumanAccountV1 {
        version: 1,
        principal: principal.clone(),
        display_name: "Synthetic identity QA account".into(),
        revision: 1,
        active: true,
    };
    let account_event = insert_mutation(
        &mut tx,
        &mut head,
        1,
        IdentityChangeV1::Account {
            expected_revision: 0,
            account: account.clone(),
        },
    )
    .await?;
    tx.exec_drop(
        "INSERT INTO ai_guard_identity_accounts (principal_key,payload_json,event_seq)
         VALUES (?,?,?)",
        (
            &principal_key,
            serde_json::to_vec(&account)?,
            account_event.seq,
        ),
    )
    .await?;
    let credential = LocalCredentialV1 {
        version: 1,
        credential_id: Uuid::new_v4().to_string(),
        principal,
        sha256_digest: hex::encode(digest),
        revision: 1,
        active: true,
        expires_at: None,
    };
    let credential_event = insert_mutation(
        &mut tx,
        &mut head,
        2,
        IdentityChangeV1::Credential {
            expected_revision: 0,
            binding: credential.clone(),
        },
    )
    .await?;
    tx.exec_drop(
        "INSERT INTO ai_guard_identity_credentials
         (credential_id,principal_key,digest,active,payload_json,event_seq)
         VALUES (?,?,?,1,?,?)",
        (
            &credential.credential_id,
            &principal_key,
            &credential.sha256_digest,
            serde_json::to_vec(&credential)?,
            credential_event.seq,
        ),
    )
    .await?;
    tx.exec_drop(
        "INSERT INTO ai_guard_identity_meta (singleton,authorization_revision,genesis_seq,genesis_hash)
         VALUES (1,2,?,?)",
        (identity_genesis.seq, &identity_genesis.hash),
    )
    .await?;
    write_head(&mut tx, &head).await?;
    tx.commit().await?;
    Ok((credential, head))
}

pub(super) async fn revoke_identity(
    conn: &mut Conn,
    previous_head: &AuditHead,
    binding: &LocalCredentialV1,
) -> Result<AuditHead> {
    let mut tx = conn.start_transaction(TxOpts::new()).await?;
    let mut head = lock_head(&mut tx).await?;
    ensure!(
        &head == previous_head,
        "identity QA audit head changed before revocation"
    );
    let binding = LocalCredentialV1 {
        revision: 2,
        active: false,
        ..binding.clone()
    };
    let stored = insert_mutation(
        &mut tx,
        &mut head,
        3,
        IdentityChangeV1::Credential {
            expected_revision: 1,
            binding: binding.clone(),
        },
    )
    .await?;
    tx.exec_drop(
        "UPDATE ai_guard_identity_credentials SET active=0,payload_json=?,event_seq=?
         WHERE credential_id=? AND active=1",
        (
            serde_json::to_vec(&binding)?,
            stored.seq,
            &binding.credential_id,
        ),
    )
    .await?;
    ensure!(
        tx.affected_rows() == 1,
        "identity QA credential was not active"
    );
    set_identity_revision(&mut tx, 2, 3).await?;
    write_head(&mut tx, &head).await?;
    tx.commit().await?;
    Ok(head)
}

async fn assert_projection_refused(
    conn: &mut Conn,
    credential_id: &str,
    reader: &MysqlAuditStore,
    reopened: &MysqlAuditStore,
) -> Result<()> {
    conn.exec_drop(
        "UPDATE ai_guard_identity_credentials SET active=1 WHERE credential_id=? AND active=0",
        (credential_id,),
    )
    .await?;
    ensure!(
        conn.affected_rows() == 1,
        "identity QA fault did not change its projection"
    );
    let known = reader.has_local_credential_binding(&DIGEST_A).await;
    let missing = reader.has_local_credential_binding(&DIGEST_B).await;
    let cold = reopened.has_local_credential_binding(&DIGEST_A).await;
    conn.exec_drop(
        "UPDATE ai_guard_identity_credentials SET active=0 WHERE credential_id=? AND active=1",
        (credential_id,),
    )
    .await?;
    ensure!(
        conn.affected_rows() == 1,
        "identity QA projection was not restored"
    );
    for rejected in [known, missing, cold] {
        let error = rejected.expect_err("lookup must refuse the divergent identity projection");
        ensure!(
            error.to_string()
                == "MySQL identity projection differs from audit: identity_credentials",
            "unexpected identity QA error: {error}"
        );
    }
    ensure!(reader.has_local_credential_binding(&DIGEST_A).await?);
    ensure!(reopened.has_local_credential_binding(&DIGEST_A).await?);
    Ok(())
}

async fn assert_genesis_refused(conn: &mut Conn, reader: &MysqlAuditStore) -> Result<()> {
    for (hash, expected) in [
        (
            None,
            "MySQL identity genesis does not match the prepared capability",
        ),
        (Some([0xff; 64]), "unexpected MySQL audit row"),
    ] {
        gateway::with_unverifiable_identity_projection(conn, hash, async {
            let refused = AssertUnwindSafe(reader.has_local_credential_binding(&DIGEST_A))
                .catch_unwind()
                .await
                .map_err(|_| {
                    anyhow::anyhow!("identity lookup panicked while reading a capability")
                })?;
            let Err(error) = refused else {
                anyhow::bail!("lookup must refuse an invalid genesis hash");
            };
            ensure!(
                error.to_string() == expected,
                "unexpected identity QA capability error"
            );
            Ok(())
        })
        .await?;
        ensure!(reader.has_local_credential_binding(&DIGEST_A).await?);
    }
    Ok(())
}

async fn insert_session_grant(
    tx: &mut Transaction<'_>,
    head: &mut AuditHead,
    revision: u64,
    membership: &MembershipV1,
    session_id: &str,
) -> Result<()> {
    let grant = ResourceGrantV1 {
        version: 1,
        tenant_id: membership.tenant_id.clone(),
        organization_scope: membership.organization_scope.clone(),
        resource: ResourceRefV1 {
            kind: ResourceKindV1::Session,
            resource_id: session_id.into(),
        },
        owner: membership.principal.clone(),
        revision: 1,
        active: true,
        shares: vec![],
    };
    let stored = insert_mutation(
        tx,
        head,
        revision,
        IdentityChangeV1::ResourceGrant {
            expected_revision: 0,
            grant: grant.clone(),
        },
    )
    .await?;
    tx.exec_drop(
        "INSERT INTO ai_guard_identity_resources
         (resource_key,resource_kind,resource_id,owner_key,active,payload_json,event_seq)
         VALUES (?,'session',?,?,1,?,?)",
        (
            grant.resource.key()?,
            session_id,
            grant.owner.key()?,
            serde_json::to_vec(&grant)?,
            stored.seq,
        ),
    )
    .await?;
    Ok(())
}

async fn assert_historical_session_grants(
    conn: &mut Conn,
    reader: &MysqlAuditStore,
    revoked: &AuditHead,
    principal: &PrincipalRefV1,
) -> Result<()> {
    let membership = MembershipV1 {
        version: 1,
        membership_id: Uuid::new_v4().to_string(),
        principal: principal.clone(),
        tenant_id: TENANT_A.into(),
        organization_scope: "identity-qa-organization".into(),
        revision: 1,
        active: true,
        capabilities: Default::default(),
    };
    let session_event = |session_id: &str| {
        let mut event = Event::new("qa.identity.session");
        event.session_id = Some(session_id.into());
        event
    };
    let mut tx = conn.start_transaction(TxOpts::new()).await?;
    let mut head = lock_head(&mut tx).await?;
    ensure!(
        &head == revoked,
        "identity QA audit changed before the session grant"
    );
    let record = insert_mutation(
        &mut tx,
        &mut head,
        4,
        IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: membership.clone(),
        },
    )
    .await?;
    tx.exec_drop(
        "INSERT INTO ai_guard_identity_memberships
         (membership_id,principal_key,organization_scope,active,payload_json,event_seq)
         VALUES (?,?,?,1,?,?)",
        (
            &membership.membership_id,
            principal.key()?,
            &membership.organization_scope,
            serde_json::to_vec(&membership)?,
            record.seq,
        ),
    )
    .await?;
    let approved_session = Uuid::new_v4().to_string();
    insert_event(&mut tx, &mut head, session_event(&approved_session)).await?;
    insert_session_grant(&mut tx, &mut head, 5, &membership, &approved_session).await?;
    set_identity_revision(&mut tx, 3, 5).await?;
    write_head(&mut tx, &head).await?;
    tx.commit().await?;
    ensure!(reader.has_local_credential_binding(&DIGEST_A).await?);
    ensure!(reader.verify().await? == head);
    let approved_prefix = fixture_prefix(conn, head.seq).await?;

    let mut tx = conn.start_transaction(TxOpts::new()).await?;
    let mut invalid_head = lock_head(&mut tx).await?;
    ensure!(
        invalid_head == head,
        "identity QA audit changed after the session grant"
    );
    let late_session = Uuid::new_v4().to_string();
    insert_session_grant(&mut tx, &mut invalid_head, 6, &membership, &late_session).await?;
    insert_event(&mut tx, &mut invalid_head, session_event(&late_session)).await?;
    set_identity_revision(&mut tx, 5, 6).await?;
    write_head(&mut tx, &invalid_head).await?;
    tx.commit().await?;

    let before_read = audit_snapshot(conn).await?;
    let refused = reader.has_local_credential_binding(&DIGEST_A).await;
    ensure!(audit_snapshot(conn).await? == before_read);
    ensure!(fixture_prefix(conn, head.seq).await? == approved_prefix);
    let error = refused
        .err()
        .context("a later session announcement validated its grant")?;
    ensure!(
        error.to_string() == "MySQL identity grant refers to an unknown prior resource",
        "unexpected identity QA resource grant error: {error}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires two fresh prepared private MySQL 8.4 QA schemas and a SELECT-only reader"]
async fn prepared_identity_binding_requires_verified_projection() -> Result<()> {
    let a = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let b = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
    ensure!(
        a.target.namespace != b.target.namespace,
        "identity QA targets must be distinct"
    );
    let mut conn_a = a.maintenance().await?;
    let mut conn_b = b.maintenance().await?;
    require_fresh(&mut conn_a).await?;
    require_fresh(&mut conn_b).await?;

    let initialized_a = a.initialize(TENANT_A).await?;
    let initialized_b = b.initialize(TENANT_B).await?;
    let reader_a = a.reader(TENANT_A, a.target.namespace)?;
    let reader_b = b.reader(TENANT_B, b.target.namespace)?;
    for (reader, initialized) in [(&reader_a, &initialized_a), (&reader_b, &initialized_b)] {
        let head = reader.verify().await?;
        ensure!(head.seq == 1 && initialized.seq == 1 && head.hash == initialized.hash);
    }
    let missing_capability = reader_a
        .has_local_credential_binding(&DIGEST_A)
        .await
        .expect_err("lookup without a verified identity capability must be refused");
    ensure!(
        missing_capability.downcast_ref::<IdentityFailureV1>()
            == Some(&IdentityFailureV1::MigrationRequired)
    );

    let (_, prepared_b) =
        prepare_identity(&mut conn_b, b.target.namespace, &initialized_b, &DIGEST_B).await?;
    let prefix_b = audit_snapshot(&mut conn_b).await?;
    let (credential, prepared_a) =
        prepare_identity(&mut conn_a, a.target.namespace, &initialized_a, &DIGEST_A).await?;
    let reader_a = a.reader(TENANT_A, a.target.namespace)?;
    let reader_b = b.reader(TENANT_B, b.target.namespace)?;
    ensure!(reader_a.has_local_credential_binding(&DIGEST_A).await?);
    ensure!(!reader_a.has_local_credential_binding(&DIGEST_B).await?);
    ensure!(reader_a.verify().await? == prepared_a);
    ensure!(reader_b.has_local_credential_binding(&DIGEST_B).await?);
    ensure!(!reader_b.has_local_credential_binding(&DIGEST_A).await?);
    ensure!(reader_b.verify().await? == prepared_b);
    for (wrong_reader, digest) in [
        (a.reader(TENANT_B, a.target.namespace)?, &DIGEST_A),
        (b.reader(TENANT_A, b.target.namespace)?, &DIGEST_B),
        (a.reader(TENANT_A, b.target.namespace)?, &DIGEST_A),
    ] {
        let refused = wrong_reader.has_local_credential_binding(digest).await;
        ensure!(
            refused
                .expect_err("wrong tenant or namespace must not read a binding")
                .to_string()
                == "MySQL audit binding mismatch"
        );
    }

    let prefix_a = fixture_prefix(&mut conn_a, prepared_a.seq).await?;
    let revoked = revoke_identity(&mut conn_a, &prepared_a, &credential).await?;
    ensure!(revoked.seq == prepared_a.seq + 1);
    ensure!(fixture_prefix(&mut conn_a, prepared_a.seq).await? == prefix_a);
    ensure!(reader_a.has_local_credential_binding(&DIGEST_A).await?);
    let reopened = a.reader(TENANT_A, a.target.namespace)?;
    ensure!(reopened.has_local_credential_binding(&DIGEST_A).await?);
    ensure!(!reopened.has_local_credential_binding(&DIGEST_B).await?);
    ensure!(reopened.verify().await? == revoked);

    let before_faults = audit_snapshot(&mut conn_a).await?;
    assert_genesis_refused(&mut conn_a, &reopened).await?;
    let cold = a.reader(TENANT_A, a.target.namespace)?;
    assert_projection_refused(&mut conn_a, &credential.credential_id, &reopened, &cold).await?;
    ensure!(
        audit_snapshot(&mut conn_a).await? == before_faults,
        "identity QA faults changed tenant A's audit state or journal"
    );
    ensure!(reader_a.verify().await? == revoked);
    let resource_grants =
        assert_historical_session_grants(&mut conn_a, &reopened, &revoked, &credential.principal)
            .await;
    ensure!(reader_b.verify().await? == prepared_b);
    ensure!(reader_b.has_local_credential_binding(&DIGEST_B).await?);
    ensure!(!reader_b.has_local_credential_binding(&DIGEST_A).await?);
    ensure!(
        audit_snapshot(&mut conn_b).await? == prefix_b,
        "tenant B's audit state and prefix changed during tenant A's identity operations"
    );
    ensure!(fixture_prefix(&mut conn_a, revoked.seq).await? == before_faults.1);
    resource_grants
}

fn registry_file_reference(file: &PrivateFile) -> Result<&str> {
    file.revalidate()?;
    file.path()
        .to_str()
        .context("registry QA requires a UTF-8 file reference")
}

pub(super) fn registry_entry(fixture: &FixtureFiles, tenant: &str) -> Result<Value> {
    let authentication = match &fixture.client_identity {
        Some((cert, key)) => json!({
            "mode": "password_and_mtls",
            "client_cert_file": registry_file_reference(cert)?,
            "client_key_file": registry_file_reference(key)?,
        }),
        None => json!({"mode": "password"}),
    };
    Ok(json!({
        "tenant_id": tenant,
        "runner_token_file": format!("unused-{tenant}-runner"),
        "viewer_token_file": format!("unused-{tenant}-viewer"),
        "storage": {
            "backend": "mysql",
            "host": fixture.target.host,
            "port": fixture.target.port,
            "database": fixture.target.database,
            "username": fixture.target.username,
            "namespace": fixture.target.namespace,
            "password_file": registry_file_reference(&fixture.reader_password)?,
            "ca_file": registry_file_reference(&fixture.ca)?,
            "authentication": authentication,
        },
    }))
}

async fn run_registry_probe(
    program: &TrustedExecutable,
    scratch: &Path,
    tenant: &str,
    entries: &[Value],
) -> Result<Output> {
    run_registry_probe_with_flags(program, scratch, tenant, entries, &[]).await
}

async fn run_registry_probe_with_flags(
    program: &TrustedExecutable,
    scratch: &Path,
    tenant: &str,
    entries: &[Value],
    flags: &[&str],
) -> Result<Output> {
    let mut registry = tempfile::NamedTempFile::new_in(scratch)?;
    serde_json::to_writer(
        registry.as_file_mut(),
        &json!({"version": 1, "tenants": entries}),
    )?;
    program.revalidate()?;
    let mut child = Command::new(program.path())
        .arg("--registry")
        .arg(registry.path())
        .args(["--tenant", tenant])
        .args(flags)
        .current_dir(scratch)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("cannot start registry QA probe")?;
    let stdout = child
        .stdout
        .take()
        .context("registry QA stdout unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("registry QA stderr unavailable")?;
    let observed = tokio::time::timeout(Duration::from_secs(12), async {
        tokio::join!(
            child.wait(),
            capture_stream(stdout, 4096),
            capture_stream(stderr, 4096),
        )
    })
    .await;
    let (status, stdout, stderr) = match observed {
        Ok(observed) => observed,
        Err(_) => {
            let _ = child.start_kill();
            tokio::time::timeout(Duration::from_secs(2), child.wait())
                .await
                .context("registry QA could not reap the probe")??;
            anyhow::bail!("registry QA probe exceeded its deadline");
        }
    };
    let (stdout, stderr) = (stdout?, stderr?);
    ensure!(
        !stdout.truncated && !stderr.truncated,
        "registry QA probe output exceeded its bound"
    );
    Ok(Output {
        status: status?,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
    })
}

fn assert_registry_probe_refusal(output: Output, expected: &str) -> Result<()> {
    let diagnostic = format!("Error: {expected}\n");
    ensure!(
        output.status.code() == Some(1)
            && output.stdout.is_empty()
            && output.stderr == diagnostic.as_bytes(),
        "registry QA expected a closed refusal for {expected}"
    );
    Ok(())
}

fn assert_registry_probe_receipt(output: Output, expected: &Value) -> Result<()> {
    ensure!(
        output.status.success() && output.stderr.is_empty(),
        "registry QA probe did not read the bound target"
    );
    let receipt: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| anyhow::anyhow!("registry QA probe did not return one JSON receipt"))?;
    ensure!(
        &receipt == expected,
        "registry QA probe returned an unexpected receipt"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires a built mysql_storage_probe and a fresh private MySQL 8.4 QA schema"]
async fn prepared_registry_probe_verifies_control_projections() -> Result<()> {
    let program = PathBuf::from(
        std::env::var_os("AI_GUARD_MYSQL_REGISTRY_QA_BIN")
            .context("absolute built registry probe path is required")?,
    );
    let program = TrustedExecutable::open(&program)?;
    let fixture = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let scratch = tempfile::Builder::new()
        .prefix("mysql-registry-control-")
        .permissions(Permissions::from_mode(0o700))
        .tempdir()?;
    let directory = scratch.path().canonicalize()?;
    let entries = [registry_entry(&fixture, TENANT_A)?];
    let flags = ["--verify-control-projections"];
    let mut conn = fixture.maintenance().await?;
    require_fresh(&mut conn).await?;

    let initial = fixture.initialize(TENANT_A).await?;
    let before = audit_snapshot(&mut conn).await?;
    assert_registry_probe_refusal(
        run_registry_probe_with_flags(&program, &directory, TENANT_A, &entries, &flags).await?,
        &IdentityFailureV1::MigrationRequired.to_string(),
    )?;
    ensure!(audit_snapshot(&mut conn).await? == before);

    let (_, identity_head) =
        prepare_identity(&mut conn, fixture.target.namespace, &initial, &DIGEST_A).await?;
    let before = audit_snapshot(&mut conn).await?;
    let authentication = &entries[0]["storage"]["authentication"]["mode"];
    assert_registry_probe_receipt(
        run_registry_probe(&program, &directory, TENANT_A, &entries).await?,
        &json!({
            "configured_authentication": authentication,
            "binding_verified": true,
            "audit_head": identity_head,
        }),
    )?;
    assert_registry_probe_refusal(
        run_registry_probe_with_flags(&program, &directory, TENANT_A, &entries, &flags).await?,
        &WorkloadFailureV1::MigrationRequired.to_string(),
    )?;
    ensure!(audit_snapshot(&mut conn).await? == before);

    let head = super::workload::prepare_capability(
        &mut conn,
        TENANT_A,
        fixture.target.namespace,
        (identity_head.seq, &identity_head.hash),
    )
    .await?;
    let before = audit_snapshot(&mut conn).await?;
    assert_registry_probe_receipt(
        run_registry_probe_with_flags(&program, &directory, TENANT_A, &entries, &flags).await?,
        &json!({
            "configured_authentication": authentication,
            "binding_verified": true,
            "audit_head": head,
            "control_projections_verified": true,
        }),
    )?;
    ensure!(audit_snapshot(&mut conn).await? == before);
    Ok(())
}

async fn assert_owned_origin_lock_released(conn: &mut Conn, lock: &str) -> Result<()> {
    let owner = conn
        .exec_first::<Option<u64>, _, _>("SELECT IS_USED_LOCK(?)", (lock,))
        .await
        .map_err(|_| anyhow::anyhow!("cannot observe the QA origin lock"))?;
    ensure!(owner == Some(None), "QA probe left its origin lock held");
    Ok(())
}

#[tokio::test]
#[ignore = "requires a built mysql_storage_probe and two fresh private MySQL 8.4 QA schemas"]
async fn prepared_registry_probe_verifies_owned_origin_and_preserves_audits() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(90), async {
        let program = PathBuf::from(
            std::env::var_os("AI_GUARD_MYSQL_REGISTRY_QA_BIN")
                .context("absolute built registry probe path is required")?,
        );
        let program = TrustedExecutable::open(&program)?;
        let a = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
        let b = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
        ensure!(
            a.target.port == b.target.port
                && a.target.namespace != b.target.namespace
                && a.target.username != b.target.username
                && a.target.maintenance_username != b.target.maintenance_username
                && a.client_identity.is_some() == b.client_identity.is_some(),
            "origin QA requires distinct schemas and accounts on one loopback profile"
        );
        let mut witness = maintenance_connection(
            b.target.endpoint(&b.target.username),
            b.credentials(&b.reader_password)?,
        )
        .await?;
        let update_plan = witness
            .query_drop("EXPLAIN UPDATE ai_guard_audit_state SET head_seq=head_seq WHERE singleton=1")
            .await;
        ensure!(
            matches!(update_plan, Err(mysql_async::Error::Server(error)) if matches!(error.code, 1142 | 1143)),
            "origin QA requires B's read-only service account"
        );
        witness
            .disconnect()
            .await
            .map_err(|_| anyhow::anyhow!("cannot close the QA witness preflight"))?;
        let mut conn_a = a.maintenance().await?;
        let mut conn_b = b.maintenance().await?;
        require_fresh(&mut conn_a).await?;
        require_fresh(&mut conn_b).await?;

        let scratch = tempfile::Builder::new()
            .prefix("mysql-registry-origin-")
            .permissions(Permissions::from_mode(0o700))
            .tempdir()?;
        let directory = scratch.path().canonicalize()?;
        let entries = [registry_entry(&a, TENANT_A)?, registry_entry(&b, TENANT_B)?];
        a.initialize(TENANT_A).await?;
        let initialized = b.initialize(TENANT_B).await?;
        let (_, identity_head) =
            prepare_identity(&mut conn_b, b.target.namespace, &initialized, &DIGEST_B).await?;
        let head = super::workload::prepare_capability(
            &mut conn_b,
            TENANT_B,
            b.target.namespace,
            (identity_head.seq, &identity_head.hash),
        )
        .await?;
        let before_a = audit_snapshot(&mut conn_a).await?;
        let before_b = audit_snapshot(&mut conn_b).await?;

        let server: String = conn_b
            .query_first("SELECT @@GLOBAL.server_uuid")
            .await
            .map_err(|_| anyhow::anyhow!("cannot read the QA server UUID"))?
            .context("QA server UUID is missing")?;
        let server = Uuid::parse_str(&server).context("invalid QA server UUID")?;
        let mut lock = Sha256::new();
        lock.update(b"ai-guard:owned-mysql-source:v1\0");
        lock.update(server.as_bytes());
        lock.update(b.target.database.as_bytes());
        let lock = hex::encode(lock.finalize());
        let server = server.to_string();
        let seq = head.seq.to_string();

        let flags = ["--verify-owned-origin", &server, &seq, &head.hash];
        let confirmed =
            run_registry_probe_with_flags(&program, &directory, TENANT_B, &entries, &flags).await?;
        assert_owned_origin_lock_released(&mut conn_b, &lock).await?;
        assert_registry_probe_receipt(
            confirmed,
            &json!({
                "configured_authentication": entries[1]["storage"]["authentication"]["mode"],
                "binding_verified": true,
                "audit_head": {
                    "seq": head.seq,
                    "hash": head.hash,
                    "payload_bytes": head.payload_bytes,
                },
                "control_projections_verified": true,
                "verification_mode": "owned_origin_preflight",
            }),
        )?;

        let wrong_seq = head
            .seq
            .checked_add(1)
            .context("QA audit sequence overflow")?
            .to_string();
        let flags = ["--verify-owned-origin", &server, &wrong_seq, &head.hash];
        let refused =
            run_registry_probe_with_flags(&program, &directory, TENANT_B, &entries, &flags).await?;
        assert_owned_origin_lock_released(&mut conn_b, &lock).await?;
        assert_registry_probe_refusal(
            refused,
            "MySQL owned audit head differs from the expected head",
        )?;
        ensure!(
            audit_snapshot(&mut conn_a).await? == before_a,
            "origin QA changed tenant A"
        );
        ensure!(
            audit_snapshot(&mut conn_b).await? == before_b,
            "origin QA changed tenant B"
        );
        Ok(())
    })
    .await
    .context("owned origin QA exceeded its deadline")?
}

#[tokio::test]
#[ignore = "requires a built mysql_storage_probe and two fresh private MySQL 8.4 QA schemas"]
async fn prepared_registry_probe_preserves_bound_target() -> Result<()> {
    let program = PathBuf::from(
        std::env::var_os("AI_GUARD_MYSQL_REGISTRY_QA_BIN")
            .context("absolute built registry probe path is required")?,
    );
    let program = TrustedExecutable::open(&program)?;
    let a = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let b = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
    ensure!(
        a.target.port != 0
            && a.target.port == b.target.port
            && a.target.namespace != b.target.namespace
            && a.client_identity.is_some() == b.client_identity.is_some(),
        "registry QA requires separate schemas on one loopback profile"
    );
    let scratch = tempfile::Builder::new()
        .prefix("mysql-registry-")
        .permissions(Permissions::from_mode(0o700))
        .tempdir()?;
    let directory = scratch.path().canonicalize()?;
    let entries = [registry_entry(&a, TENANT_A)?, registry_entry(&b, TENANT_B)?];
    let mut conn_a = a.maintenance().await?;
    let mut conn_b = b.maintenance().await?;
    require_fresh(&mut conn_a).await?;
    require_fresh(&mut conn_b).await?;
    let binding_mismatch = "MySQL audit binding mismatch";
    assert_registry_probe_refusal(
        run_registry_probe(&program, &directory, TENANT_B, &entries).await?,
        binding_mismatch,
    )?;
    require_fresh(&mut conn_a).await?;
    require_fresh(&mut conn_b).await?;

    a.initialize(TENANT_A).await?;
    let initialized = b.initialize(TENANT_B).await?;
    let (_, head) =
        prepare_identity(&mut conn_b, b.target.namespace, &initialized, &DIGEST_B).await?;
    let prefix_a = audit_snapshot(&mut conn_a).await?;
    let prefix_b = audit_snapshot(&mut conn_b).await?;
    let mut wrong_tenant = entries[1].clone();
    wrong_tenant["tenant_id"] = json!(TENANT_A);
    let mut wrong_namespace = entries[1].clone();
    wrong_namespace["storage"]["namespace"] = json!(a.target.namespace);

    let expected = json!({
        "configured_authentication": entries[1]["storage"]["authentication"]["mode"],
        "binding_verified": true,
        "audit_head": head,
    });
    assert_registry_probe_receipt(
        run_registry_probe(&program, &directory, TENANT_B, &entries).await?,
        &expected,
    )?;
    assert_registry_probe_refusal(
        run_registry_probe(&program, &directory, TENANT_A, &[wrong_tenant]).await?,
        binding_mismatch,
    )?;
    assert_registry_probe_refusal(
        run_registry_probe(&program, &directory, TENANT_B, &[wrong_namespace]).await?,
        binding_mismatch,
    )?;

    if b.client_identity.is_some() {
        let login_refusal = "MySQL refused the probe (server code 1045)";
        let mut without_certificate = entries[1].clone();
        without_certificate["storage"]["authentication"] = json!({"mode": "password"});
        assert_registry_probe_refusal(
            run_registry_probe(&program, &directory, TENANT_B, &[without_certificate]).await?,
            login_refusal,
        )?;

        let mut wrong_password = entries[1].clone();
        let mut invalid = tempfile::NamedTempFile::new_in(&directory)?;
        let fake = format!("invalid-registry-qa-{}", Uuid::new_v4());
        ensure!(
            b.credentials(&b.reader_password)?.password != fake,
            "registry QA synthetic password collides with the fixture"
        );
        invalid.as_file_mut().write_all(fake.as_bytes())?;
        let invalid_path = invalid
            .path()
            .to_str()
            .context("registry QA requires a UTF-8 file reference")?;
        wrong_password["storage"]["password_file"] = json!(invalid_path);
        assert_registry_probe_refusal(
            run_registry_probe(&program, &directory, TENANT_B, &[wrong_password]).await?,
            login_refusal,
        )?;
    }

    assert_registry_probe_receipt(
        run_registry_probe(&program, &directory, TENANT_B, &entries).await?,
        &expected,
    )?;
    ensure!(
        audit_snapshot(&mut conn_a).await? == prefix_a,
        "registry QA changed tenant A"
    );
    ensure!(
        audit_snapshot(&mut conn_b).await? == prefix_b,
        "registry QA changed tenant B"
    );
    Ok(())
}
