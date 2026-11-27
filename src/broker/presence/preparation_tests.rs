//! Real filesystem preparation regressions. No network, worker or provider.
//!
//! This test-only source is prepared before the implementation gate. The
//! presence module will attach it under cfg(test) once that gate is opened.

use super::{PreparedPresence, PresenceError, PresenceScope};
use crate::broker_registry::BrokerRuntimeKindV1;
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub(super) const OWN_TOKEN: &str = "presence-fixture-own-bearer-0123456789abcdef";
const OTHER_TOKEN: &str = "presence-fixture-other-bearer-9876543210abcdef";

pub(super) struct Fixture {
    _temporary: tempfile::TempDir,
    pub(super) root: PathBuf,
    pub(super) state: PathBuf,
    pub(super) config: PathBuf,
    pub(super) value: Value,
}

fn private_file(path: &Path, bytes: &[u8]) {
    use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

impl Fixture {
    pub(super) fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let state = root.join("presence-state");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        let token = root.join("broker-bearer");
        private_file(&token, OWN_TOKEN.as_bytes());
        let config = root.join("presence-config.json");
        let value = json!({
            "version": 1,
            "broker_id": "10000000-0000-4000-8000-000000000001",
            "credential_id": "20000000-0000-4000-8000-000000000001",
            "incarnation_id": "30000000-0000-4000-8000-000000000001",
            "authority_id": "fixture-authority",
            "broker_token_file": token,
            "state_dir": state,
        });
        Self {
            _temporary: temporary,
            root,
            state,
            config,
            value,
        }
    }

    pub(super) fn write_config(&self) {
        private_file(&self.config, &serde_json::to_vec(&self.value).unwrap());
    }

    pub(super) fn scope(&self) -> PresenceScope {
        PresenceScope {
            installation_id: Uuid::parse_str("40000000-0000-4000-8000-000000000001").unwrap(),
            profile_id: "fixture-profile".into(),
            gateway: "http://127.0.0.1:9/".into(),
            expected_tenant_id: "fixture-tenant".into(),
            control_namespace_uuid: Uuid::parse_str("50000000-0000-4000-8000-000000000001")
                .unwrap(),
            runtime_kind: BrokerRuntimeKindV1::Docker,
        }
    }

    fn empty_state(&self) -> bool {
        fs::read_dir(&self.state).unwrap().next().is_none()
    }
}

#[test]
fn load_retains_protection_without_initializing_state_or_contacting_gateway() {
    // A create-on-load regression would violate cross-profile secret screening.
    let fixture = Fixture::new();
    fixture.write_config();
    let before = fs::read(&fixture.config).unwrap();
    let prepared = PreparedPresence::load(&fixture.config, fixture.scope(), vec![]).unwrap();
    assert!(fixture.empty_state());
    assert_eq!(fs::read(&fixture.config).unwrap(), before);
    assert!(
        prepared
            .protected_files()
            .iter()
            .any(|file| file.path() == fixture.config)
    );
    assert!(
        prepared
            .protected_files()
            .iter()
            .any(|file| file.path() == fixture.root.join("broker-bearer"))
    );
    assert!(
        prepared
            .protected_roots()
            .iter()
            .any(|root| root.path() == fixture.state)
    );
    assert!(
        prepared
            .secret_literals()
            .iter()
            .any(|secret| secret == OWN_TOKEN)
    );
    prepared.revalidate().unwrap();
}

#[test]
fn malformed_closed_config_never_creates_publisher_state() {
    // Accepting an unknown field or unsafe bound would silently ignore policy.
    for (field, value) in [
        ("unknown_policy", json!(true)),
        ("version", json!(2)),
        ("broker_id", json!("10000000-0000-4000-8000-00000000000A")),
        ("interval_seconds", json!(4)),
        ("interval_seconds", json!(151)),
        ("max_operations", json!(0)),
        ("max_operations", json!(32769)),
        ("max_state_bytes", json!(0)),
        ("max_state_bytes", json!(134217729)),
        ("broker_token_file", json!("relative-bearer")),
        ("state_dir", json!("relative-state")),
    ] {
        let mut fixture = Fixture::new();
        fixture.value[field] = value;
        fixture.write_config();
        assert!(PreparedPresence::load(&fixture.config, fixture.scope(), vec![]).is_err());
        assert!(fixture.empty_state());
    }
}

#[test]
fn finalization_refuses_other_profile_secret_before_persistence() {
    // Material learned after all loads must fence admission whether the
    // literal occurs in decoded config, trusted scope, or a retained pathname.
    for surface in ["config", "scope", "path"] {
        let mut fixture = Fixture::new();
        let mut scope = fixture.scope();
        match surface {
            "config" => fixture.value["authority_id"] = json!(OTHER_TOKEN),
            "scope" => scope.profile_id = OTHER_TOKEN.into(),
            "path" => fixture.config = fixture.root.join(OTHER_TOKEN),
            _ => unreachable!(),
        }
        fixture.write_config();
        let mut prepared = PreparedPresence::load(&fixture.config, scope, vec![]).unwrap();
        let error = prepared.finalize(&[OTHER_TOKEN.to_owned()]).unwrap_err();
        assert_eq!(error, PresenceError::InvalidConfig);
        assert!(error.fences_admission());
        assert!(fixture.empty_state());
        assert!(!format!("{error:?}").contains(OTHER_TOKEN));
    }
}

#[test]
fn unknown_existing_state_is_preserved_and_cannot_be_adopted() {
    // Fresh-state initialization must never overwrite partial/operator evidence.
    let fixture = Fixture::new();
    fixture.write_config();
    let foreign = fixture.state.join("unrecognized-record.json");
    let bytes = b"retained partial evidence";
    private_file(&foreign, bytes);
    let outcome = PreparedPresence::load(&fixture.config, fixture.scope(), vec![])
        .and_then(|mut prepared| prepared.finalize(&[]));
    assert!(outcome.is_err());
    assert_eq!(fs::read(foreign).unwrap(), bytes);
}

#[test]
fn finalized_stream_holds_exclusive_lock_until_its_owner_is_dropped() {
    // A lock on a transient FD would permit two publishers to mint operations.
    let fixture = Fixture::new();
    fixture.write_config();
    let mut first = PreparedPresence::load(&fixture.config, fixture.scope(), vec![]).unwrap();
    first.finalize(&[]).unwrap();
    let mut second = PreparedPresence::load(&fixture.config, fixture.scope(), vec![]).unwrap();
    assert!(second.finalize(&[]).is_err());
    drop(second);
    drop(first);
    let mut reopened = PreparedPresence::load(&fixture.config, fixture.scope(), vec![]).unwrap();
    reopened.finalize(&[]).unwrap();
}

#[tokio::test]
async fn named_control_retains_only_the_finalized_private_identity() {
    let fixture = Fixture::new();
    fixture.write_config();
    let mut prepared = PreparedPresence::load(&fixture.config, fixture.scope(), vec![]).unwrap();
    assert!(matches!(
        prepared.named_control(),
        Err(PresenceError::UnsafeState)
    ));
    prepared.finalize(&[OTHER_TOKEN.to_owned()]).unwrap();
    let control = prepared.named_control().unwrap();
    drop(prepared);

    let authority = &control.0;
    authority.revalidate().unwrap();
    assert!(authority.screen_text(OTHER_TOKEN).is_err());
    let token = fixture.root.join("broker-bearer");
    fs::rename(&token, fixture.root.join("old-bearer")).unwrap();
    private_file(&token, OWN_TOKEN.as_bytes());
    assert!(matches!(
        control
            .read_legacy_confirmed_plan("fixture-profile", Uuid::new_v4())
            .await,
        Err(PresenceError::UnsafeFilesystem)
    ));
}
