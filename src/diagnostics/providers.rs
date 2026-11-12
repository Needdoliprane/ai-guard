//! AI client profiles, as a provider profile file declares them.
//!
//! This axis is independent of single sign-on: nothing here decodes an OIDC
//! file or a registry. The file is loaded by the product loader
//! (`provider::load_config`), which opens the profile file alone: credential
//! paths are normalised lexically and never opened. A refusal of that loader
//! is reported by a closed reason; its text is compared with the product's own
//! literals and is never printed.
//!
//! The reference of each profile is described from `lstat` metadata and judged
//! by the rule of its reader, `agentsandbox run`, which is a private-file
//! reader.
//!
//! Which profile a managed launch selects is asked from the product itself:
//! `provider::choose_profile` runs the choice of `provider::prepare` and stops
//! before the credential of the chosen profile is opened. Nothing of the rule
//! is restated here. The selection is the one `run` builds from the same four
//! options; an option that is not given is left to the automatic choice. A
//! missing or unsafe reference is certain to fail a managed launch only for
//! the profile that selection resolves; for any other profile it is a warning
//! that names the class the launch would refuse with.

use super::{
    Context, Draft,
    catalogue::FindingCode,
    fs_facts::{self, Lookup, PrivateVerdict},
    output::{IdKind, IdTable, MAX_DYNAMIC_BYTES},
    references::{self, Declared, RawOwner},
    report::{
        Consumer, ContentRole, CredentialSourceKind, Details, EntryKind, InputName, Location,
        Presence, PrivateRule, ProviderFile, ProviderFileState, ProviderProfile,
        ProviderRefusalClass, Providers, Reason, ReferenceRole, RequestedSelection,
        ResolvedSelection, Scope, Selection,
    },
};
use crate::provider::{
    AuthMode, AuthPreference, CredentialSourceV1, ProviderConfigV1, ProviderProfileV1,
    ProviderSelection,
};
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

/// The name `run` and `providers` give to the profile file of a config
/// directory (`src/runner.rs`, `src/operations.rs`).
pub(crate) const PROFILE_FILE_NAME: &str = "providers.json";

const PROFILES_ID: &str = "profiles[].id";
const CREDENTIAL_PATH: &str = "profiles[].credential.path";

/// The one refusal `provider::load_config` gives for a profile file that the
/// private-file reader does not return: absent, unsafe or too large.
const FILE_REFUSAL: &str =
    "credential_unsafe: provider configuration is unavailable or not private";
/// The class the product gives to an ambiguous configuration.
const AMBIGUOUS_CLASS: &str = "ambiguous_configuration:";

/// Refusals of `provider::load_config` once the file is read, by their exact
/// text (`src/provider/selection.rs`; `src/control_fs.rs` for an absolute
/// reference). Only whole literals are compared. A text that is not listed is
/// `unclassified`: a changed literal degrades the reason, never the verdict.
const REFUSALS: &[(&str, Reason)] = &[
    (
        "ambiguous_configuration: duplicate or invalid provider profile",
        Reason::DuplicateOrInvalidProfile,
    ),
    (
        "ambiguous_configuration: unknown or duplicate provider priority",
        Reason::UnknownOrDuplicatePriority,
    ),
    (
        "ambiguous_configuration: default provider profile does not exist",
        Reason::DefaultProfileMissing,
    ),
    ("invalid provider configuration JSON", Reason::JsonInvalid),
    (
        "invalid provider configuration version or size",
        Reason::VersionOrSize,
    ),
    (
        "invalid provider profile identifier",
        Reason::ProfileIdentifier,
    ),
    ("invalid provider model identifier", Reason::ModelIdentifier),
    (
        "invalid credential source for provider authentication mode",
        Reason::SourceForMode,
    ),
    (
        "invalid credential variable for provider",
        Reason::VariableForClient,
    ),
    (
        "credential source requires an absolute path",
        Reason::PathNotAbsolute,
    ),
    (
        "credential source must not contain traversal or special path components",
        Reason::PathTraversal,
    ),
    (
        "control path must not contain parent traversal",
        Reason::PathTraversal,
    ),
];

/// The code and the closed reason of a refusal of the loader.
fn classify(refusal: &str) -> (FindingCode, Reason) {
    let code = if refusal.starts_with(AMBIGUOUS_CLASS) {
        FindingCode::ProviderConfigurationAmbiguous
    } else {
        FindingCode::ProviderConfigurationInvalid
    };
    let reason = REFUSALS
        .iter()
        .find(|(literal, _)| *literal == refusal)
        .map_or(Reason::Unclassified, |(_, reason)| *reason);
    (code, reason)
}

/// The class the product gives to a selection that no profile answers.
const NO_PROFILE_CLASS: &str = "credential_missing:";

/// Refusals of `provider::choose_profile`, by their exact text
/// (`src/provider/selection.rs`: `choose`, `validate_id`, `validate_model`).
/// Only whole literals are compared; a text that is not listed keeps the code
/// of its class and the reason `unclassified`.
const SELECTION_REFUSALS: &[(&str, Reason)] = &[
    (
        "credential_missing: requested provider profile is not configured",
        Reason::RequestedProfileNotConfigured,
    ),
    (
        "ambiguous_configuration: explicit client or authentication conflicts with the selected profile",
        Reason::ExplicitConflict,
    ),
    (
        "credential_missing: no configured profile matches the requested client and authentication mode",
        Reason::NoProfileMatches,
    ),
    (
        "ambiguous_configuration: several provider profiles match without an explicit priority",
        Reason::SeveralMatchWithoutPriority,
    ),
    (
        "invalid provider profile identifier",
        Reason::ProfileIdentifier,
    ),
    ("invalid provider model identifier", Reason::ModelIdentifier),
];

/// The code and the closed reason of a refusal of the choice.
fn classify_selection(refusal: &str) -> (FindingCode, Reason) {
    let code = if refusal.starts_with(AMBIGUOUS_CLASS) {
        FindingCode::ProviderSelectionAmbiguous
    } else if refusal.starts_with(NO_PROFILE_CLASS) {
        FindingCode::ProviderSelectionNoProfile
    } else {
        FindingCode::ProviderSelectionInvalid
    };
    let reason = SELECTION_REFUSALS
        .iter()
        .find(|(literal, _)| *literal == refusal)
        .map_or(Reason::Unclassified, |(_, reason)| *reason);
    (code, reason)
}

/// The fields of the profile file a reason is about.
fn fields(reason: Reason) -> &'static [&'static str] {
    match reason {
        Reason::DuplicateOrInvalidProfile => &[PROFILES_ID, "profiles[].revision"],
        Reason::UnknownOrDuplicatePriority => &["auto_priority"],
        Reason::DefaultProfileMissing => &["default_profile"],
        Reason::VersionOrSize => &["version", "profiles", "auto_priority"],
        Reason::ProfileIdentifier => &[PROFILES_ID],
        Reason::ModelIdentifier => &["profiles[].model"],
        Reason::SourceForMode => &[
            "profiles[].credential.kind",
            "profiles[].client",
            "profiles[].auth_mode",
        ],
        Reason::VariableForClient => &["profiles[].credential.key", "profiles[].client"],
        Reason::PathNotAbsolute | Reason::PathTraversal => &[CREDENTIAL_PATH],
        _ => &[],
    }
}

/// Where the profile file is looked for.
pub(crate) struct ProfileFile {
    pub path: PathBuf,
    /// The input that locates the file: the config directory whose
    /// `providers.json` is read, or the file given alone.
    pub input: InputName,
    pub display: String,
    /// The display of the config directory, when it is a historical one:
    /// `providers init` applies to no other directory.
    pub init_target: Option<String>,
}

impl ProfileFile {
    /// True for a file given by option; false for the file at its fixed name
    /// in a config directory.
    pub(crate) fn given_alone(&self) -> bool {
        self.input == InputName::ProvidersFile
    }

    /// How the file is named as the origin of a finding.
    fn label(&self) -> &'static str {
        if self.given_alone() {
            "provider profile file"
        } else {
            PROFILE_FILE_NAME
        }
    }
}

/// The selection a managed launch is asked for, from the options of this
/// tool: what `run` builds from its own options of the same names.
pub(crate) struct Request {
    pub selection: ProviderSelection,
    pub requested: RequestedSelection,
    /// True when at least one of the four options is on the command line.
    pub given: bool,
}

/// The section of the report, and the displays of the profile identifiers
/// for the entries and the findings that name a profile.
pub(crate) struct Outcome {
    pub section: Providers,
    pub ids: IdTable,
}

fn unloaded(context: &Context, file: &ProfileFile, state: ProviderFileState) -> Outcome {
    Outcome {
        section: Providers {
            file: ProviderFile {
                state,
                display: file.display.clone(),
            },
            default_profile: None,
            auto_priority: Vec::new(),
            profiles: Vec::new(),
            selection: None,
        },
        ids: context.safe.ids(IdKind::ProviderProfile, []),
    }
}

/// Inspect one profile file, the reference of every profile it declares, and
/// the profile a managed launch selects for `request`.
pub(crate) fn inspect(context: &mut Context, file: &ProfileFile, request: &Request) -> Outcome {
    let input = Scope::Input { input: file.input };
    // A path this installation uses for a key, a setup record or a store is
    // never opened, whatever option names it.
    if context.never_opened.contains(&file.path) {
        context.report(
            Draft::new(FindingCode::ProviderFileUnavailableOrNotPrivate, input).variant("reserved"),
        );
        return unloaded(context, file, ProviderFileState::Refused);
    }
    // The loader gives one refusal for an absent and for an unsafe file;
    // `lstat` tells them apart.
    let facts = match fs_facts::lookup(&file.path) {
        Lookup::Absent => {
            let draft = if file.given_alone() {
                Draft::new(FindingCode::ProviderFileUnavailableOrNotPrivate, input)
                    .variant("absent")
            } else {
                // `providers init` is proposed for a historical config
                // directory only: it initialises the directory first.
                match &file.init_target {
                    Some(target) => {
                        Draft::new(FindingCode::ProviderFileAbsent, input).target(target)
                    }
                    None => {
                        Draft::new(FindingCode::ProviderFileAbsent, input).variant("not_historical")
                    }
                }
            };
            context.report(draft);
            return unloaded(context, file, ProviderFileState::Absent);
        }
        Lookup::Unavailable => None,
        Lookup::Found(facts) => Some(facts),
    };
    let config = match crate::provider::load_config(&file.path) {
        Ok(config) => config,
        Err(error) => {
            let refusal = error.to_string();
            if refusal == FILE_REFUSAL {
                // A file of another account may be read by a launch that runs
                // as that account: that is not judged here.
                let code = match facts {
                    Some(facts) if !facts.owner_is_inspector => {
                        FindingCode::ReferenceInspectorIdentityDiffers
                    }
                    _ => FindingCode::ProviderFileUnavailableOrNotPrivate,
                };
                context.report(Draft::new(code, input));
                return unloaded(context, file, ProviderFileState::Refused);
            }
            // The path is already normalised: every other refusal comes
            // after the bounded read of the file.
            context.opened(ContentRole::ProviderProfiles, &file.path);
            let (code, reason) = classify(&refusal);
            let mut draft = Draft::new(
                code,
                Scope::File {
                    file: ContentRole::ProviderProfiles,
                },
            )
            .details(Details::Reason { reason });
            let fields = fields(reason);
            if fields.is_empty() {
                draft = draft.source(file.label(), None);
            }
            for field in fields.iter().copied() {
                draft = draft.source(file.label(), Some(field));
            }
            context.report(draft);
            return unloaded(context, file, ProviderFileState::Invalid);
        }
    };
    context.opened(ContentRole::ProviderProfiles, &file.path);
    context.report(
        Draft::new(
            FindingCode::ProviderConfigurationValid,
            Scope::File {
                file: ContentRole::ProviderProfiles,
            },
        )
        .source(file.label(), None),
    );
    loaded(context, file, &config, request)
}

fn source_kind(profile: &ProviderProfileV1) -> (&Path, CredentialSourceKind, Option<&str>) {
    match &profile.credential {
        CredentialSourceV1::ApiEnvFile { path, key } => {
            (path, CredentialSourceKind::ApiEnvFile, key.as_deref())
        }
        CredentialSourceV1::CodexAuthFile { path } => {
            (path, CredentialSourceKind::CodexAuthFile, None)
        }
        CredentialSourceV1::ClaudeOAuthEnvFile { path } => {
            (path, CredentialSourceKind::ClaudeOauthEnvFile, None)
        }
    }
}

/// The word of a source kind, as the profile file and the report write it.
fn source_word(kind: CredentialSourceKind) -> &'static str {
    match kind {
        CredentialSourceKind::ApiEnvFile => "api_env_file",
        CredentialSourceKind::CodexAuthFile => "codex_auth_file",
        CredentialSourceKind::ClaudeOauthEnvFile => "claude_oauth_env_file",
    }
}

fn loaded(
    context: &mut Context,
    file: &ProfileFile,
    config: &ProviderConfigV1,
    request: &Request,
) -> Outcome {
    // Every identifier of the file gets its display at once, so that the
    // number of a placeholder is the same wherever the profile appears.
    let ids = context.safe.ids(
        IdKind::ProviderProfile,
        config.profiles.iter().map(|profile| profile.id.as_str()),
    );
    let mut sorted: Vec<&ProviderProfileV1> = config.profiles.iter().collect();
    sorted.sort_by(|left, right| left.id.cmp(&right.id));

    // The choice of the product, for the selection `run` would build. It
    // validates and compares declared fields only: no credential is opened.
    let choice = crate::provider::choose_profile(config, &request.selection);
    let chosen = choice.as_ref().ok().map(|(profile, _)| profile.id.as_str());
    let mut chosen_fault = None;

    let mut profiles = Vec::new();
    for (index, profile) in sorted.into_iter().enumerate() {
        let (_, kind, variable) = source_kind(profile);
        profiles.push(ProviderProfile {
            profile: ids.display(&profile.id),
            revision: profile.revision,
            client: profile.client,
            auth_mode: profile.auth_mode,
            model: profile
                .model
                .as_deref()
                .map(|model| context.safe.dynamic(model, MAX_DYNAMIC_BYTES)),
            source_kind: kind,
            // One of the closed variable names the loader accepts.
            variable_name: variable.map(|name| context.safe.dynamic(name, MAX_DYNAMIC_BYTES)),
        });
        let selected = chosen == Some(profile.id.as_str());
        let fault = reference(context, file, profile, index + 1, selected);
        if selected {
            chosen_fault = fault;
        }
    }

    let api_declared = config
        .profiles
        .iter()
        .any(|profile| profile.auth_mode == AuthMode::Api);
    let subscription_declared = config
        .profiles
        .iter()
        .any(|profile| profile.auth_mode == AuthMode::Subscription);
    // `Some` when a subscription is refused, with the situation that the
    // finding names: none for a resolved profile whose credential is refused.
    let (resolved, refused_subscription) = match choice {
        Ok((profile, chosen_by)) => {
            context.report(
                Draft::new(
                    FindingCode::ProviderSelectionResolved,
                    Scope::ProviderProfiles,
                )
                .profile(&profile.id),
            );
            if profile.auth_mode == AuthMode::Api {
                context.report(
                    Draft::new(
                        FindingCode::ProviderApiBillingNotice,
                        Scope::ProviderProfiles,
                    )
                    .profile(&profile.id),
                );
            }
            // `prepare` gives the client the requested model, else the one
            // of the profile (`src/provider/selection.rs`).
            let model = request
                .selection
                .model
                .as_deref()
                .or(profile.model.as_deref())
                .map(|model| context.safe.dynamic(model, MAX_DYNAMIC_BYTES));
            (
                Some(ResolvedSelection {
                    profile: ids.display(&profile.id),
                    revision: profile.revision,
                    client: profile.client,
                    auth_mode: profile.auth_mode,
                    model,
                    chosen_by,
                }),
                (profile.auth_mode == AuthMode::Subscription && chosen_fault.is_some())
                    .then_some(None),
            )
        }
        Err(error) => {
            let (code, reason) = classify_selection(&error.to_string());
            context.report(
                Draft::new(code, Scope::ProviderProfiles).details(Details::Reason { reason }),
            );
            (
                None,
                (request.selection.auth_mode == AuthPreference::Subscription).then_some(Some(
                    if subscription_declared {
                        "request_refused"
                    } else {
                        "no_subscription_profile"
                    },
                )),
            )
        }
    };
    // Said only where it can be mistaken: the file declares API profiles
    // that a launch could have used, and the product never does.
    if let Some(situation) = refused_subscription
        && api_declared
    {
        let draft = Draft::new(
            FindingCode::ProviderSubscriptionNeverFallsBackToApi,
            Scope::ProviderProfiles,
        );
        context.report(match situation {
            Some(situation) => draft.variant(situation),
            None => draft,
        });
    }

    // What the profile file cannot show, whatever it declares. That the
    // content of the credentials is not checked is said at the end of the
    // run, once it is known whether one of them was read as another input.
    context.report(Draft::new(
        FindingCode::ProviderLaunchRequiresObserveAndBridge,
        Scope::ProviderProfiles,
    ));
    context.report(Draft::new(
        FindingCode::ProviderManualLaunchPathNotChecked,
        Scope::Host,
    ));

    Outcome {
        section: Providers {
            file: ProviderFile {
                state: ProviderFileState::Loaded,
                display: file.display.clone(),
            },
            default_profile: config.default_profile.as_deref().map(|id| ids.display(id)),
            // Declared order: the selection rule reads it in that order.
            auto_priority: config
                .auto_priority
                .iter()
                .map(|id| ids.display(id))
                .collect(),
            profiles,
            selection: Some(Selection {
                requested: request.requested,
                resolved,
            }),
        },
        ids,
    }
}

/// What the reader of a credential does with an entry it cannot use.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// No entry at the path: `login_required` or `credential_missing`.
    Absent,
    /// An entry the private-file reader refuses: `credential_unsafe`.
    Unsafe,
    /// A conforming entry below directories that the reader refuses:
    /// `credential_unsafe` as well.
    UnsafePath,
    /// A path through an entry that is not a directory. No file can exist
    /// there, and the reader refuses it as `credential_unsafe` as well.
    BelowANonDirectory,
}

/// Describe the reference of one profile and report what its reader refuses
/// (`src/provider/credentials.rs`, `prepare`). The file is never opened.
/// `ordinal` is the rank of the profile among the sorted identifiers;
/// `selected` is true for the profile the requested selection resolves.
/// Returns what the reader would refuse, if anything.
fn reference(
    context: &mut Context,
    file: &ProfileFile,
    profile: &ProviderProfileV1,
    ordinal: usize,
    selected: bool,
) -> Option<Fault> {
    let (path, kind, _) = source_kind(profile);
    // The location of a credential is chosen by the operator: it is shown by
    // its source kind and the rank of its profile, never by a file name,
    // unless absolute paths are asked for. The rank depends on this file
    // alone, so that no other axis changes the display.
    let inside = file
        .path
        .parent()
        .is_some_and(|directory| path.starts_with(directory));
    let opaque = format!(
        "provider reference #{ordinal} ({}, {} the directory of the profile file)",
        source_word(kind),
        if inside { "inside" } else { "outside" }
    );
    let display = context.safe.path(path, &[], context.show_paths, &opaque);
    let declared = Declared {
        path,
        owner: RawOwner::profile(&profile.id),
        role: ReferenceRole::Provider,
        input: file.input,
        init_target: None,
    };
    let (mut record, facts) = references::observe_shown(
        context,
        &declared,
        Consumer::PrivateFileReader,
        if inside {
            Location::InsideRoot
        } else {
            Location::OutsideRoot
        },
        display,
    );

    // What is not a verdict on the launch is scoped to the input that
    // declares the reference.
    let unknown = |code| Draft::new(code, Scope::Input { input: file.input });
    let fault = match (record.presence, record.entry_kind, facts) {
        // The reader calls a missing entry absent; a path it cannot examine,
        // such as one below a regular file, is unsafe for it.
        (Presence::Absent, _, _) => Some(match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == ErrorKind::NotFound => Fault::Absent,
            Err(error) if error.kind() == ErrorKind::NotADirectory => Fault::BelowANonDirectory,
            _ => Fault::Unsafe,
        }),
        (Presence::Unknown, _, _) => {
            record.attention = true;
            context.report(unknown(FindingCode::ReferenceMetadataUnavailable).profile(&profile.id));
            None
        }
        (Presence::Present, Some(EntryKind::RegularFile), Some(facts)) => {
            match fs_facts::private_file_rule(path, &facts) {
                PrivateVerdict::Satisfied => {
                    record.private_rule = PrivateRule::Satisfied;
                    None
                }
                PrivateVerdict::Violated => {
                    record.private_rule = PrivateRule::Violated;
                    Some(Fault::Unsafe)
                }
                PrivateVerdict::PathRefused => {
                    record.private_rule = PrivateRule::Violated;
                    Some(Fault::UnsafePath)
                }
                PrivateVerdict::InspectorDiffers => {
                    record.attention = true;
                    context.report(
                        unknown(FindingCode::ReferenceInspectorIdentityDiffers)
                            .profile(&profile.id),
                    );
                    None
                }
                // The entry check of the product did not run: that is said,
                // never left as a bare word in the entry.
                PrivateVerdict::NotEvaluated => {
                    context.report(
                        unknown(FindingCode::ReferencePrivateRuleNotEvaluated).profile(&profile.id),
                    );
                    None
                }
            }
        }
        // A symbolic link, a directory or a special file.
        (Presence::Present, _, _) => Some(Fault::Unsafe),
    };
    if let Some(fault) = fault {
        record.attention = true;
        let class = if profile.auth_mode == AuthMode::Subscription {
            ProviderRefusalClass::LoginRequired
        } else {
            ProviderRefusalClass::CredentialMissing
        };
        let (code, details) = match (fault, selected, class) {
            (Fault::Absent, true, ProviderRefusalClass::LoginRequired) => {
                (FindingCode::ProviderLoginRequired, None)
            }
            (Fault::Absent, true, ProviderRefusalClass::CredentialMissing) => {
                (FindingCode::ProviderCredentialMissing, None)
            }
            (Fault::Absent, false, class) => (
                FindingCode::ProviderUnselectedProfileReferenceAbsent,
                Some(Details::WouldRefuseWith { class }),
            ),
            (Fault::Unsafe | Fault::UnsafePath | Fault::BelowANonDirectory, true, _) => {
                (FindingCode::ProviderCredentialUnsafe, None)
            }
            (Fault::Unsafe | Fault::UnsafePath | Fault::BelowANonDirectory, false, _) => {
                (FindingCode::ProviderUnselectedProfileReferenceUnsafe, None)
            }
        };
        let mut draft = Draft::new(code, Scope::ProviderProfiles)
            .source(file.label(), Some(CREDENTIAL_PATH))
            .profile(&profile.id);
        match fault {
            Fault::BelowANonDirectory => draft = draft.variant("below_a_non_directory"),
            Fault::UnsafePath => draft = draft.variant("path"),
            Fault::Absent | Fault::Unsafe => {}
        }
        if let Some(details) = details {
            draft = draft.details(details);
        }
        context.report(draft);
    }
    context.references.push(record);
    fault
}
