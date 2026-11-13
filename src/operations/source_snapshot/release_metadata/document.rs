//! Release manifest format 1 (`release-manifest.json`), the build-input
//! identity and `SHA256SUMS`.
//!
//! Every type has a fixed field order, refuses unknown fields and holds no
//! float and no map of nondeterministic order. The canonical bytes are
//! `serde_json::to_vec_pretty` followed by one line feed; a file in another
//! form is not a format-1 publication. Any change of the field set, the field
//! order, a statement or a derivation rule requires `format_version` 2.
use super::{
    Derived,
    artifact::ArtifactDigest,
    declared::{DeclaredBuild, Known, RecipeV1, ScalarV1, ToolchainV1},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const FORMAT: &str = "ai-guard.release-manifest";
pub(super) const FORMAT_VERSION: u32 = 1;
pub(super) const INPUTS_IDENTITY_LAYOUT: &str = "ai-guard:build-inputs:v1";
pub(super) const MANIFEST_FILE: &str = "release-manifest.json";
pub(super) const SBOM_FILE: &str = "sbom.cdx.json";
pub(super) const SUMS_FILE: &str = "SHA256SUMS";
pub(super) const CODEX_ARGUMENT: &str = "CODEX_VERSION";
pub(super) const CLAUDE_CODE_ARGUMENT: &str = "CLAUDE_CODE_VERSION";
const ROOT_PACKAGE: &str = "Cargo.lock root package";
const PRE_BUILD_TARGET: &str = "pre-build manifest; the platform of a build is not observed";
const ARTIFACT_PROVENANCE: &str = "artefact provenance is declared, not observed";
const ARTIFACT_EVIDENCE: &str = "declared by the producer; hashed when the manifest was made";
const PROVIDER_EVIDENCE: &str = "declared by the operator; not observed in an image";
const INPUTS_STATEMENT: &str = "identifies these build inputs, not a build: target, artefact \
    profile, base-image digests and compiler are not observed";
const BINDS: [&str; 4] = [
    "prepared build inputs",
    "lock files",
    "declared build facts",
    "declared artefacts",
];
const DOES_NOT_ESTABLISH: [&str; 8] = [
    "signature",
    "build attestation",
    "SLSA provenance",
    "anti-rollback",
    "approved commit",
    "image digest",
    "installed image content",
    "trust in the selected distribution",
];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ProducerV1 {
    pub(super) name: String,
    pub(super) version: String,
    /// SHA-256 of the `.dockerignore` whose allowlist this CLI compiles in.
    pub(super) allowlist_sha256: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct StatementV1 {
    pub(super) lifecycle: String,
    pub(super) binds: Vec<String>,
    pub(super) does_not_establish: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct DigestV1 {
    pub(super) alg: String,
    pub(super) layout: String,
    pub(super) value: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct EntryV1 {
    pub(super) path: String,
    pub(super) bytes: u64,
    pub(super) sha256: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceV1 {
    pub(super) scope: String,
    pub(super) digest: DigestV1,
    pub(super) files: u64,
    pub(super) bytes: u64,
    /// Sorted by path, as the digest was computed over them.
    pub(super) entries: Vec<EntryV1>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct PackageV1 {
    pub(super) name: String,
    pub(super) version: String,
    pub(super) from: String,
    pub(super) license: Known<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct CargoLockV1 {
    pub(super) path: String,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    pub(super) lock_version: u32,
    pub(super) packages: u64,
    pub(super) registry_packages: u64,
    pub(super) path_packages: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct NpmLockV1 {
    pub(super) path: String,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    pub(super) lockfile_version: u32,
    pub(super) packages: u64,
    pub(super) dev: u64,
    pub(super) optional: u64,
    pub(super) install_scripts: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct LocksV1 {
    pub(super) cargo: CargoLockV1,
    pub(super) npm: NpmLockV1,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct InputsIdentityV1 {
    pub(super) alg: String,
    pub(super) layout: String,
    pub(super) value: String,
    pub(super) covers: Vec<String>,
    pub(super) statement: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ProviderClientV1 {
    pub(super) build_argument: String,
    pub(super) requested_version: String,
    pub(super) package: Known<String>,
    pub(super) evidence: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct BuildV1 {
    pub(super) inputs_identity: InputsIdentityV1,
    pub(super) target: Known<String>,
    pub(super) toolchain: Known<ToolchainV1>,
    pub(super) release_profile: Known<BTreeMap<String, ScalarV1>>,
    pub(super) recipes: Vec<RecipeV1>,
    /// Only the build arguments the operator declared, sorted by name.
    pub(super) provider_clients: Vec<ProviderClientV1>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct SbomRefV1 {
    pub(super) path: String,
    pub(super) format: String,
    pub(super) spec_version: String,
    pub(super) bytes: u64,
    pub(super) sha256: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ArtifactV1 {
    pub(super) name: String,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    pub(super) profile: Known<String>,
    pub(super) target: Known<String>,
    pub(super) evidence: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct UncoveredV1 {
    pub(super) code: String,
    pub(super) subjects: Vec<String>,
    pub(super) statement: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ReleaseManifestV1 {
    pub(super) format: String,
    pub(super) format_version: u32,
    pub(super) producer: ProducerV1,
    pub(super) statement: StatementV1,
    pub(super) source: SourceV1,
    pub(super) package: PackageV1,
    pub(super) locks: LocksV1,
    pub(super) build: BuildV1,
    pub(super) sbom: SbomRefV1,
    /// Sorted by name; empty for a pure pre-build manifest.
    pub(super) artifacts: Vec<ArtifactV1>,
    pub(super) uncovered: Vec<UncoveredV1>,
}

/// The provider versions an operator declared for `manifest`, or that a
/// manifest records for `verify`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct DeclaredInputs {
    pub(super) codex: Option<String>,
    pub(super) claude_code: Option<String>,
}

/// This CLI as a producer of format 1. The version is the package version of
/// every build of this package: it does not identify the CLI build.
pub(super) fn producer() -> ProducerV1 {
    ProducerV1 {
        name: env!("CARGO_PKG_NAME").to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        allowlist_sha256: super::super::capture::BUILD_IGNORE_SHA256.to_owned(),
    }
}

/// Build-input identity, layout `ai-guard:build-inputs:v1`:
///
/// ```text
/// SHA-256( "ai-guard:build-inputs:v1\0"
///          ‖ u64be(64) ‖ source digest (64 lowercase hex)
///          ‖ u8(p) ‖ u64be(len) ‖ CODEX_VERSION or ""          p = 1 if declared, else 0
///          ‖ u8(p) ‖ u64be(len) ‖ CLAUDE_CODE_VERSION or "" )
/// ```
///
/// It names these build inputs and these declared provider versions only.
/// No target, artefact profile, base-image digest or compiler enters it: two
/// builds for different platforms, or that resolve different images behind
/// one tag, share it.
pub(super) fn inputs_identity(
    source_digest: &str,
    codex: Option<&str>,
    claude_code: Option<&str>,
) -> String {
    let mut hash = Sha256::new();
    hash.update(INPUTS_IDENTITY_LAYOUT.as_bytes());
    hash.update([0_u8]);
    hash.update((source_digest.len() as u64).to_be_bytes());
    hash.update(source_digest.as_bytes());
    for declared in [codex, claude_code] {
        let (present, text) = match declared {
            Some(text) => (1_u8, text),
            None => (0_u8, ""),
        };
        hash.update([present]);
        hash.update((text.len() as u64).to_be_bytes());
        hash.update(text.as_bytes());
    }
    hex::encode(hash.finalize())
}

pub(super) fn canonical<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes =
        serde_json::to_vec_pretty(value).context("a release document cannot be serialised")?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// A format-1 manifest from its canonical bytes: unknown fields are refused,
/// and the bytes must equal the serialisation of what was read.
pub(super) fn parse_canonical(bytes: &[u8]) -> Result<ReleaseManifestV1> {
    let manifest: ReleaseManifestV1 = serde_json::from_slice(bytes)
        .context("release-manifest.json is not a well-formed format-1 manifest")?;
    ensure!(
        canonical(&manifest)? == bytes,
        "release-manifest.json is not in the canonical form of format 1 (to_vec_pretty and one \
         line feed)"
    );
    Ok(manifest)
}

/// `SHA256SUMS`: one `<sha256>  <name>` line per document, sorted by name.
pub(super) fn sums(manifest: &[u8], sbom: &[u8]) -> Vec<u8> {
    format!(
        "{}  {MANIFEST_FILE}\n{}  {SBOM_FILE}\n",
        hex::encode(Sha256::digest(manifest)),
        hex::encode(Sha256::digest(sbom))
    )
    .into_bytes()
}

fn lowercase_hex64(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The digests `(release-manifest.json, sbom.cdx.json)` of a `SHA256SUMS` in
/// exactly the form [`sums`] writes, or why it is not.
pub(super) fn parse_sums(bytes: &[u8]) -> std::result::Result<(String, String), String> {
    let malformed = || {
        format!(
            "{SUMS_FILE} is not exactly the two lines \"<sha256>  {MANIFEST_FILE}\" and \
             \"<sha256>  {SBOM_FILE}\""
        )
    };
    let text = std::str::from_utf8(bytes).map_err(|_| malformed())?;
    let body = text.strip_suffix('\n').ok_or_else(malformed)?;
    let lines: Vec<&str> = body.split('\n').collect();
    let [first, second] = lines.as_slice() else {
        return Err(malformed());
    };
    let digest = |line: &str, name: &str| -> Option<String> {
        let (hex, rest) = line.split_once("  ")?;
        (lowercase_hex64(hex) && rest == name).then(|| hex.to_owned())
    };
    match (digest(*first, MANIFEST_FILE), digest(*second, SBOM_FILE)) {
        (Some(manifest), Some(sbom)) => Ok((manifest, sbom)),
        _ => Err(malformed()),
    }
}

/// The provider versions `manifest` records, refusing a build argument this
/// format does not know or one that appears twice.
pub(super) fn declared_inputs(
    manifest: &ReleaseManifestV1,
) -> std::result::Result<DeclaredInputs, String> {
    let mut inputs = DeclaredInputs::default();
    for client in &manifest.build.provider_clients {
        let slot = match client.build_argument.as_str() {
            CODEX_ARGUMENT => &mut inputs.codex,
            CLAUDE_CODE_ARGUMENT => &mut inputs.claude_code,
            _ => {
                return Err(
                    "build.provider_clients names a build argument outside format 1".into(),
                );
            }
        };
        if slot.replace(client.requested_version.clone()).is_some() {
            return Err("build.provider_clients names one build argument twice".into());
        }
    }
    Ok(inputs)
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn provider_clients(declared: &DeclaredInputs, build: &DeclaredBuild) -> Vec<ProviderClientV1> {
    let mut clients = Vec::new();
    for (argument, version, package) in [
        (
            CLAUDE_CODE_ARGUMENT,
            &declared.claude_code,
            &build.claude_code_package,
        ),
        (CODEX_ARGUMENT, &declared.codex, &build.codex_package),
    ] {
        if let Some(version) = version {
            clients.push(ProviderClientV1 {
                build_argument: argument.to_owned(),
                requested_version: version.clone(),
                package: package.clone(),
                evidence: PROVIDER_EVIDENCE.to_owned(),
            });
        }
    }
    clients
}

/// The external references of the declared base images, sorted and unique,
/// and the recipes whose base images are not all known.
pub(super) fn base_images(recipes: &[RecipeV1]) -> (Vec<String>, Vec<String>) {
    let mut references = BTreeSet::new();
    let mut unknown = BTreeSet::new();
    for recipe in recipes {
        match &recipe.base_images {
            Known::Declared { value, .. } => {
                for image in value {
                    match image {
                        Known::Declared { value, .. } if !value.internal_stage => {
                            references.insert(value.reference.clone());
                        }
                        Known::Declared { .. } => {}
                        Known::Unknown { .. } => {
                            unknown.insert(recipe.path.clone());
                        }
                    }
                }
            }
            Known::Unknown { .. } => {
                unknown.insert(recipe.path.clone());
            }
        }
    }
    (
        references.into_iter().collect(),
        unknown.into_iter().collect(),
    )
}

fn uncovered(
    derived: &Derived,
    declared: &DeclaredInputs,
    artifacts: &[ArtifactDigest],
) -> Vec<UncoveredV1> {
    let recipes = &derived.declared.recipes;
    let recipe_paths: Vec<String> = recipes.iter().map(|recipe| recipe.path.clone()).collect();
    let (references, unknown_images) = base_images(recipes);
    let tag_pinned: Vec<String> = references
        .iter()
        .filter(|reference| !reference.contains("@sha256:"))
        .cloned()
        .collect();
    let mut image_content: BTreeSet<String> = references.iter().cloned().collect();
    image_content.extend(unknown_images);
    let mut targets = recipe_paths.clone();
    if !artifacts.is_empty() {
        targets.push("declared artefacts".to_owned());
    }
    let mut downloads = Vec::new();
    for (requested, package) in [
        (&declared.claude_code, &derived.declared.claude_code_package),
        (&declared.codex, &derived.declared.codex_package),
    ] {
        if let (Some(_), Some(package)) = (requested, package.value()) {
            downloads.push(package.clone());
        }
    }
    let with_invocations: Vec<String> = recipes
        .iter()
        .filter(|recipe| match &recipe.cargo_build_invocations {
            Known::Declared { value, .. } => !value.is_empty(),
            Known::Unknown { .. } => true,
        })
        .map(|recipe| recipe.path.clone())
        .collect();
    let optional: Vec<String> = derived
        .npm
        .packages
        .iter()
        .filter(|package| package.optional)
        .map(|package| package.name.clone())
        .collect();
    let scripts: Vec<String> = derived
        .npm
        .packages
        .iter()
        .filter(|package| package.install_script)
        .map(|package| package.name.clone())
        .collect();
    let lock = || vec![super::cargo_lock::CARGO_LOCK.to_owned()];
    let entries: [(&str, Vec<String>, &str); 16] = [
        (
            "build-target",
            targets,
            "the platform a build targets is not observed by a pre-build manifest",
        ),
        (
            "base-image-digest",
            tag_pinned,
            "a tag names no content: the image digest a build resolves is not observed",
        ),
        (
            "base-image-content",
            image_content.into_iter().collect(),
            "the content of base images is not inventoried",
        ),
        (
            "system-packages",
            recipe_paths.clone(),
            "packages that recipe instructions install (for example APT) are not inventoried",
        ),
        (
            "build-time-downloads",
            downloads,
            "packages an image build downloads outside the locks, such as provider clients, are \
             not inventoried with their content or integrity",
        ),
        (
            "toolchain-content",
            vec![super::declared::TOOLCHAIN.to_owned()],
            "the toolchain is requested, not observed: the compiler that ran and its components \
             are not inventoried",
        ),
        (
            "cargo-configuration",
            with_invocations,
            "Cargo configuration of base images, ENV and ARG values and the build environment are \
             not interpreted",
        ),
        (
            "rust-applicability",
            lock(),
            "the lock resolves neither targets nor dependency kinds: crate scope is omitted \
             (ai-guard:scope=unresolved)",
        ),
        (
            "npm-installed-subset",
            optional,
            "optional platform packages are listed; which are installed depends on the build \
             platform",
        ),
        (
            "native-code-in-crates",
            lock(),
            "crates may build or link native code, which is not inventoried",
        ),
        (
            "rust-licenses",
            lock(),
            "Cargo.lock records no licence: crate licences are not stated",
        ),
        (
            "install-scripts",
            scripts,
            "these packages run install scripts during npm ci; that code is not judged",
        ),
        (
            "frontend-bundle",
            vec![super::npm_lock::NPM_LOCK.to_owned()],
            "the built frontend bundle is not inventoried; only its locked build inputs are",
        ),
        (
            "package-identifiers",
            vec![SBOM_FILE.to_owned()],
            "no purl or CPE in format 1: automated vulnerability matching is limited",
        ),
        (
            "artifact-provenance",
            artifacts
                .iter()
                .map(|artifact| artifact.name.clone())
                .collect(),
            "artefacts are bound by name, size and SHA-256 as declared; they are not attested as \
             built from this source, and their profile and target are not observed",
        ),
        (
            "image-identity",
            recipe_paths,
            "no image ID or image digest is observed",
        ),
    ];
    entries
        .into_iter()
        .map(|(code, subjects, statement)| UncoveredV1 {
            code: code.to_owned(),
            subjects,
            statement: statement.to_owned(),
        })
        .collect()
}

/// The manifest of `derived` with these declared versions, artefacts and SBOM.
pub(super) fn manifest(
    derived: &Derived,
    declared: &DeclaredInputs,
    artifacts: &[ArtifactDigest],
    sbom: &[u8],
) -> ReleaseManifestV1 {
    let root = &derived.cargo.packages[derived.cargo.root];
    let identity = inputs_identity(
        &derived.source.digest.value,
        declared.codex.as_deref(),
        declared.claude_code.as_deref(),
    );
    let mut bound: Vec<ArtifactV1> = artifacts
        .iter()
        .map(|artifact| ArtifactV1 {
            name: artifact.name.clone(),
            bytes: artifact.bytes,
            sha256: artifact.sha256.clone(),
            profile: Known::unknown(ARTIFACT_PROVENANCE),
            target: Known::unknown(ARTIFACT_PROVENANCE),
            evidence: ARTIFACT_EVIDENCE.to_owned(),
        })
        .collect();
    bound.sort_by(|left, right| left.name.cmp(&right.name));
    ReleaseManifestV1 {
        format: FORMAT.to_owned(),
        format_version: FORMAT_VERSION,
        producer: producer(),
        statement: StatementV1 {
            lifecycle: "pre-build".to_owned(),
            binds: strings(&BINDS),
            does_not_establish: strings(&DOES_NOT_ESTABLISH),
        },
        source: derived.source.clone(),
        package: PackageV1 {
            name: root.name.clone(),
            version: root.version.clone(),
            from: ROOT_PACKAGE.to_owned(),
            license: derived.declared.package_license.clone(),
        },
        locks: derived.locks.clone(),
        build: BuildV1 {
            inputs_identity: InputsIdentityV1 {
                alg: "SHA-256".to_owned(),
                layout: INPUTS_IDENTITY_LAYOUT.to_owned(),
                value: identity,
                covers: strings(&[
                    "source.digest",
                    "declared CODEX_VERSION",
                    "declared CLAUDE_CODE_VERSION",
                ]),
                statement: INPUTS_STATEMENT.to_owned(),
            },
            target: Known::unknown(PRE_BUILD_TARGET),
            toolchain: derived.declared.toolchain.clone(),
            release_profile: derived.declared.release_profile.clone(),
            recipes: derived.declared.recipes.clone(),
            provider_clients: provider_clients(declared, &derived.declared),
        },
        sbom: SbomRefV1 {
            path: SBOM_FILE.to_owned(),
            format: "CycloneDX".to_owned(),
            spec_version: "1.6".to_owned(),
            bytes: sbom.len() as u64,
            sha256: hex::encode(Sha256::digest(sbom)),
        },
        artifacts: bound,
        uncovered: uncovered(derived, declared, artifacts),
    }
}
