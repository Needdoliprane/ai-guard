//! The tenant registry, and its correspondence with the capacity plan of the
//! OIDC file.
//!
//! The registry is decoded by `gateway_files`, as the gateway loader reads it.
//! What is concluded here from the two files is `inferred`: the gateway also
//! compares the plan with the one stored in the authentication audit
//! (`src/gateway_config.rs`, `load_oidc`; `src/api.rs`, `with_oidc`); this
//! diagnostic does not open that durable state.

use super::{
    Context, Draft,
    catalogue::FindingCode,
    gateway_files::{self, Refusal, RegistryInspection},
    identity::OidcFacts,
    output::IdTable,
    references::{self, Declared, RawOwner},
    report::{
        ContentRole, Correspondence, DataRole, Details, InputName, MAX_SUBJECT_IDS, PolicyKind,
        ReferenceRole, Scope,
    },
};
use std::{collections::BTreeSet, path::Path};

const REGISTRY_IDS: &str = "tenants[].tenant_id";
const PLAN_IDS: &str = "authentication_audit.capacity_plan.tenants[].tenant_id";

/// One registry to report: the one of an installation directory, or a file
/// given alone.
pub(crate) struct RegistrySource {
    pub input: InputName,
    /// How the file is named as the origin of a finding.
    pub label: &'static str,
    /// False for a setup staging directory, whose registry records the final
    /// paths: the entries it names are not described.
    pub describe_entries: bool,
    pub inspection: RegistryInspection,
}

/// What the rest of the report needs from a registry.
pub(crate) struct RegistryFacts {
    pub decoded: bool,
    /// Declared identifiers, one per entry, in file order.
    pub ids: Vec<String>,
    pub uses_max_events: bool,
    pub label: &'static str,
}

/// The fields of the registry a refusal is about.
fn fields(refusal: &Refusal) -> &'static [&'static str] {
    match (refusal.code, &refusal.details) {
        (FindingCode::RegistryVersionUnknown, _) => &["version"],
        (FindingCode::RegistryEmpty, _) => &["tenants"],
        (FindingCode::RegistryTenantIdInvalid | FindingCode::RegistryTenantIdDuplicate, _) => {
            &[REGISTRY_IDS]
        }
        (FindingCode::RegistryStorageInvalid | FindingCode::RegistryMySqlUnavailable, _) => {
            &["tenants[].storage"]
        }
        (FindingCode::RegistryReferenceEmpty, _) => &[
            "tenants[].data",
            "tenants[].storage.path",
            "tenants[].runner_token_file",
            "tenants[].viewer_token_file",
        ],
        (_, Some(Details::Policy { policy })) => match policy {
            PolicyKind::WorkerQuota => &["tenants[].worker_quota"],
            PolicyKind::KubernetesHeldCreate => &["tenants[].held_create_policy"],
            PolicyKind::KubernetesSessionReservation => &["tenants[].session_reservation_policy"],
            PolicyKind::KubernetesSessionRecovery => &["tenants[].session_recovery_policy"],
            PolicyKind::KubernetesSessionControlRecovery => {
                &["tenants[].session_control_recovery_policy"]
            }
            PolicyKind::AuditCapacity => &["tenants[].audit_capacity_policy"],
            PolicyKind::WorkloadAudit => &["tenants[].workload_audit_policy"],
            PolicyKind::RelayRetention => &["tenants[].relay_retention_policy"],
        },
        _ => &[],
    }
}

/// Whether the verdict of a refusal depends on the gateway event bound: the
/// validators of these two policies take the bound as an argument.
fn depends_on_event_bound(refusal: &Refusal) -> bool {
    matches!(
        refusal.details,
        Some(Details::Policy {
            policy: PolicyKind::AuditCapacity | PolicyKind::WorkloadAudit
        })
    )
}

/// Read a registry as the gateway reads it. The read is recorded here, where
/// it happens: a registry is also read to tell what a directory is, and it is
/// listed as read whatever becomes of the directory. The key files and the
/// stores it names are registered at once, whatever is described of them
/// afterwards: a staging directory records the final paths, which are not
/// described, and a file option or a CA reference that points at one of them
/// is never opened.
pub(crate) fn inspect(context: &mut Context, path: &Path, max_events: u64) -> RegistryInspection {
    let inspection = gateway_files::inspect_registry(path, max_events);
    if inspection.read {
        context.opened(ContentRole::TenantRegistry, path);
    }
    for path in &inspection.protected_paths.files {
        context.never_opened.add(path);
    }
    for path in &inspection.protected_paths.stores {
        context.never_opened.add_store(path);
    }
    inspection
}

/// Report one registry: its refusals or its offline verdict, then the key
/// files and the store of every tenant. `bound_assumed` is true when the
/// gateway event bound is the default of this tool, not a given value.
pub(crate) fn report_registry(
    context: &mut Context,
    source: &RegistrySource,
    bound_assumed: bool,
) -> RegistryFacts {
    let inspection = &source.inspection;
    let file = Scope::File {
        file: ContentRole::TenantRegistry,
    };
    for refusal in &inspection.refusals {
        // A refusal that rests on the assumed event bound is not a certain
        // one: the gateway may be started with another bound.
        let on_assumed_bound = bound_assumed && depends_on_event_bound(refusal);
        let code = if on_assumed_bound {
            FindingCode::RegistryPolicyRefusedAtAssumedEventBound
        } else {
            refusal.code
        };
        let mut draft = Draft::new(code, file.clone());
        let fields = fields(refusal);
        if fields.is_empty() {
            draft = draft.source(source.label, None);
        }
        for field in fields.iter().copied() {
            draft = draft.source(source.label, Some(field));
        }
        if let Some(details) = &refusal.details {
            draft = draft.details(details.clone());
        }
        if let Some(id) = &refusal.tenant_id {
            draft = draft.tenant(id);
        }
        if let Some(variant) = refusal.variant {
            // The registry of an installation directory has a fixed name:
            // no option of this tool names it.
            draft = draft.variant(match (variant, source.input) {
                ("absent", InputName::InstallationDir) => "absent_from_installation",
                _ => variant,
            });
        }
        if on_assumed_bound {
            draft = draft.assumes(InputName::MaxEvents);
        }
        context.report(draft);
    }
    let facts = RegistryFacts {
        decoded: inspection.decoded,
        ids: inspection
            .tenants
            .iter()
            .map(|tenant| tenant.tenant_id.clone())
            .collect(),
        uses_max_events: inspection.decoded && inspection.uses_max_events,
        label: source.label,
    };
    if !inspection.decoded {
        return facts;
    }
    if inspection.refusals.is_empty() {
        let mut passed = Draft::new(FindingCode::RegistryOfflineRulesPassed, file.clone())
            .source(source.label, None);
        if bound_assumed && inspection.uses_max_events {
            passed = passed.assumes(InputName::MaxEvents);
        }
        context.report(passed);
        context.report(
            Draft::new(
                FindingCode::RegistryLoaderAcceptanceNotChecked,
                file.clone(),
            )
            .source(source.label, None),
        );
    }
    context.report(
        Draft::new(FindingCode::TopologyRegistryTenants, file)
            .source(source.label, Some("tenants")),
    );
    if !source.describe_entries {
        return facts;
    }
    let mut stores = false;
    for tenant in &inspection.tenants {
        for (role, path) in [
            (ReferenceRole::Runner, &tenant.runner_token_file),
            (ReferenceRole::Viewer, &tenant.viewer_token_file),
        ] {
            if let Some(path) = path {
                references::gateway_token(
                    context,
                    Declared {
                        path,
                        owner: RawOwner::tenant(&tenant.tenant_id),
                        role,
                        input: source.input,
                        init_target: None,
                    },
                );
            }
        }
        if let Some(path) = &tenant.data {
            stores = true;
            references::store(
                context,
                path,
                RawOwner::tenant(&tenant.tenant_id),
                DataRole::TenantStore,
                source.input,
                false,
            );
        }
    }
    if stores {
        context.report(
            Draft::new(
                FindingCode::DataOfflineInspectionNotAvailable,
                Scope::Input {
                    input: source.input,
                },
            )
            .variant("prepared"),
        );
        context.report(Draft::new(
            FindingCode::DataSignedCheckpointNotChecked,
            Scope::Input {
                input: source.input,
            },
        ));
    }
    facts
}

/// Compare the tenant set of the registry with the capacity plan, by exact
/// string equality, as the gateway does (`src/api.rs`, `with_oidc`). `None`
/// for a file means that none was given; a file that could not be decoded
/// leaves the comparison not checked. `historical` is true when a historical
/// config directory is inspected: a gateway started with it and without a
/// registry serves one tenant.
pub(crate) fn correspondence(
    context: &mut Context,
    registry: Option<&RegistryFacts>,
    oidc: Option<&OidcFacts>,
    historical: bool,
    scope: Scope,
    ids: &IdTable,
) -> Option<Correspondence> {
    let (registry, oidc) = match (registry, oidc) {
        (None, None) => return None,
        (Some(registry), Some(oidc)) => (registry, oidc),
        (registry, _) => {
            let draft = Draft::new(FindingCode::TenantsCorrespondenceNotChecked, Scope::Tenants);
            // An OIDC file beside a historical directory, and no registry:
            // the rule of the single tenant is what the operator needs.
            context.report(if registry.is_none() && historical {
                draft.variant("single_local")
            } else {
                draft
            });
            return None;
        }
    };
    let (true, Some(version)) = (registry.decoded, oidc.version) else {
        context.report(
            Draft::new(FindingCode::TenantsCorrespondenceNotChecked, Scope::Tenants)
                .variant("undecoded"),
        );
        return None;
    };
    let declared: BTreeSet<&str> = registry.ids.iter().map(String::as_str).collect();
    let draft = |code, plan_field: &'static str| {
        Draft::new(code, scope.clone())
            .source(registry.label, Some(REGISTRY_IDS))
            .source(oidc.label, Some(plan_field))
    };
    let durable = |context: &mut Context| {
        context.report(draft(
            FindingCode::TenantsDurablePlanComparisonNotAvailable,
            "authentication_audit.capacity_plan",
        ));
    };
    match (version, &oidc.plan) {
        (2, Some(plan)) => {
            let planned: BTreeSet<&str> = plan.iter().map(String::as_str).collect();
            if planned == declared {
                context.report(draft(FindingCode::TenantsPlanMatchesRegistry, PLAN_IDS));
                durable(context);
                return Some(Correspondence::Match);
            }
            let side = |from: &BTreeSet<&str>, other: &BTreeSet<&str>| {
                let only: Vec<&str> = from.difference(other).copied().collect();
                let listed = only
                    .iter()
                    .take(MAX_SUBJECT_IDS)
                    .map(|id| ids.display(id))
                    .collect::<Vec<_>>();
                let omitted = only.len() - listed.len();
                (listed, omitted)
            };
            let (only_in_registry, omitted_registry) = side(&declared, &planned);
            let (only_in_plan, omitted_plan) = side(&planned, &declared);
            context.report(
                draft(FindingCode::TenantsPlanSetMismatch, PLAN_IDS).details(
                    Details::TenantSetDifference {
                        only_in_registry,
                        only_in_plan,
                        omitted: u64::try_from(omitted_registry + omitted_plan).unwrap_or(u64::MAX),
                    },
                ),
            );
            // The stored plan is the third party to the comparison: what
            // the operator may change depends on it.
            durable(context);
            Some(Correspondence::Mismatch)
        }
        (1, None) if declared.len() > 1 => {
            context.report(draft(
                FindingCode::TenantsPlanRequiredForMultiTenant,
                "version",
            ));
            None
        }
        (1, None) if declared.len() == 1 => {
            context.report(draft(
                FindingCode::TenantsSingleTenantWithoutPlan,
                "version",
            ));
            durable(context);
            None
        }
        // Any other pair of version and plan is refused as such by the
        // inspection of the OIDC file; an empty registry by its own.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{DiagnoseArgs, build_report};
    use clap::Parser;
    use serde_json::json;
    use std::{
        ffi::OsStr,
        fs,
        os::unix::fs::{MetadataExt, PermissionsExt},
    };

    #[derive(Parser)]
    struct TestCommand {
        #[command(flatten)]
        arguments: DiagnoseArgs,
    }

    #[tokio::test]
    async fn a_mysql_private_reference_does_not_open_as_a_gateway_ca() {
        let dir = tempfile::tempdir().unwrap();
        let writer_password = dir.path().join("writer-password");
        let alias = dir.path().canonicalize().unwrap().join("writer-password");
        let registry = dir.path().join("tenants.json");
        fs::write(&writer_password, "synthetic-mysql-secret-must-not-be-read").unwrap();
        fs::set_permissions(&writer_password, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(fs::metadata(&alias).unwrap().nlink(), 1);
        let args = TestCommand::try_parse_from([
            OsStr::new("ai-guard-diagnose"),
            OsStr::new("--tenants-file"),
            registry.as_os_str(),
            OsStr::new("--gateway-ca-file"),
            alias.as_os_str(),
        ])
        .unwrap();

        let valid = json!({"version":1,"tenants":[{
            "tenant_id":"tenant-a","runner_token_file":"runner","viewer_token_file":"viewer",
            "storage":{"backend":"mysql","host":"db.example.invalid","port":3306,
                "database":"audit_a","username":"writer_a","namespace":"5657019d-5302-4ced-922f-406aa3d8ec5c",
                "password_file":"writer-password","ca_file":"mysql-ca.pem",
                "authentication":{"mode":"password_and_mtls","client_cert_file":"mysql-client.pem","client_key_file":"mysql-client.key"}
            }
        }]});
        let mut invalid_port = valid.clone();
        invalid_port["tenants"][0]["storage"]["port"] = json!(0);
        let mut incomplete_mtls = valid.clone();
        incomplete_mtls["tenants"][0]["storage"]["authentication"]
            .as_object_mut()
            .unwrap()
            .remove("client_key_file");
        for (document, code) in [
            (valid, FindingCode::RegistryMySqlUnavailable),
            (invalid_port, FindingCode::RegistryJsonInvalid),
            (incomplete_mtls, FindingCode::RegistryJsonInvalid),
        ] {
            fs::write(&registry, serde_json::to_vec(&document).unwrap()).unwrap();
            let report = build_report(&args.arguments).await.unwrap();
            assert!(report.findings.iter().any(|finding| finding.code == code));
            assert!(
                report
                    .findings
                    .iter()
                    .any(|finding| finding.code
                        == FindingCode::TrustCaReferenceIsACredentialReference)
            );
            assert_eq!(
                report.mode.files_read_for_content,
                [ContentRole::TenantRegistry]
            );
            let rendered = serde_json::to_string(&report).unwrap();
            assert!(!rendered.contains("synthetic-mysql-secret-must-not-be-read"));
            assert!(!rendered.contains("writer-password"));
        }
    }
}
