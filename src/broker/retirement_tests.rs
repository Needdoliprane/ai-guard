//! D5-U record regressions; execution status belongs to the validation report.
//! These fixtures are durable data, never live revocation/join/authentication.
use super::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const INSTALLATION: &str = "11111111-1111-4111-8111-111111111111";
const ROLE: &str = "22222222-2222-4222-8222-222222222222";
const INIT: &str = "33333333-3333-4333-8333-333333333333";
const EPOCH: &str = "44444444-4444-4444-8444-444444444444";
const ACTOR: &str = "55555555-5555-4555-8555-555555555555";
const GENESIS: &str = r#"{"init_id":"33333333-3333-4333-8333-333333333333","installation_id":"11111111-1111-4111-8111-111111111111","native_binding":[1,2,3,4,5,6],"role_id":"22222222-2222-4222-8222-222222222222","version":1}"#;
const GENESIS_HASH: &str = "3d5303beb8bbe092bbd79a22ab4e927da8bd27432ecc5d66cdd5c613ec3f64e1";
// Independent wire fixtures, assembled outside production and SHA256 checked
// with the literal NUL-terminated domains. [1..6] is format data, not FS proof.
const ENTRY_CLOSING: &str = r#"{"accepted":{"audit_ack":{"audit_seq":1,"event_id":"33333333-3333-4333-8333-333333333333","input_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","version":1},"closing_nonce":"22222222-2222-4222-8222-222222222222","evidence":{"identity_hash":"6d611df67644e569e31d218634b7afbfb34e57185a04dfb14cab9d4354ec106d","original_actor_nonce":"55555555-5555-4555-8555-555555555555","prepare_revocation_nonce":"44444444-4444-4444-8444-444444444444","version":1},"identity":{"arm":{"attempt_id":"33333333-3333-4333-8333-333333333333","authority_binding_hash":"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff","clock":{"boot_id":"44444444-4444-4444-8444-444444444444","continuous_ns_decimal":"1","method":"darwin_mach_continuous_v1","observed_utc":null,"version":1},"configuration_fingerprint":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","configuration_fingerprint_version":2,"deadline_ns_decimal":"60000000001","expected_revision":1,"inspection":{"authority_id":"44444444-4444-4444-8444-444444444444","container_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","cpu_millis":1000,"image_id":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","memory_bytes":1073741824,"network":"none","pids_limit":256,"restart_policy":"no","runtime_kind":"docker","security_preset":"fixed_sandbox_v2","spec_hash":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","stable_name":"retirement-fixture","volume_name":"retirement-fixture-work"},"intent_hash":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","operation_id":"55555555-5555-4555-8555-555555555555","subject":{"kind":"worker"},"timeout_seconds":60,"version":1,"worker_id":"22222222-2222-4222-8222-222222222222"},"arm_receipt":{"arm_id":"55555555-5555-4555-8555-555555555555","attempt_id":"33333333-3333-4333-8333-333333333333","audit_hash":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","audit_seq":1,"control_namespace_uuid":"22222222-2222-4222-8222-222222222222","input_hash":"9259ac5a825c489bc10fdb81bd648870ed0fababc14a5343826659ee9542f163","version":1,"worker_id":"22222222-2222-4222-8222-222222222222"},"generation":0,"intent_hash":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","job_operation_id":"33333333-3333-4333-8333-333333333333","original_epoch":"44444444-4444-4444-8444-444444444444","reason":"unused_prepare_revoked","registration_hash":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","retirement_id":"11111111-1111-4111-8111-111111111111","scope":{"control_namespace_uuid":"22222222-2222-4222-8222-222222222222","gateway_origin":"http://127.0.0.1:8787","installation_id":"11111111-1111-4111-8111-111111111111","profile_id":"retirement-test","tenant_id":"tenant-retirement","version":1},"version":1},"version":1},"expected_tombstone_hash":"7b11437d08c20dc48f1600d931d4a950bced7186c5b5e78d3ee301fc4b89dd06","phase":{"kind":"closing"},"version":1}"#;
const ENTRY_CLOSING_HASH: &str = "f6b6da40672303ee7f0de1922dd7bd4b93520e79a4f96d2242bdb45978cce406";
const DELTA_BEGIN: &str = r#"{"arm_id":"55555555-5555-4555-8555-555555555555","expected_entry_hash":null,"ledger":{"genesis_hash":"3d5303beb8bbe092bbd79a22ab4e927da8bd27432ecc5d66cdd5c613ec3f64e1","init_id":"33333333-3333-4333-8333-333333333333","installation_id":"11111111-1111-4111-8111-111111111111","native_binding":[1,2,3,4,5,6],"role_id":"22222222-2222-4222-8222-222222222222","version":1},"previous_hash":"3d5303beb8bbe092bbd79a22ab4e927da8bd27432ecc5d66cdd5c613ec3f64e1","revision":1,"transition":{"entry":{"accepted":{"audit_ack":{"audit_seq":1,"event_id":"33333333-3333-4333-8333-333333333333","input_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","version":1},"closing_nonce":"22222222-2222-4222-8222-222222222222","evidence":{"identity_hash":"6d611df67644e569e31d218634b7afbfb34e57185a04dfb14cab9d4354ec106d","original_actor_nonce":"55555555-5555-4555-8555-555555555555","prepare_revocation_nonce":"44444444-4444-4444-8444-444444444444","version":1},"identity":{"arm":{"attempt_id":"33333333-3333-4333-8333-333333333333","authority_binding_hash":"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff","clock":{"boot_id":"44444444-4444-4444-8444-444444444444","continuous_ns_decimal":"1","method":"darwin_mach_continuous_v1","observed_utc":null,"version":1},"configuration_fingerprint":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","configuration_fingerprint_version":2,"deadline_ns_decimal":"60000000001","expected_revision":1,"inspection":{"authority_id":"44444444-4444-4444-8444-444444444444","container_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","cpu_millis":1000,"image_id":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","memory_bytes":1073741824,"network":"none","pids_limit":256,"restart_policy":"no","runtime_kind":"docker","security_preset":"fixed_sandbox_v2","spec_hash":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","stable_name":"retirement-fixture","volume_name":"retirement-fixture-work"},"intent_hash":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","operation_id":"55555555-5555-4555-8555-555555555555","subject":{"kind":"worker"},"timeout_seconds":60,"version":1,"worker_id":"22222222-2222-4222-8222-222222222222"},"arm_receipt":{"arm_id":"55555555-5555-4555-8555-555555555555","attempt_id":"33333333-3333-4333-8333-333333333333","audit_hash":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","audit_seq":1,"control_namespace_uuid":"22222222-2222-4222-8222-222222222222","input_hash":"9259ac5a825c489bc10fdb81bd648870ed0fababc14a5343826659ee9542f163","version":1,"worker_id":"22222222-2222-4222-8222-222222222222"},"generation":0,"intent_hash":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","job_operation_id":"33333333-3333-4333-8333-333333333333","original_epoch":"44444444-4444-4444-8444-444444444444","reason":"unused_prepare_revoked","registration_hash":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","retirement_id":"11111111-1111-4111-8111-111111111111","scope":{"control_namespace_uuid":"22222222-2222-4222-8222-222222222222","gateway_origin":"http://127.0.0.1:8787","installation_id":"11111111-1111-4111-8111-111111111111","profile_id":"retirement-test","tenant_id":"tenant-retirement","version":1},"version":1},"version":1},"expected_tombstone_hash":"7b11437d08c20dc48f1600d931d4a950bced7186c5b5e78d3ee301fc4b89dd06","phase":{"kind":"closing"},"version":1},"kind":"begin_closing"},"version":1}"#;
const DELTA_BEGIN_HASH: &str = "5729571441395a789ee67ccf3554eedd9befe3101660a1d96b4bccf3626fb029";

fn literal_digest(domain: &[u8], bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(bytes);
    hex::encode(hash.finalize())
}

fn ledger() -> Value {
    json!({"version":1,"installation_id":INSTALLATION,"role_id":ROLE,
        "init_id":INIT,"native_binding":[1,2,3,4,5,6],"genesis_hash":GENESIS_HASH})
}

fn joined() -> Value {
    json!({"version":1,"identity_hash":"a".repeat(64),
        "closing_hash":"b".repeat(64),"tombstone_hash":"c".repeat(64),
        "original_epoch":EPOCH,"original_actor_nonce":ACTOR})
}

fn identity() -> Value {
    // Existing Arm validation/input hashing is the upstream contract, not a
    // retirement oracle. This fixture cannot create a live original actor.
    let arm = json!({"version":1,"operation_id":ACTOR,"worker_id":ROLE,
        "attempt_id":INIT,"expected_revision":1,"intent_hash":"e".repeat(64),
        "subject":{"kind":"worker"},"inspection":{
            "authority_id":EPOCH,"runtime_kind":"docker","container_id":"a".repeat(64),
            "stable_name":"retirement-fixture","volume_name":"retirement-fixture-work",
            "image_id":format!("sha256:{}", "b".repeat(64)),"spec_hash":"d".repeat(64),
            "cpu_millis":1000,"memory_bytes":1073741824_u64,"pids_limit":256,
            "network":"none","security_preset":"fixed_sandbox_v2","restart_policy":"no"},
        "authority_binding_hash":"f".repeat(64),"configuration_fingerprint_version":2,
        "configuration_fingerprint":"c".repeat(64),"clock":{
            "version":1,"method":"darwin_mach_continuous_v1","boot_id":EPOCH,
            "continuous_ns_decimal":"1","observed_utc":null},
        "timeout_seconds":60,"deadline_ns_decimal":"60000000001"});
    let typed_arm: crate::control::ArmV1 = serde_json::from_value(arm.clone()).unwrap();
    let receipt = json!({"version":1,"arm_id":ACTOR,"worker_id":ROLE,"attempt_id":INIT,
        "input_hash":typed_arm.input_hash().unwrap(),"control_namespace_uuid":ROLE,
        "audit_seq":1,"audit_hash":"b".repeat(64)});
    json!({"version":1,"scope":{"version":1,"installation_id":INSTALLATION,
        "tenant_id":"tenant-retirement","gateway_origin":"http://127.0.0.1:8787",
        "control_namespace_uuid":ROLE,"profile_id":"retirement-test"},
        "job_operation_id":INIT,"intent_hash":"e".repeat(64),"arm":arm,
        "arm_receipt":receipt,"registration_hash":"d".repeat(64),
        "original_epoch":EPOCH,"generation":0,"retirement_id":INSTALLATION,
        "reason":"unused_prepare_revoked"})
}

fn rejects<T: DeserializeOwned>(value: Value) {
    rejects_at::<T>(value, "record validation");
}

fn rejects_at<T: DeserializeOwned>(value: Value, label: &str) {
    assert!(
        serde_json::from_value::<T>(value).is_err(),
        "{} unexpectedly accepted {label}",
        std::any::type_name::<T>()
    );
}

fn rejects_unknown_and_version<T: DeserializeOwned>(value: Value) {
    let mut unknown = value.clone();
    unknown["unrecognized"] = json!(true);
    rejects::<T>(unknown);
    for version in [json!(0), json!(2), json!(-1), json!("1")] {
        let mut invalid = value.clone();
        invalid["version"] = version;
        rejects::<T>(invalid);
    }
}

// A missing NUL, wrong domain, struct-order JSON, or omitted native binding
// changes the persisted commitment and must fail this independent golden.
#[test]
fn genesis_commitment_matches_literal_canonical_vector() {
    let record: GenesisCoreV1 = serde_json::from_str(GENESIS).unwrap();
    assert_eq!(record.canonical_bytes().unwrap(), GENESIS.as_bytes());
    assert_eq!(record.digest().unwrap().as_str(), GENESIS_HASH);
    assert_eq!(
        literal_digest(
            b"ai-guard:enforcer-retirement-genesis:v1\0",
            GENESIS.as_bytes()
        ),
        GENESIS_HASH
    );
    let reordered = r#"{"version":1,"role_id":"22222222-2222-4222-8222-222222222222","native_binding":[1,2,3,4,5,6],"installation_id":"11111111-1111-4111-8111-111111111111","init_id":"33333333-3333-4333-8333-333333333333"}"#;
    let same: GenesisCoreV1 = serde_json::from_str(reordered).unwrap();
    assert_eq!(same.digest().unwrap().as_str(), GENESIS_HASH);
}

#[test]
fn canonical_wire_parser_rejects_alternate_encodings_without_normalizing() {
    let valid = GenesisCoreV1::from_canonical_bytes(GENESIS.as_bytes()).unwrap();
    assert_eq!(valid.digest().unwrap().as_str(), GENESIS_HASH);
    for raw in [
        format!(" {GENESIS}"),
        format!("{GENESIS}\n"),
        GENESIS.replace(":1", ":1.0"),
    ] {
        assert!(GenesisCoreV1::from_canonical_bytes(raw.as_bytes()).is_err());
    }
    let noncanonical = GENESIS.replace(INSTALLATION, "11111111111141118111111111111111");
    assert!(GenesisCoreV1::from_canonical_bytes(noncanonical.as_bytes()).is_err());
    let duplicate = GENESIS.replacen("{", "{\"version\":1,", 1);
    assert!(GenesisCoreV1::from_canonical_bytes(duplicate.as_bytes()).is_err());
}

#[test]
fn digest_rejects_truncated_uppercase_and_nonhex_commitments() {
    for value in [
        "".into(),
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
        format!("0x{}", "a".repeat(64)),
    ] {
        rejects::<RetirementDigestV1>(json!(value));
    }
    let valid: RetirementDigestV1 = serde_json::from_value(json!("a".repeat(64))).unwrap();
    assert_eq!(valid.as_str(), "a".repeat(64));
}

#[test]
fn genesis_rejects_noncanonical_nil_and_malformed_native_identity() {
    let fixture: Value = serde_json::from_str(GENESIS).unwrap();
    rejects_unknown_and_version::<GenesisCoreV1>(fixture.clone());
    for field in ["installation_id", "role_id", "init_id"] {
        for id in [
            "00000000-0000-0000-0000-000000000000",
            "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA",
            "11111111111141118111111111111111",
            "urn:uuid:11111111-1111-4111-8111-111111111111",
        ] {
            let mut invalid = fixture.clone();
            invalid[field] = json!(id);
            rejects::<GenesisCoreV1>(invalid);
        }
    }
    for binding in [
        json!([1, 2, 3, 4, 5]),
        json!([1, 2, 3, 4, 5, 6, 7]),
        json!([1, 2, 3, 4, 5, -1]),
    ] {
        let mut invalid = fixture.clone();
        invalid["native_binding"] = binding;
        rejects::<GenesisCoreV1>(invalid);
    }
}

// Uses literal domains, independent of any production constant/helper.
fn check_domain<T: Serialize>(record: &T, bytes: &[u8], actual: &str, domain: &[u8]) {
    assert_eq!(
        serde_json::to_value(record).unwrap(),
        serde_json::from_slice::<Value>(bytes).unwrap()
    );
    assert_eq!(actual, literal_digest(domain, bytes));
}

#[test]
fn seal_floor_evidence_and_join_have_distinct_literal_domains() {
    let seal: RoleSealV2 = serde_json::from_value(json!({"version":2,"ledger":ledger()})).unwrap();
    check_domain(
        &seal,
        &seal.canonical_bytes().unwrap(),
        seal.digest().unwrap().as_str(),
        b"ai-guard:enforcer-role-seal:v2\0",
    );
    let floor: LedgerFloorV1 = serde_json::from_value(json!({"version":1,"ledger":ledger(),"revision":0,"delta_hash":null,"previous_floor_hash":null})).unwrap();
    check_domain(
        &floor,
        &floor.canonical_bytes().unwrap(),
        floor.digest().unwrap().as_str(),
        b"ai-guard:enforcer-ledger-floor:v1\0",
    );
    let evidence: RetirementEvidenceV1 = serde_json::from_value(json!({"version":1,"identity_hash":"a".repeat(64),"original_actor_nonce":ACTOR,"prepare_revocation_nonce":EPOCH})).unwrap();
    check_domain(
        &evidence,
        &evidence.canonical_bytes().unwrap(),
        evidence.digest().unwrap().as_str(),
        b"ai-guard:broker-retirement-evidence:v1\0",
    );
    let joined: JoinedQuiescenceV1 = serde_json::from_value(joined()).unwrap();
    check_domain(
        &joined,
        &joined.canonical_bytes().unwrap(),
        joined.digest().unwrap().as_str(),
        b"ai-guard:enforcer-joined-quiescence:v1\0",
    );
}

#[test]
fn floor_zero_and_successor_require_exact_predecessor_shape() {
    let zero = json!({"version":1,"ledger":ledger(),"revision":0,"delta_hash":null,"previous_floor_hash":null});
    let next = json!({"version":1,"ledger":ledger(),"revision":1,"delta_hash":"a".repeat(64),"previous_floor_hash":"b".repeat(64)});
    assert!(serde_json::from_value::<LedgerFloorV1>(zero.clone()).is_ok());
    assert!(serde_json::from_value::<LedgerFloorV1>(next.clone()).is_ok());
    rejects_unknown_and_version::<LedgerFloorV1>(zero.clone());
    for field in ["delta_hash", "previous_floor_hash"] {
        let mut invalid_zero = zero.clone();
        invalid_zero[field] = json!("a".repeat(64));
        rejects::<LedgerFloorV1>(invalid_zero);
        let mut invalid_next = next.clone();
        invalid_next[field] = Value::Null;
        rejects::<LedgerFloorV1>(invalid_next);
        let mut missing = zero.clone();
        missing.as_object_mut().unwrap().remove(field);
        rejects::<LedgerFloorV1>(missing);
    }
}

#[test]
fn ledger_identity_rejects_genesis_substitution_and_nested_extensions() {
    let valid = ledger();
    assert!(serde_json::from_value::<LedgerIdentityV1>(valid.clone()).is_ok());
    rejects_unknown_and_version::<LedgerIdentityV1>(valid.clone());
    for field in [
        "installation_id",
        "role_id",
        "init_id",
        "genesis_hash",
        "native_binding",
    ] {
        let mut bad = valid.clone();
        bad[field] = match field {
            "genesis_hash" => json!("d".repeat(64)),
            "native_binding" => json!([1, 2, 3, 4, 5, 7]),
            _ => json!(EPOCH),
        };
        rejects::<LedgerIdentityV1>(bad);
    }
    let mut floor = json!({"version":1,"ledger":valid,"revision":0,"delta_hash":null,"previous_floor_hash":null});
    floor["ledger"]["unrecognized"] = json!(true);
    rejects::<LedgerFloorV1>(floor);
}

#[test]
fn audit_ack_requires_positive_sequence_and_closed_shape() {
    let ack = json!({"version":1,"event_id":EPOCH,"input_hash":"a".repeat(64),"audit_seq":1});
    assert!(serde_json::from_value::<RetirementAuditAckV1>(ack.clone()).is_ok());
    rejects_unknown_and_version::<RetirementAuditAckV1>(ack.clone());
    for sequence in [json!(0), json!(-1), json!(null), json!("1")] {
        let mut invalid = ack.clone();
        invalid["audit_seq"] = sequence;
        rejects::<RetirementAuditAckV1>(invalid);
    }
}

#[test]
fn finalization_rejects_cross_bound_join_and_future_head_cycles() {
    let core = json!({"version":1,"identity_hash":"a".repeat(64),"closing_hash":"b".repeat(64),"tombstone_hash":"c".repeat(64),"joined":joined()});
    let valid: FinalizationCoreV1 = serde_json::from_value(core.clone()).unwrap();
    check_domain(
        &valid,
        &valid.canonical_bytes().unwrap(),
        valid.digest().unwrap().as_str(),
        b"ai-guard:enforcer-finalization-core:v1\0",
    );
    rejects_unknown_and_version::<FinalizationCoreV1>(core.clone());
    for field in ["identity_hash", "closing_hash", "tombstone_hash"] {
        let mut invalid = core.clone();
        invalid["joined"][field] = json!("d".repeat(64));
        rejects::<FinalizationCoreV1>(invalid);
    }
    for field in ["head", "head_hash", "receipt", "ledger_revision"] {
        let mut cyclic = core.clone();
        cyclic[field] = json!("d".repeat(64));
        rejects::<FinalizationCoreV1>(cyclic);
    }
    let receipt = json!({"version":1,"ledger":ledger(),"retirement_id":EPOCH,"identity_hash":"a".repeat(64),"finalization":core});
    let valid: RetiredReceiptV1 = serde_json::from_value(receipt.clone()).unwrap();
    check_domain(
        &valid,
        &valid.canonical_bytes().unwrap(),
        valid.digest().unwrap().as_str(),
        b"ai-guard:enforcer-retired-receipt:v1\0",
    );
    let mut cross_bound = receipt.clone();
    cross_bound["identity_hash"] = json!("d".repeat(64));
    rejects::<RetiredReceiptV1>(cross_bound);
    let mut cyclic = receipt;
    cyclic["head_hash"] = json!("d".repeat(64));
    rejects::<RetiredReceiptV1>(cyclic);
}

#[test]
fn tombstone_binds_complete_ledger_without_embedding_closing_record() {
    let value = json!({"version":1,"ledger":ledger(),"identity_hash":"a".repeat(64),"closing_hash":"b".repeat(64)});
    let record: RetiredTombstoneV1 = serde_json::from_value(value.clone()).unwrap();
    check_domain(
        &record,
        &record.canonical_bytes().unwrap(),
        record.digest().unwrap().as_str(),
        b"ai-guard:enforcer-retired-tombstone:v1\0",
    );
    rejects_unknown_and_version::<RetiredTombstoneV1>(value.clone());
    let mut invalid = value;
    invalid["closing"] = json!({"future_tombstone":"c".repeat(64)});
    rejects::<RetiredTombstoneV1>(invalid);
}

#[test]
fn identity_rejects_cross_bound_arm_receipt_intent_and_namespace() {
    let fixture = identity();
    let valid: RetirementIdentityV1 = serde_json::from_value(fixture.clone()).unwrap();
    check_domain(
        &valid,
        &valid.canonical_bytes().unwrap(),
        valid.digest().unwrap().as_str(),
        b"ai-guard:broker-retirement-identity:v1\0",
    );
    rejects_unknown_and_version::<RetirementIdentityV1>(fixture.clone());
    for pointer in [
        "/arm_receipt/arm_id",
        "/arm_receipt/worker_id",
        "/arm_receipt/attempt_id",
        "/arm_receipt/control_namespace_uuid",
    ] {
        let mut bad = fixture.clone();
        *bad.pointer_mut(pointer).unwrap() = json!(EPOCH);
        rejects::<RetirementIdentityV1>(bad);
    }
    for pointer in ["/intent_hash", "/arm_receipt/input_hash"] {
        let mut bad = fixture.clone();
        *bad.pointer_mut(pointer).unwrap() = json!("a".repeat(64));
        rejects::<RetirementIdentityV1>(bad);
    }
    let mut unknown_reason = fixture;
    unknown_reason["reason"] = json!("normal_completion");
    rejects::<RetirementIdentityV1>(unknown_reason);
}

#[test]
fn identity_and_closing_reject_nested_extensions_and_future_tombstone() {
    let fixture = identity();
    for pointer in [
        "/scope",
        "/arm",
        "/arm/subject",
        "/arm/inspection",
        "/arm/clock",
        "/arm_receipt",
    ] {
        let mut bad = fixture.clone();
        bad.pointer_mut(pointer).unwrap()["unrecognized"] = json!(true);
        rejects_at::<RetirementIdentityV1>(bad, pointer);
    }
    let mut helper = fixture.clone();
    helper["arm"]["subject"] = json!({"kind":"helper","helper_id":EPOCH,
        "purpose":"import","policy_version":1,"permission":{"kind":"admission",
            "receipt":{"operation_id":INIT,"worker_id":ROLE,"revision":1,
                "input_hash":"a".repeat(64),"audit_seq":1,"audit_hash":"b".repeat(64)}}});
    helper["arm"]["timeout_seconds"] = json!(120);
    helper["arm"]["deadline_ns_decimal"] = json!("120000000001");
    let helper_arm: crate::control::ArmV1 = serde_json::from_value(helper["arm"].clone()).unwrap();
    helper["arm_receipt"]["input_hash"] = json!(helper_arm.input_hash().unwrap());
    assert!(serde_json::from_value::<RetirementIdentityV1>(helper.clone()).is_ok());
    for pointer in [
        "/arm/subject",
        "/arm/subject/permission",
        "/arm/subject/permission/receipt",
    ] {
        let mut bad = helper.clone();
        bad.pointer_mut(pointer).unwrap()["unrecognized"] = json!(true);
        rejects_at::<RetirementIdentityV1>(bad, pointer);
    }
    let typed: RetirementIdentityV1 = serde_json::from_value(fixture.clone()).unwrap();
    let closing = json!({"version":1,"identity":fixture,
        "evidence":{"version":1,"identity_hash":typed.digest().unwrap().as_str(),
            "original_actor_nonce":ACTOR,"prepare_revocation_nonce":EPOCH},
        "audit_ack":{"version":1,"event_id":INIT,"input_hash":"a".repeat(64),"audit_seq":1},
        "closing_nonce":ROLE});
    let valid: ClosingAcceptanceV1 = serde_json::from_value(closing.clone()).unwrap();
    check_domain(
        &valid,
        &valid.canonical_bytes().unwrap(),
        valid.digest().unwrap().as_str(),
        b"ai-guard:enforcer-closing-acceptance:v1\0",
    );
    let mut mismatched = closing.clone();
    mismatched["evidence"]["identity_hash"] = json!("f".repeat(64));
    rejects::<ClosingAcceptanceV1>(mismatched);
    for field in ["expected_tombstone_hash", "tombstone_hash", "head_hash"] {
        let mut cycle = closing.clone();
        cycle[field] = json!("c".repeat(64));
        rejects::<ClosingAcceptanceV1>(cycle);
    }
}

#[test]
fn canonical_identity_wire_rejects_legacy_uuid_normalization() {
    let fixture = identity();
    // serde_json::Value emits ordered object keys here, independently from the
    // production parser. Retained legacy UUID Serde must not hide wire changes.
    let bytes = serde_json::to_vec(&fixture).unwrap();
    assert!(RetirementIdentityV1::from_canonical_bytes(&bytes).is_ok());
    for pointer in [
        "/scope/installation_id",
        "/scope/control_namespace_uuid",
        "/arm/operation_id",
        "/arm/clock/boot_id",
        "/arm_receipt/arm_id",
    ] {
        let mut bad = fixture.clone();
        let id = bad
            .pointer(pointer)
            .unwrap()
            .as_str()
            .unwrap()
            .replace('-', "");
        *bad.pointer_mut(pointer).unwrap() = json!(id);
        assert!(
            RetirementIdentityV1::from_canonical_bytes(&serde_json::to_vec(&bad).unwrap()).is_err()
        );
    }
}

fn entry_phases() -> [Value; 3] {
    let closing: Value = serde_json::from_str(ENTRY_CLOSING).unwrap();
    let identity_hash = closing["accepted"]["evidence"]["identity_hash"].clone();
    let tombstone_hash = closing["expected_tombstone_hash"].clone();
    let accepted: ClosingAcceptanceV1 =
        serde_json::from_value(closing["accepted"].clone()).unwrap();
    let closing_hash = accepted.digest().unwrap();
    let mut tombstone = closing.clone();
    tombstone["phase"] = json!({"kind":"tombstone","tombstone_hash":tombstone_hash});
    let mut final_entry = closing.clone();
    final_entry["phase"] = json!({"kind":"final","tombstone_hash":tombstone_hash,
        "receipt":{"version":1,"ledger":ledger(),"retirement_id":INSTALLATION,
            "identity_hash":identity_hash,"finalization":{"version":1,
                "identity_hash":identity_hash,"closing_hash":closing_hash.as_str(),
                "tombstone_hash":tombstone_hash,"joined":{"version":1,
                    "identity_hash":identity_hash,"closing_hash":closing_hash.as_str(),
                    "tombstone_hash":tombstone_hash,"original_epoch":EPOCH,
                    "original_actor_nonce":ACTOR}}}});
    [closing, tombstone, final_entry]
}

#[test]
fn entry_and_delta_commitments_match_independent_literal_wire_goldens() {
    let entry = RetirementLedgerEntryV1::from_canonical_bytes(ENTRY_CLOSING.as_bytes()).unwrap();
    assert_eq!(entry.canonical_bytes().unwrap(), ENTRY_CLOSING.as_bytes());
    assert_eq!(entry.digest().unwrap().as_str(), ENTRY_CLOSING_HASH);
    assert_eq!(
        literal_digest(
            b"ai-guard:enforcer-retirement-entry:v1\0",
            ENTRY_CLOSING.as_bytes()
        ),
        ENTRY_CLOSING_HASH
    );
    let delta = RetirementDeltaV1::from_canonical_bytes(DELTA_BEGIN.as_bytes()).unwrap();
    assert_eq!(delta.canonical_bytes().unwrap(), DELTA_BEGIN.as_bytes());
    assert_eq!(delta.digest().unwrap().as_str(), DELTA_BEGIN_HASH);
    assert_eq!(
        literal_digest(
            b"ai-guard:enforcer-retirement-delta:v1\0",
            DELTA_BEGIN.as_bytes()
        ),
        DELTA_BEGIN_HASH
    );
}

#[test]
fn entry_successor_allows_only_begin_exact_replay_and_monotone_phases() {
    let [c, t, f] = entry_phases()
        .map(|value| serde_json::from_value::<RetirementLedgerEntryV1>(value).unwrap());
    assert!(RetirementLedgerEntryV1::validate_successor(None, &c).is_ok());
    assert!(RetirementLedgerEntryV1::validate_successor(None, &t).is_err());
    assert!(RetirementLedgerEntryV1::validate_successor(None, &f).is_err());
    for (previous, next) in [(&c, &c), (&c, &t), (&t, &t), (&t, &f), (&f, &f)] {
        assert!(RetirementLedgerEntryV1::validate_successor(Some(previous), next).is_ok());
    }
    for (previous, next) in [(&c, &f), (&t, &c), (&f, &c), (&f, &t)] {
        assert!(RetirementLedgerEntryV1::validate_successor(Some(previous), next).is_err());
    }
}

#[test]
fn entry_successor_rejects_valid_but_changed_accepted_prefix() {
    let [closing, mut changed, _] = entry_phases();
    // Recompute the changed entry's own data commitments, so this exercises
    // immutable-prefix comparison rather than merely invalid local hashes.
    changed["accepted"]["audit_ack"]["input_hash"] = json!("f".repeat(64));
    let accepted: ClosingAcceptanceV1 =
        serde_json::from_value(changed["accepted"].clone()).unwrap();
    let tombstone: RetiredTombstoneV1 = serde_json::from_value(json!({"version":1,
        "ledger":ledger(),"identity_hash":changed["accepted"]["evidence"]["identity_hash"],
        "closing_hash":accepted.digest().unwrap().as_str()}))
    .unwrap();
    let hash = tombstone.digest().unwrap();
    changed["expected_tombstone_hash"] = json!(hash.as_str());
    changed["phase"]["tombstone_hash"] = json!(hash.as_str());
    let previous: RetirementLedgerEntryV1 = serde_json::from_value(closing).unwrap();
    let next: RetirementLedgerEntryV1 = serde_json::from_value(changed).unwrap();
    assert!(RetirementLedgerEntryV1::validate_successor(Some(&previous), &next).is_err());
}

#[test]
fn phase_and_transition_wire_are_closed_and_cannot_disagree() {
    let phases = entry_phases();
    for (index, entry) in phases.iter().enumerate() {
        rejects_unknown_and_version::<RetirementLedgerEntryV1>(entry.clone());
        let mut extra = entry.clone();
        extra["phase"]["unrecognized"] = json!(true);
        // Direct phase decode and nested Entry both enforce the boundary;
        // Closing is the internally tagged unit variant from the RED run.
        rejects_at::<RetirementLedgerPhaseV1>(
            extra["phase"].clone(),
            &format!("phase[{index}] unrecognized key"),
        );
        rejects_at::<RetirementLedgerEntryV1>(
            extra,
            &format!("entry.phase[{index}] unrecognized key"),
        );
    }
    let delta: Value = serde_json::from_str(DELTA_BEGIN).unwrap();
    rejects_unknown_and_version::<RetirementDeltaV1>(delta.clone());
    for (kind, index) in [
        ("begin_closing", 0),
        ("commit_tombstone", 1),
        ("finalize", 2),
    ] {
        let mut valid = delta.clone();
        valid["revision"] = json!(index + 1);
        valid["expected_entry_hash"] = if index == 0 {
            Value::Null
        } else {
            json!("a".repeat(64))
        };
        valid["transition"] = json!({"kind":kind,"entry":phases[index]});
        assert!(serde_json::from_value::<RetirementDeltaV1>(valid.clone()).is_ok());
        let mut wrong = valid.clone();
        wrong["transition"]["entry"] = phases[(index + 1) % 3].clone();
        rejects::<RetirementDeltaV1>(wrong);
        let mut extra = valid;
        extra["transition"]["unrecognized"] = json!(true);
        rejects_at::<RetirementLedgerTransitionV1>(
            extra["transition"].clone(),
            &format!("direct transition {kind} unrecognized key"),
        );
        rejects_at::<RetirementDeltaV1>(extra, &format!("transition {kind} unrecognized key"));
    }
    let mut wrong_arm = delta.clone();
    wrong_arm["arm_id"] = json!(EPOCH);
    rejects::<RetirementDeltaV1>(wrong_arm);
    for kind in ["restore", "Closing", "final", "unknown"] {
        let mut invalid = delta.clone();
        invalid["transition"]["kind"] = json!(kind);
        rejects::<RetirementDeltaV1>(invalid);
    }
}

#[test]
fn canonical_record_parser_bounds_input_before_decoding() {
    let oversized = vec![b' '; 8 * 1024 * 1024 + 1];
    assert!(matches!(
        GenesisCoreV1::from_canonical_bytes(&oversized),
        Err(JournalError::TooLarge)
    ));
    assert!(matches!(
        RetirementDeltaV1::from_canonical_bytes(&oversized),
        Err(JournalError::TooLarge)
    ));
}

#[test]
fn delta_expected_entry_requires_exact_previous_commitment() {
    let [c, t, _] = entry_phases();
    let previous: RetirementLedgerEntryV1 = serde_json::from_value(c).unwrap();
    let mut delta: Value = serde_json::from_str(DELTA_BEGIN).unwrap();
    let begin: RetirementDeltaV1 = serde_json::from_value(delta.clone()).unwrap();
    assert!(begin.validate_previous(None).is_ok());
    assert!(begin.validate_previous(Some(&previous)).is_err());
    delta["revision"] = json!(2);
    delta["expected_entry_hash"] = json!(previous.digest().unwrap().as_str());
    delta["transition"] = json!({"kind":"commit_tombstone","entry":t});
    let next: RetirementDeltaV1 = serde_json::from_value(delta.clone()).unwrap();
    assert!(next.validate_previous(Some(&previous)).is_ok());
    assert!(next.validate_previous(None).is_err());
    delta["expected_entry_hash"] = json!("a".repeat(64));
    let wrong: RetirementDeltaV1 = serde_json::from_value(delta).unwrap();
    assert!(wrong.validate_previous(Some(&previous)).is_err());
}
