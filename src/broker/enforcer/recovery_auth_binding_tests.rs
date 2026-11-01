//! Mutations of an actual challenge must invalidate its authentication proof.
use super::*;
use serde_json::{Value, json};

fn different_uuid(current: &Value) -> Value {
    let first = json!(Uuid::from_u128(1));
    if current == &first {
        json!(Uuid::from_u128(2))
    } else {
        first
    }
}

#[tokio::test]
async fn recovery_authentication_binds_every_challenge_field_and_direction() {
    let key = [0xa5; 32];
    let state = Arc::new(RecoveryState::new(
        Uuid::new_v4(),
        1,
        key,
        "b".repeat(64),
        None,
        None,
    ));
    let (server, mut client) = UnixStream::pair().unwrap();
    let connection = tokio::spawn(serve_connection(server, state));
    let received = read_frame::<Value>(&mut client, HANDSHAKE_FRAME, IO_TIMEOUT).await;
    // Join before any oracle so a failed read cannot leave an unauthenticated
    // server task waiting on this fixture's stream.
    connection.abort();
    match connection.await {
        Err(error) => assert!(error.is_cancelled(), "challenge task panicked"),
        Ok(result) => assert!(result.is_err(), "unauthenticated session completed"),
    }
    drop(client);
    let wire = received.expect("read the production recovery challenge");
    let challenge: ChallengeV2 = serde_json::from_value(wire.clone()).unwrap();
    let mutations = [
        ("protocol", json!(3)),
        ("role", json!("different_enforcer")),
        ("epoch", different_uuid(&wire["epoch"])),
        ("nonce", different_uuid(&wire["nonce"])),
        ("root_binding", json!("a".repeat(64))),
        ("offered_roles", json!(["full"])),
        (
            "ledger_state",
            json!({"state":"unverified","reason":"different_recovery_reason"}),
        ),
    ];

    for server_proof in [false, true] {
        let proof = auth_proof(&key, &challenge, RequestedRole::StopsOnly, server_proof)
            .expect("create a proof for this exact challenge");
        verify_auth(
            &key,
            &challenge,
            RequestedRole::StopsOnly,
            server_proof,
            &proof,
        )
        .expect("the unchanged challenge must authenticate");
        for (field, replacement) in &mutations {
            let mut changed = wire.clone();
            changed[*field] = replacement.clone();
            assert_ne!(changed, wire, "mutation must change {field}");
            let changed: ChallengeV2 = serde_json::from_value(changed).unwrap();
            assert!(
                verify_auth(
                    &key,
                    &changed,
                    RequestedRole::StopsOnly,
                    server_proof,
                    &proof,
                )
                .is_err(),
                "proof must bind {field} in each direction"
            );
        }
        assert!(
            verify_auth(&key, &challenge, RequestedRole::Full, server_proof, &proof,).is_err(),
            "the requested role is authenticated separately from the offer"
        );
        assert!(
            verify_auth(
                &key,
                &challenge,
                RequestedRole::StopsOnly,
                !server_proof,
                &proof,
            )
            .is_err(),
            "a client proof cannot serve as a server proof, or conversely"
        );
        assert!(
            verify_auth(
                &[0xa6; 32],
                &challenge,
                RequestedRole::StopsOnly,
                server_proof,
                &proof,
            )
            .is_err(),
            "a different capability cannot authenticate the same challenge"
        );
    }
}
