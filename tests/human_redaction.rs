mod support;
use agentsandbox::{
    audit::Store,
    identity::generate_local_credential,
    model::{CommandSpec, Event},
    redaction::Redactor,
};
use serde_json::json;

// A different person's recognizable local credential must be masked before
// storage even when it is absent from the literal redaction dictionary.
#[test]
fn unknown_local_credentials_are_masked_in_text_json_commands_and_audit() {
    let token = generate_local_credential().unwrap();
    let redactor = Redactor::new(vec![]);
    let text = format!("prefix{token}suffix");
    let masked = redactor.text(&text);
    assert!(!masked.contains(&token));
    assert!(masked.contains("[REDACTED]"));
    let value = redactor.value(&json!({"nested":[{"message":text}]}));
    assert!(!serde_json::to_string(&value).unwrap().contains(&token));
    let files = support::private_tempdir();
    let store = Store::open(&files.path().join("audit.sqlite"), redactor, 1000).unwrap();
    let mut event = Event::new("fixture.human.secret");
    event.command = Some(CommandSpec {
        argv: vec!["program".into(), token.clone()],
        cwd: "/workspace".into(),
    });
    event.data = json!({"nested":{"note":token}});
    let stored = store.append(event).unwrap();
    assert!(!serde_json::to_string(&stored).unwrap().contains(&token));
    assert!(store.integrity().unwrap().valid);
}

// Literal masking remains necessary for a credential truncated by capture,
// even though a generic complete-token rule also exists.
#[test]
fn current_local_bearer_is_masked_before_truncated_capture_is_persisted() {
    let token = generate_local_credential().unwrap();
    let redactor = Redactor::new(vec![token.clone()]);
    let prefix = &token.as_bytes()[..48];
    let masked = redactor.captured_text(prefix, true);
    assert!(
        masked == "[REDACTED]",
        "partial local credential must be hidden"
    );
    assert!(!masked.contains("aigh1_"));
}
