//! Native logout promise regressions with real private SQLite fixtures.
//! Contract: PROPOSAL-R2.md, source d5c46b23fc2326d730d21a6f3d8285ec326646aa.
//! Real SQLite and approved BEGIN/END APIs; no provider, network or owner proof.

use super::*;
use rusqlite::{Connection, OpenFlags, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{Arc, Barrier},
    thread,
};

const INSTALLATION: Uuid = Uuid::from_u128(0x11111111111141118111111111111111);
const ISSUER: &str = "https://identity.example.test/realm";
const CLIENT: &str = "public-native";
const TERMINAL_BYTES: u64 = 32_768;

fn id(value: u128) -> Uuid {
    Uuid::from_u128(0x10000000000040008000000000000000 + value)
}

fn policy(records: u64, bytes: u64) -> AuthenticationAuditPolicyV1 {
    AuthenticationAuditPolicyV1 {
        max_records: records,
        max_serialized_bytes: bytes,
        approved_clients: vec![ApprovedAuthenticationClientV1 {
            issuer: ISSUER.into(),
            client_id: CLIENT.into(),
        }],
    }
}

fn open(path: &Path, policy: &AuthenticationAuditPolicyV1) -> AuthenticationAudit {
    AuthenticationAudit::open_existing(path, INSTALLATION, policy.clone(), Redactor::new(vec![]))
        .unwrap()
}

fn initial(operation: Uuid) -> AuthenticationRecordV1 {
    AuthenticationRecordV1 {
        operation_id: operation,
        action: AuthenticationActionV1::SessionAdmission,
        phase: AuthenticationPhaseV1::Intention,
        issuer: ISSUER.into(),
        client_id: CLIENT.into(),
        principal: None,
        transaction_id: Some(operation),
        session_id: None,
        causal: None,
    }
}

fn admit(audit: &AuthenticationAudit, slot: u128) -> AuthenticationSessionBindingV1 {
    let binding = AuthenticationSessionBindingV1 {
        session_id: id(100 + slot),
        generation: 1,
        principal: PrincipalRefV1::HumanOidc {
            issuer: ISSUER.into(),
            subject: format!("native-fixture-{slot}"),
        },
        issuer: ISSUER.into(),
        client_id: CLIENT.into(),
        kind: OidcSessionKindV1::NativeCli,
        access_fingerprint: AuthenticationTokenFingerprintV1::access_token(&format!(
            "synthetic-access-{slot}"
        )),
        refresh_fingerprint: Some(AuthenticationTokenFingerprintV1::refresh_token(&format!(
            "synthetic-refresh-{slot}"
        ))),
        expires_at_unix_seconds: 4_000_000_000,
    };
    let intention = AuthenticationRecordV1 {
        session_id: Some(binding.session_id),
        ..initial(id(200 + slot))
    };
    let receipt = audit.append_typed(intention.clone()).unwrap();
    audit
        .admit_session(
            AuthenticationRecordV1 {
                phase: AuthenticationPhaseV1::Result {
                    outcome: AuthenticationOutcomeV1::Succeeded,
                    reason: AuthenticationReasonV1::Verified,
                },
                principal: Some(binding.principal.clone()),
                causal: Some(receipt),
                ..intention
            },
            binding.clone(),
        )
        .unwrap();
    binding
}

fn intention(
    binding: &AuthenticationSessionBindingV1,
    operation: Uuid,
    action: AuthenticationActionV1,
) -> AuthenticationRecordV1 {
    AuthenticationRecordV1 {
        operation_id: operation,
        action,
        phase: AuthenticationPhaseV1::Intention,
        issuer: binding.issuer.clone(),
        client_id: binding.client_id.clone(),
        principal: Some(binding.principal.clone()),
        transaction_id: None,
        session_id: Some(binding.session_id),
        causal: None,
    }
}

struct Fixture {
    root: tempfile::TempDir,
    path: PathBuf,
    policy: AuthenticationAuditPolicyV1,
    audit: AuthenticationAudit,
    binding: AuthenticationSessionBindingV1,
    refresh: AuthenticationRecordV1,
    irefresh: AuthenticationReceiptV1,
}

impl Fixture {
    fn new(records: u64, bytes: u64) -> Self {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = root.path().join("authentication.sqlite");
        let policy = policy(records, bytes);
        let audit = AuthenticationAudit::init_new(
            &path,
            INSTALLATION,
            policy.clone(),
            Redactor::new(vec![]),
        )
        .unwrap();
        assert_eq!(audit.format_version().unwrap(), 3);
        let binding = admit(&audit, 1);
        let refresh = intention(&binding, id(301), AuthenticationActionV1::Refresh);
        let pending = audit
            .refresh_pending(refresh.clone(), 1, binding.access_fingerprint)
            .unwrap();
        assert!(pending.inserted);
        assert_eq!(pending.receipt.sequence, 4);
        Self {
            root,
            path,
            policy,
            audit,
            binding,
            refresh,
            irefresh: pending.receipt,
        }
    }

    fn logout(&self, operation: Uuid) -> AuthenticationRecordV1 {
        intention(&self.binding, operation, AuthenticationActionV1::Logout)
    }

    fn begin(&self, logout: &AuthenticationRecordV1) -> anyhow::Result<AuthenticationReceiptV1> {
        self.audit.begin_native_logout_during_refresh(
            logout.clone(),
            self.binding.clone(),
            self.refresh.operation_id,
            self.irefresh.clone(),
        )
    }

    fn refresh_result(&self) -> AuthenticationRecordV1 {
        AuthenticationRecordV1 {
            phase: AuthenticationPhaseV1::Result {
                outcome: AuthenticationOutcomeV1::Unknown,
                reason: AuthenticationReasonV1::Interrupted,
            },
            causal: Some(self.irefresh.clone()),
            ..self.refresh.clone()
        }
    }

    fn close_refresh(&self) -> AuthenticationReceiptV1 {
        self.audit
            .close_session(self.refresh_result(), 1, self.binding.access_fingerprint)
            .unwrap()
    }

    fn logout_result(
        &self,
        logout: &AuthenticationRecordV1,
        ilogout: &AuthenticationReceiptV1,
    ) -> AuthenticationRecordV1 {
        AuthenticationRecordV1 {
            phase: AuthenticationPhaseV1::Result {
                outcome: AuthenticationOutcomeV1::Succeeded,
                reason: AuthenticationReasonV1::UserRequested,
            },
            causal: Some(ilogout.clone()),
            ..logout.clone()
        }
    }

    fn end(
        &self,
        logout: &AuthenticationRecordV1,
        ilogout: &AuthenticationReceiptV1,
        frefresh: &AuthenticationReceiptV1,
    ) -> anyhow::Result<AuthenticationReceiptV1> {
        self.audit.record_native_logout_after_refresh_closed(
            self.logout_result(logout, ilogout),
            self.binding.clone(),
            self.refresh.operation_id,
            self.irefresh.clone(),
            frefresh.clone(),
        )
    }

    fn current(&self) -> AuthenticationSessionCurrentV1 {
        self.audit
            .session_current(self.binding.session_id)
            .unwrap()
            .unwrap()
    }

    fn cold(self) -> Self {
        self.reopen_fixture(None)
    }

    // Synthetic compatibility fixture only: no V3-only BEGIN/END may precede
    // this helper. V1/V2 generic/session wire representations are unchanged.
    // This is not a migration path, downgrade operation or historical proof.
    fn reopen_fixture(self, header: Option<i64>) -> Self {
        let Self {
            root,
            path,
            policy,
            audit,
            binding,
            refresh,
            irefresh,
        } = self;
        drop(audit);
        if let Some(version) = header {
            assert!(matches!(version, 1_000_001 | 1_000_002));
            let db = Connection::open(&path).unwrap();
            db.pragma_update(None, "user_version", version).unwrap();
            drop(db);
        }
        let audit = open(&path, &policy);
        Self {
            root,
            path,
            policy,
            audit,
            binding,
            refresh,
            irefresh,
        }
    }
}

fn capacity(error: anyhow::Error) {
    assert_eq!(
        error.downcast_ref::<AuthenticationAuditFailureV1>(),
        Some(&AuthenticationAuditFailureV1::Capacity)
    );
}

// Independent documented wire shape, not the implementation's Entry builder.
fn begin_wire_bytes(f: &Fixture, logout: &AuthenticationRecordV1) -> u64 {
    json!({
        "kind":"session",
        "mutation": {
            "transition":"begin_native_logout_during_refresh",
            "record":logout,
            "expected_binding":f.binding,
            "refresh_operation_id":f.refresh.operation_id,
            "refresh_intention":f.irefresh
        }
    })
    .to_string()
    .len() as u64
}

fn exact_begin_byte_limit() -> u64 {
    let reference = Fixture::new(100, 90_000);
    let required = reference.audit.verify().unwrap().serialized_bytes
        + begin_wire_bytes(&reference, &reference.logout(id(401)))
        + 2 * TERMINAL_BYTES;
    // The policy occurs in genesis. Both reference and rebuilt policies have
    // five decimal digits, so changing the limit does not change genesis size.
    assert!((10_001..100_000).contains(&required));
    required
}

// Removing either R or L from BEGIN's admission calculation must fail this.
#[test]
fn native_logout_intention_protects_its_own_terminal_record_during_refresh() {
    for limit in [6, 7] {
        let f = Fixture::new(limit, 262_144);
        let logout = f.logout(id(401));
        let before = f.audit.verify().unwrap();
        let current = f.current();
        let result = f.begin(&logout);
        if limit == 6 {
            capacity(result.unwrap_err());
            assert_eq!(f.audit.verify().unwrap(), before);
            assert_eq!(f.current(), current);
        } else {
            assert_eq!(result.unwrap().sequence, 5);
            assert_eq!(f.audit.verify().unwrap().records, 5);
            assert_eq!(f.current(), current);
            capacity(f.audit.append_typed(initial(id(501))).unwrap_err());
        }
    }
}

// Charging one terminal, or reserving only the unwrapped record, fails here.
#[test]
fn begin_has_an_exact_two_terminal_byte_boundary() {
    let required = exact_begin_byte_limit();
    for limit in [required - 1, required] {
        let f = Fixture::new(100, limit);
        let before = f.audit.verify().unwrap();
        let current = f.current();
        let result = f.begin(&f.logout(id(401)));
        if limit < required {
            capacity(result.unwrap_err());
            assert_eq!(f.audit.verify().unwrap(), before);
            assert_eq!(f.current(), current);
        } else {
            result.unwrap();
            assert_eq!(
                f.audit.verify().unwrap().serialized_bytes + 2 * TERMINAL_BYTES,
                limit
            );
            capacity(f.audit.append_typed(initial(id(501))).unwrap_err());
        }
    }
}

// An overbroad promise guard, a second closure, or a replay that rearms L
// breaks this real three-stage path, including completely closed handles.
#[test]
fn native_logout_result_is_acknowledged_after_the_original_refresh_is_closed() {
    let mut f = Fixture::new(7, 262_144);
    let logout = f.logout(id(401));
    let ilogout = f.begin(&logout).unwrap();
    assert_eq!(ilogout.sequence, 5);
    capacity(f.audit.append_typed(initial(id(501))).unwrap_err());
    let began = f.audit.verify().unwrap();
    assert_eq!(f.begin(&logout).unwrap(), ilogout);
    assert_eq!(f.audit.verify().unwrap(), began);
    f = f.cold();
    assert_eq!(f.audit.verify().unwrap(), began);
    assert_eq!(f.begin(&logout).unwrap(), ilogout);
    let frefresh = f.close_refresh();
    assert_eq!(frefresh.sequence, 6);
    let closed = f.current();
    assert_eq!(closed.binding, f.binding);
    assert_eq!(
        closed.state,
        AuthenticationSessionStateV1::Closed {
            reason: AuthenticationReasonV1::Interrupted
        }
    );
    capacity(f.audit.append_typed(initial(id(501))).unwrap_err());
    let refreshed = f.audit.verify().unwrap();
    f = f.cold();
    assert_eq!(f.begin(&logout).unwrap(), ilogout);
    assert_eq!(f.close_refresh(), frefresh);
    assert_eq!(f.audit.verify().unwrap(), refreshed);
    let flogout = f.end(&logout, &ilogout, &frefresh).unwrap();
    assert_eq!(flogout.sequence, 7);
    assert_ne!(flogout, frefresh);
    assert_eq!(f.current(), closed);
    let finished = f.audit.verify().unwrap();
    assert_eq!(finished.head, flogout);
    for cold in [false, true] {
        if cold {
            f = f.cold();
        }
        assert_eq!(f.begin(&logout).unwrap(), ilogout);
        assert_eq!(f.close_refresh(), frefresh);
        assert_eq!(f.end(&logout, &ilogout, &frefresh).unwrap(), flogout);
        assert_eq!(f.audit.verify().unwrap(), finished);
        assert_eq!(f.current(), closed);
        capacity(f.audit.append_typed(initial(id(501))).unwrap_err());
    }
    let before = f.audit.verify().unwrap();
    assert!(
        f.audit
            .append_typed(f.logout_result(&logout, &ilogout))
            .is_err()
    );
    assert!(
        f.audit
            .close_session(
                f.logout_result(&logout, &ilogout),
                1,
                f.binding.access_fingerprint
            )
            .is_err()
    );
    assert_eq!(f.audit.verify().unwrap(), before);
}

// A second SQLite handle may spend spare capacity, never R or L.
#[test]
fn competing_handle_cannot_spend_either_terminal_and_both_terminals_still_finish() {
    let f = Fixture::new(12, 262_144);
    let other_binding = admit(&f.audit, 2);
    let other_refresh = intention(&other_binding, id(302), AuthenticationActionV1::Refresh);
    let other_pending = f
        .audit
        .refresh_pending(other_refresh.clone(), 1, other_binding.access_fingerprint)
        .unwrap();
    assert!(other_pending.inserted);
    let other = open(&f.path, &f.policy);
    let logout = f.logout(id(401));
    let ilogout = f.begin(&logout).unwrap();
    other.append_typed(initial(id(501))).unwrap();
    let before = f.audit.verify().unwrap();
    assert_eq!(before.records, 9);
    capacity(other.append_typed(initial(id(502))).unwrap_err());
    assert_eq!(f.audit.verify().unwrap(), before);
    let frefresh = f.close_refresh();
    capacity(other.append_typed(initial(id(502))).unwrap_err());
    let flogout = other
        .record_native_logout_after_refresh_closed(
            f.logout_result(&logout, &ilogout),
            f.binding.clone(),
            f.refresh.operation_id,
            f.irefresh.clone(),
            frefresh.clone(),
        )
        .unwrap();
    assert_eq!(flogout.sequence, 11);
    assert_eq!(f.end(&logout, &ilogout, &frefresh).unwrap(), flogout);
    assert_eq!(f.audit.verify().unwrap().records, 11);
    capacity(other.append_typed(initial(id(502))).unwrap_err());
    assert_eq!(
        other
            .close_session(
                AuthenticationRecordV1 {
                    phase: AuthenticationPhaseV1::Result {
                        outcome: AuthenticationOutcomeV1::Unknown,
                        reason: AuthenticationReasonV1::Interrupted,
                    },
                    causal: Some(other_pending.receipt),
                    ..other_refresh
                },
                1,
                other_binding.access_fingerprint
            )
            .unwrap()
            .sequence,
        12
    );
}

// Non-atomic lookup/insert of the one-promise slot permits two winners.
#[test]
fn concurrent_begins_on_one_session_have_one_winner_and_a_completable_promise() {
    let f = Fixture::new(20, 262_144);
    let barrier = Arc::new(Barrier::new(2));
    let mut threads = Vec::new();
    for operation in [id(401), id(402)] {
        let audit = open(&f.path, &f.policy);
        let binding = f.binding.clone();
        let refresh_operation = f.refresh.operation_id;
        let irefresh = f.irefresh.clone();
        let logout = f.logout(operation);
        let ready = barrier.clone();
        threads.push(thread::spawn(move || {
            ready.wait();
            let result = audit.begin_native_logout_during_refresh(
                logout.clone(),
                binding,
                refresh_operation,
                irefresh,
            );
            (logout, result)
        }));
    }
    let outcomes: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(
        outcomes.iter().filter(|(_, result)| result.is_ok()).count(),
        1
    );
    assert_eq!(f.audit.verify().unwrap().records, 5);
    let (logout, ilogout) = outcomes
        .into_iter()
        .find(|(_, result)| result.is_ok())
        .unwrap();
    let frefresh = f.close_refresh();
    assert_eq!(
        f.end(&logout, &ilogout.unwrap(), &frefresh)
            .unwrap()
            .sequence,
        7
    );
}

// Each contender owns a separate real SQLite handle. The durable winner may
// advance first, but a BEGIN that wins must retain a completable R/L pair.
#[test]
fn concurrent_begin_serializes_with_rotation_original_close_and_other_close() {
    for action in 0..3 {
        let f = Fixture::new(50, 262_144);
        let logout = f.logout(id(401));
        let mut next = f.binding.clone();
        next.generation += 1;
        next.access_fingerprint = AuthenticationTokenFingerprintV1::access_token("race-next");
        let peer_record = match action {
            0 => AuthenticationRecordV1 {
                phase: AuthenticationPhaseV1::Result {
                    outcome: AuthenticationOutcomeV1::Succeeded,
                    reason: AuthenticationReasonV1::Verified,
                },
                causal: Some(f.irefresh.clone()),
                ..f.refresh.clone()
            },
            1 => f.refresh_result(),
            _ => {
                let requested = f.logout(id(402));
                let receipt = f.audit.append_typed(requested.clone()).unwrap();
                f.logout_result(&requested, &receipt)
            }
        };
        let begin_audit = open(&f.path, &f.policy);
        let peer_audit = open(&f.path, &f.policy);
        let binding = f.binding.clone();
        let begin_record = logout.clone();
        let operation = f.refresh.operation_id;
        let irefresh = f.irefresh.clone();
        let access = binding.access_fingerprint;
        let ready = Arc::new(Barrier::new(2));
        let (began, peer) = thread::scope(|scope| {
            let begin_ready = Arc::clone(&ready);
            let begin = scope.spawn(move || {
                begin_ready.wait();
                begin_audit.begin_native_logout_during_refresh(
                    begin_record,
                    binding,
                    operation,
                    irefresh,
                )
            });
            let peer = scope.spawn(move || {
                ready.wait();
                if action == 0 {
                    peer_audit.rotate_session(peer_record, next, 1, access)
                } else {
                    peer_audit.close_session(peer_record, 1, access)
                }
            });
            (begin.join().unwrap(), peer.join().unwrap())
        });
        match began {
            Ok(ilogout) => {
                let frefresh = if action == 1 {
                    peer.expect("the exact original Close must progress after BEGIN")
                } else {
                    assert!(peer.is_err(), "a competing transition bypassed the promise");
                    f.close_refresh()
                };
                let closed = f.current();
                assert_eq!(closed.binding, f.binding);
                assert_eq!(
                    closed.state,
                    AuthenticationSessionStateV1::Closed {
                        reason: AuthenticationReasonV1::Interrupted,
                    }
                );
                f.end(&logout, &ilogout, &frefresh).unwrap();
                assert_eq!(f.begin(&logout).unwrap(), ilogout);
                assert_eq!(f.current(), closed);
            }
            Err(_) => {
                assert!(peer.is_ok(), "neither serialized transition progressed");
                let before = f.audit.verify().unwrap();
                assert!(
                    f.begin(&logout).is_err(),
                    "a stale binding acquired a promise"
                );
                assert_eq!(f.audit.verify().unwrap(), before);
            }
        }
    }
}

// Removing the promise guard must not let a late successful refresh rotate,
// or an unrelated Logout/Expiration close strand the promised END.
#[test]
fn promise_refuses_rotation_and_other_closes_but_allows_its_original_refresh_close() {
    let f = Fixture::new(50, 262_144);
    let logout = f.logout(id(401));
    let ilogout = f.begin(&logout).unwrap();
    let mut next = f.binding.clone();
    next.generation += 1;
    next.access_fingerprint =
        AuthenticationTokenFingerprintV1::access_token("synthetic-next-access");
    let successful_refresh = AuthenticationRecordV1 {
        phase: AuthenticationPhaseV1::Result {
            outcome: AuthenticationOutcomeV1::Succeeded,
            reason: AuthenticationReasonV1::Verified,
        },
        causal: Some(f.irefresh.clone()),
        ..f.refresh.clone()
    };
    let before = f.audit.verify().unwrap();
    assert!(
        f.audit
            .rotate_session(successful_refresh, next, 1, f.binding.access_fingerprint)
            .is_err()
    );
    assert_eq!(f.audit.verify().unwrap(), before);
    for (operation, action, reason) in [
        (
            id(402),
            AuthenticationActionV1::Logout,
            AuthenticationReasonV1::UserRequested,
        ),
        (
            id(403),
            AuthenticationActionV1::Expiration,
            AuthenticationReasonV1::Expired,
        ),
    ] {
        let requested = intention(&f.binding, operation, action);
        let receipt = f.audit.append_typed(requested.clone()).unwrap();
        let before = f.audit.verify().unwrap();
        let current = f.current();
        assert!(
            f.audit
                .close_session(
                    AuthenticationRecordV1 {
                        phase: AuthenticationPhaseV1::Result {
                            outcome: AuthenticationOutcomeV1::Succeeded,
                            reason,
                        },
                        causal: Some(receipt),
                        ..requested
                    },
                    1,
                    f.binding.access_fingerprint
                )
                .is_err()
        );
        assert_eq!(f.audit.verify().unwrap(), before);
        assert_eq!(f.current(), current);
    }
    let frefresh = f.close_refresh();
    f.end(&logout, &ilogout, &frefresh).unwrap();
}

// Receipt existence and valid independent bindings are insufficient to splice
// two sessions, generations or fingerprints into one logout proof.
#[test]
fn begin_and_end_refuse_cross_binding_and_false_receipts_without_changes() {
    let f = Fixture::new(50, 262_144);
    let other_binding = admit(&f.audit, 2);
    let logout = f.logout(id(401));
    let mut next_generation = f.binding.clone();
    next_generation.generation += 1;
    let mut wrong_fingerprint = f.binding.clone();
    wrong_fingerprint.access_fingerprint =
        AuthenticationTokenFingerprintV1::access_token("synthetic-other-access");
    let mut wrong_namespace = f.irefresh.clone();
    wrong_namespace.namespace_id = id(999);
    for (binding, receipt) in [
        (other_binding.clone(), f.irefresh.clone()),
        (next_generation, f.irefresh.clone()),
        (wrong_fingerprint, f.irefresh.clone()),
        (f.binding.clone(), wrong_namespace),
    ] {
        let before = f.audit.verify().unwrap();
        let current = f.current();
        assert!(
            f.audit
                .begin_native_logout_during_refresh(
                    logout.clone(),
                    binding,
                    f.refresh.operation_id,
                    receipt
                )
                .is_err()
        );
        assert_eq!(f.audit.verify().unwrap(), before);
        assert_eq!(f.current(), current);
    }
    let ilogout = f.begin(&logout).unwrap();
    let frefresh = f.close_refresh();
    let before = f.audit.verify().unwrap();
    assert!(
        f.audit
            .record_native_logout_after_refresh_closed(
                f.logout_result(&logout, &ilogout),
                other_binding,
                f.refresh.operation_id,
                f.irefresh.clone(),
                frefresh.clone()
            )
            .is_err()
    );
    let mut false_hash = frefresh.clone();
    false_hash.hash = "0".repeat(64);
    assert!(f.end(&logout, &ilogout, &false_hash).is_err());
    assert_eq!(f.audit.verify().unwrap(), before);
    f.end(&logout, &ilogout, &frefresh).unwrap();
}

// A generic intention cannot be upgraded into a reserved BEGIN, even in V3.
#[test]
fn generic_logout_intention_never_creates_a_terminal_promise() {
    let f = Fixture::new(20, 262_144);
    let logout = f.logout(id(401));
    let ilogout = f.audit.append_typed(logout.clone()).unwrap();
    let before = f.audit.verify().unwrap();
    assert_eq!(
        f.begin(&logout)
            .unwrap_err()
            .downcast_ref::<AuthenticationAuditFailureV1>(),
        Some(&AuthenticationAuditFailureV1::Conflict)
    );
    assert_eq!(f.audit.verify().unwrap(), before);
    let frefresh = f.close_refresh();
    let closed = f.audit.verify().unwrap();
    assert!(f.end(&logout, &ilogout, &frefresh).is_err());
    assert!(f.begin(&f.logout(id(402))).is_err());
    assert_eq!(f.audit.verify().unwrap(), closed);
}

type StoredEntry = (
    i64,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);

fn stored_entries(path: &Path) -> Vec<StoredEntry> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut statement = db.prepare(
        "SELECT seq,received_at,prev_hash,hash,payload,operation_id,phase FROM authentication_entries ORDER BY seq"
    ).unwrap();
    let entries = statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    entries
}

// Header migration preserves old bytes and does not turn any generic ILogout
// into a promise. Only a new distinct BEGIN may acquire L after migration.
#[test]
fn v2_to_v3_requires_a_new_backup_preserves_history_and_does_not_promote_generic_logout() {
    let mut f = Fixture::new(32, 262_144);
    let old_logout = f.logout(id(401));
    let old_receipt = f.audit.append_typed(old_logout.clone()).unwrap();
    f = f.reopen_fixture(Some(1_000_002));
    assert_eq!(f.audit.format_version().unwrap(), 2);
    let before = f.audit.verify().unwrap();
    let entries = stored_entries(&f.path);
    assert!(f.begin(&f.logout(id(402))).is_err());
    assert_eq!(f.audit.verify().unwrap(), before);
    let existing = f.root.path().join("existing.backup");
    fs::write(&existing, b"synthetic-preserve-existing").unwrap();
    fs::set_permissions(&existing, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(f.audit.migrate_native_logout_v3(&existing).is_err());
    assert_eq!(fs::read(&existing).unwrap(), b"synthetic-preserve-existing");
    assert_eq!(f.audit.format_version().unwrap(), 2);
    assert_eq!(f.audit.verify().unwrap(), before);
    let backup = f.root.path().join("new-v2.backup");
    assert_eq!(f.audit.migrate_native_logout_v3(&backup).unwrap(), before);
    assert_eq!(f.audit.format_version().unwrap(), 3);
    assert_eq!(f.audit.verify().unwrap(), before);
    assert_eq!(stored_entries(&f.path), entries);
    assert_eq!(stored_entries(&backup), entries);
    let old = open(&backup, &f.policy);
    assert_eq!(old.format_version().unwrap(), 2);
    assert_eq!(old.verify().unwrap(), before);
    drop(old);
    assert!(f.begin(&old_logout).is_err());
    assert_eq!(f.audit.verify().unwrap(), before);
    let new_logout = f.logout(id(402));
    let new_receipt = f.begin(&new_logout).unwrap();
    let frefresh = f.close_refresh();
    let closed = f.audit.verify().unwrap();
    assert!(f.end(&old_logout, &old_receipt, &frefresh).is_err());
    assert_eq!(f.audit.verify().unwrap(), closed);
    f.end(&new_logout, &new_receipt, &frefresh).unwrap();
    let second_backup = f.root.path().join("must-not-exist.backup");
    assert!(f.audit.migrate_native_logout_v3(&second_backup).is_err());
    assert!(!second_backup.exists());
}

// Bumping VERSION must not silently redirect the established V1->V2 port.
#[test]
fn v1_migration_still_stops_at_v2_before_the_explicit_v3_step() {
    let f = Fixture::new(32, 262_144).reopen_fixture(Some(1_000_001));
    let before = f.audit.verify().unwrap();
    let entries = stored_entries(&f.path);
    assert_eq!(f.audit.format_version().unwrap(), 1);
    assert_eq!(
        f.audit
            .migrate_verified_initial_v2(&f.root.path().join("v1.backup"))
            .unwrap(),
        before
    );
    assert_eq!(f.audit.format_version().unwrap(), 2);
    assert_eq!(stored_entries(&f.path), entries);
    assert_eq!(
        f.audit
            .migrate_native_logout_v3(&f.root.path().join("v2.backup"))
            .unwrap(),
        before
    );
    assert_eq!(f.audit.format_version().unwrap(), 3);
    assert_eq!(stored_entries(&f.path), entries);
}

// Only this new synthetic fixture is rewritten, with all handles closed. It
// represents a hash/causal/quota-valid legacy prefix whose R was not funded.
// That history remains readable; migration must not declare a new L funded.
#[test]
fn legacy_underfunded_pending_stays_readable_but_cannot_acquire_a_logout_promise() {
    let f = Fixture::new(50, 262_144);
    let Fixture {
        root,
        path,
        mut policy,
        audit,
        binding,
        refresh,
        mut irefresh,
    } = f;
    drop(audit);
    policy.max_records = 4;
    let rows = stored_entries(&path);
    let mut db = Connection::open(&path).unwrap();
    let tx = db.transaction().unwrap();
    let mut previous = "0".repeat(64);
    let mut hashes = std::collections::BTreeMap::<u64, String>::new();
    let mut bytes = 0_u64;
    for (sequence, received, _, _, payload, _, _) in rows {
        let mut value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        if sequence == 1 {
            value["policy"] = serde_json::to_value(&policy).unwrap();
        } else {
            let record = if value["kind"] == "session" {
                &mut value["mutation"]["record"]
            } else {
                &mut value["record"]
            };
            if let Some(causal) = record.get_mut("causal").filter(|value| value.is_object()) {
                let sequence = causal["sequence"].as_u64().unwrap();
                causal["hash"] = json!(hashes[&sequence]);
            }
        }
        let payload = value.to_string();
        let mut digest = Sha256::new();
        digest.update(b"ai-guard:authentication-audit:v1\0");
        digest.update((sequence as u64).to_be_bytes());
        for part in [received.as_str(), previous.as_str(), payload.as_str()] {
            digest.update(part.as_bytes());
            digest.update([0]);
        }
        let hash = hex::encode(digest.finalize());
        tx.execute(
            "UPDATE authentication_entries SET prev_hash=?1,hash=?2,payload=?3 WHERE seq=?4",
            params![previous, hash, payload, sequence],
        )
        .unwrap();
        if sequence == 1 {
            tx.execute(
                "UPDATE authentication_meta SET genesis_hash=?1 WHERE singleton=1",
                [&hash],
            )
            .unwrap();
        }
        if sequence as u64 == irefresh.sequence {
            irefresh.hash = hash.clone();
        }
        bytes += payload.len() as u64;
        hashes.insert(sequence as u64, hash.clone());
        previous = hash;
    }
    tx.execute(
        "UPDATE authentication_quota SET bytes=?1 WHERE singleton=1",
        [bytes as i64],
    )
    .unwrap();
    tx.pragma_update(None, "user_version", 1_000_002_i64)
        .unwrap();
    tx.commit().unwrap();
    drop(db);
    let audit = open(&path, &policy);
    let f = Fixture {
        root,
        path,
        policy,
        audit,
        binding,
        refresh,
        irefresh,
    };
    let legacy = f.audit.verify().unwrap();
    assert_eq!(legacy.records, 4);
    assert_eq!(f.audit.format_version().unwrap(), 2);
    assert_eq!(
        f.audit
            .migrate_native_logout_v3(&f.root.path().join("legacy-v2.backup"))
            .unwrap(),
        legacy
    );
    let before = f.audit.verify().unwrap();
    capacity(f.begin(&f.logout(id(401))).unwrap_err());
    assert_eq!(f.audit.verify().unwrap(), before);
    assert!(matches!(
        f.current().state,
        AuthenticationSessionStateV1::RefreshPending { .. }
    ));
    assert_eq!(f.cold().audit.verify().unwrap(), before);
}

#[test]
fn v3_with_a_live_promise_is_not_remigrated_or_reset() {
    let f = Fixture::new(7, 262_144);
    let logout = f.logout(id(401));
    let ilogout = f.begin(&logout).unwrap();
    let before = f.audit.verify().unwrap();
    let refused_backup = f.root.path().join("must-not-reset.backup");
    assert!(f.audit.migrate_native_logout_v3(&refused_backup).is_err());
    assert!(!refused_backup.exists());
    assert_eq!(f.audit.verify().unwrap(), before);
    let f = f.cold();
    let frefresh = f.close_refresh();
    assert_eq!(f.end(&logout, &ilogout, &frefresh).unwrap().sequence, 7);
}

// A hash-valid ordinary append that spends L must fail cold replay too, even
// though committed counters alone are below the byte limit. This deliberately
// corrupts only its new synthetic tempfile after every audit handle is dropped.
#[test]
fn cold_replay_refuses_an_ordinary_prefix_that_spends_reserved_logout_bytes() {
    let f = Fixture::new(100, exact_begin_byte_limit());
    f.begin(&f.logout(id(401))).unwrap();
    let before = f.audit.verify().unwrap();
    let Fixture {
        root: _root,
        path,
        policy,
        audit,
        ..
    } = f;
    drop(audit);
    let payload = json!({"kind":"authentication", "record":initial(id(501))}).to_string();
    let sequence = before.records + 1;
    let received = "2026-11-10T00:00:00.000000000Z";
    let mut digest = Sha256::new();
    digest.update(b"ai-guard:authentication-audit:v1\0");
    digest.update(sequence.to_be_bytes());
    for value in [received, before.head.hash.as_str(), payload.as_str()] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    let hash = hex::encode(digest.finalize());
    let bytes = before.serialized_bytes + payload.len() as u64;
    assert!(bytes < policy.max_serialized_bytes);
    let mut db = Connection::open(&path).unwrap();
    let tx = db.transaction().unwrap();
    tx.execute(
        "INSERT INTO authentication_entries VALUES(?1,?2,?3,?4,?5,?6,'intention')",
        params![
            sequence as i64,
            received,
            before.head.hash,
            hash,
            payload,
            id(501).to_string()
        ],
    )
    .unwrap();
    tx.execute(
        "UPDATE authentication_quota SET records=?1,bytes=?2 WHERE singleton=1",
        params![sequence as i64, bytes as i64],
    )
    .unwrap();
    tx.commit().unwrap();
    drop(db);
    assert!(
        AuthenticationAudit::open_existing(&path, INSTALLATION, policy, Redactor::new(vec![]))
            .is_err()
    );
}

// Shared only with format's private arithmetic oracle. This is an authentic
// stored prefix before the final END, retaining another session's R allowance.
pub(super) fn last_end_fixture() -> (
    tempfile::TempDir,
    PathBuf,
    AuthenticationAuditPolicyV1,
    Uuid,
    format::Entry,
) {
    let f = Fixture::new(50, 262_144);
    let other = admit(&f.audit, 2);
    f.audit
        .refresh_pending(
            intention(&other, id(302), AuthenticationActionV1::Refresh),
            1,
            other.access_fingerprint,
        )
        .unwrap();
    let logout = f.logout(id(401));
    let ilogout = f.begin(&logout).unwrap();
    let frefresh = f.close_refresh();
    let entry = format::Entry::Session {
        mutation: Box::new(
            format::SessionMutation::RecordNativeLogoutAfterRefreshClosed {
                record: f.logout_result(&logout, &ilogout),
                expected_binding: f.binding.clone(),
                refresh_operation_id: f.refresh.operation_id,
                refresh_intention: f.irefresh.clone(),
                refresh_result: frefresh,
            },
        ),
    };
    let Fixture {
        root,
        path,
        policy,
        audit,
        ..
    } = f;
    drop(audit);
    (root, path, policy, INSTALLATION, entry)
}
