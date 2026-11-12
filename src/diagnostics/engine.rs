//! The engine program, approved and never executed.
//!
//! `Runtime::new` and `Runtime::with_program` (`src/runtime.rs`) apply the
//! approval the product applies before any engine call, and build no command.
//! The approval is not free of host reads: it asks the password database for
//! the home of the control account, looks at `/run/user/<uid>` and at the
//! system directories of the helper `PATH`, and hashes the program twice.
//!
//! Only four facts of an approved program reach the report. The invocation
//! binding itself is never serialised: its environment, working directory,
//! canonical path, device, inode and length stay out. A refusal is reported
//! by a closed reason, compared with the product's own literals; its text may
//! carry a system error and is never printed. Whether the engine runs is never
//! examined here: that needs `agentsandbox doctor`, which executes it.
//!
//! The compared texts and their reasons are public so that a test can bind
//! each of them to the product source: a refusal that the product rewords
//! would otherwise degrade to `unclassified` without any failure.

use super::{
    Context, Draft,
    catalogue::FindingCode,
    fs_facts::{self, Lookup},
    output::MAX_PATH_BYTES,
    report::{ContentRole, Details, Engine, InputName, Reason, Scope},
};
use crate::runtime::{Runtime, RuntimeKind};
use std::path::Path;

/// The refusal of `Runtime::new` when none of the fixed entries exists
/// (`src/runtime/invocation.rs`, `factory`).
pub const ABSENT_FROM_FIXED_ENTRIES: &str = "container runtime is absent from approved installation locations; provide --runtime-program with an approved absolute executable; no PATH or local fallback exists";

/// The refusal of the approval for a path that is not absolute, that holds a
/// traversal or a control character, or that is not UTF-8.
const PATH_FORM_REFUSED: &str =
    "runtime program must be an absolute UTF-8 path without traversal or control characters";
/// The refusal of the approval when the path cannot be resolved.
const UNRESOLVED: &str = "approved runtime program cannot be resolved";

/// Refusals of the approval, by their exact text
/// (`src/runtime/invocation.rs`), each with its closed reason and with the
/// situation that the texts of the report name for it. A text that is not
/// listed is `unclassified` and has the generic texts: a changed literal
/// degrades the reason and the wording, never the verdict. The first literal
/// is also the one the product gives to an account home of an unusable form,
/// which cannot be told apart from a program path.
pub const REFUSALS: &[(&str, Reason, &str)] = &[
    (
        PATH_FORM_REFUSED,
        Reason::ProgramOrDirectoryRefused,
        "path_form",
    ),
    (UNRESOLVED, Reason::ProgramOrDirectoryRefused, "unresolved"),
    (
        "runtime executable directory is unavailable",
        Reason::ProgramOrDirectoryRefused,
        "directory_unavailable",
    ),
    (
        "runtime executable directory must be owned by root or control, without group/other write permission",
        Reason::ProgramOrDirectoryRefused,
        "directory_not_trusted",
    ),
    (
        "runtime program must be a regular executable owned by root or control, without group/other write permission",
        Reason::ProgramOrDirectoryRefused,
        "program_not_trusted",
    ),
    (
        "runtime executable exceeds the 512 MiB approval limit",
        Reason::ProgramOrDirectoryRefused,
        "size_bound",
    ),
    (
        "runtime installation symlink points outside approved installation roots; provide an explicit absolute runtime program",
        Reason::ProgramOrDirectoryRefused,
        "link_outside_approved_roots",
    ),
    // The file system that holds the program (`src/control_fs.rs`,
    // `validate_filesystem_permissions`), on Linux and on macOS.
    (
        "control filesystem type is outside the supported local POSIX-ACL policy; network/FUSE ACL semantics require separate support",
        Reason::ProgramOrDirectoryRefused,
        "filesystem_unsupported",
    ),
    (
        "control filesystem type is outside the supported local APFS/HFS policy; choose a supported private destination",
        Reason::ProgramOrDirectoryRefused,
        "filesystem_unsupported",
    ),
    (
        "control/runtime filesystem ignores ownership permissions; enable ownership on a supported volume or use a new private destination",
        Reason::ProgramOrDirectoryRefused,
        "filesystem_unsupported",
    ),
    (
        "control OS account home is unavailable",
        Reason::AccountHomeUnavailable,
        "account_home",
    ),
    (
        "control configuration path must be UTF-8",
        Reason::AccountHomeUnavailable,
        "account_home",
    ),
    (
        "runtime helper PATH requires system directories owned by root without group/other write permission",
        Reason::HelperDirectoriesRefused,
        "helper_directories",
    ),
    // Two refusals that none of the three reasons names: a refused
    // `/run/user/<uid>`, and an extended access entry, which the product
    // gives for the program and for a helper directory alike. Their reason
    // stays `unclassified`; their cause is said by their own texts.
    (
        "standard control runtime directory has unsafe ownership or permissions",
        Reason::Unclassified,
        "runtime_directory",
    ),
    (
        "runtime executable or its final directory has an extended ACL; explicit control installation without ACL entries is required",
        Reason::Unclassified,
        "extended_access_entry",
    ),
];

/// Refusals whose text ends with a system error: only the fixed part is
/// compared.
pub const REFUSAL_PREFIXES: &[(&str, Reason, &str)] = &[
    (
        "approved runtime directory cannot be pinned without following a link: ",
        Reason::ProgramOrDirectoryRefused,
        "directory_not_pinned",
    ),
    (
        "approved runtime executable cannot be opened relative to its pinned directory: ",
        Reason::ProgramOrDirectoryRefused,
        "program_not_opened",
    ),
];

/// The closed reason of a refusal of the approval, and the situation that
/// its text names, from its text.
pub fn classify(refusal: &str) -> (Reason, Option<&'static str>) {
    REFUSALS
        .iter()
        .find(|(literal, ..)| *literal == refusal)
        .or_else(|| {
            REFUSAL_PREFIXES
                .iter()
                .find(|(prefix, ..)| refusal.starts_with(prefix))
        })
        .map_or((Reason::Unclassified, None), |(_, reason, situation)| {
            (*reason, Some(*situation))
        })
}

/// The closed reason of a refusal of the approval, from its text.
pub fn reason(refusal: &str) -> Reason {
    classify(refusal).0
}

/// An engine program given by option.
pub(crate) struct GivenProgram<'a> {
    /// As written on the command line: the product approves that exact path
    /// and refuses a relative one.
    pub given: &'a Path,
    /// The same path, normalised, for this tool's own look at it.
    pub normalised: &'a Path,
    pub display: &'a str,
}

/// Approve the engine program when an engine is named. The state of the
/// engine is reported as not checked in every run.
pub(crate) fn report(
    context: &mut Context,
    kind: Option<RuntimeKind>,
    program: Option<&GivenProgram>,
) -> Option<Engine> {
    context.report(Draft::new(FindingCode::EngineStateNotChecked, Scope::Host));
    let Some(kind) = kind else {
        context.report(Draft::new(
            FindingCode::EngineProgramNotInspected,
            Scope::Host,
        ));
        return None;
    };
    let scope = Scope::Input {
        input: InputName::Runtime,
    };
    // A path this installation uses for a key, a setup record or a store is
    // never opened, whatever option names it. The approval would resolve its
    // links, hash it and hand its digest to the report.
    if let Some(program) = program
        && context.never_opened.contains(program.normalised)
    {
        context.report(Draft::new(
            FindingCode::EngineProgramIsACredentialReference,
            Scope::Input {
                input: InputName::RuntimeProgram,
            },
        ));
        return None;
    }
    let approval = match program {
        Some(program) => Runtime::with_program(kind, program.given),
        None => Runtime::new(kind),
    };
    let refusal = match approval {
        Ok(runtime) => {
            let binding = runtime.invocation();
            context.read.insert(ContentRole::EngineProgram);
            context.report(
                Draft::new(FindingCode::EngineProgramApproved, scope).details(
                    Details::EngineProgram {
                        explicit_program: program.is_some(),
                    },
                ),
            );
            return Some(Engine {
                kind,
                entry_display: match program {
                    Some(program) => program.display.to_owned(),
                    // One of the fixed system locations of the product.
                    None => context
                        .safe
                        .dynamic(&binding.approved_entry.to_string_lossy(), MAX_PATH_BYTES),
                },
                sha256: binding.file_identity.sha256.clone(),
                policy_version: binding.policy_version,
            });
        }
        Err(error) => error.to_string(),
    };
    if program.is_none() && refusal == ABSENT_FROM_FIXED_ENTRIES {
        context.report(Draft::new(
            FindingCode::EngineProgramAbsentFromFixedEntries,
            scope,
        ));
        return None;
    }
    // The approval hashes the program before most of its checks can refuse.
    // A refused program is therefore listed as read unless nothing exists at
    // the given path: the list of files read never understates a read.
    if program.is_none_or(|program| fs_facts::exists(program.normalised)) {
        context.read.insert(ContentRole::EngineProgram);
    }
    let (reason, situation) = classify(&refusal);
    let mut draft =
        Draft::new(FindingCode::EngineProgramRefused, scope).details(Details::Reason { reason });
    // The closed reason is the product's, and so is the cause that the
    // message names. This tool's own look at the given path says what is
    // wrong where the product's text leaves several causes.
    if let Some(situation) = situation {
        draft = draft.variant(situation);
    }
    if let Some(program) = program {
        if refusal == PATH_FORM_REFUSED && program.given.is_relative() {
            draft = draft.variant("relative");
        } else if refusal == UNRESOLVED
            && matches!(fs_facts::lookup(program.normalised), Lookup::Absent)
        {
            draft = draft.variant("absent");
        }
    }
    context.report(draft);
    None
}
