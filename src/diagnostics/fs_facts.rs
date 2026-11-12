//! `lstat` facts about a referenced entry, and the rule of its consumer.
//!
//! Nothing here opens a referenced file. A verdict is reached from metadata,
//! and by the product's own checks where one exists
//! (`Directory::open_existing`, `Directory::private_entry`, and the walk of
//! `PrivateFile::open` over the directories of a path). A referenced file
//! is judged by the rule of the command that reads it, never by one common
//! rule: the gateway token reader checks no owner, mode or link count
//! (`src/main.rs`, `read_token`), the private-file readers and the stores do
//! (`src/control_fs.rs`, `check_private_file`, `Directory::private_entry`).

use super::report::{EntryKind, Sidecar};
use crate::control_fs::{self, Directory};
use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    io::ErrorKind,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
};

/// What `lstat` says about one path.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Lookup {
    Absent,
    /// The metadata could not be read, for a reason other than absence.
    Unavailable,
    Found(Facts),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Facts {
    pub kind: EntryKind,
    /// Permission bits, with the set-ID and sticky bits.
    pub mode: u32,
    pub owner_is_inspector: bool,
    pub links: u64,
    /// Device and inode: the identity of the entry, not of a link target.
    pub identity: (u64, u64),
}

impl Facts {
    /// The mode as the four octal digits of the report.
    pub(crate) fn mode_text(&self) -> String {
        format!("{:04o}", self.mode)
    }

    /// Whether the mode lets an account other than the owner use the file.
    pub(crate) fn grants_group_or_other(&self) -> bool {
        self.mode & 0o077 != 0
    }
}

pub(crate) fn effective_uid() -> u32 {
    // SAFETY: geteuid has no pointers or preconditions.
    unsafe { libc::geteuid() }
}

/// `lstat` of one path: the last component is never followed. A path below a
/// regular file cannot exist, so it is absent like a missing entry.
pub(crate) fn lookup(path: &Path) -> Lookup {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            let kind = if file_type.is_file() {
                EntryKind::RegularFile
            } else if file_type.is_dir() {
                EntryKind::Directory
            } else if file_type.is_symlink() {
                EntryKind::SymbolicLink
            } else {
                EntryKind::Other
            };
            Lookup::Found(Facts {
                kind,
                mode: metadata.mode() & 0o7777,
                owner_is_inspector: metadata.uid() == effective_uid(),
                links: metadata.nlink(),
                identity: (metadata.dev(), metadata.ino()),
            })
        }
        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            Lookup::Absent
        }
        Err(_) => Lookup::Unavailable,
    }
}

pub(crate) fn exists(path: &Path) -> bool {
    matches!(lookup(path), Lookup::Found(_))
}

/// Verdict of the private-file rule on a regular file, without opening it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrivateVerdict {
    /// The product's own entry check accepted the file in a private parent.
    Satisfied,
    /// The reader refuses for certain: mode, link count or access entries.
    Violated,
    /// The reader refuses for certain the directories that lead to the file.
    PathRefused,
    /// Another account owns the file; its reader may run as that account.
    InspectorDiffers,
    /// The metadata conforms and the reader accepts the directories that
    /// lead to the file, but the parent is not a private directory, so the
    /// product check that needs one could not run. The reader checks the file
    /// system and the access entries of the file when it opens it.
    NotEvaluated,
}

/// The rule of `PrivateFile::open` (`src/control_fs.rs`, `check_private_file`):
/// a regular file owned by the effective UID, mode 0600, one link. `facts`
/// are those of a regular file.
pub(crate) fn private_file_rule(path: &Path, facts: &Facts) -> PrivateVerdict {
    if !facts.owner_is_inspector {
        return PrivateVerdict::InspectorDiffers;
    }
    if facts.mode != 0o600 || facts.links != 1 {
        return PrivateVerdict::Violated;
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return PrivateVerdict::NotEvaluated;
    };
    match Directory::open_existing(parent) {
        Ok(directory) => match directory.private_entry(name) {
            Ok(Some(_)) => PrivateVerdict::Satisfied,
            // The entry disappeared between the two looks.
            Ok(None) => PrivateVerdict::NotEvaluated,
            Err(_) => PrivateVerdict::Violated,
        },
        // The reader needs no private parent: it is asked about the
        // directories themselves.
        Err(_) if reader_accepts_directory(parent) => PrivateVerdict::NotEvaluated,
        Err(_) => PrivateVerdict::PathRefused,
    }
}

/// The refusal that `PrivateFile::open` gives to a bound above its ceiling
/// (`src/control_fs.rs`, `read_private`). The reader reaches it once it has
/// walked the directory of the path, and before it opens anything in it.
pub const BOUND_REFUSAL: &str = "private file bound exceeds the supported size limit";

/// A file name that no supported file system can hold: longer than the 255
/// bytes of a name.
const NAME_NO_FILE_CAN_HAVE: [u8; 256] = [b'x'; 256];

/// Whether the private-file readers accept the directories that lead to a
/// file of `directory`: each one owned by root or by the account, not
/// writable by group or others, opened without following a link, on a
/// supported file system (`src/control_fs.rs`, `walk`).
///
/// The rule is not restated: the reader itself is asked, through its own
/// entry point, for a file it cannot open. It walks the directory exactly as
/// it does for a real file, then refuses the bound it is given, which is
/// above its ceiling, before it opens anything. The name it is given is not
/// the one of the file, and no file can have it: whatever the reader does
/// after its walk, no credential is opened. Only directories are opened.
pub(crate) fn reader_accepts_directory(directory: &Path) -> bool {
    let Ok(directory) = control_fs::absolute_path(directory) else {
        return false;
    };
    let probe = directory.join(OsStr::from_bytes(&NAME_NO_FILE_CAN_HAVE));
    match control_fs::PrivateFile::open(&probe, u64::MAX) {
        // The walk passed: the next step of the reader is the bound.
        Err(error) => error.to_string() == BOUND_REFUSAL,
        // Not reachable: no file has that name and no read has that bound.
        Ok(_) => true,
    }
}

/// State of a store path under the rule of the stores
/// (`src/audit/storage.rs`, `StoreStorage::prepare`). No store, sidecar or
/// lock file is opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StoreState {
    Present {
        sidecars: Vec<Sidecar>,
    },
    /// No store and no sidecar; the directory that would hold it may be absent.
    Absent,
    /// Sidecars without their store: the store refuses.
    OrphanSidecars(Vec<Sidecar>),
    /// The path is not one the store accepts: a parent traversal.
    PathRefused,
    /// The store accepts no file of that name.
    NameRefused,
    ParentNotPrivate,
    /// The store or one of its sidecars is refused by the private-entry rule.
    PostureRefused,
    /// The store, or the directory that holds it, belongs to another account.
    InspectorDiffers,
    Unavailable,
}

const SIDECARS: [(&str, Sidecar); 3] = [
    ("-wal", Sidecar::Wal),
    ("-shm", Sidecar::Shm),
    ("-journal", Sidecar::Journal),
];

/// The sidecar paths of a store path.
pub(crate) fn sidecar_paths(store: &Path) -> Vec<PathBuf> {
    SIDECARS
        .iter()
        .map(|(suffix, _)| {
            let mut name = OsString::from(store.as_os_str());
            name.push(suffix);
            PathBuf::from(name)
        })
        .collect()
}

/// The prefix of the names that the product reserves beside a store
/// (`src/control_fs.rs`, `sqlite_initialization_lock`).
const RESERVED_STORE_PREFIX: &[u8] = b".ai-guard-sqlite-";

/// The file names that the stores refuse before they look at the directory
/// (`src/audit/storage.rs`, `StoreStorage::prepare`): a name that is not
/// ASCII, a name with the reserved prefix, or one that ends like a SQLite
/// sidecar, whatever the case. The rule is restated here; a test binds it to
/// the gateway and to the product source.
fn store_name_refused(name: &OsStr) -> bool {
    let name = name.as_bytes();
    let lower = name.to_ascii_lowercase();
    !name.is_ascii()
        || lower.starts_with(RESERVED_STORE_PREFIX)
        || SIDECARS
            .iter()
            .any(|(suffix, _)| lower.ends_with(suffix.as_bytes()))
}

pub(crate) fn store_state(path: &Path) -> StoreState {
    let Ok(path) = control_fs::absolute_path(path) else {
        return StoreState::PathRefused;
    };
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return StoreState::PathRefused;
    };
    if store_name_refused(name) {
        return StoreState::NameRefused;
    }
    let parent_facts = match lookup(parent) {
        Lookup::Absent => return StoreState::Absent,
        Lookup::Unavailable => return StoreState::Unavailable,
        Lookup::Found(facts) => facts,
    };
    let Ok(directory) = Directory::open_existing(parent) else {
        return if parent_facts.owner_is_inspector {
            StoreState::ParentNotPrivate
        } else {
            StoreState::InspectorDiffers
        };
    };
    let store = match directory.private_entry(name) {
        Ok(store) => store,
        Err(_) => {
            return match lookup(&path) {
                Lookup::Found(facts) if !facts.owner_is_inspector => StoreState::InspectorDiffers,
                _ => StoreState::PostureRefused,
            };
        }
    };
    let mut sidecars = Vec::new();
    for (suffix, sidecar) in SIDECARS {
        let mut sidecar_name = name.to_os_string();
        sidecar_name.push(suffix);
        match directory.private_entry(&sidecar_name) {
            Ok(Some(_)) => sidecars.push(sidecar),
            Ok(None) => {}
            Err(_) => return StoreState::PostureRefused,
        }
    }
    match (store, sidecars.is_empty()) {
        (Some(_), _) => StoreState::Present { sidecars },
        (None, true) => StoreState::Absent,
        (None, false) => StoreState::OrphanSidecars(sidecars),
    }
}

/// Paths this tool must never open for content: key files, setup records and
/// stores. A file is compared with them before it is opened: by path, by
/// identity, so that a second name of the same file is recognised, and by the
/// directories a layout reserves for its keys and stores, so that a file no
/// inspected reference names is covered as well. A path is registered as it
/// is written and as it resolves, with the identity of both.
#[derive(Default)]
pub(crate) struct NeverOpened {
    paths: BTreeSet<PathBuf>,
    identities: BTreeSet<(u64, u64)>,
    directories: Vec<PathBuf>,
}

impl NeverOpened {
    pub(crate) fn add(&mut self, path: &Path) {
        if let Lookup::Found(facts) = lookup(path) {
            self.identities.insert(facts.identity);
        }
        // A reference that is itself a link names its target as well. Its
        // readers refuse the link, and the file behind it would otherwise be
        // opened by an option that names it directly. Resolving reads link
        // targets and metadata only.
        if let Ok(resolved) = path.canonicalize()
            && resolved != path
        {
            if let Lookup::Found(facts) = lookup(&resolved) {
                self.identities.insert(facts.identity);
            }
            self.paths.insert(resolved);
        }
        self.paths.insert(path.to_owned());
    }

    /// A store, and the sidecars that hold part of its content.
    pub(crate) fn add_store(&mut self, store: &Path) {
        for sidecar in sidecar_paths(store) {
            self.add(&sidecar);
        }
        self.add(store);
    }

    /// Everything below a directory that a layout reserves for keys or
    /// stores, under the name it was given and under its resolved name.
    pub(crate) fn reserve(&mut self, directory: &Path) {
        if let Ok(resolved) = directory.canonicalize()
            && resolved != directory
        {
            self.directories.push(resolved);
        }
        self.directories.push(directory.to_owned());
    }

    fn names(&self, path: &Path) -> bool {
        self.paths.contains(path)
            || self
                .directories
                .iter()
                .any(|directory| path.starts_with(directory))
            || matches!(lookup(path), Lookup::Found(facts) if self.identities.contains(&facts.identity))
    }

    /// Whether a path is one of these files, as written or as resolved. The
    /// private-file readers follow no link, but the gateway's read of a
    /// registry follows the links of the directories that lead to it, and the
    /// engine approval resolves every link. Resolving a path reads link
    /// targets and metadata only: no file is opened and no directory listed.
    pub(crate) fn contains(&self, path: &Path) -> bool {
        self.names(path)
            || path
                .canonicalize()
                .is_ok_and(|resolved| resolved != path && self.names(&resolved))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    /// The source of the reader that `reader_accepts_directory` asks.
    const READER_SOURCE: &str = include_str!("../control_fs.rs");

    // The question rests on the order of the reader's own steps. A reader
    // that is reworded or reordered fails here, before any verdict changes.
    #[test]
    fn the_reader_walks_the_directories_before_it_can_open_a_file() {
        assert_eq!(
            READER_SOURCE
                .matches(&format!("\"{BOUND_REFUSAL}\""))
                .count(),
            1,
            "the refusal of a bound above the ceiling is one literal of the reader"
        );
        let open = READER_SOURCE
            .find("pub fn open(path: &Path, max_bytes: u64) -> Result<Self> {")
            .map(|start| &READER_SOURCE[start..])
            .expect("the entry point of the private-file reader");
        let walk = open
            .find("walk(parent, false, false)")
            .expect("the reader walks the directory of the path");
        let read = open
            .find("read_private(anchor, name, max_bytes)")
            .expect("the reader reads through the walked directory");
        assert!(walk < read, "the walk comes before the read");
        let read_private = READER_SOURCE
            .find("fn read_private(")
            .map(|start| &READER_SOURCE[start..])
            .expect("the bounded read of the reader");
        let bound = read_private
            .find(BOUND_REFUSAL)
            .expect("the read refuses a bound above the ceiling");
        let opened = read_private
            .find("open_file(")
            .expect("the read opens the file");
        assert!(
            bound < opened,
            "the bound is refused before the file is opened"
        );
    }

    #[test]
    fn the_reader_is_asked_about_directories_and_opens_nothing() {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = root.path().canonicalize().unwrap();
        let directory = |name: &str, mode: u32| {
            let path = root.join(name);
            fs::create_dir(&path).unwrap();
            let file = path.join("file");
            fs::write(&file, "synthetic").unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            path
        };
        let private = directory("private", 0o700);
        let readable = directory("readable", 0o755);
        let group_writable = directory("group-writable", 0o775);
        let link = root.join("link");
        std::os::unix::fs::symlink(&private, &link).unwrap();

        for (name, path, accepted) in [
            ("a private directory", &private, true),
            ("a directory that others can read", &readable, true),
            (
                "a directory that its group can write",
                &group_writable,
                false,
            ),
            ("a link to a private directory", &link, false),
        ] {
            assert_eq!(reader_accepts_directory(path), accepted, "{name}");
            // The reader itself, on a real file of that directory.
            assert_eq!(
                control_fs::PrivateFile::open(&path.join("file"), 16).is_ok(),
                accepted,
                "{name}: the reader agrees"
            );
        }
        assert!(!reader_accepts_directory(&root.join("absent")));
        // The question created nothing and left each directory as it was.
        for path in [&private, &readable, &group_writable] {
            assert_eq!(fs::read_dir(path).unwrap().count(), 1);
        }
    }
}
