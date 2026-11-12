//! Real private SQLite/FS fixtures; no provider or live OIDC registry.
use super::*;
use crate::human_authority::{
    HumanAuthorityConfigurationV1, HumanConfigurationFileKindV1, HumanConfigurationFileV1,
    HumanOidcConfigurationV1, HumanStorageProfileV1, SourceStorageBindingV1,
    maintenance::{
        HumanMaintenanceResolvedViewV1, HumanMaintenanceViewV1,
        HumanSourcesMaintenanceConfigurationV1, HumanSourcesMaintenanceGuard,
        HumanSourcesMaintenancePlanV1,
    },
};
use std::{ffi::OsStr, os::unix::fs::PermissionsExt};

pub(super) struct Fixture {
    pub(super) temporary: tempfile::TempDir,
    pub(super) audit: AuthenticationAudit,
    pub(super) path: std::path::PathBuf,
    pub(super) installation: Uuid,
    pub(super) policy: AuthenticationAuditPolicyV1,
    pub(super) token: HumanMaintenanceSourceToken,
    pub(super) configuration: HumanAuthorityConfigurationV1,
    pub(super) files: Vec<(HumanConfigurationFileKindV1, crate::control_fs::PrivateFile)>,
}
fn storage_binding(parent: &Directory, name: &str) -> SourceStorageBindingV1 {
    let (parent_device, parent_inode) = parent.physical_identity().unwrap();
    let (source_device, source_inode) = parent
        .private_entry(OsStr::new(name))
        .unwrap()
        .unwrap()
        .physical_identity();
    SourceStorageBindingV1 {
        parent_path: parent.path().to_owned(),
        file_name: name.into(),
        parent_device,
        parent_inode,
        source_device,
        source_inode,
    }
}
impl Fixture {
    pub(super) fn new(unscoped_records: u64) -> Self {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let root = Directory::open_existing(temporary.path()).unwrap();
        let source = root.persistent_child("source", true).unwrap().unwrap();
        let configuration = root
            .persistent_child("configuration", true)
            .unwrap()
            .unwrap();
        let installation = Uuid::new_v4();
        let policy = AuthenticationAuditPolicyV1 {
            max_records: 128,
            max_serialized_bytes: 2_000_000,
            approved_clients: vec![ApprovedAuthenticationClientV1 {
                issuer: "https://identity.example.test/realm".into(),
                client_id: "public-client".into(),
            }],
        };
        let plan = AuthenticationCapacityPlanV1 {
            version: 1,
            unscoped: AuthenticationCapacityV1 {
                max_records: unscoped_records,
                max_serialized_bytes: 200_000,
            },
            tenants: vec![AuthenticationTenantCapacityV1 {
                tenant_id: "A".into(),
                capacity: AuthenticationCapacityV1 {
                    max_records: 32,
                    max_serialized_bytes: 1_000_000,
                },
            }],
        };
        let path = source.path().join("authentication.sqlite");
        let audit = AuthenticationAudit::init_new_v4(
            &path,
            installation,
            policy.clone(),
            plan,
            Redactor::new(vec![]),
        )
        .unwrap();
        let namespace = audit.verify().unwrap().namespace_id;
        let kinds = [
            HumanConfigurationFileKindV1::ControlConfiguration,
            HumanConfigurationFileKindV1::TechnicalCredentials,
            HumanConfigurationFileKindV1::OidcConfiguration,
            HumanConfigurationFileKindV1::OidcTrustBundle,
        ];
        let mut files = Vec::new();
        let mut descriptors = Vec::new();
        for (index, kind) in kinds.into_iter().enumerate() {
            let name = format!("config-{index}.json");
            files.push((
                kind,
                configuration
                    .persistent_create(&name, b"private synthetic configuration")
                    .unwrap(),
            ));
            descriptors.push(HumanConfigurationFileV1 {
                kind,
                storage_binding: storage_binding(&configuration, &name),
            });
        }
        let configuration = HumanAuthorityConfigurationV1 {
            control_config_revision: 1,
            public_config_digest: "ab".repeat(32),
            oidc: Some(HumanOidcConfigurationV1 {
                public_config_digest: "cd".repeat(32),
                mapping_revision: 1,
            }),
            files: descriptors,
        };
        let source_id = Uuid::new_v4();
        let view = HumanMaintenanceViewV1 {
            sources: vec![
                crate::human_authority::maintenance::HumanMaintenanceSourceV1 {
                    source_id,
                    kind: HumanSourceKindV1::AuthenticationLedger,
                    installation_id: installation,
                    durable_namespace_id: namespace,
                    tenant_id: None,
                    storage_binding: storage_binding(&source, "authentication.sqlite"),
                    required_format: 5,
                },
            ],
            configuration,
        };
        let plan = HumanSourcesMaintenancePlanV1 {
            version: 1,
            migration_id: Uuid::new_v4(),
            installation_id: installation,
            prior: None,
            target: view.clone(),
            prior_active_manifest: None,
            supported_storage_profile: HumanStorageProfileV1::LocalPosixFlockV1,
        };
        let file = root
            .persistent_create("maintenance.json", &serde_json::to_vec(&plan).unwrap())
            .unwrap();
        let resolved = HumanMaintenanceResolvedViewV1::new(
            view.sources,
            view.configuration.clone(),
            files.clone(),
        )
        .unwrap();
        let guard = HumanSourcesMaintenanceGuard::acquire(
            file,
            HumanSourcesMaintenanceConfigurationV1::new(None, resolved, None),
        )
        .unwrap();
        let token = guard.target_source(source_id).unwrap();
        Self {
            temporary,
            audit,
            path,
            installation,
            policy,
            token,
            configuration: view.configuration,
            files,
        }
    }
    pub(super) fn maintenance(&self) -> AuthenticationReviewMaintenance {
        AuthenticationAudit::open_review_maintenance(
            &self.path,
            self.installation,
            self.policy.clone(),
            Redactor::new(vec![]),
            &self.token,
        )
        .unwrap()
    }
    pub(super) fn backup(&self) -> std::path::PathBuf {
        let root = Directory::open_existing(self.temporary.path()).unwrap();
        root.persistent_child("backup", true)
            .unwrap()
            .unwrap()
            .path()
            .join("authentication.sqlite")
    }
    pub(super) fn migrate(&self) -> AuthenticationReceiptV1 {
        self.maintenance()
            .with_review_maintenance_writer(&self.token, |writer| {
                let backup = if writer.inspect()?.anchor().is_some() {
                    writer.resume_backup(&self.token, &self.backup())?
                } else {
                    writer.prepare_backup(&self.token, &self.backup())?
                };
                let receipt = writer.install_format5(&backup)?;
                writer.commit()?;
                Ok(receipt)
            })
            .unwrap()
    }
}
// Break: conversion is only PRAGMA5, changes historical bytes, or replaces the V4 plan.
#[test]
fn real_v4_to_v5_migration_preserves_plan_debts_and_receipts() {
    let fixture = Fixture::new(8);
    let b = binding();
    let initial = AuthenticationRecordV1 {
        principal: None,
        ..intention(&b, AuthenticationActionV1::SessionAdmission)
    };
    let candidates = AuthenticationTenantCandidatesV1::from_verified_memberships(
        b.principal.clone(),
        vec!["A".into()],
    )
    .unwrap();
    let initial_receipt = fixture
        .audit
        .begin_verified_initial_scoped(
            initial.clone(),
            b.principal.clone(),
            b.access_fingerprint,
            candidates,
        )
        .unwrap();
    fixture
        .audit
        .admit_session(
            result(
                &initial,
                initial_receipt.receipt,
                b.principal.clone(),
                AuthenticationOutcomeV1::Succeeded,
                AuthenticationReasonV1::Verified,
            ),
            b.clone(),
        )
        .unwrap();
    let refresh = intention(&b, AuthenticationActionV1::Refresh);
    let pending = fixture
        .audit
        .refresh_pending(refresh.clone(), b.generation, b.access_fingerprint)
        .unwrap();
    let logout = intention(&b, AuthenticationActionV1::Logout);
    let logout_receipt = fixture
        .audit
        .begin_native_logout_during_refresh(
            logout.clone(),
            b.clone(),
            refresh.operation_id,
            pending.receipt.clone(),
        )
        .unwrap();
    let before = fixture.audit.verify().unwrap();
    let plan = fixture.audit.capacity_plan().unwrap();
    let anchor = fixture.migrate();
    let maintenance = fixture.maintenance();
    maintenance
        .with_review_maintenance_writer(&fixture.token, |writer| {
            assert_eq!(writer.inspect()?.anchor(), Some(anchor.clone()));
            let backup = writer.resume_backup(&fixture.token, &fixture.backup())?;
            assert_eq!(writer.install_format5(&backup)?, anchor);
            assert_eq!(writer.verification()?.records, before.records + 1);
            assert_eq!(writer.capacity_plan()?, plan);
            writer.commit()
        })
        .unwrap();
    let restored = AuthenticationAudit::open_existing(
        &fixture.backup(),
        fixture.installation,
        fixture.policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap();
    assert_eq!(restored.verify().unwrap(), before);
    assert_eq!(restored.capacity_plan().unwrap(), plan);
    assert!(matches!(
        restored
            .session_current(b.session_id)
            .unwrap()
            .unwrap()
            .state,
        AuthenticationSessionStateV1::RefreshPending { .. }
    ));
    drop(maintenance);
    let owned = fixture.activate();
    let epoch = owned.owner.try_write_epoch().unwrap();
    let closed = owned
        .audit
        .close_session_owned(
            &epoch,
            &owned.source,
            result(
                &refresh,
                pending.receipt.clone(),
                b.principal.clone(),
                AuthenticationOutcomeV1::Failed,
                AuthenticationReasonV1::ProviderUnavailable,
            ),
            b.generation,
            b.access_fingerprint,
        )
        .unwrap();
    owned
        .audit
        .record_native_logout_after_refresh_closed_owned(
            &epoch,
            &owned.source,
            result(
                &logout,
                logout_receipt,
                b.principal.clone(),
                AuthenticationOutcomeV1::Succeeded,
                AuthenticationReasonV1::UserRequested,
            ),
            b,
            refresh.operation_id,
            pending.receipt,
            closed,
        )
        .unwrap();
}
// Break: a previously opened legacy handle reaches replay/projection after migration.
#[test]
fn preopened_v4_handle_refuses_append_retry_and_projection_after_v5() {
    let fixture = Fixture::new(8);
    let b = binding();
    let record = AuthenticationRecordV1 {
        principal: None,
        ..intention(&b, AuthenticationActionV1::SessionAdmission)
    };
    let candidates = AuthenticationTenantCandidatesV1::from_verified_memberships(
        b.principal.clone(),
        vec!["A".into()],
    )
    .unwrap();
    fixture
        .audit
        .begin_verified_initial_scoped(
            record.clone(),
            b.principal.clone(),
            b.access_fingerprint,
            candidates,
        )
        .unwrap();
    fixture.migrate();
    let candidates = AuthenticationTenantCandidatesV1::from_verified_memberships(
        b.principal.clone(),
        vec!["A".into()],
    )
    .unwrap();
    let retry = fixture
        .audit
        .begin_verified_initial_scoped(
            record.clone(),
            b.principal,
            b.access_fingerprint,
            candidates,
        )
        .unwrap_err();
    assert_eq!(
        retry.downcast_ref::<AuthenticationSourceFailure>(),
        Some(&AuthenticationSourceFailure::Required)
    );
    assert!(fixture.audit.append_typed(record).is_err());
    assert!(fixture.audit.verify().is_err());
    assert!(fixture.audit.format_version().is_err());
    assert!(fixture.audit.capacity_plan().is_err());
    assert!(fixture.audit.session_current(Uuid::new_v4()).is_err());
    fixture
        .maintenance()
        .with_review_maintenance_writer(&fixture.token, |writer| {
            writer.verification()?;
            writer.commit()
        })
        .unwrap();
}
// Break: authority borrows a tenant's unused partition or an outstanding terminal ticket.
#[test]
fn v5_marker_charge_never_spends_tenant_or_terminal_reserve() {
    let fixture = Fixture::new(0);
    let before = fixture.audit.verify().unwrap();
    fixture
        .maintenance()
        .with_review_maintenance_writer(&fixture.token, |writer| {
            let backup = writer.prepare_backup(&fixture.token, &fixture.backup())?;
            assert!(writer.install_format5(&backup).is_err());
            Ok(())
        })
        .unwrap();
    assert_eq!(fixture.audit.verify().unwrap(), before);
    assert_eq!(fixture.audit.format_version().unwrap(), 4);
    assert!(fixture.backup().exists());
}
// Break: a trusted marker hash hides a divergent materialized authority/floor.
#[test]
fn v5_replay_rejects_marker_projection_and_floor_divergence() {
    let fixture = Fixture::new(8);
    fixture.migrate();
    let db = Connection::open(&fixture.path).unwrap();
    db.execute(
        "UPDATE authentication_source_authority SET wall_floor='1970-01-01T00:00:00.000000000Z'",
        [],
    )
    .unwrap();
    db.close().unwrap();
    assert!(
        AuthenticationAudit::open_review_maintenance(
            &fixture.path,
            fixture.installation,
            fixture.policy.clone(),
            Redactor::new(vec![]),
            &fixture.token
        )
        .is_err()
    );
}

struct OwnedFixture {
    _temporary: tempfile::TempDir,
    owner: crate::human_authority::OwnedHumanSources,
    source: OwnedSourceToken,
    audit: AuthenticationAudit,
}
impl Fixture {
    fn activate(self) -> OwnedFixture {
        use crate::human_authority::{
            HumanAuthorityClosureV1, HumanAuthorityMarkerReceiptV1, HumanSourceDescriptorV1,
            HumanSourcesConfigurationV1, OwnedHumanSources,
        };
        let anchor = self.migrate();
        let expected = self.token.descriptor().clone();
        let source = HumanSourceDescriptorV1 {
            source_id: expected.source_id,
            kind: expected.kind,
            installation_id: expected.installation_id,
            durable_namespace_id: expected.durable_namespace_id,
            tenant_id: expected.tenant_id,
            storage_binding: expected.storage_binding,
            required_format: expected.required_format,
            authority_marker_receipt: HumanAuthorityMarkerReceiptV1 {
                seq: anchor.sequence as i64,
                hash: anchor.hash,
            },
        };
        let mut closure = HumanAuthorityClosureV1 {
            version: 1,
            installation_id: self.installation,
            closure_id: Uuid::new_v4(),
            closure_revision: 1,
            closure_digest: String::new(),
            sources: vec![source.clone()],
            configuration: self.configuration.clone(),
            supported_storage_profile: HumanStorageProfileV1::LocalPosixFlockV1,
        };
        closure.closure_digest = closure.compute_digest().unwrap();
        let root = Directory::open_existing(self.temporary.path()).unwrap();
        let manifest = root
            .persistent_create("authority.json", &serde_json::to_vec(&closure).unwrap())
            .unwrap();
        drop(self.token);
        let owner = OwnedHumanSources::acquire(
            manifest,
            HumanSourcesConfigurationV1::new(vec![source], self.configuration, self.files).unwrap(),
        )
        .unwrap();
        let source = owner.source_token(expected.source_id).unwrap();
        let epoch = owner.try_write_epoch().unwrap();
        let startup = AuthenticationAudit::open_review_startup(
            &self.path,
            self.installation,
            self.policy,
            Redactor::new(vec![]),
            &source,
        )
        .unwrap();
        startup
            .append_incarnation(&epoch, &source, Uuid::new_v4(), 0)
            .unwrap();
        let audit = startup.retain(&epoch, &source).unwrap();
        drop(epoch);
        OwnedFixture {
            _temporary: self.temporary,
            owner,
            source,
            audit,
        }
    }
}
fn binding() -> AuthenticationSessionBindingV1 {
    AuthenticationSessionBindingV1 {
        session_id: Uuid::new_v4(),
        generation: 1,
        principal: PrincipalRefV1::HumanOidc {
            issuer: "https://identity.example.test/realm".into(),
            subject: "private-fixture".into(),
        },
        issuer: "https://identity.example.test/realm".into(),
        client_id: "public-client".into(),
        kind: OidcSessionKindV1::NativeCli,
        access_fingerprint: AuthenticationTokenFingerprintV1::access_token(
            "synthetic-private-access",
        ),
        refresh_fingerprint: Some(AuthenticationTokenFingerprintV1::refresh_token(
            "synthetic-private-refresh",
        )),
        expires_at_unix_seconds: 4_000_000_000,
    }
}
fn intention(
    b: &AuthenticationSessionBindingV1,
    action: AuthenticationActionV1,
) -> AuthenticationRecordV1 {
    AuthenticationRecordV1 {
        operation_id: Uuid::new_v4(),
        action,
        phase: AuthenticationPhaseV1::Intention,
        issuer: b.issuer.clone(),
        client_id: b.client_id.clone(),
        principal: Some(b.principal.clone()),
        transaction_id: None,
        session_id: Some(b.session_id),
        causal: None,
    }
}
fn result(
    record: &AuthenticationRecordV1,
    receipt: AuthenticationReceiptV1,
    principal: PrincipalRefV1,
    outcome: AuthenticationOutcomeV1,
    reason: AuthenticationReasonV1,
) -> AuthenticationRecordV1 {
    AuthenticationRecordV1 {
        phase: AuthenticationPhaseV1::Result { outcome, reason },
        causal: Some(receipt),
        principal: Some(principal),
        ..record.clone()
    }
}
fn admit_owned(
    f: &OwnedFixture,
    epoch: &HumanSourcesWriteEpoch,
    b: &AuthenticationSessionBindingV1,
) {
    let record = AuthenticationRecordV1 {
        principal: None,
        ..intention(b, AuthenticationActionV1::SessionAdmission)
    };
    let candidates = AuthenticationTenantCandidatesV1::from_verified_memberships(
        b.principal.clone(),
        vec!["A".into()],
    )
    .unwrap();
    let pending = f
        .audit
        .begin_verified_initial_with_candidates_owned(
            epoch,
            &f.source,
            record.clone(),
            b.principal.clone(),
            b.access_fingerprint,
            candidates,
        )
        .unwrap();
    f.audit
        .admit_session_owned(
            epoch,
            &f.source,
            result(
                &record,
                pending.receipt,
                b.principal.clone(),
                AuthenticationOutcomeV1::Succeeded,
                AuthenticationReasonV1::Verified,
            ),
            b.clone(),
        )
        .unwrap();
}
// Break: accepting a foreign epoch/source or permitting a reader to mutate.
#[test]
fn v5_requires_exact_source_epoch_and_current_incarnation() {
    let f = Fixture::new(8).activate();
    let foreign = Fixture::new(8).activate();
    let reader = f.owner.try_read_epoch().unwrap();
    let access = AuthenticationSourceAccess::read(&reader, &f.source).unwrap();
    assert_eq!(f.audit.format_version_owned(&access).unwrap(), 5);
    let wrong = foreign.owner.try_read_epoch().unwrap();
    assert!(AuthenticationSourceAccess::read(&wrong, &f.source).is_err());
    let foreign_access = AuthenticationSourceAccess::read(&wrong, &foreign.source).unwrap();
    assert!(f.audit.verify_owned(&foreign_access).is_err());
    assert!(access.check(true).is_err());
    assert!(
        f.audit.verify_owned(&access).is_ok(),
        "foreign access must not latch the healthy source"
    );
    drop(access);
    drop(reader);
    let epoch = f.owner.try_write_epoch().unwrap();
    let b = binding();
    admit_owned(&f, &epoch, &b);
    let access = AuthenticationSourceAccess::write(&epoch, &f.source).unwrap();
    assert_eq!(
        f.audit
            .session_current_owned(&access, b.session_id)
            .unwrap()
            .unwrap()
            .binding,
        b
    );
    assert!(
        f.audit.verify().is_err(),
        "active modern handles still reject the legacy API"
    );
    assert!(f.audit.verify_owned(&access).is_ok());
}
// Break: spending/dropping the independent native logout or refresh terminal promise.
#[test]
fn v5_native_logout_and_refresh_keep_existing_capacity_contract() {
    let f = Fixture::new(8).activate();
    let epoch = f.owner.try_write_epoch().unwrap();
    let b = binding();
    admit_owned(&f, &epoch, &b);
    let refresh = intention(&b, AuthenticationActionV1::Refresh);
    let pending = f
        .audit
        .refresh_pending_owned(
            &epoch,
            &f.source,
            refresh.clone(),
            b.generation,
            b.access_fingerprint,
        )
        .unwrap();
    let logout = intention(&b, AuthenticationActionV1::Logout);
    let logout_receipt = f
        .audit
        .begin_native_logout_during_refresh_owned(
            &epoch,
            &f.source,
            logout.clone(),
            b.clone(),
            refresh.operation_id,
            pending.receipt.clone(),
        )
        .unwrap();
    let closed = f
        .audit
        .close_session_owned(
            &epoch,
            &f.source,
            result(
                &refresh,
                pending.receipt.clone(),
                b.principal.clone(),
                AuthenticationOutcomeV1::Failed,
                AuthenticationReasonV1::ProviderUnavailable,
            ),
            b.generation,
            b.access_fingerprint,
        )
        .unwrap();
    let end = f
        .audit
        .record_native_logout_after_refresh_closed_owned(
            &epoch,
            &f.source,
            result(
                &logout,
                logout_receipt.clone(),
                b.principal.clone(),
                AuthenticationOutcomeV1::Succeeded,
                AuthenticationReasonV1::UserRequested,
            ),
            b.clone(),
            refresh.operation_id,
            pending.receipt.clone(),
            closed.clone(),
        )
        .unwrap();
    assert_eq!(
        f.audit
            .record_native_logout_after_refresh_closed_owned(
                &epoch,
                &f.source,
                result(
                    &logout,
                    logout_receipt,
                    b.principal.clone(),
                    AuthenticationOutcomeV1::Succeeded,
                    AuthenticationReasonV1::UserRequested
                ),
                b.clone(),
                refresh.operation_id,
                pending.receipt,
                closed
            )
            .unwrap(),
        end
    );
    let access = AuthenticationSourceAccess::write(&epoch, &f.source).unwrap();
    assert!(matches!(
        f.audit
            .session_current_owned(&access, b.session_id)
            .unwrap()
            .unwrap()
            .state,
        AuthenticationSessionStateV1::Closed { .. }
    ));
    assert_eq!(
        f.audit
            .capacity_plan_owned(&access)
            .unwrap()
            .unwrap()
            .unscoped
            .max_records,
        8
    );
}
// Break: returning an ACK after the epoch expires, or losing a known committed floor.
#[test]
fn v5_commit_floor_survives_late_ack_refusal() {
    let f = Fixture::new(8).activate();
    let epoch = f.owner.try_write_epoch().unwrap();
    let b = binding();
    let record = AuthenticationRecordV1 {
        principal: None,
        ..intention(&b, AuthenticationActionV1::SessionAdmission)
    };
    let before = f.audit.inner.state.lock().unwrap().floor.clone();
    *f.audit.inner.postcommit_hook.lock().unwrap() =
        Some(Box::new(|| std::thread::sleep(Duration::from_millis(2100))));
    let candidates = AuthenticationTenantCandidatesV1::from_verified_memberships(
        b.principal.clone(),
        vec!["A".into()],
    )
    .unwrap();
    let error = f
        .audit
        .begin_verified_initial_with_candidates_owned(
            &epoch,
            &f.source,
            record.clone(),
            b.principal.clone(),
            b.access_fingerprint,
            candidates,
        )
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<crate::human_authority::HumanAuthorityFailure>(),
        Some(&crate::human_authority::HumanAuthorityFailure::EpochExpired)
    );
    let committed = f.audit.inner.state.lock().unwrap().floor.clone();
    assert_eq!(committed.sequence, before.sequence + 1);
    drop(epoch);
    let epoch = f.owner.try_write_epoch().unwrap();
    let candidates = AuthenticationTenantCandidatesV1::from_verified_memberships(
        b.principal.clone(),
        vec!["A".into()],
    )
    .unwrap();
    let retry = f
        .audit
        .begin_verified_initial_with_candidates_owned(
            &epoch,
            &f.source,
            record,
            b.principal,
            b.access_fingerprint,
            candidates,
        )
        .unwrap();
    assert!(!retry.inserted);
    assert_eq!(retry.receipt, committed);
}

// Break: an owner restart treats the previous process's incarnation as current.
#[test]
fn new_owner_requires_real_new_incarnation_before_retention() {
    use crate::human_authority::{HumanSourcesConfigurationV1, OwnedHumanSources};
    let fixture = Fixture::new(8).activate();
    let closure = fixture.source.closure().clone();
    let path = closure.sources[0].storage_binding.path();
    let installation = closure.installation_id;
    let policy = fixture.audit.inner.policy.clone();
    let root = Directory::open_existing(fixture._temporary.path()).unwrap();
    let manifest = root
        .persistent_read("authority.json", 512 * 1024, &[])
        .unwrap()
        .unwrap();
    let files = closure
        .configuration
        .files
        .iter()
        .map(|descriptor| {
            let parent = Directory::open_existing(&descriptor.storage_binding.parent_path).unwrap();
            (
                descriptor.kind,
                parent
                    .persistent_read(&descriptor.storage_binding.file_name, 1_048_576, &[])
                    .unwrap()
                    .unwrap(),
            )
        })
        .collect();
    let actual = HumanSourcesConfigurationV1::new(
        closure.sources.clone(),
        closure.configuration.clone(),
        files,
    )
    .unwrap();
    drop(fixture.audit);
    drop(fixture.source);
    drop(fixture.owner);
    let owner = OwnedHumanSources::acquire(manifest, actual).unwrap();
    let source = owner.source_token(closure.sources[0].source_id).unwrap();
    let epoch = owner.try_write_epoch().unwrap();
    let startup = AuthenticationAudit::open_review_startup(
        &path,
        installation,
        policy.clone(),
        Redactor::new(vec![]),
        &source,
    )
    .unwrap();
    assert!(startup.retain(&epoch, &source).is_err());
    // A mere non-current incarnation is a refused access, not a source latch.
    let startup = AuthenticationAudit::open_review_startup(
        &path,
        installation,
        policy,
        Redactor::new(vec![]),
        &source,
    )
    .unwrap();
    let operation = Uuid::new_v4();
    let receipt = startup
        .append_incarnation(&epoch, &source, operation, 1)
        .unwrap();
    assert_eq!(
        startup
            .append_incarnation(&epoch, &source, operation, 1)
            .unwrap(),
        receipt
    );
    let audit = startup.retain(&epoch, &source).unwrap();
    let access = AuthenticationSourceAccess::write(&epoch, &source).unwrap();
    assert_eq!(audit.verify_owned(&access).unwrap().head, receipt);
    assert!(
        audit
            .session_current_owned(&access, Uuid::new_v4())
            .unwrap()
            .is_none()
    );
}

// Break: a resumed conversion trusts the existence of the old backup instead of its bytes.
#[test]
fn v5_resume_refuses_a_changed_backup_and_keeps_the_anchor() {
    let fixture = Fixture::new(8);
    let receipt = fixture.migrate();
    let db = Connection::open(fixture.backup()).unwrap();
    db.execute(
        "UPDATE authentication_entries SET hash=?1 WHERE seq=2",
        ["00".repeat(32)],
    )
    .unwrap();
    db.close().unwrap();
    fixture
        .maintenance()
        .with_review_maintenance_writer(&fixture.token, |writer| {
            assert!(
                writer
                    .resume_backup(&fixture.token, &fixture.backup())
                    .is_err()
            );
            assert_eq!(writer.inspect()?.anchor(), Some(receipt));
            writer.commit()
        })
        .unwrap();
}

// Break: using raw SQLite version5 as though it were the logical generation5.
#[test]
fn v5_rejects_raw_generation_as_sqlite_user_version() {
    let fixture = Fixture::new(8);
    fixture.migrate();
    let db = Connection::open(&fixture.path).unwrap();
    db.pragma_update(None, "user_version", 5).unwrap();
    db.close().unwrap();
    assert!(
        AuthenticationAudit::open_review_maintenance(
            &fixture.path,
            fixture.installation,
            fixture.policy.clone(),
            Redactor::new(vec![]),
            &fixture.token
        )
        .is_err()
    );
}

// Break: charging an anchor consumes the already-promised unscoped terminal slot.
#[test]
fn authority_charge_preserves_existing_unscoped_terminal_promise() {
    let fixture = Fixture::new(2);
    let b = binding();
    let initial = AuthenticationRecordV1 {
        principal: None,
        ..intention(&b, AuthenticationActionV1::SessionAdmission)
    };
    let candidates =
        AuthenticationTenantCandidatesV1::from_verified_memberships(b.principal.clone(), vec![])
            .unwrap();
    let pending = fixture
        .audit
        .begin_verified_initial_scoped(
            initial.clone(),
            b.principal.clone(),
            b.access_fingerprint,
            candidates,
        )
        .unwrap();
    let before = fixture.audit.verify().unwrap();
    fixture
        .maintenance()
        .with_review_maintenance_writer(&fixture.token, |writer| {
            let backup = writer.prepare_backup(&fixture.token, &fixture.backup())?;
            let error = writer.install_format5(&backup).unwrap_err();
            assert_eq!(
                error.downcast_ref::<AuthenticationAuditFailureV1>(),
                Some(&AuthenticationAuditFailureV1::Capacity)
            );
            assert!(
                writer.commit().is_err(),
                "a failed install cannot commit a partial conversion"
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(fixture.audit.verify().unwrap(), before);
    fixture
        .audit
        .append_typed(result(
            &initial,
            pending.receipt,
            b.principal,
            AuthenticationOutcomeV1::Rejected,
            AuthenticationReasonV1::NoMembership,
        ))
        .unwrap();
    assert_eq!(fixture.audit.verify().unwrap().records, before.records + 1);
    assert_eq!(fixture.audit.format_version().unwrap(), 4);
}

#[path = "tests/review_regressions.rs"]
mod review_regressions;
