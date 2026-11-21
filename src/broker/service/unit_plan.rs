//! Declarative foreground broker service plans. The service manager and live
//! guardian are outside this read-only operation.
use super::{ServiceEnforcerSelectionV1, profile::Registry};
use crate::control_fs::Directory;
use anyhow::{Context, Result, anyhow, ensure};
use serde::Serialize;
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, clap::ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ServicePlatform {
    Systemd,
    Launchd,
}

impl ServicePlatform {
    fn is_native(self) -> bool {
        match self {
            Self::Systemd => cfg!(target_os = "linux"),
            Self::Launchd => cfg!(target_os = "macos"),
        }
    }
}

#[derive(Serialize)]
struct ControlAccount {
    uid: u32,
    name: String,
}

#[derive(Serialize)]
struct GuardianPreconditions {
    role: String,
    directory: PathBuf,
    requires_independently_supervised_full3: bool,
    full3_presence_and_independence: &'static str,
    new_admissions_after_recovery: &'static str,
    may_spawn_stop_only_recovery_helper: bool,
    stop_helper_lifetime_under_manager: &'static str,
}

#[derive(Serialize)]
struct UnitFile {
    proposed_path: String,
    contents: String,
}

#[derive(Serialize)]
pub struct ServiceUnitPlanV1 {
    version: u32,
    platform: ServicePlatform,
    installation_id: Uuid,
    account: ControlAccount,
    broker_root: PathBuf,
    argv: Vec<String>,
    guardian: GuardianPreconditions,
    installed_state: &'static str,
    diff_baseline: &'static str,
    file: UnitFile,
    diff: String,
}

/// Capture the current installation's inputs and render one unit without
/// reading a service-manager namespace or opening any runner credential.
#[cfg(unix)]
pub fn plan_service_unit(
    platform: ServicePlatform,
    config: &Path,
    role: &str,
) -> Result<ServiceUnitPlanV1> {
    ensure!(
        platform.is_native(),
        "broker service plan platform must match the current executable"
    );
    ServiceEnforcerSelectionV1::Full3Existing {
        role_name: role.to_owned(),
    }
    .validate()?;
    let registry = Registry::load(config)?;
    let root = Directory::open_existing(&registry.data.root)
        .map_err(|_| anyhow!("private broker root is unavailable or unsafe"))?;
    let role_directory = root
        .persistent_child(role, false)
        .map_err(|_| anyhow!("private Full3 role directory is unsafe"))?
        .context("private Full3 role directory is unavailable")?;
    let (uid, name) = crate::runtime::control_service_account()?;
    let executable = current_executable(uid)?;
    let plan = render(
        platform,
        registry.data.installation_id,
        ControlAccount { uid, name },
        &executable.0,
        registry.source.path(),
        root.path(),
        role,
    )?;
    registry
        .source
        .revalidate()
        .and_then(|()| registry.source_directory.revalidate())
        .map_err(|_| anyhow!("private broker configuration changed"))?;
    root.revalidate()
        .and_then(|()| role_directory.revalidate())
        .map_err(|_| anyhow!("private broker root or Full3 role directory changed"))?;
    ensure!(
        current_executable(uid)? == executable,
        "current broker executable changed during planning"
    );
    Ok(plan)
}

#[cfg(not(unix))]
pub fn plan_service_unit(
    _platform: ServicePlatform,
    _config: &Path,
    _role: &str,
) -> Result<ServiceUnitPlanV1> {
    anyhow::bail!("broker service plan requires a supported Unix platform")
}

#[cfg(unix)]
fn current_executable(uid: u32) -> Result<(PathBuf, u64, u64)> {
    use std::{fs, os::unix::fs::MetadataExt};
    let path = std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|_| anyhow!("current broker executable is unavailable"))?;
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| anyhow!("current broker executable is unavailable"))?;
    ensure!(
        metadata.is_file()
            && (metadata.uid() == 0 || metadata.uid() == uid)
            && metadata.mode() & 0o7022 == 0
            && metadata.mode() & 0o111 != 0,
        "current broker executable is unsafe for a service plan"
    );
    Ok((path, metadata.dev(), metadata.ino()))
}

fn plan_text(value: &str) -> Result<&str> {
    ensure!(
        !value.is_empty()
            && !value
                .chars()
                .any(|ch| ch.is_control() || matches!(ch, '\u{fffe}' | '\u{ffff}')),
        "broker service plan value is not representable"
    );
    Ok(value)
}

fn plan_path(path: &Path) -> Result<&str> {
    ensure!(
        path.is_absolute() && path.as_os_str().len() <= 4096,
        "broker service plan paths must be bounded absolute paths"
    );
    plan_text(
        path.to_str()
            .context("broker service plan path is not UTF-8")?,
    )
}

fn render(
    platform: ServicePlatform,
    installation_id: Uuid,
    account: ControlAccount,
    executable: &Path,
    registry: &Path,
    root: &Path,
    role: &str,
) -> Result<ServiceUnitPlanV1> {
    plan_text(&account.name)?;
    plan_path(root)?;
    // A valid role may begin with a hyphen. The equals form keeps Clap
    // from treating such a role as a second option when serve is launched.
    let role_argument = format!("--enforcer-full3-existing={role}");
    let argv = [
        plan_path(executable)?,
        "broker",
        "serve",
        "--config",
        plan_path(registry)?,
        role_argument.as_str(),
    ];
    let unit_name = format!("ai-guard-broker-{installation_id}");
    let file = match platform {
        ServicePlatform::Systemd => UnitFile {
            proposed_path: format!("/etc/systemd/system/{unit_name}.service"),
            contents: systemd_unit(&argv, account.uid),
        },
        ServicePlatform::Launchd => {
            let label = format!("org.ai-guard.broker.{installation_id}");
            UnitFile {
                proposed_path: format!("/Library/LaunchDaemons/{label}.plist"),
                contents: launchd_unit(&argv, &account.name, &label),
            }
        }
    };
    let mut diff = format!(
        "--- /dev/null\n+++ {}\n@@ -0,0 +1,{} @@\n",
        file.proposed_path,
        file.contents.lines().count(),
    );
    for line in file.contents.lines() {
        writeln!(diff, "+{line}").expect("writing to String cannot fail");
    }
    Ok(ServiceUnitPlanV1 {
        version: 1,
        platform,
        installation_id,
        account,
        broker_root: root.to_owned(),
        argv: argv.into_iter().map(str::to_owned).collect(),
        guardian: GuardianPreconditions {
            role: role.to_owned(),
            directory: root.join(role),
            requires_independently_supervised_full3: true,
            full3_presence_and_independence: "unverified",
            new_admissions_after_recovery: "fenced",
            may_spawn_stop_only_recovery_helper: true,
            stop_helper_lifetime_under_manager: "unqualified",
        },
        installed_state: "unknown",
        diff_baseline: "declared_empty",
        file,
        diff,
    })
}

fn systemd_unit(argv: &[&str], uid: u32) -> String {
    // The colon prefix disables environment variable substitution. C-style
    // escapes preserve argument boundaries and %% escapes unit specifiers.
    let exec = argv
        .iter()
        .map(|word| systemd_word(word))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "[Unit]\nDescription=ai-guard broker\n\n[Service]\nType=simple\nUser={uid}\nExecStart=:{exec}\nRestart=on-failure\n"
    )
}

fn systemd_word(word: &str) -> String {
    let mut encoded = String::new();
    for ch in word.chars() {
        match ch {
            ' ' => encoded.push_str(r"\x20"),
            '\\' => encoded.push_str(r"\\"),
            '"' => encoded.push_str(r"\x22"),
            '\'' => encoded.push_str(r"\x27"),
            '%' => encoded.push_str("%%"),
            _ => encoded.push(ch),
        }
    }
    encoded
}

fn launchd_unit(argv: &[&str], user: &str, label: &str) -> String {
    let mut contents = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \t<key>Label</key>\n\t<string>{label}</string>\n\
         \t<key>UserName</key>\n\t<string>{user}</string>\n\
         \t<key>Disabled</key>\n\t<true/>\n\
         \t<key>KeepAlive</key>\n\t<dict>\n\
         \t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>\n\
         \t<key>ProgramArguments</key>\n\t<array>\n",
        user = xml_text(user),
    );
    for argument in argv {
        writeln!(contents, "\t\t<string>{}</string>", xml_text(argument))
            .expect("writing to String cannot fail");
    }
    contents.push_str("\t</array>\n</dict>\n</plist>\n");
    contents
}

fn xml_text(text: &str) -> String {
    let mut encoded = String::new();
    for ch in text.chars() {
        match ch {
            '&' => encoded.push_str("&amp;"),
            '<' => encoded.push_str("&lt;"),
            '>' => encoded.push_str("&gt;"),
            '"' => encoded.push_str("&quot;"),
            '\'' => encoded.push_str("&apos;"),
            _ => encoded.push(ch),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: Uuid = Uuid::from_u128(0x11111111_1111_4111_8111_111111111111);
    const ROLE: &str = "full3-qa";
    const EXECUTABLE: &str = r#"/opt/ai guard/$ai%\"'agentsandbox"#;
    const REGISTRY: &str = "/private/ai guard/&<control>/registry.json";

    fn fixture(platform: ServicePlatform) -> ServiceUnitPlanV1 {
        render(
            platform,
            ID,
            ControlAccount {
                uid: 501,
                name: "operator".into(),
            },
            Path::new(EXECUTABLE),
            Path::new(REGISTRY),
            Path::new("/private/broker-state"),
            ROLE,
        )
        .unwrap()
    }

    fn assert_plan_envelope(plan: &ServiceUnitPlanV1) {
        assert_eq!(plan.version, 1);
        assert_eq!(plan.installed_state, "unknown");
        assert_eq!(plan.diff_baseline, "declared_empty");
        assert_eq!(
            plan.argv,
            [
                EXECUTABLE,
                "broker",
                "serve",
                "--config",
                REGISTRY,
                "--enforcer-full3-existing=full3-qa",
            ]
        );
        assert!(plan.guardian.requires_independently_supervised_full3);
        assert_eq!(plan.guardian.full3_presence_and_independence, "unverified");
        assert_eq!(plan.guardian.new_admissions_after_recovery, "fenced");
        assert!(plan.guardian.may_spawn_stop_only_recovery_helper);
        assert_eq!(
            plan.guardian.stop_helper_lifetime_under_manager,
            "unqualified"
        );
        assert!(plan.diff.starts_with(&format!(
            "--- /dev/null\n+++ {}\n@@ -0,0 +1,{} @@\n",
            plan.file.proposed_path,
            plan.file.contents.lines().count()
        )));
        assert_eq!(
            plan.diff
                .lines()
                .skip(3)
                .map(|line| line.strip_prefix('+').unwrap())
                .collect::<Vec<_>>(),
            plan.file.contents.lines().collect::<Vec<_>>(),
        );
    }

    #[test]
    fn systemd_plan_renders_literal_argv_with_normal_process_group_policy() {
        let plan = fixture(ServicePlatform::Systemd);
        assert_plan_envelope(&plan);
        assert_eq!(
            plan.file.proposed_path,
            format!("/etc/systemd/system/ai-guard-broker-{ID}.service")
        );
        let script = &plan.file.contents;
        assert!(script.contains("\nUser=501\n"));
        assert!(script.lines().any(|line| line == r"ExecStart=:/opt/ai\x20guard/$ai%%\\\x22\x27agentsandbox broker serve --config /private/ai\x20guard/&<control>/registry.json --enforcer-full3-existing=full3-qa"));
        assert!(script.contains("\nRestart=on-failure\n"));
        assert!(!script.contains("[Install]"));
        assert!(!script.contains("KillMode="));
    }

    #[test]
    fn launchd_plan_escapes_xml_and_is_disabled_by_default() {
        let plan = fixture(ServicePlatform::Launchd);
        assert_plan_envelope(&plan);
        assert_eq!(
            plan.file.proposed_path,
            format!("/Library/LaunchDaemons/org.ai-guard.broker.{ID}.plist")
        );
        let plist = &plan.file.contents;
        assert!(plist.contains("<key>UserName</key>\n\t<string>operator</string>"));
        assert!(plist.contains("<key>Disabled</key>\n\t<true/>"));
        assert!(plist.contains("<key>SuccessfulExit</key>\n\t\t<false/>"));
        assert!(plist.contains(concat!(
            "<key>ProgramArguments</key>\n\t<array>\n",
            "\t\t<string>/opt/ai guard/$ai%\\&quot;&apos;agentsandbox</string>\n",
            "\t\t<string>broker</string>\n",
            "\t\t<string>serve</string>\n",
            "\t\t<string>--config</string>\n",
            "\t\t<string>/private/ai guard/&amp;&lt;control&gt;/registry.json</string>\n",
            "\t\t<string>--enforcer-full3-existing=full3-qa</string>\n",
            "\t</array>",
        )));
    }

    #[test]
    fn plan_paths_refuse_unrepresentable_input() {
        for invalid in ["relative", "/private/bad\npath", "/private/bad\u{fffe}"] {
            assert!(plan_path(Path::new(invalid)).is_err());
        }
        assert!(plan_path(Path::new("/private/guard café")).is_ok());
    }
}
