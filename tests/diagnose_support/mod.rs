//! Helpers of the `ai-guard-diagnose` integration tests.
//!
//! Every scenario lives under one private parent directory that holds the
//! inspected installation, external credential files, CA files, decoys and the
//! child's working directory. Installations are built by product code or by
//! the documented file formats; no test writes a status of its own.

pub mod child;
pub mod fixtures;
pub mod recording_issuer;
pub mod tree_manifest;

use std::os::unix::fs::PermissionsExt;

/// The private parent the product requires for its own directories.
pub fn private_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("create a private test directory")
}

/// Names the oracles that a run accepts not to execute, separated by commas.
/// It is read by the test process alone: every child still starts with a
/// cleared environment.
pub const ORACLE_NOT_RUN_VARIABLE: &str = "AI_GUARD_TEST_ORACLE_NOT_RUN";

/// Whether an oracle that cannot run in this environment may be left out:
/// only when the value of the variable names it.
pub fn oracle_may_be_left_out(named: Option<&str>, oracle: &str) -> bool {
    named.is_some_and(|names| names.split(',').any(|name| name.trim() == oracle))
}

/// An oracle that could not run in this environment. The test fails: an
/// assertion that did not run is `NOT_RUN`, never a pass, and a passing test
/// hides what it prints. A run that accepts the gap names the oracle in the
/// variable; the line is then the record of it.
pub fn oracle_not_run(oracle: &str, reason: &str) {
    let line = format!("ORACLE NOT_RUN {oracle}: {reason}");
    let named = std::env::var(ORACLE_NOT_RUN_VARIABLE).ok();
    assert!(
        oracle_may_be_left_out(named.as_deref(), oracle),
        "{line}. This oracle did not run, so the test fails. To accept that for one run, name the oracle in {ORACLE_NOT_RUN_VARIABLE}."
    );
    eprintln!("{line}");
}

#[test]
fn an_oracle_that_cannot_run_is_left_out_only_when_it_is_named() {
    assert!(
        !oracle_may_be_left_out(None, "never-read"),
        "without the variable the test fails"
    );
    assert!(!oracle_may_be_left_out(Some(""), "never-read"));
    assert!(oracle_may_be_left_out(Some("never-read"), "never-read"));
    assert!(oracle_may_be_left_out(
        Some("account-home, never-read"),
        "never-read"
    ));
    assert!(
        !oracle_may_be_left_out(Some("account-home"), "never-read"),
        "naming another oracle accepts nothing for this one"
    );
    assert!(!oracle_may_be_left_out(
        Some("never-read-and-more"),
        "never-read"
    ));
}
