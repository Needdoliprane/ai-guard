//! Which layout an inspected directory holds, and what the historical layout
//! references.
//!
//! Two layouts exist. The historical one is created by `agentsandbox init`
//! (`src/operations.rs`, `initialize_config_directory`) and is the only one
//! the legacy commands read. The prepared one is published by
//! `agentsandbox setup prepare` (`src/setup/storage.rs`,
//! `src/setup/installation.rs`) and is read by the gateway through its
//! registry and its OIDC file. A layout is recognised from the presence of
//! fixed entry names and from the name of the directory itself: no directory
//! is listed, no parent is searched, and no setup record is opened.

use super::{
    Context, Draft, Target,
    catalogue::FindingCode,
    fs_facts::{self, Lookup},
    gateway_files::RegistryInspection,
    references::{self, Declared, RawOwner},
    report::{
        Consumer, EntryKind, InputName, InputOrigin, Layout, LayoutKind, PreparationState,
        ReferenceRole, Scope,
    },
    tenants,
};
use crate::control_fs::Directory;
use std::path::{Path, PathBuf};

pub(crate) const REGISTRY_NAME: &str = "tenants.json";
pub(crate) const OIDC_FILE_NAME: &str = "oidc.json";
const SETUP_RECORDS: [&str; 3] = [
    "secret-material.json",
    "preparation.json",
    "completion.json",
];
/// The records of the authentication audit that `setup prepare` keeps beside
/// it (`src/setup/installation.rs`): the creation binding, which only a
/// version 2 preparation writes, and the origin of the audit. They hold
/// identifiers, a digest and a receipt, and setup reads them again when it
/// resumes. Like every private record of an installation they are described
/// by their metadata and never opened.
const AUTHENTICATION_CREATION_RECORD: &str = "authentication-creation.json";
const AUTHENTICATION_ORIGIN_RECORD: &str = "authentication-origin.json";
const AUTHENTICATION_AUDIT: &str = "authentication.sqlite";
/// Directories that hold nothing but key files or stores: the key sources
/// and their projections of the historical layout, and the tenant
/// directories of a prepared installation.
const KEY_DIRECTORIES: [&str; 3] = ["secrets", "mounted-secrets", "tenants"];

/// `setup prepare` works in a sibling of its destination named `.setup-`
/// followed by 24 hexadecimal digits (`src/setup/storage.rs`, `stage_name`).
/// Only the form of the name is recognised here; the name of the staging
/// directory of a given destination is never derived.
const STAGING_PREFIX: &str = ".setup-";
const STAGING_DIGITS: usize = 24;

fn has_staging_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(STAGING_PREFIX))
        .is_some_and(|digits| {
            digits.len() == STAGING_DIGITS
                && digits
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

/// Presence of the fixed names that identify a layout.
struct Markers {
    secrets: bool,
    mounted_secrets: bool,
    preparation: bool,
    completion: bool,
    registry: bool,
    oidc_file: bool,
}

impl Markers {
    fn observe(directory: &Path) -> Self {
        let present = |name: &str| fs_facts::exists(&directory.join(name));
        Self {
            secrets: present("secrets"),
            mounted_secrets: present("mounted-secrets"),
            preparation: present("preparation.json"),
            completion: present("completion.json"),
            registry: present(REGISTRY_NAME),
            oidc_file: present(OIDC_FILE_NAME),
        }
    }
}

/// The gateway files of an installation directory.
pub(crate) struct Installation {
    /// The registry, decoded once: its recorded paths also tell a published
    /// installation from a staging directory.
    pub registry: Option<RegistryInspection>,
    pub oidc_file: Option<PathBuf>,
    /// False for a setup staging directory: its files record the final paths,
    /// so the entries they name are not described.
    pub published: bool,
}

/// What a directory is, and what is inspected next in it.
#[derive(Default)]
pub(crate) struct Outcome {
    pub layout: Option<Layout>,
    /// The directory is described as a historical config directory.
    pub historical: bool,
    pub installation: Option<Installation>,
}

/// Every reference a decoded registry resolves.
pub(crate) fn recorded_paths(inspection: &RegistryInspection) -> impl Iterator<Item = &PathBuf> {
    inspection.tenants.iter().flat_map(|tenant| {
        [
            &tenant.data,
            &tenant.runner_token_file,
            &tenant.viewer_token_file,
        ]
        .into_iter()
        .flatten()
    })
}

/// Reserve every fixed name under which either layout keeps key files, setup
/// records or stores. The reservation depends neither on the option that gave
/// the directory nor on what the directory turns out to hold: a directory
/// given through the wrong option, a mixed one, a moved one or a refused one
/// still holds its keys, and a file option or a CA reference that points
/// there is never opened.
fn reserve(context: &mut Context, directory: &Path) {
    for name in KEY_DIRECTORIES {
        context.never_opened.reserve(&directory.join(name));
    }
    context
        .never_opened
        .add_store(&directory.join(AUTHENTICATION_AUDIT));
    for name in SETUP_RECORDS
        .into_iter()
        .chain([AUTHENTICATION_CREATION_RECORD, AUTHENTICATION_ORIGIN_RECORD])
    {
        context.never_opened.add(&directory.join(name));
    }
}

/// Classify one directory target and report its layout.
pub(crate) fn inspect(context: &mut Context, target: &Target, max_events: u64) -> Outcome {
    reserve(context, &target.path);
    let scope = Scope::Input { input: target.name };
    let draft = |code| Draft::new(code, scope.clone()).target(&target.display);
    let mut outcome = Outcome::default();

    let facts = match fs_facts::lookup(&target.path) {
        Lookup::Absent => {
            context.report(
                draft(FindingCode::LayoutTargetAbsent).variant(match target.origin {
                    InputOrigin::Default => "default",
                    InputOrigin::Cli => "given",
                }),
            );
            outcome.layout = Some(Layout {
                input: target.name,
                kind: LayoutKind::Absent,
                preparation: None,
                oidc_file_version: None,
            });
            return outcome;
        }
        Lookup::Unavailable => {
            context
                .report(draft(FindingCode::LayoutInspectorIdentityDiffers).variant("unavailable"));
            return outcome;
        }
        Lookup::Found(facts) => facts,
    };
    if facts.kind != EntryKind::Directory {
        context.report(draft(FindingCode::LayoutPostureRefused).variant("not_a_directory"));
        return outcome;
    }
    // The rule of the control commands for their own directory. Nothing below
    // a refused directory is described.
    if Directory::open_existing(&target.path).is_err() {
        context.report(if facts.owner_is_inspector {
            // `lstat` tells the mode of the directory from every other
            // condition of the rule.
            draft(FindingCode::LayoutPostureRefused).variant(if facts.mode == 0o700 {
                "above_or_file_system"
            } else {
                "mode"
            })
        } else {
            draft(FindingCode::LayoutInspectorIdentityDiffers)
        });
        return outcome;
    }

    let markers = Markers::observe(&target.path);
    let historical = markers.secrets || markers.mounted_secrets;
    let prepared = markers.preparation || markers.completion;
    let staging_name = has_staging_name(&target.path);
    let as_installation = target.name == InputName::InstallationDir;
    let registry_path = target.path.join(REGISTRY_NAME);

    // The registry at its fixed name is compared like a file given by option
    // before it is opened: a second name of a key file, a setup record or a
    // store of an inspected directory is reported and never read.
    let withheld =
        as_installation && markers.registry && context.never_opened.contains(&registry_path);
    let mut registry =
        (as_installation && markers.registry && !withheld && (prepared || staging_name))
            .then(|| tenants::inspect(context, &registry_path, max_events));
    // A destination may itself carry a staging name. A directory with that
    // name is unpublished when it holds no registry yet, or when its registry
    // records paths outside it: `setup prepare` records the final paths. The
    // registry is therefore read before the layout is known, and that read is
    // listed even when the directory turns out to be described by no arm.
    let records_inside = registry.as_ref().is_some_and(|inspection| {
        inspection.decoded && recorded_paths(inspection).all(|path| path.starts_with(&target.path))
    });
    let staging = staging_name && !records_inside;

    let published_state = match (markers.preparation, markers.completion) {
        (true, true) => PreparationState::Complete,
        (true, false) => PreparationState::CompletionMarkerAbsent,
        (false, _) => PreparationState::NoIntent,
    };
    let (kind, preparation) = if historical && (prepared || staging) {
        (LayoutKind::Mixed, Some(published_state))
    } else if staging {
        let state = match (markers.preparation, markers.completion) {
            (true, true) => PreparationState::Complete,
            (true, false) => PreparationState::Incomplete,
            (false, _) => PreparationState::NoIntent,
        };
        (LayoutKind::SetupStagingDirectory, Some(state))
    } else if prepared {
        (LayoutKind::PreparedInstallation, Some(published_state))
    } else if historical {
        (LayoutKind::HistoricalConfigDir, None)
    } else {
        (LayoutKind::Unrecognized, None)
    };
    outcome.layout = Some(Layout {
        input: target.name,
        kind,
        preparation,
        oidc_file_version: None,
    });

    let oidc_file = target.path.join(OIDC_FILE_NAME);
    match kind {
        LayoutKind::Absent => {}
        // The gateway reads just these two files, and two options of this
        // tool read them as it does: the directory itself is not inspected.
        // A registry that was read above, to tell a staging directory from a
        // published one, is listed as read: the message then says why.
        LayoutKind::Unrecognized if markers.registry || markers.oidc_file => {
            let registry_read = registry.as_ref().is_some_and(|inspection| inspection.read);
            context.report(
                draft(FindingCode::LayoutUnrecognized).variant(if registry_read {
                    "gateway_files_registry_read"
                } else {
                    "gateway_files"
                }),
            );
        }
        LayoutKind::Unrecognized => context.report(draft(FindingCode::LayoutUnrecognized)),
        LayoutKind::HistoricalConfigDir if as_installation => {
            context.report(draft(FindingCode::LayoutHistoricalPassedAsInstallationDir));
        }
        LayoutKind::HistoricalConfigDir => {
            context.report(draft(FindingCode::LayoutHistoricalConfigDir));
            outcome.historical = true;
        }
        LayoutKind::HandWrittenRegistry => {}
        LayoutKind::PreparedInstallation | LayoutKind::SetupStagingDirectory
            if !as_installation =>
        {
            context.report(draft(FindingCode::LayoutPreparedPassedAsConfigDir));
        }
        LayoutKind::PreparedInstallation => {
            context.report(draft(FindingCode::LayoutPreparedInstallation));
            context.report(draft(FindingCode::LayoutLegacyCommandsDoNotReadThisLayout));
            if preparation == Some(PreparationState::CompletionMarkerAbsent) {
                context.report(draft(FindingCode::LayoutCompletionMarkerAbsent));
            }
            describe_setup_records(context, target);
            // A published installation without its registry or its OIDC file
            // is reported by the read of that file.
            outcome.installation = Some(Installation {
                registry: published_registry(
                    context,
                    target,
                    withheld,
                    registry.take(),
                    &registry_path,
                    max_events,
                ),
                oidc_file: Some(oidc_file),
                published: true,
            });
        }
        LayoutKind::SetupStagingDirectory => {
            if !markers.preparation {
                context.report(draft(FindingCode::LayoutStagingWithoutIntent));
                return outcome;
            }
            // Only what the files of this directory record is looked at: the
            // final path is never derived from the staging name.
            let final_exists = registry.as_ref().is_some_and(|inspection| {
                recorded_paths(inspection).any(|path| fs_facts::exists(path))
            });
            context.report(draft(if final_exists {
                FindingCode::LayoutStagingAndFinalBothExist
            } else {
                FindingCode::LayoutSetupStagingDirectory
            }));
            describe_setup_records(context, target);
            if withheld {
                report_withheld_registry(context, target);
            }
            outcome.installation = Some(Installation {
                registry: registry.take(),
                oidc_file: markers.oidc_file.then_some(oidc_file),
                published: false,
            });
        }
        LayoutKind::Mixed => {
            context.report(draft(FindingCode::LayoutMixed));
            if as_installation && prepared {
                describe_setup_records(context, target);
                outcome.installation = Some(Installation {
                    registry: published_registry(
                        context,
                        target,
                        withheld,
                        registry.take(),
                        &registry_path,
                        max_events,
                    ),
                    oidc_file: Some(oidc_file),
                    published: !staging,
                });
            } else if !as_installation {
                outcome.historical = true;
            }
        }
    }
    outcome
}

fn report_withheld_registry(context: &mut Context, target: &Target) {
    context.report(
        Draft::new(
            FindingCode::RegistryFileUnreadable,
            Scope::Input { input: target.name },
        )
        .variant("reserved_name"),
    );
}

/// The registry of an installation, for the report: the one already read, or
/// a read of the fixed name now, so that an absent registry is reported by
/// that read. `None` when the name was withheld: that is reported instead.
fn published_registry(
    context: &mut Context,
    target: &Target,
    withheld: bool,
    read: Option<RegistryInspection>,
    path: &Path,
    max_events: u64,
) -> Option<RegistryInspection> {
    if withheld {
        report_withheld_registry(context, target);
        return None;
    }
    Some(match read {
        Some(inspection) => inspection,
        None => tenants::inspect(context, path, max_events),
    })
}

/// The records that setup keeps in a prepared installation: presence and
/// metadata only. The first one holds the generated tokens; none is ever
/// opened. A creation record that does not exist is not listed: a version 1
/// preparation writes none.
fn describe_setup_records(context: &mut Context, target: &Target) {
    let records = SETUP_RECORDS.into_iter().map(|name| (name, false)).chain([
        (AUTHENTICATION_CREATION_RECORD, true),
        (AUTHENTICATION_ORIGIN_RECORD, false),
    ]);
    for (name, optional) in records {
        let present = references::facts_only(
            context,
            Declared {
                path: &target.path.join(name),
                owner: RawOwner::installation(),
                role: ReferenceRole::SetupMaterial,
                input: target.name,
                init_target: None,
            },
            Consumer::SetupRecovery,
            optional,
        );
        if present && name == SETUP_RECORDS[0] {
            context.report(Draft::new(
                FindingCode::ReferenceSetupSecretMaterialPresent,
                Scope::Input { input: target.name },
            ));
        }
    }
}

/// The references of the historical layout: the private key sources read by
/// the legacy commands (`src/runner.rs`, `read_token`) and their read-only
/// projections for the Compose stack. The decision-engine key is optional.
/// `init_applies` is true for a directory that holds the historical layout
/// alone: `init` recreates a missing key there, and must not be run again
/// over a mixed directory.
pub(crate) fn describe_historical(context: &mut Context, target: &Target, init_applies: bool) {
    let sources = target.path.join("secrets");
    let projections = target.path.join("mounted-secrets");
    let engine_key = fs_facts::exists(&sources.join("jev"));
    for (role, name, optional) in [
        (ReferenceRole::Runner, "runner", false),
        (ReferenceRole::Viewer, "viewer", false),
        (ReferenceRole::DecisionEngine, "jev", true),
    ] {
        references::private_file(
            context,
            Declared {
                path: &sources.join(name),
                owner: RawOwner::installation(),
                role,
                input: target.name,
                init_target: init_applies.then_some(target.display.as_str()),
            },
            optional,
        );
    }
    for (name, optional) in [("runner", false), ("viewer", false), ("jev", !engine_key)] {
        references::facts_only(
            context,
            Declared {
                path: &projections.join(name),
                owner: RawOwner::installation(),
                role: ReferenceRole::Projection,
                input: target.name,
                init_target: None,
            },
            Consumer::ComposeProjection,
            optional,
        );
    }
    let scope = Scope::Input { input: target.name };
    context.report(Draft::new(
        FindingCode::DataLocationNotPersisted,
        scope.clone(),
    ));
    context.report(
        Draft::new(
            FindingCode::DataOfflineInspectionNotAvailable,
            scope.clone(),
        )
        .variant("historical"),
    );
    context.report(Draft::new(
        FindingCode::DataSignedCheckpointNotChecked,
        scope,
    ));
}
