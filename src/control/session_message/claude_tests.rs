use super::{ClaudeObservationV1, ClaudeStreamJsonAdapterV1, ClaudeTurnOutcomeV1};
use crate::control::session_message::TurnRefusalV1;
use serde_json::{Value, json};
use uuid::Uuid;

const SESSION: &str = "aecc407c-e857-4179-818e-a546d1c35b90";
const OTHER_SESSION: &str = "aecc407c-e857-4179-818e-a546d1c35b91";
const FIRST_MESSAGE: &str = "f5af991b-3171-4c92-8484-c6edf4bc6440";
const SECOND_MESSAGE: &str = "f5af991b-3171-4c92-8484-c6edf4bc6441";
const FIRST_RESULT: &str = "bb6dd206-23a4-4666-8364-f48f5b0ed912";
const SECOND_RESULT: &str = "bb6dd206-23a4-4666-8364-f48f5b0ed913";

fn uuid(text: &str) -> Uuid {
    Uuid::parse_str(text).unwrap()
}

fn line(value: &Value) -> String {
    format!("{value}\n")
}

fn replay(message_uuid: &str, content: Value) -> Value {
    json!({
        "type": "user",
        "session_id": SESSION,
        "uuid": message_uuid,
        "parent_tool_use_id": null,
        "isReplay": true,
        "message": {"role": "user", "content": content},
    })
}

fn result(message_uuid: &str, result_uuid: &str) -> Value {
    json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "session_id": SESSION,
        "uuid": result_uuid,
        "user_message_uuid": message_uuid,
        "user_message_uuids": [message_uuid],
    })
}

#[test]
fn literal_message_waits_for_its_distinct_result_after_the_exact_echo() {
    let mut codec = ClaudeStreamJsonAdapterV1::new(uuid(SESSION)).unwrap();
    let text = "/literal @workspace  \né\u{301}\t";
    let frame = codec.issue_turn(uuid(FIRST_MESSAGE), text).unwrap();
    assert!(!frame.contains(['\n', '\r']));
    assert_eq!(
        serde_json::from_str::<Value>(&frame).unwrap(),
        json!({
            "type": "user",
            "message": {"role": "user", "content": text},
            "parent_tool_use_id": null,
            "session_id": SESSION,
            "uuid": FIRST_MESSAGE,
            "client_composed": true,
            "priority": "later",
        })
    );
    let echo = line(&replay(
        FIRST_MESSAGE,
        json!([{"type": "text", "text": text}]),
    ));
    assert_eq!(
        codec.reconcile_line(&echo).unwrap(),
        Some(ClaudeObservationV1::ExactReplay {
            message_uuid: uuid(FIRST_MESSAGE),
        })
    );
    assert_eq!(codec.reconcile_line(&echo).unwrap(), None);
    let refused = codec.issue_turn(uuid(SECOND_MESSAGE), "next").unwrap_err();
    assert_eq!(
        refused.downcast_ref::<TurnRefusalV1>(),
        Some(&TurnRefusalV1::TurnInFlight)
    );
    assert_eq!(
        codec
            .reconcile_line(&line(&result(FIRST_MESSAGE, FIRST_RESULT)))
            .unwrap(),
        Some(ClaudeObservationV1::Result {
            message_uuid: uuid(FIRST_MESSAGE),
            result_uuid: uuid(FIRST_RESULT),
            outcome: ClaudeTurnOutcomeV1::Success,
        })
    );
    codec.issue_turn(uuid(SECOND_MESSAGE), "next").unwrap();
}

#[test]
fn ambiguous_echo_and_early_result_cannot_acknowledge_or_complete() {
    let mut codec = ClaudeStreamJsonAdapterV1::new(uuid(SESSION)).unwrap();
    let text = "cafe\u{301} ";
    codec.issue_turn(uuid(FIRST_MESSAGE), text).unwrap();

    let mut foreign = replay(FIRST_MESSAGE, json!(text));
    foreign["session_id"] = json!(OTHER_SESSION);
    let mut nested = replay(FIRST_MESSAGE, json!(text));
    nested["parent_tool_use_id"] = json!("toolu_example");
    let mut injected = replay(FIRST_MESSAGE, json!(text));
    injected["origin"] = json!({"kind": "channel", "server": "fixture"});
    let mut not_a_replay = replay(FIRST_MESSAGE, json!(text));
    not_a_replay["isReplay"] = json!(false);
    for frame in [
        foreign,
        nested,
        injected,
        not_a_replay,
        replay(SECOND_MESSAGE, json!(text)),
        replay(FIRST_MESSAGE, json!("café ")),
        replay(
            FIRST_MESSAGE,
            json!([{"type": "text", "text": text}, {"type": "text", "text": ""}]),
        ),
        result(FIRST_MESSAGE, FIRST_RESULT),
    ] {
        assert_eq!(codec.reconcile_line(&line(&frame)).unwrap(), None);
    }

    assert!(codec.issue_turn(uuid(FIRST_MESSAGE), text).is_err());
    assert!(codec.issue_turn(uuid(SECOND_MESSAGE), "next").is_err());
    assert_eq!(
        codec
            .reconcile_line(&line(&replay(FIRST_MESSAGE, json!(text))))
            .unwrap(),
        Some(ClaudeObservationV1::ExactReplay {
            message_uuid: uuid(FIRST_MESSAGE),
        })
    );
}

#[test]
fn unattributed_and_conflicting_results_keep_the_turn_in_flight() {
    let mut codec = ClaudeStreamJsonAdapterV1::new(uuid(SESSION)).unwrap();
    codec.issue_turn(uuid(FIRST_MESSAGE), "first").unwrap();
    assert_eq!(
        codec
            .reconcile_line(&line(&replay(FIRST_MESSAGE, json!("first"))))
            .unwrap(),
        Some(ClaudeObservationV1::ExactReplay {
            message_uuid: uuid(FIRST_MESSAGE),
        })
    );

    // Older output supplies only its own UUID. Neither order nor a lexical
    // match between that UUID and the input is an explicit attribution.
    let mut unlinked = result(FIRST_MESSAGE, FIRST_RESULT);
    unlinked
        .as_object_mut()
        .unwrap()
        .remove("user_message_uuid");
    unlinked
        .as_object_mut()
        .unwrap()
        .remove("user_message_uuids");
    let mut coincidental_uuid = unlinked.clone();
    coincidental_uuid["uuid"] = json!(FIRST_MESSAGE);
    let mut merged = result(FIRST_MESSAGE, FIRST_RESULT);
    merged["user_message_uuids"] = json!([SECOND_MESSAGE, FIRST_MESSAGE]);
    let mut conflicting = result(FIRST_MESSAGE, FIRST_RESULT);
    conflicting["user_message_uuids"] = json!([SECOND_MESSAGE]);
    let mut foreign = result(FIRST_MESSAGE, FIRST_RESULT);
    foreign["session_id"] = json!(OTHER_SESSION);
    let mut autonomous = result(FIRST_MESSAGE, FIRST_RESULT);
    autonomous["origin"] = json!({"kind": "task-notification"});
    let mut local_command = result(FIRST_MESSAGE, FIRST_RESULT);
    local_command["local_command"] = json!("clear");
    let mut malformed = result(FIRST_MESSAGE, FIRST_RESULT);
    malformed["is_error"] = json!(true);
    for frame in [
        unlinked,
        coincidental_uuid,
        merged,
        conflicting,
        foreign,
        autonomous,
        local_command,
        malformed,
        result(SECOND_MESSAGE, FIRST_RESULT),
        result(FIRST_MESSAGE, FIRST_MESSAGE),
        result(FIRST_MESSAGE, "00000000-0000-0000-0000-000000000000"),
    ] {
        assert_eq!(codec.reconcile_line(&line(&frame)).unwrap(), None);
        assert!(codec.issue_turn(uuid(SECOND_MESSAGE), "next").is_err());
    }

    let mut explicitly_linked = result(FIRST_MESSAGE, FIRST_RESULT);
    explicitly_linked
        .as_object_mut()
        .unwrap()
        .remove("user_message_uuids");
    assert_eq!(
        codec.reconcile_line(&line(&explicitly_linked)).unwrap(),
        Some(ClaudeObservationV1::Result {
            message_uuid: uuid(FIRST_MESSAGE),
            result_uuid: uuid(FIRST_RESULT),
            outcome: ClaudeTurnOutcomeV1::Success,
        })
    );
}

#[test]
fn stale_frames_do_not_complete_the_next_turn_with_its_own_error_result() {
    let mut codec = ClaudeStreamJsonAdapterV1::new(uuid(SESSION)).unwrap();
    codec.issue_turn(uuid(FIRST_MESSAGE), "first").unwrap();
    let first_echo = line(&replay(FIRST_MESSAGE, json!("first")));
    let first_result = line(&result(FIRST_MESSAGE, FIRST_RESULT));
    assert!(codec.reconcile_line(&first_echo).unwrap().is_some());
    assert!(codec.reconcile_line(&first_result).unwrap().is_some());

    codec.issue_turn(uuid(SECOND_MESSAGE), "second").unwrap();
    assert_eq!(codec.reconcile_line(&first_echo).unwrap(), None);
    assert_eq!(
        codec
            .reconcile_line(&line(&replay(SECOND_MESSAGE, json!("second"))))
            .unwrap(),
        Some(ClaudeObservationV1::ExactReplay {
            message_uuid: uuid(SECOND_MESSAGE),
        })
    );
    assert_eq!(codec.reconcile_line(&first_result).unwrap(), None);

    let mut error = result(SECOND_MESSAGE, SECOND_RESULT);
    error["is_error"] = json!(true);
    error["subtype"] = json!("error_during_execution");
    let error = line(&error);
    assert_eq!(
        codec.reconcile_line(&error).unwrap(),
        Some(ClaudeObservationV1::Result {
            message_uuid: uuid(SECOND_MESSAGE),
            result_uuid: uuid(SECOND_RESULT),
            outcome: ClaudeTurnOutcomeV1::ExecutionError,
        })
    );
    assert_eq!(codec.reconcile_line(&error).unwrap(), None);
}

#[test]
fn malformed_or_incomplete_jsonl_never_advances_the_turn() {
    let mut codec = ClaudeStreamJsonAdapterV1::new(uuid(SESSION)).unwrap();
    codec.issue_turn(uuid(FIRST_MESSAGE), "first").unwrap();
    let echo = replay(FIRST_MESSAGE, json!("first"));
    let repeated = format!(
        "{{\"session_id\":\"{OTHER_SESSION}\",{}\n",
        echo.to_string().strip_prefix('{').unwrap(),
    );
    let oversized = format!(
        "{}{}\n",
        " ".repeat(crate::control::session_message::MAX_FRAME_BYTES),
        echo,
    );
    for malformed in [echo.to_string(), repeated, oversized, "[]\n".to_owned()] {
        assert!(codec.reconcile_line(&malformed).is_err());
    }
    assert_eq!(
        codec
            .reconcile_line(&line(&result(FIRST_MESSAGE, FIRST_RESULT)))
            .unwrap(),
        None
    );
    assert!(codec.issue_turn(uuid(SECOND_MESSAGE), "next").is_err());
    assert!(codec.reconcile_line(&line(&echo)).unwrap().is_some());
}

#[test]
fn divergent_session_init_fences_the_codec() {
    let mut codec = ClaudeStreamJsonAdapterV1::new(uuid(SESSION)).unwrap();
    codec.issue_turn(uuid(FIRST_MESSAGE), "first").unwrap();
    let foreign_init = line(&json!({
        "type": "system",
        "subtype": "init",
        "session_id": OTHER_SESSION,
    }));
    assert!(codec.reconcile_line(&foreign_init).is_err());
    assert!(
        codec
            .reconcile_line(&line(&replay(FIRST_MESSAGE, json!("first"))))
            .is_err()
    );
    assert!(codec.issue_turn(uuid(SECOND_MESSAGE), "next").is_err());
}
