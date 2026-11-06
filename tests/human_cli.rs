//! Product CLI regressions over private files, real SQLite and the human router.
//! No IdP, provider model, container engine or external service is required.
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    identity::*,
    model::Event,
    policy::Policy,
    redaction::Redactor,
};
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use uuid::Uuid;

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "human CLI exited with {:?}",
        output.status.code()
    );
}
fn failure(output: &Output) {
    assert!(!output.status.success());
}
fn json_output(output: &Output) -> Value {
    success(output);
    serde_json::from_slice(&output.stdout).unwrap()
}
fn no_secret(output: &Output, secret: &str) {
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(secret),
        "credential appeared in stdout"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains(secret),
        "credential appeared in stderr"
    );
}
fn private_file(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
fn initialized(path: &Path, tenant: &str, identity: bool) {
    let store = Store::create_supervised_for_tenant(
        path,
        tenant,
        Redactor::new(vec![]),
        5000,
        &path.with_extension("before-v3.sqlite"),
    )
    .unwrap();
    if identity {
        store
            .migrate_identity_v4(&path.with_extension("before-v4.sqlite"))
            .unwrap();
    }
}
fn initialized_for_http(path: &Path, tenant: &str) {
    initialized(path, tenant, true);
    let audit = store(path, tenant);
    audit
        .migrate_human_access_v5(&path.with_extension("before-v5.sqlite"))
        .unwrap();
    assert_eq!(audit.supervision_state().unwrap().schema_version, 5);
    assert!(!audit.supervision_state().unwrap().active);
}
fn public_authorization(value: &Value) {
    assert!(
        value.get("authorization_revision").is_none(),
        "tenant-global authorization revision must stay private"
    );
    let version = value["authorization_version"].as_str().unwrap();
    assert!(
        version.len() == 64
            && version
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "public authorization version must be opaque lowercase hex"
    );
}
fn store(path: &Path, tenant: &str) -> Store {
    Store::open_existing_for_tenant(path, tenant, Redactor::new(vec![]), 5000).unwrap()
}
fn provision_command(path: &Path, tenant: &str, output: &Path, name: &str) -> Command {
    let mut cmd = command();
    cmd.args(["human", "provision", "--data"])
        .arg(path)
        .args([
            "--tenant-id",
            tenant,
            "--name",
            name,
            "--organization",
            "organization-a",
            "--output-dir",
        ])
        .arg(output)
        .arg("--confirm-control-stopped");
    cmd
}
fn resume_command(path: &Path, tenant: &str, output: &Path) -> Command {
    let mut cmd = command();
    cmd.args(["human", "resume-provision", "--data"])
        .arg(path)
        .args(["--tenant-id", tenant, "--output-dir"])
        .arg(output)
        .arg("--confirm-control-stopped");
    cmd
}
fn preparation(path: &Path) -> LocalHumanProvisionV1 {
    let value: Value =
        serde_json::from_slice(&std::fs::read(path.join("recovery.json")).unwrap()).unwrap();
    serde_json::from_value(value["provision"].clone()).unwrap()
}
fn secret(path: &Path) -> String {
    std::fs::read_to_string(path.join("credential"))
        .unwrap()
        .trim_end_matches('\n')
        .to_owned()
}
fn account(input: &LocalHumanProvisionV1) -> HumanAccountV1 {
    let IdentityChangeV1::Account { account, .. } = &input.mutations[0].change else {
        panic!("typed account preparation")
    };
    account.clone()
}
fn membership(input: &LocalHumanProvisionV1) -> MembershipV1 {
    let IdentityChangeV1::Membership { membership, .. } = &input.mutations[2].change else {
        panic!("typed membership preparation")
    };
    membership.clone()
}
fn mutation(actor: &PrincipalRefV1, change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: actor.clone(),
        change,
    }
}

#[test]
fn provision_retains_private_files_and_replays_commit_with_missing_completion() {
    let files = support::private_tempdir();
    let data = files.path().join("audit.sqlite");
    initialized(&data, "tenant-a", true);
    let output_dir = files.path().join("alice");
    let output = provision_command(&data, "tenant-a", &output_dir, "Alice")
        .output()
        .unwrap();
    success(&output);
    let bearer = secret(&output_dir);
    no_secret(&output, &bearer);
    let input = preparation(&output_dir);
    assert!(input.mutations.iter().all(|m| matches!(&m.actor,PrincipalRefV1::LegacyControl { credential_id,role:LegacyControlRoleV1::Runner } if credential_id.starts_with("offline-filesystem:uid:"))));
    assert_eq!(
        std::fs::metadata(&output_dir).unwrap().permissions().mode() & 0o7777,
        0o700
    );
    for name in ["credential", "recovery.json", "completion.json"] {
        assert_eq!(
            std::fs::metadata(output_dir.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o600
        );
    }
    let audit = store(&data, "tenant-a");
    assert!(!audit.supervision_state().unwrap().active);
    assert!(
        audit
            .human_account(&account(&input).principal)
            .unwrap()
            .is_some()
    );
    let before = audit.head().unwrap();
    drop(audit);
    let completion = std::fs::read(output_dir.join("completion.json")).unwrap();
    // A crash can commit SQLite before this final receipt is durable.
    std::fs::rename(
        output_dir.join("completion.json"),
        output_dir.join("retained-completion.json"),
    )
    .unwrap();
    let resumed = resume_command(&data, "tenant-a", &output_dir)
        .output()
        .unwrap();
    success(&resumed);
    no_secret(&resumed, &bearer);
    assert!(std::fs::read(output_dir.join("completion.json")).unwrap() == completion);
    assert!(secret(&output_dir) == bearer);
    assert_eq!(store(&data, "tenant-a").head().unwrap(), before);
    let duplicate = provision_command(&data, "tenant-a", &output_dir, "Another account")
        .output()
        .unwrap();
    failure(&duplicate);
    no_secret(&duplicate, &bearer);
    assert_eq!(store(&data, "tenant-a").head().unwrap(), before);
}

#[test]
fn absent_wrong_tenant_or_unmigrated_database_is_never_created_adopted_or_migrated() {
    let files = support::private_tempdir();
    let missing = files.path().join("absent.sqlite");
    let output_dir = files.path().join("missing-account");
    failure(
        &provision_command(&missing, "tenant-a", &output_dir, "Alice")
            .output()
            .unwrap(),
    );
    assert!(!missing.exists());
    assert!(!output_dir.exists());
    let data = files.path().join("audit.sqlite");
    initialized(&data, "tenant-a", false);
    let before = store(&data, "tenant-a").head().unwrap();
    failure(
        &provision_command(&data, "tenant-a", &output_dir, "Alice")
            .output()
            .unwrap(),
    );
    assert_eq!(
        store(&data, "tenant-a")
            .supervision_state()
            .unwrap()
            .schema_version,
        3
    );
    assert_eq!(store(&data, "tenant-a").head().unwrap(), before);
    failure(
        &provision_command(&data, "tenant-b", &output_dir, "Alice")
            .output()
            .unwrap(),
    );
    assert!(!output_dir.exists());
}

#[test]
fn resume_refuses_modified_credential_unsafe_mode_and_copied_namespace_without_mutation() {
    let files = support::private_tempdir();
    let data = files.path().join("audit.sqlite");
    initialized(&data, "tenant-a", true);
    let output_dir = files.path().join("alice");
    success(
        &provision_command(&data, "tenant-a", &output_dir, "Alice")
            .output()
            .unwrap(),
    );
    let bearer = secret(&output_dir);
    let before = store(&data, "tenant-a").head().unwrap();
    std::fs::set_permissions(
        output_dir.join("credential"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let refused = resume_command(&data, "tenant-a", &output_dir)
        .output()
        .unwrap();
    failure(&refused);
    no_secret(&refused, &bearer);
    std::fs::set_permissions(
        output_dir.join("credential"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let copied = files.path().join("copied");
    std::fs::create_dir(&copied).unwrap();
    std::fs::set_permissions(&copied, std::fs::Permissions::from_mode(0o700)).unwrap();
    for name in ["credential", "recovery.json", "completion.json"] {
        private_file(
            &copied.join(name),
            &std::fs::read(output_dir.join(name)).unwrap(),
        );
    }
    failure(&resume_command(&data, "tenant-a", &copied).output().unwrap());
    std::fs::rename(
        output_dir.join("credential"),
        output_dir.join("retained-credential"),
    )
    .unwrap();
    private_file(
        &output_dir.join("credential"),
        format!("aigh1_{}\n", "0".repeat(64)).as_bytes(),
    );
    let refused = resume_command(&data, "tenant-a", &output_dir)
        .output()
        .unwrap();
    failure(&refused);
    no_secret(&refused, &bearer);
    assert_eq!(store(&data, "tenant-a").head().unwrap(), before);
}

#[test]
fn concurrent_provision_has_one_fresh_namespace_and_no_duplicate_account() {
    let files = support::private_tempdir();
    let data = files.path().join("audit.sqlite");
    initialized(&data, "tenant-a", true);
    let output_dir = files.path().join("alice");
    let first = provision_command(&data, "tenant-a", &output_dir, "Alice")
        .spawn()
        .unwrap();
    let second = provision_command(&data, "tenant-a", &output_dir, "Alice")
        .spawn()
        .unwrap();
    let first = first.wait_with_output().unwrap();
    let second = second.wait_with_output().unwrap();
    assert_ne!(first.status.success(), second.status.success());
    let audit = store(&data, "tenant-a");
    assert_eq!(audit.identity_authorization_revision().unwrap(), 3);
    assert_eq!(
        audit
            .identity_memberships(&account(&preparation(&output_dir)).principal)
            .unwrap()
            .len(),
        1
    );
    let before = audit.head().unwrap();
    drop(audit);
    success(
        &resume_command(&data, "tenant-a", &output_dir)
            .output()
            .unwrap(),
    );
    assert_eq!(store(&data, "tenant-a").head().unwrap(), before);
}

#[test]
fn offline_apply_refuses_actor_json_and_replays_only_the_exact_operation() {
    let files = support::private_tempdir();
    let data = files.path().join("audit.sqlite");
    initialized(&data, "tenant-a", true);
    let output_dir = files.path().join("alice");
    success(
        &provision_command(&data, "tenant-a", &output_dir, "Alice")
            .output()
            .unwrap(),
    );
    let input = preparation(&output_dir);
    let mut human = account(&input);
    human.active = false;
    human.revision = 2;
    let change = IdentityChangeV1::Account {
        expected_revision: 1,
        account: human.clone(),
    };
    let op = Uuid::new_v4().to_string();
    let malicious = files.path().join("actor.json");
    private_file(
        &malicious,
        &serde_json::to_vec(
            &json!({"version":1,"operation_id":op,"actor":human.principal,"change":change}),
        )
        .unwrap(),
    );
    let apply = |file: &Path| {
        command()
            .args(["human", "apply", "--data"])
            .arg(&data)
            .args(["--tenant-id", "tenant-a", "--mutation-file"])
            .arg(file)
            .arg("--confirm-control-stopped")
            .output()
            .unwrap()
    };
    let before = store(&data, "tenant-a").head().unwrap();
    failure(&apply(&malicious));
    assert_eq!(store(&data, "tenant-a").head().unwrap(), before);
    let valid = files.path().join("mutation.json");
    private_file(
        &valid,
        &serde_json::to_vec(&json!({"version":1,"operation_id":op,"change":change})).unwrap(),
    );
    let applied = apply(&valid);
    success(&applied);
    no_secret(&applied, &secret(&output_dir));
    assert!(
        json_output(&applied)
            .get("authorization_revision")
            .is_none()
    );
    let head = store(&data, "tenant-a").head().unwrap();
    success(&apply(&valid));
    assert_eq!(store(&data, "tenant-a").head().unwrap(), head);
    let conflict = files.path().join("conflict.json");
    human.display_name = "Different payload".into();
    private_file(&conflict,&serde_json::to_vec(&json!({"version":1,"operation_id":op,"change":IdentityChangeV1::Account { expected_revision:1,account:human }})).unwrap());
    failure(&apply(&conflict));
    assert_eq!(store(&data, "tenant-a").head().unwrap(), head);
}

#[test]
fn output_links_and_shared_parents_are_refused_without_repair() {
    let files = support::private_tempdir();
    let data = files.path().join("audit.sqlite");
    initialized(&data, "tenant-a", true);
    let existing = files.path().join("existing");
    std::fs::create_dir(&existing).unwrap();
    std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o700)).unwrap();
    let link = files.path().join("link");
    symlink(&existing, &link).unwrap();
    failure(
        &provision_command(&data, "tenant-a", &link, "Alice")
            .output()
            .unwrap(),
    );
    assert!(std::fs::read_dir(&existing).unwrap().next().is_none());
    let shared = files.path().join("shared");
    std::fs::create_dir(&shared).unwrap();
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
    failure(
        &provision_command(&data, "tenant-a", &shared.join("alice"), "Alice")
            .output()
            .unwrap(),
    );
    assert_eq!(
        std::fs::metadata(&shared).unwrap().permissions().mode() & 0o777,
        0o777
    );
    assert_eq!(
        store(&data, "tenant-a")
            .identity_authorization_revision()
            .unwrap(),
        0
    );
}

struct HumanFixture {
    directory: tempfile::TempDir,
    store_a: Arc<Store>,
    store_b: Arc<Store>,
    a1: LocalHumanProvisionV1,
    credential_a1: PathBuf,
    credential_a2: PathBuf,
    credential_b1: PathBuf,
    resource: ResourceRefV1,
}
impl HumanFixture {
    fn new(events: usize) -> Self {
        let directory = support::private_tempdir();
        let data_a = directory.path().join("a.sqlite");
        let data_b = directory.path().join("b.sqlite");
        initialized_for_http(&data_a, "tenant-a");
        initialized_for_http(&data_b, "tenant-b");
        let out_a1 = directory.path().join("a1");
        let out_a2 = directory.path().join("a2");
        let out_b1 = directory.path().join("b1");
        success(
            &provision_command(&data_a, "tenant-a", &out_a1, "A1")
                .output()
                .unwrap(),
        );
        success(
            &provision_command(&data_a, "tenant-a", &out_a2, "A2")
                .output()
                .unwrap(),
        );
        success(
            &provision_command(&data_b, "tenant-b", &out_b1, "B1")
                .output()
                .unwrap(),
        );
        let a1 = preparation(&out_a1);
        let a2 = preparation(&out_a2);
        let b1 = preparation(&out_b1);
        let store_a = Arc::new(store(&data_a, "tenant-a"));
        let store_b = Arc::new(store(&data_b, "tenant-b"));
        let resource = ResourceRefV1 {
            kind: ResourceKindV1::Session,
            resource_id: "session/a?b=é".into(),
        };
        let bearer = secret(&out_a1);
        let mut a1_observation = None;
        for index in 0..events {
            let mut event = Event::new("agent.output");
            event.session_id = Some(resource.resource_id.clone());
            event.data = json!({"text":format!("output-{index}"),"echo":bearer});
            let observed = store_a.append(event).unwrap();
            if a1_observation.is_none() {
                a1_observation = Some(observed);
            }
        }
        // Resource grants follow actual durable observations. The A1 results
        // stay exactly at the requested count; the other sessions remain
        // separate facts in their respective authorized resource/tenant.
        // Transport-only new(0) fixtures need no grants or business activity.
        if let Some(a1_observation) = a1_observation {
            let a2_resource = ResourceRefV1 {
                kind: ResourceKindV1::Session,
                resource_id: "a2-private".into(),
            };
            let b1_resource = ResourceRefV1 {
                kind: ResourceKindV1::Session,
                resource_id: "b1-private".into(),
            };
            let mut a2_event = Event::new("session.observed");
            a2_event.session_id = Some(a2_resource.resource_id.clone());
            let a2_observation = store_a.append(a2_event).unwrap();
            let mut b1_event = Event::new("session.observed");
            b1_event.session_id = Some(b1_resource.resource_id.clone());
            let b1_observation = store_b.append(b1_event).unwrap();
            for (audit, input, resource, known) in [
                (&store_a, &a1, resource.clone(), a1_observation),
                (&store_a, &a2, a2_resource, a2_observation),
                (&store_b, &b1, b1_resource, b1_observation),
            ] {
                assert_eq!(
                    known.event.session_id.as_deref(),
                    Some(resource.resource_id.as_str())
                );
                assert!(!known.event.kind.starts_with("control.identity."));
                let receipt = audit
                    .mutate_identity(&mutation(
                        &input.mutations[0].actor,
                        IdentityChangeV1::ResourceGrant {
                            expected_revision: 0,
                            grant: ResourceGrantV1 {
                                version: 1,
                                tenant_id: audit.tenant_id().into(),
                                organization_scope: "organization-a".into(),
                                resource,
                                owner: account(input).principal,
                                revision: 1,
                                active: true,
                                shares: vec![],
                            },
                        },
                    ))
                    .unwrap();
                assert!(receipt.audit_seq > known.seq);
            }
        }
        Self {
            directory,
            store_a,
            store_b,
            a1,
            credential_a1: out_a1.join("credential"),
            credential_a2: out_a2.join("credential"),
            credential_b1: out_b1.join("credential"),
            resource,
        }
    }
    fn router(&self) -> axum::Router {
        gateway_router(
            GatewayState::for_tenants(
                vec![
                    TenantGateway {
                        tenant_id: "tenant-a".into(),
                        store: self.store_a.clone(),
                        runner_token: "runner-a-private-test".into(),
                        viewer_token: "viewer-a-private-test".into(),
                    },
                    TenantGateway {
                        tenant_id: "tenant-b".into(),
                        store: self.store_b.clone(),
                        runner_token: "runner-b-private-test".into(),
                        viewer_token: "viewer-b-private-test".into(),
                    },
                ],
                Policy::default(),
                None,
                Redactor::new(vec![]),
            )
            .unwrap(),
        )
    }
}
async fn serve(router: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (origin, task)
}
async fn read(origin: &str, credential: &Path, subcommand: &str, args: &[&str]) -> Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .kill_on_drop(true)
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .env("ALL_PROXY", "http://127.0.0.1:1")
        .args([
            "human",
            subcommand,
            "--gateway",
            origin,
            "--credential-file",
        ])
        .arg(credential)
        .args(args)
        .output()
        .await
        .unwrap()
}
async fn context(origin: &str, credential: &Path) -> String {
    let identity = json_output(&read(origin, credential, "identity", &[]).await);
    public_authorization(&identity["contexts"][0]);
    identity["contexts"][0]["id"].as_str().unwrap().to_owned()
}
fn selection<'a>(context: &'a str, resource: &'a str, view: &'a str) -> Vec<&'a str> {
    vec![
        "--context",
        context,
        "--resource-kind",
        "session",
        "--resource-id",
        resource,
        "--view",
        view,
    ]
}

#[tokio::test]
async fn human_cli_reads_exact_context_and_resource_exports_privately_and_observes_revocation() {
    let fixture = HumanFixture::new(2);
    let (origin, server) = serve(fixture.router()).await;
    let bearer = std::fs::read_to_string(&fixture.credential_a1).unwrap();
    let bearer = bearer.trim();
    let offered = read(&origin, &fixture.credential_a1, "resources", &[]).await;
    assert_eq!(
        json_output(&offered)["status"],
        "context_selection_required"
    );
    no_secret(&offered, bearer);
    let ctx = context(&origin, &fixture.credential_a1).await;
    let page = read(
        &origin,
        &fixture.credential_a1,
        "resources",
        &["--context", &ctx],
    )
    .await;
    let value = json_output(&page);
    assert_eq!(value["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        value["items"][0]["resource"]["resource_id"],
        fixture.resource.resource_id
    );
    public_authorization(&value);
    let metadata = read(
        &origin,
        &fixture.credential_a1,
        "events",
        &selection(&ctx, &fixture.resource.resource_id, "metadata"),
    )
    .await;
    let value = json_output(&metadata);
    public_authorization(&value);
    assert_eq!(value["items"].as_array().unwrap().len(), 2);
    assert!(value["items"][0]["content"].is_null());
    no_secret(&metadata, bearer);
    let content = read(
        &origin,
        &fixture.credential_a1,
        "events",
        &selection(&ctx, &fixture.resource.resource_id, "content"),
    )
    .await;
    assert!(json_output(&content)["items"][0]["content"].is_object());
    no_secret(&content, bearer);
    let foreign = context(&origin, &fixture.credential_b1).await;
    let refused = read(
        &origin,
        &fixture.credential_a1,
        "resources",
        &["--context", &foreign],
    )
    .await;
    failure(&refused);
    no_secret(&refused, bearer);
    let a2 = context(&origin, &fixture.credential_a2).await;
    let inaccessible = read(
        &origin,
        &fixture.credential_a2,
        "events",
        &selection(&a2, &fixture.resource.resource_id, "metadata"),
    )
    .await;
    failure(&inaccessible);
    let final_dir = fixture.directory.path().join("export-a1");
    let mut args = selection(&ctx, &fixture.resource.resource_id, "content");
    args.extend(["--output-dir", final_dir.to_str().unwrap()]);
    let exported = read(&origin, &fixture.credential_a1, "export", &args).await;
    let exported_value = json_output(&exported);
    assert_eq!(exported_value["selection_complete"], true);
    public_authorization(&exported_value);
    no_secret(&exported, bearer);
    assert!(final_dir.join("completion.json").exists());
    assert!(
        !fixture
            .directory
            .path()
            .join("export-a1.preparing")
            .exists()
    );
    let manifest = std::fs::read(final_dir.join("completion.json")).unwrap();
    public_authorization(&serde_json::from_slice::<Value>(&manifest).unwrap());
    let page = std::fs::read(final_dir.join("page-0001.json")).unwrap();
    assert!(!String::from_utf8_lossy(&page).contains(bearer));
    assert_eq!(
        std::fs::metadata(final_dir.join("page-0001.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o600
    );
    failure(&read(&origin, &fixture.credential_a1, "export", &args).await);
    assert!(std::fs::read(final_dir.join("completion.json")).unwrap() == manifest);
    let mut member = membership(&fixture.a1);
    member.active = false;
    member.revision = 2;
    fixture
        .store_a
        .mutate_identity(&mutation(
            &fixture.a1.mutations[0].actor,
            IdentityChangeV1::Membership {
                expected_revision: 1,
                membership: member,
            },
        ))
        .unwrap();
    let revoked = read(
        &origin,
        &fixture.credential_a1,
        "resources",
        &["--context", &ctx],
    )
    .await;
    failure(&revoked);
    assert!(String::from_utf8_lossy(&revoked.stderr).contains("context_revoked"));
    no_secret(&revoked, bearer);
    let IdentityChangeV1::Credential { binding, .. } = &fixture.a1.mutations[1].change else {
        panic!("typed credential")
    };
    let mut binding = binding.clone();
    binding.expires_at = Some("2000-01-01T00:00:00Z".into());
    binding.revision = 2;
    fixture
        .store_a
        .mutate_identity(&mutation(
            &fixture.a1.mutations[0].actor,
            IdentityChangeV1::Credential {
                expected_revision: 1,
                binding: binding.clone(),
            },
        ))
        .unwrap();
    let expired = read(&origin, &fixture.credential_a1, "identity", &[]).await;
    failure(&expired);
    assert!(String::from_utf8_lossy(&expired.stderr).contains("credential_expired"));
    no_secret(&expired, bearer);
    binding.active = false;
    binding.revision = 3;
    fixture
        .store_a
        .mutate_identity(&mutation(
            &fixture.a1.mutations[0].actor,
            IdentityChangeV1::Credential {
                expected_revision: 2,
                binding,
            },
        ))
        .unwrap();
    let revoked = read(&origin, &fixture.credential_a1, "identity", &[]).await;
    failure(&revoked);
    no_secret(&revoked, bearer);
    server.abort();
}

#[tokio::test]
async fn export_retains_complete_pages_on_transport_failure_and_resumes_exact_snapshot() {
    let fixture = HumanFixture::new(101);
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let router=fixture.router().layer(axum::middleware::from_fn(move |request:axum::extract::Request,next:axum::middleware::Next| {
        let observed=observed.clone();
        async move {
            use axum::response::IntoResponse;
            if request.uri().path()=="/api/v1/human/export" && observed.fetch_add(1,Ordering::SeqCst)==1 {
                return (axum::http::StatusCode::SERVICE_UNAVAILABLE,[("cache-control","no-store")],axum::Json(json!({"error":{"code":"audit_unavailable","message":"injected transport outage","retry_action":"retry"}}))).into_response();
            }
            next.run(request).await
        }
    }));
    let (origin, server) = serve(router).await;
    let ctx = context(&origin, &fixture.credential_a1).await;
    let final_dir = fixture.directory.path().join("resumable");
    let mut args = selection(&ctx, &fixture.resource.resource_id, "metadata");
    args.extend(["--output-dir", final_dir.to_str().unwrap()]);
    let interrupted = read(&origin, &fixture.credential_a1, "export", &args).await;
    failure(&interrupted);
    assert!(!final_dir.exists());
    let stage = fixture.directory.path().join("resumable.preparing");
    assert!(stage.join("page-0001.json").exists());
    assert!(!stage.join("completion.json").exists());
    // Existing default trust keeps the historical export binding exactly.
    {
        use sha2::{Digest, Sha256};
        let plan: Value =
            serde_json::from_slice(&std::fs::read(stage.join("selection.json")).unwrap()).unwrap();
        let token = std::fs::read_to_string(&fixture.credential_a1).unwrap();
        assert_eq!(
            plan["credential_digest"],
            hex::encode(Sha256::digest(token.trim().as_bytes()))
        );
    }
    let retained_page = std::fs::read(stage.join("page-0001.json")).unwrap();
    let retained_lock = std::fs::File::open(stage.join("selection.json")).unwrap();
    // SAFETY: this test owns a valid private non-SQLite record descriptor.
    assert_eq!(
        unsafe { libc::flock(retained_lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let busy = read(
        &origin,
        &fixture.credential_a1,
        "resume-export",
        &[
            "--context",
            &ctx,
            "--output-dir",
            final_dir.to_str().unwrap(),
        ],
    )
    .await;
    failure(&busy);
    assert!(std::fs::read(stage.join("page-0001.json")).unwrap() == retained_page);
    drop(retained_lock);
    let resumed = read(
        &origin,
        &fixture.credential_a1,
        "resume-export",
        &[
            "--context",
            &ctx,
            "--output-dir",
            final_dir.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(json_output(&resumed)["pages"], 2);
    assert!(std::fs::read(final_dir.join("page-0001.json")).unwrap() == retained_page);
    assert!(!stage.exists());
    assert!(final_dir.join("completion.json").exists());
    server.abort();
}

#[tokio::test]
async fn export_revocation_between_batches_preserves_prefix_without_publication() {
    let fixture = HumanFixture::new(101);
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let audit = fixture.store_a.clone();
    let mut member = membership(&fixture.a1);
    member.active = false;
    member.revision = 2;
    let revoke = mutation(
        &fixture.a1.mutations[0].actor,
        IdentityChangeV1::Membership {
            expected_revision: 1,
            membership: member,
        },
    );
    let router = fixture.router().layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let observed = observed.clone();
            let audit = audit.clone();
            let revoke = revoke.clone();
            async move {
                if request.uri().path() == "/api/v1/human/export"
                    && observed.fetch_add(1, Ordering::SeqCst) == 1
                {
                    audit.mutate_identity(&revoke).unwrap();
                }
                next.run(request).await
            }
        },
    ));
    let (origin, server) = serve(router).await;
    let ctx = context(&origin, &fixture.credential_a1).await;
    let final_dir = fixture.directory.path().join("revoked-export");
    let mut args = selection(&ctx, &fixture.resource.resource_id, "metadata");
    args.extend(["--output-dir", final_dir.to_str().unwrap()]);
    let refused = read(&origin, &fixture.credential_a1, "export", &args).await;
    failure(&refused);
    assert!(String::from_utf8_lossy(&refused.stderr).contains("context_revoked"));
    assert!(!final_dir.exists());
    let stage = fixture.directory.path().join("revoked-export.preparing");
    assert!(stage.join("page-0001.json").exists());
    assert!(!stage.join("page-0002.json").exists());
    assert!(!stage.join("completion.json").exists());
    let retained = std::fs::read(stage.join("page-0001.json")).unwrap();
    failure(
        &read(
            &origin,
            &fixture.credential_a1,
            "resume-export",
            &[
                "--context",
                &ctx,
                "--output-dir",
                final_dir.to_str().unwrap(),
            ],
        )
        .await,
    );
    assert!(std::fs::read(stage.join("page-0001.json")).unwrap() == retained);
    assert!(!final_dir.exists());
    server.abort();
}

#[tokio::test]
async fn bearer_file_modes_invalid_origins_and_redirects_never_send_or_print_credentials() {
    let fixture = HumanFixture::new(0);
    let bearer = std::fs::read_to_string(&fixture.credential_a1).unwrap();
    let bearer = bearer.trim();
    let refused = read(
        "http://example.com",
        &fixture.credential_a1,
        "identity",
        &[],
    )
    .await;
    failure(&refused);
    no_secret(&refused, bearer);
    let reached = Arc::new(AtomicUsize::new(0));
    let seen = reached.clone();
    let echoed = bearer.to_owned();
    let router = axum::Router::new()
        .route(
            "/api/v1/human/identity",
            axum::routing::get(move || {
                let echoed = echoed.clone();
                async move {
                    (
                        axum::http::StatusCode::FOUND,
                        [
                            ("location", "/leak"),
                            ("content-type", "application/json"),
                            ("cache-control", "no-store"),
                        ],
                        axum::Json(
                            json!({"error":{"code":echoed,"message":echoed,"retry_action":echoed}}),
                        ),
                    )
                }
            }),
        )
        .route(
            "/leak",
            axum::routing::get(move || {
                let seen = seen.clone();
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    "leak"
                }
            }),
        );
    let (origin, server) = serve(router).await;
    let refused = read(&origin, &fixture.credential_a1, "identity", &[]).await;
    failure(&refused);
    no_secret(&refused, bearer);
    assert_eq!(reached.load(Ordering::SeqCst), 0);
    std::fs::set_permissions(
        &fixture.credential_a1,
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    failure(&read(&origin, &fixture.credential_a1, "identity", &[]).await);
    server.abort();
}

#[test]
fn schema_five_maintenance_replays_schema_four_preparation_without_rotation() {
    let files = support::private_tempdir();
    let data = files.path().join("audit.sqlite");
    initialized(&data, "tenant-a", true);
    let alice = files.path().join("alice");
    success(
        &provision_command(&data, "tenant-a", &alice, "Alice")
            .output()
            .unwrap(),
    );
    let bearer = secret(&alice);
    let prepared = std::fs::read(alice.join("recovery.json")).unwrap();
    let completion = std::fs::read(alice.join("completion.json")).unwrap();
    std::fs::rename(
        alice.join("completion.json"),
        alice.join("retained-completion.json"),
    )
    .unwrap();
    let audit = store(&data, "tenant-a");
    audit
        .migrate_human_access_v5(&data.with_extension("before-v5.sqlite"))
        .unwrap();
    let head = audit.head().unwrap();
    assert_eq!(audit.supervision_state().unwrap().schema_version, 5);
    assert!(!audit.supervision_state().unwrap().active);
    drop(audit);
    let resumed = resume_command(&data, "tenant-a", &alice).output().unwrap();
    success(&resumed);
    no_secret(&resumed, &bearer);
    assert!(std::fs::read(alice.join("recovery.json")).unwrap() == prepared);
    assert!(std::fs::read(alice.join("completion.json")).unwrap() == completion);
    assert!(secret(&alice) == bearer);
    assert_eq!(store(&data, "tenant-a").head().unwrap(), head);
    let bob = files.path().join("bob");
    success(
        &provision_command(&data, "tenant-a", &bob, "Bob")
            .output()
            .unwrap(),
    );
    let mut human = account(&preparation(&alice));
    human.active = false;
    human.revision = 2;
    let mutation_file = files.path().join("disable-alice.json");
    private_file(
        &mutation_file,
        &serde_json::to_vec(&json!({
            "version":1,"operation_id":Uuid::new_v4().to_string(),
            "change":IdentityChangeV1::Account { expected_revision:1,account:human.clone() }
        }))
        .unwrap(),
    );
    let apply = || {
        command()
            .args(["human", "apply", "--data"])
            .arg(&data)
            .args(["--tenant-id", "tenant-a", "--mutation-file"])
            .arg(&mutation_file)
            .arg("--confirm-control-stopped")
            .output()
            .unwrap()
    };
    let output = apply();
    success(&output);
    no_secret(&output, &bearer);
    assert!(json_output(&output).get("authorization_revision").is_none());
    let audit = store(&data, "tenant-a");
    assert!(
        !audit
            .human_account(&human.principal)
            .unwrap()
            .unwrap()
            .active
    );
    assert!(!audit.supervision_state().unwrap().active);
    let head = audit.head().unwrap();
    drop(audit);
    success(&apply());
    assert_eq!(store(&data, "tenant-a").head().unwrap(), head);
}

fn migrate_human_cli_to(data: &Path, version: u32) {
    let audit = store(data, "tenant-a");
    let current = audit.supervision_state().unwrap();
    assert!(!current.active);
    for target in current.schema_version + 1..=version {
        let backup = data.with_extension(format!("before-v{target}.sqlite"));
        match target {
            5 => audit.migrate_human_access_v5(&backup).unwrap(),
            6 => audit.migrate_workload_v6(&backup).unwrap(),
            7 => audit.migrate_human_brokers_v7(&backup).unwrap(),
            _ => panic!("unexpected human CLI migration target"),
        };
    }
    assert_eq!(audit.supervision_state().unwrap().schema_version, version);
    assert!(!audit.supervision_state().unwrap().active);
    assert!(audit.integrity().unwrap().valid);
}

#[test]
fn broker_registry_cli_option_requires_schema7_before_preparation_and_is_never_default() {
    let files = support::private_tempdir();
    let data = files.path().join("audit.sqlite");
    initialized(&data, "tenant-a", true);
    migrate_human_cli_to(&data, 6);
    let refused_directory = files.path().join("schema6-refused");
    let before = store(&data, "tenant-a").integrity().unwrap();
    let refused = provision_command(&data, "tenant-a", &refused_directory, "Broker reader")
        .args([
            "--capabilities",
            "read-broker-registry",
            "--max-events",
            "5000",
        ])
        .output()
        .unwrap();
    failure(&refused);
    assert!(String::from_utf8_lossy(&refused.stderr).contains("requires explicit migration6→7"));
    assert!(
        !refused_directory.exists(),
        "refused schema6 preparation must not publish credentials or recovery files"
    );
    assert_eq!(
        store(&data, "tenant-a").integrity().unwrap().head_hash,
        before.head_hash
    );
    assert_eq!(
        store(&data, "tenant-a")
            .supervision_state()
            .unwrap()
            .schema_version,
        6
    );

    migrate_human_cli_to(&data, 7);
    let explicit = files.path().join("explicit-broker-reader");
    let granted = provision_command(&data, "tenant-a", &explicit, "Broker reader")
        .args([
            "--capabilities",
            "read-broker-registry",
            "--max-events",
            "5000",
        ])
        .output()
        .unwrap();
    success(&granted);
    let bearer = secret(&explicit);
    no_secret(&granted, &bearer);
    let prepared = preparation(&explicit);
    let explicit_membership = membership(&prepared);
    assert_eq!(
        explicit_membership.capabilities,
        std::collections::BTreeSet::from([HumanCapabilityV1::ReadBrokerRegistry])
    );
    let audit = store(&data, "tenant-a");
    assert_eq!(
        audit
            .identity_memberships(&explicit_membership.principal)
            .unwrap(),
        [explicit_membership]
    );
    assert!(!audit.supervision_state().unwrap().active);
    drop(audit);

    let defaults = files.path().join("default-reader");
    let default_output = provision_command(&data, "tenant-a", &defaults, "Default reader")
        .args(["--max-events", "5000"])
        .output()
        .unwrap();
    success(&default_output);
    assert_eq!(
        membership(&preparation(&defaults)).capabilities,
        std::collections::BTreeSet::from([
            HumanCapabilityV1::ReadMetadata,
            HumanCapabilityV1::ReadContent,
            HumanCapabilityV1::Export
        ])
    );
    no_secret(&default_output, &secret(&defaults));
}

#[test]
fn schema_seven_replays_exact_historical_preparations_without_rotation_or_new_grants() {
    for historical in [4, 5, 6] {
        let files = support::private_tempdir();
        let data = files.path().join("audit.sqlite");
        initialized(&data, "tenant-a", true);
        migrate_human_cli_to(&data, historical);
        let output = files.path().join("retained-reader");
        let provisioned = provision_command(&data, "tenant-a", &output, "Historical reader")
            .args(["--max-events", "5000"])
            .output()
            .unwrap();
        success(&provisioned);
        let bearer = secret(&output);
        no_secret(&provisioned, &bearer);
        let prepared = preparation(&output);
        let original_membership = membership(&prepared);
        assert!(
            !original_membership
                .capabilities
                .contains(&HumanCapabilityV1::ReadBrokerRegistry)
        );
        let recovery = std::fs::read(output.join("recovery.json")).unwrap();
        let completion = std::fs::read(output.join("completion.json")).unwrap();
        std::fs::rename(
            output.join("completion.json"),
            output.join("retained-completion.json"),
        )
        .unwrap();
        migrate_human_cli_to(&data, 7);
        let before = store(&data, "tenant-a").integrity().unwrap();
        let resumed = resume_command(&data, "tenant-a", &output)
            .args(["--max-events", "5000"])
            .output()
            .unwrap();
        success(&resumed);
        no_secret(&resumed, &bearer);
        assert_eq!(secret(&output), bearer);
        assert_eq!(
            std::fs::read(output.join("recovery.json")).unwrap(),
            recovery
        );
        assert_eq!(
            std::fs::read(output.join("completion.json")).unwrap(),
            completion
        );
        let audit = store(&data, "tenant-a");
        assert_eq!(audit.integrity().unwrap().head_hash, before.head_hash);
        assert_eq!(
            audit
                .identity_memberships(&original_membership.principal)
                .unwrap(),
            [original_membership]
        );
        assert_eq!(audit.supervision_state().unwrap().schema_version, 7);
        assert!(!audit.supervision_state().unwrap().active);
    }
}

#[tokio::test]
async fn schema_four_human_get_requires_explicit_migration_without_read_audit() {
    let files = support::private_tempdir();
    let data = files.path().join("audit.sqlite");
    initialized(&data, "tenant-a", true);
    let alice = files.path().join("alice");
    success(
        &provision_command(&data, "tenant-a", &alice, "Alice")
            .output()
            .unwrap(),
    );
    let audit = Arc::new(store(&data, "tenant-a"));
    let head = audit.head().unwrap();
    let router = gateway_router(GatewayState::new(
        audit.clone(),
        "schema-four-runner-test".into(),
        "schema-four-viewer-test".into(),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    ));
    let (origin, server) = serve(router).await;
    let output = read(&origin, &alice.join("credential"), "identity", &[]).await;
    failure(&output);
    no_secret(&output, &secret(&alice));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("HTTP 503"));
    assert!(stderr.contains("migration_required"));
    assert!(stderr.contains("contact_operator"));
    assert!(!stderr.contains("protocol_error"));
    assert_eq!(audit.head().unwrap(), head);
    assert_eq!(audit.supervision_state().unwrap().schema_version, 4);
    assert!(!audit.supervision_state().unwrap().active);
    server.abort();
}

#[tokio::test]
async fn migration_error_preserves_public_code_and_local_guidance_without_hostile_body() {
    let fixture = HumanFixture::new(0);
    let bearer = std::fs::read_to_string(&fixture.credential_a1).unwrap();
    let bearer = bearer.trim().to_owned();
    let echoed = bearer.clone();
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let router = axum::Router::new().route("/api/v1/human/identity", axum::routing::get(move || {
        let echoed = echoed.clone();
        let observed = observed.clone();
        async move {
            let code = if observed.fetch_add(1,Ordering::SeqCst) == 0 { "migration_required" } else { "unsupported_future_code" };
            (axum::http::StatusCode::SERVICE_UNAVAILABLE,[("cache-control","no-store")],axum::Json(json!({"error":{
                "code":code,"retry_action":"contact_operator", "message":format!("hostile-message-marker {echoed} https://example.invalid/private?bearer={echoed}")
            }})))
        }
    }));
    let (origin, server) = serve(router).await;
    let output = read(&origin, &fixture.credential_a1, "identity", &[]).await;
    failure(&output);
    no_secret(&output, &bearer);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("HTTP 503"));
    assert!(stderr.contains("migration_required"));
    assert!(stderr.contains("contact_operator"));
    assert!(stderr.contains("stop") && stderr.contains("backup"));
    assert!(
        !stderr.contains("hostile-message-marker")
            && !stderr.contains("example.invalid")
            && !stderr.contains("protocol_error")
    );
    let unknown = read(&origin, &fixture.credential_a1, "identity", &[]).await;
    failure(&unknown);
    no_secret(&unknown, &bearer);
    let stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(stderr.contains("protocol_error"));
    assert!(
        !stderr.contains("unsupported_future_code")
            && !stderr.contains("hostile-message-marker")
            && !stderr.contains("example.invalid")
    );
    server.abort();
}

#[tokio::test]
async fn public_authorization_version_refuses_numeric_or_noncanonical_hex() {
    let fixture = HumanFixture::new(0);
    let bearer = std::fs::read_to_string(&fixture.credential_a1).unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let router = axum::Router::new().route("/api/v1/human/resources", axum::routing::get(move || {
        let observed = observed.clone();
        async move {
            let version = if observed.fetch_add(1,Ordering::SeqCst) == 0 { "7".to_owned() } else { "A".repeat(64) };
            ([("cache-control","no-store")],axum::Json(json!({
                "version":1,"context_id":"offered-test-context","authorization_version":version,
                "items":[],"next_cursor":null,"has_more":false,"coverage":"authorized_selection",
                "read_receipt":{"request_id":Uuid::new_v4().to_string(),"audit_event_id":Uuid::new_v4().to_string(),"received_at":chrono::Utc::now().to_rfc3339()}
            })))
        }
    }));
    let (origin, server) = serve(router).await;
    for _ in 0..2 {
        let output = read(
            &origin,
            &fixture.credential_a1,
            "resources",
            &["--context", "offered-test-context"],
        )
        .await;
        failure(&output);
        no_secret(&output, bearer.trim());
        assert!(String::from_utf8_lossy(&output.stderr).contains("opaque authorization version"));
    }
    server.abort();
}

#[test]
fn human_gateway_ca_configuration_is_rejected_before_credentials_or_http() {
    let temp = support::private_tempdir();
    let missing_credential = temp.path().join("credential-must-not-be-opened");
    for (label, bytes) in [
        ("invalid", b"not a PEM certificate".to_vec()),
        (
            "key",
            b"-----BEGIN PRIVATE KEY-----\nAA==\n-----END PRIVATE KEY-----\n".to_vec(),
        ),
        (
            "unknown-section",
            b"-----BEGIN OPAQUE TEST BLOCK-----\nAA==\n-----END OPAQUE TEST BLOCK-----\n".to_vec(),
        ),
        ("oversized", vec![b' '; 65_537]),
    ] {
        let ca = temp.path().join(label);
        private_file(&ca, &bytes);
        for mode in ["identity", "sso"] {
            let mut cmd = command();
            cmd.env_clear()
                .args([
                    "human",
                    mode,
                    "--gateway",
                    "https://127.0.0.1:9",
                    "--gateway-ca-file",
                ])
                .arg(&ca);
            if mode == "identity" {
                cmd.arg("--credential-file").arg(&missing_credential);
            } else {
                cmd.args(["--browser", "manual"]);
            }
            let rejected = cmd.output().unwrap();
            failure(&rejected);
            let stderr = String::from_utf8_lossy(&rejected.stderr);
            assert!(
                stderr.contains("gateway CA configuration is invalid or unavailable"),
                "invalid CA must fail as configuration, before credentials or HTTP; unknown option is not an accepted refusal"
            );
            assert!(!stderr.contains(ca.to_str().unwrap()));
            assert!(!stderr.contains(missing_credential.to_str().unwrap()));
            assert!(!stderr.contains("BEGIN "));
        }
    }
}

#[test]
fn human_gateway_ca_requires_https_in_both_modes() {
    let temp = support::private_tempdir();
    // No file is needed: an explicit CA on HTTP is an incompatible choice,
    // rejected before either trust or credential paths are opened.
    let ca = temp.path().join("not-opened.pem");
    for mode in ["identity", "sso"] {
        let mut cmd = command();
        cmd.env_clear()
            .args([
                "human",
                mode,
                "--gateway",
                "http://127.0.0.1:9",
                "--gateway-ca-file",
            ])
            .arg(&ca);
        if mode == "identity" {
            cmd.arg("--credential-file")
                .arg(temp.path().join("not-opened-credential"));
        } else {
            cmd.args(["--browser", "manual"]);
        }
        let rejected = cmd.output().unwrap();
        failure(&rejected);
        let stderr = String::from_utf8_lossy(&rejected.stderr);
        assert!(stderr.contains("--gateway-ca-file requires an HTTPS gateway origin"));
        assert!(!stderr.contains(ca.to_str().unwrap()));
    }
}
