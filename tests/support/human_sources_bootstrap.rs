use agentsandbox::{
    audit::{
        Store,
        authentication::{
            AuthenticationAudit, AuthenticationAuditPolicyV1, AuthenticationCapacityPlanV1,
        },
    },
    identity::*,
    redaction::Redactor,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output},
};
use uuid::Uuid;

pub struct Fixture {
    pub _temporary: tempfile::TempDir,
    pub root: PathBuf,
    pub registry: PathBuf,
    pub preparation: PathBuf,
    pub installation: Uuid,
    pub oidc: Option<PathBuf>,
    pub sources: Vec<PathBuf>,
    pub human_bearer: String,
}
pub fn private_dir(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
pub fn private_file(path: &Path, bytes: &[u8]) {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    f.write_all(bytes).unwrap();
    f.sync_all().unwrap();
}
impl Fixture {
    pub fn new(oidc: bool) -> Self {
        let temporary = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let installation = Uuid::new_v4();
        let human_bearer = format!("aigh1_{}", "42".repeat(32));
        let principal = PrincipalRefV1::HumanLocal {
            installation_id: installation.to_string(),
            account_id: Uuid::new_v4().to_string(),
        };
        let mut sources = Vec::new();
        let mut tenants = Vec::new();
        for tenant in ["tenant-a", "tenant-b"] {
            let parent = root.join(tenant);
            private_dir(&parent);
            let historical = root.join(format!("historical-{tenant}"));
            private_dir(&historical);
            let path = parent.join("events.sqlite");
            let store =
                Store::open_for_tenant(&path, tenant, Redactor::new(vec![]), 100_000).unwrap();
            for (i, migrate) in [
                Store::migrate_control_v3,
                Store::migrate_identity_v4,
                Store::migrate_human_access_v5,
                Store::migrate_workload_v6,
                Store::migrate_human_brokers_v7,
                Store::migrate_kubernetes_create_v8,
                Store::migrate_kubernetes_policy_v9,
                Store::migrate_human_workers_v10,
                Store::migrate_kubernetes_session_reservations_v11,
                Store::migrate_broker_checkpoints_v12,
                Store::migrate_kubernetes_recovery_v13,
            ]
            .into_iter()
            .enumerate()
            {
                migrate(&store, &historical.join(format!("before-{}.sqlite", i + 3))).unwrap();
            }
            let mutation = |change| IdentityMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                actor: PrincipalRefV1::LegacyControl {
                    credential_id: "synthetic-stopped-operator".into(),
                    role: LegacyControlRoleV1::Runner,
                },
                change,
            };
            let account = mutation(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Synthetic named operator".into(),
                    revision: 1,
                    active: true,
                },
            });
            // Only A receives the bearer: B has an independently valid person
            // but must never appear in this authenticated user's projection.
            let token = if tenant == "tenant-a" {
                human_bearer.clone()
            } else {
                format!("aigh1_{}", "24".repeat(32))
            };
            let credential = mutation(IdentityChangeV1::Credential {
                expected_revision: 0,
                binding: LocalCredentialV1 {
                    version: 1,
                    credential_id: Uuid::new_v4().to_string(),
                    principal: principal.clone(),
                    sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                    revision: 1,
                    active: true,
                    expires_at: None,
                },
            });
            let membership = mutation(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: MembershipV1 {
                    version: 1,
                    membership_id: Uuid::new_v4().to_string(),
                    principal: principal.clone(),
                    tenant_id: tenant.into(),
                    organization_scope: "synthetic-org".into(),
                    revision: 1,
                    active: true,
                    capabilities: BTreeSet::from([
                        HumanCapabilityV1::ReadMetadata,
                        HumanCapabilityV1::ReadContent,
                    ]),
                },
            });
            store
                .provision_local_human(&LocalHumanProvisionV1 {
                    version: 1,
                    mutations: [account, credential, membership],
                })
                .unwrap();
            assert_eq!(store.supervision_state().unwrap().schema_version, 13);
            drop(store);
            let runner = root.join(format!("{tenant}-runner.token"));
            let viewer = root.join(format!("{tenant}-viewer.token"));
            private_file(
                &runner,
                format!("synthetic-{tenant}-runner-credential-long-enough").as_bytes(),
            );
            private_file(
                &viewer,
                format!("synthetic-{tenant}-viewer-credential-long-enough").as_bytes(),
            );
            tenants.push(json!({"tenant_id":tenant,"data":path,"runner_token_file":runner,"viewer_token_file":viewer}));
            sources.push(path);
        }
        let registry = root.join("tenants.json");
        private_file(
            &registry,
            &serde_json::to_vec(&json!({"version":1,"tenants":tenants})).unwrap(),
        );
        let oidc = if oidc {
            let auth_parent = root.join("authentication");
            private_dir(&auth_parent);
            let auth_path = auth_parent.join("audit.sqlite");
            let ca = root.join("public-ca.pem");
            private_file(
                &ca,
                include_bytes!("../diagnose_support/fixtures/public-test-root.pem"),
            );
            let mut config: Value = serde_json::from_slice(include_bytes!(
                "../../docs/setup/sso/examples/gateway.https.json"
            ))
            .unwrap();
            config["authentication_audit"]["path"] = json!(auth_path);
            config["authentication_audit"]["installation_id"] = json!(installation);
            config["ca_bundle_file"] = json!(ca);
            let policy: AuthenticationAuditPolicyV1 =
                serde_json::from_value(config["authentication_audit"]["policy"].clone()).unwrap();
            let plan: AuthenticationCapacityPlanV1 =
                serde_json::from_value(config["authentication_audit"]["capacity_plan"].clone())
                    .unwrap();
            let audit = AuthenticationAudit::init_new_v4(
                &auth_path,
                installation,
                policy,
                plan,
                Redactor::new(vec![]),
            )
            .unwrap();
            assert_eq!(audit.format_version().unwrap(), 4);
            drop(audit);
            sources.push(auth_path);
            let path = root.join("oidc.json");
            private_file(&path, &serde_json::to_vec(&config).unwrap());
            Some(path)
        } else {
            None
        };
        Self {
            _temporary: temporary,
            preparation: root.join("preparation"),
            root,
            registry,
            installation,
            oidc,
            sources,
            human_bearer,
        }
    }
    pub fn prepare(&self) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        command.env_clear();
        command
            .args(["human-sources", "prepare", "--tenant-registry"])
            .arg(&self.registry)
            .arg("--installation-id")
            .arg(self.installation.to_string())
            .args([
                "--control-config-revision",
                "7",
                "--max-events",
                "100000",
                "--output-dir",
            ])
            .arg(&self.preparation)
            .arg("--confirm-control-stopped");
        if let Some(path) = &self.oidc {
            command
                .arg("--oidc-config")
                .arg(path)
                .args(["--oidc-mapping-revision", "9"]);
        }
        command.output().unwrap()
    }
    pub fn step(&self, step: &str, stopped: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        command
            .env_clear()
            .args(["human-sources", step, "--preparation-dir"])
            .arg(&self.preparation);
        if stopped {
            command.arg("--confirm-control-stopped");
        }
        command.output().unwrap()
    }
}
