//! Static component build identity, without configuration or runtime discovery.
use serde::{Deserialize, Serialize, de};
use std::{ffi::OsString, fmt, io::Write};

pub const MAX_STATUS_BYTES: usize = 8_192;

// AI_GUARD_VERSION is a builder consistency assertion, never a version source.
const _: () = assert!(
    version_matches(option_env!("AI_GUARD_VERSION"), env!("CARGO_PKG_VERSION")),
    "AI_GUARD_VERSION must match Cargo package version"
);

const fn version_matches(supplied: Option<&str>, package: &str) -> bool {
    let Some(supplied) = supplied else {
        return true;
    };
    // Unset Docker ARG values arrive as empty; they supply no assertion.
    if supplied.is_empty() {
        return true;
    }
    let supplied = supplied.as_bytes();
    let package = package.as_bytes();
    if supplied.len() != package.len() {
        return false;
    }
    let mut index = 0;
    while index < supplied.len() {
        if supplied[index] != package[index] {
            return false;
        }
        index += 1;
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Component {
    #[serde(rename = "gateway")]
    Gateway,
    #[serde(rename = "dashboard")]
    Dashboard,
    #[serde(rename = "agentsandbox")]
    Agentsandbox,
    #[serde(rename = "ai-guard-diagnose")]
    Diagnose,
    #[serde(rename = "ai-guard-provider")]
    Provider,
    #[serde(rename = "ai-guard-demo-pki")]
    DemoPki,
    #[serde(rename = "ai-guard-relay")]
    Relay,
    #[serde(rename = "ai-guard-audit-checkpoint")]
    AuditCheckpoint,
}

impl Component {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gateway => "gateway",
            Self::Dashboard => "dashboard",
            Self::Agentsandbox => "agentsandbox",
            Self::Diagnose => "ai-guard-diagnose",
            Self::Provider => "ai-guard-provider",
            Self::DemoPki => "ai-guard-demo-pki",
            Self::Relay => "ai-guard-relay",
            Self::AuditCheckpoint => "ai-guard-audit-checkpoint",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildIdentityError {
    InvalidStatus,
    InvalidArguments,
    Encoding,
}

impl fmt::Display for BuildIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidStatus => "build_identity_invalid_status",
            Self::InvalidArguments => "build_identity_invalid_arguments",
            Self::Encoding => "build_identity_encoding_failed",
        })
    }
}

impl std::error::Error for BuildIdentityError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct BuildMetadata {
    id: Option<String>,
    revision: Option<String>,
}

impl BuildMetadata {
    fn from_candidates(id: Option<&str>, revision: Option<&str>) -> Self {
        Self {
            id: id.filter(|value| valid_build_id(value)).map(str::to_owned),
            revision: revision
                .filter(|value| valid_revision(value))
                .map(str::to_owned),
        }
    }
}

fn valid_build_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-')
        })
}

fn valid_revision(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BuildStatusV1 {
    schema_version: u32,
    product: String,
    version: String,
    component: Component,
    build: BuildMetadata,
}

// This wrapper deliberately uses deserialize_any: a missing field is an error,
// whereas an explicit JSON null calls visit_unit. Option<String> alone would
// silently treat a missing build metadata field as null.
struct RequiredNullableString(Option<String>);

impl<'de> Deserialize<'de> for RequiredNullableString {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NullableStringVisitor;
        impl<'de> de::Visitor<'de> for NullableStringVisitor {
            type Value = RequiredNullableString;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an explicit string or null")
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(RequiredNullableString(None))
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(RequiredNullableString(Some(value.to_owned())))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(RequiredNullableString(Some(value)))
            }
        }
        deserializer.deserialize_any(NullableStringVisitor)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBuildMetadata {
    id: RequiredNullableString,
    revision: RequiredNullableString,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBuildStatus {
    schema_version: u32,
    product: String,
    version: String,
    component: Component,
    build: RawBuildMetadata,
}

impl BuildStatusV1 {
    /// Describe this compiled component; absent build facts remain null.
    pub fn current(component: Component) -> Self {
        Self {
            schema_version: 1,
            product: "ai-guard".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            component,
            build: BuildMetadata::from_candidates(
                option_env!("AI_GUARD_BUILD_ID"),
                option_env!("AI_GUARD_BUILD_REVISION"),
            ),
        }
    }

    /// Decode bounded public identity, preserving a valid different version.
    pub fn from_json(bytes: &[u8], expected: Component) -> Result<Self, BuildIdentityError> {
        if bytes.is_empty() || bytes.len() > MAX_STATUS_BYTES {
            return Err(BuildIdentityError::InvalidStatus);
        }
        // Derived serde structs also accept sequences. Require the wire objects
        // first, then decode the original bytes to retain duplicate-key checks.
        let shape: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| BuildIdentityError::InvalidStatus)?;
        if !shape.is_object()
            || !shape.get("build").is_some_and(serde_json::Value::is_object)
            || !shape.get("component").is_some_and(serde_json::Value::is_string)
        {
            return Err(BuildIdentityError::InvalidStatus);
        }
        let raw: RawBuildStatus =
            serde_json::from_slice(bytes).map_err(|_| BuildIdentityError::InvalidStatus)?;
        if raw.schema_version != 1
            || raw.product != "ai-guard"
            || raw.component != expected
            || !valid_version(&raw.version)
            || raw.build.id.0.as_deref().is_some_and(|value| !valid_build_id(value))
            || raw
                .build
                .revision
                .0
                .as_deref()
                .is_some_and(|value| !valid_revision(value))
        {
            return Err(BuildIdentityError::InvalidStatus);
        }
        Ok(Self {
            schema_version: raw.schema_version,
            product: raw.product,
            version: raw.version,
            component: raw.component,
            build: BuildMetadata {
                id: raw.build.id.0,
                revision: raw.build.revision.0,
            },
        })
    }
}

fn numeric_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value.len() == 1 || !value.starts_with('0'))
}

fn identifiers(value: &str, prerelease: bool) -> bool {
    value.split('.').all(|identifier| {
        !identifier.is_empty()
            && identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && (!prerelease
                || !identifier.bytes().all(|byte| byte.is_ascii_digit())
                || numeric_identifier(identifier))
    })
}

fn valid_version(value: &str) -> bool {
    if value.is_empty() || value.len() > 64 || !value.is_ascii() {
        return false;
    }
    let release = match value.split_once('+') {
        Some((release, build)) if identifiers(build, false) => release,
        Some(_) => return false,
        None => value,
    };
    let core = match release.split_once('-') {
        Some((core, prerelease)) if identifiers(prerelease, true) => core,
        Some(_) => return false,
        None => release,
    };
    let mut numbers = core.split('.');
    matches!(
        (numbers.next(), numbers.next(), numbers.next(), numbers.next()),
        (Some(major), Some(minor), Some(patch), None)
            if numeric_identifier(major) && numeric_identifier(minor) && numeric_identifier(patch)
    )
}

fn text_identity(component: Component) -> String {
    let status = BuildStatusV1::current(component);
    format!(
        "{} {}\nbuild: {}\nrevision: {}\n",
        component.as_str(),
        status.version,
        status.build.id.as_deref().unwrap_or("unknown"),
        status.build.revision.as_deref().unwrap_or("unknown"),
    )
}

/// Recognize static version requests before configuration or provider startup.
/// Arguments exclude argv[0]. Unrelated commands remain the caller's concern.
pub fn early_output(
    component: Component,
    args: &[OsString],
) -> Option<Result<String, BuildIdentityError>> {
    let exact_flag = |argument: &OsString| argument == "--version" || argument == "-V";
    let has_flag = args.iter().take_while(|argument| *argument != "--").any(|argument| {
        exact_flag(argument)
            || argument.as_encoded_bytes().starts_with(b"--version")
            || argument.as_encoded_bytes().starts_with(b"-V")
    });
    if has_flag {
        return Some(if args.len() == 1 && exact_flag(&args[0]) {
            Ok(text_identity(component))
        } else {
            Err(BuildIdentityError::InvalidArguments)
        });
    }
    if component != Component::Agentsandbox || !args.first().is_some_and(|arg| arg == "version") {
        return None;
    }
    Some(match args {
        [_] => Ok(text_identity(component)),
        [_, format] if format == "--json" => serde_json::to_string(&BuildStatusV1::current(component))
            .map(|mut value| {
                value.push('\n');
                value
            })
            .map_err(|_| BuildIdentityError::Encoding),
        _ => Err(BuildIdentityError::InvalidArguments),
    })
}

fn emit_early_result(
    result: Result<String, BuildIdentityError>,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> i32 {
    match result {
        Ok(output) => {
            if stdout.write_all(output.as_bytes()).and_then(|()| stdout.flush()).is_ok() {
                0
            } else {
                74
            }
        }
        Err(error) => {
            let message = format!("{error}\n");
            if stderr.write_all(message.as_bytes()).and_then(|()| stderr.flush()).is_ok() {
                2
            } else {
                74
            }
        }
    }
}

/// Return the process exit code for a static version request, if present.
/// The caller decides when to exit; no runtime is entered here.
pub fn early_exit(component: Component) -> Option<i32> {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    let result = early_output(component, &arguments)?;
    Some(emit_early_result(
        result,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    ))
}

#[cfg(test)]
mod tests;
