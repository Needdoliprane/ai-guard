//! `egress_decision_*`: the backend-neutral decision and renewal rules.

use super::*;

const SESSION: &str = "00000000-0000-4000-8000-0000000000a1";
const NOW: i64 = 1_000_000;

fn policy(grantable: EgressGrantableV1, listed: &[&str]) -> EgressPolicySnapshot {
    EgressTenantPolicyV1 {
        version: 1,
        revision: 4,
        allow: vec!["api.anthropic.com:443".into(), "api.openai.com:443".into()],
        grantable,
        grantable_destinations: listed.iter().map(|raw| raw.to_string()).collect(),
        max_grant_seconds: 3600,
    }
    .snapshot()
    .unwrap()
}
fn authority(raw: &str) -> Authority {
    Authority::parse_connect(raw).unwrap()
}
fn grant(session: &str, raw: &str, expires: i64) -> EgressGrantV1 {
    EgressGrantV1 {
        version: 1,
        grant_id: IdV1::parse("00000000-0000-4000-8000-0000000000b1").unwrap(),
        session_id: session.into(),
        authority: raw.into(),
        not_before_ms: NOW - 1_000,
        expires_at_ms: expires,
        approver: "human:alice".into(),
        revoked_at_ms: None,
    }
}
fn outcome(decided: &Decided) -> (EgressOutcomeV1, EgressDecisionReasonV1) {
    (decided.outcome, decided.reason)
}

#[test]
fn egress_decision_policy_destinations_are_allowed_and_unknown_ones_follow_the_grantable_class() {
    let public = policy(EgressGrantableV1::PublicNames, &[]);
    let closed = policy(EgressGrantableV1::None, &[]);
    let eligible = WorkerFacts::Eligible;
    let allowed = decide(
        &public,
        &[],
        eligible,
        SESSION,
        &authority("api.openai.com:443"),
        NOW,
    );
    assert_eq!(
        outcome(&allowed),
        (
            EgressOutcomeV1::Allow,
            EgressDecisionReasonV1::AllowedByPolicy
        )
    );
    assert_eq!(allowed.remaining_ms, Some(24 * 3600 * 1000));
    let unknown = authority("unknown.example.test:443");
    assert_eq!(
        outcome(&decide(&public, &[], eligible, SESSION, &unknown, NOW)),
        (
            EgressOutcomeV1::Review,
            EgressDecisionReasonV1::RequiresApproval
        )
    );
    assert_eq!(
        outcome(&decide(&closed, &[], eligible, SESSION, &unknown, NOW)),
        (EgressOutcomeV1::Deny, EgressDecisionReasonV1::NotInPolicy)
    );
    // Same name, other port: not the allowed destination.
    assert_eq!(
        outcome(&decide(
            &closed,
            &[],
            eligible,
            SESSION,
            &authority("api.openai.com:22"),
            NOW
        )),
        (EgressOutcomeV1::Deny, EgressDecisionReasonV1::NotInPolicy)
    );
}

#[test]
fn egress_decision_a_live_grant_allows_only_its_session_destination_and_bounds_validity() {
    let public = policy(EgressGrantableV1::PublicNames, &[]);
    let target = authority("docs.example.test:443");
    let live = grant(SESSION, "docs.example.test:443", 1_300_000);
    let decided = decide(
        &public,
        &[live.clone()],
        WorkerFacts::Eligible,
        SESSION,
        &target,
        NOW,
    );
    assert_eq!(
        outcome(&decided),
        (
            EgressOutcomeV1::Allow,
            EgressDecisionReasonV1::AllowedByGrant
        )
    );
    assert_eq!(decided.grant_id, Some(live.grant_id.clone()));
    assert_eq!(decided.remaining_ms, Some(300_000));
    let other_session = grant(
        "00000000-0000-4000-8000-0000000000ff",
        "docs.example.test:443",
        1_300_000,
    );
    let mut revoked = live.clone();
    revoked.revoked_at_ms = Some(NOW - 1);
    let expired = grant(SESSION, "docs.example.test:443", NOW);
    for grants in [vec![other_session], vec![revoked], vec![expired]] {
        assert_eq!(
            outcome(&decide(
                &public,
                &grants,
                WorkerFacts::Eligible,
                SESSION,
                &target,
                NOW
            )),
            (
                EgressOutcomeV1::Review,
                EgressDecisionReasonV1::RequiresApproval
            )
        );
    }
    // A grant outside a listed grantable set never opens the destination.
    let listed = policy(EgressGrantableV1::Listed, &["mirror.example.test:443"]);
    assert_eq!(
        outcome(&decide(
            &listed,
            &[live],
            WorkerFacts::Eligible,
            SESSION,
            &target,
            NOW
        )),
        (EgressOutcomeV1::Deny, EgressDecisionReasonV1::NotInPolicy)
    );
}

#[test]
fn egress_decision_worker_binding_refusals_precede_policy() {
    let public = policy(EgressGrantableV1::PublicNames, &[]);
    for (facts, reason) in [
        (
            WorkerFacts::NotRunning,
            EgressDecisionReasonV1::WorkerNotRunning,
        ),
        (
            WorkerFacts::EgressNotLaunched,
            EgressDecisionReasonV1::EgressNotLaunched,
        ),
        (
            WorkerFacts::BindingMismatch,
            EgressDecisionReasonV1::BindingMismatch,
        ),
    ] {
        let decided = decide(
            &public,
            &[],
            facts,
            SESSION,
            &authority("api.openai.com:443"),
            NOW,
        );
        assert_eq!(outcome(&decided), (EgressOutcomeV1::Deny, reason));
    }
}

#[test]
fn egress_decision_renewal_closes_revoked_expired_or_no_longer_allowed_flows() {
    let public = policy(EgressGrantableV1::PublicNames, &[]);
    let target = authority("docs.example.test:443");
    let live = grant(SESSION, "docs.example.test:443", 1_300_000);
    let by_grant = decide(
        &public,
        &[live.clone()],
        WorkerFacts::Eligible,
        SESSION,
        &target,
        NOW,
    );
    assert_eq!(
        renewal(&public, &by_grant, Some(&live), &target, NOW + 1),
        None
    );
    let mut revoked = live.clone();
    revoked.revoked_at_ms = Some(NOW + 1);
    assert_eq!(
        renewal(&public, &by_grant, Some(&revoked), &target, NOW + 2),
        Some(EgressCloseReasonV1::Revoked)
    );
    assert_eq!(
        renewal(&public, &by_grant, Some(&live), &target, 1_300_000),
        Some(EgressCloseReasonV1::Expired)
    );
    assert_eq!(
        renewal(&public, &by_grant, None, &target, NOW),
        Some(EgressCloseReasonV1::Revoked)
    );
    let openai = authority("api.openai.com:443");
    let by_policy = decide(&public, &[], WorkerFacts::Eligible, SESSION, &openai, NOW);
    assert_eq!(renewal(&public, &by_policy, None, &openai, NOW), None);
    let narrowed = EgressTenantPolicyV1 {
        version: 1,
        revision: 5,
        allow: vec!["api.anthropic.com:443".into()],
        grantable: EgressGrantableV1::None,
        grantable_destinations: vec![],
        max_grant_seconds: 60,
    }
    .snapshot()
    .unwrap();
    assert_eq!(
        renewal(&narrowed, &by_policy, None, &openai, NOW),
        Some(EgressCloseReasonV1::Revoked)
    );
}

#[test]
fn egress_decision_operator_policy_is_closed_bounded_and_digest_bound() {
    let base = EgressTenantPolicyV1 {
        version: 1,
        revision: 1,
        allow: vec!["api.openai.com:443".into()],
        grantable: EgressGrantableV1::None,
        grantable_destinations: vec![],
        max_grant_seconds: 300,
    };
    let first = base.snapshot().unwrap();
    let mut changed = base.clone();
    changed.revision = 2;
    assert_ne!(first.digest, changed.snapshot().unwrap().digest);
    for broken in [
        EgressTenantPolicyV1 {
            version: 2,
            ..base.clone()
        },
        EgressTenantPolicyV1 {
            revision: 0,
            ..base.clone()
        },
        EgressTenantPolicyV1 {
            max_grant_seconds: 0,
            ..base.clone()
        },
        EgressTenantPolicyV1 {
            max_grant_seconds: 3601,
            ..base.clone()
        },
        EgressTenantPolicyV1 {
            allow: vec!["api.openai.com:443".into(), "API.openai.com:443".into()],
            ..base.clone()
        },
        EgressTenantPolicyV1 {
            allow: vec!["https://api.openai.com/".into()],
            ..base.clone()
        },
        EgressTenantPolicyV1 {
            allow: vec!["10.0.0.1:443".into()],
            ..base.clone()
        },
        EgressTenantPolicyV1 {
            allow: vec!["*.openai.com:443".into()],
            ..base.clone()
        },
        EgressTenantPolicyV1 {
            grantable_destinations: vec!["x.example.test:443".into()],
            ..base.clone()
        },
    ] {
        assert!(broken.snapshot().is_err(), "{broken:?}");
    }
    let wide = EgressTenantPolicyV1 {
        allow: (0..129)
            .map(|i| format!("host{i}.example.test:443"))
            .collect(),
        ..base.clone()
    };
    assert!(wide.snapshot().is_err());
    assert!(serde_json::from_value::<EgressTenantPolicyV1>(serde_json::json!({
        "version": 1, "revision": 1, "allow": [], "grantable": "none", "max_grant_seconds": 60, "extra": true
    }))
    .is_err());
}
