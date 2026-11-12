//! Entries of the report for referenced files: key files, setup records and
//! stores. Each is described by metadata only and judged by the rule of the
//! command that reads it (`fs_facts`). Conforming facts never produce an `ok`:
//! they prove a necessary condition, and the content is checked by the
//! consuming command alone.

use super::{
    Context, Draft,
    catalogue::FindingCode,
    fs_facts::{self, Facts, Lookup, PrivateVerdict, StoreState},
    report::{
        Consumer, DataRole, Details, EntryKind, InputName, Location, OwnerKind, Presence,
        PrivateRule, ReferenceRole, Scope, Sidecar,
    },
};
use std::path::{Path, PathBuf};

/// The owner of an entry, before identifiers have their display.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RawOwner {
    pub kind: OwnerKind,
    pub id: Option<String>,
}

impl RawOwner {
    pub(crate) fn installation() -> Self {
        Self {
            kind: OwnerKind::Installation,
            id: None,
        }
    }

    pub(crate) fn tenant(id: &str) -> Self {
        Self {
            kind: OwnerKind::Tenant,
            id: Some(id.to_owned()),
        }
    }

    pub(crate) fn profile(id: &str) -> Self {
        Self {
            kind: OwnerKind::ProviderProfile,
            id: Some(id.to_owned()),
        }
    }
}

/// One referenced credential file, as evaluated.
pub(crate) struct ReferenceRecord {
    /// The referenced path, to tell at the end of the run whether this file
    /// was opened before it was named. Never printed as such.
    pub path: PathBuf,
    pub owner: RawOwner,
    pub role: ReferenceRole,
    pub consumer: Consumer,
    pub location: Location,
    pub display: String,
    pub presence: Presence,
    pub entry_kind: Option<EntryKind>,
    pub mode: Option<String>,
    pub owner_is_inspector: Option<bool>,
    pub single_link: Option<bool>,
    pub private_rule: PrivateRule,
    /// Tied to an error, an unknown or a warning: printed first.
    pub attention: bool,
}

/// One referenced store, as evaluated.
pub(crate) struct DataRecord {
    /// The store path, for the same comparison.
    pub path: PathBuf,
    pub owner: RawOwner,
    pub role: DataRole,
    pub location: Location,
    pub display: String,
    pub presence: Presence,
    pub sidecars: Vec<Sidecar>,
    pub attention: bool,
}

/// A reference as a file declares it.
pub(crate) struct Declared<'a> {
    pub path: &'a Path,
    pub owner: RawOwner,
    pub role: ReferenceRole,
    /// The input whose file declares the reference: the scope of its findings.
    pub input: InputName,
    /// The display of the historical config directory, for a key that
    /// `agentsandbox init` recreates when it is missing.
    pub init_target: Option<&'a str>,
}

fn role_noun(role: ReferenceRole) -> &'static str {
    match role {
        ReferenceRole::Runner => "runner key",
        ReferenceRole::Viewer => "viewer key",
        ReferenceRole::DecisionEngine => "decision engine key",
        ReferenceRole::Projection => "projection",
        ReferenceRole::Provider => "provider",
        ReferenceRole::SetupMaterial => "setup record",
    }
}

/// The finding of one entry: scoped to the declaring input, with the tenant
/// that owns the entry as its subject.
fn draft(code: FindingCode, input: InputName, owner: &RawOwner) -> Draft {
    let draft = Draft::new(code, Scope::Input { input });
    match (&owner.kind, &owner.id) {
        (OwnerKind::Tenant, Some(id)) => draft.tenant(id),
        _ => draft,
    }
}

/// The record of a reference with the facts of `lstat`, and those facts when
/// the entry is a regular file. Absence, an unreadable entry and an entry that
/// no reader accepts are reported here: every reader opens these files without
/// following a link and requires a regular file.
fn observe(
    context: &mut Context,
    declared: &Declared,
    consumer: Consumer,
) -> (ReferenceRecord, Option<Facts>) {
    let (location, display) = context.place(declared.path, role_noun(declared.role));
    observe_shown(context, declared, consumer, location, display)
}

/// The same record for a reference whose location and display its axis
/// chose. The file is registered as never opened, like every reference.
pub(crate) fn observe_shown(
    context: &mut Context,
    declared: &Declared,
    consumer: Consumer,
    location: Location,
    display: String,
) -> (ReferenceRecord, Option<Facts>) {
    context.never_opened.add(declared.path);
    let mut record = ReferenceRecord {
        path: declared.path.to_owned(),
        owner: declared.owner.clone(),
        role: declared.role,
        consumer,
        location,
        display,
        presence: Presence::Unknown,
        entry_kind: None,
        mode: None,
        owner_is_inspector: None,
        single_link: None,
        private_rule: PrivateRule::NotEvaluated,
        attention: false,
    };
    let facts = match fs_facts::lookup(declared.path) {
        Lookup::Absent => {
            record.presence = Presence::Absent;
            return (record, None);
        }
        Lookup::Unavailable => return (record, None),
        Lookup::Found(facts) => facts,
    };
    record.presence = Presence::Present;
    record.entry_kind = Some(facts.kind);
    record.mode = Some(facts.mode_text());
    record.owner_is_inspector = Some(facts.owner_is_inspector);
    record.single_link = Some(facts.links == 1);
    let regular = (facts.kind == EntryKind::RegularFile).then_some(facts);
    (record, regular)
}

/// Report what no reader accepts: an absent entry, unreadable metadata, or an
/// entry that is not a regular file. Returns true when one was reported.
fn report_unusable(
    context: &mut Context,
    declared: &Declared,
    record: &mut ReferenceRecord,
) -> bool {
    let code = match (record.presence, record.entry_kind) {
        (Presence::Absent, _) if record.location == Location::InsideRoot => {
            FindingCode::ReferenceAbsent
        }
        (Presence::Absent, _) => FindingCode::ReferenceNotFoundFromHere,
        (Presence::Unknown, _) => FindingCode::ReferenceMetadataUnavailable,
        (Presence::Present, Some(EntryKind::RegularFile)) => return false,
        (Presence::Present, _) => FindingCode::ReferenceNotARegularFile,
    };
    record.attention = true;
    let mut finding = draft(code, declared.input, &declared.owner);
    if code == FindingCode::ReferenceAbsent
        && let Some(target) = declared.init_target
    {
        finding = finding.variant("historical_key").target(target);
    }
    context.report(finding);
    true
}

/// A key file read by the gateway token reader: a plain open that follows no
/// final link, a regular file. No owner, mode or link count is checked, so a
/// mode that other accounts can use is a warning, never a refusal.
pub(crate) fn gateway_token(context: &mut Context, declared: Declared) {
    let (mut record, facts) = observe(context, &declared, Consumer::GatewayTokenReader);
    if !report_unusable(context, &declared, &mut record)
        && facts.is_some_and(|facts| facts.grants_group_or_other())
    {
        record.attention = true;
        context.report(draft(
            FindingCode::ReferenceTokenFileAccessibleToOtherAccounts,
            declared.input,
            &declared.owner,
        ));
    }
    context.references.push(record);
}

/// A key file read through the private-file rule. An optional file that is
/// absent is not listed.
pub(crate) fn private_file(context: &mut Context, declared: Declared, optional: bool) {
    let (mut record, facts) = observe(context, &declared, Consumer::PrivateFileReader);
    if optional && record.presence == Presence::Absent {
        return;
    }
    if !report_unusable(context, &declared, &mut record)
        && let Some(facts) = facts
    {
        let finding = |code| draft(code, declared.input, &declared.owner);
        match fs_facts::private_file_rule(declared.path, &facts) {
            PrivateVerdict::Satisfied => record.private_rule = PrivateRule::Satisfied,
            PrivateVerdict::Violated => {
                record.private_rule = PrivateRule::Violated;
                record.attention = true;
                context.report(finding(FindingCode::ReferencePrivateRuleViolated));
            }
            PrivateVerdict::PathRefused => {
                record.private_rule = PrivateRule::Violated;
                record.attention = true;
                context.report(finding(FindingCode::ReferencePrivateRuleViolated).variant("path"));
            }
            PrivateVerdict::InspectorDiffers => {
                record.attention = true;
                context.report(finding(FindingCode::ReferenceInspectorIdentityDiffers));
            }
            // The entry check of the product did not run: that is said,
            // never left as a bare word in the entry.
            PrivateVerdict::NotEvaluated => {
                context.report(finding(FindingCode::ReferencePrivateRuleNotEvaluated));
            }
        }
    }
    context.references.push(record);
}

/// An entry listed with its facts and no verdict: a read-only projection of a
/// key for the Compose stack, or a setup recovery record. An optional entry
/// that is absent is not listed. Returns whether the entry exists.
pub(crate) fn facts_only(
    context: &mut Context,
    declared: Declared,
    consumer: Consumer,
    optional: bool,
) -> bool {
    let (record, _) = observe(context, &declared, consumer);
    let present = record.presence == Presence::Present;
    if !(optional && record.presence == Presence::Absent) {
        context.references.push(record);
    }
    present
}

/// A store, under the rule of the stores. A tenant store that is absent is
/// created by the gateway; an absent authentication audit refuses the start.
/// `created_by_setup` is true for the audit of a published prepared
/// installation: `setup prepare` created it, so its absence is no state of a
/// first start.
pub(crate) fn store(
    context: &mut Context,
    path: &Path,
    owner: RawOwner,
    role: DataRole,
    input: InputName,
    created_by_setup: bool,
) {
    context.never_opened.add_store(path);
    let noun = match role {
        DataRole::TenantStore => "tenant store",
        DataRole::AuthenticationAudit => "authentication audit",
    };
    let (location, display) = context.place(path, noun);
    // Presence by `lstat` where the state alone does not say it.
    let seen = || match fs_facts::lookup(path) {
        Lookup::Found(_) => Presence::Present,
        Lookup::Absent => Presence::Absent,
        Lookup::Unavailable => Presence::Unknown,
    };
    let mut sidecars = Vec::new();
    let mut variant = None;
    let (presence, code) = match fs_facts::store_state(path) {
        StoreState::Present { sidecars: found } => {
            let code = (!found.is_empty()).then_some(FindingCode::DataSidecarsPresent);
            sidecars = found;
            (Presence::Present, code)
        }
        StoreState::Absent => (
            Presence::Absent,
            Some(match (location, role) {
                (Location::OutsideRoot, _) => FindingCode::ReferenceNotFoundFromHere,
                (Location::InsideRoot, DataRole::TenantStore) => FindingCode::DataStoreAbsent,
                (Location::InsideRoot, DataRole::AuthenticationAudit) if created_by_setup => {
                    FindingCode::DataAuthenticationAuditMissingFromPreparedInstallation
                }
                (Location::InsideRoot, DataRole::AuthenticationAudit) => {
                    FindingCode::DataAuthenticationAuditAbsent
                }
            }),
        ),
        StoreState::OrphanSidecars(found) => {
            sidecars = found;
            (Presence::Absent, Some(FindingCode::DataOrphanSidecars))
        }
        StoreState::PathRefused => {
            variant = Some("path");
            (
                Presence::Unknown,
                Some(FindingCode::DataStorePostureRefused),
            )
        }
        StoreState::NameRefused => {
            variant = Some("name");
            (seen(), Some(FindingCode::DataStorePostureRefused))
        }
        StoreState::ParentNotPrivate => (seen(), Some(FindingCode::DataParentDirectoryNotPrivate)),
        StoreState::PostureRefused => (seen(), Some(FindingCode::DataStorePostureRefused)),
        StoreState::InspectorDiffers => {
            (seen(), Some(FindingCode::ReferenceInspectorIdentityDiffers))
        }
        StoreState::Unavailable => (
            Presence::Unknown,
            Some(FindingCode::ReferenceMetadataUnavailable),
        ),
    };
    // Sidecars beside their store are a fact, not a deviation.
    let attention = code.is_some_and(|code| code != FindingCode::DataSidecarsPresent);
    if let Some(code) = code {
        let mut finding = draft(code, input, &owner);
        if let Some(variant) = variant {
            finding = finding.variant(variant);
        }
        if !sidecars.is_empty() {
            finding = finding.details(Details::Sidecars {
                sidecars: sidecars.clone(),
            });
        }
        context.report(finding);
    }
    context.data.push(DataRecord {
        path: path.to_owned(),
        owner,
        role,
        location,
        display,
        presence,
        sidecars,
        attention,
    });
}
