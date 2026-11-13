//! Strict reader of a flat `package-lock.json` of version 3.
//!
//! Every key is typed (`deny_unknown_fields`), every object key is unique,
//! every installed package has a version, an `https` registry URL without
//! user information, query or fragment and one SHA-512 integrity, and every
//! dependency resolves to an entry. The root and `front/package.json` declare
//! only `dependencies` and `devDependencies`, and no entry is marked
//! `devOptional`. Anything else refuses the file: format 1 never summarises a
//! lock it cannot read completely.
use super::LockRefusal;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};
use serde_json::error::Category;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    marker::PhantomData,
};

pub(super) const NPM_LOCK: &str = "front/package-lock.json";
pub(super) const NPM_MANIFEST: &str = "front/package.json";
const MAX_ENTRIES: usize = 16_384;
const MAX_NAME_BYTES: usize = 214;
const MODULES: &str = "node_modules/";

/// An object whose keys are unique, in file order. `serde_json` maps keep the
/// last of two equal keys, which would let a lock say two things at once.
struct UniqueMap<V>(Vec<(String, V)>);

struct UniqueVisitor<V>(PhantomData<V>);

impl<'de, V: Deserialize<'de>> Visitor<'de> for UniqueVisitor<V> {
    type Value = UniqueMap<V>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object without repeated keys")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut entries: Vec<(String, V)> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if entries.len() == MAX_ENTRIES {
                return Err(de::Error::custom(format!(
                    "an object holds more than {MAX_ENTRIES} keys"
                )));
            }
            if !seen.insert(key.clone()) {
                return Err(de::Error::custom(format!("key {key:?} is repeated")));
            }
            let value = map.next_value::<V>()?;
            entries.push((key, value));
        }
        Ok(UniqueMap(entries))
    }
}

impl<'de, V: Deserialize<'de>> Deserialize<'de> for UniqueMap<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(UniqueVisitor(PhantomData))
    }
}

impl<V> UniqueMap<V> {
    fn names(&self) -> BTreeSet<&str> {
        self.0.iter().map(|(name, _)| name.as_str()).collect()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LockFile {
    name: String,
    version: String,
    lockfile_version: u32,
    /// Known and admitted, not recorded.
    #[serde(rename = "requires")]
    _requires: Option<bool>,
    packages: UniqueMap<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PeerMeta {
    optional: Option<bool>,
}

/// Every key the lock format of npm writes for a flat entry and that format 1
/// knows. `link`, `inBundle`, `extraneous` and any other key are refused.
/// Keys whose field starts with `_` are admitted and not recorded.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Entry {
    name: Option<String>,
    version: Option<String>,
    resolved: Option<String>,
    integrity: Option<String>,
    license: Option<String>,
    dev: Option<bool>,
    optional: Option<bool>,
    /// Admitted only when absent or `false`: format 1 records an entry as dev,
    /// optional, both or neither, and does not interpret this marking.
    dev_optional: Option<bool>,
    #[serde(rename = "peer")]
    _peer: Option<bool>,
    os: Option<Vec<String>>,
    cpu: Option<Vec<String>>,
    libc: Option<Vec<String>>,
    #[serde(rename = "engines")]
    _engines: Option<serde::de::IgnoredAny>,
    #[serde(rename = "bin")]
    _bin: Option<serde::de::IgnoredAny>,
    #[serde(rename = "funding")]
    _funding: Option<serde::de::IgnoredAny>,
    #[serde(rename = "deprecated")]
    _deprecated: Option<String>,
    has_install_script: Option<bool>,
    dependencies: Option<UniqueMap<String>>,
    optional_dependencies: Option<UniqueMap<String>>,
    peer_dependencies: Option<UniqueMap<String>>,
    peer_dependencies_meta: Option<UniqueMap<PeerMeta>>,
    dev_dependencies: Option<UniqueMap<String>>,
}

/// The fields of `front/package.json` that are compared with the lock root,
/// and the two dependency kinds format 1 refuses there; its other fields are
/// not read.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    name: Option<String>,
    version: Option<String>,
    dependencies: Option<UniqueMap<String>>,
    dev_dependencies: Option<UniqueMap<String>>,
    optional_dependencies: Option<serde::de::IgnoredAny>,
    peer_dependencies: Option<serde::de::IgnoredAny>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NpmRoot {
    pub(super) name: String,
    pub(super) version: String,
    /// Indices into `PackageLock::packages` of the root's `dependencies` and
    /// `devDependencies`, resolved, sorted and unique.
    pub(super) dependencies: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NpmPackage {
    /// Full npm name, scope included.
    pub(super) name: String,
    pub(super) version: String,
    pub(super) resolved: String,
    /// Hexadecimal of the SHA-512 the integrity field states: 128 characters.
    pub(super) sha512_hex: String,
    pub(super) license: Option<String>,
    pub(super) dev: bool,
    pub(super) optional: bool,
    pub(super) os: Vec<String>,
    pub(super) cpu: Vec<String>,
    pub(super) libc: Vec<String>,
    pub(super) install_script: bool,
    /// Resolved `dependencies`, `optionalDependencies` and present peers,
    /// sorted and unique.
    pub(super) dependencies: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PackageLock {
    pub(super) lockfile_version: u32,
    pub(super) root: NpmRoot,
    /// Sorted by name.
    pub(super) packages: Vec<NpmPackage>,
}

fn refusal(file: &'static str, line: Option<usize>, reason: impl Into<String>) -> LockRefusal {
    LockRefusal {
        file,
        line,
        reason: reason.into(),
    }
}

fn refused(reason: impl Into<String>) -> LockRefusal {
    refusal(NPM_LOCK, None, reason)
}

/// Messages of `serde_json` data errors that name keys only: an unknown,
/// missing or duplicate field, and the two refusals of [`UniqueMap`].
const KEY_MESSAGES: [&str; 5] = [
    "unknown field `",
    "missing field `",
    "duplicate field `",
    "key \"",
    "an object holds more than ",
];

/// The refusal of a JSON file that does not read into the types of format 1.
/// Syntax messages and messages that name keys are kept. Any other data
/// error can quote the value it found, such as a URL with user information
/// placed in a boolean field, so it is replaced by a fixed text with its
/// column: no value of the file is repeated.
fn json_refusal(file: &'static str, error: &serde_json::Error) -> LockRefusal {
    let message = error.to_string();
    let names_keys = KEY_MESSAGES
        .iter()
        .any(|prefix| message.starts_with(prefix));
    let reason = match error.classify() {
        Category::Data if !names_keys => format!(
            "a field has a type or value outside format 1 at column {}; the value found is not \
             repeated",
            error.column()
        ),
        _ => message,
    };
    refusal(file, Some(error.line()), reason)
}

/// `[@scope/]name` of `[A-Za-z0-9._~-]`, at most 214 bytes.
fn valid_name(name: &str) -> bool {
    let plain = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._~-".contains(&byte))
    };
    if name.len() > MAX_NAME_BYTES {
        return false;
    }
    match name.strip_prefix('@') {
        Some(scoped) => match scoped.split_once('/') {
            Some((scope, rest)) => plain(scope) && plain(rest),
            None => false,
        },
        None => plain(name),
    }
}

/// An `https` URL whose authority has no user information and which carries
/// neither query nor fragment. A token placed in a path segment is not
/// detected.
fn safe_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    !authority.is_empty()
        && !authority.contains('@')
        && !url.contains('?')
        && !url.contains('#')
        && url
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'\\')
}

/// Hexadecimal of the one `sha512-<base64>` hash of an integrity field.
fn sha512_hex(integrity: &str) -> Option<String> {
    let encoded = integrity.strip_prefix("sha512-")?;
    if encoded.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return None;
    }
    let digest = STANDARD.decode(encoded).ok()?;
    (digest.len() == 64).then(|| hex::encode(digest))
}

pub(super) fn parse(lock: &[u8], manifest: &[u8]) -> Result<PackageLock, LockRefusal> {
    let file: LockFile =
        serde_json::from_slice(lock).map_err(|error| json_refusal(NPM_LOCK, &error))?;
    if file.lockfile_version != 3 {
        return Err(refused(format!(
            "lockfileVersion {} is outside format 1, which reads version 3 only",
            file.lockfile_version
        )));
    }
    let mut root = None;
    let mut entries: Vec<(String, Entry)> = Vec::new();
    for (key, entry) in file.packages.0 {
        if key.is_empty() {
            root = Some(entry);
            continue;
        }
        let Some(name) = key.strip_prefix(MODULES) else {
            return Err(refused(format!(
                "package key {key:?} is not of the form node_modules/<name>; linked and \
                 workspace layouts are outside format 1"
            )));
        };
        if name.contains("/node_modules/") || !valid_name(name) {
            return Err(refused(format!(
                "package key {key:?} is nested or names no valid package; format 1 reads a \
                 flat lock only"
            )));
        }
        if entry.name.as_deref().is_some_and(|named| named != name) {
            return Err(refused(format!(
                "package {name} is installed under another name; aliases are outside format 1"
            )));
        }
        entries.push((name.to_owned(), entry));
    }
    let Some(root) = root else {
        return Err(refused("the lock has no root package entry \"\""));
    };
    if root.name.as_deref() != Some(file.name.as_str())
        || root.version.as_deref() != Some(file.version.as_str())
    {
        return Err(refused(
            "the name and version of the root entry differ from those of the lock",
        ));
    }
    if root.optional_dependencies.is_some()
        || root.peer_dependencies.is_some()
        || root.peer_dependencies_meta.is_some()
    {
        return Err(refused(
            "the root entry declares optionalDependencies or peerDependencies; format 1 resolves \
             only the dependencies and devDependencies of the root",
        ));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let index: BTreeMap<String, usize> = entries
        .iter()
        .enumerate()
        .map(|(position, (name, _))| (name.clone(), position))
        .collect();
    let resolve = |owner: &str, names: &[&str]| -> Result<Vec<usize>, LockRefusal> {
        let mut resolved = BTreeSet::new();
        for name in names {
            let Some(position) = index.get(*name) else {
                return Err(refused(format!(
                    "{owner} depends on {name}, which has no node_modules/{name} entry"
                )));
            };
            resolved.insert(*position);
        }
        Ok(resolved.into_iter().collect())
    };
    let mut packages = Vec::with_capacity(entries.len());
    for (name, entry) in &entries {
        let missing = |field: &str| refused(format!("package {name} has no {field}"));
        let version = entry.version.clone().ok_or_else(|| missing("version"))?;
        let resolved = entry
            .resolved
            .clone()
            .ok_or_else(|| missing("resolved URL"))?;
        let integrity = entry
            .integrity
            .as_deref()
            .ok_or_else(|| missing("integrity"))?;
        if !safe_url(&resolved) {
            return Err(refused(format!(
                "the resolved URL of {name} is not an https URL without user information, \
                 query or fragment; such a URL is never recorded"
            )));
        }
        let Some(digest) = sha512_hex(integrity) else {
            return Err(refused(format!(
                "the integrity of {name} is not exactly one sha512-<base64> hash of 64 bytes"
            )));
        };
        if entry.dev_optional == Some(true) {
            return Err(refused(format!(
                "package {name} is marked devOptional; format 1 records an entry as dev, \
                 optional, both or neither and does not interpret this marking"
            )));
        }
        let mut names: Vec<&str> = Vec::new();
        for map in [&entry.dependencies, &entry.optional_dependencies]
            .into_iter()
            .flatten()
        {
            names.extend(map.0.iter().map(|(dependency, _)| dependency.as_str()));
        }
        let optional_peers: BTreeSet<&str> = entry
            .peer_dependencies_meta
            .iter()
            .flat_map(|meta| meta.0.iter())
            .filter(|(_, meta)| meta.optional == Some(true))
            .map(|(peer, _)| peer.as_str())
            .collect();
        for (peer, _) in entry
            .peer_dependencies
            .iter()
            .flat_map(|peers| peers.0.iter())
        {
            if index.contains_key(peer.as_str()) {
                names.push(peer.as_str());
            } else if !optional_peers.contains(peer.as_str()) {
                return Err(refused(format!(
                    "{name} requires the peer {peer}, which has no node_modules/{peer} entry"
                )));
            }
        }
        packages.push(NpmPackage {
            name: name.clone(),
            version,
            resolved,
            sha512_hex: digest,
            license: entry.license.clone(),
            dev: entry.dev == Some(true),
            optional: entry.optional == Some(true),
            os: entry.os.clone().unwrap_or_default(),
            cpu: entry.cpu.clone().unwrap_or_default(),
            libc: entry.libc.clone().unwrap_or_default(),
            install_script: entry.has_install_script == Some(true),
            dependencies: resolve(name, &names)?,
        });
    }
    let manifest: Manifest =
        serde_json::from_slice(manifest).map_err(|error| json_refusal(NPM_MANIFEST, &error))?;
    if manifest.optional_dependencies.is_some() || manifest.peer_dependencies.is_some() {
        return Err(refusal(
            NPM_MANIFEST,
            None,
            "the manifest declares optionalDependencies or peerDependencies; format 1 compares \
             only its dependencies and devDependencies with the lock root",
        ));
    }
    let lock_names = |map: &Option<UniqueMap<String>>| -> BTreeSet<String> {
        map.iter()
            .flat_map(|map| map.names())
            .map(str::to_owned)
            .collect()
    };
    if manifest.name.as_deref() != Some(file.name.as_str())
        || manifest.version.as_deref() != Some(file.version.as_str())
        || lock_names(&manifest.dependencies) != lock_names(&root.dependencies)
        || lock_names(&manifest.dev_dependencies) != lock_names(&root.dev_dependencies)
    {
        return Err(refused(
            "the root entry does not match front/package.json (name, version, dependencies or \
             devDependencies); the lock is stale",
        ));
    }
    let mut root_names: Vec<&str> = Vec::new();
    for map in [&root.dependencies, &root.dev_dependencies]
        .into_iter()
        .flatten()
    {
        root_names.extend(map.0.iter().map(|(dependency, _)| dependency.as_str()));
    }
    let root_dependencies = resolve(&file.name, &root_names)?;
    Ok(PackageLock {
        lockfile_version: file.lockfile_version,
        root: NpmRoot {
            name: file.name.clone(),
            version: file.version.clone(),
            dependencies: root_dependencies,
        },
        packages,
    })
}
