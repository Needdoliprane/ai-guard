//! Operator inspection of an ai-guard installation: `ai-guard-diagnose`.
//!
//! The tool is offline and read-only. It starts nothing, contacts nothing,
//! writes nothing and reads no environment variable. It opens no file that
//! the inspected files name as a key, a token, a credential or a store; a
//! file given by option is opened as what that option says it is, and the
//! report says so when a file read later names it as one of those. It looks
//! only at the paths it is given and lists no directory. What it cannot
//! examine is reported as `not_checked` or `not_available`, never as a
//! success.
//!
//! One thing leaves the offline mode, and only when its option names it: an
//! active check. `--active oidc-discovery` asks the identity provider of an
//! inspected OIDC file for its discovery document and its keys, through the
//! client of the product and within its bounds. The report then says so in
//! its mode.
//!
//! The report is a versioned JSON document or a plain text of the same
//! content (`report`). It is written to stdout in one write, after the output
//! guards of `output`. A refusal writes nothing to stdout.

pub mod catalogue;
pub mod engine;
mod fs_facts;
pub mod gateway_files;
mod identity;
mod layout;
mod output;
mod providers;
mod references;
mod render;
pub mod report;
mod tenants;
mod trust;

use crate::control_fs;
use catalogue::{FindingCode, TARGET_PLACEHOLDER};
use output::{IdKind, IdTable, Root, Safe, quoted};
use references::{DataRecord, RawOwner, ReferenceRecord};
use report::{
    ContentRole, ContentState, Counts, CredentialContentUse, DataEntry, Details, Finding, Identity,
    Inference, Input, InputName, InputOrigin, Layout, LayoutKind, Location, Mode, MutationUse,
    NetworkUse, Omitted, Origin, Owner, OwnerKind, Presence, ProviderFileState, ReferenceCounts,
    ReferenceEntry, ReferenceRole, ReportV1, Requested, RequestedSelection, Scope, Source, Status,
    SubjectKind, Subjects, Summary, SummaryStatus, TenantModel, Tenants, Tool, Topology,
};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fmt,
    io::Write,
    path::{Component, Path, PathBuf},
};

/// Bound of the written document, in bytes.
pub const MAX_DOCUMENT_BYTES: usize = 524_288;
/// The gateway default for `--max-events`, assumed when none is given.
pub const DEFAULT_MAX_EVENTS: u64 = 100_000;
/// The default of `init`, `run` and `providers`, used when no target is given.
pub const DEFAULT_CONFIG_DIR: &str = ".agentsandbox";

pub const EXIT_OK: i32 = 0;
pub const EXIT_REFUSED: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_ERROR: i32 = 3;
pub const EXIT_UNKNOWN: i32 = 4;

const CONFIG_PLACEHOLDER: &str = "<config>";
const INSTALLATION_PLACEHOLDER: &str = "<installation>";
/// The directory of a registry or of an OIDC file given alone: a reference
/// below it is shown relative to it.
const TENANTS_FILE_DIRECTORY_PLACEHOLDER: &str = "<tenants-file-dir>";
const OIDC_CONFIG_DIRECTORY_PLACEHOLDER: &str = "<oidc-config-dir>";

/// The AI client a managed launch is asked for, with the values `run` accepts
/// for its option of the same name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ClientChoice {
    Auto,
    Codex,
    ClaudeCode,
}

impl ClientChoice {
    /// The value as it is written on the command line.
    fn word(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
        }
    }
}

/// No option reads an environment variable: the tool takes its inputs from
/// the command line only, and uses the current directory only to resolve
/// relative arguments.
#[derive(clap::Args, Clone, Debug)]
pub struct DiagnoseArgs {
    /// Historical config directory, as given to init, run and providers
    /// [default: .agentsandbox when no target is given]
    #[arg(long, value_name = "DIR")]
    pub config_dir: Option<PathBuf>,
    /// Directory created by setup prepare, or its retained staging directory;
    /// its own registry and OIDC file are inspected
    #[arg(long, value_name = "DIR", conflicts_with_all = ["tenants_file", "oidc_config"])]
    pub installation_dir: Option<PathBuf>,
    /// Tenant registry, as given to the gateway; not together with an installation directory
    #[arg(long, value_name = "FILE")]
    pub tenants_file: Option<PathBuf>,
    /// OIDC file, as given to the gateway; not together with an installation directory
    #[arg(long, value_name = "FILE")]
    pub oidc_config: Option<PathBuf>,
    /// Provider profiles, as given to run [default: providers.json of the config directory]
    #[arg(long, value_name = "FILE")]
    pub providers_file: Option<PathBuf>,
    /// AI client asked from a managed launch, as given to run [default: auto]
    #[arg(long, value_enum, value_name = "CLIENT")]
    pub client: Option<ClientChoice>,
    /// Authentication mode asked from a managed launch, as given to run [default: auto]
    #[arg(long, value_enum, value_name = "MODE")]
    pub auth_mode: Option<crate::provider::AuthPreference>,
    /// Provider profile asked from a managed launch, as given to run
    #[arg(long, value_name = "ID")]
    pub client_profile: Option<String>,
    /// Model asked from a managed launch, as given to run; it replaces the model of the profile
    #[arg(long, value_name = "ID")]
    pub model: Option<String>,
    /// Gateway event bound used to validate tenant policies [default: 100000, reported as assumed]
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..=1_000_000))]
    pub max_events: Option<u64>,
    /// Per-session live provider activity event budget of the broker [default: 1000]
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..=5_000))]
    pub activity_max_events: Option<u64>,
    /// Per-session live provider activity byte budget of the broker [default: 2097152]
    #[arg(long, value_name = "BYTES", value_parser = clap::value_parser!(u64).range(65_536..=8_388_608))]
    pub activity_max_bytes: Option<u64>,
    /// Decision-engine URL to validate; never contacted; not together with --jev-local-url
    #[arg(long, value_name = "URL", conflicts_with = "jev_local_url")]
    pub jev_url: Option<String>,
    /// Local decision-engine URL to validate; never contacted; not together with --jev-url
    #[arg(long, value_name = "URL")]
    pub jev_local_url: Option<String>,
    /// The gateway is started with its explicit local HTTP permission; judged only with a
    /// decision-engine URL
    #[arg(long)]
    pub jev_allow_local_http: bool,
    /// Public CA bundle given to the gateway for the decision engine
    #[arg(long, value_name = "FILE")]
    pub jev_ca_file: Option<PathBuf>,
    /// Public CA bundle used by the clients of an HTTPS gateway
    #[arg(long, value_name = "FILE")]
    pub gateway_ca_file: Option<PathBuf>,
    /// Engine whose program is approved without being executed
    #[arg(long, value_enum)]
    pub runtime: Option<crate::runtime::RuntimeKind>,
    /// Absolute engine program, instead of the fixed system locations; needs --runtime
    #[arg(long, value_name = "ABS", requires = "runtime")]
    pub runtime_program: Option<PathBuf>,
    /// Write the versioned JSON document instead of text
    #[arg(long)]
    pub json: bool,
    /// Show absolute paths instead of placeholders
    #[arg(long)]
    pub show_paths: bool,
    /// Check that contacts something, never run unless it is named here;
    /// oidc-discovery asks the identity provider of the OIDC file for its
    /// discovery document and its keys
    #[arg(long, value_enum, value_name = "CHECK")]
    pub active: Vec<report::ActiveCheck>,
}

/// Closed refusals: no report is written. Each text names a next step and
/// carries no operator data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnoseError {
    InvalidInput,
    ProtectedOutput,
    OutputTooLarge,
    OutputUnavailable,
    Internal,
}

impl fmt::Display for DiagnoseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidInput => {
                "ai-guard-diagnose: an argument path cannot be used: it contains a parent traversal, or the current directory is unavailable. No report was written. Use a path without parent traversal."
            }
            Self::ProtectedOutput => {
                "ai-guard-diagnose: the report was withheld by the output guard. No report was written. Rerun with --json; if it persists, report a defect with the command line only."
            }
            Self::OutputTooLarge => {
                "ai-guard-diagnose: the report exceeds its size bound. No report was written. Inspect one target at a time."
            }
            Self::OutputUnavailable => {
                "ai-guard-diagnose: the report could not be written. Check the output destination."
            }
            Self::Internal => {
                "ai-guard-diagnose: internal error. No report was written. Report a defect with the command line only."
            }
        })
    }
}

impl std::error::Error for DiagnoseError {}

/// Builds the report, guards it and writes it once to stdout. Returns the
/// exit code of a written report: 0, 3 or 4.
pub async fn execute(args: DiagnoseArgs) -> Result<i32, DiagnoseError> {
    let mut report = build_report(&args).await?;
    let mut bytes = encode(&report, args.json)?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        omit_details(&mut report);
        bytes = encode(&report, args.json)?;
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(DiagnoseError::OutputTooLarge);
        }
    }
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&bytes)
        .and_then(|()| stdout.flush())
        .map_err(|_| DiagnoseError::OutputUnavailable)?;
    Ok(report.summary.exit_code)
}

/// Everything except the write: no stdout, no stderr. Nothing is awaited
/// unless an active check was asked for.
pub async fn build_report(args: &DiagnoseArgs) -> Result<ReportV1, DiagnoseError> {
    let safe = Safe::new();
    let inspection = Inspection::new(args, &safe)?;
    Ok(inspection.run().await)
}

/// Help and version print clap's static text on stdout and return 0. Any
/// other parse failure prints one closed sentence on stderr and returns 2:
/// clap's own message would echo the offending argument.
pub fn usage_exit(error: &clap::Error) -> i32 {
    use clap::error::ErrorKind;
    let sentence = match error.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            return if error.print().is_ok() {
                EXIT_OK
            } else {
                EXIT_REFUSED
            };
        }
        ErrorKind::UnknownArgument | ErrorKind::InvalidSubcommand | ErrorKind::TooManyValues => {
            "unknown option or unexpected argument"
        }
        ErrorKind::InvalidValue | ErrorKind::ValueValidation => {
            "an option has a value outside its closed set"
        }
        _ => "invalid command line",
    };
    eprintln!("ai-guard-diagnose: {sentence}; run ai-guard-diagnose --help");
    EXIT_USAGE
}

/// A panic prints the fixed internal text on stderr and exits 1, with an
/// empty stdout: the report is written in one write, after every check.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|_| {
        eprintln!("{}", DiagnoseError::Internal);
        std::process::exit(EXIT_REFUSED);
    }));
}

fn encode(report: &ReportV1, json: bool) -> Result<Vec<u8>, DiagnoseError> {
    let guard = Safe::new();
    if json {
        let value = serde_json::to_value(report).map_err(|_| DiagnoseError::Internal)?;
        guard.guard_json(&value)?;
        // Encode the typed report, not the checked value: the document keeps
        // the field order of the contract.
        let mut bytes = serde_json::to_vec_pretty(report).map_err(|_| DiagnoseError::Internal)?;
        bytes.push(b'\n');
        Ok(bytes)
    } else {
        let text = render::render(report);
        guard.guard_text(&text)?;
        Ok(text.into_bytes())
    }
}

/// A finding built from its catalogue entry. `variant` names a situation of
/// the entry; `target` is the display of the operator's own input, written
/// quoted wherever the action names it.
fn finding(
    code: FindingCode,
    scope: Scope,
    variant: Option<&str>,
    target: Option<&str>,
) -> Finding {
    let entry = code.entry();
    let target = target.map_or_else(|| "DIR".to_owned(), quoted);
    Finding {
        code,
        axis: entry.axis,
        status: entry.status,
        basis: entry.basis,
        scope,
        origin: Origin {
            sources: Vec::new(),
            consumer: entry.consumer.map(str::to_owned),
            assumes: Vec::new(),
        },
        message: entry.message_for(variant).to_owned(),
        action: entry
            .action_for(variant)
            .replace(TARGET_PLACEHOLDER, &target),
        requires: entry.requires.map(str::to_owned),
        subjects: None,
        details: None,
    }
}

/// Order findings by axis, code and scope, then derive the summary. Two runs
/// on an unchanged tree give the same bytes.
fn finalise(report: &mut ReportV1) {
    report.findings.sort_by(|left, right| {
        (left.axis, left.code.as_str(), &left.scope).cmp(&(
            right.axis,
            right.code.as_str(),
            &right.scope,
        ))
    });
    let mut counts = Counts::default();
    for finding in report.findings.iter().chain(&report.limits) {
        let count = match finding.status {
            Status::Ok => &mut counts.ok,
            Status::Info => &mut counts.info,
            Status::Warning => &mut counts.warning,
            Status::Error => &mut counts.error,
            Status::Unknown => &mut counts.unknown,
            Status::NotChecked => &mut counts.not_checked,
            Status::NotAvailable => &mut counts.not_available,
        };
        *count += 1;
    }
    let (status, exit_code) = if counts.error > 0 {
        (SummaryStatus::Error, EXIT_ERROR)
    } else if counts.unknown > 0 {
        (SummaryStatus::Unknown, EXIT_UNKNOWN)
    } else {
        (SummaryStatus::Ok, EXIT_OK)
    };
    report.summary.status = status;
    report.summary.exit_code = exit_code;
    report.summary.counts = counts;
}

/// Empty the detail arrays of a document that exceeds its bound. Every item
/// was evaluated; the totals and the findings stay exact.
fn omit_details(report: &mut ReportV1) {
    let count = |length: usize| u64::try_from(length).unwrap_or(u64::MAX);
    let omitted = &mut report.summary.omitted;
    omitted.reference_entries = omitted
        .reference_entries
        .saturating_add(count(report.credential_references.len()));
    omitted.data_entries = omitted
        .data_entries
        .saturating_add(count(report.data.len()));
    report.credential_references.clear();
    report.data.clear();
    if let Some(tenants) = &mut report.tenants {
        for list in [&mut tenants.registry, &mut tenants.plan]
            .into_iter()
            .flatten()
        {
            omitted.tenant_ids = omitted.tenant_ids.saturating_add(count(list.len()));
            list.clear();
        }
    }
    report.summary.truncated = true;
    if !report
        .findings
        .iter()
        .any(|finding| finding.code == FindingCode::OutputDetailOmitted)
    {
        report.findings.push(finding(
            FindingCode::OutputDetailOmitted,
            Scope::Host,
            None,
            None,
        ));
    }
    finalise(report);
}

/// One directory the operator asked to inspect.
pub(crate) struct Target {
    pub(crate) name: InputName,
    pub(crate) origin: InputOrigin,
    pub(crate) path: PathBuf,
    pub(crate) display: String,
}

/// A finding before identifiers have their display. Entries that share a
/// code, a scope and the same details are one finding, with the tenants or
/// the provider profiles they concern as its subjects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Draft {
    code: FindingCode,
    scope: Scope,
    variant: Option<&'static str>,
    target: Option<String>,
    sources: Vec<Source>,
    assumes: Vec<InputName>,
    details: Option<Details>,
    tenants: BTreeSet<String>,
    profiles: BTreeSet<String>,
}

impl Draft {
    pub(crate) fn new(code: FindingCode, scope: Scope) -> Self {
        Self {
            code,
            scope,
            variant: None,
            target: None,
            sources: Vec::new(),
            assumes: Vec::new(),
            details: None,
            tenants: BTreeSet::new(),
            profiles: BTreeSet::new(),
        }
    }

    /// A named situation of the catalogue entry.
    pub(crate) fn variant(mut self, name: &'static str) -> Self {
        self.variant = Some(name);
        self
    }

    /// The display of the operator's own input, for an action that names it.
    pub(crate) fn target(mut self, display: &str) -> Self {
        self.target = Some(display.to_owned());
        self
    }

    /// A file, and a field of it, the finding is drawn from. Both are fixed
    /// words of this tool, never operator data.
    pub(crate) fn source(mut self, file: &'static str, field: Option<&'static str>) -> Self {
        self.sources.push(Source {
            file: file.to_owned(),
            field: field.map(str::to_owned),
        });
        self
    }

    /// An assumed input the verdict depends on.
    pub(crate) fn assumes(mut self, input: InputName) -> Self {
        self.assumes.push(input);
        self
    }

    pub(crate) fn details(mut self, details: Details) -> Self {
        self.details = Some(details);
        self
    }

    /// A tenant the finding concerns, as its file declares it.
    pub(crate) fn tenant(mut self, id: &str) -> Self {
        self.tenants.insert(id.to_owned());
        self
    }

    /// A provider profile the finding concerns, as its file declares it. A
    /// finding concerns tenants or profiles, never both.
    pub(crate) fn profile(mut self, id: &str) -> Self {
        self.profiles.insert(id.to_owned());
        self
    }

    fn same_finding(&self, other: &Self) -> bool {
        self.code == other.code
            && self.scope == other.scope
            && self.variant == other.variant
            && self.target == other.target
            && self.details == other.details
    }

    fn build(self, tenants: &IdTable, profiles: &IdTable) -> Finding {
        let mut built = finding(self.code, self.scope, self.variant, self.target.as_deref());
        built.origin.sources = self.sources;
        built.origin.assumes = self.assumes;
        built.details = self.details;
        built.subjects = subjects(SubjectKind::Tenants, &self.tenants, tenants)
            .or_else(|| subjects(SubjectKind::ProviderProfiles, &self.profiles, profiles));
        built
    }
}

/// The subjects of a finding: sorted by identifier; every one is counted, a
/// bounded number is printed.
fn subjects(kind: SubjectKind, raw: &BTreeSet<String>, displays: &IdTable) -> Option<Subjects> {
    if raw.is_empty() {
        return None;
    }
    let ids: Vec<_> = raw
        .iter()
        .take(report::MAX_SUBJECT_IDS)
        .map(|id| displays.display(id))
        .collect();
    Some(Subjects {
        kind,
        total: count(raw.len()),
        omitted: count(raw.len() - ids.len()),
        ids,
    })
}

fn count(length: usize) -> u64 {
    u64::try_from(length).unwrap_or(u64::MAX)
}

/// What the inspections of one run share. Nothing here writes or prints.
pub(crate) struct Context<'a> {
    pub(crate) safe: &'a Safe,
    show_paths: bool,
    /// Directories whose content is shown relative to a placeholder.
    roots: Vec<Root>,
    /// The files opened for content in this run.
    pub(crate) read: BTreeSet<ContentRole>,
    /// The same files by path, in the order they were opened.
    opened: Vec<(ContentRole, PathBuf)>,
    /// Key files, setup records and stores: never opened, whatever names them.
    pub(crate) never_opened: fs_facts::NeverOpened,
    pub(crate) references: Vec<ReferenceRecord>,
    pub(crate) data: Vec<DataRecord>,
    drafts: Vec<Draft>,
    outside: u64,
}

impl Context<'_> {
    /// Add a finding, or its subjects to the same finding already reported.
    pub(crate) fn report(&mut self, draft: Draft) {
        match self
            .drafts
            .iter_mut()
            .find(|existing| existing.same_finding(&draft))
        {
            Some(existing) => {
                existing.tenants.extend(draft.tenants);
                existing.profiles.extend(draft.profiles);
            }
            None => self.drafts.push(draft),
        }
    }

    /// Record a file opened for content. Its role is listed in the report.
    /// Its path is compared again, once every inspected file is read, with
    /// the files that must never be opened: a file can only be compared with
    /// what was known when it was opened.
    pub(crate) fn opened(&mut self, role: ContentRole, path: &Path) {
        self.read.insert(role);
        self.opened.push((role, path.to_owned()));
    }

    /// Where a referenced path lies, and how it is shown. A path below an
    /// inspected directory is shown relative to its placeholder. Any other
    /// location is chosen by the operator: without `--show-paths` it is shown
    /// as its role and an ordinal, never as a file name.
    pub(crate) fn place(&mut self, path: &Path, noun: &str) -> (Location, String) {
        let inside = self.roots.iter().any(|root| path.starts_with(&root.path));
        let opaque = if inside {
            String::new()
        } else {
            self.outside += 1;
            format!(
                "{noun} reference #{} (outside the inspected directory)",
                self.outside
            )
        };
        let display = self.safe.path(path, &self.roots, self.show_paths, &opaque);
        let location = if inside {
            Location::InsideRoot
        } else {
            Location::OutsideRoot
        };
        (location, display)
    }

    fn add_root(&mut self, placeholder: &'static str, path: &Path) {
        if is_display_root(path) && !self.roots.iter().any(|root| root.path == path) {
            self.roots.push(Root {
                placeholder,
                path: path.to_owned(),
            });
        }
    }
}

/// Whether the paths below a directory are shown relative to it. The root of
/// the file system has no name of its own and holds every absolute path:
/// shown relative to it, a path would be printed in full without
/// `--show-paths`, and every reference would count as inside an inspected
/// directory.
fn is_display_root(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::Normal(_)))
}

/// A file given by option: its normalised path and its display.
struct GivenFile {
    path: PathBuf,
    display: String,
}

impl GivenFile {
    fn bundle(&self) -> trust::GivenBundle<'_> {
        trust::GivenBundle {
            path: &self.path,
            display: &self.display,
        }
    }
}

/// The gateway event bound used to validate tenant policies.
struct EventBound {
    value: u64,
    /// True when no value was given and the gateway default is assumed.
    assumed: bool,
}

/// State of one run: the operator's inputs, as given and as displayed.
struct Inspection<'a> {
    safe: &'a Safe,
    /// The explicit inputs that are no path: URLs, flags and the engine kind.
    args: &'a DiagnoseArgs,
    show_paths: bool,
    targets: Vec<Target>,
    tenants_file: Option<PathBuf>,
    oidc_config: Option<PathBuf>,
    /// The profile file given alone.
    providers_file: Option<GivenFile>,
    jev_ca_file: Option<GivenFile>,
    gateway_ca_file: Option<GivenFile>,
    runtime_program: Option<GivenFile>,
    event_bound: EventBound,
    /// Per-session live provider activity budget, given or default.
    activity_budget: (u64, u64),
    inputs: Vec<Input>,
    /// Where the profile file of the config directory is listed, when it is
    /// the one inspected.
    providers_slot: usize,
    /// Where the assumed event bound is listed, when a check uses it.
    event_bound_slot: usize,
    roots: Vec<Root>,
}

impl<'a> Inspection<'a> {
    fn new(args: &'a DiagnoseArgs, safe: &'a Safe) -> Result<Self, DiagnoseError> {
        let normalise =
            |path: &Path| control_fs::absolute_path(path).map_err(|_| DiagnoseError::InvalidInput);
        let no_target = args.config_dir.is_none()
            && args.installation_dir.is_none()
            && args.tenants_file.is_none()
            && args.oidc_config.is_none()
            && args.providers_file.is_none();
        let config_dir = match (&args.config_dir, no_target) {
            (Some(path), _) => Some((normalise(path)?, InputOrigin::Cli)),
            (None, true) => Some((
                normalise(Path::new(DEFAULT_CONFIG_DIR))?,
                InputOrigin::Default,
            )),
            (None, false) => None,
        };
        let installation_dir = args
            .installation_dir
            .as_deref()
            .map(normalise)
            .transpose()?;
        let tenants_file = args.tenants_file.as_deref().map(normalise).transpose()?;
        let oidc_config = args.oidc_config.as_deref().map(normalise).transpose()?;
        let roots: Vec<Root> = [
            (
                CONFIG_PLACEHOLDER,
                config_dir.as_ref().map(|(path, _)| path),
            ),
            (INSTALLATION_PLACEHOLDER, installation_dir.as_ref()),
        ]
        .into_iter()
        .filter_map(|(placeholder, path)| {
            path.map(|path| Root {
                placeholder,
                path: path.clone(),
            })
        })
        .collect();

        let mut targets = Vec::new();
        let mut inputs = Vec::new();
        let mut directory = |name: InputName, origin: InputOrigin, path: PathBuf| {
            // The root of the file system stands for itself only. Every other
            // directory lies below it: shown relative to it, the directory
            // would be printed in full without `--show-paths`.
            let shown: Vec<Root> = roots
                .iter()
                .filter(|root| root.path == path || is_display_root(&root.path))
                .cloned()
                .collect();
            let display = safe.path(&path, &shown, args.show_paths, "[path not shown]");
            inputs.push(Input {
                name,
                origin,
                display: display.clone(),
            });
            targets.push(Target {
                name,
                origin,
                path,
                display,
            });
        };
        if let Some((path, origin)) = config_dir {
            directory(InputName::ConfigDir, origin, path);
        }
        if let Some(path) = installation_dir {
            directory(InputName::InstallationDir, InputOrigin::Cli, path);
        }
        for (name, path, placeholder) in [
            (InputName::TenantsFile, &tenants_file, "<tenants-file>"),
            (InputName::OidcConfig, &oidc_config, "<oidc-config>"),
        ] {
            if let Some(path) = path {
                inputs.push(Input {
                    name,
                    origin: InputOrigin::Cli,
                    display: safe.path(path, &[], args.show_paths, placeholder),
                });
            }
        }
        // An assumed value is an input only for the check that uses it: the
        // profile file of the config directory is listed when that directory
        // is inspected, and the default event bound by the validation of
        // tenant policies, never for a target that holds no registry.
        // A file given by option is an operator-chosen location: it is shown
        // by a placeholder, never relative to an inspected directory.
        let given_file = |inputs: &mut Vec<Input>,
                          name: InputName,
                          path: &Option<PathBuf>,
                          placeholder: &str|
         -> Result<Option<GivenFile>, DiagnoseError> {
            let Some(path) = path else {
                return Ok(None);
            };
            let path = normalise(path)?;
            let display = safe.path(&path, &[], args.show_paths, placeholder);
            inputs.push(Input {
                name,
                origin: InputOrigin::Cli,
                display: display.clone(),
            });
            Ok(Some(GivenFile { path, display }))
        };
        let providers_slot = inputs.len();
        let providers_file = given_file(
            &mut inputs,
            InputName::ProvidersFile,
            &args.providers_file,
            "<providers-file>",
        )?;
        // The selection options. Two are closed words; the two others are
        // free strings that only the product validates, shown like any other
        // operator string: escaped, masked and bounded.
        let free = |value: &Option<String>| {
            value
                .as_deref()
                .map(|value| safe.dynamic(value, output::MAX_DYNAMIC_BYTES))
        };
        for (name, display) in [
            (
                InputName::Client,
                args.client.map(|client| client.word().to_owned()),
            ),
            (
                InputName::AuthMode,
                args.auth_mode.map(|mode| {
                    match mode {
                        crate::provider::AuthPreference::Auto => "auto",
                        crate::provider::AuthPreference::Api => "api",
                        crate::provider::AuthPreference::Subscription => "subscription",
                    }
                    .to_owned()
                }),
            ),
            (InputName::ClientProfile, free(&args.client_profile)),
            (InputName::Model, free(&args.model)),
        ] {
            if let Some(display) = display {
                inputs.push(Input {
                    name,
                    origin: InputOrigin::Cli,
                    display,
                });
            }
        }
        let event_bound_slot = inputs.len();
        if let Some(value) = args.max_events {
            inputs.push(Input {
                name: InputName::MaxEvents,
                origin: InputOrigin::Cli,
                display: value.to_string(),
            });
        }
        for (name, url) in [
            (InputName::JevUrl, &args.jev_url),
            (InputName::JevLocalUrl, &args.jev_local_url),
        ] {
            if let Some(url) = url {
                inputs.push(Input {
                    name,
                    origin: InputOrigin::Cli,
                    display: safe.url(url),
                });
            }
        }
        if args.jev_allow_local_http {
            inputs.push(Input {
                name: InputName::JevAllowLocalHttp,
                origin: InputOrigin::Cli,
                display: "given".to_owned(),
            });
        }
        let jev_ca_file = given_file(
            &mut inputs,
            InputName::JevCaFile,
            &args.jev_ca_file,
            "<jev-ca-file>",
        )?;
        let gateway_ca_file = given_file(
            &mut inputs,
            InputName::GatewayCaFile,
            &args.gateway_ca_file,
            "<gateway-ca-file>",
        )?;
        if let Some(kind) = args.runtime {
            inputs.push(Input {
                name: InputName::Runtime,
                origin: InputOrigin::Cli,
                display: kind.program().to_owned(),
            });
        }
        let runtime_program = given_file(
            &mut inputs,
            InputName::RuntimeProgram,
            &args.runtime_program,
            "<runtime-program>",
        )?;
        if args.active.contains(&report::ActiveCheck::OidcDiscovery) {
            inputs.push(Input {
                name: InputName::Active,
                origin: InputOrigin::Cli,
                display: "oidc-discovery".to_owned(),
            });
        }
        Ok(Self {
            safe,
            args,
            show_paths: args.show_paths,
            targets,
            tenants_file,
            oidc_config,
            providers_file,
            jev_ca_file,
            gateway_ca_file,
            runtime_program,
            event_bound: EventBound {
                value: args.max_events.unwrap_or(DEFAULT_MAX_EVENTS),
                assumed: args.max_events.is_none(),
            },
            activity_budget: (
                args.activity_max_events
                    .unwrap_or(crate::provider_activity::collector::DEFAULT_MAX_EVENTS),
                args.activity_max_bytes
                    .unwrap_or(crate::provider_activity::collector::DEFAULT_MAX_BYTES),
            ),
            inputs,
            providers_slot,
            event_bound_slot,
            // The directory itself keeps its placeholder; what lies below the
            // root of the file system does not.
            roots: roots
                .into_iter()
                .filter(|root| is_display_root(&root.path))
                .collect(),
        })
    }

    async fn run(mut self) -> ReportV1 {
        let mut context = Context {
            safe: self.safe,
            show_paths: self.show_paths,
            roots: std::mem::take(&mut self.roots),
            read: BTreeSet::new(),
            opened: Vec::new(),
            never_opened: fs_facts::NeverOpened::default(),
            references: Vec::new(),
            data: Vec::new(),
            drafts: Vec::new(),
            outside: 0,
        };
        let mut layouts = Vec::new();
        let mut historical = false;
        let mut registry = None;
        let mut oidc = None;
        // A prepared installation, published at the inspected path.
        let mut published = None;
        let mut staging_without_oidc_file = false;
        // The config directory, and the layout found in it: `None` for a
        // directory that is absent or could not be inspected.
        let mut config_directory = None;

        // Directories first: every key file and setup record they hold is
        // known before a file that may reference them is read.
        for target in &self.targets {
            let outcome = layout::inspect(&mut context, target, self.event_bound.value);
            if target.name == InputName::ConfigDir {
                let kind = outcome
                    .layout
                    .as_ref()
                    .map(|layout| layout.kind)
                    .filter(|kind| *kind != LayoutKind::Absent);
                config_directory = Some((target, kind));
            }
            let init_applies = outcome
                .layout
                .as_ref()
                .is_some_and(|layout| layout.kind == LayoutKind::HistoricalConfigDir);
            layouts.extend(outcome.layout);
            if outcome.historical {
                layout::describe_historical(&mut context, target, init_applies);
                historical = true;
            }
            if let Some(installation) = outcome.installation {
                if installation.published {
                    published = Some(target);
                }
                staging_without_oidc_file =
                    !installation.published && installation.oidc_file.is_none();
                registry = installation
                    .registry
                    .map(|inspection| tenants::RegistrySource {
                        input: target.name,
                        label: layout::REGISTRY_NAME,
                        describe_entries: installation.published,
                        inspection,
                    });
                oidc = installation.oidc_file.map(|path| identity::OidcSource {
                    path,
                    input: target.name,
                    label: layout::OIDC_FILE_NAME,
                    describe_entries: installation.published,
                });
            }
        }
        // The order of the reads. A file is compared, before it is opened,
        // with every key file, credential, setup record and store known at
        // that moment. The files at the fixed names of the inspected
        // directories come first: the registry of an installation, read
        // above, then the profile file of a config directory. The files
        // given by option follow: registry, profile file, OIDC file, bundles
        // and engine program. A registry option that names a credential of
        // the profile file of the config directory is therefore not opened.
        let mut registry_facts = registry
            .as_ref()
            .map(|source| tenants::report_registry(&mut context, source, self.event_bound.assumed));
        let profile_file = match (self.providers_file.take(), config_directory) {
            // The file given alone is the one a launch reads, as for `run`.
            (Some(GivenFile { path, display }), _) => Some(providers::ProfileFile {
                path,
                input: InputName::ProvidersFile,
                display,
                init_target: None,
            }),
            (None, Some((target, Some(kind)))) => {
                let path = target.path.join(providers::PROFILE_FILE_NAME);
                let display =
                    context
                        .safe
                        .path(&path, &context.roots, self.show_paths, "[path not shown]");
                self.inputs.insert(
                    self.providers_slot,
                    Input {
                        name: InputName::ProvidersFile,
                        origin: InputOrigin::Default,
                        display: display.clone(),
                    },
                );
                self.event_bound_slot += 1;
                Some(providers::ProfileFile {
                    path,
                    input: InputName::ConfigDir,
                    display,
                    init_target: (kind == LayoutKind::HistoricalConfigDir)
                        .then(|| target.display.clone()),
                })
            }
            (None, Some((_, None))) => {
                context.report(
                    Draft::new(
                        FindingCode::ProviderNotInspected,
                        Scope::Input {
                            input: InputName::ConfigDir,
                        },
                    )
                    .variant("target"),
                );
                None
            }
            (None, None) => {
                context.report(Draft::new(FindingCode::ProviderNotInspected, Scope::Host));
                None
            }
        };
        let request = self.selection_request();
        let mut provider_outcome = profile_file
            .as_ref()
            .filter(|file| !file.given_alone())
            .map(|file| providers::inspect(&mut context, file, &request));

        // Files given alone. The command line refuses them together with an
        // installation directory, which brings its own two files.
        if let Some(path) = self.tenants_file.take() {
            // A path this installation uses for a key, a setup record or a
            // store is never opened, whatever option names it.
            if context.never_opened.contains(&path) {
                context.report(
                    Draft::new(
                        FindingCode::RegistryFileUnreadable,
                        Scope::Input {
                            input: InputName::TenantsFile,
                        },
                    )
                    .variant("reserved"),
                );
            } else {
                let inspection = tenants::inspect(&mut context, &path, self.event_bound.value);
                if inspection.read {
                    layouts.push(Layout {
                        input: InputName::TenantsFile,
                        kind: LayoutKind::HandWrittenRegistry,
                        preparation: None,
                        oidc_file_version: None,
                    });
                    context.report(Draft::new(
                        FindingCode::LayoutHandWrittenRegistry,
                        Scope::Input {
                            input: InputName::TenantsFile,
                        },
                    ));
                }
                // The gateway resolves a relative reference against the
                // canonical directory of the registry.
                if let Some(parent) = path.parent().and_then(|parent| parent.canonicalize().ok()) {
                    context.add_root(TENANTS_FILE_DIRECTORY_PLACEHOLDER, &parent);
                }
                registry = Some(tenants::RegistrySource {
                    input: InputName::TenantsFile,
                    label: "tenant registry",
                    describe_entries: true,
                    inspection,
                });
            }
        }
        if let Some(path) = self.oidc_config.take() {
            if let Some(parent) = path.parent() {
                context.add_root(OIDC_CONFIG_DIRECTORY_PLACEHOLDER, parent);
            }
            oidc = Some(identity::OidcSource {
                path,
                input: InputName::OidcConfig,
                label: "OIDC file",
                describe_entries: true,
            });
        }

        // The registry and the provider profiles given by option before the
        // OIDC file: the key files, the stores and the credentials they name
        // are known before a profile file, an OIDC file or a CA reference is
        // opened.
        if registry_facts.is_none() {
            registry_facts = registry.as_ref().map(|source| {
                tenants::report_registry(&mut context, source, self.event_bound.assumed)
            });
        }
        if provider_outcome.is_none() {
            provider_outcome = profile_file
                .as_ref()
                .map(|file| providers::inspect(&mut context, file, &request));
        }
        // A selection option is never accepted without effect: when no
        // profile file was loaded, the report says that nothing was resolved.
        if request.given
            && provider_outcome
                .as_ref()
                .is_none_or(|outcome| outcome.section.selection.is_none())
        {
            context.report(Draft::new(
                FindingCode::ProviderSelectionNotResolved,
                Scope::Host,
            ));
        }
        let oidc_facts = oidc
            .as_ref()
            .map(|source| identity::report_oidc(&mut context, source));
        // The explicit inputs last: every file the inspected installation
        // uses as a key, a setup record or a store is known before a bundle
        // given by option is opened.
        let trust_facts = trust::report(
            &mut context,
            &trust::Given {
                jev_url: self.args.jev_url.as_deref(),
                jev_local_url: self.args.jev_local_url.as_deref(),
                jev_allow_local_http: self.args.jev_allow_local_http,
                jev_ca_file: self.jev_ca_file.as_ref().map(GivenFile::bundle),
                gateway_ca_file: self.gateway_ca_file.as_ref().map(GivenFile::bundle),
            },
        );
        let engine_program = match (&self.args.runtime_program, &self.runtime_program) {
            (Some(given), Some(file)) => Some(engine::GivenProgram {
                given,
                normalised: &file.path,
                display: &file.display,
            }),
            _ => None,
        };
        let engine = engine::report(&mut context, self.args.runtime, engine_program.as_ref());

        // The only network operation of the tool, and only on request.
        let idp_contacted = identity::reachability(
            &mut context,
            oidc_facts.as_ref(),
            self.args
                .active
                .contains(&report::ActiveCheck::OidcDiscovery),
        )
        .await;

        // A file can only be compared with what was known when it was opened.
        // One that a file read later names as a key file, a credential, a
        // setup record or a store was read before that could be known: the
        // report says so, and never claims that no such content was read.
        let mut named_later = BTreeSet::new();
        let mut read_before_named = fs_facts::NeverOpened::default();
        for (role, path) in &context.opened {
            if context.never_opened.contains(path) {
                named_later.insert(*role);
                read_before_named.add(path);
            }
        }
        for role in &named_later {
            context.report(Draft::new(
                FindingCode::ReferenceFileReadBeforeItWasNamed,
                Scope::File { file: *role },
            ));
        }

        if let Some(target) = published {
            let recorded = registry
                .iter()
                .flat_map(|source| layout::recorded_paths(&source.inspection))
                .chain(oidc_facts.iter().flat_map(|facts| &facts.paths));
            if recorded
                .into_iter()
                .any(|path| !path.starts_with(&target.path))
            {
                context.report(
                    Draft::new(
                        FindingCode::LayoutReferencesOutsideInspectedRoot,
                        Scope::Input { input: target.name },
                    )
                    .target(&target.display),
                );
            }
        }
        if let Some(version) = oidc_facts.as_ref().and_then(|facts| facts.version)
            && let Some(layout) = layouts
                .iter_mut()
                .find(|layout| layout.input == InputName::InstallationDir)
        {
            layout.oidc_file_version = Some(version);
        }
        if self.event_bound.assumed
            && registry_facts
                .as_ref()
                .is_some_and(|facts| facts.uses_max_events)
        {
            self.inputs.insert(
                self.event_bound_slot,
                Input {
                    name: InputName::MaxEvents,
                    origin: InputOrigin::Default,
                    display: self.event_bound.value.to_string(),
                },
            );
        }

        // A bound that was given and that no decoded policy depends on
        // validated nothing: listed among the inputs alone, it would read as
        // used.
        if !self.event_bound.assumed
            && !registry_facts
                .as_ref()
                .is_some_and(|facts| facts.uses_max_events)
        {
            context.report(Draft::new(
                FindingCode::RegistryEventBoundNotUsed,
                Scope::Input {
                    input: InputName::MaxEvents,
                },
            ));
        }

        // Live provider activity fills the tenant audit, which has no rotation
        // yet: fewer than 50 sessions per audit is reported as a sizing risk.
        let (activity_events, activity_bytes) = self.activity_budget;
        if self.event_bound.value / activity_events < 50
            || crate::audit::capacity_policy::PHYSICAL_BYTES / activity_bytes < 50
        {
            context.report(Draft::new(
                FindingCode::AuditLiveActivityBudgetCapacity,
                Scope::Input {
                    input: InputName::MaxEvents,
                },
            ));
        }

        // Every tenant identifier of the run gets its display at once, so
        // that the number of a placeholder is the same wherever it appears.
        let declared: BTreeSet<&str> = registry_facts
            .iter()
            .flat_map(|facts| &facts.ids)
            .map(String::as_str)
            .collect();
        let planned: Option<BTreeSet<&str>> = oidc_facts
            .as_ref()
            .and_then(|facts| facts.plan.as_ref())
            .map(|plan| plan.iter().map(String::as_str).collect());
        let ids = self.safe.ids(
            IdKind::Tenant,
            declared.iter().chain(planned.iter().flatten()).copied(),
        );
        let correspondence = tenants::correspondence(
            &mut context,
            registry_facts.as_ref(),
            oidc_facts.as_ref(),
            historical,
            match published {
                Some(target) => Scope::Input { input: target.name },
                None => Scope::Tenants,
            },
            &ids,
        );

        // An identifier that a file declares twice is one tenant: the totals
        // count what the lists hold. The duplicate has its own finding.
        let registry_decoded = registry_facts.as_ref().filter(|facts| facts.decoded);
        let topology = match registry_decoded {
            Some(_) => Some(Topology {
                tenant_model: TenantModel::Registry,
                tenant_count: count(declared.len()),
            }),
            None if historical => {
                // Stated for the directory the legacy gateway is started
                // with: no registry, one tenant.
                context.report(Draft::new(
                    FindingCode::TopologySingleLocalTenant,
                    Scope::Input {
                        input: InputName::ConfigDir,
                    },
                ));
                Some(Topology {
                    tenant_model: TenantModel::SingleLocal,
                    tenant_count: 1,
                })
            }
            None => None,
        };
        // What no file of an installation can show. Present in every report,
        // so that what was not examined stays visible beside the result.
        let running = Draft::new(FindingCode::TopologyRunningServicesNotChecked, Scope::Host);
        context.report(if idp_contacted {
            // The one thing this run did contact is then named.
            running.variant("after_active_check")
        } else {
            running
        });
        for code in [
            FindingCode::TopologyGatewayArgumentsNotPersisted,
            FindingCode::IdentityLocalNamedAccountsNotAvailable,
            FindingCode::InferenceDecisionEngineSelectionNotPersisted,
            FindingCode::InferenceLocalModelBackendNotAvailable,
            FindingCode::InferenceModelTlsOutputsNotAvailable,
        ] {
            context.report(Draft::new(code, Scope::Host));
        }
        if oidc.is_none() {
            let draft = Draft::new(FindingCode::IdentitySsoNotInspected, Scope::Host);
            context.report(if staging_without_oidc_file {
                draft.variant("staging")
            } else {
                draft
            });
        }
        // What is said of content depends on the whole run: when a file was
        // read before it was named, neither sentence claims the contrary.
        let content_not_checked = |code, scope| {
            let draft = Draft::new(code, scope);
            if named_later.is_empty() {
                draft
            } else {
                draft.variant("after_read")
            }
        };
        if !context.references.is_empty() {
            context.report(content_not_checked(
                FindingCode::ReferenceContentNotChecked,
                Scope::Host,
            ));
        }
        if provider_outcome
            .as_ref()
            .is_some_and(|outcome| outcome.section.file.state == ProviderFileState::Loaded)
        {
            context.report(content_not_checked(
                FindingCode::ProviderCredentialContentNotChecked,
                Scope::ProviderProfiles,
            ));
        }

        // Identifier lists are sorted and bounded; the totals count every
        // distinct identifier, listed or not.
        let mut omitted = Omitted::default();
        let mut listed = Vec::new();
        for set in [registry_decoded.map(|_| &declared), planned.as_ref()] {
            listed.push(set.map(|set| {
                omitted.tenant_ids = omitted
                    .tenant_ids
                    .saturating_add(count(set.len().saturating_sub(report::MAX_LISTED_IDS)));
                set.iter()
                    .take(report::MAX_LISTED_IDS)
                    .map(|id| ids.display(id))
                    .collect::<Vec<_>>()
            }));
        }
        let plan = listed.pop().flatten();
        let registry_ids = listed.pop().flatten();
        let tenants = (registry_ids.is_some() || plan.is_some()).then(|| Tenants {
            registry: registry_ids,
            plan,
            registry_total: registry_decoded.map(|_| count(declared.len())),
            plan_total: planned.as_ref().map(|planned| count(planned.len())),
            correspondence,
        });

        // The technical keys of the tenants the topology describes: those of
        // the registry when one is inspected, else the two keys of the single
        // local tenant. Another inspected directory never changes the count.
        let of_tenants = registry_decoded.is_some();
        let keys: Vec<Presence> = context
            .references
            .iter()
            .filter(|record| {
                matches!(record.role, ReferenceRole::Runner | ReferenceRole::Viewer)
                    && (record.owner.kind == OwnerKind::Tenant) == of_tenants
            })
            .map(|record| record.presence)
            .collect();
        let of = |presence| count(keys.iter().filter(|seen| **seen == presence).count());
        let technical_key_references = (!keys.is_empty()).then(|| ReferenceCounts {
            present: of(Presence::Present),
            absent: of(Presence::Absent),
            unknown: of(Presence::Unknown),
        });

        let (providers, profile_ids) = match provider_outcome {
            Some(outcome) => (Some(outcome.section), outcome.ids),
            None => (None, self.safe.ids(IdKind::ProviderProfile, [])),
        };
        let owner = |raw: &RawOwner| {
            let display = raw.id.as_deref().and_then(|id| match raw.kind {
                OwnerKind::Tenant => Some(ids.display(id)),
                OwnerKind::ProviderProfile => Some(profile_ids.display(id)),
                OwnerKind::Installation => None,
            });
            Owner {
                kind: raw.kind,
                masked: display.as_ref().is_some_and(|display| display.masked),
                id: display.map(|display| display.id),
            }
        };
        // Entries tied to an error, an unknown or a warning first, then by
        // owner and role.
        context.references.sort_by(|left, right| {
            (!left.attention, &left.owner, left.role).cmp(&(
                !right.attention,
                &right.owner,
                right.role,
            ))
        });
        context.data.sort_by(|left, right| {
            (!left.attention, &left.owner, left.role).cmp(&(
                !right.attention,
                &right.owner,
                right.role,
            ))
        });
        // The entry of a file that was read before it was named says so. A
        // store is read when it or one of its sidecars was.
        let content = |path: &Path, store: bool| {
            let read = read_before_named.contains(path)
                || (store
                    && fs_facts::sidecar_paths(path)
                        .iter()
                        .any(|sidecar| read_before_named.contains(sidecar)));
            if read {
                ContentState::ReadBeforeNamed
            } else {
                ContentState::NotRead
            }
        };
        let (credential_references, references_omitted) = bounded(
            context
                .references
                .iter()
                .map(|record| ReferenceEntry {
                    owner: owner(&record.owner),
                    role: record.role,
                    consumer: record.consumer,
                    location: record.location,
                    display: record.display.clone(),
                    presence: record.presence,
                    entry_kind: record.entry_kind,
                    mode: record.mode.clone(),
                    owner_is_inspector: record.owner_is_inspector,
                    single_link: record.single_link,
                    private_rule: record.private_rule,
                    content: content(&record.path, false),
                })
                .collect(),
        );
        let (data, data_omitted) = bounded(
            context
                .data
                .iter()
                .map(|record| DataEntry {
                    owner: owner(&record.owner),
                    role: record.role,
                    location: record.location,
                    display: record.display.clone(),
                    presence: record.presence,
                    sidecars: record.sidecars.clone(),
                    content: content(&record.path, true),
                })
                .collect(),
        );
        omitted.reference_entries = references_omitted;
        omitted.data_entries = data_omitted;

        let mut findings: Vec<Finding> = context
            .drafts
            .into_iter()
            .map(|draft| draft.build(&ids, &profile_ids))
            .collect();
        // After every display was produced: the count of masked values is final.
        if self.safe.masked_values() > 0 {
            findings.push(finding(
                FindingCode::OutputValueMasked,
                Scope::Host,
                None,
                None,
            ));
        }
        let limits = catalogue::CATALOGUE
            .iter()
            .filter(|entry| entry.limit)
            .map(|entry| finding(entry.code, Scope::Build, None, None))
            .collect();
        // The bundle of the OIDC file first, then the bundles given by option.
        let (mut sso, mut trust) = match oidc_facts {
            Some(facts) => (facts.sso, facts.trust),
            None => (None, Vec::new()),
        };
        if let Some(sso) = &mut sso {
            sso.idp_contacted = idp_contacted;
        }
        trust.extend(trust_facts.trust);
        let mut report = ReportV1 {
            schema: report::SCHEMA.to_owned(),
            version: report::VERSION,
            tool: Tool {
                name: report::TOOL_NAME.to_owned(),
                package_version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            mode: Mode {
                network: if idp_contacted {
                    NetworkUse::ActiveChecksOnly
                } else {
                    NetworkUse::None
                },
                mutation: MutationUse::None,
                credential_contents_read: if named_later.is_empty() {
                    CredentialContentUse::None
                } else {
                    CredentialContentUse::DetectedAfterRead
                },
                files_read_for_content: context.read.into_iter().collect(),
                active_checks_executed: if idp_contacted {
                    vec![report::ActiveCheck::OidcDiscovery]
                } else {
                    Vec::new()
                },
            },
            summary: Summary {
                status: SummaryStatus::Ok,
                exit_code: EXIT_OK,
                counts: Counts::default(),
                truncated: false,
                omitted,
            },
            inputs: self.inputs,
            layouts,
            topology,
            identity: Identity {
                technical_key_references,
                sso,
            },
            tenants,
            credential_references,
            providers,
            inference: Inference {
                decision_engine_transport: trust_facts.transport,
            },
            trust,
            engine,
            data,
            findings,
            limits,
        };
        finalise(&mut report);
        report
    }

    /// The selection `run` builds from its options of the same names: an
    /// option that is not given, or the client `auto`, leaves that part to
    /// the automatic choice of the product.
    fn selection_request(&self) -> providers::Request {
        use crate::provider::{AuthPreference, ClientKind, ProviderSelection};
        let args = self.args;
        let client = match args.client {
            None | Some(ClientChoice::Auto) => None,
            Some(ClientChoice::Codex) => Some(ClientKind::Codex),
            Some(ClientChoice::ClaudeCode) => Some(ClientKind::ClaudeCode),
        };
        let auth_mode = args.auth_mode.unwrap_or_default();
        let requested = |explicit| {
            if explicit {
                Requested::Explicit
            } else {
                Requested::Auto
            }
        };
        providers::Request {
            requested: RequestedSelection {
                client: requested(client.is_some()),
                auth_mode: requested(auth_mode != AuthPreference::Auto),
                profile: requested(args.client_profile.is_some()),
                model: requested(args.model.is_some()),
            },
            given: args.client.is_some()
                || args.auth_mode.is_some()
                || args.client_profile.is_some()
                || args.model.is_some(),
            selection: ProviderSelection {
                client,
                auth_mode,
                profile_id: args.client_profile.clone(),
                model: args.model.clone(),
                ..ProviderSelection::default()
            },
        }
    }
}

/// Keep the first entries of a sorted list within the bounds of what is
/// printed. Returns the kept entries and the number left out.
fn bounded<T: Serialize>(entries: Vec<T>) -> (Vec<T>, u64) {
    let total = entries.len();
    let mut bytes = 0_usize;
    let mut kept = Vec::new();
    for entry in entries {
        let size = serde_json::to_vec(&entry).map_or(usize::MAX, |encoded| encoded.len());
        bytes = bytes.saturating_add(size).saturating_add(1);
        if kept.len() == report::MAX_LISTED_ENTRIES || bytes > report::MAX_ENTRY_LIST_BYTES {
            break;
        }
        kept.push(entry);
    }
    let omitted = count(total - kept.len());
    (kept, omitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A registry or an OIDC file directly below `/` cannot be built in a test
    // tree: the rule is exercised here on the root itself.
    #[test]
    fn the_root_of_the_file_system_is_no_display_root() {
        let safe = Safe::new();
        let mut context = Context {
            safe: &safe,
            show_paths: false,
            roots: Vec::new(),
            read: BTreeSet::new(),
            opened: Vec::new(),
            never_opened: fs_facts::NeverOpened::default(),
            references: Vec::new(),
            data: Vec::new(),
            drafts: Vec::new(),
            outside: 0,
        };
        context.add_root(TENANTS_FILE_DIRECTORY_PLACEHOLDER, Path::new("/"));
        let (location, display) =
            context.place(Path::new("/home/operator/keys/runner"), "runner key");
        assert_eq!(location, Location::OutsideRoot);
        assert_eq!(
            display,
            "runner key reference #1 (outside the inspected directory)"
        );

        // A directory that has a name is one, as before.
        context.add_root(
            OIDC_CONFIG_DIRECTORY_PLACEHOLDER,
            Path::new("/srv/ai-guard"),
        );
        let (location, display) =
            context.place(Path::new("/srv/ai-guard/tenant-a/runner"), "runner key");
        assert_eq!(location, Location::InsideRoot);
        assert_eq!(display, "<oidc-config-dir>/tenant-a/runner");
    }
}
