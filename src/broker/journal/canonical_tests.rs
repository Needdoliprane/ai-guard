//! Canonical bytes are a persisted contract. Keep the old implementation below
//! frozen when changing the production implementation; it is an independent
//! reference to Efq7q2, rather than a call through the implementation under test.
use super::*;

// Exact pre-optimization canonical() body from the Efq7q2 source packet.
fn reference_canonical(value: &impl Serialize) -> JournalResult<Vec<u8>> {
    fn order(value: Value) -> Value {
        match value {
            Value::Object(fields) => Value::Object(
                fields
                    .into_iter()
                    .map(|(k, v)| (k, order(v)))
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(values) => Value::Array(values.into_iter().map(order).collect()),
            other => other,
        }
    }
    serde_json::to_vec(&order(
        serde_json::to_value(value).map_err(|_| JournalError::InvalidInput)?,
    ))
    .map_err(|_| JournalError::InvalidInput)
}

#[test]
fn canonical_matches_frozen_reference_for_scalar_nested_and_typed_values() {
    let cases = vec![
        Value::Null,
        serde_json::json!(false),
        serde_json::json!(true),
        serde_json::json!(i64::MIN),
        serde_json::json!(u64::MAX),
        serde_json::json!(-0.0),
        serde_json::json!(0.0),
        serde_json::json!(2.5),
        serde_json::json!(1e30),
        serde_json::json!(1e-20),
        serde_json::json!("quote\" slash\\ control\n\0 é β 😀"),
        serde_json::json!([]),
        serde_json::json!({}),
        serde_json::json!({
            "z": [{"β": [3, null, {"é": false, "e\u{301}": true}], "a": -0.0}],
            "a": [[{"z": 2.5, "a": "\t\r\n"}], [], {}],
            "\u{10000}": "supplementary", "\u{e000}": "private-use"
        }),
    ];
    for (index, value) in cases.iter().enumerate() {
        assert_eq!(canonical(value), reference_canonical(value), "case {index}");
    }

    #[derive(Serialize)]
    struct Typed<'a> {
        z: Vec<Option<i64>>,
        a: &'a str,
        signed_zero: f64,
    }
    let typed = Typed {
        z: vec![Some(-1), None, Some(i64::MAX)],
        a: "typed serialization retains the same canonical contract",
        signed_zero: -0.0,
    };
    assert_eq!(canonical(&typed), reference_canonical(&typed));
}

#[test]
fn canonical_nested_unicode_arrays_and_escaped_strings_have_frozen_bytes() {
    let value = serde_json::json!({
        "😀": "last",
        "é": {"β": [3, 2, 1]},
        "z": false,
        "e\u{301}": "combining",
        "a": [null, {"z": 2.5, "a": -0.0}, true, 0, "line\n\0"]
    });
    // Literal expected bytes: UTF-8 keys retain str ordering, strings are not
    // Unicode-normalized, array order stays intact, and signed zero survives.
    let expected = r#"{"a":[null,{"a":-0.0,"z":2.5},true,0,"line\n\u0000"],"é":"combining","z":false,"é":{"β":[3,2,1]},"😀":"last"}"#;
    assert_eq!(canonical(&value).unwrap(), expected.as_bytes());
    assert_eq!(reference_canonical(&value).unwrap(), expected.as_bytes());
}

#[test]
fn canonical_numbers_signed_zero_and_null_have_frozen_bytes() {
    let value = serde_json::json!({
        "zero": 0.0,
        "signed_zero": -0.0,
        "null": null,
        "minimum": i64::MIN,
        "maximum": u64::MAX,
        "float": 2.5
    });
    let expected = br#"{"float":2.5,"maximum":18446744073709551615,"minimum":-9223372036854775808,"null":null,"signed_zero":-0.0,"zero":0.0}"#;
    assert_eq!(canonical(&value).unwrap(), expected);
    assert_eq!(reference_canonical(&value).unwrap(), expected);
}

#[test]
fn canonical_maps_serializer_failure_to_invalid_input() {
    struct CannotSerialize;
    impl Serialize for CannotSerialize {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom(
                "closed canonical fixture failure",
            ))
        }
    }
    assert_eq!(canonical(&CannotSerialize), Err(JournalError::InvalidInput));
    assert_eq!(
        reference_canonical(&CannotSerialize),
        Err(JournalError::InvalidInput)
    );
}
