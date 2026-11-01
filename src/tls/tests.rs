use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

#[test]
fn projected_material_accepts_private_and_readonly_and_rejects_mutable_aliases() {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = temp.path().join("projection");
    fs::write(&path, b"public fixture").unwrap();
    for mode in [0o600, 0o444] {
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(read_material(&path, 14).unwrap(), b"public fixture");
    }
    assert!(read_material(&path, 2).is_err());
    let link = temp.path().join("link");
    symlink(&path, &link).unwrap();
    assert!(read_material(&link, 14).is_err());
    let alias = temp.path().join("hardlink");
    fs::hard_link(&path, &alias).unwrap();
    assert!(read_material(&path, 14).is_err());
    fs::remove_file(alias).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(read_material(&path, 14).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(read_material(&path, 14).is_err());
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let traversal = temp.path().join("../projection");
    assert!(read_material(&traversal, 14).is_err());
}

#[test]
fn pem_parser_never_skips_unknown_sections_or_extra_material() {
    for bytes in [
        b"".as_slice(),
        b"not a pem",
        b"-----BEGIN PRIVATE KEY-----\nAA==\n-----END PRIVATE KEY-----",
        b"-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE----- trailing",
    ] {
        assert!(certificates(bytes).is_err());
    }
    assert!(certificates(&vec![b' '; 65_537]).is_err());
}

#[test]
fn key_section_grammar_rejects_nested_markers_for_every_supported_label() {
    let labels = ["PRIVATE KEY", "RSA PRIVATE KEY", "EC PRIVATE KEY"];
    for label in labels {
        // AA== exercises framing only; no valid key or DER acceptance is
        // claimed by this private grammar test.
        let clean = format!("-----BEGIN {label}-----\nAA==\n-----END {label}-----\n");
        assert!(sections(clean.as_bytes(), &labels, MAX_KEY_BYTES).is_ok());
        for nested_label in labels
            .into_iter()
            .chain(["CERTIFICATE", "OPAQUE TEST BLOCK"])
        {
            let nested = format!(
                "-----BEGIN {label}-----\n-----BEGIN {nested_label}-----\nAA==\n-----END {nested_label}-----\n"
            );
            assert!(sections(nested.as_bytes(), &labels, MAX_KEY_BYTES).is_err());
            let nested_with_outer_end = format!("{nested}-----END {label}-----\n");
            assert!(sections(nested_with_outer_end.as_bytes(), &labels, MAX_KEY_BYTES).is_err());
        }
        let wrong_end = format!("-----BEGIN {label}-----\nAA==\n-----END CERTIFICATE-----\n");
        assert!(sections(wrong_end.as_bytes(), &labels, MAX_KEY_BYTES).is_err());
        let suffix = format!("{clean}unparsed suffix");
        assert!(sections(suffix.as_bytes(), &labels, MAX_KEY_BYTES).is_err());
    }
    assert!(sections(&vec![b' '; MAX_KEY_BYTES + 1], &labels, MAX_KEY_BYTES).is_err());
}
