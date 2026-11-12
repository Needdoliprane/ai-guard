//! Negative closed-format tests. No DTO is used as a positive owner capability.
use super::*;

// Break: accepting a broad future/raw SQLite generation via a logical5 anchor.
#[test]
fn anchor_rejects_sqlite_version_in_the_logical_format_field() {
    let entry = format::Entry::SourceAnchor {
        anchor: SourceAnchor {
            version: 1,
            protocol: PROTOCOL.into(),
            from_format: 4,
            format: 1_000_005,
            migration_id: Uuid::new_v4(),
            installation_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            durable_namespace_id: Uuid::new_v4(),
            kind: HumanSourceKindV1::AuthenticationLedger,
            previous_sequence: 2,
            previous_hash: "ab".repeat(32),
        },
    };
    let format::Entry::SourceAnchor { anchor } = &entry else {
        unreachable!()
    };
    let receipt = AuthenticationReceiptV1 {
        namespace_id: anchor.durable_namespace_id,
        sequence: 3,
        hash: "cd".repeat(32),
    };
    let mut state = None;
    assert!(
        Snapshot::apply(
            &mut state,
            &entry,
            &receipt,
            &anchor.previous_hash,
            anchor.installation_id
        )
        .is_err()
    );
    assert!(state.is_none());
}
// Break: an unknown authority field becomes silently accepted and hash-canonical.
#[test]
fn unknown_authority_fields_are_refused_by_the_typed_decoder() {
    let value = serde_json::json!({ "kind": "source_anchor", "anchor": {
        "version": 1, "protocol": PROTOCOL, "from_format": 4, "format": 5,
        "migration_id": Uuid::new_v4(), "installation_id": Uuid::new_v4(), "source_id": Uuid::new_v4(),
        "durable_namespace_id": Uuid::new_v4(), "kind": "authentication_ledger", "previous_sequence": 2,
        "previous_hash": "ab".repeat(32), "closure_digest": "cd".repeat(32)
    }});
    assert!(serde_json::from_value::<format::Entry>(value).is_err());
}
