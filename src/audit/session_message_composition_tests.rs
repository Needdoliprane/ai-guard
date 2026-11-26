//! End-to-end broker composition regressions for SendSessionMessage: a real
//! adopted owned source (Store14, real maintenance guard, journal and verified
//! backup), the real durable effect points, and the real broker composition
//! (`broker::conversation`) driving a controlled stdio fixture child. No
//! provider, credential or network is involved: real CLI reception stays
//! NOT_RUN. The fixture child is the same controlled JSON-RPC peer used by the
//! adapter tests, re-executed from this test binary.
use super::*;
use crate::audit::review_authority as authority;
use crate::broker::conversation::SessionConversationComposerV1;
use crate::control::session_control::{
    ProvisionSessionLaunchTargetV1, SessionConversationChannelV1, SessionConversationStatusV1,
    SessionLaunchPrepareRequestV1, SessionLaunchTargetRecordV1, SessionMessageStatusV1,
};
use crate::control::session_message::StdioConversation;
use crate::control_fs::{Directory, PrivateFile};
use crate::human_authority::migration::egress_journal::RetainedSourceIntent;
use crate::human_authority::{
    HumanAuthorityClosureV1, HumanAuthorityConfigurationV1, HumanAuthorityMarkerReceiptV1,
    HumanConfigurationFileKindV1, HumanConfigurationFileV1, HumanSourceDescriptorV1,
    HumanSourceKindV1, HumanSourcesConfigurationV1, HumanStorageProfileV1, OwnedHumanSources,
    OwnedSourceToken,
    maintenance::{
        HumanMaintenanceResolvedViewV1, HumanMaintenanceSourceToken, HumanMaintenanceSourceV1,
        HumanMaintenanceViewV1, HumanSourcesMaintenanceConfigurationV1,
        HumanSourcesMaintenanceGuard, HumanSourcesMaintenancePlanV1,
    },
};
use crate::identity::{AuthorizedScopeV1, ContextModeV1, MembershipV1, PrincipalRefV1};
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

const FUTURE_DEADLINE: &str = "2126-01-01T00:00:00Z";

fn binding(parent: &Directory, name: &str) -> crate::human_authority::SourceStorageBindingV1 {
    let (parent_device, parent_inode) = parent.physical_identity().unwrap();
    let (source_device, source_inode) = parent
        .private_entry(std::ffi::OsStr::new(name))
        .unwrap()
        .unwrap()
        .physical_identity();
    crate::human_authority::SourceStorageBindingV1 {
        parent_path: parent.path().to_owned(),
        file_name: name.into(),
        parent_device,
        parent_inode,
        source_device,
        source_inode,
    }
}

/// One adopted owned source at format 14 with the identity markers installed,
/// as the gateway retains it. Single tenant "A"; the neighbor isolation is
/// proven by the marker fixtures, not repeated here.
struct Harness {
    _temporary: tempfile::TempDir,
    root: Directory,
    plan: HumanSourcesMaintenancePlanV1,
    plan_file: PrivateFile,
    configuration_files: Vec<(HumanConfigurationFileKindV1, PrivateFile)>,
    anchors: Vec<authority::AuthorityReceiptV1>,
}

impl Harness {
    fn new() -> Self {
        let temporary = tempfile::Builder::new()
            .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let root = Directory::open_existing(temporary.path()).unwrap();
        let installation_id = Uuid::new_v4();
        let parent = root.persistent_child("source-0", true).unwrap().unwrap();
        let store = Store::open_for_tenant(
            &parent.path().join("tenant.sqlite"),
            "A",
            Redactor::new(vec![]),
            10_000,
        )
        .unwrap();
        for target in 3..=13 {
            store
                .migrate_control_version(
                    &root.path().join(format!("history-0-{target}.sqlite")),
                    target - 1,
                    target,
                )
                .unwrap();
        }
        let namespace = authority::namespace(&store.db.lock().unwrap()).unwrap();
        drop(store);
        let sources = vec![HumanMaintenanceSourceV1 {
            source_id: Uuid::from_u128(1),
            kind: HumanSourceKindV1::TenantStore,
            installation_id,
            durable_namespace_id: namespace,
            tenant_id: Some("A".into()),
            storage_binding: binding(&parent, "tenant.sqlite"),
            required_format: 14,
        }];
        let config = root
            .persistent_child("configuration", true)
            .unwrap()
            .unwrap();
        let control = config
            .persistent_create("control.json", b"synthetic configuration")
            .unwrap();
        let envelope = json!({"version":1,"installation_id":installation_id,"closure_id":Uuid::from_u128(9000),"closure_revision":1,"control_config_public_digest":"ac".repeat(32),"entries":[
            {"tenant_id":"A","role":"runner","credential_id":Uuid::from_u128(100),"generation":1,"token":"synthetic-A-runner"},
            {"tenant_id":"A","role":"viewer","credential_id":Uuid::from_u128(101),"generation":1,"token":"synthetic-A-viewer"}
        ]});
        let credentials = config
            .persistent_create("credentials.json", &serde_json::to_vec(&envelope).unwrap())
            .unwrap();
        let configuration = HumanAuthorityConfigurationV1 {
            control_config_revision: 1,
            public_config_digest: "ac".repeat(32),
            oidc: None,
            files: vec![
                HumanConfigurationFileV1 {
                    kind: HumanConfigurationFileKindV1::ControlConfiguration,
                    storage_binding: binding(&config, "control.json"),
                },
                HumanConfigurationFileV1 {
                    kind: HumanConfigurationFileKindV1::TechnicalCredentials,
                    storage_binding: binding(&config, "credentials.json"),
                },
            ],
        };
        let plan = HumanSourcesMaintenancePlanV1 {
            version: 1,
            migration_id: Uuid::new_v4(),
            installation_id,
            prior: None,
            target: HumanMaintenanceViewV1 {
                sources,
                configuration,
            },
            prior_active_manifest: None,
            supported_storage_profile: HumanStorageProfileV1::LocalPosixFlockV1,
            selected_source_id: None,
        };
        let plan_file = config
            .persistent_create("maintenance.json", &serde_json::to_vec(&plan).unwrap())
            .unwrap();
        let configuration_files = vec![
            (HumanConfigurationFileKindV1::ControlConfiguration, control),
            (
                HumanConfigurationFileKindV1::TechnicalCredentials,
                credentials,
            ),
        ];
        // Adopt format 14 through the real maintenance writer and backup.
        let target = HumanMaintenanceResolvedViewV1::new(
            plan.target.sources.clone(),
            plan.target.configuration.clone(),
            configuration_files.clone(),
        )
        .unwrap();
        let guard = HumanSourcesMaintenanceGuard::acquire(
            plan_file.clone(),
            HumanSourcesMaintenanceConfigurationV1::new(None, target, None),
        )
        .unwrap();
        let token = guard
            .target_source(plan.target.sources[0].source_id)
            .unwrap();
        let backups = root.persistent_child("backups", true).unwrap().unwrap();
        let opened = Self::open_maintenance(&plan, &token);
        let receipt = opened
            .with_review_maintenance_writer(&token, |writer| {
                let backup =
                    writer.prepare_backup(&token, &backups.path().join("source-0.sqlite"))?;
                let receipt = writer.install_format14(&token, &backup)?;
                writer.commit()?;
                Ok(receipt)
            })
            .unwrap();
        drop(opened);
        drop(guard);
        Self {
            _temporary: temporary,
            root,
            plan,
            plan_file,
            configuration_files,
            anchors: vec![receipt],
        }
    }

    fn source_id(&self) -> Uuid {
        self.plan.target.sources[0].source_id
    }

    fn configuration_dir(&self) -> Directory {
        Directory::open_existing(self.plan_file.path().parent().unwrap()).unwrap()
    }

    fn open_maintenance(
        plan: &HumanSourcesMaintenancePlanV1,
        token: &HumanMaintenanceSourceToken,
    ) -> crate::audit::review_startup::ReviewStoreMaintenance {
        let source = &plan.target.sources[0];
        Store::open_review_maintenance_for_tenant(
            &source.storage_binding.path(),
            "A",
            Redactor::new(vec![]),
            10_000,
            token,
        )
        .unwrap()
    }

    fn owner(&self, manifest_name: &str) -> OwnedHumanSources {
        let source = &self.plan.target.sources[0];
        let receipt = &self.anchors[0];
        let descriptors = vec![HumanSourceDescriptorV1 {
            source_id: source.source_id,
            kind: source.kind,
            installation_id: source.installation_id,
            durable_namespace_id: source.durable_namespace_id,
            tenant_id: source.tenant_id.clone(),
            storage_binding: source.storage_binding.clone(),
            required_format: 14,
            authority_marker_receipt: HumanAuthorityMarkerReceiptV1 {
                seq: receipt.seq,
                hash: receipt.hash.clone(),
            },
        }];
        let mut closure = HumanAuthorityClosureV1 {
            version: 1,
            installation_id: self.plan.installation_id,
            closure_id: Uuid::from_u128(9000),
            closure_revision: 1,
            closure_digest: String::new(),
            sources: descriptors.clone(),
            configuration: self.plan.target.configuration.clone(),
            supported_storage_profile: HumanStorageProfileV1::LocalPosixFlockV1,
        };
        closure.closure_digest = closure.compute_digest().unwrap();
        let config = self.configuration_dir();
        let manifest = config
            .persistent_create(manifest_name, &serde_json::to_vec(&closure).unwrap())
            .unwrap();
        OwnedHumanSources::acquire(
            manifest,
            HumanSourcesConfigurationV1::new(
                descriptors,
                closure.configuration,
                self.configuration_files.clone(),
            )
            .unwrap(),
        )
        .unwrap()
    }

    /// Install the identity markers (human rights + session prepare) through
    /// the real journaled in-place maintenance path, with verified backup.
    fn install_markers(&self) {
        let manifest_name = "active14-composition.json";
        let closure = {
            let owner = self.owner(manifest_name);
            let closure = owner.closure().clone();
            drop(owner);
            closure
        };
        let configuration = self.configuration_dir();
        let manifest = PrivateFile::open(
            &configuration.path().join(manifest_name),
            crate::human_authority::MAX_HUMAN_MANIFEST_BYTES as u64,
        )
        .unwrap();
        let plan = HumanSourcesMaintenancePlanV1::in_place(
            Uuid::new_v4(),
            &closure,
            binding(&configuration, manifest_name),
            self.source_id(),
        );
        let file = configuration
            .persistent_create(
                &format!("in-place-{}.json", plan.migration_id),
                &serde_json::to_vec(&plan).unwrap(),
            )
            .unwrap();
        let resolve = || {
            HumanMaintenanceResolvedViewV1::new(
                plan.target.sources.clone(),
                plan.target.configuration.clone(),
                self.configuration_files.clone(),
            )
            .unwrap()
        };
        let guard = HumanSourcesMaintenanceGuard::acquire(
            file.clone(),
            HumanSourcesMaintenanceConfigurationV1::new(Some(resolve()), resolve(), Some(manifest)),
        )
        .unwrap();
        let token = guard.target_source(self.source_id()).unwrap();
        let journal = self
            .root
            .persistent_child("composition-journal", true)
            .unwrap()
            .unwrap();
        let backups = self
            .root
            .persistent_child("composition-backup", true)
            .unwrap()
            .unwrap();
        let opened = Self::open_maintenance(&self.plan, &token);
        let intent = opened
            .with_review_maintenance_writer(&token, |writer| {
                let intent = RetainedSourceIntent::prepare_identity_markers(
                    &token, writer, journal, backups,
                )?;
                let backup = intent.prepare_backup(writer)?;
                backup.install_identity_markers(writer)?;
                drop(backup);
                writer.commit()?;
                Ok(intent)
            })
            .unwrap();
        drop(opened);
        intent
            .confirm_identity_markers(Redactor::new(vec![]), 10_000)
            .unwrap();
        drop(guard);
    }

    /// Reopen the source through a fresh owner startup, replaying every event,
    /// and retain it as the gateway would.
    fn retained(&self, manifest: &str) -> Retained {
        let owner = self.owner(manifest);
        let source = owner.source_token(self.source_id()).unwrap();
        let descriptor = &self.plan.target.sources[0];
        let startup = Store::open_review_startup_for_tenant(
            &descriptor.storage_binding.path(),
            "A",
            Redactor::new(vec![]),
            160,
            &source,
        )
        .unwrap();
        let epoch = owner.try_write_epoch().unwrap();
        let revision = startup.current_incarnation_revision(&epoch).unwrap();
        startup
            .append_incarnation(&epoch, &source, Uuid::new_v4(), revision)
            .unwrap();
        let store = startup.retain(&epoch, &source).unwrap();
        drop(epoch);
        Retained {
            store,
            source,
            owner,
        }
    }
}

struct Retained {
    store: Store,
    source: OwnedSourceToken,
    owner: OwnedHumanSources,
}

fn provision_membership(retained: &Retained) -> anyhow::Result<(PrincipalRefV1, MembershipV1)> {
    use crate::identity::*;
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: retained.source.descriptor().installation_id.to_string(),
        account_id: Uuid::new_v4().to_string(),
    };
    let membership = MembershipV1 {
        version: 1,
        membership_id: Uuid::new_v4().to_string(),
        principal: principal.clone(),
        tenant_id: "A".into(),
        organization_scope: "org-a".into(),
        revision: 1,
        active: true,
        capabilities: [
            HumanCapabilityV1::LaunchSession,
            HumanCapabilityV1::SendSessionMessage,
            HumanCapabilityV1::ReadTenantMetadata,
        ]
        .into_iter()
        .collect(),
    };
    let mutation = |change| IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "synthetic-offline-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    };
    let provision = LocalHumanProvisionV1 {
        version: 1,
        mutations: [
            mutation(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Synthetic composer".into(),
                    revision: 1,
                    active: true,
                },
            }),
            mutation(IdentityChangeV1::Credential {
                expected_revision: 0,
                binding: LocalCredentialV1 {
                    version: 1,
                    credential_id: Uuid::new_v4().to_string(),
                    principal: principal.clone(),
                    sha256_digest: format!(
                        "{}{}",
                        Uuid::new_v4().simple(),
                        Uuid::new_v4().simple()
                    ),
                    revision: 1,
                    active: true,
                    expires_at: None,
                },
            }),
            mutation(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: membership.clone(),
            }),
        ],
    };
    let epoch = retained.owner.try_write_epoch()?;
    retained
        .store
        .provision_local_human_scoped(&epoch, &retained.source, &provision)?;
    Ok((principal, membership))
}

fn personal_scope(
    retained: &Retained,
    principal: &PrincipalRefV1,
    membership: &MembershipV1,
) -> AuthorizedScopeV1 {
    let revision = {
        let db = retained.store.db.lock().unwrap();
        crate::audit::identity::authorization_revision(&db).unwrap()
    };
    crate::identity::authorize_context(
        principal,
        membership,
        ContextModeV1::Personal,
        membership.revision,
    )
    .unwrap()
    .at_authorization_revision(revision)
}

fn generation(retained: &Retained) -> crate::audit::human_modern::HumanDiscoveryGeneration {
    let epoch = retained.owner.try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &retained.source).unwrap();
    let mut db = retained.store.db.lock().unwrap();
    let tx = retained
        .store
        .begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)
        .unwrap();
    let generation = crate::audit::human_modern::discovery_generation(&tx).unwrap();
    drop(tx);
    generation
}

fn launched_session(
    retained: &Retained,
    scope: &AuthorizedScopeV1,
    principal: &PrincipalRefV1,
) -> crate::control::session_control::SessionLaunchReceiptV1 {
    let target_id = Uuid::new_v4();
    let epoch = retained.owner.try_write_epoch().unwrap();
    retained
        .store
        .provision_session_launch_target_scoped(
            &epoch,
            &retained.source,
            &ProvisionSessionLaunchTargetV1 {
                version: 1,
                operation_id: Uuid::new_v4(),
                expected_revision: 0,
                target: SessionLaunchTargetRecordV1 {
                    version: 1,
                    launch_target_id: target_id,
                    revision: 1,
                    tenant_id: "A".into(),
                    organization_scope: "org-a".into(),
                    active: true,
                    launchers: vec![principal.clone()],
                },
            },
        )
        .unwrap();
    drop(epoch);
    let expected = generation(retained);
    let epoch = retained.owner.try_write_epoch().unwrap();
    let (preparation, _) = retained
        .store
        .modern_session_launch_prepare(
            &epoch,
            &retained.source,
            scope,
            &SessionLaunchPrepareRequestV1 {
                version: 1,
                operation_id: Uuid::new_v4(),
                launch_target_id: target_id,
                expected_target_revision: 1,
                expires_at: FUTURE_DEADLINE.into(),
            },
            expected,
        )
        .unwrap();
    drop(epoch);
    let expected = generation(retained);
    let epoch = retained.owner.try_write_epoch().unwrap();
    let (launch, _) = retained
        .store
        .modern_session_launch(
            &epoch,
            &retained.source,
            scope,
            &crate::control::session_control::SessionLaunchRequestV1 {
                version: 1,
                operation_id: Uuid::new_v4(),
                launch_target_id: target_id,
                expected_target_revision: 1,
                preparation_operation_id: preparation.operation_id,
                preparation_input_hash: preparation.input_hash.clone(),
            },
            expected,
        )
        .unwrap();
    launch
}

fn send_message(
    retained: &Retained,
    scope: &AuthorizedScopeV1,
    operation_id: Uuid,
    session_id: Uuid,
    expected_input_epoch: u64,
    content: &str,
) -> anyhow::Result<crate::control::session_control::SessionMessageReceiptV1> {
    let expected = generation(retained);
    let epoch = retained.owner.try_write_epoch()?;
    retained
        .store
        .modern_session_message_send(
            &epoch,
            &retained.source,
            scope,
            &crate::control::session_control::SessionMessageRequestV1 {
                version: 1,
                operation_id,
                session_id,
                expected_input_epoch,
                content: content.into(),
            },
            expected,
        )
        .map(|(receipt, _)| receipt)
}

fn message_failure(error: &anyhow::Error) -> Option<SessionMessageFailureV1> {
    error.downcast_ref::<SessionMessageFailureV1>().copied()
}

fn channel_facts(attempt: &str) -> SessionConversationChannelV1 {
    SessionConversationChannelV1 {
        broker_id: "broker-1".into(),
        broker_incarnation: "incarnation-1".into(),
        worker_id: "worker-1".into(),
        attempt_id: attempt.into(),
        runtime_instance: format!("runtime-{attempt}"),
        provider: "codex".into(),
        protocol: "codex-app-server-v2".into(),
    }
}

/// Spawn the controlled JSON-RPC fixture child (the adapter tests' peer) with
/// its own mode in the child environment. It is a fixture, not a provider.
fn spawn_fixture(mode: &str) -> StdioConversation {
    let exe = std::env::current_exe().unwrap();
    let args: Vec<std::ffi::OsString> = vec![
        "--exact".into(),
        "control::session_message::tests::conversation_fixture_child".into(),
        "--ignored".into(),
        "--nocapture".into(),
    ];
    let env = [(
        std::ffi::OsString::from("AI_GUARD_CONV_FIXTURE_MODE"),
        std::ffi::OsString::from(mode),
    )];
    StdioConversation::spawn_with_env(exe.as_os_str(), &args, &env).unwrap()
}

fn event_kinds(store: &Store) -> Vec<String> {
    let db = store.db.lock().unwrap();
    db.prepare("SELECT kind FROM events WHERE kind LIKE 'control.session_message.%' ORDER BY seq")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn kind_count(store: &Store, kind: &str) -> i64 {
    let db = store.db.lock().unwrap();
    db.query_row("SELECT count(*) FROM events WHERE kind=?1", [kind], |r| {
        r.get(0)
    })
    .unwrap()
}

fn message_status(store: &Store, operation_id: Uuid) -> String {
    let db = store.db.lock().unwrap();
    db.query_row(
        "SELECT status FROM session_messages WHERE operation_id=?1",
        [operation_id.to_string()],
        |r| r.get(0),
    )
    .unwrap()
}

/// The composed journey: launch retains the binding, the admission binds the
/// exact channel durably, two queued instructions reach the fixture worker as
/// complete framed turns and are acknowledged by the exact correlated
/// responses, then STOP closes the right to emit before the channel.
#[test]
fn composition_delivers_two_turns_then_stop_closes_right_and_channel() {
    let harness = Harness::new();
    harness.install_markers();
    let retained = harness.retained("composition-two-turns.json");
    let (principal, membership) = provision_membership(&retained).unwrap();
    let scope = personal_scope(&retained, &principal, &membership);
    let session = launched_session(&retained, &scope, &principal);
    let session_id = session.session_id.clone();
    let session_uuid = Uuid::parse_str(&session_id).unwrap();
    let composer = SessionConversationComposerV1::new(
        &retained.store,
        &retained.owner,
        &retained.source,
        Duration::from_secs(10),
    )
    .unwrap();

    // Admission: the durable bind precedes the attached channel.
    let binding = composer
        .admit_channel(
            Uuid::new_v4(),
            &session_id,
            0,
            channel_facts("attempt-1"),
            spawn_fixture("normal"),
        )
        .unwrap();
    assert_eq!(binding.status, SessionConversationStatusV1::Active);
    assert_eq!(binding.input_epoch, 1);
    assert_eq!(binding.worker_id.as_deref(), Some("worker-1"));

    // First instruction: queued durably, then delivered and acknowledged.
    let body_one = "synthetic instruction one: summarize the fixture report";
    let operation_one = Uuid::new_v4();
    let queued = send_message(&retained, &scope, operation_one, session_uuid, 1, body_one).unwrap();
    assert_eq!(queued.status, SessionMessageStatusV1::Queued);
    let delivered = composer.dispatch(&session_id).unwrap();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].operation_id, operation_one);
    assert_eq!(delivered[0].status, SessionMessageStatusV1::Acknowledged);
    let ack = delivered[0].ack.as_ref().unwrap();
    assert_eq!(ack.rpc_id, 1);
    assert_eq!(ack.turn_id, "turn-1");

    // Second instruction on the same admitted channel: one turn at a time.
    let body_two = "synthetic instruction two: list the fixture findings";
    let operation_two = Uuid::new_v4();
    send_message(&retained, &scope, operation_two, session_uuid, 1, body_two).unwrap();
    let delivered = composer.dispatch(&session_id).unwrap();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].status, SessionMessageStatusV1::Acknowledged);
    let ack = delivered[0].ack.as_ref().unwrap();
    assert_eq!(ack.rpc_id, 2);
    assert_eq!(ack.turn_id, "turn-2");

    // The durable chain records each distinct fact; the body never enters it.
    assert_eq!(
        event_kinds(&retained.store),
        vec![
            "control.session_message.conversation_bound",
            "control.session_message.enqueued",
            "control.session_message.emission_issued",
            "control.session_message.transmitted",
            "control.session_message.acknowledged",
            "control.session_message.enqueued",
            "control.session_message.emission_issued",
            "control.session_message.transmitted",
            "control.session_message.acknowledged",
        ]
    );
    {
        let db = retained.store.db.lock().unwrap();
        let payloads: Vec<String> = db
            .prepare("SELECT payload FROM events")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        for payload in payloads {
            assert!(!payload.contains(body_one) && !payload.contains(body_two));
        }
    }

    // STOP: the right to emit closes durably, then the channel closes.
    let closed = composer.stop(Uuid::new_v4(), &session_id).unwrap();
    assert_eq!(closed.status, SessionConversationStatusV1::Closed);
    let refusal = send_message(
        &retained,
        &scope,
        Uuid::new_v4(),
        session_uuid,
        1,
        "synthetic instruction after stop",
    )
    .unwrap_err();
    assert_eq!(
        message_failure(&refusal),
        Some(SessionMessageFailureV1::SessionClosed)
    );
    let refusal = composer.dispatch(&session_id).unwrap_err();
    assert_eq!(
        message_failure(&refusal),
        Some(SessionMessageFailureV1::SessionClosed)
    );
    assert_eq!(
        event_kinds(&retained.store).last().unwrap(),
        "control.session_message.conversation_closed"
    );
}

/// A discordant response id never acknowledges; the bounded reconciliation
/// ends without a new fact and the message is never resent.
#[test]
fn composition_discordant_id_never_acknowledges_nor_resends() {
    let harness = Harness::new();
    harness.install_markers();
    let retained = harness.retained("composition-discordant.json");
    let (principal, membership) = provision_membership(&retained).unwrap();
    let scope = personal_scope(&retained, &principal, &membership);
    let session = launched_session(&retained, &scope, &principal);
    let session_uuid = Uuid::parse_str(&session.session_id).unwrap();
    let composer = SessionConversationComposerV1::new(
        &retained.store,
        &retained.owner,
        &retained.source,
        Duration::from_millis(500),
    )
    .unwrap();
    composer
        .admit_channel(
            Uuid::new_v4(),
            &session.session_id,
            0,
            channel_facts("attempt-1"),
            spawn_fixture("discordant"),
        )
        .unwrap();
    let operation = Uuid::new_v4();
    send_message(
        &retained,
        &scope,
        operation,
        session_uuid,
        1,
        "synthetic instruction",
    )
    .unwrap();
    let delivered = composer.dispatch(&session.session_id).unwrap();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].status, SessionMessageStatusV1::Transmitted);
    assert!(delivered[0].ack.is_none());
    // No automatic resend: the issued message is no longer queued.
    assert!(composer.dispatch(&session.session_id).unwrap().is_empty());
    assert_eq!(
        kind_count(&retained.store, "control.session_message.emission_issued"),
        1
    );
    assert_eq!(
        kind_count(&retained.store, "control.session_message.transmitted"),
        1
    );
    assert_eq!(
        kind_count(&retained.store, "control.session_message.acknowledged"),
        0
    );
}

/// A cut after a possible emission keeps the truthful durable status and never
/// resends; a channel already closed before the write retains `unknown`.
#[test]
fn composition_cut_after_emission_retains_uncertainty() {
    let harness = Harness::new();
    harness.install_markers();
    let retained = harness.retained("composition-cut.json");
    let (principal, membership) = provision_membership(&retained).unwrap();
    let scope = personal_scope(&retained, &principal, &membership);
    let session = launched_session(&retained, &scope, &principal);
    let session_uuid = Uuid::parse_str(&session.session_id).unwrap();
    let composer = SessionConversationComposerV1::new(
        &retained.store,
        &retained.owner,
        &retained.source,
        Duration::from_secs(2),
    )
    .unwrap();
    composer
        .admit_channel(
            Uuid::new_v4(),
            &session.session_id,
            0,
            channel_facts("attempt-1"),
            spawn_fixture("cut"),
        )
        .unwrap();
    let operation = Uuid::new_v4();
    send_message(
        &retained,
        &scope,
        operation,
        session_uuid,
        1,
        "synthetic instruction",
    )
    .unwrap();
    let delivered = composer.dispatch(&session.session_id).unwrap();
    assert_eq!(delivered.len(), 1);
    assert_ne!(delivered[0].status, SessionMessageStatusV1::Acknowledged);
    assert!(delivered[0].ack.is_none());
    assert!(composer.dispatch(&session.session_id).unwrap().is_empty());
    assert_eq!(
        kind_count(&retained.store, "control.session_message.acknowledged"),
        0
    );
    assert_eq!(
        kind_count(&retained.store, "control.session_message.emission_issued"),
        1
    );

    // A channel closed before the write: the latch retains `unknown` exactly.
    let session_two = launched_session(&retained, &scope, &principal);
    let session_two_uuid = Uuid::parse_str(&session_two.session_id).unwrap();
    let mut dead = spawn_fixture("normal");
    dead.close();
    composer
        .admit_channel(
            Uuid::new_v4(),
            &session_two.session_id,
            0,
            channel_facts("attempt-1"),
            dead,
        )
        .unwrap();
    let operation_two = Uuid::new_v4();
    send_message(
        &retained,
        &scope,
        operation_two,
        session_two_uuid,
        1,
        "synthetic instruction",
    )
    .unwrap();
    assert!(composer.dispatch(&session_two.session_id).is_err());
    assert_eq!(message_status(&retained.store, operation_two), "unknown");
    assert!(
        composer
            .dispatch(&session_two.session_id)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        kind_count(&retained.store, "control.session_message.emission_issued"),
        2
    );
    assert_eq!(
        kind_count(&retained.store, "control.session_message.transmitted"),
        1
    );
}

/// Duplicate response and outcome frames acknowledge exactly once.
#[test]
fn composition_duplicate_frames_acknowledge_once() {
    let harness = Harness::new();
    harness.install_markers();
    let retained = harness.retained("composition-duplicate.json");
    let (principal, membership) = provision_membership(&retained).unwrap();
    let scope = personal_scope(&retained, &principal, &membership);
    let session = launched_session(&retained, &scope, &principal);
    let session_uuid = Uuid::parse_str(&session.session_id).unwrap();
    let composer = SessionConversationComposerV1::new(
        &retained.store,
        &retained.owner,
        &retained.source,
        Duration::from_secs(10),
    )
    .unwrap();
    composer
        .admit_channel(
            Uuid::new_v4(),
            &session.session_id,
            0,
            channel_facts("attempt-1"),
            spawn_fixture("duplicate"),
        )
        .unwrap();
    let operation = Uuid::new_v4();
    send_message(
        &retained,
        &scope,
        operation,
        session_uuid,
        1,
        "synthetic instruction",
    )
    .unwrap();
    let delivered = composer.dispatch(&session.session_id).unwrap();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].status, SessionMessageStatusV1::Acknowledged);
    assert_eq!(
        kind_count(&retained.store, "control.session_message.acknowledged"),
        1
    );
    assert!(composer.dispatch(&session.session_id).unwrap().is_empty());
}

/// STOP racing a dispatch: the durable close keeps priority; no emission is
/// ever issued after the conversation closed, whatever the interleaving.
#[test]
fn composition_stop_races_dispatch_without_emitting_after_close() {
    let harness = Harness::new();
    harness.install_markers();
    let retained = harness.retained("composition-race.json");
    let (principal, membership) = provision_membership(&retained).unwrap();
    let scope = personal_scope(&retained, &principal, &membership);
    let session = launched_session(&retained, &scope, &principal);
    let session_uuid = Uuid::parse_str(&session.session_id).unwrap();
    let composer = SessionConversationComposerV1::new(
        &retained.store,
        &retained.owner,
        &retained.source,
        Duration::from_secs(10),
    )
    .unwrap();
    composer
        .admit_channel(
            Uuid::new_v4(),
            &session.session_id,
            0,
            channel_facts("attempt-1"),
            spawn_fixture("normal"),
        )
        .unwrap();
    for index in 0..8 {
        send_message(
            &retained,
            &scope,
            Uuid::new_v4(),
            session_uuid,
            1,
            &format!("synthetic racing instruction {index}"),
        )
        .unwrap();
    }
    let session_id = session.session_id.clone();
    std::thread::scope(|threads| {
        let dispatch = threads.spawn(|| composer.dispatch(&session_id));
        std::thread::sleep(Duration::from_millis(15));
        let closed = composer.stop(Uuid::new_v4(), &session_id).unwrap();
        assert_eq!(closed.status, SessionConversationStatusV1::Closed);
        let outcome = dispatch.join().unwrap();
        // Either the drain finished before the close or the effect point
        // refused it; both are truthful, neither emits after the close.
        if let Err(error) = &outcome {
            assert_eq!(
                message_failure(error),
                Some(SessionMessageFailureV1::SessionClosed)
            );
        }
    });
    let db = retained.store.db.lock().unwrap();
    let close_seq: i64 = db
        .query_row(
            "SELECT seq FROM events WHERE kind='control.session_message.conversation_closed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let late_emissions: i64 = db
        .query_row(
            "SELECT count(*) FROM events WHERE kind='control.session_message.emission_issued' AND seq>?1",
            [close_seq],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        late_emissions, 0,
        "no emission may follow the durable close"
    );
}

/// An unavailable audit blocks both the emission and the close: no effect,
/// the message stays queued and the channel stays open until STOP succeeds.
#[test]
fn composition_audit_failure_blocks_emission_and_close() {
    let harness = Harness::new();
    harness.install_markers();
    let retained = harness.retained("composition-audit-failure.json");
    let (principal, membership) = provision_membership(&retained).unwrap();
    let scope = personal_scope(&retained, &principal, &membership);
    let session = launched_session(&retained, &scope, &principal);
    let session_uuid = Uuid::parse_str(&session.session_id).unwrap();
    let composer = SessionConversationComposerV1::new(
        &retained.store,
        &retained.owner,
        &retained.source,
        Duration::from_secs(10),
    )
    .unwrap();
    composer
        .admit_channel(
            Uuid::new_v4(),
            &session.session_id,
            0,
            channel_facts("attempt-1"),
            spawn_fixture("normal"),
        )
        .unwrap();
    let operation = Uuid::new_v4();
    send_message(
        &retained,
        &scope,
        operation,
        session_uuid,
        1,
        "synthetic instruction",
    )
    .unwrap();
    // Hold the store exactly as a concurrent unavailable audit would.
    let held = retained.store.db.lock().unwrap();
    assert!(composer.dispatch(&session.session_id).is_err());
    assert!(composer.stop(Uuid::new_v4(), &session.session_id).is_err());
    drop(held);
    assert_eq!(
        kind_count(&retained.store, "control.session_message.emission_issued"),
        0
    );
    assert_eq!(
        kind_count(
            &retained.store,
            "control.session_message.conversation_closed"
        ),
        0
    );
    assert_eq!(message_status(&retained.store, operation), "queued");
    // Once the audit is available again, the same composition delivers and STOP
    // closes durably.
    let delivered = composer.dispatch(&session.session_id).unwrap();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].status, SessionMessageStatusV1::Acknowledged);
    let closed = composer.stop(Uuid::new_v4(), &session.session_id).unwrap();
    assert_eq!(closed.status, SessionConversationStatusV1::Closed);
}

/// An explicit restart binds a new epoch and channel; a message pinned to the
/// prior binding is refused at the effect point, never silently retargeted.
#[test]
fn composition_rebind_never_retargets_a_pinned_message() {
    let harness = Harness::new();
    harness.install_markers();
    let retained = harness.retained("composition-rebind.json");
    let (principal, membership) = provision_membership(&retained).unwrap();
    let scope = personal_scope(&retained, &principal, &membership);
    let session = launched_session(&retained, &scope, &principal);
    let session_uuid = Uuid::parse_str(&session.session_id).unwrap();
    let composer = SessionConversationComposerV1::new(
        &retained.store,
        &retained.owner,
        &retained.source,
        Duration::from_secs(10),
    )
    .unwrap();
    composer
        .admit_channel(
            Uuid::new_v4(),
            &session.session_id,
            0,
            channel_facts("attempt-1"),
            spawn_fixture("normal"),
        )
        .unwrap();
    let operation = Uuid::new_v4();
    send_message(
        &retained,
        &scope,
        operation,
        session_uuid,
        1,
        "synthetic instruction",
    )
    .unwrap();
    // Explicit restart: a new attempt binds durably at the retained epoch.
    let rebound = composer
        .admit_channel(
            Uuid::new_v4(),
            &session.session_id,
            1,
            channel_facts("attempt-2"),
            spawn_fixture("normal"),
        )
        .unwrap();
    assert_eq!(rebound.input_epoch, 2);
    assert_eq!(rebound.attempt_id.as_deref(), Some("attempt-2"));
    // The pinned message is refused, not retargeted to the new channel.
    let refusal = composer.dispatch(&session.session_id).unwrap_err();
    assert_eq!(
        message_failure(&refusal),
        Some(SessionMessageFailureV1::BindingChanged)
    );
    assert_eq!(message_status(&retained.store, operation), "queued");
    assert_eq!(
        kind_count(&retained.store, "control.session_message.emission_issued"),
        0
    );
}

/// A failed bind leaves no channel effect and no admitted conversation.
#[test]
fn composition_bind_failure_leaves_no_channel() {
    let harness = Harness::new();
    harness.install_markers();
    let retained = harness.retained("composition-bind-failure.json");
    let (principal, membership) = provision_membership(&retained).unwrap();
    let scope = personal_scope(&retained, &principal, &membership);
    let session = launched_session(&retained, &scope, &principal);
    let composer = SessionConversationComposerV1::new(
        &retained.store,
        &retained.owner,
        &retained.source,
        Duration::from_secs(10),
    )
    .unwrap();
    let refusal = composer
        .admit_channel(
            Uuid::new_v4(),
            &session.session_id,
            99,
            channel_facts("attempt-1"),
            spawn_fixture("normal"),
        )
        .unwrap_err();
    assert_eq!(
        message_failure(&refusal),
        Some(SessionMessageFailureV1::BindingChanged)
    );
    assert_eq!(
        kind_count(
            &retained.store,
            "control.session_message.conversation_bound"
        ),
        0
    );
    let refusal = composer.dispatch(&session.session_id).unwrap_err();
    assert_eq!(
        message_failure(&refusal),
        Some(SessionMessageFailureV1::ChannelNotAdmitted)
    );
}
