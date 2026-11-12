//! Trust material given on the command line: the CA bundle of the decision
//! engine, the CA bundle of the clients of an HTTPS gateway, and the
//! decision-engine transport.
//!
//! None of these is stored in an installation: they are gateway and client
//! options, so they are examined only when they are given here. A bundle is
//! read by the reader of its consumer, the projected-file reader
//! (`src/tls.rs`, `read_material`), and judged by the bundle grammar of the
//! product (`src/tls.rs`, `certificates`). The transport is judged by the
//! validator the gateway runs before it opens anything
//! (`src/jev/transport.rs`, `validate_transport_configuration`). Nothing is
//! contacted. The CA bundle of an OIDC file belongs to `identity`.

use super::{
    Context, Draft,
    catalogue::FindingCode,
    fs_facts::{self, Lookup},
    report::{ContentRole, DecisionTransport, DecisionTransportKind, InputName, Scope, TrustEntry},
};
use crate::jev::{JevTrust, validate_transport_configuration};
use std::{os::unix::fs::MetadataExt, path::Path};

/// The bound both consumers give to the reader of a bundle
/// (`JevTrust::from_ca_file`, `ClientTrust::from_ca_file`). It is also the
/// bound of the bundle grammar.
const MAX_BUNDLE_BYTES: usize = 65_536;

/// A bundle given by option: its normalised path and its display.
pub(crate) struct GivenBundle<'a> {
    pub path: &'a Path,
    pub display: &'a str,
}

/// The explicit inputs of this axis.
pub(crate) struct Given<'a> {
    pub jev_url: Option<&'a str>,
    pub jev_local_url: Option<&'a str>,
    pub jev_allow_local_http: bool,
    pub jev_ca_file: Option<GivenBundle<'a>>,
    pub gateway_ca_file: Option<GivenBundle<'a>>,
}

/// What the report shows of this axis.
pub(crate) struct Facts {
    pub trust: Vec<TrustEntry>,
    pub transport: Option<DecisionTransport>,
}

/// Why the projected-file reader did not return a file, from `lstat` alone,
/// with the situation that the texts name for an absent file.
fn reader_refusal(path: &Path, scope: Scope) -> Draft {
    if matches!(fs_facts::lookup(path), Lookup::Absent) {
        return Draft::new(FindingCode::TrustCaUnavailableOrNotPrivate, scope).variant("absent");
    }
    let code = match std::fs::symlink_metadata(path) {
        // The reader bounds the file before the grammar does.
        Ok(metadata) if metadata.is_file() && metadata.len() > MAX_BUNDLE_BYTES as u64 => {
            FindingCode::TrustCaBundleInvalid
        }
        // The reader accepts a file of root or of its own account. A
        // conforming file of another account may be read by a consumer that
        // runs as that account: that is not judged here.
        Ok(metadata)
            if metadata.is_file()
                && metadata.nlink() == 1
                && metadata.mode() & 0o7022 == 0
                && metadata.uid() != 0
                && metadata.uid() != fs_facts::effective_uid() =>
        {
            FindingCode::ReferenceInspectorIdentityDiffers
        }
        _ => FindingCode::TrustCaUnavailableOrNotPrivate,
    };
    Draft::new(code, scope)
}

/// Read and judge one bundle. Returns its entry, and its bytes when the
/// grammar accepted them.
fn bundle(
    context: &mut Context,
    given: &GivenBundle,
    role: ContentRole,
    accepted: FindingCode,
) -> (TrustEntry, Option<Vec<u8>>) {
    let scope = Scope::File { file: role };
    let mut entry = TrustEntry {
        role,
        display: given.display.to_owned(),
        read: false,
        certificate_count: None,
    };
    // A path this installation uses for a key, a setup record or a store is
    // never opened, whatever option names it.
    if context.never_opened.contains(given.path) {
        context.report(Draft::new(
            FindingCode::TrustCaReferenceIsACredentialReference,
            scope,
        ));
        return (entry, None);
    }
    let Ok(bytes) = crate::tls::read_material(given.path, MAX_BUNDLE_BYTES) else {
        context.report(reader_refusal(given.path, scope));
        return (entry, None);
    };
    entry.read = true;
    context.opened(role, given.path);
    let Ok(certificates) = crate::tls::certificates(&bytes) else {
        context.report(Draft::new(FindingCode::TrustCaBundleInvalid, scope));
        return (entry, None);
    };
    entry.certificate_count = Some(u64::try_from(certificates.len()).unwrap_or(u64::MAX));
    for code in [
        accepted,
        FindingCode::TrustCertificateValidityPeriodNotAvailable,
    ] {
        context.report(Draft::new(code, scope.clone()));
    }
    (entry, Some(bytes))
}

/// The bundle given for the decision engine, as the transport check needs it.
enum EngineBundle {
    NotGiven,
    /// Given, and withheld, unreadable or refused by the grammar.
    Unusable,
    Accepted(Vec<u8>),
}

/// Report the bundles and the transport that were given.
pub(crate) fn report(context: &mut Context, given: &Given) -> Facts {
    let mut trust = Vec::new();
    let engine_bundle = match &given.jev_ca_file {
        None => EngineBundle::NotGiven,
        Some(file) => {
            let (entry, bytes) = bundle(
                context,
                file,
                ContentRole::DecisionEngineCaBundle,
                FindingCode::TrustDecisionEngineCaAcceptedByGrammar,
            );
            trust.push(entry);
            bytes.map_or(EngineBundle::Unusable, EngineBundle::Accepted)
        }
    };
    if let Some(file) = &given.gateway_ca_file {
        let (entry, _) = bundle(
            context,
            file,
            ContentRole::GatewayCaBundle,
            FindingCode::TrustGatewayCaAcceptedByGrammar,
        );
        trust.push(entry);
    }

    // The command line refuses the two URL options together.
    let endpoint = given
        .jev_url
        .map(|url| (DecisionTransportKind::Remote, url, InputName::JevUrl))
        .or_else(|| {
            given
                .jev_local_url
                .map(|url| (DecisionTransportKind::Local, url, InputName::JevLocalUrl))
        });
    let Some((kind, url, input)) = endpoint else {
        // An input of the transport that no transport uses is said not
        // judged: listed among the inputs alone, it would read as validated.
        let unused = if given.jev_allow_local_http {
            Some(InputName::JevAllowLocalHttp)
        } else {
            given.jev_ca_file.as_ref().map(|_| InputName::JevCaFile)
        };
        if let Some(input) = unused {
            context.report(Draft::new(
                FindingCode::InferenceDecisionTransportInputsWithoutUrl,
                Scope::Input { input },
            ));
        }
        return Facts {
            trust,
            transport: None,
        };
    };
    let accepts = |trust: &JevTrust| {
        validate_transport_configuration(url, given.jev_allow_local_http, trust).is_ok()
    };
    let scope = Scope::Input { input };
    let valid = Draft::new(FindingCode::InferenceDecisionTransportValid, scope.clone());
    let invalid = Draft::new(FindingCode::TrustDecisionTransportInvalid, scope.clone());
    let verdict = match &engine_bundle {
        EngineBundle::NotGiven => Some(if accepts(&JevTrust::default()) {
            valid
        } else {
            invalid
        }),
        // The grammar accepted these bytes, so the trust is built from them
        // as the gateway builds it.
        EngineBundle::Accepted(bytes) => JevTrust::from_ca_pem(bytes)
            .ok()
            .map(|trust| if accepts(&trust) { valid } else { invalid }),
        // The gateway stops at the bundle, so the transport it would use
        // cannot be built. What can still be observed is a refusal of the URL
        // without any bundle.
        EngineBundle::Unusable => {
            (!accepts(&JevTrust::default())).then(|| invalid.variant("without_bundle"))
        }
    };
    // A transport that could not be judged is said not judged: its absence
    // from the report would read as nothing to say.
    context.report(
        verdict.unwrap_or_else(|| {
            Draft::new(FindingCode::InferenceDecisionTransportNotChecked, scope)
        }),
    );
    Facts {
        trust,
        transport: Some(DecisionTransport {
            kind,
            url: context.safe.url(url),
            allow_local_http: given.jev_allow_local_http,
            ca_bundle_given: given.jev_ca_file.is_some(),
        }),
    }
}
