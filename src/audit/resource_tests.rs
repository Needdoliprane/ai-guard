//! Local, opt-in faults at the filesystem boundary; SQLite and the Store stay real.
use super::*;
use std::{io, os::unix::fs::PermissionsExt, sync::atomic::AtomicUsize};

mod api_cases;

type Hook = Box<dyn FnMut() -> anyhow::Result<()> + Send>;

#[derive(Default)]
pub(super) struct StorageHook(Mutex<Option<Hook>>);

impl StorageHook {
    pub(super) fn run(&self) -> anyhow::Result<()> {
        if let Some(hook) = self.0.lock().unwrap().as_mut() {
            hook()?;
        }
        Ok(())
    }

    pub(super) fn set(&self, hook: impl FnMut() -> anyhow::Result<()> + Send + 'static) {
        *self.0.lock().unwrap() = Some(Box::new(hook));
    }
}

fn fixture() -> (tempfile::TempDir, Store) {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Store::open(
        &directory.path().join("audit.sqlite"),
        Redactor::new(vec![]),
        100,
    )
    .unwrap();
    (directory, store)
}

fn fail_names_on(store: &Store, nth: usize, code: i32) -> Arc<AtomicUsize> {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    store.backing.test_hook.set(move || {
        let call = observed.fetch_add(1, Ordering::SeqCst) + 1;
        if call == nth {
            return Err(io::Error::from_raw_os_error(code)).context("injected metadata failure");
        }
        Ok(())
    });
    calls
}

fn typed_code(error: &anyhow::Error, code: i32) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.raw_os_error() == Some(code))
    })
}

#[test]
fn descriptor_shortage_at_every_store_boundary_requires_full_validation_then_recovers() {
    for code in [libc::EMFILE, libc::ENFILE] {
        for (operation, nth, committed) in [
            ("events", 1, false),
            ("head", 1, false),
            ("integrity", 1, false),
            ("begin_before", 1, false),
            ("begin_after", 2, false),
            ("commit_before", 3, false),
            ("commit_after", 4, true),
        ] {
            let (_directory, store) = fixture();
            let before = store.integrity().unwrap().event_count;
            let event = Event::new("fixture.resource_boundary");
            let calls = fail_names_on(&store, nth, code);
            let result = match operation {
                "events" => store.events(&EventQuery::default()).map(|_| ()),
                "head" => store.head().map(|_| ()),
                "integrity" => store.integrity().map(|_| ()),
                _ => store.append(event.clone()).map(|_| ()),
            };
            let error = result.expect_err("the faulted request must never be acknowledged");
            assert!(typed_code(&error, code), "{operation}: {error:#}");
            assert!(calls.load(Ordering::SeqCst) >= nth);
            assert!(
                !store.healthy(),
                "{operation}: authority opened before validation"
            );
            // No explicit integrity endpoint, reopen, or lease reset: an ordinary
            // validating read must recover this same live origin automatically.
            let events = store.events(&EventQuery::default()).unwrap();
            assert!(
                store.healthy(),
                "{operation}: resource error latched permanently"
            );
            assert_eq!(events.len() as u64, before + u64::from(committed));
            let stored = store.append(event.clone()).unwrap();
            assert_eq!(store.append(event).unwrap().seq, stored.seq);
            assert_eq!(store.integrity().unwrap().event_count, before + 1);
        }
    }
}

#[test]
fn suspended_origin_recovers_before_append_or_redaction_attachment() {
    for recover_by_append in [true, false] {
        let (_directory, store) = fixture();
        let store = Arc::new(store);
        let sibling = store.sibling().unwrap();
        fail_names_on(&store, 1, libc::EMFILE);
        assert!(store.events(&EventQuery::default()).is_err());
        assert!(!store.healthy());
        assert!(
            !sibling.healthy(),
            "suspension must cover the shared origin"
        );
        if recover_by_append {
            store
                .append(Event::new("fixture.recovery_by_append"))
                .unwrap();
        } else {
            let attached = store
                .with_additional_redaction(&Redactor::new(vec!["SyntheticResourceCanary".into()]))
                .unwrap();
            assert!(Arc::ptr_eq(
                &store.supervision_lease,
                &attached.supervision_lease
            ));
            attached
                .append(Event::new("fixture.recovery_by_attachment"))
                .unwrap();
        }
        assert!(store.healthy() && sibling.healthy());
        assert!(store.integrity().unwrap().valid);
    }
}

#[test]
fn permanent_io_unknown_error_and_error_text_never_restore_authority() {
    for code in [libc::EIO, libc::EACCES, libc::ENOENT, libc::ENOMEM] {
        let (_directory, store) = fixture();
        fail_names_on(&store, 1, code);
        let error = store
            .append(Event::new("fixture.permanent_failure"))
            .unwrap_err();
        assert!(typed_code(&error, code));
        assert!(!store.healthy());
        assert!(store.integrity().unwrap().valid);
        assert!(
            !store.healthy(),
            "unclassified IO must remain permanently closed"
        );
        assert!(
            store
                .append(Event::new("fixture.refused_after_permanent"))
                .is_err()
        );
    }
    let (_directory, store) = fixture();
    let mut first = true;
    store.backing.test_hook.set(move || {
        if std::mem::take(&mut first) {
            anyhow::bail!("Too many open files (os error {})", libc::EMFILE);
        }
        Ok(())
    });
    assert!(store.append(Event::new("fixture.untyped_error")).is_err());
    assert!(store.integrity().unwrap().valid);
    assert!(
        !store.healthy(),
        "formatted text must never be classified as an OS error"
    );
}

#[test]
fn resource_recovery_does_not_skip_metadata_schema_or_chain_validation() {
    for mutation in [
        "UPDATE quota SET bytes=0",
        "CREATE INDEX unexpected_resource_recovery ON events(kind)",
        "UPDATE events SET hash='tampered'",
        "UPDATE control_meta SET genesis_hash='tampered'",
    ] {
        let (_directory, store) = fixture();
        fail_names_on(&store, 1, libc::EMFILE);
        assert!(store.append(Event::new("fixture.pre_tamper")).is_err());
        store.db.lock().unwrap().execute_batch(mutation).unwrap();
        assert!(store.events(&EventQuery::default()).is_err(), "{mutation}");
        assert!(!store.healthy(), "{mutation}");
        assert!(
            store
                .append(Event::new("fixture.refused_after_tamper"))
                .is_err()
        );
    }
}

#[test]
fn successful_sqlite_commit_keeps_the_new_rollback_floor_when_names_are_unavailable() {
    let (_directory, store) = fixture();
    let sibling = store.sibling().unwrap();
    let before = store.head().unwrap();
    fail_names_on(&store, 4, libc::EMFILE);
    let error = store
        .append(Event::new("fixture.committed_without_ack"))
        .unwrap_err();
    assert!(typed_code(&error, libc::EMFILE));
    {
        let db = store.db.lock().unwrap();
        let committed: i64 = db
            .query_row("SELECT max(seq) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            committed,
            before + 1,
            "postcommit fault must follow a real durable mutation"
        );
        // Restore a valid earlier chain in the same inode/connection. Its quota
        // matches that prefix, so only the retained newer witness rejects it.
        db.execute("DELETE FROM events WHERE seq>?1", [before])
            .unwrap();
        db.execute("UPDATE quota SET bytes=(SELECT SUM(length(CAST(payload AS BLOB))) FROM events) WHERE id=1", []).unwrap();
        assert!(Store::check_integrity(&db).unwrap().valid);
        Store::validate_metadata(&db, store.tenant_id()).unwrap();
    }
    assert!(
        store.events(&EventQuery::default()).is_err(),
        "rollback crossed the retained witness"
    );
    assert!(!store.healthy() && !sibling.healthy());
    assert!(
        store
            .append(Event::new("fixture.refused_rollback"))
            .is_err()
    );
}

#[test]
fn activation_from_an_unacknowledged_migration_is_retained_but_never_inferred() {
    let (directory, store) = fixture();
    let observer = Connection::open(directory.path().join("audit.sqlite")).unwrap();
    let seen_commit = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&seen_commit);
    store.backing.test_hook.set(move || {
        let version: i64 = observer.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version == 3 && !observed.swap(true, Ordering::SeqCst) {
            return Err(io::Error::from_raw_os_error(libc::ENFILE))
                .context("postmigration names unavailable");
        }
        Ok(())
    });
    let error = store
        .migrate_control_v3(&directory.path().join("backup-v2.sqlite"))
        .unwrap_err();
    assert!(typed_code(&error, libc::ENFILE), "{error:#}");
    assert!(seen_commit.load(Ordering::SeqCst));
    assert!(!store.healthy());
    assert!(
        !store.lock_supervision_lease().unwrap().active,
        "activation escaped before name verification"
    );
    assert!(
        store.supervision_state().unwrap().active,
        "the exact pending activation was lost"
    );
    let reopened = Store::open(
        &directory.path().join("audit.sqlite"),
        Redactor::new(vec![]),
        100,
    )
    .unwrap();
    assert!(
        !reopened.supervision_state().unwrap().active,
        "schema alone must never recreate execution authority"
    );
}

#[test]
fn sibling_alone_reconciles_a_postcommit_fault_before_exact_replay_and_activation() {
    use crate::identity::{
        HumanAccountV1, IdentityChangeV1, IdentityMutationV1, LegacyControlRoleV1, PrincipalRefV1,
    };

    let (directory, first) = fixture();
    first
        .migrate_control_v3(&directory.path().join("backup-v2.sqlite"))
        .unwrap();
    first
        .migrate_identity_v4(&directory.path().join("backup-v3.sqlite"))
        .unwrap();
    first
        .migrate_human_access_v5(&directory.path().join("backup-v4.sqlite"))
        .unwrap();
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: uuid::Uuid::new_v4().to_string(),
    };
    first
        .mutate_identity(&IdentityMutationV1 {
            version: 1,
            operation_id: uuid::Uuid::new_v4().to_string(),
            actor: PrincipalRefV1::LegacyControl {
                credential_id: "resource-recovery-fixture".into(),
                role: LegacyControlRoleV1::Runner,
            },
            change: IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Resource recovery reader".into(),
                    revision: 1,
                    active: true,
                },
            },
        })
        .unwrap();
    let sibling = first.sibling().unwrap();
    let before_head = first.head().unwrap();
    let before_bytes: i64 = first
        .db
        .lock()
        .unwrap()
        .query_row("SELECT bytes FROM quota WHERE id=1", [], |r| r.get(0))
        .unwrap();
    let committed = {
        let mut db = first.db.lock().unwrap();
        let tx = first.begin_checked(&mut db).unwrap();
        let receipt = first
            .record_human_read(&tx, &principal, None, "identity", 1, &Redactor::new(vec![]))
            .unwrap();
        let stored = tx
            .query_row(
                "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE id=?1",
                [&receipt.audit_event_id],
                row_event,
            )
            .unwrap();
        // Begin and the typed append are complete; the second names check
        // below is therefore after SQLite commit, not a preflight refusal.
        fail_names_on(&first, 2, libc::EMFILE);
        let error = first.commit_checked(tx).unwrap_err();
        assert!(typed_code(&error, libc::EMFILE));
        stored
    };
    let payload_bytes: i64 = first
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT length(CAST(payload AS BLOB)) FROM events WHERE id=?1",
            [&committed.event.id],
            |r| r.get(0),
        )
        .unwrap();
    let payload_bytes = u64::try_from(payload_bytes).unwrap();
    assert_eq!(committed.seq, before_head + 1);
    assert!(!first.healthy() && !sibling.healthy());
    assert_eq!(
        first
            .supervision_lease
            .capacity
            .lock()
            .unwrap()
            .tenant
            .events,
        0
    );
    // Only B performs a validating access. A's private/raw observations above
    // cannot restore the shared origin or publish its previously pending debit.
    let observed = sibling.events(&EventQuery::default()).unwrap();
    assert_eq!(observed.len() as i64, before_head + 1);
    assert!(first.healthy() && sibling.healthy());
    assert_eq!(
        first.lock_supervision_lease().unwrap().floor,
        (committed.seq, committed.hash.clone())
    );
    let replay = {
        let mut db = first.db.lock().unwrap();
        let tx = first.begin_checked(&mut db).unwrap();
        let (event, payload) = first.prepare_event(committed.event.clone()).unwrap();
        let replay = first
            .append_human_in_transaction(&tx, event, payload, &principal)
            .unwrap();
        first.commit_checked(tx).unwrap();
        replay
    };
    assert_eq!(replay.seq, committed.seq);
    assert_eq!(replay.hash, committed.hash);
    assert_eq!(replay.event.id, committed.event.id);
    let (head, copies, bytes): (i64, i64, i64) = first
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT (SELECT max(seq) FROM events),(SELECT count(*) FROM events WHERE id=?1),bytes FROM quota WHERE id=1",
            [&committed.event.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(head, before_head + 1);
    assert_eq!(copies, 1);
    assert_eq!(bytes as u64, before_bytes as u64 + payload_bytes);
    {
        let cache = first.supervision_lease.capacity.lock().unwrap();
        let expected = AuditCapacityV1 {
            events: 1,
            bytes: payload_bytes,
        };
        assert_eq!(cache.tenant, expected);
        assert_eq!(cache.principals[&principal.key().unwrap()], expected);
    }
    assert!(first.integrity().unwrap().valid);

    // A separate fresh origin exercises the exact pending activation as well.
    // B exists before migration and alone reconciles A's unacknowledged commit.
    let (directory, first) = fixture();
    let sibling = first.sibling().unwrap();
    let observer = Connection::open(directory.path().join("audit.sqlite")).unwrap();
    let mut faulted = false;
    first.backing.test_hook.set(move || {
        let version: i64 = observer.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version == 3 && !std::mem::replace(&mut faulted, true) {
            return Err(io::Error::from_raw_os_error(libc::ENFILE))
                .context("postmigration sibling recovery");
        }
        Ok(())
    });
    let error = first
        .migrate_control_v3(&directory.path().join("backup-v2.sqlite"))
        .unwrap_err();
    assert!(typed_code(&error, libc::ENFILE));
    assert!(!first.healthy() && !sibling.healthy());
    {
        let lease = first.lock_supervision_lease().unwrap();
        assert!(!lease.active);
        assert!(lease.pending_activation);
    }
    let recovered = sibling.supervision_state().unwrap();
    assert!(recovered.active);
    let recovered_floor = sibling.lock_supervision_lease().unwrap().floor.clone();
    assert!(first.healthy() && sibling.healthy());
    {
        let lease = first.lock_supervision_lease().unwrap();
        assert!(lease.active);
        assert!(!lease.pending_activation);
        assert_eq!(lease.floor, recovered_floor);
    }
    assert!(first.supervision_state().unwrap().active);
    assert_eq!(first.head().unwrap(), recovered_floor.0);
    let reopened = Store::open(
        &directory.path().join("audit.sqlite"),
        Redactor::new(vec![]),
        100,
    )
    .unwrap();
    assert!(!reopened.supervision_state().unwrap().active);
}

#[test]
fn identity_substitution_and_poison_stay_permanent_after_resource_suspension() {
    let (directory, store) = fixture();
    fail_names_on(&store, 1, libc::EMFILE);
    assert!(store.events(&EventQuery::default()).is_err());
    let original = directory.path().join("audit.sqlite");
    let retained = directory.path().join("preserved.sqlite");
    std::fs::rename(&original, &retained).unwrap();
    assert!(store.events(&EventQuery::default()).is_err());
    std::fs::rename(&retained, &original).unwrap();
    assert!(store.integrity().unwrap().valid);
    assert!(!store.healthy());

    let (_directory, store) = fixture();
    let store = Arc::new(store);
    let poison = Arc::clone(&store);
    assert!(
        std::thread::spawn(move || {
            let _lease = poison.supervision_lease.state.lock().unwrap();
            panic!("fixture lease poison");
        })
        .join()
        .is_err()
    );
    assert!(store.events(&EventQuery::default()).is_err());
    assert!(!store.healthy());
    assert!(store.integrity().is_err());
}
