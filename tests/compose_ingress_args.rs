#![cfg(unix)]

use axum::{Router, extract::Request, routing::any};
use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
struct ComposeServiceCommand {
    service: String,
    args: Vec<String>,
}

fn substitute_compose_vars(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let after_prefix = &rest[start + 2..];
        let Some(end) = after_prefix.find('}') else {
            output.push_str(&rest[start..]);
            return output;
        };
        let expr = &after_prefix[..end];
        let substituted = if let Some((_name, default)) = expr.split_once(":-") {
            default.to_owned()
        } else if let Some((name, _err)) = expr.split_once(":?") {
            match name {
                "AI_GUARD_LOCAL_AI_TLS_SERVER_NAME" => "llm.ai-guard.test".to_owned(),
                _ => "synthetic-required-value".to_owned(),
            }
        } else {
            match expr {
                "AI_GUARD_DEMO_IDP_PORT" => "18080".to_owned(),
                "AI_GUARD_GATEWAY_PORT" => "8787".to_owned(),
                "AI_GUARD_DASHBOARD_PORT" => "4000".to_owned(),
                "AI_GUARD_LOCAL_AI_PORT" => "18090".to_owned(),
                _ => "synthetic-value".to_owned(),
            }
        };
        output.push_str(&substituted);
        rest = &after_prefix[end + 1..];
    }
    output.push_str(rest);
    output
}

fn unquote_yaml_scalar(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1].to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn parse_inline_yaml_list(raw: &str) -> Vec<String> {
    let inner = raw
        .trim()
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .expect("inline YAML command array must close with ]");
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for ch in inner.chars() {
        match quote {
            Some(q) if ch == q => {
                quote = None;
            }
            Some(_) => current.push(ch),
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
            }
            None if ch == ',' => {
                let token = current.trim();
                if !token.is_empty() {
                    items.push(substitute_compose_vars(&unquote_yaml_scalar(token)));
                }
                current.clear();
            }
            None => current.push(ch),
        }
    }
    let token = current.trim();
    if !token.is_empty() {
        items.push(substitute_compose_vars(&unquote_yaml_scalar(token)));
    }
    items
}

fn split_shell_words(raw: &str) -> Vec<String> {
    raw.split_whitespace()
        .map(|part| substitute_compose_vars(&unquote_yaml_scalar(part)))
        .collect()
}

fn extract_compose_commands(path: &Path) -> Vec<ComposeServiceCommand> {
    let content = fs::read_to_string(path).unwrap();
    let mut results = Vec::new();
    let mut in_services = false;
    let mut current_service: Option<String> = None;
    let mut collecting_command = false;
    let mut command_indent = 0usize;
    let mut collected_args: Vec<String> = Vec::new();

    let flush = |results: &mut Vec<ComposeServiceCommand>,
                 current_service: &Option<String>,
                 collected_args: &mut Vec<String>| {
        if let Some(service) = current_service
            && !collected_args.is_empty()
        {
            results.push(ComposeServiceCommand {
                service: service.clone(),
                args: std::mem::take(collected_args),
            });
        } else {
            collected_args.clear();
        }
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent == 0 {
            if collecting_command {
                flush(&mut results, &current_service, &mut collected_args);
                collecting_command = false;
            }
            in_services = trimmed == "services:";
            current_service = None;
            continue;
        }
        if !in_services {
            continue;
        }
        if indent == 2 && trimmed.ends_with(':') {
            if collecting_command {
                flush(&mut results, &current_service, &mut collected_args);
                collecting_command = false;
            }
            current_service = Some(trimmed.trim_end_matches(':').to_owned());
            continue;
        }
        if collecting_command {
            if indent > command_indent
                && let Some(item) = trimmed.strip_prefix('-')
            {
                collected_args.push(substitute_compose_vars(&unquote_yaml_scalar(item)));
                continue;
            }
            flush(&mut results, &current_service, &mut collected_args);
            collecting_command = false;
        }
        if indent == 4
            && let Some(rest) = trimmed.strip_prefix("command:")
        {
            let rest = rest.trim();
            if rest.is_empty() {
                collecting_command = true;
                command_indent = indent;
            } else if rest.starts_with('[') {
                collected_args = parse_inline_yaml_list(rest);
                flush(&mut results, &current_service, &mut collected_args);
            } else {
                collected_args = split_shell_words(rest);
                flush(&mut results, &current_service, &mut collected_args);
            }
        }
    }
    if collecting_command {
        flush(&mut results, &current_service, &mut collected_args);
    }
    results
}

fn option_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_str())
}

fn rewrite_flag_value(args: &[String], flag: &str, replacement: &str) -> Vec<String> {
    let mut out = args.to_vec();
    for index in 0..out.len().saturating_sub(1) {
        if out[index] == flag {
            out[index + 1] = replacement.to_owned();
        }
    }
    out
}

async fn spawn_and_stop_relay(args: &[String], expected_banner_prefix: &str) -> SocketAddr {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), stderr.read_line(&mut line))
        .await
        .expect("timed out waiting for relay startup banner")
        .unwrap();
    assert!(
        line.starts_with(expected_banner_prefix),
        "unexpected banner for {args:?}: {line}"
    );
    let addr = line
        .split_whitespace()
        .find_map(|token| token.parse::<SocketAddr>().ok())
        .expect("banner must include bound SocketAddr");
    let pid = i32::try_from(child.id().unwrap()).unwrap();
    // SAFETY: pid identifies this test's unreaped child process before child.wait().
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let status = tokio::time::timeout(Duration::from_secs(9), child.wait())
        .await
        .expect("timed out waiting for clean relay shutdown")
        .unwrap();
    assert!(status.success(), "relay exited unsuccessfully: {status}");
    addr
}

#[tokio::test]
async fn all_deploy_compose_relays_match_binary_contract_and_ingress_control_separation() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let deploy_dir = manifest_dir.join("deploy");
    let mut compose_files: Vec<PathBuf> = WalkDir::new(&deploy_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("compose")
                        && (name.ends_with(".yaml") || name.ends_with(".yml"))
                })
        })
        .collect();
    compose_files.sort();
    assert!(
        compose_files.len() >= 12,
        "expected all deploy/**/compose*.yaml files, found {}",
        compose_files.len()
    );

    let mut ingress_count = 0usize;
    let mut http_relay_count = 0usize;
    let mut model_ingress_count = 0usize;

    for file in &compose_files {
        let rel = file.strip_prefix(&manifest_dir).unwrap().to_path_buf();
        let commands = extract_compose_commands(file);
        for service in commands {
            let subcommand = service.args[0].as_str();
            if service.service.contains("ingress") {
                assert!(
                    matches!(subcommand, "ingress" | "http-relay" | "model-ingress"),
                    "unexpected command {subcommand} on ingress-named service {} in {}",
                    service.service,
                    rel.display()
                );
            }
            match subcommand {
                "ingress" => {
                    ingress_count += 1;
                    assert_ne!(
                        rel,
                        Path::new("deploy/keycloak-demo/compose.yaml"),
                        "keycloak-demo must use http-relay without /control or secondary dashboard listener"
                    );
                    assert_ne!(
                        rel,
                        Path::new("deploy/local-ai/compose.yaml"),
                        "local-ai HTTP compose must use http-relay without /control or secondary dashboard listener"
                    );
                    let gateway_listen: SocketAddr =
                        option_value(&service.args, "--gateway-listen")
                            .expect("ingress requires --gateway-listen")
                            .parse()
                            .unwrap();
                    let dashboard_listen: SocketAddr =
                        option_value(&service.args, "--dashboard-listen")
                            .expect("ingress requires --dashboard-listen")
                            .parse()
                            .unwrap();
                    if !gateway_listen.ip().is_loopback() || !dashboard_listen.ip().is_loopback() {
                        assert!(
                            service.args.iter().any(|arg| arg == "--container-listen"),
                            "non-loopback ingress in {} requires --container-listen",
                            rel.display()
                        );
                    }
                    let gateway_upstream = option_value(&service.args, "--gateway-upstream")
                        .expect("ingress requires --gateway-upstream");
                    let control_upstream = option_value(&service.args, "--control-upstream")
                        .expect("ingress requires explicit --control-upstream");
                    let dashboard_upstream = option_value(&service.args, "--dashboard-upstream")
                        .expect("ingress requires --dashboard-upstream");
                    assert_ne!(
                        gateway_upstream,
                        control_upstream,
                        "ingress in {} must isolate --control-upstream from --gateway-upstream",
                        rel.display()
                    );
                    let _ = agentsandbox::ingress::ingress_router_with_control(
                        gateway_upstream,
                        control_upstream,
                    )
                    .unwrap_or_else(|err| {
                        panic!("invalid ingress origins in {}: {err}", rel.display())
                    });
                    let _ = agentsandbox::ingress::ingress_router(dashboard_upstream)
                        .unwrap_or_else(|err| {
                            panic!("invalid dashboard origin in {}: {err}", rel.display())
                        });

                    let probe_args = rewrite_flag_value(
                        &rewrite_flag_value(&service.args, "--gateway-listen", "127.0.0.1:0"),
                        "--dashboard-listen",
                        "127.0.0.1:0",
                    );
                    spawn_and_stop_relay(&probe_args, "Ingress ").await;

                    let missing_control: Vec<String> = service
                        .args
                        .iter()
                        .enumerate()
                        .filter(|(idx, arg)| {
                            *arg != "--control-upstream"
                                && (*idx == 0 || service.args[idx - 1] != "--control-upstream")
                        })
                        .map(|(_, arg)| arg.clone())
                        .collect();
                    let missing_output = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
                        .args(&missing_control)
                        .output()
                        .await
                        .unwrap();
                    assert!(
                        !missing_output.status.success(),
                        "ingress binary unexpectedly succeeded without --control-upstream"
                    );
                }
                "http-relay" => {
                    http_relay_count += 1;
                    let listen: SocketAddr = option_value(&service.args, "--listen")
                        .expect("http-relay requires --listen")
                        .parse()
                        .unwrap();
                    if !listen.ip().is_loopback() {
                        assert!(
                            service.args.iter().any(|arg| arg == "--container-listen"),
                            "non-loopback http-relay in {} requires --container-listen",
                            rel.display()
                        );
                    }
                    assert!(
                        !service.args.iter().any(|arg| arg == "--control-upstream"
                            || arg == "--dashboard-listen"
                            || arg == "--dashboard-upstream"),
                        "http-relay in {} must not accept control or dashboard flags",
                        rel.display()
                    );
                    let upstream = option_value(&service.args, "--upstream")
                        .expect("http-relay requires --upstream");
                    let _ = agentsandbox::ingress::ingress_router(upstream).unwrap_or_else(|err| {
                        panic!("invalid http-relay upstream in {}: {err}", rel.display())
                    });

                    let probe_args = rewrite_flag_value(&service.args, "--listen", "127.0.0.1:0");
                    spawn_and_stop_relay(&probe_args, "HTTP relay ").await;
                }
                "model-ingress" => {
                    model_ingress_count += 1;
                    let listen: SocketAddr = option_value(&service.args, "--listen")
                        .expect("model-ingress requires --listen")
                        .parse()
                        .unwrap();
                    if !listen.ip().is_loopback() {
                        assert!(
                            service.args.iter().any(|arg| arg == "--container-listen"),
                            "non-loopback model-ingress in {} requires --container-listen",
                            rel.display()
                        );
                    }
                    let temp = tempfile::tempdir().unwrap();
                    let missing_cert = temp.path().join("absent-cert.pem");
                    let missing_key = temp.path().join("absent-key.pem");
                    let probe_args = rewrite_flag_value(
                        &rewrite_flag_value(
                            &rewrite_flag_value(&service.args, "--listen", "127.0.0.1:0"),
                            "--tls-cert-file",
                            missing_cert.to_str().unwrap(),
                        ),
                        "--tls-key-file",
                        missing_key.to_str().unwrap(),
                    );
                    let output = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
                        .args(&probe_args)
                        .output()
                        .await
                        .unwrap();
                    assert!(!output.status.success());
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    assert_ne!(
                        output.status.code(),
                        Some(2),
                        "model-ingress arguments in {} failed CLI parsing: {stderr}",
                        rel.display()
                    );
                }
                _ => {}
            }
        }
    }

    assert_eq!(ingress_count, 4, "expected 4 control ingress services");
    assert_eq!(
        http_relay_count, 2,
        "expected 2 http-relay services (keycloak-demo and local-ai)"
    );
    assert_eq!(
        model_ingress_count, 2,
        "expected 2 model-ingress TLS services"
    );
}

#[tokio::test]
async fn http_relay_binary_forwards_http_without_mounting_control_route() {
    let upstream_app = Router::new().fallback(any(|request: Request| async move {
        format!("path={}", request.uri().path())
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_origin = format!("http://{}", listener.local_addr().unwrap());
    let upstream_task = tokio::spawn(async move {
        axum::serve(listener, upstream_app).await.unwrap();
    });

    let mut child = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args([
            "http-relay",
            "--listen",
            "127.0.0.1:0",
            "--upstream",
            &upstream_origin,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), stderr.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let addr = line
        .split_whitespace()
        .find_map(|token| token.parse::<SocketAddr>().ok())
        .unwrap();

    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let health = http
        .get(format!("http://{addr}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
    assert_eq!(health.text().await.unwrap(), "path=/health");

    let control_probe = http
        .get(format!("http://{addr}/control/api/v1/control/protocol"))
        .send()
        .await
        .unwrap();
    assert_eq!(control_probe.status(), 200);
    assert_eq!(
        control_probe.text().await.unwrap(),
        "path=/control/api/v1/control/protocol",
        "http-relay must not strip /control or mount a control router"
    );

    let pid = i32::try_from(child.id().unwrap()).unwrap();
    // SAFETY: pid identifies this test's unreaped child process before child.wait().
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let status = tokio::time::timeout(Duration::from_secs(9), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    upstream_task.abort();
}
