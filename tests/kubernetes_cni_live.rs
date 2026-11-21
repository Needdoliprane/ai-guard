//! Opt-in observations on a local Docker Desktop cluster.
#[path = "kubernetes_cni_live/workspace_pending.rs"]
mod workspace_pending;

use agentsandbox::runtime::capture_stream;
use serde_json::{Value, json};
use std::{
    env,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    process::{Child, Command},
    time::{sleep, timeout},
};
use uuid::Uuid;

const CONTEXT: &str = "docker-desktop";
const OWNER: &str = "qa.ai-guard.local/cni-owner";
const LANE: &str = "qa.ai-guard.local/cni-lane";
const CORE_CONFIG: &str = "qa.ai-guard.local/corefile";
const COREDNS_IMAGE: &str = "registry.k8s.io/coredns/coredns:v1.14.2";
const COREDNS_EXEC_CAP: &str = "NET_BIND_SERVICE";
const POD: &str = "qa-dns";
const DNS_PORT: u16 = 1053;
const CANARY: &str = "canary.probe.qa.invalid";
const HEALTH: &str = "canary.health.qa.invalid";
const CANARY_IP: [u8; 4] = [192, 0, 2, 42];
const HEALTH_IP: [u8; 4] = [192, 0, 2, 43];
type Result<T> = std::result::Result<T, &'static str>;

struct Kubectl {
    executable: PathBuf,
    endpoint: Option<String>,
}

impl Kubectl {
    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command.env_clear();
        for variable in ["HOME", "KUBECONFIG"] {
            if let Some(value) = env::var_os(variable) {
                command.env(variable, value);
            }
        }
        command
            .args(["--context", CONTEXT])
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(endpoint) = &self.endpoint {
            command.arg(format!("--server={endpoint}"));
        }
        command
    }

    async fn call_raw(&self, args: &[&str], body: Option<&Value>, limit: usize) -> Result<Vec<u8>> {
        let mut child = self
            .command()
            .arg("--request-timeout=7s")
            .args(args)
            .stdin(if body.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|_| "kubectl_spawn")?;
        timeout(Duration::from_secs(12), async move {
            if let Some(body) = body {
                let bytes = serde_json::to_vec(body).map_err(|_| "request_json")?;
                child
                    .stdin
                    .take()
                    .ok_or("kubectl_stdin")?
                    .write_all(&bytes)
                    .await
                    .map_err(|_| "kubectl_stdin")?;
            }
            let stdout = child.stdout.take().ok_or("kubectl_stdout")?;
            let (output, status) = tokio::join!(capture_stream(stdout, limit), child.wait());
            if !status.map_err(|_| "kubectl_wait")?.success() {
                return Err("kubectl_nonzero_exit");
            }
            let output = output.map_err(|_| "kubectl_stdout")?;
            if output.truncated {
                return Err("kubectl_output_limit");
            }
            Ok(output.bytes)
        })
        .await
        .map_err(|_| "kubectl_timeout")?
    }

    async fn call(&self, args: &[&str], body: Option<&Value>) -> Result<Value> {
        let output = self.call_raw(args, body, 4 * 1024 * 1024).await?;
        if output.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&output).map_err(|_| "kubectl_output_json")
    }

    async fn create(&self, object: &Value) -> Result<Value> {
        self.call(&["create", "-f", "-", "-o", "json"], Some(object))
            .await
    }

    async fn delete_uid(&self, path: &str, uid: Uuid) -> Result<()> {
        let options = json!({
            "apiVersion": "v1", "kind": "DeleteOptions", "preconditions": {"uid": uid},
        });
        self.call(&["delete", "--raw", path, "-f", "-"], Some(&options))
            .await?;
        Ok(())
    }
}

fn literal_loopback(endpoint: &str) -> bool {
    let endpoint = endpoint.strip_suffix('/').unwrap_or(endpoint);
    endpoint
        .strip_prefix("https://127.0.0.1:")
        .or_else(|| endpoint.strip_prefix("https://[::1]:"))
        .is_some_and(|port| {
            port.parse::<u16>()
                .is_ok_and(|n| n != 0 && n.to_string() == port)
        })
}

async fn verified_kubectl(opt_in: &str) -> Result<Kubectl> {
    if env::var(opt_in).as_deref() != Ok("1") {
        return Err("opt_in_required");
    }
    let executable = PathBuf::from(env::var_os("AI_GUARD_QA_KUBECTL").ok_or("kubectl_required")?);
    if !executable.is_absolute() || !executable.is_file() {
        return Err("kubectl_absolute_file_required");
    }
    let mut kube = Kubectl {
        executable,
        endpoint: None,
    };
    let config = kube
        .call(&["config", "view", "--minify", "-o", "json"], None)
        .await?;
    if config["contexts"][0]["name"] != CONTEXT {
        return Err("docker_desktop_context_required");
    }
    let cluster = &config["clusters"][0]["cluster"];
    let user = &config["users"][0]["user"];
    let endpoint = cluster["server"]
        .as_str()
        .filter(|endpoint| literal_loopback(endpoint))
        .ok_or("literal_https_loopback_required")?;
    if cluster["insecure-skip-tls-verify"] == true
        || !cluster["proxy-url"].is_null()
        || !user["exec"].is_null()
        || !user["auth-provider"].is_null()
    {
        return Err("proxy_plugin_or_unverified_tls_not_supported");
    }
    kube.endpoint = Some(endpoint.into());
    Ok(kube)
}

fn uid(object: &Value) -> Result<Uuid> {
    object["metadata"]["uid"]
        .as_str()
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| !value.is_nil())
        .ok_or("invalid_resource_uid")
}

fn resource_version(object: &Value) -> Result<&str> {
    object["metadata"]["resourceVersion"]
        .as_str()
        .filter(|value| {
            !value.is_empty() && value.len() <= 24 && value.bytes().all(|b| b.is_ascii_digit())
        })
        .ok_or("invalid_resource_version")
}

fn ready(pod: &Value) -> bool {
    pod["status"]["phase"] == "Running"
        && pod["status"]["conditions"]
            .as_array()
            .is_some_and(|conditions| {
                conditions
                    .iter()
                    .any(|item| item["type"] == "Ready" && item["status"] == "True")
            })
}

struct CoreDns {
    image: String,
    image_id: String,
    node: String,
}

async fn loaded_coredns(kube: &Kubectl) -> Result<CoreDns> {
    let list = kube
        .call(
            &[
                "get",
                "pods",
                "-n",
                "kube-system",
                "-l",
                "k8s-app=kube-dns",
                "-o",
                "json",
            ],
            None,
        )
        .await?;
    for pod in list["items"]
        .as_array()
        .ok_or("coredns_inventory_missing")?
    {
        if !ready(pod) {
            continue;
        }
        let image = pod["spec"]["containers"]
            .as_array()
            .and_then(|containers| {
                containers
                    .iter()
                    .find(|container| container["name"] == "coredns")
            })
            .and_then(|container| container["image"].as_str());
        let image_id = pod["status"]["containerStatuses"]
            .as_array()
            .and_then(|containers| {
                containers
                    .iter()
                    .find(|container| container["name"] == "coredns")
            })
            .and_then(|container| container["imageID"].as_str());
        if let (Some(image), Some(image_id), Some(node)) =
            (image, image_id, pod["spec"]["nodeName"].as_str())
        {
            let digest = image_id.rsplit_once("sha256:").map(|(_, value)| value);
            if digest.is_some_and(|value| {
                value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
            }) && image == COREDNS_IMAGE
                && !node.is_empty()
            {
                return Ok(CoreDns {
                    image: image.into(),
                    image_id: image_id.into(),
                    node: node.into(),
                });
            }
        }
    }
    Err("ready_locally_loaded_coredns_v1_14_2_required")
}

struct Namespace {
    name: String,
    owner: String,
    lane: &'static str,
    uid: Option<Uuid>,
}

impl Namespace {
    fn new(owner: &str, lane: &'static str) -> Self {
        Self {
            name: format!("aig-v1-cni-{lane}-{owner}"),
            owner: owner.into(),
            lane,
            uid: None,
        }
    }

    fn selector(&self) -> Value {
        json!({ OWNER: self.owner, LANE: self.lane })
    }

    async fn create(&mut self, kube: &Kubectl) -> Result<()> {
        let mut labels = self.selector();
        labels["pod-security.kubernetes.io/enforce"] = json!("restricted");
        labels["pod-security.kubernetes.io/enforce-version"] = json!("v1.36");
        let object = kube
            .create(&json!({"apiVersion":"v1","kind":"Namespace",
            "metadata":{"name":self.name,"labels":labels}}))
            .await?;
        if !self.owns(&object) {
            return Err("namespace_create_identity_mismatch");
        }
        self.uid = Some(uid(&object)?);
        Ok(())
    }

    fn owns(&self, object: &Value) -> bool {
        object["metadata"]["name"] == self.name
            && object["metadata"]["labels"][OWNER] == self.owner
            && object["metadata"]["labels"][LANE] == self.lane
    }

    async fn cleanup(&self, kube: &Kubectl) -> Result<()> {
        // No mutation for an object whose creation or retained UID is uncertain.
        let Some(expected_uid) = self.uid else {
            return Err("namespace_ownership_unconfirmed");
        };
        let get = [
            "get",
            "namespace",
            &self.name,
            "--ignore-not-found",
            "-o",
            "json",
        ];
        let current = kube.call(&get, None).await?;
        if current.is_null() {
            return Ok(());
        }
        if !self.owns(&current) || uid(&current)? != expected_uid {
            return Err("namespace_cleanup_identity_mismatch");
        }
        kube.delete_uid(&format!("/api/v1/namespaces/{}", self.name), expected_uid)
            .await?;
        let deadline = Instant::now() + Duration::from_secs(35);
        while Instant::now() < deadline {
            let current = kube.call(&get, None).await?;
            if current.is_null() {
                return Ok(());
            }
            if uid(&current)? != expected_uid {
                return Err("namespace_cleanup_identity_changed");
            }
            sleep(Duration::from_secs(1)).await;
        }
        Err("namespace_cleanup_pending")
    }
}

fn dns_pod(namespace: &Namespace, source: &CoreDns, corefile: &str) -> Value {
    // The v1.14.2 image sets cap_net_bind_service on its /coredns entrypoint.
    json!({"apiVersion":"v1","kind":"Pod",
    "metadata":{"name":POD,"namespace":namespace.name,"labels":namespace.selector(),
        "annotations":{CORE_CONFIG:corefile}},
    "spec":{
        "restartPolicy":"Never","activeDeadlineSeconds":420,"terminationGracePeriodSeconds":1,
        "automountServiceAccountToken":false,"enableServiceLinks":false,
        "dnsPolicy":"None","dnsConfig":{"nameservers":["127.0.0.1"]},
        "nodeSelector":{"kubernetes.io/hostname":source.node},
        "securityContext":{"runAsNonRoot":true,"runAsUser":65532,"runAsGroup":65532,
            "seccompProfile":{"type":"RuntimeDefault"}},
        "containers":[{"name":"coredns","image":source.image,"imagePullPolicy":"Never",
            "args":["-conf","/qa/Corefile"],"ports":[{"containerPort":DNS_PORT,"protocol":"TCP"}],
            "securityContext":{"readOnlyRootFilesystem":true,"allowPrivilegeEscalation":false,
                "capabilities":{"drop":["ALL"],"add":[COREDNS_EXEC_CAP]}},
            "resources":{"requests":{"cpu":"25m","memory":"24Mi"},
                "limits":{"cpu":"200m","memory":"128Mi"}},
            "readinessProbe":{"tcpSocket":{"port":DNS_PORT},"periodSeconds":1},
            "volumeMounts":[{"name":"corefile","mountPath":"/qa","readOnly":true}]}],
        "volumes":[{"name":"corefile","downwardAPI":{"items":[{"path":"Corefile",
            "fieldRef":{"fieldPath":format!("metadata.annotations['{CORE_CONFIG}']")}}]}}]
    }})
}

struct ProbePod {
    uid: Uuid,
    ip: Ipv4Addr,
}

async fn get_pod(kube: &Kubectl, namespace: &Namespace) -> Result<Value> {
    kube.call(
        &["get", "pod", POD, "-n", &namespace.name, "-o", "json"],
        None,
    )
    .await
}

fn inspect_pod(
    pod: &Value,
    namespace: &Namespace,
    source: &CoreDns,
    expected_uid: Uuid,
) -> Result<ProbePod> {
    let spec = &pod["spec"];
    let container = &spec["containers"][0];
    let pod_security = &spec["securityContext"];
    let container_security = &container["securityContext"];
    if !ready(pod)
        || uid(pod)? != expected_uid
        || pod["metadata"]["name"] != POD
        || pod["metadata"]["namespace"] != namespace.name
        || pod["metadata"]["labels"][OWNER] != namespace.owner
        || pod["metadata"]["labels"][LANE] != namespace.lane
        || pod["spec"]["nodeName"] != source.node
        || pod["status"]["containerStatuses"][0]["imageID"] != source.image_id
    {
        return Err("pod_readiness_image_node_or_identity_changed");
    }
    if !spec["containers"]
        .as_array()
        .is_some_and(|items| items.len() == 1)
        || spec["initContainers"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
        || spec["ephemeralContainers"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
        || !spec["volumes"]
            .as_array()
            .is_some_and(|items| items.len() == 1 && items[0]["downwardAPI"].is_object())
        || spec["automountServiceAccountToken"] != false
        || !matches!(spec.get("hostNetwork"), None | Some(Value::Bool(false)))
        || spec["hostPID"] == true
        || spec["hostIPC"] == true
        || pod_security["runAsNonRoot"] != true
        || pod_security["runAsUser"] != 65532
        || pod_security["runAsGroup"] != 65532
        || pod_security["seccompProfile"]["type"] != "RuntimeDefault"
        || container_security["readOnlyRootFilesystem"] != true
        || container_security["allowPrivilegeEscalation"] != false
        || container_security["capabilities"]["drop"] != json!(["ALL"])
        || container_security["capabilities"]["add"] != json!([COREDNS_EXEC_CAP])
        || container["image"] != COREDNS_IMAGE
        || container["imagePullPolicy"] != "Never"
        || container["args"] != json!(["-conf", "/qa/Corefile"])
        || container["ports"][0]["hostPort"]
            .as_u64()
            .unwrap_or_default()
            != 0
    {
        return Err("admitted_pod_scope_changed");
    }
    let host_ip = pod["status"]["hostIP"]
        .as_str()
        .and_then(|ip| ip.parse::<IpAddr>().ok())
        .ok_or("pod_host_address_missing")?;
    let ip = pod["status"]["podIP"]
        .as_str()
        .and_then(|ip| ip.parse::<Ipv4Addr>().ok())
        .filter(|ip| ip.is_private() && IpAddr::V4(*ip) != host_ip)
        .ok_or("private_ipv4_pod_address_distinct_from_host_required")?;
    Ok(ProbePod {
        uid: expected_uid,
        ip,
    })
}

async fn start_pod(
    kube: &Kubectl,
    namespace: &Namespace,
    source: &CoreDns,
    corefile: &str,
    report: &mut Value,
) -> Result<ProbePod> {
    report["stage"] = json!(format!("{}_pod_create", namespace.lane));
    let created = kube.create(&dns_pod(namespace, source, corefile)).await?;
    let expected_uid = uid(&created)?;
    report["stage"] = json!(format!("{}_pod_readiness", namespace.lane));
    let deadline = Instant::now() + Duration::from_secs(55);
    while Instant::now() < deadline {
        let pod = get_pod(kube, namespace).await?;
        if uid(&pod)? != expected_uid
            || pod["metadata"]["name"] != POD
            || pod["metadata"]["namespace"] != namespace.name
            || pod["metadata"]["labels"][OWNER] != namespace.owner
            || pod["metadata"]["labels"][LANE] != namespace.lane
        {
            return Err("pod_startup_identity_changed");
        }
        let status = &pod["status"];
        let container = status["containerStatuses"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["name"] == "coredns"));
        let absent = Value::Null;
        let state = container.map_or(&absent, |container| &container["state"]);
        let phase = match status["phase"].as_str() {
            Some("Pending") => "pending",
            Some("Running") => "running",
            Some("Succeeded") => "succeeded",
            Some("Failed") => "failed",
            _ => "unknown",
        };
        let evidence = &mut report["pod_startup"][namespace.lane];
        *evidence = json!({
            "pod_uid":expected_uid, "phase":phase,
            "pod_reason":pod_startup_reason(&status["reason"]),
            "waiting_reason":pod_startup_reason(&state["waiting"]["reason"]),
            "terminated_reason":pod_startup_reason(&state["terminated"]["reason"]),
            "exit_code":state["terminated"]["exitCode"].as_u64().filter(|code| *code <= 255),
            "signal":state["terminated"]["signal"].as_u64().filter(|signal| *signal > 0 && *signal <= 64),
            "restart_count":container.and_then(|item| item["restartCount"].as_u64()),
            "scheduled":pod_condition(status, "PodScheduled"),
            "ready":pod_condition(status, "Ready"),
        });
        if ready(&pod) {
            return inspect_pod(&pod, namespace, source, expected_uid);
        }
        if state["waiting"]["reason"] == "ErrImageNeverPull" {
            return Err("coredns_image_not_available_with_never_pull");
        }
        if state["terminated"].is_object() || matches!(phase, "failed" | "succeeded") {
            if state["terminated"].is_object() {
                let message = state["terminated"]["message"].as_str().unwrap_or_default();
                if coredns_exec_denied(message) {
                    evidence["coredns_termination_diagnostic"] =
                        json!(CoreDnsLogDiagnostic::ExecOperationNotPermitted);
                }
                evidence["coredns_log_diagnostic"] =
                    json!(closed_coredns_log(kube, namespace).await);
            }
            return Err("coredns_pod_ended_before_readiness");
        }
        sleep(Duration::from_secs(1)).await;
    }
    Err("pod_readiness_deadline")
}

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum PodStartupReason {
    Missing,
    ContainerCreating,
    ErrImageNeverPull,
    ErrImagePull,
    ImagePullBackOff,
    CreateContainerConfigError,
    CreateContainerError,
    RunContainerError,
    ContainerCannotRun,
    CrashLoopBackOff,
    Error,
    OomKilled,
    Completed,
    Evicted,
    DeadlineExceeded,
    Other,
}

fn pod_startup_reason(reason: &Value) -> PodStartupReason {
    use PodStartupReason::*;
    match reason.as_str() {
        None => Missing,
        Some("ContainerCreating") => ContainerCreating,
        Some("ErrImageNeverPull") => ErrImageNeverPull,
        Some("ErrImagePull") => ErrImagePull,
        Some("ImagePullBackOff") => ImagePullBackOff,
        Some("CreateContainerConfigError") => CreateContainerConfigError,
        Some("CreateContainerError") => CreateContainerError,
        Some("RunContainerError") => RunContainerError,
        Some("ContainerCannotRun") => ContainerCannotRun,
        Some("CrashLoopBackOff") => CrashLoopBackOff,
        Some("Error") => Error,
        Some("OOMKilled") => OomKilled,
        Some("Completed") => Completed,
        Some("Evicted") => Evicted,
        Some("DeadlineExceeded") => DeadlineExceeded,
        Some(_) => Other,
    }
}

fn pod_condition(status: &Value, name: &str) -> &'static str {
    match status["conditions"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["type"] == name))
        .and_then(|item| item["status"].as_str())
    {
        Some("True") => "true",
        Some("False") => "false",
        _ => "unknown",
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum CoreDnsLogDiagnostic {
    Unavailable,
    Empty,
    CorefileUnreadable,
    CorefileParseFailed,
    ListenerBindFailed,
    ExecOperationNotPermitted,
    Other,
}

fn coredns_exec_denied(message: &str) -> bool {
    message.contains("exec")
        && message.contains("/coredns")
        && message.contains("operation not permitted")
}

async fn closed_coredns_log(kube: &Kubectl, namespace: &Namespace) -> CoreDnsLogDiagnostic {
    use CoreDnsLogDiagnostic::*;
    let answer = timeout(
        Duration::from_secs(5),
        kube.command()
            .args([
                "--request-timeout=3s",
                "logs",
                POD,
                "-n",
                &namespace.name,
                "-c",
                "coredns",
                "--tail=8",
                "--limit-bytes=2048",
            ])
            .stdin(Stdio::null())
            .output(),
    )
    .await;
    let Ok(Ok(output)) = answer else {
        return Unavailable;
    };
    if !output.status.success() || output.stdout.len() > 2048 {
        return Unavailable;
    }
    let log = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    if log.trim().is_empty() {
        return Empty;
    }
    if coredns_exec_denied(&log) {
        return ExecOperationNotPermitted;
    }
    if log.contains("/qa/corefile")
        && (log.contains("no such file") || log.contains("permission denied"))
    {
        return CorefileUnreadable;
    }
    if log.contains("unknown directive")
        || log.contains("error during parsing")
        || log.contains("unexpected token")
    {
        return CorefileParseFailed;
    }
    if log.contains("listen ")
        && (log.contains("address already in use") || log.contains("permission denied"))
    {
        return ListenerBindFailed;
    }
    Other
}

fn policy(
    namespace: &Namespace,
    name: &str,
    ingress: Option<Value>,
    egress: Option<Value>,
) -> Value {
    let mut spec = json!({"podSelector":{"matchLabels":namespace.selector()},"policyTypes":[]});
    let mut axes = Vec::new();
    if let Some(rules) = ingress {
        axes.push("Ingress");
        spec["ingress"] = rules;
    }
    if let Some(rules) = egress {
        axes.push("Egress");
        spec["egress"] = rules;
    }
    spec["policyTypes"] = json!(axes);
    json!({"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy",
        "metadata":{"name":name,"namespace":namespace.name,"labels":namespace.selector()},"spec":spec})
}

fn peer(namespace: &Namespace) -> Value {
    json!({"namespaceSelector":{"matchLabels":namespace.selector()},
        "podSelector":{"matchLabels":namespace.selector()}})
}

async fn create_policy(kube: &Kubectl, requested: &Value) -> Result<Value> {
    let admitted = kube.create(requested).await?;
    let spec = &admitted["spec"];
    if admitted["kind"] != "NetworkPolicy"
        || ["name", "namespace", "labels"]
            .iter()
            .any(|key| admitted["metadata"][*key] != requested["metadata"][*key])
        || spec["podSelector"] != requested["spec"]["podSelector"]
        || spec["policyTypes"] != requested["spec"]["policyTypes"]
        || ["ingress", "egress"].iter().any(|key| {
            let rules = &requested["spec"][*key];
            // The API may omit an explicitly empty list.
            !(spec[*key] == *rules
                || spec[*key].is_null() && rules.as_array().is_some_and(Vec::is_empty))
        })
    {
        return Err("admitted_network_policy_changed");
    }
    Ok(admitted)
}

async fn remove_allow(
    kube: &Kubectl,
    namespace: &Namespace,
    name: &str,
    original: &Value,
) -> Result<()> {
    let current = kube
        .call(
            &[
                "get",
                "networkpolicy",
                name,
                "-n",
                &namespace.name,
                "-o",
                "json",
            ],
            None,
        )
        .await?;
    if uid(&current)? != uid(original)? || current["metadata"]["labels"][OWNER] != namespace.owner {
        return Err("policy_cleanup_identity_mismatch");
    }
    let path = format!(
        "/apis/networking.k8s.io/v1/namespaces/{}/networkpolicies/{name}",
        namespace.name
    );
    kube.delete_uid(&path, uid(original)?).await?;
    let current = kube
        .call(
            &[
                "get",
                "networkpolicy",
                name,
                "-n",
                &namespace.name,
                "--ignore-not-found",
                "-o",
                "json",
            ],
            None,
        )
        .await?;
    if current.is_null() {
        Ok(())
    } else {
        Err("policy_removal_not_confirmed")
    }
}

struct Forward {
    child: Child,
    port: u16,
    _stdout: BufReader<tokio::process::ChildStdout>,
}

impl Forward {
    async fn start(kube: &Kubectl, namespace: &Namespace) -> Result<Self> {
        let mut child = kube
            .command()
            .args([
                "port-forward",
                "--address",
                "127.0.0.1",
                "-n",
                &namespace.name,
                "pod/qa-dns",
            ])
            .arg(format!(":{DNS_PORT}"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|_| "port_forward_spawn")?;
        let mut stdout = BufReader::new(child.stdout.take().ok_or("port_forward_stdout")?);
        let port = timeout(Duration::from_secs(12), async {
            let mut first = String::new();
            stdout
                .read_line(&mut first)
                .await
                .map_err(|_| "port_forward_read")?;
            first
                .trim()
                .strip_prefix("Forwarding from 127.0.0.1:")
                .and_then(|line| line.strip_suffix(" -> 1053"))
                .and_then(|value| value.parse::<u16>().ok())
                .filter(|port| *port != 0)
                .ok_or("port_forward_not_ready")
        })
        .await
        .map_err(|_| "port_forward_deadline")??;
        Ok(Self {
            child,
            port,
            _stdout: stdout,
        })
    }

    fn ensure_running(&mut self) -> Result<()> {
        if self
            .child
            .try_wait()
            .map_err(|_| "port_forward_wait")?
            .is_some()
        {
            return Err("port_forward_stopped");
        }
        Ok(())
    }

    async fn close(mut self) {
        let _ = self.child.start_kill();
        let _ = timeout(Duration::from_secs(3), self.child.wait()).await;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DnsResult {
    Answer,
    ServiceFailure,
    ResponseTimeout,
}

impl DnsResult {
    fn label(self) -> &'static str {
        match self {
            Self::Answer => "answer",
            Self::ServiceFailure => "servfail",
            Self::ResponseTimeout => "response_timeout",
        }
    }
}

fn question(name: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for label in name.split('.') {
        bytes.push(label.len() as u8);
        bytes.extend_from_slice(label.as_bytes());
    }
    bytes.extend_from_slice(&[0, 0, 1, 0, 1]); // root, A, IN
    bytes
}

fn read_dns_answer(
    data: &[u8],
    id: [u8; 2],
    question: &[u8],
    expected: [u8; 4],
) -> Result<DnsResult> {
    if data.len() < 12
        || data[..2] != id
        || data[2] & 0xfa != 0x80
        || data[4..6] != [0, 1]
        || data.get(12..12 + question.len()) != Some(question)
    {
        return Err("dns_response_question_or_transaction_mismatch");
    }
    let count = u16::from_be_bytes([data[6], data[7]]);
    if data[3] & 15 == 2 && count == 0 {
        return Ok(DnsResult::ServiceFailure);
    }
    if data[3] & 15 != 0 || count != 1 {
        return Err("unexpected_dns_response_code_or_count");
    }
    let record = &data[12 + question.len()..];
    let name = if record.starts_with(&[0xc0, 0x0c]) {
        2
    } else {
        let encoded_name = &question[..question.len() - 4];
        if record.starts_with(encoded_name) {
            encoded_name.len()
        } else {
            return Err("dns_answer_name_mismatch");
        }
    };
    let fields = record.get(name..name + 14).ok_or("dns_answer_too_short")?;
    if fields[..4] != [0, 1, 0, 1] || fields[8..10] != [0, 4] || fields[10..14] != expected {
        return Err("dns_answer_not_the_expected_canary");
    }
    Ok(DnsResult::Answer)
}

async fn dns(forward: &mut Forward, name: &str, expected: [u8; 4]) -> Result<DnsResult> {
    forward.ensure_running()?;
    let mut socket = timeout(
        Duration::from_secs(2),
        TcpStream::connect((Ipv4Addr::LOCALHOST, forward.port)),
    )
    .await
    .map_err(|_| "loopback_connect_timeout")?
    .map_err(|_| "loopback_connect_error")?;
    let id: [u8; 2] = Uuid::new_v4().as_bytes()[..2]
        .try_into()
        .map_err(|_| "dns_transaction_id")?;
    let question = question(name);
    let mut packet = Vec::new();
    packet.extend_from_slice(&((12 + question.len()) as u16).to_be_bytes());
    packet.extend_from_slice(&id);
    packet.extend_from_slice(&[1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    packet.extend_from_slice(&question);
    timeout(Duration::from_secs(2), socket.write_all(&packet))
        .await
        .map_err(|_| "loopback_write_timeout")?
        .map_err(|_| "loopback_write_error")?;
    // CoreDNS may spend up to 30 seconds dialing the upstream on a dropped TCP SYN.
    let response = timeout(Duration::from_secs(34), async {
        let len = socket.read_u16().await.map_err(|_| "loopback_read_error")? as usize;
        if !(12..=1024).contains(&len) {
            return Err("dns_response_length");
        }
        let mut bytes = vec![0; len];
        socket
            .read_exact(&mut bytes)
            .await
            .map_err(|_| "loopback_read_error")?;
        read_dns_answer(&bytes, id, &question, expected)
    })
    .await;
    forward.ensure_running()?;
    response.unwrap_or(Ok(DnsResult::ResponseTimeout))
}

async fn measure(
    client: &mut Forward,
    server: &mut Forward,
    phase: &str,
    allowed: bool,
    samples: &mut Vec<Value>,
) -> Result<()> {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(110);
    let mut consecutive = 0;
    let mut ambiguous = false;
    while Instant::now() < deadline {
        let result = dns(client, CANARY, CANARY_IP).await?;
        if dns(client, HEALTH, HEALTH_IP).await? != DnsResult::Answer
            || dns(server, CANARY, CANARY_IP).await? != DnsResult::Answer
        {
            return Err("direct_controls_unavailable");
        }
        samples.push(
            json!({"phase":phase,"at":chrono::Utc::now(),"peer_tcp_dns":result.label(),
            "client_control":"answer","server_control":"answer"}),
        );
        // A read timeout at the host is evidence of uncertainty, not network denial.
        let expected = if allowed {
            DnsResult::Answer
        } else {
            DnsResult::ServiceFailure
        };
        if result == expected {
            consecutive += 1;
        } else {
            consecutive = 0;
            ambiguous |= result == DnsResult::ResponseTimeout;
        }
        if consecutive == 3 {
            return Ok(());
        }
        if consecutive == 0 && !ambiguous && started.elapsed() >= Duration::from_secs(22) {
            break;
        }
        sleep(Duration::from_secs(1)).await;
    }
    if ambiguous {
        Err("peer_timeout_not_discriminated")
    } else {
        Err("policy_effect_not_observed")
    }
}

fn snapshot(namespace: &Namespace, policy: &Value) -> Result<Value> {
    Ok(
        json!({"namespace":namespace.name,"uid":uid(policy)?,"resource_version":resource_version(policy)?}),
    )
}

async fn exercise(
    kube: &Kubectl,
    source: &CoreDns,
    client_ns: &Namespace,
    server_ns: &Namespace,
    report: &mut Value,
    samples: &mut Vec<Value>,
) -> Result<()> {
    report["stage"] = json!("policy_and_pod_setup");
    let deny_client = create_policy(
        kube,
        &policy(client_ns, "deny", Some(json!([])), Some(json!([]))),
    )
    .await?;
    let deny_server = create_policy(
        kube,
        &policy(server_ns, "deny", Some(json!([])), Some(json!([]))),
    )
    .await?;
    let ports = json!([{"port":DNS_PORT,"protocol":"TCP"}]);
    let egress = policy(
        client_ns,
        "allow-pair",
        None,
        Some(json!([{"to":[peer(server_ns)],"ports":ports}])),
    );
    let ingress = policy(
        server_ns,
        "allow-pair",
        Some(json!([{"from":[peer(client_ns)],"ports":ports}])),
        None,
    );
    let allow_client = create_policy(kube, &egress).await?;
    let allow_server = create_policy(kube, &ingress).await?;
    report["initial_policies"] = json!([
        snapshot(client_ns, &deny_client)?,
        snapshot(server_ns, &deny_server)?,
        snapshot(client_ns, &allow_client)?,
        snapshot(server_ns, &allow_server)?
    ]);
    let server_core =
        "probe.qa.invalid:1053 {\n hosts {\n 192.0.2.42 canary.probe.qa.invalid\n }\n}\n";
    let server_pod = start_pod(kube, server_ns, source, server_core, report).await?;
    let client_core = format!(
        "health.qa.invalid:1053 {{\n hosts {{\n 192.0.2.43 canary.health.qa.invalid\n }}\n}}\nprobe.qa.invalid:1053 {{\n forward . {}:1053 {{\n force_tcp\n max_fails 0\n expire 100ms\n }}\n}}\n",
        server_pod.ip
    );
    let client_pod = start_pod(kube, client_ns, source, &client_core, report).await?;
    if client_pod.ip == server_pod.ip {
        return Err("pod_addresses_not_distinct");
    }
    report["pods"] = json!([{"namespace":server_ns.name,"uid":server_pod.uid,"ip":server_pod.ip},
        {"namespace":client_ns.name,"uid":client_pod.uid,"ip":client_pod.ip}]);
    let mut client = Forward::start(kube, client_ns).await?;
    let mut server = Forward::start(kube, server_ns).await?;
    let outcome = async {
        report["stage"] = json!("baseline");
        measure(&mut client, &mut server, "baseline", true, samples).await?;
        report["stage"] = json!("remove_egress_allow");
        remove_allow(kube, client_ns, "allow-pair", &allow_client).await?;
        report["stage"] = json!("egress_denied");
        measure(&mut client, &mut server, "egress_denied", false, samples).await?;
        report["stage"] = json!("restore_egress_allow");
        let reallow_client = create_policy(kube, &egress).await?;
        report["restored_egress_allow"] = snapshot(client_ns, &reallow_client)?;
        report["stage"] = json!("egress_restored");
        measure(&mut client, &mut server, "egress_restored", true, samples).await?;
        report["stage"] = json!("remove_ingress_allow");
        remove_allow(kube, server_ns, "allow-pair", &allow_server).await?;
        report["stage"] = json!("ingress_denied");
        measure(&mut client, &mut server, "ingress_denied", false, samples).await?;
        report["stage"] = json!("restore_ingress_allow");
        let reallow_server = create_policy(kube, &ingress).await?;
        report["restored_ingress_allow"] = snapshot(server_ns, &reallow_server)?;
        report["stage"] = json!("ingress_restored");
        measure(&mut client, &mut server, "ingress_restored", true, samples).await?;
        for (namespace, original) in [(client_ns, client_pod), (server_ns, server_pod)] {
            let pod = get_pod(kube, namespace).await?;
            if inspect_pod(&pod, namespace, source, original.uid)?.ip != original.ip {
                return Err("pod_address_changed");
            }
            let original = if namespace.lane == "client" {
                &deny_client
            } else {
                &deny_server
            };
            let current = kube
                .call(
                    &[
                        "get",
                        "networkpolicy",
                        "deny",
                        "-n",
                        &namespace.name,
                        "-o",
                        "json",
                    ],
                    None,
                )
                .await?;
            if uid(&current)? != uid(original)?
                || resource_version(&current)? != resource_version(original)?
            {
                return Err("default_deny_policy_changed");
            }
        }
        Ok(())
    }
    .await;
    client.close().await;
    server.close().await;
    outcome
}

#[tokio::test]
#[ignore = "requires opt-in and operator-authorized local Docker Desktop Pods and NetworkPolicies"]
async fn docker_desktop_tcp_network_policy() {
    let mut report =
        json!({"scope":"same_node_pod_to_pod_tcp_only","status":"BLOCKED","stage":"preflight"});
    let mut samples = Vec::new();
    let outcome: Result<()> = async {
        let kube = verified_kubectl("AI_GUARD_QA_KUBE_CNI").await?;
        let source = loaded_coredns(&kube).await?;
        report["image"] = json!(COREDNS_IMAGE);
        report["image_digest"] = json!(
            source
                .image_id
                .rsplit_once("sha256:")
                .map(|(_, digest)| digest)
        );
        let owner = Uuid::new_v4().simple().to_string();
        let mut client = Namespace::new(&owner, "client");
        let mut server = Namespace::new(&owner, "server");
        let ownership =
            json!({"owner":owner,"label":OWNER,"client":client.name,"server":server.name});
        eprintln!("{}", json!({"qa_namespace_attempt":ownership}));
        report["ownership"] = ownership;
        report["stage"] = json!("namespace_setup");
        let outcome = async {
            client.create(&kube).await?;
            server.create(&kube).await?;
            report["namespace_uids"] = json!({"client":client.uid,"server":server.uid});
            exercise(&kube, &source, &client, &server, &mut report, &mut samples).await
        }
        .await;
        let server_cleanup = server.cleanup(&kube).await;
        let client_cleanup = client.cleanup(&kube).await;
        report["cleanup"] = json!({"server":server_cleanup.err().unwrap_or("removed"),
            "client":client_cleanup.err().unwrap_or("removed")});
        outcome?;
        if client_cleanup.is_err() || server_cleanup.is_err() {
            return Err("cleanup_unverified");
        }
        Ok(())
    }
    .await;
    report["samples"] = json!(samples);
    match outcome {
        Ok(()) => {
            report["status"] = json!("PASS");
            report["stage"] = json!("complete");
        }
        Err(reason) => {
            if (reason == "policy_effect_not_observed" && report["stage"] != "baseline")
                || reason == "cleanup_unverified"
            {
                report["status"] = json!("FAIL");
            }
            report["diagnostic"] = json!(reason);
        }
    }
    println!("{report}");
    assert!(
        outcome.is_ok(),
        "live TCP NetworkPolicy recipe did not pass"
    );
}

#[tokio::test]
#[ignore = "requires opt-in and operator-authorized local Docker Desktop PVC and scoped RBAC"]
async fn docker_desktop_workspace_pending_diagnostic() {
    workspace_pending::run().await;
}
