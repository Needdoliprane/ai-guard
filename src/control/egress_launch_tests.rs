//! Launch-contract seam for the broker-owned egress profile (`egress_launch_*`).
//! A provider launch keeps physical network `none` and receives its proxied
//! route only through the explicit, versioned `egress` axis.

use super::LaunchSpecV1;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const HEX_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HEX_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// A valid Codex API batch launch, as authorized today on `bridge`.
fn provider_launch(network: &str) -> Value {
    json!({
        "version": 1,
        "session_id": "00000000-0000-4000-8000-000000000001",
        "command": {
            "argv": ["/usr/local/bin/ai-guard-provider", "exec", "--client", "codex",
                "--auth-mode", "api", "--transport", "batch", "--preset-version", "1",
                "--prompt", "synthetic egress launch"],
            "cwd": "/workspace"
        },
        "profile": "observe",
        "runtime_kind": "docker",
        "authority_id": "local-docker",
        "image_ref": "agentsandbox-worker:local",
        "image_id": format!("sha256:{HEX_A}"),
        "network": network,
        "cpu_millis": 2000,
        "memory_bytes": 1073741824u64,
        "pids_limit": 256,
        "timeout_seconds": 300,
        "security_preset": "fixed_sandbox_v2",
        "restart_policy": "no",
        "workspace_manifest_digest": HEX_B,
        "workspace_import_policy_version": 1,
        "provider": "codex",
        "capture_output": true,
        "credential_binding_id": "00000000-0000-4000-8000-000000000002",
        "credential_names": ["AI_GUARD_PROVIDER_BOOTSTRAP"],
        "provider_resolution": {
            "version": 1,
            "client": "codex",
            "auth_mode": "api",
            "transport": "batch",
            "profile_id": "codex-api",
            "profile_revision": 1,
            "credential_binding_id": "00000000-0000-4000-8000-000000000002",
            "credential_names": ["AI_GUARD_PROVIDER_BOOTSTRAP"],
            "invocation_preset_version": 1,
            "provenance": {"client": "cli", "auth_mode": "cli", "profile": "cli", "model": "cli"}
        }
    })
}

fn broker_egress() -> Value {
    json!({"version": 1, "profile": "broker_proxy_v1", "proxy_port": 3128})
}

fn parse(value: Value) -> anyhow::Result<LaunchSpecV1> {
    let spec: LaunchSpecV1 = serde_json::from_value(value)?;
    spec.validate()?;
    Ok(spec)
}

#[test]
fn egress_launch_provider_credentials_are_admitted_on_network_none_with_broker_egress() {
    let mut launch = provider_launch("none");
    launch["egress"] = broker_egress();
    let spec =
        parse(launch).expect("a provider launch on none + broker egress is a valid contract");
    let round_trip = serde_json::to_value(&spec).unwrap();
    assert_eq!(
        round_trip["network"], "none",
        "the physical network stays none"
    );
    assert_eq!(
        round_trip["egress"],
        broker_egress(),
        "the egress axis is retained exactly"
    );
}

#[test]
fn egress_launch_provider_credentials_stay_refused_on_network_none_without_egress() {
    assert!(parse(provider_launch("none")).is_err());
}

/// Exact historical byte form of the fixture, in the field order of the
/// launch contract before the egress axis existed.
const HISTORICAL_BRIDGE_JSON: &str = concat!(
    r#"{"version":1,"session_id":"00000000-0000-4000-8000-000000000001","#,
    r#""command":{"argv":["/usr/local/bin/ai-guard-provider","exec","--client","codex","#,
    r#""--auth-mode","api","--transport","batch","--preset-version","1","--prompt","#,
    r#""synthetic egress launch"],"cwd":"/workspace"},"profile":"observe","#,
    r#""runtime_kind":"docker","authority_id":"local-docker","#,
    r#""image_ref":"agentsandbox-worker:local","#,
    r#""image_id":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","#,
    r#""network":"bridge","cpu_millis":2000,"memory_bytes":1073741824,"pids_limit":256,"#,
    r#""timeout_seconds":300,"security_preset":"fixed_sandbox_v2","restart_policy":"no","#,
    r#""workspace_manifest_digest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","#,
    r#""workspace_import_policy_version":1,"provider":"codex","capture_output":true,"#,
    r#""credential_binding_id":"00000000-0000-4000-8000-000000000002","#,
    r#""credential_names":["AI_GUARD_PROVIDER_BOOTSTRAP"],"#,
    r#""provider_resolution":{"version":1,"client":"codex","auth_mode":"api","#,
    r#""transport":"batch","profile_id":"codex-api","profile_revision":1,"#,
    r#""credential_binding_id":"00000000-0000-4000-8000-000000000002","#,
    r#""credential_names":["AI_GUARD_PROVIDER_BOOTSTRAP"],"invocation_preset_version":1,"#,
    r#""provenance":{"client":"cli","auth_mode":"cli","profile":"cli","model":"cli"}}}"#
);

#[test]
fn egress_launch_historical_bridge_spec_keeps_its_exact_serialization_and_hash() {
    let spec = parse(provider_launch("bridge")).expect("historical bridge provider launch");
    // No egress field is invented for history: the bytes, hence every
    // historical spec_hash, stay those of the pre-egress contract.
    assert_eq!(
        serde_json::to_string(&spec).unwrap(),
        HISTORICAL_BRIDGE_JSON
    );
    let mut digest = Sha256::new();
    digest.update(b"ai-guard:launch:v1\0");
    digest.update(HISTORICAL_BRIDGE_JSON.as_bytes());
    assert_eq!(spec.spec_hash().unwrap(), hex::encode(digest.finalize()));
}

#[test]
fn egress_launch_refuses_unknown_profiles_versions_ports_and_bridge_combinations() {
    let cases = [
        (
            "bridge",
            json!({"version": 1, "profile": "broker_proxy_v1", "proxy_port": 3128}),
        ),
        (
            "none",
            json!({"version": 2, "profile": "broker_proxy_v1", "proxy_port": 3128}),
        ),
        (
            "none",
            json!({"version": 1, "profile": "transparent_v1", "proxy_port": 3128}),
        ),
        (
            "none",
            json!({"version": 1, "profile": "broker_proxy_v1", "proxy_port": 0}),
        ),
        (
            "none",
            json!({"version": 1, "profile": "broker_proxy_v1", "proxy_port": 80}),
        ),
        (
            "none",
            json!({"version": 1, "profile": "broker_proxy_v1", "proxy_port": 3128, "extra": 1}),
        ),
    ];
    for (network, egress) in cases {
        let mut launch = provider_launch(network);
        launch["egress"] = egress.clone();
        assert!(parse(launch).is_err(), "{network} {egress}");
    }
    let mut strict = provider_launch("none");
    strict["egress"] = broker_egress();
    strict["profile"] = json!("strict");
    assert!(
        parse(strict).is_err(),
        "egress requires the explicit observe profile"
    );
}
