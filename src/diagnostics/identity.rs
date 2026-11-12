//! Single sign-on, as an OIDC file declares it.
//!
//! SSO is optional and independent of the AI provider axis: nothing here
//! loads a provider profile. The file is decoded by `gateway_files`, as the
//! gateway prepares it, and the authentication audit is never opened. The
//! parameters shown are the public ones: issuer, client identifiers, endpoints
//! and redirections. Every URL is rebuilt from its parsed value, so that user
//! information, a query or a fragment written in the file never reaches the
//! report.
//!
//! The identity provider is contacted by one function only, `reachability`,
//! and only when the operator asks for the active discovery. It calls the
//! discovery of the product's own client, with the bounds of that client and
//! of the file: no proxy, no redirect, no retry, the time limit and the size
//! bound of the file, two GET requests and no credential. That client
//! returns a closed error and nothing else: no status line, header, body or
//! transport text exists here to be printed.

use super::{
    Context, Draft,
    catalogue::FindingCode,
    fs_facts,
    gateway_files::{self, CaBundle, OidcInspection, Refusal},
    output::MAX_DYNAMIC_BYTES,
    references::{self, RawOwner},
    report::{
        ContentRole, DataRole, Details, InputName, Reason, Scope, Sso, SsoMapping, SsoTransport,
        TrustEntry,
    },
};
use crate::oidc::{OidcError, OidcTransportModeV1};
use std::path::PathBuf;

/// One OIDC file to report: the one of an installation directory, or a file
/// given alone.
pub(crate) struct OidcSource {
    pub path: PathBuf,
    pub input: InputName,
    /// How the file is named as the origin of a finding.
    pub label: &'static str,
    /// False for a setup staging directory, whose file records the final
    /// paths: the authentication audit it names is not described.
    pub describe_entries: bool,
}

/// What the rest of the report needs from an OIDC file.
pub(crate) struct OidcFacts {
    /// The file version, when the file decoded.
    pub version: Option<u32>,
    /// Tenant identifiers of the capacity plan, in file order.
    pub plan: Option<Vec<String>>,
    pub sso: Option<Sso>,
    pub trust: Vec<TrustEntry>,
    /// The references the file resolves.
    pub paths: Vec<PathBuf>,
    pub label: &'static str,
    /// The input that names the file.
    input: InputName,
    /// The inspection of a file that decoded: what the client of the gateway
    /// is built from, when the file passes every offline rule.
    decoded: Option<OidcInspection>,
}

/// The fields of the gateway block that the product check behind a reason
/// reads (`src/api/oidc.rs`, `OidcGatewayConfigV1::validate`). For the
/// provider object the validator returns one closed value, so its reason
/// names the whole object.
fn gateway_block_fields(reason: Reason) -> &'static [&'static str] {
    match reason {
        Reason::VersionOrTokenBound => &[
            "gateway.version",
            "gateway.provider.limits.max_token_bytes",
            "gateway.provider.limits.pkce_ttl_seconds",
        ],
        Reason::UrlUnparseable => &["gateway.web_origin", "gateway.provider.web_redirect_uri"],
        Reason::WebOriginOrCallback => &[
            "gateway.web_origin",
            "gateway.provider.web_redirect_uri",
            "gateway.provider.transport",
        ],
        Reason::ClaimMappingInvalid => &[
            "gateway.provider_label",
            "gateway.mapping.revision",
            "gateway.mapping.rules",
            "gateway.mapping.default_capabilities",
        ],
        Reason::ClaimMappingAmbiguous => &["gateway.mapping.rules"],
        Reason::ProviderBlockInvalid => &["gateway.provider"],
        _ => &["gateway"],
    }
}

/// The fields of the OIDC file a refusal is about.
fn fields(refusal: &Refusal) -> &'static [&'static str] {
    match refusal.code {
        FindingCode::OidcVersionPlanPairInvalid => {
            &["version", "authentication_audit.capacity_plan"]
        }
        FindingCode::OidcCapacityPlanInvalid => &["authentication_audit.capacity_plan"],
        FindingCode::OidcGatewayBlockInvalid => match refusal.details {
            Some(Details::Reason { reason }) => gateway_block_fields(reason),
            _ => &["gateway"],
        },
        FindingCode::OidcReferenceInvalid => &["ca_bundle_file", "authentication_audit.path"],
        FindingCode::OidcAuthenticationPolicyInvalid => &["authentication_audit.policy"],
        FindingCode::TrustCaReferenceIsACredentialReference
        | FindingCode::TrustCaUnavailableOrNotPrivate
        | FindingCode::TrustCaBundleInvalid
        | FindingCode::TrustCaIncompatibleWithTransport => &["ca_bundle_file"],
        _ => &[],
    }
}

/// A finding about the CA bundle is scoped to that file, any other to the
/// OIDC file.
fn scope(code: FindingCode) -> Scope {
    Scope::File {
        file: match code {
            FindingCode::TrustCaReferenceIsACredentialReference
            | FindingCode::TrustCaUnavailableOrNotPrivate
            | FindingCode::TrustCaBundleInvalid
            | FindingCode::TrustCaIncompatibleWithTransport
            | FindingCode::TrustOidcCaBundleAcceptedByGrammar
            | FindingCode::TrustCertificateValidityPeriodNotAvailable => ContentRole::OidcCaBundle,
            _ => ContentRole::OidcFile,
        },
    }
}

/// The public parameters of a decoded gateway block, valid or not.
fn sso(context: &Context, inspection: &OidcInspection) -> Option<Sso> {
    let gateway = inspection.gateway.as_ref()?;
    let provider = &gateway.provider;
    let text = |value: &str| Some(context.safe.dynamic(value, MAX_DYNAMIC_BYTES));
    let url = |value: &str| Some(context.safe.url(value));
    Some(Sso {
        file_version: inspection.version,
        issuer: url(&provider.issuer),
        transport: Some(match provider.transport {
            OidcTransportModeV1::HttpsOnly => SsoTransport::HttpsOnly,
            OidcTransportModeV1::IsolatedDemoHttp { .. } => SsoTransport::IsolatedDemoHttp,
        }),
        provider_label: text(&gateway.provider_label),
        web_origin: url(&gateway.web_origin),
        web_client_id: text(&provider.web_client_id),
        cli_client_id: text(&provider.cli_client_id),
        api_audience: text(&provider.api_audience),
        authorization_endpoint: url(&provider.authorization_endpoint),
        token_endpoint: url(&provider.token_endpoint),
        jwks_uri: url(&provider.jwks_uri),
        end_session_endpoint: provider.end_session_endpoint.as_deref().and_then(url),
        web_redirect_uri: url(&provider.web_redirect_uri),
        cli_callback_path: text(&provider.cli_callback_path),
        mapping: Some(SsoMapping {
            revision: context
                .safe
                .dynamic(&gateway.mapping.revision, MAX_DYNAMIC_BYTES),
            rule_count: u64::try_from(gateway.mapping.rules.len()).unwrap_or(u64::MAX),
        }),
        ca_bundle_declared: inspection.ca_bundle_declared,
        idp_contacted: false,
    })
}

/// Report one OIDC file: its refusals or its offline verdict, its CA bundle
/// and the authentication audit it names. A CA reference that points at a
/// file this installation uses as a key file, a setup record or a store is
/// never opened: every such file is already known when this runs.
pub(crate) fn report_oidc(context: &mut Context, source: &OidcSource) -> OidcFacts {
    // The same rule for the OIDC file itself, given by option or opened at
    // its fixed name in an installation directory.
    if context.never_opened.contains(&source.path) {
        context.report(
            Draft::new(
                FindingCode::OidcFileUnavailableOrNotPrivate,
                Scope::Input {
                    input: source.input,
                },
            )
            .variant(if source.input == InputName::OidcConfig {
                "reserved"
            } else {
                "reserved_name"
            }),
        );
        return OidcFacts {
            version: None,
            plan: None,
            sso: None,
            trust: Vec::new(),
            paths: Vec::new(),
            label: source.label,
            input: source.input,
            decoded: None,
        };
    }
    let inspection = {
        let never_opened = &context.never_opened;
        gateway_files::inspect_oidc_file_guarded(&source.path, &|ca, audit| {
            // The audit that this same file names is known before its CA
            // reference is opened: a reference to it is not opened either.
            let mut named_by_this_file = fs_facts::NeverOpened::default();
            if let Some(audit) = audit {
                named_by_this_file.add_store(audit);
            }
            !never_opened.contains(ca) && !named_by_this_file.contains(ca)
        })
    };
    // The audit that the file names is never opened afterwards, whether it
    // is described or not: a staging directory records the final path.
    if let Some(path) = &inspection.audit_path {
        context.never_opened.add_store(path);
    }
    if inspection.read {
        context.opened(ContentRole::OidcFile, &source.path);
    }
    for refusal in inspection.refusals.iter().chain(&inspection.extra_refusals) {
        let mut draft = Draft::new(refusal.code, scope(refusal.code));
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
        if let Some(variant) = refusal.variant {
            draft = draft.variant(variant);
        }
        // One closed value stands for every rule of the provider object:
        // the message says so instead of naming a rule.
        if refusal.code == FindingCode::OidcGatewayBlockInvalid
            && matches!(
                refusal.details,
                Some(Details::Reason {
                    reason: Reason::ProviderBlockInvalid
                })
            )
        {
            draft = draft.variant("provider_object");
        }
        context.report(draft);
    }

    let mut trust = Vec::new();
    if let Some(path) = &inspection.ca_bundle_file {
        let (read, certificate_count) = match inspection.ca_bundle {
            CaBundle::Accepted { certificates, .. } => (true, Some(certificates)),
            CaBundle::Refused => (true, None),
            _ => (false, None),
        };
        if read {
            context.opened(ContentRole::OidcCaBundle, path);
        }
        let (_, display) = context.place(path, "CA bundle");
        trust.push(TrustEntry {
            role: ContentRole::OidcCaBundle,
            display,
            read,
            certificate_count,
        });
        if matches!(
            inspection.ca_bundle,
            CaBundle::Accepted {
                with_client: true,
                ..
            }
        ) {
            for code in [
                FindingCode::TrustOidcCaBundleAcceptedByGrammar,
                FindingCode::TrustCertificateValidityPeriodNotAvailable,
            ] {
                context.report(
                    Draft::new(code, scope(code)).source(source.label, Some("ca_bundle_file")),
                );
            }
        }
    }

    let decoded = inspection.version.is_some();
    let file = Scope::File {
        file: ContentRole::OidcFile,
    };
    // Said as a finding, valid file or not: the result line is what gets
    // shared, and a fact among the parameters does not reach it.
    if inspection.gateway.as_ref().is_some_and(|gateway| {
        matches!(
            gateway.provider.transport,
            OidcTransportModeV1::IsolatedDemoHttp { .. }
        )
    }) {
        context.report(
            Draft::new(FindingCode::IdentitySsoDemonstrationTransport, file.clone())
                .source(source.label, Some("gateway.provider.transport")),
        );
    }
    if decoded && inspection.refusals.is_empty() && inspection.extra_refusals.is_empty() {
        for code in [
            FindingCode::IdentitySsoFileValidOffline,
            FindingCode::OidcLoaderAcceptanceNotChecked,
            FindingCode::IdentitySsoGatewayEnablementNotPersisted,
        ] {
            context.report(Draft::new(code, file.clone()).source(source.label, None));
        }
    }
    if source.describe_entries
        && let Some(path) = &inspection.audit_path
    {
        references::store(
            context,
            path,
            RawOwner::installation(),
            DataRole::AuthenticationAudit,
            source.input,
            // The OIDC file of an installation directory whose entries are
            // described is the one of a published preparation.
            source.input == InputName::InstallationDir,
        );
        context.report(
            Draft::new(
                FindingCode::DataOfflineInspectionNotAvailable,
                Scope::Input {
                    input: source.input,
                },
            )
            .variant("prepared"),
        );
    }
    OidcFacts {
        version: inspection.version,
        plan: inspection.plan_tenants.clone(),
        sso: sso(context, &inspection),
        trust,
        paths: inspection
            .audit_path
            .iter()
            .chain(&inspection.ca_bundle_file)
            .cloned()
            .collect(),
        label: source.label,
        input: source.input,
        decoded: decoded.then_some(inspection),
    }
}

/// Whether the identity provider answers. Without a request this is said as
/// not checked, and nothing is contacted. With a request, the discovery of
/// the product client runs against a file that passes every offline rule,
/// and against no other. Returns true when the discovery ran, whatever its
/// result: that is the only network operation of the tool.
pub(crate) async fn reachability(
    context: &mut Context<'_>,
    facts: Option<&OidcFacts>,
    requested: bool,
) -> bool {
    let code = FindingCode::IdentityIdpReachabilityNotChecked;
    let Some(facts) = facts else {
        if requested {
            context.report(Draft::new(code, Scope::Host).variant("no_file"));
        }
        return false;
    };
    let file = Scope::File {
        file: ContentRole::OidcFile,
    };
    if !requested {
        if facts.decoded.is_some() {
            context.report(Draft::new(code, file));
        }
        return false;
    }
    let Some(client) = facts.decoded.as_ref().and_then(OidcInspection::client) else {
        // A file that was not read is named by its input, not as a file.
        let scope = match facts.decoded {
            Some(_) => file,
            None => Scope::Input { input: facts.input },
        };
        context.report(Draft::new(code, scope).variant("not_run"));
        return false;
    };
    let (code, variant) = match client.discover().await {
        Ok(()) => (FindingCode::ActiveOidcDiscoveryOk, None),
        Err(OidcError::TransportUnavailable) => {
            (FindingCode::ActiveOidcDiscoveryTransportUnavailable, None)
        }
        Err(OidcError::ProviderRejected) => {
            (FindingCode::ActiveOidcDiscoveryProviderRejected, None)
        }
        Err(OidcError::InvalidMetadata) => (FindingCode::ActiveOidcDiscoveryMetadataMismatch, None),
        Err(OidcError::InvalidKeys) => (FindingCode::ActiveOidcDiscoveryKeysInvalid, None),
        // The discovery of a client built from a valid file gives none of
        // these; if it ever does, nothing is concluded about the provider.
        Err(
            OidcError::NotConfigured
            | OidcError::InvalidConfiguration
            | OidcError::InvalidToken
            | OidcError::InvalidLogin
            | OidcError::Capacity,
        ) => (
            FindingCode::ActiveOidcDiscoveryTransportUnavailable,
            Some("not_completed"),
        ),
    };
    let draft = Draft::new(code, file).source(facts.label, Some("gateway.provider"));
    context.report(match variant {
        Some(variant) => draft.variant(variant),
        None => draft,
    });
    true
}
