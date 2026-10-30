use agentsandbox::{
    adapters::{AdapterContext, parse_redacted_capture, parse_redacted_line},
    redaction::Redactor,
};
use serde_json::json;

#[tokio::test]
async fn runtime_capture_boundary_never_keeps_a_known_secret_fragment() {
    let secret = "Q".repeat(4097);
    let output = secret.repeat(64);
    let captured = agentsandbox::runtime::capture_stream(output.as_bytes(), 256 * 1024)
        .await
        .unwrap();
    assert!(captured.truncated);
    assert_eq!(captured.total_bytes, 262_208);
    let redactor = Redactor::new(vec![secret]);
    let masked = agentsandbox::redaction::clip(
        &redactor.captured_text(&captured.bytes, captured.truncated),
        8192,
    );
    assert!(
        !masked.contains("QQQQQQQQ"),
        "a known secret fragment survived the runtime capture boundary"
    );
}

#[test]
fn capture_masking_keeps_useful_text_and_only_hides_unfinished_known_secrets() {
    let redactor = Redactor::new(vec!["opaque-provider-credential".into()]);
    for (input, truncated, expected) in [
        (
            "useful report\nopaque-provider-credentialopaque-provider",
            true,
            "useful report\n[REDACTED]",
        ),
        (
            "useful report\nopaque-provider",
            false,
            "useful report\nopaque-provider",
        ),
        ("useful report\nordinary", true, "useful report\nordinary"),
        ("", true, ""),
    ] {
        assert!(redactor.captured_text(input.as_bytes(), truncated) == expected);
    }
}

#[test]
fn unfinished_secret_bytes_are_masked_before_lossy_utf8_decoding() {
    let redactor = Redactor::new(vec!["épisode-private-value".into()]);
    let mut input = b"useful report\n".to_vec();
    input.push(0xc3); // Capture stops within the first character of the secret.
    assert!(redactor.captured_text(&input, true) == "useful report\n[REDACTED]");
    assert!(redactor.captured_text(&input, false) == "useful report\n�");
    let mut input = b"useful report\n".to_vec();
    input.extend_from_slice("épisode-pri".as_bytes());
    assert!(redactor.captured_text(&input, true) == "useful report\n[REDACTED]");
}

#[test]
fn capture_suffix_matching_handles_multiple_secrets_and_prefix_fallbacks() {
    let redactor = Redactor::new(vec![
        "unrelated-provider-value".into(),
        "ababababac-provider-value".into(),
    ]);
    assert!(redactor.captured_text(b"useful ababababa", true) == "useful [REDACTED]");
    assert!(redactor.captured_text(b"useful unrelated-pro", true) == "useful [REDACTED]");
}

#[test]
fn a_complete_short_secret_cannot_hide_a_longer_unfinished_secret() {
    let redactor = Redactor::new(vec!["aaaa".into(), "aaaabbbbbbbb".into()]);
    let masked = redactor.captured_text(b"useful aaaabbbb", true);
    assert!(
        masked == "useful [REDACTED]",
        "a complete short match obscured a longer unfinished secret"
    );
}

#[test]
fn extending_a_capture_dictionary_preserves_all_secrets_before_masking() {
    let original = Redactor::new(vec!["aaaabbbbbbbb".into()]);
    let combined = original.with_secrets(["aaaa".into(), "private-runtime-value".into()]);
    let masked = combined.captured_text(b"useful private-runtime-value aaaabbbb", true);
    assert!(
        masked == "useful [REDACTED] [REDACTED]",
        "private and caller dictionaries were not applied together"
    );
    let unchanged = original.text("private-runtime-value");
    assert!(
        unchanged == "private-runtime-value",
        "extending a dictionary mutated its original values"
    );
}

#[test]
fn complete_overlapping_secret_occurrences_are_masked_to_their_last_byte() {
    let long = "ab".repeat(2048);
    for (secret, input) in [
        ("abab".to_owned(), "useful abababX".to_owned()),
        (long.clone(), format!("useful {long}{}X", "ab".repeat(2047))),
    ] {
        let masked = Redactor::new(vec![secret]).captured_text(input.as_bytes(), true);
        assert!(
            masked == "useful [REDACTED]X",
            "an overlapping complete secret left an unmasked fragment"
        );
    }
}

#[test]
fn provider_capture_only_accepts_delimited_records_when_truncated() {
    let context = AdapterContext {
        session_id: "session".into(),
        project_id: "project".into(),
        sandbox_id: "sandbox".into(),
        agent_id: "agent".into(),
        correlation_id: "correlation".into(),
        trace_id: "trace".into(),
    };
    let redactor = Redactor::new(vec![]);
    let line = json!({"type":"agentsandbox.telemetry","version":1,"kind":"file.declared","data":{"note":"useful"}}).to_string();
    // A valid-looking final prefix still has no record delimiter. The capture
    // flag says that further bytes existed, so only the first line is complete.
    let captured = format!("{line}\n{line}");
    let normalized =
        parse_redacted_capture("generic", captured.as_bytes(), true, &context, &redactor);
    assert_eq!(
        normalized.events.len(),
        1,
        "an undelimited truncated record was accepted"
    );
    assert!(normalized.unterminated_final_line_omitted);
    let complete =
        parse_redacted_capture("generic", captured.as_bytes(), false, &context, &redactor);
    assert_eq!(complete.events.len(), 2);
    assert!(!complete.unterminated_final_line_omitted);
}

#[test]
fn truncated_provider_capture_keeps_escaped_json_records_useful() {
    let context = AdapterContext {
        session_id: "session".into(),
        project_id: "project".into(),
        sandbox_id: "sandbox".into(),
        agent_id: "agent".into(),
        correlation_id: "correlation".into(),
        trace_id: "trace".into(),
    };
    // This literal also occurs as JSON syntax for another field. Masking raw
    // JSON bytes would break the record instead of masking its decoded value.
    let secret = r#""message":"useful\\path""#;
    let line = json!({"type":"agentsandbox.telemetry","version":1,"kind":"file.declared","data":{"message":r"useful\path","credential_note":secret}}).to_string();
    assert!(line.contains(secret));
    let mut captured = format!("{line}\n{{\"unfinished\":\"").into_bytes();
    captured.push(0xc3);
    let normalized = parse_redacted_capture(
        "generic",
        &captured,
        true,
        &context,
        &Redactor::new(vec![secret.into()]),
    );
    assert!(normalized.unterminated_final_line_omitted);
    assert_eq!(normalized.events.len(), 1);
    assert_eq!(normalized.events[0]["data"]["message"], r"useful\path");
    assert_eq!(
        normalized.events[0]["data"]["credential_note"],
        "[REDACTED]"
    );
    let delimited = parse_redacted_capture(
        "generic",
        format!("{line}\n").as_bytes(),
        true,
        &context,
        &Redactor::new(vec![secret.into()]),
    );
    assert_eq!(delimited.events.len(), 1);
    assert!(!delimited.unterminated_final_line_omitted);
}

#[test]
fn known_secrets_in_object_keys_are_masked_before_serialization() {
    let secret = "a-generated-canary-value-for-json-keys";
    let redacted = Redactor::new(vec![secret.into()]).value(&json!({secret: {"message":"safe"}}));
    assert!(!redacted.to_string().contains(secret));
    assert_eq!(redacted["[REDACTED]"]["message"], "safe");
}

#[test]
fn decoded_escaped_secrets_are_masked_before_adapter_projection() {
    let secret = "canary\"with\\json-escapes";
    let context = AdapterContext {
        session_id: "session".into(),
        project_id: "project".into(),
        sandbox_id: "sandbox".into(),
        agent_id: "agent".into(),
        correlation_id: "correlation".into(),
        trace_id: "trace".into(),
    };
    let line = json!({"type":"agentsandbox.telemetry","version":1,"kind":"file.declared","data":{"note":format!("{}{secret}", "x".repeat(4073))}}).to_string();
    let events = parse_redacted_line(
        "generic",
        &line,
        &context,
        &Redactor::new(vec![secret.into()]),
    );
    assert_eq!(
        events.len(),
        1,
        "valid escaped telemetry must remain usable"
    );
    let note = events[0]["data"]["note"].as_str().unwrap();
    assert!(note.ends_with("[REDACTED]"));
    assert!(!note.contains("canary"));
}
