//! Conversation adapter regressions. The correlation layer is exercised
//! directly; the framing is exercised against a real stdio fixture child (the
//! test binary re-executed as a controlled JSON-RPC peer). No provider,
//! credential or network is involved: real CLI reception stays NOT_RUN.
use super::*;
use serde_json::json;

fn response(rpc_id: u64, turn_id: &str) -> Value {
    json!({"id":rpc_id,"result":{"turn":{"id":turn_id}}})
}
fn completed(turn_id: &str) -> Value {
    json!({"method":"turn/completed","params":{"turn":{"id":turn_id,"status":"completed"}}})
}

#[test]
fn valid_receipt_acknowledges_only_the_exact_issued_rpc() {
    let mut adapter = ConversationAdapterV1::new();
    let request = adapter
        .issue_turn(uuid::Uuid::new_v4(), "thread-1", "first instruction")
        .unwrap();
    assert_eq!(request.rpc_id, 1);
    // The exact correlated typed response acknowledges.
    let observation = adapter
        .reconcile_response(&response(request.rpc_id, "turn-a"))
        .unwrap();
    assert_eq!(
        observation,
        AckObservationV1::Acknowledged {
            turn_id: "turn-a".into()
        }
    );
    assert_eq!(
        adapter.pending().unwrap().acknowledged_turn_id.as_deref(),
        Some("turn-a")
    );
    // The correlated outcome closes the turn.
    let outcome = adapter.reconcile_outcome(&completed("turn-a")).unwrap();
    assert!(matches!(outcome, OutcomeObservationV1::Completed { .. }));
    assert!(adapter.pending().unwrap().completed);
}

#[test]
fn response_accepts_absent_or_exact_optional_jsonrpc_version() {
    let absent = response(7, "turn-7");
    assert!(matches!(
        observe_response(&absent, 7),
        AckObservationV1::Acknowledged { .. }
    ));
    let mut compatible = absent.clone();
    compatible["jsonrpc"] = json!("2.0");
    assert!(matches!(
        observe_response(&compatible, 7),
        AckObservationV1::Acknowledged { .. }
    ));
    compatible["jsonrpc"] = json!("1.0");
    assert_eq!(
        observe_response(&compatible, 7),
        AckObservationV1::NotAResponse
    );
}

#[test]
fn discordant_id_never_acknowledges() {
    let mut adapter = ConversationAdapterV1::new();
    let request = adapter
        .issue_turn(uuid::Uuid::new_v4(), "thread-1", "instruction")
        .unwrap();
    let observation = adapter
        .reconcile_response(&response(request.rpc_id + 999, "turn-other"))
        .unwrap();
    assert_eq!(observation, AckObservationV1::DiscordantId);
    assert!(adapter.pending().unwrap().acknowledged_turn_id.is_none());
}

#[test]
fn divergent_provenance_never_completes_the_turn() {
    let mut adapter = ConversationAdapterV1::new();
    let request = adapter
        .issue_turn(uuid::Uuid::new_v4(), "thread-1", "instruction")
        .unwrap();
    adapter.reconcile_response(&response(request.rpc_id, "turn-a"));
    let outcome = adapter
        .reconcile_outcome(&completed("turn-someone-else"))
        .unwrap();
    assert_eq!(outcome, OutcomeObservationV1::DivergentProvenance);
    assert!(!adapter.pending().unwrap().completed);
}

#[test]
fn duplicate_receipt_and_outcome_are_ignored() {
    let mut adapter = ConversationAdapterV1::new();
    let request = adapter
        .issue_turn(uuid::Uuid::new_v4(), "thread-1", "instruction")
        .unwrap();
    assert!(
        adapter
            .reconcile_response(&response(request.rpc_id, "turn-a"))
            .is_some()
    );
    // A second identical response is a duplicate, not a new acknowledgement.
    assert!(
        adapter
            .reconcile_response(&response(request.rpc_id, "turn-a"))
            .is_none()
    );
    assert!(adapter.reconcile_outcome(&completed("turn-a")).is_some());
    assert!(adapter.reconcile_outcome(&completed("turn-a")).is_none());
}

#[test]
fn only_one_turn_is_in_flight() {
    let mut adapter = ConversationAdapterV1::new();
    adapter
        .issue_turn(uuid::Uuid::new_v4(), "thread-1", "first")
        .unwrap();
    let refusal = adapter
        .issue_turn(uuid::Uuid::new_v4(), "thread-1", "second")
        .unwrap_err();
    assert!(refusal.is::<TurnRefusalV1>());
    // Closing the turn frees the channel for the next instruction.
    adapter.close_turn();
    let second = adapter
        .issue_turn(uuid::Uuid::new_v4(), "thread-1", "second")
        .unwrap();
    assert_eq!(second.rpc_id, 2);
}

#[test]
fn rejected_and_malformed_responses_do_not_acknowledge() {
    let mut adapter = ConversationAdapterV1::new();
    let request = adapter
        .issue_turn(uuid::Uuid::new_v4(), "thread-1", "instruction")
        .unwrap();
    let error = json!({"id":request.rpc_id,"error":{"code":-1,"message":"no"}});
    assert_eq!(
        adapter.reconcile_response(&error).unwrap(),
        AckObservationV1::Rejected
    );
    let malformed = json!({"id":request.rpc_id,"result":{"turn":{}}});
    assert_eq!(
        adapter.reconcile_response(&malformed).unwrap(),
        AckObservationV1::Malformed
    );
    assert!(adapter.pending().unwrap().acknowledged_turn_id.is_none());
}

#[test]
fn frame_is_bounded_and_carries_the_exact_text() {
    let request = TurnRequestV1 {
        rpc_id: 7,
        thread_id: "thread-1".into(),
        text: "synthetic instruction".into(),
    };
    let frame = request.frame().unwrap();
    let value: Value = serde_json::from_str(&frame).unwrap();
    assert_eq!(value["method"], "turn/start");
    assert!(value.get("jsonrpc").is_none());
    assert_eq!(value["id"], 7);
    assert_eq!(value["params"]["threadId"], "thread-1");
    assert_eq!(value["params"]["input"][0]["text"], "synthetic instruction");
    // An over-bound instruction is refused before any write.
    let too_big = TurnRequestV1 {
        rpc_id: 8,
        thread_id: "thread-1".into(),
        text: "x".repeat(crate::control::session_control::SESSION_MESSAGE_MAX_CONTENT_BYTES + 1),
    };
    assert!(too_big.frame().is_err());
}

// --- stdio fixture child ---

/// The controlled JSON-RPC peer. Re-executed by tests via the test binary; it
/// supports the Codex initialize/thread handshake and synthetic turns. It is
/// a fixture, not a provider.
#[test]
#[ignore = "stdio fixture child, re-executed by the conversation tests"]
fn conversation_fixture_child() {
    use std::io::BufRead;
    let mode = std::env::var("AI_GUARD_CONV_FIXTURE_MODE").unwrap_or_else(|_| "normal".into());
    let native_thread_id = format!("fixture-thread-{}", std::process::id());
    let mut saw_jsonrpc_header = false;
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        saw_jsonrpc_header |= value.get("jsonrpc").is_some();
        match value.get("method").and_then(Value::as_str) {
            Some("initialized") => continue,
            Some("initialize") => {
                if let Some(rpc_id) = value.get("id").and_then(Value::as_u64) {
                    if saw_jsonrpc_header {
                        println!(
                            "{}",
                            json!({"id":rpc_id,"error":{"code":-32600,"message":"unexpected jsonrpc field"}})
                        );
                    } else {
                        println!("{}", json!({"id":rpc_id,"result":{}}));
                    }
                }
                continue;
            }
            Some("thread/start") => {
                if let Some(rpc_id) = value.get("id").and_then(Value::as_u64) {
                    if saw_jsonrpc_header {
                        println!(
                            "{}",
                            json!({"id":rpc_id,"error":{"code":-32600,"message":"unexpected jsonrpc field"}})
                        );
                    } else if mode.starts_with("expect-thread-id=") {
                        println!(
                            "{}",
                            json!({"id":rpc_id,"error":{"code":-32602,"message":"thread evidence unavailable"}})
                        );
                    } else {
                        println!(
                            "{}",
                            json!({"id":rpc_id,"result":{"thread":{"id":native_thread_id}}})
                        );
                    }
                }
                continue;
            }
            Some("turn/start") => {}
            _ => continue,
        }
        let Some(rpc_id) = value.get("id").and_then(Value::as_u64) else {
            continue;
        };
        if saw_jsonrpc_header {
            println!(
                "{}",
                json!({"id":rpc_id,"error":{"code":-32600,"message":"unexpected jsonrpc field"}})
            );
            continue;
        }
        if mode == "native-thread"
            && value.pointer("/params/threadId").and_then(Value::as_str)
                != Some(native_thread_id.as_str())
        {
            println!(
                "{}",
                json!({"id":rpc_id,"error":{"code":-32602,"message":"native thread not used"}})
            );
            continue;
        }
        if let Some(expected) = mode.strip_prefix("expect-thread-id=")
            && value.pointer("/params/threadId").and_then(Value::as_str) != Some(expected)
        {
            println!(
                "{}",
                json!({
                    "id": rpc_id,
                    "error": {"code": -32602, "message": "unexpected fixture thread"}
                })
            );
            continue;
        }
        match mode.as_str() {
            "cut" => {
                // Read the request (possible emission) then drop the channel
                // without any reconcilable response.
                std::process::exit(0);
            }
            "unterminated" => {
                let mut stdout = std::io::stdout().lock();
                writeln!(stdout, "\nfixture received the request").unwrap();
                serde_json::to_writer(&mut stdout, &response(rpc_id, "incomplete-turn")).unwrap();
                stdout.flush().unwrap();
                std::process::exit(0);
            }
            "discordant" => {
                println!(
                    "{}",
                    serde_json::to_string(&response(rpc_id + 999, "turn-other")).unwrap()
                );
            }
            "divergent" => {
                println!(
                    "{}",
                    serde_json::to_string(&response(rpc_id, &format!("turn-{rpc_id}"))).unwrap()
                );
                println!(
                    "{}",
                    serde_json::to_string(&completed("turn-someone-else")).unwrap()
                );
            }
            "duplicate" => {
                let ack =
                    serde_json::to_string(&response(rpc_id, &format!("turn-{rpc_id}"))).unwrap();
                println!("{ack}");
                println!("{ack}");
                let done = serde_json::to_string(&completed(&format!("turn-{rpc_id}"))).unwrap();
                println!("{done}");
                println!("{done}");
            }
            _ => {
                println!(
                    "{}",
                    serde_json::to_string(&response(rpc_id, &format!("turn-{rpc_id}"))).unwrap()
                );
                println!(
                    "{}",
                    serde_json::to_string(&completed(&format!("turn-{rpc_id}"))).unwrap()
                );
            }
        }
    }
}

#[test]
fn stdio_thread_id_is_learned_from_and_used_on_the_same_child() {
    let mut child = spawn_fixture("native-thread");
    let thread_id = child.codex_thread_proof().unwrap().thread_id().to_owned();
    assert!(thread_id.starts_with("fixture-thread-"));
    let mut adapter = ConversationAdapterV1::new();
    let request = adapter
        .issue_turn(uuid::Uuid::new_v4(), &thread_id, "synthetic instruction")
        .unwrap();
    child.write_frame(&request.frame().unwrap()).unwrap();
    let turn = next_json(&mut child);
    assert_eq!(turn["id"], request.rpc_id);
    assert!(matches!(
        adapter.reconcile_response(&turn),
        Some(AckObservationV1::Acknowledged { .. })
    ));
}

/// Spawn the fixture child with its own mode in the child environment (no
/// parent-process mutation, so concurrent tests never race on the mode).
fn spawn_fixture(mode: &str) -> StdioConversation {
    let exe = std::env::current_exe().unwrap();
    let args: Vec<std::ffi::OsString> = vec![
        "--exact".into(),
        "control::session_message::tests::conversation_fixture_child".into(),
        "--ignored".into(),
        "--nocapture".into(),
    ];
    let env = [(
        std::ffi::OsString::from("AI_GUARD_CONV_FIXTURE_MODE"),
        std::ffi::OsString::from(mode),
    )];
    StdioConversation::spawn_with_env(exe.as_os_str(), &args, &env).unwrap()
}

#[test]
fn dropping_stdio_reaps_the_owned_fixture_child() {
    use std::time::{Duration, Instant};

    let channel = spawn_fixture("normal");
    let pid = channel.child.as_ref().unwrap().id() as libc::id_t;
    drop(channel);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // SAFETY: waitid writes into info. WNOWAIT observes without taking
        // the exit status from the retirement owner.
        let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::uninit();
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid,
                info.as_mut_ptr(),
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                break;
            }
            assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        }
        assert!(Instant::now() < deadline, "fixture child was not reaped");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Read the next JSON object frame, skipping harness noise, within a deadline.
fn next_json(child: &mut StdioConversation) -> Value {
    let deadline = std::time::Duration::from_secs(10);
    loop {
        let line = child
            .read_line(deadline)
            .expect("a frame within the deadline");
        if let Ok(value) = serde_json::from_str::<Value>(line.trim()) {
            if value.is_object() {
                return value;
            }
        }
    }
}

#[test]
fn stdio_two_turns_acknowledge_and_complete_in_order() {
    let mut child = spawn_fixture("normal");
    let mut adapter = ConversationAdapterV1::new();
    for text in [
        "first synthetic instruction",
        "second synthetic instruction",
    ] {
        let request = adapter
            .issue_turn(uuid::Uuid::new_v4(), "thread-1", text)
            .unwrap();
        child.write_frame(&request.frame().unwrap()).unwrap();
        let ack = next_json(&mut child);
        assert_eq!(
            adapter.reconcile_response(&ack).unwrap(),
            AckObservationV1::Acknowledged {
                turn_id: format!("turn-{}", request.rpc_id)
            }
        );
        let outcome = next_json(&mut child);
        assert!(matches!(
            adapter.reconcile_outcome(&outcome).unwrap(),
            OutcomeObservationV1::Completed { .. }
        ));
        adapter.close_turn();
    }
    child.close();
}

#[test]
fn stdio_discordant_id_leaves_delivery_unacknowledged() {
    let mut child = spawn_fixture("discordant");
    let mut adapter = ConversationAdapterV1::new();
    let request = adapter
        .issue_turn(uuid::Uuid::new_v4(), "thread-1", "instruction")
        .unwrap();
    child.write_frame(&request.frame().unwrap()).unwrap();
    let ack = next_json(&mut child);
    assert_eq!(
        adapter.reconcile_response(&ack).unwrap(),
        AckObservationV1::DiscordantId
    );
    assert!(adapter.pending().unwrap().acknowledged_turn_id.is_none());
    child.close();
}

#[test]
fn stdio_cut_after_possible_emission_keeps_delivery_unknown() {
    let mut child = spawn_fixture("cut");
    let mut adapter = ConversationAdapterV1::new();
    let request = adapter
        .issue_turn(uuid::Uuid::new_v4(), "thread-1", "instruction")
        .unwrap();
    // The complete write succeeds (possible emission)...
    child.write_frame(&request.frame().unwrap()).unwrap();
    // ...but the channel closes with no reconcilable response. Delivery stays
    // unknown: no acknowledgement, and the adapter never resends.
    let deadline = std::time::Duration::from_secs(10);
    let closed = loop {
        match child.read_line(deadline) {
            Ok(line) => {
                if serde_json::from_str::<Value>(line.trim()).is_ok() {
                    panic!("the cut fixture must not answer");
                }
            }
            Err(error) => break error,
        }
    };
    assert_eq!(closed.kind(), std::io::ErrorKind::UnexpectedEof);
    // The reader already delivered EOF and disconnected. A later read is
    // still terminal, independently of its wait budget.
    assert_eq!(
        child.read_line(deadline).unwrap_err().kind(),
        std::io::ErrorKind::UnexpectedEof
    );
    assert!(adapter.pending().unwrap().acknowledged_turn_id.is_none());
    child.close();
}

#[test]
fn stdio_requires_a_delimiter_before_delivering_a_response() {
    let mut child = spawn_fixture("unterminated");
    let mut adapter = ConversationAdapterV1::new();
    let request = adapter
        .issue_turn(uuid::Uuid::new_v4(), "thread-1", "instruction")
        .unwrap();
    child.write_frame(&request.frame().unwrap()).unwrap();

    let deadline = Duration::from_secs(10);
    let mut received_request = false;
    loop {
        match child.read_line(deadline) {
            Ok(line) => {
                assert!(line.ends_with('\n'));
                received_request |= line == "fixture received the request\n";
            }
            Err(error) => {
                assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
                assert_eq!(
                    error.to_string(),
                    "conversation response ended before the frame delimiter"
                );
                break;
            }
        }
    }
    assert!(received_request);
    child.close();
}

#[test]
fn stdio_divergent_provenance_does_not_complete_the_turn() {
    let mut child = spawn_fixture("divergent");
    let mut adapter = ConversationAdapterV1::new();
    let request = adapter
        .issue_turn(uuid::Uuid::new_v4(), "thread-1", "instruction")
        .unwrap();
    child.write_frame(&request.frame().unwrap()).unwrap();
    let ack = next_json(&mut child);
    assert!(matches!(
        adapter.reconcile_response(&ack).unwrap(),
        AckObservationV1::Acknowledged { .. }
    ));
    let outcome = next_json(&mut child);
    assert_eq!(
        adapter.reconcile_outcome(&outcome).unwrap(),
        OutcomeObservationV1::DivergentProvenance
    );
    assert!(!adapter.pending().unwrap().completed);
    child.close();
}

#[test]
fn stdio_duplicate_frames_do_not_double_acknowledge() {
    let mut child = spawn_fixture("duplicate");
    let mut adapter = ConversationAdapterV1::new();
    let request = adapter
        .issue_turn(uuid::Uuid::new_v4(), "thread-1", "instruction")
        .unwrap();
    child.write_frame(&request.frame().unwrap()).unwrap();
    let first = next_json(&mut child);
    assert!(adapter.reconcile_response(&first).is_some());
    let second = next_json(&mut child);
    assert!(adapter.reconcile_response(&second).is_none());
    let done = next_json(&mut child);
    assert!(adapter.reconcile_outcome(&done).is_some());
    let done_again = next_json(&mut child);
    assert!(adapter.reconcile_outcome(&done_again).is_none());
    child.close();
}
