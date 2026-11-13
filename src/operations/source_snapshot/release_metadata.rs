//! `agentsandbox release manifest` and `agentsandbox release verify`: a
//! release manifest, a CycloneDX inventory and `SHA256SUMS` derived from a
//! build source prepared exactly as `build` prepares it.
//!
//! The only prepared source is the one `prepare_build` returns. What is read
//! from it goes through the retained-entry accessors of the snapshot, which
//! verify the private copy before and after the read; the live checkout is
//! never walked, and no path into the copy is handled here. The documents
//! hold no host path, device, inode, user, time or environment value.
//!
//! What the documents establish: the prepared build inputs, the content of
//! the locks, the facts the inputs declare and the artefacts as declared by
//! the producer, bound by digest. They carry a build-input identity, which
//! identifies these inputs and the declared provider versions, and is not a
//! build identity: the target, the profile an artefact was built with and the
//! base-image digests a build resolves are not observed and are stated as
//! unknown. Nothing here is a signature, a build attestation, SLSA
//! provenance or an anti-rollback measure, and no declared value is
//! authenticated.
mod artifact;
mod cargo_lock;
mod compare;
mod cyclonedx;
mod declared;
mod document;
mod npm_lock;
mod output;
#[cfg(test)]
mod tests;
mod toml_subset;

use super::{
    PreparedSource,
    capture::{self, Snapshot},
    interrupt, prepare_build,
};
use crate::{
    control_fs,
    operations::ReleaseCommand,
    runtime::retained_cli::{Interruption, InvocationFailure, signal_name},
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use artifact::ArtifactDigest;
use compare::{Divergence, Unverified};
use document::DeclaredInputs;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Bound of one declaration file read from the prepared copy.
const MAX_DECLARATION_BYTES: u64 = 8 * 1024 * 1024;
/// Bound of one release document, written or read.
const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
/// The retained-entry accessor reads at most 64 names: these five and the
/// recipes.
const FIXED_INPUTS: [&str; 5] = [
    cargo_lock::CARGO_LOCK,
    npm_lock::NPM_LOCK,
    declared::CARGO_TOML,
    declared::TOOLCHAIN,
    npm_lock::NPM_MANIFEST,
];
const MAX_RECIPES: usize = 64 - FIXED_INPUTS.len();
const LIMITS_LINE: &str = "limits     declared values are not authenticated; this is not a \
    signature, a build attestation, SLSA provenance or anti-rollback";
/// What may exist after a step that runs before any publication.
const NOTHING_PUBLISHED: &str = "nothing was published and no staging directory was created";

/// A lock that format 1 cannot read completely. Never downgraded to a
/// warning or an unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LockRefusal {
    file: &'static str,
    line: Option<usize>,
    reason: String,
}

impl fmt::Display for LockRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = capture::shown(Path::new(&self.reason));
        match self.line {
            Some(line) => write!(formatter, "{}:{line}: {reason}", self.file)?,
            None => write!(formatter, "{}: {reason}", self.file)?,
        }
        formatter
            .write_str("; release manifest format 1 accepts only a complete lock it can resolve")
    }
}

impl std::error::Error for LockRefusal {}

/// Everything derived from one prepared source, in memory, before any output.
struct Derived {
    source: document::SourceV1,
    cargo: cargo_lock::CargoLock,
    npm: npm_lock::PackageLock,
    declared: declared::DeclaredBuild,
    locks: document::LocksV1,
}

struct ProduceRequest {
    source: PathBuf,
    output: PathBuf,
    artifacts: Vec<PathBuf>,
    declared: DeclaredInputs,
    expect_source_digest: Option<String>,
}

struct VerifyRequest {
    source: PathBuf,
    manifest: PathBuf,
    artifacts: Vec<PathBuf>,
    source_only: bool,
}

/// One `release` operator command. It runs inside the operator interruption
/// scope, holds it for its whole body, starts no engine and opens no socket.
pub(super) async fn execute(command: ReleaseCommand) -> Result<()> {
    let interruption = interrupt::current();
    // Taken on the command's own task before its first await: the scope never
    // drops this command while it prepares, reads, hashes or publishes.
    let hold = interruption.as_ref().map(Interruption::hold);
    let result = match command {
        ReleaseCommand::Manifest {
            source,
            output,
            artifacts,
            codex_version,
            claude_code_version,
            expect_source_digest,
        } => {
            let request = ProduceRequest {
                source,
                output,
                artifacts,
                declared: DeclaredInputs {
                    codex: codex_version,
                    claude_code: claude_code_version,
                },
                expect_source_digest,
            };
            produce(request, interruption.as_ref()).await
        }
        ReleaseCommand::Verify {
            source,
            manifest,
            artifacts,
            source_only,
        } => {
            let request = VerifyRequest {
                source,
                manifest,
                artifacts,
                source_only,
            };
            verify(request, interruption.as_ref()).await
        }
    };
    drop(hold);
    result
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn lowercase_hex64(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn stopped(stop: Option<&Interruption>) -> Result<()> {
    if let Some(signal) = stop.and_then(Interruption::requested) {
        return Err(InvocationFailure::Interrupted { signal }.into());
    }
    Ok(())
}

/// What the operator is told when a stop request ended a step before
/// publication.
fn interrupted(error: anyhow::Error) -> anyhow::Error {
    match error.downcast_ref::<InvocationFailure>() {
        Some(InvocationFailure::Interrupted { signal }) => anyhow!(
            "interrupted by {} before publication; nothing was published and no staging \
             directory was created; nothing was written outside the temporary prepared copy, \
             which ai-guard removes (a failed removal is reported as a warning)",
            signal_name(*signal)
        ),
        _ => error,
    }
}

/// A blocking step whose thread ended without returning its result: it
/// panicked, or the runtime cancelled it. What the step did is then unknown
/// to the command, so the message says only what may exist after it.
#[derive(Debug)]
struct StepLost {
    step: &'static str,
    consequence: String,
}

impl fmt::Display for StepLost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the {} step ended without a result; {}",
            self.step, self.consequence
        )
    }
}

impl std::error::Error for StepLost {}

/// Runs `work` off the command's task and returns its own result. A step
/// that ends without one is reported as [`StepLost`] with `consequence`.
async fn blocking<T, F>(step: &'static str, consequence: String, work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(_) => Err(StepLost { step, consequence }.into()),
    }
}

/// `images/<name>.Dockerfile`, one level below `images`.
fn recipe_name(name: &str) -> bool {
    name.strip_prefix("images/")
        .is_some_and(|rest| !rest.contains('/') && rest.ends_with(".Dockerfile"))
}

/// The source record of `snapshot`, from the entries recorded at capture;
/// nothing is read from the copy.
fn source_of(snapshot: &Snapshot) -> document::SourceV1 {
    document::SourceV1 {
        scope: "build".to_owned(),
        digest: document::DigestV1 {
            alg: "SHA-256".to_owned(),
            layout: capture::DIGEST_LAYOUT.to_owned(),
            value: snapshot.digest().to_owned(),
        },
        files: snapshot.entries().len() as u64,
        bytes: snapshot.total_bytes(),
        entries: snapshot
            .entries()
            .map(|entry| document::EntryV1 {
                path: entry.name().to_owned(),
                bytes: entry.bytes(),
                sha256: entry.sha256().to_owned(),
            })
            .collect(),
    }
}

/// Blocking: reads the retained declarations of `snapshot` (verified before
/// and after the read, all or nothing), then parses them. `stop` is looked at
/// between steps.
fn derive(snapshot: &Snapshot, stop: Option<&Interruption>) -> Result<Derived> {
    stopped(stop)?;
    let recipes: Vec<String> = snapshot
        .entries()
        .map(|entry| entry.name())
        .filter(|name| recipe_name(name))
        .map(str::to_owned)
        .collect();
    ensure!(
        recipes.len() <= MAX_RECIPES,
        "the prepared source holds more than {MAX_RECIPES} recipes; format 1 reads at most \
         {MAX_RECIPES}"
    );
    let mut names: Vec<&str> = FIXED_INPUTS.to_vec();
    names.extend(recipes.iter().map(String::as_str));
    let recorded: BTreeMap<&str, (u64, &str)> = snapshot
        .entries()
        .map(|entry| (entry.name(), (entry.bytes(), entry.sha256())))
        .collect();
    let mut total = 0_u64;
    for name in &names {
        let Some((length, _)) = recorded.get(name) else {
            bail!("required build input is not a prepared entry: {name}");
        };
        total = total.saturating_add(*length);
    }
    ensure!(
        total <= control_fs::MAX_PRIVATE_FILE_BYTES,
        "the declarations to read exceed the bound of {} bytes",
        control_fs::MAX_PRIVATE_FILE_BYTES
    );
    let contents = snapshot.read_retained(&names, MAX_DECLARATION_BYTES)?;
    ensure!(
        contents.len() == names.len(),
        "the prepared copy returned another number of declarations than was asked"
    );
    for (name, content) in names.iter().zip(&contents) {
        let Some((length, sha256)) = recorded.get(name) else {
            bail!("required build input is not a prepared entry: {name}");
        };
        ensure!(
            content.len() as u64 == *length && sha256_hex(content) == *sha256,
            "the bytes read for {name} are not those recorded when the source was prepared"
        );
    }
    stopped(stop)?;
    let cargo = cargo_lock::parse(&contents[0])?;
    stopped(stop)?;
    let npm = npm_lock::parse(&contents[1], &contents[4])?;
    stopped(stop)?;
    let recipe_inputs: Vec<(&str, &[u8])> = recipes
        .iter()
        .map(String::as_str)
        .zip(contents[FIXED_INPUTS.len()..].iter().map(Vec::as_slice))
        .collect();
    let facts = declared::read(&contents[2], &contents[3], &recipe_inputs);
    let root = &cargo.packages[cargo.root];
    if let Some((name, version)) = facts.cargo_package.value() {
        ensure!(
            *name == root.name && *version == root.version,
            "{} declares package {} {} but the root package of {} is {} {}: the lock is stale \
             and is refused",
            declared::CARGO_TOML,
            capture::shown(Path::new(name)),
            capture::shown(Path::new(version)),
            cargo_lock::CARGO_LOCK,
            root.name,
            root.version
        );
    }
    let source = source_of(snapshot);
    let mut install_scripts: Vec<String> = npm
        .packages
        .iter()
        .filter(|package| package.install_script)
        .map(|package| package.name.clone())
        .collect();
    install_scripts.sort();
    let locks = document::LocksV1 {
        cargo: document::CargoLockV1 {
            path: cargo_lock::CARGO_LOCK.to_owned(),
            bytes: contents[0].len() as u64,
            sha256: sha256_hex(&contents[0]),
            lock_version: cargo_lock::LOCK_VERSION,
            packages: cargo.packages.len() as u64,
            registry_packages: cargo.registry_packages() as u64,
            path_packages: cargo.path_packages() as u64,
        },
        npm: document::NpmLockV1 {
            path: npm_lock::NPM_LOCK.to_owned(),
            bytes: contents[1].len() as u64,
            sha256: sha256_hex(&contents[1]),
            lockfile_version: npm.lockfile_version,
            packages: npm.packages.len() as u64,
            dev: npm.packages.iter().filter(|package| package.dev).count() as u64,
            optional: npm
                .packages
                .iter()
                .filter(|package| package.optional)
                .count() as u64,
            install_scripts,
        },
    };
    Ok(Derived {
        source,
        cargo,
        npm,
        declared: facts,
        locks,
    })
}

/// Prepares `source` as `build` does: the structural check of the selected
/// distribution, then `prepare_build` (capture, confirmation, the check of
/// the copy and the provenance line).
async fn prepare_source(source: &Path) -> Result<PreparedSource> {
    crate::operations::preflight_build_inputs(source)?;
    prepare_build(source).await
}

/// Derives the documents of `prepared` off the command's task. The prepared
/// copy is released before this returns, whatever the result.
async fn derive_and_release(
    prepared: PreparedSource,
    stop: Option<&Interruption>,
) -> Result<Derived> {
    let snapshot = Arc::clone(&prepared.snapshot);
    let owned_stop = stop.cloned();
    let derived = blocking("derivation", NOTHING_PUBLISHED.to_owned(), move || {
        derive(&snapshot, owned_stop.as_ref())
    })
    .await;
    // The copy is released here, before any artefact is read or any output
    // is written: nothing below reads from it.
    drop(prepared);
    derived
}

async fn produce(request: ProduceRequest, stop: Option<&Interruption>) -> Result<()> {
    let ProduceRequest {
        source,
        output,
        artifacts: paths,
        declared: inputs,
        expect_source_digest,
    } = request;
    // Everything that can be refused before the source is prepared.
    for (option, version) in [
        ("--codex-version", &inputs.codex),
        ("--claude-code-version", &inputs.claude_code),
    ] {
        if let Some(version) = version {
            declared::validate_declared_provider_version(version)
                .with_context(|| format!("{option} is refused; nothing was prepared"))?;
        }
    }
    if let Some(expected) = &expect_source_digest {
        ensure!(
            lowercase_hex64(expected),
            "--expect-source-digest must be 64 lowercase hexadecimal characters, as printed after \
             sha256= by build; nothing was prepared"
        );
    }
    artifact::check_paths(&paths).context("--artifact is refused; nothing was prepared")?;
    let source = source.canonicalize().with_context(|| {
        format!(
            "--source is unavailable: {}; nothing was prepared",
            capture::shown(&source)
        )
    })?;
    let destination = output::destination(&output, &source)
        .context("--output is refused; nothing was prepared")?;

    let prepared = prepare_source(&source).await.map_err(interrupted)?;
    if let Some(expected) = expect_source_digest.as_deref() {
        let digest = prepared.snapshot.digest().to_owned();
        if digest != expected {
            drop(prepared);
            bail!(
                "the prepared source digest {digest} differs from the expected {expected} \
                 (declared by the operator); nothing was read from it and nothing was written \
                 outside the temporary prepared copy, which ai-guard removes"
            );
        }
    }
    let derived = derive_and_release(prepared, stop)
        .await
        .map_err(interrupted)?;

    let owned_stop = stop.cloned();
    let artifacts = blocking(
        "artefact hashing",
        NOTHING_PUBLISHED.to_owned(),
        move || artifact::digest_all(&paths, owned_stop.as_ref()),
    )
    .await
    .map_err(interrupted)?;

    let identity = document::inputs_identity(
        &derived.source.digest.value,
        inputs.codex.as_deref(),
        inputs.claude_code.as_deref(),
    );
    let sbom = cyclonedx::sbom(&derived, &inputs, &identity)?;
    let manifest = document::manifest(&derived, &inputs, &artifacts, &sbom);
    let manifest_bytes = document::canonical(&manifest)?;
    ensure!(
        document::parse_canonical(&manifest_bytes)? == manifest,
        "the release manifest does not read back as itself; nothing was published"
    );
    ensure!(
        manifest_bytes.len() as u64 <= MAX_DOCUMENT_BYTES
            && sbom.len() as u64 <= MAX_DOCUMENT_BYTES,
        "a release document exceeds the bound of {MAX_DOCUMENT_BYTES} bytes; nothing was \
         published"
    );
    let sums = document::sums(&manifest_bytes, &sbom);
    // The last stop request looked at: from here the command writes three
    // small files and one rename, and runs to its confirmation.
    stopped(stop).map_err(interrupted)?;

    let shown_output = destination.shown();
    // Said only if the publication step ends without a result: the rename may
    // or may not have happened, and neither name is removed.
    let lost = destination.unknown_publication();
    let files = [
        (document::MANIFEST_FILE, manifest_bytes.clone()),
        (document::SBOM_FILE, sbom.clone()),
        (document::SUMS_FILE, sums),
    ];
    blocking("publication", lost, move || {
        output::publish(&destination, &files)
    })
    .await?;

    let producer = &manifest.producer;
    let mut summary = vec![
        format!("release manifest: published {shown_output}"),
        format!(
            "manifest   sha256={} format={}/{} producer={} {}",
            sha256_hex(&manifest_bytes),
            document::FORMAT,
            document::FORMAT_VERSION,
            producer.name,
            producer.version
        ),
        source_line(&manifest),
        locks_line(&manifest),
        format!(
            "sbom       {} sha256={} CycloneDX 1.6 pre-build",
            document::SBOM_FILE,
            sha256_hex(&sbom)
        ),
        identity_line(&manifest.build.inputs_identity.value, &inputs),
        "target     unknown (not observed)".to_owned(),
        format!(
            "artifacts  {} bound by name, size and SHA-256 (declared, not attested)",
            manifest.artifacts.len()
        ),
    ];
    if expect_source_digest.is_some() {
        summary.push("expected source digest matched (declared by the operator)".to_owned());
    }
    summary.push(LIMITS_LINE.to_owned());
    println!("{}", summary.join("\n"));
    Ok(())
}

fn source_line(manifest: &document::ReleaseManifestV1) -> String {
    format!(
        "source     sha256={} layout={} files={} bytes={}",
        manifest.source.digest.value,
        manifest.source.digest.layout,
        manifest.source.files,
        manifest.source.bytes
    )
}

fn locks_line(manifest: &document::ReleaseManifestV1) -> String {
    let locks = &manifest.locks;
    format!(
        "locks      {} sha256={} packages={}; {} sha256={} packages={}",
        locks.cargo.path,
        locks.cargo.sha256,
        locks.cargo.packages,
        locks.npm.path,
        locks.npm.sha256,
        locks.npm.packages
    )
}

fn identity_line(identity: &str, inputs: &DeclaredInputs) -> String {
    format!(
        "inputs identity {identity} (build inputs only; declared: {}={}, {}={})",
        document::CODEX_ARGUMENT,
        inputs.codex.as_deref().unwrap_or("none"),
        document::CLAUDE_CODE_ARGUMENT,
        inputs.claude_code.as_deref().unwrap_or("none")
    )
}

/// Why `release verify` reports no match.
enum Refusal {
    Unverified(Unverified),
    Divergent(Divergence),
}

impl From<Unverified> for Refusal {
    fn from(unverified: Unverified) -> Self {
        Self::Unverified(unverified)
    }
}

impl From<Divergence> for Refusal {
    fn from(divergence: Divergence) -> Self {
        Self::Divergent(divergence)
    }
}

fn unreadable(input: &'static str, error: anyhow::Error) -> Refusal {
    Refusal::Unverified(Unverified::Unreadable {
        input,
        reason: format!("{error:#}"),
    })
}

/// What `verify` reports when a step that reads `input` failed. A stop
/// request and a step that ended without a result are named as such: they
/// say nothing about `input`. Any other failure is `input` unreadable under
/// its policy.
fn not_judged(input: &'static str, error: anyhow::Error, stop: Option<&Interruption>) -> Refusal {
    if let Some(InvocationFailure::Interrupted { signal }) =
        error.downcast_ref::<InvocationFailure>()
    {
        return Refusal::Unverified(Unverified::Interrupted {
            signal: *signal,
            reason: None,
        });
    }
    if let Some(lost) = error.downcast_ref::<StepLost>() {
        return Refusal::Unverified(Unverified::Incomplete {
            reason: lost.to_string(),
        });
    }
    // The preparation reports its own interruption in a message shared with
    // `build`; the step's own words are kept.
    if let Some(signal) = stop.and_then(Interruption::requested) {
        return Refusal::Unverified(Unverified::Interrupted {
            signal,
            reason: Some(format!("{error:#}")),
        });
    }
    unreadable(input, error)
}

fn format_divergence(detail: String) -> Refusal {
    Refusal::Divergent(Divergence::Format { detail })
}

async fn verify(request: VerifyRequest, stop: Option<&Interruption>) -> Result<()> {
    match judge(request, stop).await {
        Ok(report) => {
            println!("{report}");
            Ok(())
        }
        Err(Refusal::Unverified(unverified)) => Err(anyhow!("{unverified}")),
        Err(Refusal::Divergent(divergence)) => Err(anyhow!("{divergence}")),
    }
}

/// The order of the comparison is fixed (CONTRACT section 4): the manifest
/// directory is read, its header probed, then its sums and canonical form are
/// checked, artefacts are hashed, the source is prepared and its digest
/// compared with the one the manifest binds, then it is derived, and the
/// derived documents are compared with the given ones.
async fn judge(
    request: VerifyRequest,
    stop: Option<&Interruption>,
) -> std::result::Result<String, Refusal> {
    let VerifyRequest {
        source,
        manifest,
        artifacts: paths,
        source_only,
    } = request;
    if source_only && !paths.is_empty() {
        return Err(Unverified::Conflict {
            reason: "--source-only verifies no artefact and cannot be combined with --artifact",
        }
        .into());
    }
    let directory =
        control_fs::absolute_path(&manifest).map_err(|error| unreadable("--manifest", error))?;
    let read = |name: &'static str| {
        control_fs::read_trusted_projected_file(&directory.join(name), MAX_DOCUMENT_BYTES)
            .map_err(|error| unreadable(name, error))
    };
    let manifest_bytes = read(document::MANIFEST_FILE)?;
    let sbom_bytes = read(document::SBOM_FILE)?;
    let sums_bytes = read(document::SUMS_FILE)?;

    compare::probe(&manifest_bytes)?;

    let (manifest_sum, sbom_sum) = document::parse_sums(&sums_bytes).map_err(format_divergence)?;
    if manifest_sum != sha256_hex(&manifest_bytes) {
        return Err(Divergence::Sums {
            file: document::MANIFEST_FILE,
        }
        .into());
    }
    if sbom_sum != sha256_hex(&sbom_bytes) {
        return Err(Divergence::Sums {
            file: document::SBOM_FILE,
        }
        .into());
    }
    let given = document::parse_canonical(&manifest_bytes)
        .map_err(|error| format_divergence(format!("{error:#}")))?;
    let inputs = document::declared_inputs(&given).map_err(format_divergence)?;
    for version in [&inputs.codex, &inputs.claude_code].into_iter().flatten() {
        declared::validate_declared_provider_version(version).map_err(|error| {
            format_divergence(format!(
                "a declared provider version is inadmissible: {error:#}"
            ))
        })?;
    }

    let artifacts: Vec<ArtifactDigest> = if source_only {
        given
            .artifacts
            .iter()
            .map(|listed| ArtifactDigest {
                name: listed.name.clone(),
                bytes: listed.bytes,
                sha256: listed.sha256.clone(),
            })
            .collect()
    } else {
        let owned_stop = stop.cloned();
        blocking(
            "artefact hashing",
            NOTHING_PUBLISHED.to_owned(),
            move || artifact::digest_all(&paths, owned_stop.as_ref()),
        )
        .await
        .map_err(|error| not_judged("--artifact", error, stop))?
    };

    let source = source
        .canonicalize()
        .with_context(|| format!("--source is unavailable: {}", capture::shown(&source)))
        .map_err(|error| unreadable("--source", error))?;
    let prepared = prepare_source(&source)
        .await
        .map_err(|error| not_judged("--source", error, stop))?;
    // Judged before anything is read from the copy: a source whose digest
    // differs from the one the manifest binds is a divergence, even when what
    // changed is something the strict readers would refuse (a lock line
    // outside format 1).
    let prepared_source = source_of(&prepared.snapshot);
    if prepared_source.digest.value != given.source.digest.value {
        drop(prepared);
        return Err(compare::source_divergence(&given.source, &prepared_source).into());
    }
    let derived = derive_and_release(prepared, stop)
        .await
        .map_err(|error| not_judged("--source", error, stop))?;

    let identity = document::inputs_identity(
        &derived.source.digest.value,
        inputs.codex.as_deref(),
        inputs.claude_code.as_deref(),
    );
    let expected_sbom = cyclonedx::sbom(&derived, &inputs, &identity)
        .map_err(|error| unreadable("--source", error))?;
    let expected = document::manifest(&derived, &inputs, &artifacts, &expected_sbom);
    if let Some(divergence) =
        compare::first_divergence(&expected, &given, &expected_sbom, &sbom_bytes)
    {
        return Err(divergence.into());
    }
    let expected_bytes =
        document::canonical(&expected).map_err(|error| unreadable("--source", error))?;
    if expected_bytes != manifest_bytes {
        return Err(Divergence::Declared { field: "manifest" }.into());
    }

    let heading = if source_only {
        format!(
            "release verify: SOURCE MATCH; artifacts NOT VERIFIED ({} listed)",
            given.artifacts.len()
        )
    } else {
        "release verify: MATCH".to_owned()
    };
    let artifacts_line = if source_only {
        format!(
            "artifacts  NOT VERIFIED ({} listed; --source-only)",
            given.artifacts.len()
        )
    } else {
        format!(
            "artifacts  {}/{} matched",
            artifacts.len(),
            given.artifacts.len()
        )
    };
    Ok([
        heading,
        format!(
            "manifest   sha256={} format={}/{} producer={} {}",
            sha256_hex(&manifest_bytes),
            given.format,
            given.format_version,
            given.producer.name,
            given.producer.version
        ),
        source_line(&given),
        locks_line(&given),
        format!(
            "sbom       {} sha256={} CycloneDX 1.6 pre-build",
            document::SBOM_FILE,
            sha256_hex(&sbom_bytes)
        ),
        identity_line(&given.build.inputs_identity.value, &inputs),
        "target     unknown (not observed)".to_owned(),
        artifacts_line,
        LIMITS_LINE.to_owned(),
    ]
    .join("\n"))
}
