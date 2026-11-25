use super::*;

fn marker() -> EgressReleaseIntentV1 {
    let target = EgressReleaseTargetV1 {
        tenant_id: "tenant-a".into(),
        operation_id: IdV1::random(),
        session_id: IdV1::random(),
        worker_id: IdV1::random(),
        attempt_id: IdV1::random(),
        authority_id: IdV1::random(),
        authority_fingerprint: DigestV1::of(b"authority"),
        generation: 7,
        container_id: DigestV1::of(b"container"),
        started_at: DateTime::parse_from_rfc3339("2026-11-25T10:15:30Z")
            .unwrap()
            .to_utc(),
        restart_count: 0,
        configuration_fingerprint: DigestV1::of(b"confinement"),
        configuration_fingerprint_version: 3,
    };
    EgressReleaseIntentV1 {
        version: 1,
        opening: EgressChannelOpeningReceiptV1 {
            version: 1,
            observation_id: IdV1::random(),
            binding: target.binding(IdV1::random(), 2).unwrap(),
            disposition: EgressChannelOpeningDispositionV1::Fresh,
            receipt: crate::control::egress_decision::EgressReceiptV1 {
                audit_seq: 10,
                audit_hash: "a".repeat(64),
            },
        },
        target,
        worker_revision: 6,
    }
}

#[test]
fn release_intent_requires_fresh_opening_for_exact_runtime_target() {
    let exact = marker();
    let wire = canonical(&exact).unwrap();
    let restored: EgressReleaseIntentV1 = serde_json::from_slice(&wire).unwrap();
    restored.validate_shape().unwrap();
    assert_eq!(restored, exact);

    let mut replay = exact.clone();
    replay.opening.disposition = EgressChannelOpeningDispositionV1::Replayed;
    assert!(replay.validate_shape().is_err());

    for target in [
        EgressReleaseTargetV1 {
            tenant_id: "tenant-b".into(),
            ..exact.target.clone()
        },
        EgressReleaseTargetV1 {
            authority_fingerprint: DigestV1::of(b"other authority"),
            ..exact.target.clone()
        },
        EgressReleaseTargetV1 {
            generation: 8,
            ..exact.target.clone()
        },
        EgressReleaseTargetV1 {
            container_id: DigestV1::of(b"other container"),
            ..exact.target.clone()
        },
        EgressReleaseTargetV1 {
            started_at: exact.target.started_at + chrono::Duration::seconds(1),
            ..exact.target.clone()
        },
        EgressReleaseTargetV1 {
            restart_count: 1,
            ..exact.target.clone()
        },
        EgressReleaseTargetV1 {
            configuration_fingerprint: DigestV1::of(b"weaker confinement"),
            ..exact.target.clone()
        },
    ] {
        let variant = EgressReleaseIntentV1 {
            target,
            ..exact.clone()
        };
        assert!(variant.validate_shape().is_err());
    }

    let mut different_epoch = exact;
    different_epoch.opening.binding.epoch = 3;
    assert!(different_epoch.validate_shape().is_err());
}
