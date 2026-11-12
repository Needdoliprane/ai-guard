//! No-mutation and never-read oracles over a scenario root.
//!
//! The manifest records, for the root entry and every entry below it: type,
//! mode, owner, group, link count, device, inode, size, modification and
//! change times with nanoseconds, the SHA-256 of regular files and the target
//! of links. Access time is excluded: it is the separate never-read oracle.

use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub kind: &'static str,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub links: u64,
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub modified: (i64, i64),
    pub changed: (i64, i64),
    pub sha256: Option<String>,
    pub link_target: Option<PathBuf>,
}

/// Relative path (empty for the root itself) to entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest(pub BTreeMap<PathBuf, Entry>);

fn walk(root: &Path, relative: &Path, paths: &mut Vec<PathBuf>) {
    paths.push(relative.to_owned());
    let path = root.join(relative);
    if !fs::symlink_metadata(&path).unwrap().is_dir() {
        return;
    }
    let mut names: Vec<_> = fs::read_dir(&path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    names.sort();
    for name in names {
        walk(root, &relative.join(name), paths);
    }
}

/// Every entry of the tree, the root first. Lists directories.
pub fn paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    walk(root, Path::new(""), &mut paths);
    paths
}

/// Content hashes of the regular files. Reads every file.
pub fn hashes(root: &Path, paths: &[PathBuf]) -> BTreeMap<PathBuf, String> {
    paths
        .iter()
        .filter(|relative| fs::symlink_metadata(root.join(relative)).unwrap().is_file())
        .map(|relative| {
            let bytes = fs::read(root.join(relative)).unwrap();
            (relative.clone(), hex::encode(Sha256::digest(bytes)))
        })
        .collect()
}

/// Metadata of the given entries, from `lstat` only: no file is read and no
/// directory is listed, so access times are left as they are.
pub fn metadata_only(
    root: &Path,
    paths: &[PathBuf],
    hashes: &BTreeMap<PathBuf, String>,
) -> Manifest {
    Manifest(
        paths
            .iter()
            .map(|relative| {
                let path = root.join(relative);
                let metadata = fs::symlink_metadata(&path).unwrap();
                let kind = if metadata.is_file() {
                    "file"
                } else if metadata.is_dir() {
                    "directory"
                } else if metadata.file_type().is_symlink() {
                    "symlink"
                } else {
                    "other"
                };
                let entry = Entry {
                    kind,
                    mode: metadata.mode(),
                    uid: metadata.uid(),
                    gid: metadata.gid(),
                    links: metadata.nlink(),
                    device: metadata.dev(),
                    inode: metadata.ino(),
                    size: metadata.size(),
                    modified: (metadata.mtime(), metadata.mtime_nsec()),
                    changed: (metadata.ctime(), metadata.ctime_nsec()),
                    sha256: hashes.get(relative).cloned(),
                    link_target: (kind == "symlink").then(|| fs::read_link(&path).unwrap()),
                };
                (relative.clone(), entry)
            })
            .collect(),
    )
}

/// The full manifest. Lists directories and reads files: take it before the
/// access times are armed, or after they were compared.
pub fn take(root: &Path) -> Manifest {
    let paths = paths(root);
    let hashes = hashes(root, &paths);
    metadata_only(root, &paths, &hashes)
}

/// Readable differences between two manifests; empty when they are equal.
pub fn differences(before: &Manifest, after: &Manifest) -> Vec<String> {
    let mut differences = Vec::new();
    for (path, entry) in &before.0 {
        match after.0.get(path) {
            None => differences.push(format!("removed: {}", path.display())),
            Some(other) if other != entry => {
                differences.push(format!("changed: {}", path.display()));
            }
            Some(_) => {}
        }
    }
    for path in after.0.keys() {
        if !before.0.contains_key(path) {
            differences.push(format!("created: {}", path.display()));
        }
    }
    differences
}

pub fn assert_unchanged(before: &Manifest, after: &Manifest, context: &str) {
    let differences = differences(before, after);
    assert!(
        differences.is_empty(),
        "the inspected tree changed ({context}): {differences:?}"
    );
}

/// Access times of the given entries, from `lstat` only.
pub fn access_times(root: &Path, paths: &[PathBuf]) -> BTreeMap<PathBuf, (i64, i64)> {
    paths
        .iter()
        .map(|relative| {
            let metadata = fs::symlink_metadata(root.join(relative)).unwrap();
            (relative.clone(), (metadata.atime(), metadata.atime_nsec()))
        })
        .collect()
}

/// Arm the never-read oracle: set the access time of every regular file and
/// directory to ten seconds before its modification time, so that the next
/// read or listing moves it even on a `relatime` mount. Links are skipped.
/// This changes the change time: take the metadata afterwards.
pub fn arm_access_times(root: &Path, paths: &[PathBuf]) {
    for relative in paths {
        let path = root.join(relative);
        let metadata = fs::symlink_metadata(&path).unwrap();
        if !(metadata.is_file() || metadata.is_dir()) {
            continue;
        }
        let modified = metadata.modified().unwrap();
        let accessed = modified
            .checked_sub(Duration::from_secs(10))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let file = fs::File::open(&path).unwrap();
        file.set_times(fs::FileTimes::new().set_accessed(accessed))
            .unwrap();
    }
}
