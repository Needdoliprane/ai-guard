//! Text renderer: plain text, no colour, ASCII punctuation, English.
//!
//! Grammar, which keeps the text a fixpoint of the output filter:
//! 1. a dynamic string is always printed between double quotes;
//! 2. a dynamic string is never followed by `:` or `=`; labels are fixed words
//!    followed by two spaces, never by a colon;
//! 3. static text never places a word of the filter directly before `:` or
//!    `=`.
//!
//! The text carries the same findings as the JSON document, in the same
//! order, grouped by axis; findings that are `not_checked` or `not_available`
//! close the report in two blocks, so that what was not examined stays
//! visible next to the result line.

use super::output::quoted;
use super::report::{
    Axis, CredentialContentUse, DataEntry, Details, Finding, IdDisplay, Input, InputName,
    InputOrigin, Layout, NetworkUse, Owner, OwnerKind, ReferenceEntry, ReportV1, Scope, Status,
    SummaryStatus,
};
use serde::Serialize;

const WIDTH: usize = 100;
const BODY_INDENT: usize = 10;

pub(crate) fn render(report: &ReportV1) -> String {
    let mut out = Text::default();
    header(&mut out, report);
    for axis in Axis::ALL {
        let facts = facts(report, axis);
        let findings: Vec<&Finding> = report
            .findings
            .iter()
            .filter(|finding| finding.axis == axis && in_axis_section(finding.status))
            .collect();
        if facts.is_empty() && findings.is_empty() {
            continue;
        }
        out.blank();
        out.line(&axis_title(axis));
        for fact in facts {
            out.line(&fact);
        }
        for finding in findings {
            finding_block(&mut out, finding);
        }
    }
    closing_block(&mut out, report, Status::NotChecked, "NOT CHECKED");
    closing_block(
        &mut out,
        report,
        Status::NotAvailable,
        "NOT AVAILABLE IN THIS BUILD",
    );
    out.line(&format!("exit {}", report.summary.exit_code));
    out.finish()
}

fn in_axis_section(status: Status) -> bool {
    !matches!(status, Status::NotChecked | Status::NotAvailable)
}

#[derive(Default)]
struct Text(String);

impl Text {
    fn line(&mut self, line: &str) {
        self.0.push_str(line);
        self.0.push('\n');
    }

    fn blank(&mut self) {
        self.0.push('\n');
    }

    /// A labelled paragraph of the body of a finding, wrapped with a hanging
    /// indent below its label.
    fn labelled(&mut self, label: &str, text: &str) {
        let first = format!("{}{label}", " ".repeat(BODY_INDENT));
        let hanging = " ".repeat(first.chars().count());
        for (index, line) in wrap(text, WIDTH.saturating_sub(hanging.len()))
            .into_iter()
            .enumerate()
        {
            let prefix = if index == 0 { &first } else { &hanging };
            self.line(&format!("{prefix}{line}"));
        }
    }

    /// A labelled line of the header, wrapped below its label when it is
    /// longer than the width of the report.
    fn header(&mut self, label: &str, text: &str) {
        let hanging = " ".repeat(label.len());
        for (index, line) in wrap(text, WIDTH - label.len()).into_iter().enumerate() {
            let prefix = if index == 0 { label } else { &hanging };
            self.line(&format!("{prefix}{line}"));
        }
    }

    fn finish(self) -> String {
        self.0
    }
}

/// Wrap on spaces. A quoted dynamic string is one word, whatever it contains.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_quotes = false;
    let mut escaped = false;
    for character in text.chars() {
        if character == ' ' && !in_quotes {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }
        if character == '"' && !escaped {
            in_quotes = !in_quotes;
        }
        escaped = character == '\\' && !escaped;
        word.push(character);
    }
    if !word.is_empty() {
        words.push(word);
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in words {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The facts of one entry, below its title: static words, wrapped at the
/// width of the report.
fn body(text: &str) -> Vec<String> {
    let indent = " ".repeat(BODY_INDENT);
    wrap(text, WIDTH - BODY_INDENT)
        .into_iter()
        .map(|line| format!("{indent}{line}"))
        .collect()
}

/// The title of one entry: what it is, then where it is. The display moves
/// below the title when both do not fit the width of the report.
fn entry_title(what: &str, display: &str) -> Vec<String> {
    let title = format!("  {what}");
    let display = quoted(display);
    if title.chars().count() + 2 + display.chars().count() <= WIDTH {
        vec![format!("{title}  {display}")]
    } else {
        vec![title, format!("{}{display}", " ".repeat(BODY_INDENT))]
    }
}

/// The serialised word of a closed enumeration.
fn word<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(word)) => word,
        _ => String::new(),
    }
}

/// The same word, with spaces for reading.
fn words<T: Serialize>(value: &T) -> String {
    word(value).replace('_', " ")
}

fn plural(count: u64, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

fn header(out: &mut Text, report: &ReportV1) {
    out.line(&format!(
        "{} {} - report version {}",
        report.tool.name, report.tool.package_version, report.version
    ));
    // What the tool can know: whether a file it opened is named as a
    // credential by the files it inspected. Nothing more is claimed.
    let credentials = match report.mode.credential_contents_read {
        CredentialContentUse::None => "no file named as a credential was read",
        CredentialContentUse::DetectedAfterRead => "a file later named as a credential was read",
    };
    // The network part is the mode of the report, never a fixed word: a run
    // is offline unless an active check ran, and the checks that ran are
    // then named on a line of their own with the use they made of it.
    match report.mode.network {
        NetworkUse::None => out.header(
            "mode       ",
            &format!("offline; nothing written; {credentials}; no network"),
        ),
        NetworkUse::ActiveChecksOnly => {
            out.header(
                "mode       ",
                &format!("active checks run; nothing written; {credentials}"),
            );
            let active: Vec<String> = report
                .mode
                .active_checks_executed
                .iter()
                .map(words)
                .collect();
            out.header(
                "active     ",
                &format!("{}; network used by these checks only", active.join(", ")),
            );
        }
    }
    let read: Vec<String> = report
        .mode
        .files_read_for_content
        .iter()
        .map(words)
        .collect();
    if read.is_empty() {
        out.line("read       no file content");
    } else {
        out.header("read       ", &read.join(", "));
    }
    for input in &report.inputs {
        out.line(&input_line(input));
    }
    let counts = &report.summary.counts;
    let verdict = match report.summary.status {
        SummaryStatus::Ok => "OK",
        SummaryStatus::Error => "ERROR",
        SummaryStatus::Unknown => "UNKNOWN",
    };
    out.line(&format!(
        "result     {verdict} - {} error, {} unknown, {} warning; {} not checked, {} not available",
        counts.error, counts.unknown, counts.warning, counts.not_checked, counts.not_available
    ));
    // Said whenever something that was evaluated is not listed: a list cut
    // at its bound of printed entries, or every list emptied to respect the
    // size bound of the document.
    let omitted = &report.summary.omitted;
    if report.summary.truncated
        || omitted.reference_entries > 0
        || omitted.data_entries > 0
        || omitted.tenant_ids > 0
    {
        let entries = |count: u64, kind: &str| {
            if count == 1 {
                format!("1 {kind} entry")
            } else {
                format!("{count} {kind} entries")
            }
        };
        out.line(&format!(
            "omitted    {}, {}, {}",
            entries(omitted.reference_entries, "reference"),
            entries(omitted.data_entries, "data"),
            plural(omitted.tenant_ids, "tenant identifier")
        ));
    }
}

fn input_noun(name: InputName) -> &'static str {
    match name {
        InputName::ConfigDir => "config directory",
        InputName::InstallationDir => "installation directory",
        InputName::TenantsFile => "tenant registry",
        InputName::OidcConfig => "OIDC file",
        InputName::ProvidersFile => "provider profiles",
        InputName::Client => "requested client",
        InputName::AuthMode => "requested authentication mode",
        InputName::ClientProfile => "requested profile",
        InputName::Model => "requested model",
        InputName::MaxEvents => "gateway event bound",
        InputName::JevUrl => "decision engine URL",
        InputName::JevLocalUrl => "local decision engine URL",
        InputName::JevAllowLocalHttp => "local HTTP permission for the decision engine",
        InputName::JevCaFile => "decision engine CA bundle",
        InputName::GatewayCaFile => "gateway CA bundle",
        InputName::Runtime => "engine",
        InputName::RuntimeProgram => "engine program",
        InputName::Active => "active check",
    }
}

fn input_line(input: &Input) -> String {
    let inspected = matches!(
        input.name,
        InputName::ConfigDir
            | InputName::InstallationDir
            | InputName::TenantsFile
            | InputName::OidcConfig
            | InputName::ProvidersFile
    );
    let label = match (inspected, input.origin) {
        (true, _) => "inspected",
        (false, InputOrigin::Cli) => "given    ",
        (false, InputOrigin::Default) => "assumed  ",
    };
    let value = match input.name {
        // The name of an active check is a closed word as well.
        InputName::Active => input.display.clone(),
        // Closed values of this tool's own arguments, not operator strings.
        InputName::MaxEvents
        | InputName::Runtime
        | InputName::JevAllowLocalHttp
        | InputName::Client
        | InputName::AuthMode => input.display.clone(),
        _ => quoted(&input.display),
    };
    let origin = match (input.name, input.origin) {
        (_, InputOrigin::Cli) => format!("from {}", input.name.flag()),
        (InputName::ConfigDir, InputOrigin::Default) => "default; no target was given".to_owned(),
        (InputName::MaxEvents, InputOrigin::Default) => {
            "default; the gateway's value is not stored anywhere".to_owned()
        }
        (InputName::ProvidersFile, InputOrigin::Default) => {
            "default; in the config directory".to_owned()
        }
        (_, InputOrigin::Default) => "default".to_owned(),
    };
    format!("{label}  {} {value} ({origin})", input_noun(input.name))
}

fn axis_title(axis: Axis) -> String {
    axis.as_str().replace('_', " ").to_uppercase()
}

fn ids(list: &[IdDisplay]) -> String {
    list.iter()
        .map(|id| quoted(&id.id))
        .collect::<Vec<_>>()
        .join(" ")
}

fn owner(owner: &Owner) -> String {
    let noun = match owner.kind {
        OwnerKind::Installation => "installation",
        OwnerKind::Tenant => "tenant",
        OwnerKind::ProviderProfile => "profile",
    };
    match &owner.id {
        Some(id) => format!("{noun} {}", quoted(id)),
        None => noun.to_owned(),
    }
}

fn optional_text(value: &Option<String>) -> String {
    value
        .as_deref()
        .map_or_else(|| "not declared".to_owned(), quoted)
}

fn facts(report: &ReportV1, axis: Axis) -> Vec<String> {
    match axis {
        Axis::Layout => report
            .layouts
            .iter()
            .flat_map(|layout| layout_fact(report, layout))
            .collect(),
        Axis::Topology => report
            .topology
            .iter()
            .map(|topology| {
                format!(
                    "  tenant model  {}; {}",
                    words(&topology.tenant_model),
                    plural(topology.tenant_count, "tenant")
                )
            })
            .collect(),
        Axis::Identity => identity_facts(report),
        Axis::Tenants => tenants_facts(report),
        Axis::CredentialReferences => report
            .credential_references
            .iter()
            .flat_map(reference_fact)
            .collect(),
        Axis::Providers => providers_facts(report),
        Axis::Inference => report
            .inference
            .decision_engine_transport
            .iter()
            .flat_map(|transport| {
                let given = |given| if given { "given" } else { "not given" };
                let mut lines = vec![format!(
                    "  decision engine transport  {} {}",
                    words(&transport.kind),
                    quoted(&transport.url)
                )];
                lines.extend(body(&format!(
                    "local HTTP permission {}; CA bundle {}",
                    given(transport.allow_local_http),
                    given(transport.ca_bundle_given)
                )));
                lines
            })
            .collect(),
        Axis::Trust => report
            .trust
            .iter()
            .map(|entry| {
                let certificates = entry.certificate_count.map_or_else(
                    || "certificates not counted".to_owned(),
                    |count| plural(count, "certificate"),
                );
                format!(
                    "  {}  {}  {}; {certificates}",
                    words(&entry.role),
                    quoted(&entry.display),
                    if entry.read { "read" } else { "not read" }
                )
            })
            .collect(),
        Axis::Engine => report
            .engine
            .iter()
            .flat_map(|engine| {
                let mut lines = vec![format!(
                    "  engine program  {} {}; approval policy version {}",
                    word(&engine.kind),
                    quoted(&engine.entry_display),
                    engine.policy_version
                )];
                lines.extend(body(&format!("sha256 {}", quoted(&engine.sha256))));
                lines
            })
            .collect(),
        Axis::Data => report.data.iter().flat_map(data_fact).collect(),
        Axis::Output => Vec::new(),
    }
}

fn layout_fact(report: &ReportV1, layout: &Layout) -> Vec<String> {
    let display = report
        .inputs
        .iter()
        .find(|input| input.name == layout.input)
        .map_or_else(String::new, |input| format!(" {}", quoted(&input.display)));
    let mut lines = vec![format!(
        "  {}{display}  {}",
        input_noun(layout.input),
        words(&layout.kind)
    )];
    let mut state = Vec::new();
    if let Some(preparation) = layout.preparation {
        state.push(format!("preparation {}", words(&preparation)));
    }
    if let Some(version) = layout.oidc_file_version {
        state.push(format!("OIDC file version {version}"));
    }
    if !state.is_empty() {
        lines.extend(body(&state.join("; ")));
    }
    lines
}

fn identity_facts(report: &ReportV1) -> Vec<String> {
    let mut facts = Vec::new();
    if let Some(counts) = report.identity.technical_key_references {
        facts.push(format!(
            "  technical key references  {} present, {} absent, {} unknown",
            counts.present, counts.absent, counts.unknown
        ));
    }
    let Some(sso) = &report.identity.sso else {
        return facts;
    };
    if let Some(version) = sso.file_version {
        facts.push(format!("  OIDC file version  {version}"));
    }
    for (label, value) in [
        ("issuer", &sso.issuer),
        ("provider label", &sso.provider_label),
        ("web origin", &sso.web_origin),
        ("web client", &sso.web_client_id),
        ("CLI client", &sso.cli_client_id),
        ("API audience", &sso.api_audience),
        ("authorization endpoint", &sso.authorization_endpoint),
        ("token endpoint", &sso.token_endpoint),
        ("keys endpoint", &sso.jwks_uri),
        ("end session endpoint", &sso.end_session_endpoint),
        ("web redirect", &sso.web_redirect_uri),
        ("CLI callback path", &sso.cli_callback_path),
    ] {
        facts.push(format!("  {label}  {}", optional_text(value)));
    }
    if let Some(transport) = sso.transport {
        facts.push(format!("  transport  {}", word(&transport)));
    }
    if let Some(mapping) = &sso.mapping {
        facts.push(format!(
            "  claim mapping  revision {}; {}",
            quoted(&mapping.revision),
            plural(mapping.rule_count, "rule")
        ));
    }
    facts.push(format!(
        "  CA bundle  {}",
        if sso.ca_bundle_declared {
            "declared"
        } else {
            "not declared"
        }
    ));
    facts.push(format!(
        "  identity provider  {}",
        if sso.idp_contacted {
            "contacted"
        } else {
            "not contacted"
        }
    ));
    facts
}

fn tenants_facts(report: &ReportV1) -> Vec<String> {
    let Some(tenants) = &report.tenants else {
        return Vec::new();
    };
    let mut facts = Vec::new();
    for (label, total, list) in [
        ("registry", tenants.registry_total, &tenants.registry),
        ("capacity plan", tenants.plan_total, &tenants.plan),
    ] {
        let Some(total) = total else {
            continue;
        };
        let mut fact = format!("  {label}  {}", plural(total, "tenant"));
        if let Some(list) = list {
            if !list.is_empty() {
                fact.push_str(&format!("  {}", ids(list)));
            }
            let shown = u64::try_from(list.len()).unwrap_or(u64::MAX);
            if total > shown {
                fact.push_str(&format!("  and {} not listed", total - shown));
            }
        }
        facts.push(fact);
    }
    if let Some(correspondence) = tenants.correspondence {
        facts.push(format!("  correspondence  {}", word(&correspondence)));
    }
    facts
}

fn reference_fact(entry: &ReferenceEntry) -> Vec<String> {
    let mut posture = vec![words(&entry.presence)];
    if let Some(kind) = entry.entry_kind {
        posture.push(words(&kind));
    }
    if let Some(mode) = &entry.mode {
        posture.push(format!("mode {mode}"));
    }
    match entry.owner_is_inspector {
        Some(true) => posture.push("owned by the inspecting account".to_owned()),
        Some(false) => posture.push("owned by another account".to_owned()),
        None => {}
    }
    match entry.single_link {
        Some(true) => posture.push("single link".to_owned()),
        Some(false) => posture.push("several links".to_owned()),
        None => {}
    }
    let mut lines = entry_title(
        &format!("{} of {}", words(&entry.role), owner(&entry.owner)),
        &entry.display,
    );
    lines.extend(body(&format!(
        "{}; private rule {}; reader {}; content {}",
        posture.join("; "),
        words(&entry.private_rule),
        words(&entry.consumer),
        words(&entry.content)
    )));
    lines
}

fn providers_facts(report: &ReportV1) -> Vec<String> {
    let Some(providers) = &report.providers else {
        return Vec::new();
    };
    let mut facts = vec![format!(
        "  profile file  {}  {}",
        quoted(&providers.file.display),
        words(&providers.file.state)
    )];
    if let Some(default) = &providers.default_profile {
        facts.push(format!("  default profile  {}", quoted(&default.id)));
    }
    if !providers.auto_priority.is_empty() {
        facts.push(format!(
            "  automatic priority  {}",
            ids(&providers.auto_priority)
        ));
    }
    for profile in &providers.profiles {
        let mut fact = format!(
            "revision {}; client {}; mode {}; model {}; source {}",
            profile.revision,
            word(&profile.client),
            word(&profile.auth_mode),
            optional_text(&profile.model),
            word(&profile.source_kind)
        );
        if let Some(variable) = &profile.variable_name {
            fact.push_str(&format!("; variable name {}", quoted(variable)));
        }
        facts.push(format!("  profile {}", quoted(&profile.profile.id)));
        facts.extend(body(&fact));
    }
    if let Some(selection) = &providers.selection {
        let requested = &selection.requested;
        facts.push("  selection of a managed launch".to_owned());
        facts.extend(body(&format!(
            "requested client {}; mode {}; profile {}; model {}",
            word(&requested.client),
            word(&requested.auth_mode),
            word(&requested.profile),
            word(&requested.model)
        )));
        facts.extend(body(&match &selection.resolved {
            Some(resolved) => format!(
                "resolved profile {}; revision {}; client {}; mode {}; model {}; chosen by {}",
                quoted(&resolved.profile.id),
                resolved.revision,
                word(&resolved.client),
                word(&resolved.auth_mode),
                optional_text(&resolved.model),
                words(&resolved.chosen_by)
            ),
            None => "resolved none; the product refuses this selection".to_owned(),
        }));
    }
    facts
}

fn data_fact(entry: &DataEntry) -> Vec<String> {
    let sidecars = if entry.sidecars.is_empty() {
        "no sidecar".to_owned()
    } else {
        format!(
            "sidecars {}",
            entry
                .sidecars
                .iter()
                .map(word)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut lines = entry_title(
        &format!("{} of {}", words(&entry.role), owner(&entry.owner)),
        &entry.display,
    );
    lines.extend(body(&format!(
        "{}; {sidecars}; content {}",
        words(&entry.presence),
        words(&entry.content)
    )));
    lines
}

fn finding_block(out: &mut Text, finding: &Finding) {
    out.line(&format!(
        "  [{}] {}",
        finding.status.as_str(),
        finding.code.as_str()
    ));
    out.labelled("", &finding.message);
    if let Some(details) = &finding.details {
        for (label, text) in details_lines(details) {
            out.labelled(&format!("{label}  "), &text);
        }
    }
    if let Some(subjects) = &finding.subjects {
        let mut text = format!("{} in total", subjects.total);
        if !subjects.ids.is_empty() {
            text.push_str(&format!("  {}", ids(&subjects.ids)));
        }
        if subjects.omitted > 0 {
            text.push_str(&format!("  and {} not listed", subjects.omitted));
        }
        out.labelled(&format!("{}  ", words(&subjects.kind)), &text);
    }
    out.labelled("scope    ", &scope_text(&finding.scope));
    out.labelled("basis    ", &basis_text(finding));
    if let Some(consumer) = &finding.origin.consumer {
        out.labelled("applies  ", consumer);
    }
    if !finding.origin.assumes.is_empty() {
        let assumed: Vec<&str> = finding
            .origin
            .assumes
            .iter()
            .map(|name| name.as_str())
            .collect();
        out.labelled("assumes  ", &assumed.join(", "));
    }
    if let Some(requires) = &finding.requires {
        out.labelled("requires  ", requires);
    }
    out.labelled("action   ", &finding.action);
}

fn scope_text(scope: &Scope) -> String {
    match scope {
        Scope::Input { input } => format!("input {}", input.as_str()),
        Scope::Tenants => "tenants".to_owned(),
        Scope::ProviderProfiles => "provider profiles".to_owned(),
        Scope::File { file } => format!("file {}", word(file)),
        Scope::Host => "host".to_owned(),
        Scope::Build => "build".to_owned(),
    }
}

fn basis_text(finding: &Finding) -> String {
    let sources: Vec<String> = finding
        .origin
        .sources
        .iter()
        .map(|source| match &source.field {
            Some(field) => format!("{} ({field})", source.file),
            None => source.file.clone(),
        })
        .collect();
    if sources.is_empty() {
        finding.basis.as_str().to_owned()
    } else {
        format!("{} from {}", finding.basis.as_str(), sources.join(" and "))
    }
}

fn details_lines(details: &Details) -> Vec<(&'static str, String)> {
    match details {
        Details::TenantSetDifference {
            only_in_registry,
            only_in_plan,
            omitted,
        } => {
            let mut lines = Vec::new();
            if !only_in_registry.is_empty() {
                lines.push(("only in the registry", ids(only_in_registry)));
            }
            if !only_in_plan.is_empty() {
                lines.push(("only in the plan", ids(only_in_plan)));
            }
            if *omitted > 0 {
                lines.push(("not listed", plural(*omitted, "tenant")));
            }
            lines
        }
        Details::JsonError {
            category,
            line,
            column,
        } => vec![(
            "position",
            format!("line {line}, column {column}; category {}", word(category)),
        )],
        Details::Reason { reason } => vec![("reason", word(reason))],
        Details::Policy { policy } => vec![("policy", word(policy))],
        Details::WouldRefuseWith { class } => vec![("a launch would refuse with", word(class))],
        Details::Sidecars { sidecars } => vec![(
            "sidecars",
            sidecars.iter().map(word).collect::<Vec<_>>().join(", "),
        )],
        Details::EngineProgram { explicit_program } => vec![(
            "program",
            if *explicit_program {
                "given explicitly".to_owned()
            } else {
                "looked up at the fixed system locations".to_owned()
            },
        )],
    }
}

fn closing_block(out: &mut Text, report: &ReportV1, status: Status, title: &str) {
    let findings: Vec<&Finding> = report
        .findings
        .iter()
        .chain(&report.limits)
        .filter(|finding| finding.status == status)
        .collect();
    if findings.is_empty() {
        return;
    }
    out.blank();
    out.line(title);
    for finding in findings {
        finding_block(out, finding);
    }
    out.blank();
}
