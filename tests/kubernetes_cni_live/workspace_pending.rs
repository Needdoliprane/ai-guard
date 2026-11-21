//! Real read-only workspace diagnostic with one non-provisionable recipe PVC.
use super::{Kubectl, LANE, Namespace, OWNER, Result, resource_version, uid, verified_kubectl};
use agentsandbox::{
    control_fs::PrivateFile,
    runtime::{capture_stream, kubernetes::KubernetesAuthorityV1},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::{
    fs::{OpenOptions, Permissions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    process::Command,
    time::{sleep, timeout},
};
use uuid::Uuid;

const READER: &str = "workspace-inspector";
const TOKEN_SECONDS: i64 = 600;
const PENDING: &str = "Kubernetes workspace claim is Pending. Verify that its StorageClass supports ReadWriteOncePod through CSI and uses Immediate binding";

fn has_owner(object: &Value, ns: &Namespace) -> bool {
    let labels = &object["metadata"]["labels"];
    labels[OWNER] == ns.owner && labels[LANE] == ns.lane
}

struct OwnedResource {
    resource: &'static str,
    name: String,
    namespaced: bool,
    path: String,
    uid: Uuid,
}

impl OwnedResource {
    async fn cleanup(&self, kube: &Kubectl, ns: &Namespace) -> Result<()> {
        let mut get = vec![
            "get",
            self.resource,
            &self.name,
            "--ignore-not-found",
            "-o",
            "json",
        ];
        if self.namespaced {
            get.extend(["--namespace", &ns.name]);
        }
        let current = kube.call(&get, None).await?;
        if current.is_null() {
            return Ok(());
        }
        if uid(&current)? != self.uid
            || current["metadata"]["name"] != self.name
            || !has_owner(&current, ns)
        {
            return Err("resource_cleanup_identity_mismatch");
        }
        kube.delete_uid(&self.path, self.uid).await?;
        let deadline = Instant::now() + Duration::from_secs(25);
        while Instant::now() < deadline {
            let current = kube.call(&get, None).await?;
            if current.is_null() {
                return Ok(());
            }
            if uid(&current)? != self.uid {
                return Err("resource_cleanup_identity_changed");
            }
            sleep(Duration::from_millis(500)).await;
        }
        Err("resource_cleanup_pending")
    }
}

async fn create_resources(
    kube: &Kubectl,
    ns: &Namespace,
    claim: &str,
    class: &str,
    owned: &mut Vec<OwnedResource>,
) -> Result<()> {
    let subject = json!([{"kind":"ServiceAccount","name":READER,"namespace":ns.name}]);
    let pvc_select = json!({"matchLabels": ns.selector()});
    let rbac_group = "rbac.authorization.k8s.io";
    for (kind, resource, name, namespaced, mut object) in [
        (
            "PersistentVolumeClaim",
            "persistentvolumeclaims",
            claim,
            true,
            json!({"spec": {
                "accessModes":["ReadWriteOncePod"], "volumeMode":"Filesystem",
                "resources":{"requests":{"storage":"1Mi"}},
                "storageClassName":class, "selector":pvc_select,
            }}),
        ),
        (
            "ServiceAccount",
            "serviceaccounts",
            READER,
            true,
            json!({"automountServiceAccountToken":false}),
        ),
        (
            "Role",
            "roles",
            READER,
            true,
            json!({"rules":[{
                "apiGroups":[""], "resources":["persistentvolumeclaims"],
                "verbs":["get"], "resourceNames":[claim],
            }]}),
        ),
        (
            "RoleBinding",
            "rolebindings",
            READER,
            true,
            json!({
                "subjects":subject, "roleRef":{"apiGroup":rbac_group,"kind":"Role","name":READER},
            }),
        ),
        (
            "ClusterRole",
            "clusterroles",
            ns.name.as_str(),
            false,
            json!({"rules":[{
                "apiGroups":[""], "resources":["namespaces"],
                "verbs":["get"], "resourceNames":["kube-system",ns.name],
            }]}),
        ),
        (
            "ClusterRoleBinding",
            "clusterrolebindings",
            ns.name.as_str(),
            false,
            json!({
                "subjects":subject,
                "roleRef":{"apiGroup":rbac_group,"kind":"ClusterRole","name":ns.name},
            }),
        ),
    ] {
        let core = matches!(kind, "PersistentVolumeClaim" | "ServiceAccount");
        let (api_version, api_root) = if core {
            ("v1", "/api/v1")
        } else {
            (
                "rbac.authorization.k8s.io/v1",
                "/apis/rbac.authorization.k8s.io/v1",
            )
        };
        object["kind"] = json!(kind);
        object["apiVersion"] = json!(api_version);
        object["metadata"] = json!({"name":name,"labels":ns.selector()});
        let mut path = api_root.to_owned();
        if namespaced {
            object["metadata"]["namespace"] = json!(ns.name);
            path.push_str(&format!("/namespaces/{}", ns.name));
        }
        path.push_str(&format!("/{resource}/{name}"));
        let created = kube.create(&object).await?;
        if created["apiVersion"] != api_version
            || created["kind"] != kind
            || created["metadata"]["name"] != name
            || created["metadata"]["namespace"] != object["metadata"]["namespace"]
            || !has_owner(&created, ns)
        {
            return Err("resource_create_identity_mismatch");
        }
        owned.push(OwnedResource {
            resource,
            name: name.into(),
            namespaced,
            path,
            uid: uid(&created)?,
        });
        let fields_match = match kind {
            "Role" | "ClusterRole" => created["rules"] == object["rules"],
            "RoleBinding" | "ClusterRoleBinding" => {
                created["subjects"] == object["subjects"] && created["roleRef"] == object["roleRef"]
            }
            "ServiceAccount" => created["automountServiceAccountToken"] == false,
            _ => true,
        };
        if !fields_match {
            return Err("recipe_rbac_or_account_changed");
        }
    }
    Ok(())
}

async fn confirm_no_workload_or_binding(kube: &Kubectl, ns: &Namespace, class: &str) -> Result<()> {
    for (args, reason) in [
        (
            vec![
                "get",
                "storageclass",
                class,
                "--ignore-not-found",
                "-o=name",
            ],
            "recipe_storage_class_exists",
        ),
        (
            vec!["get", "pods", "--namespace", &ns.name, "-o=name"],
            "recipe_namespace_has_pods",
        ),
    ] {
        if !kube.call_raw(&args, None, 4096).await?.is_empty() {
            return Err(reason);
        }
    }
    Ok(())
}

async fn pending_claim(
    kube: &Kubectl,
    ns: &Namespace,
    class: &str,
    claim: &OwnedResource,
) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let current = kube.call(&["get", "--raw", &claim.path], None).await?;
        if current["kind"] != "PersistentVolumeClaim"
            || current["apiVersion"] != "v1"
            || current["metadata"]["name"] != claim.name
            || current["metadata"]["namespace"] != ns.name
            || uid(&current)? != claim.uid
            || !has_owner(&current, ns)
            || !current["metadata"]["deletionTimestamp"].is_null()
            || current["spec"]["storageClassName"] != class
            || current["spec"]["selector"] != json!({"matchLabels": ns.selector()})
            || current["spec"]["accessModes"] != json!(["ReadWriteOncePod"])
            || current["spec"]["volumeMode"] != "Filesystem"
            || current["spec"]["resources"]["requests"]["storage"] != "1Mi"
            || !current["spec"]["volumeName"].is_null()
        {
            return Err("recipe_claim_identity_or_spec_changed");
        }
        match current["status"]["phase"].as_str() {
            Some("Pending") => return resource_version(&current).map(str::to_owned),
            None if current["status"]["phase"].is_null() && Instant::now() < deadline => {
                sleep(Duration::from_millis(250)).await;
            }
            _ => return Err("recipe_claim_pending_not_observed"),
        }
    }
}

fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|_| "private_recipe_file")
}

async fn inspect(kube: &Kubectl, ns: &Namespace, claim: &str, root: &Path) -> Result<Uuid> {
    // Only the public CA field is projected from the verified context config.
    let ca_data = kube
        .call_raw(
            &[
                "config",
                "view",
                "--minify",
                "--raw",
                "-o=jsonpath={.clusters[0].cluster.certificate-authority-data}",
            ],
            None,
            96 * 1024,
        )
        .await?;
    if ca_data.is_empty() {
        return Err("recipe_context_embedded_ca_required");
    }
    let ca = STANDARD.decode(ca_data).map_err(|_| "recipe_ca_pem")?;
    let ca_path = root.join("ca.pem");
    private_write(&ca_path, &ca)?;
    let ca = PrivateFile::open(&ca_path, 64 * 1024).map_err(|_| "recipe_ca_pem")?;
    let cluster = kube
        .call(&["get", "--raw", "/api/v1/namespaces/kube-system"], None)
        .await?;
    let authority_id = Uuid::new_v4();
    let authority = KubernetesAuthorityV1::new(
        authority_id,
        kube.endpoint.as_deref().ok_or("recipe_endpoint")?,
        &uid(&cluster)?.to_string(),
        &ns.name,
        &ns.uid.ok_or("namespace_ownership_unconfirmed")?.to_string(),
        ca.digest(),
    )
    .map_err(|_| "recipe_authority")?;
    let authority_path = root.join("authority.json");
    private_write(
        &authority_path,
        &serde_json::to_vec(&authority).map_err(|_| "recipe_authority")?,
    )?;

    let request = json!({"apiVersion":"authentication.k8s.io/v1","kind":"TokenRequest",
        "spec":{"audiences":[],"expirationSeconds":TOKEN_SECONDS}});
    let path = format!(
        "/api/v1/namespaces/{}/serviceaccounts/{READER}/token",
        ns.name
    );
    let response = kube
        .call(&["create", "--raw", &path, "-f", "-"], Some(&request))
        .await?;
    let expiration = response["status"]["expirationTimestamp"]
        .as_str()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .ok_or("recipe_token_expiration")?;
    if !(1..=TOKEN_SECONDS).contains(&(expiration.with_timezone(&Utc) - Utc::now()).num_seconds()) {
        return Err("recipe_token_lifetime");
    }
    let token = response["status"]["token"]
        .as_str()
        .filter(|value| (32..=8192).contains(&value.len()))
        .ok_or("recipe_token_invalid")?;
    let token_path = root.join("bearer");
    private_write(&token_path, token.as_bytes())?;

    let mut process = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .args(["kubernetes", "inspect-workspace", "--claim-name", claim])
        .arg("--authority-file")
        .arg(authority_path)
        .arg("--ca-file")
        .arg(ca_path)
        .arg("--token-file")
        .arg(token_path)
        .spawn()
        .map_err(|_| "recipe_cli_spawn")?;
    let stdout = process.stdout.take().ok_or("recipe_cli_stdout")?;
    let stderr = process.stderr.take().ok_or("recipe_cli_stderr")?;
    let (out, err, status) = timeout(Duration::from_secs(50), async move {
        tokio::join!(
            capture_stream(stdout, 4096),
            capture_stream(stderr, 4096),
            process.wait()
        )
    })
    .await
    .map_err(|_| "recipe_cli_timeout")?;
    let out = out.map_err(|_| "recipe_cli_stdout")?;
    let err = err.map_err(|_| "recipe_cli_stderr")?;
    if status.map_err(|_| "recipe_cli_exit")?.code() != Some(1)
        || out.total_bytes != 0
        || err.truncated
        || err.bytes != format!("Error: {PENDING}\n").as_bytes()
    {
        return Err("recipe_cli_pending_diagnostic_not_observed");
    }
    Ok(authority_id)
}

pub(super) async fn run() {
    let mut report =
        json!({"scope":"docker_desktop_pending_workspace_diagnostic","status":"BLOCKED"});
    let outcome: Result<()> = async {
        let kube = verified_kubectl("AI_GUARD_QA_KUBE_WORKSPACE_PENDING").await?;
        let private = tempfile::Builder::new()
            .permissions(Permissions::from_mode(0o700))
            .tempdir()
            .map_err(|_| "private_recipe_directory")?;
        let root = private
            .path()
            .canonicalize()
            .map_err(|_| "private_recipe_directory")?;
        let owner = Uuid::new_v4().simple().to_string();
        let mut ns = Namespace::new(&owner, "pending");
        let claim = format!("ai-guard-ws-{owner}");
        let class = format!("aig-v1-no-provisioner-{owner}");
        let mut owned = Vec::new();
        report["namespace"] = json!(ns.name);
        report["claim"] = json!(claim);
        report["absent_storage_class"] = json!(class);
        report["owner"] = json!(owner);
        eprintln!("{}", json!({"qa_pending_attempt":report}));

        let checked: Result<()> = async {
            ns.create(&kube).await?;
            report["namespace_uid"] = json!(ns.uid);
            confirm_no_workload_or_binding(&kube, &ns, &class).await?;
            create_resources(&kube, &ns, &claim, &class, &mut owned).await?;
            let pvc = owned.first().ok_or("recipe_claim_untracked")?;
            let before = pending_claim(&kube, &ns, &class, pvc).await?;
            confirm_no_workload_or_binding(&kube, &ns, &class).await?;
            let diagnostic = inspect(&kube, &ns, &claim, &root).await;
            if diagnostic.is_ok()
                || matches!(
                    diagnostic,
                    Err("recipe_cli_pending_diagnostic_not_observed" | "recipe_cli_timeout")
                )
            {
                report["status"] = json!("FAIL");
            }
            let after = pending_claim(&kube, &ns, &class, pvc).await?;
            confirm_no_workload_or_binding(&kube, &ns, &class).await?;
            report["observation"] = json!({"phase":"Pending","claim_uid":pvc.uid,
                "resource_version_before":before,"resource_version_after":after,
                "access_mode":"ReadWriteOncePod","volume_mode":"Filesystem","request":"1Mi",
                "pods":0,"bound_volume":false});
            report["cli"] = json!({"authority_id":diagnostic?,"exit_code":1,"stdout_bytes":0,
                "diagnostic":PENDING});
            Ok(())
        }
        .await;

        let mut cleanup = Vec::new();
        let mut removed = true;
        for resource in owned.iter().rev() {
            let result = resource.cleanup(&kube, &ns).await;
            removed &= result.is_ok();
            cleanup.push(json!({"resource":resource.resource,"name":resource.name,
                "uid":resource.uid,"result":result.err().unwrap_or("removed")}));
        }
        let namespace = ns.cleanup(&kube).await;
        report["cleanup"] =
            json!({"resources":cleanup,"namespace":namespace.err().unwrap_or("removed")});
        // A lost create response can leave an untracked cluster-scope object.
        // Report it without trying to recover ownership from a name alone.
        if ns.uid.is_some() {
            for resource in ["clusterrolebinding", "clusterrole"] {
                let absent = kube
                    .call_raw(
                        &["get", resource, &ns.name, "--ignore-not-found", "-o=name"],
                        None,
                        4096,
                    )
                    .await
                    .is_ok_and(|value| value.is_empty());
                report["cleanup"][resource] = json!(if absent { "absent" } else { "unverified" });
                removed &= absent;
            }
        }
        if !removed || (ns.uid.is_some() && namespace.is_err()) {
            report["status"] = json!("FAIL");
            report["cleanup_unverified"] = json!(true);
        }
        checked?;
        if removed && namespace.is_ok() {
            Ok(())
        } else {
            Err("cleanup_unverified")
        }
    }
    .await;

    match outcome {
        Ok(()) => report["status"] = json!("PASS"),
        Err(reason) => report["diagnostic"] = json!(reason),
    }
    println!("{report}");
    assert!(
        outcome.is_ok(),
        "live workspace Pending recipe did not pass"
    );
}
