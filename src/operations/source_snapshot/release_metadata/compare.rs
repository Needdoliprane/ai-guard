//! Outcome classes of `release verify` other than a match.
//!
//! An [`Unverified`] outcome means this CLI cannot judge the manifest (an
//! input unreadable under its policy, a source that cannot be prepared, a
//! file that is not an ai-guard release manifest, another format version or
//! another producer), or did not judge it (options that cannot be combined,
//! a stop request, a step that ended without a result). It is never reported
//! as a divergence nor as a match.
//!
//! A [`Divergence`] means this CLI judged a format-1 manifest of its own
//! producer and found a difference, or the directory is not a well-formed
//! format-1 publication (tampering or corruption).
//!
//! The producer version is the package version of every build of this
//! package, so the producer check does not identify the CLI build: a change
//! of a derivation rule without a new `format_version` would make another
//! build of the same version report a divergence.
use super::super::capture::shown;
use super::document::{
    self, CargoLockV1, EntryV1, FORMAT, FORMAT_VERSION, NpmLockV1, ProducerV1, ReleaseManifestV1,
    SourceV1,
};
use crate::runtime::retained_cli::signal_name;
use serde_json::Value;
use std::{collections::BTreeMap, fmt, path::Path};

/// Names listed per kind in a source divergence.
const MAX_LISTED: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Unverified {
    Unreadable {
        input: &'static str,
        reason: String,
    },
    /// Options that cannot be judged together; nothing was read.
    Conflict {
        reason: &'static str,
    },
    /// A stop request ended the comparison before a result. `reason` is
    /// what the interrupted step reported, when that was not the stop itself.
    Interrupted {
        signal: i32,
        reason: Option<String>,
    },
    /// A step of this CLI ended without a result.
    Incomplete {
        reason: String,
    },
    NotAManifest,
    UnsupportedFormatVersion {
        found: u64,
    },
    OtherProducer {
        manifest: Box<ProducerV1>,
        cli: Box<ProducerV1>,
    },
}

/// The entries that explain a source divergence, at most [`MAX_LISTED`] names
/// per kind, and the first differing field of `source`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SourceDifference {
    pub(super) manifest: String,
    pub(super) prepared: String,
    pub(super) added: Vec<String>,
    pub(super) removed: Vec<String>,
    pub(super) changed: Vec<String>,
    /// Named when no entry is listed, so that the divergence is never
    /// reported without a cause.
    pub(super) field: Option<&'static str>,
}

impl fmt::Display for Unverified {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { input, reason } => write!(
                formatter,
                "UNVERIFIED: {input} could not be read under its policy: {}",
                escaped(reason)
            ),
            Self::Conflict { reason } => {
                write!(formatter, "UNVERIFIED: {reason}; nothing was compared")
            }
            Self::Interrupted { signal, reason } => {
                write!(
                    formatter,
                    "UNVERIFIED: interrupted by {} before a result; nothing was judged and \
                     nothing was published",
                    signal_name(*signal)
                )?;
                match reason {
                    Some(reason) => write!(
                        formatter,
                        "; the interrupted step reported: {}",
                        escaped(reason)
                    ),
                    None => Ok(()),
                }
            }
            Self::Incomplete { reason } => write!(
                formatter,
                "UNVERIFIED: {}; this CLI judged nothing",
                escaped(reason)
            ),
            Self::NotAManifest => formatter.write_str(
                "UNVERIFIED: not an ai-guard release manifest; this CLI judges nothing in it",
            ),
            Self::UnsupportedFormatVersion { found } => write!(
                formatter,
                "UNVERIFIED: unsupported format_version {found}; this CLI judges format_version \
                 {FORMAT_VERSION} only"
            ),
            Self::OtherProducer { manifest, cli } => write!(
                formatter,
                "UNVERIFIED: produced by {} {} (allowlist {}); this CLI is {} {} (allowlist {}). \
                 Verify with the CLI built from the distribution the manifest describes",
                escaped(&manifest.name),
                escaped(&manifest.version),
                escaped(&manifest.allowlist_sha256),
                cli.name,
                cli.version,
                cli.allowlist_sha256
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Divergence {
    /// A `SHA256SUMS` line does not match the bytes read.
    Sums {
        file: &'static str,
    },
    /// `SHA256SUMS` grammar, unknown field, non-canonical bytes or an
    /// inadmissible declared value in a format-1 file.
    Format {
        detail: String,
    },
    Source(Box<SourceDifference>),
    Lock {
        file: &'static str,
        field: &'static str,
    },
    Declared {
        field: &'static str,
    },
    Identity,
    Inventory,
    ArtifactChanged {
        name: String,
    },
    ArtifactMissing {
        name: String,
    },
    ArtifactUnlisted {
        name: String,
    },
}

impl Divergence {
    pub(super) fn category(&self) -> &'static str {
        match self {
            Self::Sums { .. } => "sums",
            Self::Format { .. } => "format",
            Self::Source(_) => "source",
            Self::Lock { .. } => "lock",
            Self::Declared { .. } => "declared",
            Self::Identity => "inputs-identity",
            Self::Inventory => "inventory",
            Self::ArtifactChanged { .. } => "artifact-changed",
            Self::ArtifactMissing { .. } => "artifact-missing",
            Self::ArtifactUnlisted { .. } => "artifact-unlisted",
        }
    }
}

/// Text that came from a file under verification, escaped and bounded.
fn escaped(text: &str) -> String {
    shown(Path::new(text))
}

fn shown_names(names: &[String]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names
            .iter()
            .map(|name| escaped(name))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for Divergence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "DIVERGENT {}: ", self.category())?;
        match self {
            Self::Sums { file } => write!(
                formatter,
                "{file} does not match its line in {}",
                document::SUMS_FILE
            ),
            Self::Format { detail } => write!(
                formatter,
                "the manifest directory is not a well-formed format-1 publication: {}",
                escaped(detail)
            ),
            Self::Source(difference) => {
                write!(
                    formatter,
                    "the manifest binds source digest {}, the prepared source has {}; changed: \
                     {}; added: {}; removed: {} (at most {MAX_LISTED} names each)",
                    escaped(&difference.manifest),
                    difference.prepared,
                    shown_names(&difference.changed),
                    shown_names(&difference.added),
                    shown_names(&difference.removed)
                )?;
                let none_listed = difference.changed.is_empty()
                    && difference.added.is_empty()
                    && difference.removed.is_empty();
                match difference.field {
                    Some(field) if none_listed => {
                        write!(formatter, "; no entry differs, source.{field} differs")
                    }
                    _ => Ok(()),
                }
            }
            Self::Lock { file, field } => write!(
                formatter,
                "{file} {field} in the manifest differs from the lock of the prepared source"
            ),
            Self::Declared { field } => write!(
                formatter,
                "{field} in the manifest differs from what the prepared source declares"
            ),
            Self::Identity => formatter.write_str(
                "build.inputs_identity differs from the build-input identity of the prepared \
                 source and the declared provider versions",
            ),
            Self::Inventory => formatter.write_str(
                "sbom.cdx.json or its record in the manifest differs from the inventory derived \
                 from the prepared source",
            ),
            Self::ArtifactChanged { name } => write!(
                formatter,
                "{} differs from the length or SHA-256 the manifest lists",
                escaped(name)
            ),
            Self::ArtifactMissing { name } => write!(
                formatter,
                "{} is listed in the manifest but was not presented with --artifact",
                escaped(name)
            ),
            Self::ArtifactUnlisted { name } => write!(
                formatter,
                "{} was presented with --artifact but is not listed in the manifest",
                escaped(name)
            ),
        }
    }
}

fn producer_of(value: &Value) -> Option<ProducerV1> {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(ProducerV1 {
        name: text("name")?,
        version: text("version")?,
        allowlist_sha256: text("allowlist_sha256")?,
    })
}

/// Lenient reading of `format`, `format_version` and `producer`, other fields
/// ignored. Decides every [`Unverified`] class about the manifest itself
/// before any strict parse. A malformed producer of a format-1 file is left to
/// the strict parse, which reports it as a format divergence.
pub(super) fn probe(bytes: &[u8]) -> Result<(), Unverified> {
    let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(bytes) else {
        return Err(Unverified::NotAManifest);
    };
    if object.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(Unverified::NotAManifest);
    }
    match object.get("format_version").and_then(Value::as_u64) {
        Some(found) if found == u64::from(FORMAT_VERSION) => {}
        Some(found) => return Err(Unverified::UnsupportedFormatVersion { found }),
        None => return Err(Unverified::NotAManifest),
    }
    if let Some(manifest) = object.get("producer").and_then(producer_of) {
        let cli = document::producer();
        if manifest != cli {
            return Err(Unverified::OtherProducer {
                manifest: Box::new(manifest),
                cli: Box::new(cli),
            });
        }
    }
    Ok(())
}

/// The source divergence between the source a manifest binds (`given`) and
/// the prepared one, with at most [`MAX_LISTED`] names per kind.
pub(super) fn source_divergence(given: &SourceV1, prepared: &SourceV1) -> Divergence {
    let index = |source: &SourceV1| -> BTreeMap<String, (u64, String)> {
        source
            .entries
            .iter()
            .map(|entry: &EntryV1| (entry.path.clone(), (entry.bytes, entry.sha256.clone())))
            .collect()
    };
    let listed = index(given);
    let current = index(prepared);
    let added: Vec<String> = current
        .keys()
        .filter(|path| !listed.contains_key(*path))
        .take(MAX_LISTED)
        .cloned()
        .collect();
    let removed: Vec<String> = listed
        .keys()
        .filter(|path| !current.contains_key(*path))
        .take(MAX_LISTED)
        .cloned()
        .collect();
    let changed: Vec<String> = current
        .iter()
        .filter(|(path, facts)| listed.get(*path).is_some_and(|known| known != *facts))
        .map(|(path, _)| path.clone())
        .take(MAX_LISTED)
        .collect();
    Divergence::Source(Box::new(SourceDifference {
        manifest: given.digest.value.clone(),
        prepared: prepared.digest.value.clone(),
        added,
        removed,
        changed,
        field: source_field(prepared, given),
    }))
}

/// The first field of `source` that differs, in the order of [`SourceV1`].
fn source_field(expected: &SourceV1, given: &SourceV1) -> Option<&'static str> {
    first_field([
        ("scope", expected.scope != given.scope),
        ("digest.alg", expected.digest.alg != given.digest.alg),
        (
            "digest.layout",
            expected.digest.layout != given.digest.layout,
        ),
        ("digest.value", expected.digest.value != given.digest.value),
        ("files", expected.files != given.files),
        ("bytes", expected.bytes != given.bytes),
        ("entries", expected.entries != given.entries),
    ])
}

fn first_field<const N: usize>(fields: [(&'static str, bool); N]) -> Option<&'static str> {
    fields
        .into_iter()
        .find(|(_, differs)| *differs)
        .map(|(field, _)| field)
}

fn cargo_field(expected: &CargoLockV1, given: &CargoLockV1) -> Option<&'static str> {
    first_field([
        ("path", expected.path != given.path),
        ("bytes", expected.bytes != given.bytes),
        ("sha256", expected.sha256 != given.sha256),
        ("lock_version", expected.lock_version != given.lock_version),
        ("packages", expected.packages != given.packages),
        (
            "registry_packages",
            expected.registry_packages != given.registry_packages,
        ),
        (
            "path_packages",
            expected.path_packages != given.path_packages,
        ),
    ])
}

fn npm_field(expected: &NpmLockV1, given: &NpmLockV1) -> Option<&'static str> {
    first_field([
        ("path", expected.path != given.path),
        ("bytes", expected.bytes != given.bytes),
        ("sha256", expected.sha256 != given.sha256),
        (
            "lockfile_version",
            expected.lockfile_version != given.lockfile_version,
        ),
        ("packages", expected.packages != given.packages),
        ("dev", expected.dev != given.dev),
        ("optional", expected.optional != given.optional),
        (
            "install_scripts",
            expected.install_scripts != given.install_scripts,
        ),
    ])
}

/// The first difference between the manifest this CLI derives (`expected`)
/// and the one it was given, in the order: source, locks, declared facts,
/// build-input identity, inventory, artefacts. `None` only if the two are
/// equal.
pub(super) fn first_divergence(
    expected: &ReleaseManifestV1,
    given: &ReleaseManifestV1,
    expected_sbom: &[u8],
    given_sbom: &[u8],
) -> Option<Divergence> {
    if expected.source != given.source {
        return Some(source_divergence(&given.source, &expected.source));
    }
    if let Some(field) = cargo_field(&expected.locks.cargo, &given.locks.cargo) {
        return Some(Divergence::Lock {
            file: super::cargo_lock::CARGO_LOCK,
            field,
        });
    }
    if let Some(field) = npm_field(&expected.locks.npm, &given.locks.npm) {
        return Some(Divergence::Lock {
            file: super::npm_lock::NPM_LOCK,
            field,
        });
    }
    let declared = first_field([
        ("format", expected.format != given.format),
        (
            "format_version",
            expected.format_version != given.format_version,
        ),
        ("producer", expected.producer != given.producer),
        ("statement", expected.statement != given.statement),
        ("package", expected.package != given.package),
        ("build.target", expected.build.target != given.build.target),
        (
            "build.toolchain",
            expected.build.toolchain != given.build.toolchain,
        ),
        (
            "build.release_profile",
            expected.build.release_profile != given.build.release_profile,
        ),
        (
            "build.recipes",
            expected.build.recipes != given.build.recipes,
        ),
        (
            "build.provider_clients",
            expected.build.provider_clients != given.build.provider_clients,
        ),
    ]);
    if let Some(field) = declared {
        return Some(Divergence::Declared { field });
    }
    if expected.build.inputs_identity != given.build.inputs_identity {
        return Some(Divergence::Identity);
    }
    if expected.sbom != given.sbom || expected_sbom != given_sbom {
        return Some(Divergence::Inventory);
    }
    for listed in &given.artifacts {
        if !expected
            .artifacts
            .iter()
            .any(|presented| presented.name == listed.name)
        {
            return Some(Divergence::ArtifactMissing {
                name: listed.name.clone(),
            });
        }
    }
    for presented in &expected.artifacts {
        let Some(listed) = given
            .artifacts
            .iter()
            .find(|listed| listed.name == presented.name)
        else {
            return Some(Divergence::ArtifactUnlisted {
                name: presented.name.clone(),
            });
        };
        if (listed.bytes, &listed.sha256) != (presented.bytes, &presented.sha256) {
            return Some(Divergence::ArtifactChanged {
                name: presented.name.clone(),
            });
        }
        if listed != presented {
            return Some(Divergence::Declared { field: "artifacts" });
        }
    }
    if expected.artifacts != given.artifacts {
        return Some(Divergence::Declared { field: "artifacts" });
    }
    if expected.uncovered != given.uncovered {
        return Some(Divergence::Declared { field: "uncovered" });
    }
    (expected != given).then_some(Divergence::Declared { field: "manifest" })
}
