use super::*;

#[test]
fn launch_target_selection_requires_a_canonical_id_and_revision() {
    let id = "abcdef01-2345-4678-9abc-def012345678";
    let selected = SessionLaunchTargetSelectionV1::parse(id, "7").unwrap();
    assert_eq!(selected.launch_target_id().to_string(), id);
    assert_eq!(selected.expected_target_revision(), 7);
    assert!(SessionLaunchTargetSelectionV1::parse(id, "9223372036854775807").is_ok());

    for invalid_id in [
        "",
        "/workspace/project",
        "00000000-0000-0000-0000-000000000000",
        "ABCDEF01-2345-4678-9ABC-DEF012345678",
        "abcdef01234546789abcdef012345678",
    ] {
        assert!(SessionLaunchTargetSelectionV1::parse(invalid_id, "7").is_err());
    }
    for invalid_revision in ["", "0", "07", "+7", " 7", "9223372036854775808"] {
        assert!(SessionLaunchTargetSelectionV1::parse(id, invalid_revision).is_err());
    }
}
