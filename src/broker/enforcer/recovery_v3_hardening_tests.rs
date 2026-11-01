//! Additional protocol oracles, without a runtime or ledger producer.
use super::*;

#[tokio::test]
async fn embedded_shape_preserves_original_numbers_and_rejects_escaped_nested_duplicates() {
    let fixture = AnchorFixture::new();
    let value = fixture.request(false).unwrap();
    let session = Session3 {
        key: [0x5a; 32],
        id: Hash3("a".repeat(64)),
        epoch: fixture.state.epoch,
        root_binding: Hash3(fixture.state.root_binding.clone()),
        max_slots: 8,
    };
    let request = Request3::new(
        &session,
        StopRequest::RestoreRequiredStop {
            value: Box::new(value),
        },
    )
    .unwrap();
    let wire = serde_json::to_string(&request).unwrap();
    let positive =
        serde_json::from_str::<Request3>(&wire).is_ok_and(|value| value.validate(&session).is_ok());
    let embedded = serde_json::to_string(&request.command).unwrap();
    let mut numbers = Vec::new();
    for token in [
        "0.0",
        "0e0",
        "-1",
        "-0",
        "18446744073709551615",
        "18446744073709551616",
    ] {
        let old = embedded.replacen(
            "\"cancel_generation\":0",
            &format!("\"cancel_generation\":{token}"),
            1,
        );
        let new = wire.replacen(
            "\"cancel_generation\":0",
            &format!("\"cancel_generation\":{token}"),
            1,
        );
        numbers.push((
            token,
            serde_json::from_str::<StopRequest>(&old).is_ok(),
            serde_json::from_str::<Request3>(&new).is_ok(),
        ));
    }
    let duplicated = wire.replacen(
        "\"durable_cancel\":null",
        r#""durable_cancel":null,"durable_\u0063ancel":null"#,
        1,
    );
    let unknown_duplicate = wire.replacen(
        "\"owned\":{",
        r#""owned":{"unrecognized":[{"a":0,"\u0061":1}],"#,
        1,
    );
    let unknown_single = wire.replacen("\"owned\":{", r#""owned":{"unrecognized":null,"#, 1);
    let too_deep = wire.replacen(
        "\"owned\":{",
        &format!(
            "\"owned\":{{\"unrecognized\":{}0{},",
            "[".repeat(140),
            "]".repeat(140)
        ),
        1,
    );
    let oversized = wire.replacen(
        "\"owned\":{",
        &format!(
            "\"owned\":{{\"unrecognized\":\"{}\",",
            "x".repeat(MAX_FRAME as usize)
        ),
        1,
    );
    let malformed = [
        duplicated,
        unknown_duplicate.clone(),
        unknown_single,
        too_deep,
        oversized,
        format!("{wire} {{}}"),
    ]
    .iter()
    .all(|value| serde_json::from_str::<Request3>(value).is_err());
    // Check this before T gets to ignore/reject an unknown branch. Escaped
    // names must collide as decoded keys at any depth, before map insertion.
    let duplicate_seen = serde_json::from_str::<Shape3>(&unknown_duplicate)
        .err()
        .is_some_and(|error| error.to_string().contains("duplicate embedded V3 field"));
    let unsigned_max_preserved = match serde_json::from_str::<Request3>(&wire.replacen(
        "\"cancel_generation\":0",
        "\"cancel_generation\":18446744073709551615",
        1,
    )) {
        Ok(Request3 {
            command: StopRequest::RestoreRequiredStop { value },
            ..
        }) => value.cancel_generation == u64::MAX,
        _ => false,
    };
    let missing_anchor_closed = fixture_state().check_transport_v3().await.is_err();
    let cleaned = fixture.finish().await;
    assert!(
        cleaned && positive,
        "the unchanged producer must be accepted first"
    );
    assert!(
        numbers.iter().all(|(_, old, new)| old == new),
        "typed numbers must consume original bytes: {numbers:?}"
    );
    assert!(
        numbers
            .iter()
            .filter(|(token, _, _)| matches!(*token, "0.0" | "0e0" | "-1" | "18446744073709551616"))
            .all(|(_, old, new)| !*old && !*new)
    );
    assert!(unsigned_max_preserved && malformed && duplicate_seen && missing_anchor_closed);
}

fn literal_mac(domain: &[u8], bytes: &[u8]) -> String {
    // Independent of proof/raw_mac/canonical and all DTO serializers.
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&[0x5a; 32]).unwrap();
    mac.update(domain);
    mac.update(bytes);
    hex::encode(mac.finalize().into_bytes())
}
fn assert_literal_mac(domain: &[u8], literal_domain: &[u8], typed: &impl Serialize, literal: &str) {
    assert_eq!(canonical(typed).unwrap().as_slice(), literal.as_bytes());
    assert_eq!(
        proof(&[0x5a; 32], domain, typed).unwrap().0,
        literal_mac(literal_domain, literal.as_bytes())
    );
}

#[test]
fn fixed_transcripts_and_mac_domains_match_independent_literal_vectors() {
    // RFC 4231 case 1: a 20-byte key padded by zero to 32 has the same
    // SHA-256 HMAC key block. This calibrates the test's direct MAC oracle.
    let mut rfc_key = [0_u8; 32];
    rfc_key[..20].fill(0x0b);
    let mut rfc = <Hmac<Sha256> as Mac>::new_from_slice(&rfc_key).unwrap();
    rfc.update(b"Hi There");
    assert_eq!(
        hex::encode(rfc.finalize().into_bytes()),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    let epoch = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
    let nonce2 = Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap();
    let client_nonce = Uuid3(Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap());
    let server_nonce = Uuid3(Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap());
    let request_id = Uuid3(Uuid::parse_str("55555555-5555-4555-8555-555555555555").unwrap());
    let binding = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let ledger = r#"{"reason":"recovery_transport_only","state":"unverified"}"#;
    let challenge = ChallengeV2 {
        protocol: 2,
        role: "enforcer".into(),
        epoch,
        nonce: nonce2,
        root_binding: binding.into(),
        offered_roles: vec![RequestedRole::StopsOnly],
        ledger_state: unverified_ledger(),
    };
    let mut negotiate = Negotiate3 {
        kind: "negotiate_v3".into(),
        protocol: 3,
        requested_role: RequestedRole::StopsOnly,
        client_nonce,
        proof: empty_proof(),
    };
    let n0 = format!(
        r#"[2,"enforcer","{epoch}","{nonce2}","{binding}",["stops_only"],{{"state":"unverified","reason":"recovery_transport_only"}},3,"stops_only","{}"]"#,
        client_nonce.0
    );
    assert_eq!(
        serde_json::to_vec(&negotiate_core(&challenge, &negotiate))
            .unwrap()
            .as_slice(),
        n0.as_bytes()
    );
    assert_eq!(
        negotiate_proof(&[0x5a; 32], &challenge, &negotiate)
            .unwrap()
            .0,
        literal_mac(b"ai-guard:enforcer-client-negotiate:v3\0", n0.as_bytes())
    );
    let n = format!(
        r#"[2,"enforcer","{epoch}","{nonce2}","{binding}",["stops_only"],{ledger},3,"stops_only","{}"]"#,
        client_nonce.0
    );
    let c = format!(
        r#"["challenge_v3",3,"enforcer","{epoch}","{binding}","stops_only","{}","{}",["stops_only"],{ledger}]"#,
        client_nonce.0, server_nonce.0
    );
    let mut offered = Challenge3 {
        kind: "challenge_v3".into(),
        protocol: 3,
        role: "enforcer".into(),
        epoch: Uuid3(epoch),
        root_binding: Hash3(binding.into()),
        requested_role: RequestedRole::StopsOnly,
        client_nonce,
        server_nonce,
        offered_roles: vec![RequestedRole::StopsOnly],
        ledger_state: ledger3(),
        session_id: empty_proof(),
        proof: empty_proof(),
    };
    let mut sid_oracle = Sha256::new();
    sid_oracle.update(b"ai-guard:enforcer-session:v3\0");
    sid_oracle.update(format!("[{n},{c}]").as_bytes());
    let sid = hex::encode(sid_oracle.finalize());
    assert_eq!(session_id(&challenge, &negotiate, &offered).unwrap().0, sid);
    offered.session_id = Hash3(sid.clone());
    assert_literal_mac(
        CHALLENGE3,
        b"ai-guard:enforcer-server-challenge:v3\0",
        &(
            negotiate_core(&challenge, &negotiate),
            offered.core(),
            &offered.session_id,
        ),
        &format!(r#"[{n},{c},"{sid}"]"#),
    );
    let authentication = Auth3 {
        kind: "auth_v3".into(),
        protocol: 3,
        session_id: Hash3(sid.clone()),
        requested_role: RequestedRole::StopsOnly,
        epoch: Uuid3(epoch),
        root_binding: Hash3(binding.into()),
        client_nonce,
        server_nonce,
        proof: empty_proof(),
    };
    let a = format!(
        r#"["auth_v3",3,"{sid}","stops_only","{epoch}","{binding}","{}","{}"]"#,
        client_nonce.0, server_nonce.0
    );
    assert_literal_mac(
        AUTH3,
        b"ai-guard:enforcer-client-auth:v3\0",
        &(
            negotiate_core(&challenge, &negotiate),
            offered.core(),
            &offered.session_id,
            authentication.core(),
        ),
        &format!(r#"[{n},{c},"{sid}",{a}]"#),
    );
    let granted = Grant3 {
        kind: "granted_v3".into(),
        protocol: 3,
        session_id: Hash3(sid.clone()),
        granted_role: RequestedRole::StopsOnly,
        epoch: Uuid3(epoch),
        root_binding: Hash3(binding.into()),
        client_nonce,
        server_nonce,
        ledger_state: ledger3(),
        admission_available: false,
        proof: empty_proof(),
    };
    let g = format!(
        r#"["granted_v3",3,"{sid}","stops_only","{epoch}","{binding}","{}","{}",{ledger},false]"#,
        client_nonce.0, server_nonce.0
    );
    assert_literal_mac(
        GRANT3,
        b"ai-guard:enforcer-server-grant:v3\0",
        &(
            negotiate_core(&challenge, &negotiate),
            offered.core(),
            &offered.session_id,
            authentication.core(),
            granted.core(),
        ),
        &format!(r#"[{n},{c},"{sid}",{a},{g}]"#),
    );
    let session = Session3 {
        key: [0x5a; 32],
        id: Hash3(sid.clone()),
        epoch,
        root_binding: Hash3(binding.into()),
        max_slots: 8,
    };
    let request = Request3 {
        kind: "request_v3".into(),
        protocol: 3,
        session_id: Hash3(sid.clone()),
        epoch: Uuid3(epoch),
        request_id,
        command: StopRequest::StatusStops {},
        proof: empty_proof(),
    };
    let q = format!(
        r#"["request_v3",3,"{sid}","{epoch}","{}",{{"kind":"status_stops"}}]"#,
        request_id.0
    );
    assert_literal_mac(
        COMMAND3,
        b"ai-guard:enforcer-client-command:v3\0",
        &(&session.id, request.core()),
        &format!(r#"["{sid}",{q}]"#),
    );
    let reply = Reply3 {
        kind: "reply_v3".into(),
        protocol: 3,
        session_id: Hash3(sid.clone()),
        epoch: Uuid3(epoch),
        request_id,
        sequence: 0,
        result: ReplyValue3::Error {
            code: EnforcerError::Unauthorized,
        },
        proof: empty_proof(),
    };
    let r = format!(
        r#"["reply_v3",3,"{sid}","{epoch}","{}",0,{{"code":"unauthorized","kind":"error"}}]"#,
        request_id.0
    );
    assert_literal_mac(
        REPLY3,
        b"ai-guard:enforcer-server-reply:v3\0",
        &(&session.id, request.core(), reply.core()),
        &format!(r#"["{sid}",{q},{r}]"#),
    );
    let status = EnforcerStopStatusV3 {
        epoch: Uuid3(epoch),
        root_binding: Hash3(binding.into()),
        granted_role: RequestedRole::StopsOnly,
        known_stop_slots: 0,
        max_slots: 8,
        progress: 1,
        transport_health: TransportHealth3::Healthy,
        clock_healthy: false,
        admission_available: false,
        ledger_state: ledger3(),
        retirement_count: unknown_retirements(),
    };
    let watch = Watch3 {
        kind: "watch_v3".into(),
        protocol: 3,
        session_id: Hash3(sid.clone()),
        epoch: Uuid3(epoch),
        request_id,
        sequence: 0,
        value: status,
        proof: empty_proof(),
    };
    let s = format!(
        r#"{{"admission_available":false,"clock_healthy":false,"epoch":"{epoch}","granted_role":"stops_only","known_stop_slots":0,"ledger_state":{ledger},"max_slots":8,"progress":1,"retirement_count":{{"reason":"ledger_unverified","state":"unknown"}},"root_binding":"{binding}","transport_health":"healthy"}}"#
    );
    let w = format!(
        r#"["watch_v3",3,"{sid}","{epoch}","{}",0,{s}]"#,
        request_id.0
    );
    assert_literal_mac(
        WATCH3,
        b"ai-guard:enforcer-server-watch:v3\0",
        &(&session.id, request.core(), watch.core()),
        &format!(r#"["{sid}",{q},{w}]"#),
    );
    negotiate.requested_role = RequestedRole::Full;
    let nf = format!(
        r#"[2,"enforcer","{epoch}","{nonce2}","{binding}",["stops_only"],{ledger},3,"full","{}"]"#,
        client_nonce.0
    );
    let refusal = Refusal3 {
        kind: "refused_v3".into(),
        protocol: 3,
        stage: RefusalStage3::Negotiate,
        requested_role: RequestedRole::Full,
        epoch: Uuid3(epoch),
        root_binding: Hash3(binding.into()),
        client_nonce,
        server_nonce,
        session_id: None,
        code: RefusalCode3::FullUnavailable,
        ledger_state: ledger3(),
        proof: empty_proof(),
    };
    let f = format!(
        r#"["refused_v3",3,"negotiate","full","{epoch}","{binding}","{}","{}",null,"full_unavailable",{ledger}]"#,
        client_nonce.0, server_nonce.0
    );
    assert_literal_mac(
        REFUSAL3,
        b"ai-guard:enforcer-server-refusal:v3\0",
        &(
            negotiate_core(&challenge, &negotiate),
            None::<()>,
            None::<()>,
            refusal.core(),
        ),
        &format!("[{nf},null,null,{f}]"),
    );
}

#[derive(Default)]
struct FragmentObservation {
    challenge_verified: bool,
    authentication_verified: bool,
    fragment_started: bool,
    grant_seen: bool,
}
async fn write_fragmented(
    stream: &mut UnixStream,
    value: &impl Serialize,
    pause: Duration,
) -> EnforcerResult<()> {
    use tokio::io::AsyncWriteExt;
    let bytes = serde_json::to_vec(value).map_err(|_| EnforcerError::InvalidInput)?;
    let length = u32::try_from(bytes.len()).map_err(|_| EnforcerError::Capacity)?;
    let midpoint = bytes.len() / 2;
    stream
        .write_all(&length.to_be_bytes())
        .await
        .map_err(|_| EnforcerError::Unavailable)?;
    stream
        .write_all(&bytes[..midpoint])
        .await
        .map_err(|_| EnforcerError::Unavailable)?;
    tokio::time::sleep(pause).await;
    stream
        .write_all(&bytes[midpoint..])
        .await
        .map_err(|_| EnforcerError::Unavailable)
}

async fn fragmented_handshake_case(
    slow_challenge: bool,
    pause: Duration,
) -> (bool, Duration, Duration, FragmentObservation, bool) {
    let fixture = AnchorFixture::new();
    let (server, mut backend) = fixture.accepted_pair().await.unwrap();
    let state = Arc::clone(&fixture.state);
    let budget = Duration::from_secs(1);
    let mut server_task = tokio::spawn(async move {
        let start = Instant::now();
        let result = if slow_challenge {
            serve_connection(server, state).await
        } else {
            // Inject a still-positive remaining portion of the same global
            // deadline, before the first preface write. No timeout is reset.
            let deadline = start + budget;
            let mut server = server;
            let challenge = challenge_for(&state);
            async {
                write_frame(&mut server, &challenge, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
                let FirstAuthentication::V3(negotiate) =
                    read_frame(&mut server, HANDSHAKE_FRAME, IO_TIMEOUT).await?
                else {
                    return Err(EnforcerError::Unauthorized);
                };
                serve(server, state, challenge, negotiate, deadline).await
            }
            .await
        };
        (result, start.elapsed())
    });
    let preface: ChallengeV2 = read_frame(&mut backend, HANDSHAKE_FRAME, IO_TIMEOUT)
        .await
        .unwrap();
    let (mut front, mut proxy) = UnixStream::pair().unwrap();
    let client = async {
        let start = Instant::now();
        let deadline = start
            + if slow_challenge {
                budget
            } else {
                HANDSHAKE_WINDOW
            };
        let result = client_handshake(
            &mut front,
            &preface,
            &[0x5a; 32],
            deadline,
            RequestedRole::StopsOnly,
            8,
        )
        .await;
        let elapsed = start.elapsed();
        drop(front);
        (result.is_ok(), elapsed)
    };
    let peer = async {
        let mut observed = FragmentObservation::default();
        let relayed = async {
            let FirstAuthentication::V3(negotiate) =
                read_frame(&mut proxy, HANDSHAKE_FRAME, IO_TIMEOUT).await?
            else {
                return Err(EnforcerError::Unauthorized);
            };
            write_frame(&mut backend, &negotiate, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
            let Handshake3::Challenge(offered) =
                read_frame(&mut backend, HANDSHAKE_FRAME, IO_TIMEOUT).await?
            else {
                return Err(EnforcerError::Unauthorized);
            };
            verify(
                &[0x5a; 32],
                CHALLENGE3,
                &(
                    negotiate_core(&preface, &negotiate),
                    offered.core(),
                    &offered.session_id,
                ),
                &offered.proof,
            )?;
            observed.challenge_verified = true;
            if slow_challenge {
                observed.fragment_started = true;
                write_fragmented(&mut proxy, &offered, pause).await?;
            } else {
                write_frame(&mut proxy, &offered, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
            }
            let Handshake3::Auth(authentication) =
                read_frame(&mut proxy, HANDSHAKE_FRAME, IO_TIMEOUT).await?
            else {
                return Err(EnforcerError::Unauthorized);
            };
            verify(
                &[0x5a; 32],
                AUTH3,
                &(
                    negotiate_core(&preface, &negotiate),
                    offered.core(),
                    &offered.session_id,
                    authentication.core(),
                ),
                &authentication.proof,
            )?;
            observed.authentication_verified = true;
            if slow_challenge {
                write_frame(&mut backend, &authentication, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
            } else {
                observed.fragment_started = true;
                write_fragmented(&mut backend, &authentication, pause).await?;
            }
            let Handshake3::Grant(granted) =
                read_frame(&mut backend, HANDSHAKE_FRAME, IO_TIMEOUT).await?
            else {
                return Err(EnforcerError::Unauthorized);
            };
            observed.grant_seen = true;
            write_frame(&mut proxy, &granted, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
            Ok::<(), EnforcerError>(())
        }
        .await;
        drop(proxy);
        drop(backend);
        (observed, relayed.is_ok())
    };
    let ((opened, client_elapsed), (observed, relayed)) = tokio::join!(client, peer);
    let server_elapsed = match tokio::time::timeout(IO_TIMEOUT, &mut server_task).await {
        Ok(Ok((_, elapsed))) => elapsed,
        _ => {
            server_task.abort();
            let _ = server_task.await;
            Duration::ZERO
        }
    };
    assert!(fixture.finish().await);
    (opened, client_elapsed, server_elapsed, observed, relayed)
}

#[tokio::test]
async fn remaining_global_budget_expires_during_fragmented_challenge_or_auth() {
    let mut observations = Vec::new();
    for challenge in [true, false] {
        let positive = fragmented_handshake_case(challenge, Duration::from_millis(20)).await;
        let expired = fragmented_handshake_case(challenge, Duration::from_millis(1200)).await;
        observations.push((challenge, positive, expired));
    }
    for (challenge, positive, expired) in observations {
        assert!(
            positive.0
                && positive.3.challenge_verified
                && positive.3.authentication_verified
                && positive.3.grant_seen
                && positive.4,
            "the authenticated fragmented positive must finish first"
        );
        assert!(
            !expired.0
                && expired.3.challenge_verified
                && expired.3.fragment_started
                && !expired.3.grant_seen
        );
        let elapsed = if challenge { expired.1 } else { expired.2 };
        assert!(
            elapsed >= Duration::from_millis(950) && elapsed < IO_TIMEOUT,
            "global deadline must expire during the frame, not at the 3s frame timeout: {elapsed:?}"
        );
        assert_eq!(
            expired.3.authentication_verified, !challenge,
            "client timeout sends no Auth3; server timeout receives only the authentic fragmented Auth3"
        );
    }
}
