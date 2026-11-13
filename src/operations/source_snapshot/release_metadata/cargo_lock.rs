//! Strict reader of a `Cargo.lock` of version 4 with one path package.
//!
//! Every package is resolved or the file is refused, with its line and the
//! reason: a lock that format 1 cannot read completely is never summarised.
//! Accepted sources are the crates.io registry, with a checksum, and exactly
//! one path package without a checksum, the package being described.
use super::{
    LockRefusal,
    toml_subset::{self, Item, Value},
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const CARGO_LOCK: &str = "Cargo.lock";
/// The only lock version format 1 reads.
pub(super) const LOCK_VERSION: u32 = 4;
const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";
const MAX_PACKAGES: usize = 16_384;
const MAX_DEPENDENCIES: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CargoSource {
    CratesIo,
    Path,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CargoPackage {
    pub(super) name: String,
    pub(super) version: String,
    pub(super) source: CargoSource,
    /// 64 lowercase hexadecimal characters; present exactly for crates.io.
    pub(super) checksum: Option<String>,
    /// Indices into `CargoLock::packages`, resolved and without repetition.
    pub(super) dependencies: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CargoLock {
    /// In file order.
    pub(super) packages: Vec<CargoPackage>,
    /// Index of the only path package.
    pub(super) root: usize,
}

impl CargoLock {
    pub(super) fn registry_packages(&self) -> usize {
        self.packages
            .iter()
            .filter(|package| package.source == CargoSource::CratesIo)
            .count()
    }

    pub(super) fn path_packages(&self) -> usize {
        self.packages
            .iter()
            .filter(|package| package.source == CargoSource::Path)
            .count()
    }
}

#[derive(Default)]
struct RawPackage {
    line: usize,
    name: Option<String>,
    version: Option<String>,
    source: Option<String>,
    checksum: Option<String>,
    dependencies: Option<(Vec<String>, usize)>,
}

fn refusal(line: Option<usize>, reason: impl Into<String>) -> LockRefusal {
    LockRefusal {
        file: CARGO_LOCK,
        line,
        reason: reason.into(),
    }
}

fn text(value: Value, line: usize, key: &str) -> Result<String, LockRefusal> {
    match value {
        Value::String(text) => Ok(text),
        _ => Err(refusal(Some(line), format!("{key} must be a string"))),
    }
}

fn texts(value: Value, line: usize) -> Result<Vec<String>, LockRefusal> {
    let Value::Array(values) = value else {
        return Err(refusal(
            Some(line),
            "dependencies must be an array of strings",
        ));
    };
    if values.len() > MAX_DEPENDENCIES {
        return Err(refusal(
            Some(line),
            format!("a package lists more than {MAX_DEPENDENCIES} dependencies"),
        ));
    }
    values
        .into_iter()
        .map(|value| text(value, line, "a dependency"))
        .collect()
}

/// `[A-Za-z0-9_-]{1,64}`.
fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// `[0-9A-Za-z.+-]{1,64}`.
fn valid_version(version: &str) -> bool {
    (1..=64).contains(&version.len())
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".+-".contains(&byte))
}

fn lowercase_hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn parse(bytes: &[u8]) -> Result<CargoLock, LockRefusal> {
    let items =
        toml_subset::parse(bytes).map_err(|refused| refusal(Some(refused.line), refused.reason))?;
    let mut lock_version = None;
    let mut raw: Vec<RawPackage> = Vec::new();
    for item in items {
        match item {
            Item::Header { array, path, line } => {
                if !(array && path.len() == 1 && path[0] == "package") {
                    return Err(refusal(
                        Some(line),
                        format!(
                            "table [{}] is outside format 1, which accepts [[package]] tables only",
                            path.join(".")
                        ),
                    ));
                }
                if raw.len() == MAX_PACKAGES {
                    return Err(refusal(
                        Some(line),
                        format!("the lock holds more than {MAX_PACKAGES} packages"),
                    ));
                }
                raw.push(RawPackage {
                    line,
                    ..RawPackage::default()
                });
            }
            Item::Pair { key, value, line } => {
                let Some(package) = raw.last_mut() else {
                    if key != "version" {
                        return Err(refusal(
                            Some(line),
                            format!("top-level key {key} is outside format 1"),
                        ));
                    }
                    match value {
                        Value::Integer(number) if number == i64::from(LOCK_VERSION) => {
                            lock_version = Some(LOCK_VERSION);
                        }
                        _ => {
                            return Err(refusal(
                                Some(line),
                                "the lock version must be 4 in format 1",
                            ));
                        }
                    }
                    continue;
                };
                match key.as_str() {
                    "name" => package.name = Some(text(value, line, "name")?),
                    "version" => package.version = Some(text(value, line, "version")?),
                    "source" => package.source = Some(text(value, line, "source")?),
                    "checksum" => package.checksum = Some(text(value, line, "checksum")?),
                    "dependencies" => package.dependencies = Some((texts(value, line)?, line)),
                    other => {
                        return Err(refusal(
                            Some(line),
                            format!("package key {other} is outside format 1"),
                        ));
                    }
                }
            }
        }
    }
    if lock_version != Some(LOCK_VERSION) {
        return Err(refusal(None, "the lock declares no version = 4"));
    }
    resolve(raw)
}

fn resolve(raw: Vec<RawPackage>) -> Result<CargoLock, LockRefusal> {
    let mut packages = Vec::with_capacity(raw.len());
    let mut wanted: Vec<(Vec<String>, usize)> = Vec::with_capacity(raw.len());
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut root = None;
    for package in raw {
        let line = package.line;
        let Some(name) = package.name else {
            return Err(refusal(Some(line), "a package has no name"));
        };
        let Some(version) = package.version else {
            return Err(refusal(
                Some(line),
                format!("package {name} has no version"),
            ));
        };
        if !valid_name(&name) {
            return Err(refusal(
                Some(line),
                "a package name is outside [A-Za-z0-9_-]{1,64}",
            ));
        }
        if !valid_version(&version) {
            return Err(refusal(
                Some(line),
                format!("the version of {name} is outside [0-9A-Za-z.+-]{{1,64}}"),
            ));
        }
        let source = match package.source.as_deref() {
            None => CargoSource::Path,
            Some(CRATES_IO) => CargoSource::CratesIo,
            Some(_) => {
                return Err(refusal(
                    Some(line),
                    format!(
                        "package {name} {version} has a source other than the crates.io registry; \
                         git, path-with-source and alternate registries are outside format 1"
                    ),
                ));
            }
        };
        match (source, &package.checksum) {
            (CargoSource::CratesIo, Some(checksum)) if lowercase_hex(checksum, 64) => {}
            (CargoSource::CratesIo, _) => {
                return Err(refusal(
                    Some(line),
                    format!(
                        "registry package {name} {version} has no checksum of 64 lowercase \
                         hexadecimal characters"
                    ),
                ));
            }
            (CargoSource::Path, None) => {}
            (CargoSource::Path, Some(_)) => {
                return Err(refusal(
                    Some(line),
                    format!("path package {name} {version} carries a checksum"),
                ));
            }
        }
        if source == CargoSource::Path {
            if root.is_some() {
                return Err(refusal(
                    Some(line),
                    "the lock holds more than one path package; workspaces are outside format 1",
                ));
            }
            root = Some(packages.len());
        }
        if !seen.insert((name.clone(), version.clone())) {
            return Err(refusal(
                Some(line),
                format!("package {name} {version} is listed twice"),
            ));
        }
        wanted.push(package.dependencies.unwrap_or((Vec::new(), line)));
        packages.push(CargoPackage {
            name,
            version,
            source,
            checksum: package.checksum,
            dependencies: Vec::new(),
        });
    }
    let Some(root) = root else {
        return Err(refusal(None, "the lock holds no path package"));
    };
    let mut by_name: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, package) in packages.iter().enumerate() {
        by_name
            .entry(package.name.as_str())
            .or_default()
            .push(index);
    }
    let mut resolved: Vec<Vec<usize>> = Vec::with_capacity(packages.len());
    for (dependencies, line) in &wanted {
        let mut indices = Vec::with_capacity(dependencies.len());
        for dependency in dependencies {
            let parts: Vec<&str> = dependency.split(' ').collect();
            let candidates = by_name.get(parts[0]).map(Vec::as_slice).unwrap_or(&[]);
            let index = match parts.as_slice() {
                [_] if candidates.len() == 1 => candidates[0],
                [_] if candidates.is_empty() => {
                    return Err(refusal(
                        Some(*line),
                        format!("dependency {dependency} names no package of the lock"),
                    ));
                }
                [_] => {
                    return Err(refusal(
                        Some(*line),
                        format!("dependency {dependency} names several packages without a version"),
                    ));
                }
                [_, version] => {
                    match candidates
                        .iter()
                        .copied()
                        .find(|index| packages[*index].version == *version)
                    {
                        Some(index) => index,
                        None => {
                            return Err(refusal(
                                Some(*line),
                                format!("dependency {dependency} names no package of the lock"),
                            ));
                        }
                    }
                }
                _ => {
                    return Err(refusal(
                        Some(*line),
                        format!(
                            "dependency {dependency} is not of the form \"name\" or \"name \
                             version\"; the (source) form is outside format 1"
                        ),
                    ));
                }
            };
            if indices.contains(&index) {
                return Err(refusal(
                    Some(*line),
                    format!("dependency {dependency} is listed twice"),
                ));
            }
            indices.push(index);
        }
        resolved.push(indices);
    }
    for (package, dependencies) in packages.iter_mut().zip(resolved) {
        package.dependencies = dependencies;
    }
    Ok(CargoLock { packages, root })
}
