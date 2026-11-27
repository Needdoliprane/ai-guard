//! A checked old connection must not overwrite a successor already observed.
//! All processes and barriers here belong to this fixture. No runtime effect
//! or deadline enforcement is established by the unavailable engine metadata.
use super::*;

fn release_epoch_barrier(root: &Path) {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join("release"))
        .and_then(|mut file| std::io::Write::write_all(&mut file, b"fixture-release\n"))
        .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn checked_old_connection_cannot_publish_over_a_newer_observed_epoch_or_ack() {
    let fixture = Fixture::with_manual_arm(true);
    let command = Uuid::new_v4();
    let generation = 1;
    let projection = Arc::new(
        fixture
            .writer
            .stops()
            .append_cancel(CancelInputV1::new(fixture.operation, command).unwrap())
            .await
            .unwrap(),
    );
    let arm = projection.stops()[0].arm().operation_id.clone();
    let (_, hash) = recovery_registration(projection.scope(), &projection.stops()[0]);
    let (_role_temp, root) = recovery_role_fixture();
    let mut child = recovery_child(&root, None);
    let client = warmed_recovery_client(&root).await;
    client
        .deliver_projection(Arc::clone(&projection), generation)
        .await
        .unwrap();
    let old_epoch = client.connection_epoch().unwrap();
    assert_eq!(client.acknowledged_restore_epoch(&arm), Some(old_epoch));
    assert_eq!(
        client
            .query_stop(&arm, &hash)
            .await
            .unwrap()
            .cancel_command_id(),
        Some(command)
    );

    // No connection is opened between arming this hook and starting the exact
    // replay. Warm Watch already owns its E1 stream, and the cached stop is ACKed.
    let checked_open = client.pause_next_checked_open();
    let replay_client = client.clone();
    let mut replay = tokio::spawn(async move {
        replay_client
            .deliver_projection(projection, generation)
            .await
    });
    let expires = std::time::Instant::now() + Duration::from_secs(3);
    while checked_open.entered_epoch().is_none() {
        assert!(
            std::time::Instant::now() < expires,
            "exact replay did not finish its real E1 endpoint/auth checks"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(checked_open.entered_epoch(), Some(old_epoch));
    assert!(checked_open.is_blocked());
    assert!(
        !replay.is_finished(),
        "the replay waiter must be the checked open held by the barrier"
    );

    let barrier = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let barrier_root = barrier.path().canonicalize().unwrap();
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    child = recovery_child_with_faults(
        &root,
        Some(old_epoch),
        &RecoveryFaults {
            barrier: Some((&arm, &barrier_root)),
            repeat_barrier: true,
            ..RecoveryFaults::default()
        },
    );
    let new_epoch = crate::broker::discovery::load_endpoint(&root)
        .unwrap()
        .unwrap()
        .epoch();
    assert_ne!(new_epoch, old_epoch);
    // A real E2 handshake publishes the successor. A Watch health transition
    // may make Status unavailable, but the authenticated observation is causal.
    let _status = client.status().await;
    assert_eq!(client.connection_epoch(), Some(new_epoch));
    let expires = std::time::Instant::now() + Duration::from_secs(3);
    while !barrier_root.join("reserved").exists() {
        assert!(
            std::time::Instant::now() < expires,
            "E2 replay did not reach the real post-Reserve barrier"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(checked_open.is_blocked());
    let history_before_release = client.observation_history();
    assert_eq!(history_before_release.last(), Some(&new_epoch));

    // E2 already owns the Cancel, but its held Restore reply prevents the
    // client from validating E2's delivery ACK. E1's cached ACK cannot
    // complete the replay.
    checked_open.release();
    let expires = std::time::Instant::now() + Duration::from_secs(3);
    while checked_open.is_blocked() {
        assert!(
            std::time::Instant::now() < expires,
            "the checked E1 connection did not resume after its explicit release"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let mut completed = match tokio::time::timeout(Duration::from_millis(100), &mut replay).await {
        Ok(result) => Some(result.unwrap()),
        Err(_) => None,
    };
    let history = client.observation_history();
    let rolled_back = history[history_before_release.len()..].contains(&old_epoch);
    let old_ack_succeeded = completed.as_ref().is_some_and(|result| result.is_ok());
    let epoch_during_hold = client.connection_epoch();
    let stop_during_hold = client.query_stop(&arm, &hash).await.unwrap();
    let ack_during_hold = client.acknowledged_restore_epoch(&arm);
    let still_held = !barrier_root.join("release").exists();

    release_epoch_barrier(&barrier_root);
    let acknowledged_in_new_epoch = tokio::time::timeout(Duration::from_secs(3), async {
        while client.acknowledged_restore_epoch(&arm) != Some(new_epoch) {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .is_ok();
    if completed.is_none() {
        match tokio::time::timeout(Duration::from_secs(5), &mut replay).await {
            Ok(result) => completed = Some(result.unwrap()),
            Err(_) => {
                replay.abort();
                let _ = replay.await;
            }
        }
    }
    let final_epoch = client.connection_epoch();
    let final_slot = client.query_stop(&arm, &hash).await.unwrap();
    drop(client);
    drop(child);

    assert!(still_held);
    assert_eq!(
        stop_during_hold.cancel_command_id(),
        Some(command),
        "E2's native STOP owner must retain the exact Cancel before the Restore reply"
    );
    assert_eq!(stop_during_hold.registration_hash(), hash.as_str());
    assert_eq!(stop_during_hold.generation(), generation);
    assert_ne!(
        ack_during_hold,
        Some(new_epoch),
        "the held Restore reply must prevent a validated E2 delivery ACK"
    );
    assert!(
        !rolled_back,
        "a checked E1 connection must never republish E1 after authenticated E2"
    );
    assert_eq!(epoch_during_hold, Some(new_epoch));
    assert!(
        !old_ack_succeeded,
        "old cached ACK must not report success before the E2 Restore can complete"
    );
    assert!(
        acknowledged_in_new_epoch,
        "the retained Cancel must receive its validated E2 ACK without a business retry"
    );
    assert!(
        completed.is_some(),
        "the original replay waiter must finish within the test bound"
    );
    assert_eq!(final_epoch, Some(new_epoch));
    assert_eq!(final_slot.cancel_command_id(), Some(command));
    assert_eq!(final_slot.registration_hash(), hash.as_str());
}
