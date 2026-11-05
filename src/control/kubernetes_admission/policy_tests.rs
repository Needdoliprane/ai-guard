use super::{KubernetesHeldCreateApprovalV1, KubernetesHeldCreatePolicyV1};
use crate::control::kubernetes_admission::{KUBERNETES_HELD_PROFILE_V1, KubernetesCreateIntentV1};
use crate::runtime::kubernetes::{KubernetesAuthorityV1, KubernetesPreflightResourcesV1};
use serde_json::json;
use uuid::Uuid;

const POLICY_HASH_GOLDEN: &str = "016a863fd235ae19ecad4d05032b01875202847ebbffd118dcf6981329bb6134";

fn approval(
    authority_fingerprint: impl Into<String>,
    image: impl Into<String>,
    profile: impl Into<String>,
) -> KubernetesHeldCreateApprovalV1 {
    KubernetesHeldCreateApprovalV1 {
        authority_fingerprint: authority_fingerprint.into(),
        image: image.into(),
        profile: profile.into(),
    }
}

fn immutable_image(name: &str, digest: char) -> String {
    format!("{name}@sha256:{}", digest.to_string().repeat(64))
}

fn valid_policy() -> KubernetesHeldCreatePolicyV1 {
    KubernetesHeldCreatePolicyV1 {
        version: 1,
        decision_ttl_seconds: 30,
        approvals: vec![approval(
            "a".repeat(64),
            immutable_image("worker", 'b'),
            KUBERNETES_HELD_PROFILE_V1,
        )],
    }
}

fn valid_intent() -> KubernetesCreateIntentV1 {
    let authority = KubernetesAuthorityV1::new(
        Uuid::from_u128(1),
        "https://127.0.0.1:6443",
        "11111111-1111-4111-8111-111111111111",
        "tenant-a",
        "22222222-2222-4222-8222-222222222222",
        &"c".repeat(64),
    )
    .unwrap();
    KubernetesCreateIntentV1 {
        version: 1,
        create_id: Uuid::from_u128(3).to_string(),
        session_id: Uuid::from_u128(4).to_string(),
        control_namespace_uuid: Uuid::from_u128(5).to_string(),
        broker_id: Uuid::from_u128(6).to_string(),
        incarnation_id: Uuid::from_u128(7).to_string(),
        authority,
        profile: KUBERNETES_HELD_PROFILE_V1.into(),
        image: immutable_image("worker", 'd'),
        resources: KubernetesPreflightResourcesV1 {
            cpu_millis: 500,
            memory_mib: 128,
            workspace_mib: 8,
            temporary_mib: 8,
            deadline_seconds: 60,
        },
    }
}

#[test]
fn policy_hash_uses_the_canonical_approval_order_and_independent_golden() {
    let entries = [
        approval(
            "c".repeat(64),
            immutable_image("agent-c", '3'),
            KUBERNETES_HELD_PROFILE_V1,
        ),
        approval(
            "a".repeat(64),
            immutable_image("agent-z", '9'),
            KUBERNETES_HELD_PROFILE_V1,
        ),
        approval(
            "a".repeat(64),
            immutable_image("agent-a", '1'),
            KUBERNETES_HELD_PROFILE_V1,
        ),
        approval(
            "b".repeat(64),
            immutable_image("agent-b", '2'),
            KUBERNETES_HELD_PROFILE_V1,
        ),
    ];
    let policy = KubernetesHeldCreatePolicyV1 {
        version: 1,
        decision_ttl_seconds: 17,
        approvals: entries.into(),
    };
    let sorted = policy.canonicalized().unwrap();
    assert_eq!(
        sorted.approvals,
        vec![
            approval(
                "a".repeat(64),
                immutable_image("agent-a", '1'),
                KUBERNETES_HELD_PROFILE_V1,
            ),
            approval(
                "a".repeat(64),
                immutable_image("agent-z", '9'),
                KUBERNETES_HELD_PROFILE_V1,
            ),
            approval(
                "b".repeat(64),
                immutable_image("agent-b", '2'),
                KUBERNETES_HELD_PROFILE_V1,
            ),
            approval(
                "c".repeat(64),
                immutable_image("agent-c", '3'),
                KUBERNETES_HELD_PROFILE_V1,
            ),
        ],
        "canonical ordering is tuple order by authority, image, and profile"
    );
    assert_eq!(
        policy.policy_hash().unwrap(),
        POLICY_HASH_GOLDEN,
        "hash of the independently frozen canonical policy preimage"
    );
    let mut reversed = policy.clone();
    reversed.approvals.reverse();
    assert_eq!(
        reversed.policy_hash().unwrap(),
        policy.policy_hash().unwrap()
    );
}

#[test]
fn policy_validation_refuses_invalid_bounds_duplicates_and_unbounded_documents() {
    assert!(valid_policy().validate().is_ok());
    for ttl in [1, 300] {
        let mut boundary = valid_policy();
        boundary.decision_ttl_seconds = ttl;
        assert!(
            boundary.validate().is_ok(),
            "TTL {ttl} is within the contract"
        );
    }

    let mut invalid = valid_policy();
    invalid.version = 2;
    assert!(invalid.validate().is_err());

    let mut invalid = valid_policy();
    invalid.decision_ttl_seconds = 0;
    assert!(invalid.validate().is_err());
    let mut invalid = valid_policy();
    invalid.decision_ttl_seconds = 301;
    assert!(invalid.validate().is_err());

    let mut invalid = valid_policy();
    invalid.approvals.clear();
    assert!(
        invalid.validate().is_err(),
        "the policy requires at least one approval"
    );
    let mut invalid = valid_policy();
    invalid.approvals = (0..65)
        .map(|index| {
            approval(
                format!("{index:064x}"),
                immutable_image("worker", 'b'),
                KUBERNETES_HELD_PROFILE_V1,
            )
        })
        .collect();
    assert!(invalid.validate().is_err());

    let duplicate = approval(
        "a".repeat(64),
        immutable_image("worker", 'b'),
        KUBERNETES_HELD_PROFILE_V1,
    );
    let mut invalid = valid_policy();
    invalid.approvals.push(duplicate);
    assert!(invalid.validate().is_err());

    for bad in [
        approval(
            "A".repeat(64),
            immutable_image("worker", 'b'),
            KUBERNETES_HELD_PROFILE_V1,
        ),
        approval(
            "a".repeat(63),
            immutable_image("worker", 'b'),
            KUBERNETES_HELD_PROFILE_V1,
        ),
        approval(
            "g".repeat(64),
            immutable_image("worker", 'b'),
            KUBERNETES_HELD_PROFILE_V1,
        ),
        approval("a".repeat(64), "worker:latest", KUBERNETES_HELD_PROFILE_V1),
        approval(
            "a".repeat(64),
            immutable_image("worker", 'b'),
            "other_profile",
        ),
        approval("*", "*", "*"),
    ] {
        let mut invalid = valid_policy();
        invalid.approvals = vec![bad];
        assert!(invalid.validate().is_err());
    }

    let compact_64 = KubernetesHeldCreatePolicyV1 {
        version: 1,
        decision_ttl_seconds: 30,
        approvals: (1..=64)
            .map(|index| {
                approval(
                    format!("{index:064x}"),
                    immutable_image("worker", 'b'),
                    KUBERNETES_HELD_PROFILE_V1,
                )
            })
            .collect(),
    };
    assert!(serde_json::to_vec(&compact_64).unwrap().len() <= 16 * 1024);
    assert!(compact_64.validate().is_ok());

    let long_name = format!(
        "registry.example/{}/{}/{}",
        "a".repeat(63),
        "b".repeat(63),
        "c".repeat(32)
    );
    let long_image = immutable_image(&long_name, 'd');
    assert_eq!(long_image.len(), 249);
    let long_image_policy = KubernetesHeldCreatePolicyV1 {
        version: 1,
        decision_ttl_seconds: 30,
        approvals: vec![approval(
            "a".repeat(64),
            long_image.clone(),
            KUBERNETES_HELD_PROFILE_V1,
        )],
    };
    assert!(long_image_policy.validate().is_ok());
    let oversized = KubernetesHeldCreatePolicyV1 {
        version: 1,
        decision_ttl_seconds: 30,
        approvals: (1..=64)
            .map(|index| {
                approval(
                    format!("{index:064x}"),
                    long_image.clone(),
                    KUBERNETES_HELD_PROFILE_V1,
                )
            })
            .collect(),
    };
    assert!(serde_json::to_vec(&oversized).unwrap().len() > 16 * 1024);
    assert!(oversized.validate().is_err());

    let mut wire = serde_json::to_value(valid_policy()).unwrap();
    wire["unknown_field"] = json!(true);
    assert!(serde_json::from_value::<KubernetesHeldCreatePolicyV1>(wire).is_err());
}

#[test]
fn policy_approves_only_the_exact_authority_image_and_profile_tuple() {
    let intent = valid_intent();
    let exact = approval(
        intent.authority.fingerprint(),
        intent.image.clone(),
        intent.profile.clone(),
    );
    let policy = KubernetesHeldCreatePolicyV1 {
        version: 1,
        decision_ttl_seconds: 30,
        approvals: vec![exact],
    };
    assert!(policy.approves(&intent).unwrap());

    for neighbor in [
        approval("e".repeat(64), intent.image.clone(), intent.profile.clone()),
        approval(
            intent.authority.fingerprint(),
            immutable_image("worker", 'e'),
            intent.profile.clone(),
        ),
        approval(
            intent.authority.fingerprint(),
            immutable_image("different-worker", 'd'),
            intent.profile.clone(),
        ),
    ] {
        let policy = KubernetesHeldCreatePolicyV1 {
            version: 1,
            decision_ttl_seconds: 30,
            approvals: vec![neighbor],
        };
        assert!(!policy.approves(&intent).unwrap());
    }
}
