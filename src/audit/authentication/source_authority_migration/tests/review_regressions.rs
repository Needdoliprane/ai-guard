//! Review hypotheses only until the two parents execute on admitted offline QA.
//! Source data and all effects belong to the real temporary private fixtures.
use super::*;
use anyhow::ensure;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Instant;

// Break: treating the only source inode as its own independent verified backup.
#[test]
fn ordinary4_resume_refuses_its_own_source_before_conversion() {
    let fixture = Fixture::new(8);
    let before = fixture.audit.verify().unwrap();
    fixture
        .maintenance()
        .with_review_maintenance_writer(&fixture.token, |writer| {
            let resumed = writer.resume_backup(&fixture.token, &fixture.path);
            assert!(
                resumed.is_err(),
                "a source cannot be its own migration backup"
            );
            assert!(matches!(
                writer.inspect()?,
                AuthenticationSourceMigrationState::Ordinary4 { .. }
            ));
            assert_eq!(writer.verification()?, before);
            writer.commit()
        })
        .unwrap();
    assert_eq!(fixture.audit.format_version().unwrap(), 4);
    assert_eq!(fixture.audit.verify().unwrap(), before);
}

fn run_owned_operation(
    f: &OwnedFixture,
    epoch: &HumanSourcesWriteEpoch,
    operation: &str,
    b: &AuthenticationSessionBindingV1,
    record: &AuthenticationRecordV1,
    backup: &Path,
) -> Result<AuthenticationReceiptV1> {
    match operation {
        "append" | "retry" => {
            let candidates = AuthenticationTenantCandidatesV1::from_verified_memberships(
                b.principal.clone(),
                vec!["A".into()],
            )?;
            Ok(f.audit
                .begin_verified_initial_with_candidates_owned(
                    epoch,
                    &f.source,
                    record.clone(),
                    b.principal.clone(),
                    b.access_fingerprint,
                    candidates,
                )?
                .receipt)
        }
        "read" => {
            let access = AuthenticationSourceAccess::write(epoch, &f.source)?;
            Ok(f.audit.verify_owned(&access)?.head)
        }
        "backup" => {
            let access = AuthenticationSourceAccess::write(epoch, &f.source)?;
            Ok(f.audit.backup_new_owned(&access, backup)?.head)
        }
        _ => unreachable!("closed test operation list"),
    }
}

// Break: the final FS validation outlives the last epoch check and still returns
// an ACK/reading. Calibration identifies the last real source-backing check of
// each actual path; the second call expires the genuine epoch inside that check.
// No forged clock, arbitrary sleep, host-load assumption or database mock.
#[test]
fn expiry_inside_last_backing_check_refuses_append_retry_read_and_backup() {
    for operation in ["append", "retry", "read", "backup"] {
        let f = Fixture::new(8).activate();
        let first_binding = binding();
        let first_record = AuthenticationRecordV1 {
            principal: None,
            ..intention(&first_binding, AuthenticationActionV1::SessionAdmission)
        };
        let root = Directory::open_existing(f._temporary.path()).unwrap();
        let backups = root
            .persistent_child("review-backups", true)
            .unwrap()
            .unwrap();
        if operation == "retry" {
            let epoch = f.owner.try_write_epoch().unwrap();
            run_owned_operation(
                &f,
                &epoch,
                operation,
                &first_binding,
                &first_record,
                &backups.path().join("unused.sqlite"),
            )
            .unwrap();
        }
        let calibration_calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calibration_calls);
        f.audit.inner.backing.test_hook.set(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        {
            let epoch = f.owner.try_write_epoch().unwrap();
            run_owned_operation(
                &f,
                &epoch,
                operation,
                &first_binding,
                &first_record,
                &backups.path().join("calibration.sqlite"),
            )
            .unwrap();
        }
        let last_check = calibration_calls.load(Ordering::SeqCst);
        assert!(
            last_check > 0,
            "{operation}: no real backing checks observed"
        );
        let before = f.audit.inner.state.lock().unwrap().floor.clone();
        let actual_binding = if operation == "append" {
            let mut next = binding();
            next.access_fingerprint =
                AuthenticationTokenFingerprintV1::access_token("synthetic-review-second-proof");
            next
        } else {
            first_binding
        };
        let actual_record = if operation == "append" {
            AuthenticationRecordV1 {
                principal: None,
                ..intention(&actual_binding, AuthenticationActionV1::SessionAdmission)
            }
        } else {
            first_record
        };
        let epoch = f.owner.try_write_epoch().unwrap();
        // Root-owned cfg(test), read-only seam: the existing genuine deadline.
        // It must return self.lease.deadline without changing production checks.
        let deadline = epoch.test_deadline();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed_calls = Arc::clone(&calls);
        let expired_here = Arc::new(AtomicBool::new(false));
        let observed_expiry = Arc::clone(&expired_here);
        f.audit.inner.backing.test_hook.set(move || {
            let call = observed_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == last_check {
                ensure!(
                    Instant::now() < deadline,
                    "test reached final backing check after its epoch had already expired"
                );
                while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
                    std::thread::sleep(remaining);
                }
                observed_expiry.store(true, Ordering::SeqCst);
            }
            Ok(())
        });
        let result = run_owned_operation(
            &f,
            &epoch,
            operation,
            &actual_binding,
            &actual_record,
            &backups.path().join("expiry.sqlite"),
        );
        // Remove the owned hook before independent state/recovery observations.
        f.audit.inner.backing.test_hook.set(|| Ok(()));
        assert!(
            expired_here.load(Ordering::SeqCst),
            "{operation}: final-check expiry hook was not reached"
        );
        let committed = f.audit.inner.state.lock().unwrap().floor.clone();
        assert_eq!(
            committed.sequence,
            before.sequence + u64::from(operation == "append"),
            "{operation}: preserve a known commit; no invented rollback or extra entry"
        );
        if operation != "append" {
            assert_eq!(committed, before);
        }
        let error = result.expect_err("an expired final backing check cannot publish a result");
        assert_eq!(
            error.downcast_ref::<crate::human_authority::HumanAuthorityFailure>(),
            Some(&crate::human_authority::HumanAuthorityFailure::EpochExpired),
            "{operation}: {error:#}"
        );
        drop(epoch);
        let fresh = f.owner.try_write_epoch().unwrap();
        let access = AuthenticationSourceAccess::write(&fresh, &f.source).unwrap();
        assert_eq!(
            f.audit.verify_owned(&access).unwrap().head,
            committed,
            "{operation}: expiry must not latch a healthy owner"
        );
        if operation == "append" {
            let replay = run_owned_operation(
                &f,
                &fresh,
                "retry",
                &actual_binding,
                &actual_record,
                &backups.path().join("unused-retry.sqlite"),
            )
            .unwrap();
            assert_eq!(replay, committed);
            assert_eq!(f.audit.verify_owned(&access).unwrap().head, committed);
        }
    }
}

// A distinct inode with the exact real backup bytes still has an incompatible
// parent. The original source and the actual distinct backup remain untouched.
#[test]
fn ordinary4_resume_refuses_a_distinct_copy_in_the_source_parent() {
    let fixture = Fixture::new(8);
    let before = fixture.audit.verify().unwrap();
    fixture
        .maintenance()
        .with_review_maintenance_writer(&fixture.token, |writer| {
            let backup = writer.prepare_backup(&fixture.token, &fixture.backup())?;
            let same_parent = fixture
                .path
                .parent()
                .unwrap()
                .join("same-parent-copy.sqlite");
            let (backing, guard) = StoreStorage::create_new(&same_parent)?;
            let mut destination = modern_connection(&backing, false, Duration::ZERO)?;
            let reader = modern_connection(&backup.backup.backing, true, Duration::ZERO)?;
            {
                let copy = rusqlite::backup::Backup::new(&reader, &mut destination)?;
                assert_eq!(copy.step(256)?, rusqlite::backup::StepResult::Done);
            }
            reader.close().map_err(|(_, error)| error)?;
            destination.close().map_err(|(_, error)| error)?;
            backing.sync_closed(&guard)?;
            drop(guard); // The refusal must be parent identity, not an init-lock timeout.
            assert_ne!(
                backing.identity_key().1,
                fixture.audit.inner.backing.identity_key().1
            );
            assert!(writer.resume_backup(&fixture.token, &same_parent).is_err());
            assert_eq!(writer.verification()?, before);
            assert_eq!(writer.inspect()?.anchor(), None);
            writer.commit()
        })
        .unwrap();
    assert_eq!(fixture.audit.format_version().unwrap(), 4);
    assert_eq!(fixture.audit.verify().unwrap(), before);
}

// The restriction must retain the real4 backup/replay contract on either side
// of conversion, with the same source/migration/prefix and no backup overwrite.
#[test]
fn distinct4_backup_resumes_before_and_after_conversion() {
    let fixture = Fixture::new(8);
    let before = fixture.audit.verify().unwrap();
    let (receipt, binding, migration_id) = fixture
        .maintenance()
        .with_review_maintenance_writer(&fixture.token, |writer| {
            let prepared = writer.prepare_backup(&fixture.token, &fixture.backup())?;
            let binding = prepared.backup_binding();
            let migration_id = prepared.migration_id();
            let resumed = writer.resume_backup(&fixture.token, &fixture.backup())?;
            assert_eq!(resumed.backup_binding(), binding);
            assert_eq!(resumed.migration_id(), migration_id);
            assert_eq!(resumed.prefix(), &before.head);
            let receipt = writer.install_format5(&resumed)?;
            writer.commit()?;
            Ok((receipt, binding, migration_id))
        })
        .unwrap();
    fixture
        .maintenance()
        .with_review_maintenance_writer(&fixture.token, |writer| {
            let resumed = writer.resume_backup(&fixture.token, &fixture.backup())?;
            assert_eq!(resumed.backup_binding(), binding);
            assert_eq!(resumed.migration_id(), migration_id);
            assert_eq!(resumed.prefix(), &before.head);
            assert_eq!(writer.install_format5(&resumed)?, receipt);
            assert_eq!(writer.verification()?.head, receipt);
            writer.commit()
        })
        .unwrap();
    let backup = AuthenticationAudit::open_existing(
        &fixture.backup(),
        fixture.installation,
        fixture.policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap();
    assert_eq!(backup.verify().unwrap(), before);
    assert_eq!(backup.format_version().unwrap(), 4);
}
