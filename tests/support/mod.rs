use std::os::unix::fs::PermissionsExt;

/// Positive database fixtures use the same private parent policy as the product.
/// Security tests create unsuitable directories explicitly in their own module.
pub fn private_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("create a private test directory")
}
