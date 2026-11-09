//! Real private SQLite/replay only. No HTTP request or identity provider is
//! invoked, and this fixture does not establish a provider exchange effect.
#![cfg(unix)]

use agentsandbox::{audit::authentication::*, identity::PrincipalRefV1, redaction::Redactor};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
use uuid::Uuid;

const INSTALLATION: Uuid = Uuid::from_u128(0x11111111111141118111111111111111);
const ISSUER: &str = "https://identity.example.test/realm";

fn policy(records: u64, bytes: u64) -> AuthenticationAuditPolicyV1 {
    AuthenticationAuditPolicyV1 {
        max_records: records,
        max_serialized_bytes: bytes,
        approved_clients: vec![ApprovedAuthenticationClientV1 {
            issuer: ISSUER.into(),
            client_id: "public-web".into(),
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
        action: AuthenticationActionV1::Login,
        phase: AuthenticationPhaseV1::Intention,
        issuer: ISSUER.into(),
        client_id: "public-web".into(),
        principal: None,
        transaction_id: Some(operation),
        session_id: None,
        causal: None,
    }
}

fn fixture(
    policy: &AuthenticationAuditPolicyV1,
) -> (
    tempfile::TempDir,
    AuthenticationAudit,
    AuthenticationSessionBindingV1,
) {
    let root = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let audit = AuthenticationAudit::init_new(
        &root.path().join("authentication.sqlite"),
        INSTALLATION,
        policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap();
    let binding = AuthenticationSessionBindingV1 {
        session_id: Uuid::new_v4(),
        generation: 1,
        principal: PrincipalRefV1::HumanOidc {
            issuer: ISSUER.into(),
            subject: "storage-fixture-subject".into(),
        },
        issuer: ISSUER.into(),
        client_id: "public-web".into(),
        kind: OidcSessionKindV1::WebPkce,
        access_fingerprint: AuthenticationTokenFingerprintV1::access_token("fixture-access-one"),
        refresh_fingerprint: Some(AuthenticationTokenFingerprintV1::refresh_token(
            "fixture-refresh-one",
        )),
        expires_at_unix_seconds: 4_000_000_000,
    };
    let intention = AuthenticationRecordV1 {
        session_id: Some(binding.session_id),
        ..initial(Uuid::new_v4())
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
    assert_eq!(audit.verify().unwrap().records, 3);
    (root, audit, binding)
}

fn refresh(binding: &AuthenticationSessionBindingV1) -> AuthenticationRecordV1 {
    AuthenticationRecordV1 {
        operation_id: Uuid::new_v4(),
        action: AuthenticationActionV1::Refresh,
        phase: AuthenticationPhaseV1::Intention,
        issuer: binding.issuer.clone(),
        client_id: binding.client_id.clone(),
        principal: Some(binding.principal.clone()),
        transaction_id: None,
        session_id: Some(binding.session_id),
        causal: None,
    }
}

fn capacity(error: anyhow::Error) {
    assert_eq!(
        error.downcast_ref::<AuthenticationAuditFailureV1>(),
        Some(&AuthenticationAuditFailureV1::Capacity)
    );
}

#[test]
fn pending_refuses_before_exchange_if_records_or_terminal_bytes_cannot_be_reserved() {
    // One case leaves exactly the intention's last record; the other has
    // abundant records but less than the existing 32_768-byte payload bound.
    for policy in [policy(4, 131_072), policy(100, 8_192)] {
        let (_root, audit, binding) = fixture(&policy);
        let before = audit.verify().unwrap();
        let pending = refresh(&binding);
        let result = audit.refresh_pending(pending, 1, binding.access_fingerprint);
        capacity(
            result
                .err()
                .expect("an intention must not spend the terminal's capacity"),
        );
        let after = audit.verify().unwrap();
        assert_eq!(after, before, "refusal must precede any durable Pending");
        assert_eq!(
            audit
                .session_current(binding.session_id)
                .unwrap()
                .unwrap()
                .state,
            AuthenticationSessionStateV1::Active
        );
    }
}

#[test]
fn terminal_record_is_protected_from_other_appends_after_reopen_and_consumed_once() {
    let policy = policy(6, 131_072);
    let (root, audit, binding) = fixture(&policy);
    let pending = refresh(&binding);
    let admitted = audit
        .refresh_pending(pending.clone(), 1, binding.access_fingerprint)
        .unwrap();
    assert!(admitted.inserted);
    audit.append_typed(initial(Uuid::new_v4())).unwrap();
    assert_eq!(audit.verify().unwrap().records, 5);
    capacity(audit.append_typed(initial(Uuid::new_v4())).unwrap_err());
    let before = audit.verify().unwrap();
    drop(audit);
    let path = root.path().join("authentication.sqlite");
    let audit = open(&path, &policy);
    assert_eq!(audit.verify().unwrap(), before);
    let replay = audit
        .refresh_pending(pending.clone(), 1, binding.access_fingerprint)
        .unwrap();
    assert!(!replay.inserted);
    assert_eq!(replay.receipt, admitted.receipt);
    let terminal = AuthenticationRecordV1 {
        phase: AuthenticationPhaseV1::Result {
            outcome: AuthenticationOutcomeV1::Failed,
            reason: AuthenticationReasonV1::ProviderUnavailable,
        },
        causal: Some(admitted.receipt),
        ..pending
    };
    let receipt = audit
        .close_session(terminal.clone(), 1, binding.access_fingerprint)
        .unwrap();
    assert_eq!(audit.verify().unwrap().records, 6);
    assert_eq!(
        audit
            .close_session(terminal.clone(), 1, binding.access_fingerprint)
            .unwrap(),
        receipt
    );
    let finished = audit.verify().unwrap();
    drop(audit);
    let audit = open(&path, &policy);
    assert_eq!(audit.verify().unwrap(), finished);
    assert_eq!(
        audit
            .session_current(binding.session_id)
            .unwrap()
            .unwrap()
            .state,
        AuthenticationSessionStateV1::Closed {
            reason: AuthenticationReasonV1::ProviderUnavailable
        }
    );
    assert_eq!(
        audit
            .close_session(terminal, 1, binding.access_fingerprint)
            .unwrap(),
        receipt
    );
    assert_eq!(audit.verify().unwrap(), finished);
    capacity(audit.append_typed(initial(Uuid::new_v4())).unwrap_err());
}

#[test]
fn terminal_bytes_survive_competing_appends_and_unused_reserve_releases_after_rotation() {
    let policy = policy(100, 40_000);
    let (root, audit, old) = fixture(&policy);
    let pending = refresh(&old);
    let admitted = audit
        .refresh_pending(pending.clone(), 1, old.access_fingerprint)
        .unwrap();
    assert!(admitted.inserted);
    let mut refused = false;
    for _ in 0..64 {
        match audit.append_typed(initial(Uuid::new_v4())) {
            Ok(_) => {
                let used = audit.verify().unwrap();
                assert!(
                    used.serialized_bytes + 32_768 <= policy.max_serialized_bytes,
                    "an unrelated entry consumed the terminal byte reserve"
                );
            }
            Err(error) => {
                capacity(error);
                refused = true;
                break;
            }
        }
    }
    assert!(
        refused,
        "bounded competing writes must hit the protected byte boundary"
    );
    let before = audit.verify().unwrap();
    assert!(
        before.records < policy.max_records,
        "this case must exhaust bytes, not records"
    );
    drop(audit);
    let path = root.path().join("authentication.sqlite");
    let audit = open(&path, &policy);
    assert_eq!(audit.verify().unwrap(), before);
    let mut next = old.clone();
    next.generation = 2;
    next.access_fingerprint = AuthenticationTokenFingerprintV1::access_token("fixture-access-two");
    next.refresh_fingerprint = Some(AuthenticationTokenFingerprintV1::refresh_token(
        "fixture-refresh-two",
    ));
    let terminal = AuthenticationRecordV1 {
        phase: AuthenticationPhaseV1::Result {
            outcome: AuthenticationOutcomeV1::Succeeded,
            reason: AuthenticationReasonV1::Verified,
        },
        causal: Some(admitted.receipt),
        ..pending
    };
    let receipt = audit
        .rotate_session(terminal.clone(), next.clone(), 1, old.access_fingerprint)
        .unwrap();
    assert_eq!(
        audit
            .rotate_session(terminal.clone(), next.clone(), 1, old.access_fingerprint)
            .unwrap(),
        receipt
    );
    audit.append_typed(initial(Uuid::new_v4())).unwrap();
    let finished = audit.verify().unwrap();
    assert_eq!(
        finished.records,
        before.records + 2,
        "unused reserved bytes become available only once"
    );
    drop(audit);
    let audit = open(&path, &policy);
    assert_eq!(audit.verify().unwrap(), finished);
    let current = audit.session_current(old.session_id).unwrap().unwrap();
    assert_eq!(current.binding, next);
    assert_eq!(current.state, AuthenticationSessionStateV1::Active);
    assert_eq!(
        audit
            .rotate_session(terminal, next, 1, old.access_fingerprint)
            .unwrap(),
        receipt
    );
    assert_eq!(audit.verify().unwrap(), finished);
}

fn second_session(
    audit: &AuthenticationAudit,
    first: &AuthenticationSessionBindingV1,
) -> AuthenticationSessionBindingV1 {
    let mut binding = first.clone();
    binding.session_id = Uuid::new_v4();
    binding.principal = PrincipalRefV1::HumanOidc {
        issuer: ISSUER.into(),
        subject: "storage-fixture-second-subject".into(),
    };
    binding.access_fingerprint = AuthenticationTokenFingerprintV1::access_token("second-access");
    binding.refresh_fingerprint = Some(AuthenticationTokenFingerprintV1::refresh_token(
        "second-refresh",
    ));
    let intention = AuthenticationRecordV1 {
        session_id: Some(binding.session_id),
        ..initial(Uuid::new_v4())
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
    assert_eq!(audit.verify().unwrap().records, 5);
    binding
}

fn failed_terminal(
    pending: AuthenticationRecordV1,
    receipt: AuthenticationReceiptV1,
) -> AuthenticationRecordV1 {
    AuthenticationRecordV1 {
        phase: AuthenticationPhaseV1::Result {
            outcome: AuthenticationOutcomeV1::Failed,
            reason: AuthenticationReasonV1::ProviderUnavailable,
        },
        causal: Some(receipt),
        ..pending
    }
}

#[test]
fn two_open_handles_preserve_both_pending_terminal_record_reserves() {
    let policy = policy(10, 262_144);
    let (root, first, a) = fixture(&policy);
    let b = second_session(&first, &a);
    let path = root.path().join("authentication.sqlite");
    // Both public handles are open before either Pending transaction. Their
    // independent retained floors must not hide the other handle's new state.
    let second = open(&path, &policy);
    let pending_a = refresh(&a);
    let pending_b = refresh(&b);
    let admitted_a = first
        .refresh_pending(pending_a.clone(), 1, a.access_fingerprint)
        .unwrap();
    let admitted_b = second
        .refresh_pending(pending_b.clone(), 1, b.access_fingerprint)
        .unwrap();
    assert!(admitted_a.inserted && admitted_b.inserted);
    assert_ne!(admitted_a.receipt, admitted_b.receipt);
    assert_eq!(first.verify().unwrap(), second.verify().unwrap());
    assert_eq!(first.verify().unwrap().records, 7);
    for (handle, binding) in [(&first, &a), (&second, &b)] {
        assert!(matches!(
            handle
                .session_current(binding.session_id)
                .unwrap()
                .unwrap()
                .state,
            AuthenticationSessionStateV1::RefreshPending { .. }
        ));
    }
    first.append_typed(initial(Uuid::new_v4())).unwrap();
    let held = first.verify().unwrap();
    assert_eq!(held.records, 8);
    capacity(second.append_typed(initial(Uuid::new_v4())).unwrap_err());
    assert_eq!(second.verify().unwrap(), held);
    // Cross handles to complete each other's Pending. These are deliberately
    // sequenced transactions, not a claim of process/crash or simultaneous races.
    let terminal_a = failed_terminal(pending_a, admitted_a.receipt);
    let receipt_a = second
        .close_session(terminal_a.clone(), 1, a.access_fingerprint)
        .unwrap();
    let one_left = second.verify().unwrap();
    assert_eq!(one_left.records, 9);
    capacity(first.append_typed(initial(Uuid::new_v4())).unwrap_err());
    assert_eq!(
        first
            .close_session(terminal_a.clone(), 1, a.access_fingerprint)
            .unwrap(),
        receipt_a
    );
    assert_eq!(
        first.verify().unwrap(),
        one_left,
        "replay must not release B's reserve"
    );
    let terminal_b = failed_terminal(pending_b, admitted_b.receipt);
    let receipt_b = first
        .close_session(terminal_b.clone(), 1, b.access_fingerprint)
        .unwrap();
    let finished = first.verify().unwrap();
    assert_eq!(finished.records, 10);
    assert_eq!(
        second
            .close_session(terminal_b.clone(), 1, b.access_fingerprint)
            .unwrap(),
        receipt_b
    );
    assert_eq!(second.verify().unwrap(), finished);
    drop(first);
    drop(second);
    let reopened = open(&path, &policy);
    assert_eq!(reopened.verify().unwrap(), finished);
    for binding in [&a, &b] {
        let current = reopened
            .session_current(binding.session_id)
            .unwrap()
            .unwrap();
        assert_eq!(current.binding, *binding);
        assert_eq!(
            current.state,
            AuthenticationSessionStateV1::Closed {
                reason: AuthenticationReasonV1::ProviderUnavailable,
            }
        );
    }
    assert_eq!(
        reopened
            .close_session(terminal_a, 1, a.access_fingerprint)
            .unwrap(),
        receipt_a
    );
    assert_eq!(
        reopened
            .close_session(terminal_b, 1, b.access_fingerprint)
            .unwrap(),
        receipt_b
    );
    assert_eq!(reopened.verify().unwrap(), finished);
}

#[test]
fn two_open_handles_preserve_both_pending_terminal_byte_reserves() {
    let policy = policy(100, 80_000);
    let (root, first, a) = fixture(&policy);
    let b = second_session(&first, &a);
    let path = root.path().join("authentication.sqlite");
    let second = open(&path, &policy);
    let pending_a = refresh(&a);
    let pending_b = refresh(&b);
    let admitted_a = first
        .refresh_pending(pending_a.clone(), 1, a.access_fingerprint)
        .unwrap();
    let admitted_b = second
        .refresh_pending(pending_b.clone(), 1, b.access_fingerprint)
        .unwrap();
    assert!(admitted_a.inserted && admitted_b.inserted);
    let mut refused = false;
    for turn in 0..64 {
        let handle = if turn % 2 == 0 { &first } else { &second };
        match handle.append_typed(initial(Uuid::new_v4())) {
            Ok(_) => assert!(
                handle.verify().unwrap().serialized_bytes + 2 * 32_768
                    <= policy.max_serialized_bytes,
                "another handle spent one of the two terminal byte reserves"
            ),
            Err(error) => {
                capacity(error);
                refused = true;
                break;
            }
        }
    }
    assert!(
        refused,
        "bounded appends must reach the protected byte boundary"
    );
    let held = first.verify().unwrap();
    assert_eq!(second.verify().unwrap(), held);
    assert!(
        held.records < policy.max_records,
        "this oracle must exhaust bytes, not records"
    );
    let terminal_a = failed_terminal(pending_a, admitted_a.receipt);
    let receipt_a = second
        .close_session(terminal_a.clone(), 1, a.access_fingerprint)
        .unwrap();
    let one_left = first.verify().unwrap();
    assert_eq!(one_left.records, held.records + 1);
    assert!(one_left.serialized_bytes + 32_768 <= policy.max_serialized_bytes);
    assert_eq!(
        first
            .close_session(terminal_a, 1, a.access_fingerprint)
            .unwrap(),
        receipt_a
    );
    assert_eq!(first.verify().unwrap(), one_left);
    // The unused allowance from A's exact terminal is now available, while B's
    // still-Pending terminal retains the whole absolute payload allowance.
    first.append_typed(initial(Uuid::new_v4())).unwrap();
    assert!(second.verify().unwrap().serialized_bytes + 32_768 <= policy.max_serialized_bytes);
    let terminal_b = failed_terminal(pending_b, admitted_b.receipt);
    let receipt_b = first
        .close_session(terminal_b.clone(), 1, b.access_fingerprint)
        .unwrap();
    let finished = first.verify().unwrap();
    assert_eq!(finished.records, held.records + 3);
    assert_eq!(
        second
            .close_session(terminal_b.clone(), 1, b.access_fingerprint)
            .unwrap(),
        receipt_b
    );
    assert_eq!(second.verify().unwrap(), finished);
    drop(first);
    drop(second);
    let reopened = open(&path, &policy);
    assert_eq!(reopened.verify().unwrap(), finished);
    assert_eq!(
        reopened
            .close_session(terminal_b, 1, b.access_fingerprint)
            .unwrap(),
        receipt_b
    );
    for binding in [&a, &b] {
        assert_eq!(
            reopened
                .session_current(binding.session_id)
                .unwrap()
                .unwrap()
                .state,
            AuthenticationSessionStateV1::Closed {
                reason: AuthenticationReasonV1::ProviderUnavailable
            }
        );
    }
    assert_eq!(reopened.verify().unwrap(), finished);
}
