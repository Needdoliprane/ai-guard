//! Software-only parser boundary: real private files, V1 positive control, then
//! a closed V2 fact. No engine, CLI, HTTP, credential or AcceptedJob is created.
use super::*;
use crate::broker::{discovery::RoleLease, relay_owner::tests::arm};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

fn legacy_fact() -> ChannelFact {
    let arm = arm();
    ChannelFact {
        version: 1,
        job_id: Uuid::from_u128(7),
        source_profile_id: "fixture".into(),
        endpoint: RelayEndpointV1 {
            worker_id: arm.worker_id,
            attempt_id: arm.attempt_id,
            generation: 1,
        },
        source_head_revision: 1,
        source_head_hash: "a".repeat(64),
        arm_id: arm.operation_id,
        original_clock: arm.clock,
        original_deadline_ns_decimal: arm.deadline_ns_decimal,
        phase: AttachmentPhase::AwaitingIssuedWorkload,
        observed_at: "2026-11-11T13:00:00Z".into(),
        error: None,
        admission: None,
        claim: None,
        core: None,
        transition: None,
        local_joined: false,
    }
}

#[test]
fn v1_prefix_then_valid_v2_fact_reopens_without_rewriting_legacy() {
    let temporary = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let root = Directory::open_existing(temporary.path()).unwrap();
    let _role = RoleLease::acquire(&root).unwrap();
    let scope = JournalScopeV1::new(
        Uuid::from_u128(8),
        "tenant-a",
        "http://127.0.0.1:9",
        Uuid::from_u128(9),
        "relay_shared",
    )
    .unwrap();
    // Public synthetic literal required by the real private-record guard.
    // It is absent from every record and filename, and is no usable credential.
    let literals = vec!["journal-v2-public-fixture-literal-not-present-in-records".into()];
    let journal = ChannelJournal::open(&root, &scope, &literals).unwrap();
    assert!(journal.get(Uuid::from_u128(7)).unwrap().is_none());
    drop(journal);
    let directory = root.persistent_child(CHILD, false).unwrap().unwrap();
    let first = legacy_fact();
    validate(&first).unwrap();
    validate_scope(&first, &scope).unwrap();
    // Freeze actual V1 bytes, rather than relying on the future writer to keep
    // emitting its old format. The same prefix must stay a V1 migration canary.
    let legacy_bytes = super::super::super::journal::canonical(&json!({
        "version": 1,
        "sequence": 1,
        "previous_sha256": null,
        "fact": first,
    }))
    .unwrap();
    assert!(legacy_bytes.len() <= MAX_RECORD);
    let legacy_file = write(&directory, &name(1), &legacy_bytes, &literals).unwrap();
    let journal = ChannelJournal::open(&root, &scope, &literals)
        .expect("the V1 prefix must reopen before presenting V2");
    let reopened = journal.get(first.job_id).unwrap().unwrap();
    assert_eq!(reopened.phase, AttachmentPhase::AwaitingIssuedWorkload);
    assert_eq!(
        super::super::super::journal::canonical(&reopened).unwrap(),
        super::super::super::journal::canonical(&first).unwrap()
    );
    drop(journal);

    let mut next = first.clone();
    next.phase = AttachmentPhase::Unknown;
    next.error = Some(RelayOwnerError::Unknown);
    next.observed_at = "2026-11-11T13:00:01Z".into();
    validate(&next).unwrap();
    validate_next(&first, &next).unwrap();
    validate_scope(&next, &scope).unwrap();
    // Closed is deliberately not involved: the V2 reader must not use an old
    // Fact::Closed shortcut instead of validating its complete history index.
    let v2_bytes = super::super::super::journal::canonical(&json!({
        "version": 2,
        "sequence": 2,
        "previous_sha256": legacy_file.digest(),
        "job_id": first.job_id,
        "observed_at": next.observed_at,
        "change": {"kind": "fact", "fact": next},
    }))
    .unwrap();
    assert!(
        v2_bytes.len() <= MAX_RECORD,
        "full V2 envelope exceeds its bound"
    );
    crate::json::decode_unique(&v2_bytes, 64).unwrap();
    write(&directory, &name(2), &v2_bytes, &literals).unwrap();
    eprintln!(
        "journal_history_v2 positive=v1_reopened records=1 v2_full_bytes={}",
        v2_bytes.len()
    );
    let journal = ChannelJournal::open(&root, &scope, &literals)
        .expect("a valid chained V2 fact must reopen after the validated V1 prefix");
    let reopened = journal.get(first.job_id).unwrap().unwrap();
    assert_eq!(reopened.phase, AttachmentPhase::Unknown);
    assert_eq!(
        super::super::super::journal::canonical(&reopened).unwrap(),
        super::super::super::journal::canonical(&next).unwrap()
    );
    let legacy_after = directory
        .persistent_read(&name(1), MAX_RECORD as u64, &[])
        .unwrap()
        .unwrap();
    assert_eq!(legacy_after.bytes(), legacy_bytes.as_slice());
    legacy_file.persistent_revalidate().unwrap();
}
