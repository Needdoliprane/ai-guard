use super::*;
use std::io;

const CARGO_VERSION: &str = env!("CARGO_PKG_VERSION");
const GATEWAY_STATUS: &str = r#"{"schema_version":1,"product":"ai-guard","version":"0.0.1","component":"gateway","build":{"id":null,"revision":null}}"#;

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(|value| OsString::from(*value)).collect()
}

fn encoded(value: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

#[test]
fn optional_version_assertion_accepts_only_exact_cargo_version() {
    assert!(version_matches(None, "0.0.1"));
    assert!(version_matches(Some(""), "0.0.1"));
    assert!(version_matches(Some("0.0.1"), "0.0.1"));
    for supplied in ["0.1.0", "0.0.1 ", " 0.0.1", "0.0.1+release", "0.0.1\n"] {
        assert!(!version_matches(Some(supplied), "0.0.1"));
    }
}

#[test]
fn current_identity_uses_product_version_and_exact_component_names() {
    for (component, name) in [
        (Component::Gateway, "gateway"),
        (Component::Dashboard, "dashboard"),
        (Component::Agentsandbox, "agentsandbox"),
        (Component::Diagnose, "ai-guard-diagnose"),
        (Component::Provider, "ai-guard-provider"),
        (Component::DemoPki, "ai-guard-demo-pki"),
        (Component::Relay, "ai-guard-relay"),
        (Component::AuditCheckpoint, "ai-guard-audit-checkpoint"),
    ] {
        let status = BuildStatusV1::current(component);
        let bytes = serde_json::to_vec(&status).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(component.as_str(), name);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["product"], "ai-guard");
        assert_eq!(value["version"], CARGO_VERSION);
        assert_eq!(value["component"], name);
        assert_eq!(BuildStatusV1::from_json(&bytes, component).unwrap(), status);
    }
}

#[test]
fn optional_compile_metadata_is_validated_independently_without_trimming() {
    let revision = "aB".repeat(20);
    let build = BuildMetadata::from_candidates(Some("release-1.2+local_3"), Some(&revision));
    assert_eq!(build.id.as_deref(), Some("release-1.2+local_3"));
    assert_eq!(build.revision.as_deref(), Some(revision.as_str()));
    assert!(BuildMetadata::from_candidates(None, None).id.is_none());
    assert!(
        BuildMetadata::from_candidates(None, None)
            .revision
            .is_none()
    );
    for id in [
        "",
        " release",
        "release ",
        "release\n",
        "../private/path",
        "é",
        "a:b",
    ] {
        let build = BuildMetadata::from_candidates(Some(id), Some(&revision));
        assert!(build.id.is_none());
        assert_eq!(build.revision.as_deref(), Some(revision.as_str()));
    }
    let too_long = "a".repeat(65);
    assert!(
        BuildMetadata::from_candidates(Some(&too_long), None)
            .id
            .is_none()
    );
    let longest = "a".repeat(64);
    assert_eq!(
        BuildMetadata::from_candidates(Some(&longest), None)
            .id
            .as_deref(),
        Some(longest.as_str())
    );
    for revision in [
        "",
        "abcd",
        "g123456789012345678901234567890123456789",
        " aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ] {
        let build = BuildMetadata::from_candidates(Some("release"), Some(revision));
        assert_eq!(build.id.as_deref(), Some("release"));
        assert!(build.revision.is_none());
    }
    for length in [40, 64] {
        let revision = "F".repeat(length);
        assert_eq!(
            BuildMetadata::from_candidates(None, Some(&revision))
                .revision
                .as_deref(),
            Some(revision.as_str())
        );
    }
}

#[test]
fn decoder_requires_every_field_and_rejects_unknown_fields_at_each_level() {
    let base: serde_json::Value = serde_json::from_str(GATEWAY_STATUS).unwrap();
    for field in ["schema_version", "product", "version", "component", "build"] {
        let mut value = base.clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(BuildStatusV1::from_json(&encoded(&value), Component::Gateway).is_err());
    }
    for field in ["id", "revision"] {
        let mut value = base.clone();
        value["build"].as_object_mut().unwrap().remove(field);
        assert!(BuildStatusV1::from_json(&encoded(&value), Component::Gateway).is_err());
    }
    for nested in [false, true] {
        let mut value = base.clone();
        let object = if nested {
            &mut value["build"]
        } else {
            &mut value
        };
        object["secret"] = "synthetic-sensitive-value".into();
        assert!(BuildStatusV1::from_json(&encoded(&value), Component::Gateway).is_err());
    }
    for field in ["schema_version", "product", "version", "component", "build"] {
        let mut value = base.clone();
        value[field] = serde_json::Value::Null;
        assert!(BuildStatusV1::from_json(&encoded(&value), Component::Gateway).is_err());
    }
}

#[test]
fn decoder_requires_objects_instead_of_sequence_shaped_structs() {
    for input in [
        r#"[1,"ai-guard","0.0.1","gateway",{"id":null,"revision":null}]"#,
        r#"{"schema_version":1,"product":"ai-guard","version":"0.0.1","component":"gateway","build":[null,null]}"#,
    ] {
        assert!(BuildStatusV1::from_json(input.as_bytes(), Component::Gateway).is_err());
    }
}

#[test]
fn decoder_rejects_duplicates_wrong_identity_and_nonnullable_metadata_types() {
    for (needle, replacement) in [
        (
            "\"schema_version\":1",
            "\"schema_version\":1,\"schema_version\":1",
        ),
        (
            "\"build\":",
            "\"build\":{\"id\":null,\"revision\":null},\"build\":",
        ),
        ("\"id\":null", "\"id\":null,\"id\":null"),
        ("\"revision\":null", "\"revision\":null,\"revision\":null"),
    ] {
        let bytes = GATEWAY_STATUS.replacen(needle, replacement, 1);
        assert!(BuildStatusV1::from_json(bytes.as_bytes(), Component::Gateway).is_err());
    }
    let base: serde_json::Value = serde_json::from_str(GATEWAY_STATUS).unwrap();
    for (field, invalid) in [
        ("schema_version", serde_json::json!(2)),
        ("product", serde_json::json!("agentsandbox")),
        ("component", serde_json::json!("unknown")),
        ("component", serde_json::json!({"gateway":null})),
    ] {
        let mut value = base.clone();
        value[field] = invalid;
        assert!(BuildStatusV1::from_json(&encoded(&value), Component::Gateway).is_err());
    }
    assert!(BuildStatusV1::from_json(GATEWAY_STATUS.as_bytes(), Component::Dashboard).is_err());
    for field in ["id", "revision"] {
        for invalid in [
            serde_json::json!(false),
            serde_json::json!(3),
            serde_json::json!([]),
            serde_json::json!({}),
        ] {
            let mut value = base.clone();
            value["build"][field] = invalid;
            assert!(BuildStatusV1::from_json(&encoded(&value), Component::Gateway).is_err());
        }
    }
}

#[test]
fn decoder_preserves_valid_future_semver_and_rejects_malformed_or_overlong_versions() {
    let base: serde_json::Value = serde_json::from_str(GATEWAY_STATUS).unwrap();
    for version in [
        "0.0.1",
        "1.2.3-alpha.1+vendor.003",
        "2027.4.1-rc-2+build.42",
        "123456789012345678901234567890.2.3",
    ] {
        let mut value = base.clone();
        value["version"] = version.into();
        let status = BuildStatusV1::from_json(&encoded(&value), Component::Gateway).unwrap();
        assert_eq!(serde_json::to_value(status).unwrap()["version"], version);
    }
    let longest = format!("1.2.3+{}", "a".repeat(58));
    assert_eq!(longest.len(), 64);
    let mut value = base.clone();
    value["version"] = longest.into();
    assert!(BuildStatusV1::from_json(&encoded(&value), Component::Gateway).is_ok());
    for version in [
        "",
        "1",
        "1.2",
        "1.2.3.4",
        "v1.2.3",
        "01.2.3",
        "1.02.3",
        "1.2.03",
        "1.2.3-",
        "1.2.3-01",
        "1.2.3-a..b",
        "1.2.3+",
        "1.2.3+a..b",
        "1.2.3+a+b",
        "1.2.3_a",
        "1.2.3-é",
        " 1.2.3",
        "1.2.3\n",
    ] {
        let mut value = base.clone();
        value["version"] = version.into();
        assert!(BuildStatusV1::from_json(&encoded(&value), Component::Gateway).is_err());
    }
    let mut value = base;
    value["version"] = format!("1.2.3+{}", "a".repeat(59)).into();
    assert!(BuildStatusV1::from_json(&encoded(&value), Component::Gateway).is_err());
}

#[test]
fn decoder_bounds_bytes_and_refuses_invalid_remote_build_values() {
    let mut bytes = GATEWAY_STATUS.as_bytes().to_vec();
    bytes.resize(MAX_STATUS_BYTES, b' ');
    assert!(BuildStatusV1::from_json(&bytes, Component::Gateway).is_ok());
    bytes.push(b' ');
    assert!(BuildStatusV1::from_json(&bytes, Component::Gateway).is_err());
    for input in [&b""[..], &b"{not-json}"[..], &b"\xff"[..]] {
        assert!(BuildStatusV1::from_json(input, Component::Gateway).is_err());
    }
    for (field, invalid) in [
        ("id", "secret/path"),
        ("id", " leading"),
        ("revision", "not-a-revision"),
    ] {
        let mut value: serde_json::Value = serde_json::from_str(GATEWAY_STATUS).unwrap();
        value["build"][field] = invalid.into();
        let error = BuildStatusV1::from_json(&encoded(&value), Component::Gateway).unwrap_err();
        assert!(!error.to_string().contains(invalid));
    }
}

#[test]
fn version_flags_are_early_only_when_unambiguous_and_other_commands_are_untouched() {
    for component in [
        Component::Gateway,
        Component::Dashboard,
        Component::Agentsandbox,
        Component::Diagnose,
        Component::Provider,
        Component::DemoPki,
        Component::Relay,
        Component::AuditCheckpoint,
    ] {
        for flag in ["--version", "-V"] {
            let output = early_output(component, &args(&[flag])).unwrap().unwrap();
            assert!(
                output.starts_with(&format!("{} {CARGO_VERSION}\nbuild: ", component.as_str()))
            );
            assert_eq!(output.lines().count(), 3);
            for input in [
                vec![flag, "--json"],
                vec!["serve", flag],
                vec![flag, "synthetic-secret"],
                vec![flag, flag],
            ] {
                let error = early_output(component, &args(&input)).unwrap().unwrap_err();
                assert!(!error.to_string().contains("synthetic-secret"));
            }
        }
        assert!(early_output(component, &args(&[])).is_none());
        assert!(early_output(component, &args(&["serve"])).is_none());
        assert!(early_output(component, &args(&["--config", "private.json"])).is_none());
        assert!(early_output(component, &args(&["run", "--", "--version"])).is_none());
        assert!(
            early_output(component, &args(&["--version=synthetic-secret"]))
                .unwrap()
                .is_err()
        );
        assert!(
            early_output(component, &args(&["--version-synthetic-secret"]))
                .unwrap()
                .is_err()
        );
        assert!(
            early_output(component, &args(&["-Vsynthetic-secret"]))
                .unwrap()
                .is_err()
        );
    }
}

#[test]
fn agentsandbox_version_json_is_static_identity_and_rejects_extra_arguments() {
    let text = early_output(Component::Agentsandbox, &args(&["version"]))
        .unwrap()
        .unwrap();
    assert!(text.starts_with(&format!("agentsandbox {CARGO_VERSION}\nbuild: ")));
    let json = early_output(Component::Agentsandbox, &args(&["version", "--json"]))
        .unwrap()
        .unwrap();
    let status = BuildStatusV1::from_json(json.as_bytes(), Component::Agentsandbox).unwrap();
    assert_eq!(status, BuildStatusV1::current(Component::Agentsandbox));
    assert!(json.ends_with('\n'));
    for input in [
        vec!["version", "extra"],
        vec!["version", "--json", "extra"],
        vec!["version", "--json", "--json"],
    ] {
        assert!(
            early_output(Component::Agentsandbox, &args(&input))
                .unwrap()
                .is_err()
        );
    }
    assert!(early_output(Component::Gateway, &args(&["version"])).is_none());
    assert!(early_output(Component::Agentsandbox, &args(&["broker", "version"])).is_none());
}

#[test]
fn text_identity_reports_the_validated_common_build_or_explicit_unknown() {
    let status = BuildStatusV1::current(Component::Relay);
    let text = text_identity(Component::Relay);
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], format!("ai-guard-relay {CARGO_VERSION}"));
    assert_eq!(
        lines[1],
        format!("build: {}", status.build.id.as_deref().unwrap_or("unknown"))
    );
    assert_eq!(
        lines[2],
        format!(
            "revision: {}",
            status.build.revision.as_deref().unwrap_or("unknown")
        )
    );
}

#[test]
fn early_emission_reports_argument_and_output_failures_without_input_values() {
    struct BrokenOutput;
    impl io::Write for BrokenOutput {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    struct BrokenFlush;
    impl io::Write for BrokenFlush {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    assert_eq!(
        emit_early_result(Ok("agentsandbox 0.0.1\n".into()), &mut stdout, &mut stderr),
        0
    );
    assert_eq!(stdout, b"agentsandbox 0.0.1\n");
    assert!(stderr.is_empty());
    assert_eq!(
        emit_early_result(
            Err(BuildIdentityError::InvalidArguments),
            &mut stdout,
            &mut stderr
        ),
        2
    );
    assert_eq!(stderr, b"build_identity_invalid_arguments\n");
    assert_eq!(
        emit_early_result(Ok("version\n".into()), &mut BrokenOutput, &mut stderr),
        74
    );
    assert_eq!(
        emit_early_result(Ok("version\n".into()), &mut BrokenFlush, &mut stderr),
        74
    );
    assert_eq!(
        emit_early_result(
            Err(BuildIdentityError::InvalidArguments),
            &mut stdout,
            &mut BrokenOutput
        ),
        74
    );
}
