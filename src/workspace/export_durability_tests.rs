//! Bounded observations and faults follow actual filesystem operations. A hook
//! can fail only after its matching real sync or exclusive rename succeeded.
use super::*;
use std::{
    cell::RefCell,
    ffi::OsStr,
    os::unix::fs::{MetadataExt, PermissionsExt},
};

const MAX_TRACE: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    FileFlushed,
    FileModeApplied,
    FileSynced,
    DirectoryCreated,
    DirectorySynced,
    StagingSynced,
    RenamedExclusive,
    ParentSynced,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Facts {
    device: u64,
    inode: u64,
    mode: u32,
    directory: bool,
}
impl Facts {
    fn of(file: &File) -> Option<Self> {
        let metadata = file.metadata().ok()?;
        Some(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode() & 0o777,
            directory: metadata.is_dir(),
        })
    }
    fn identity(self) -> (u64, u64) {
        (self.device, self.inode)
    }
}
#[derive(Debug)]
struct Observation {
    stage: Stage,
    relative: PathBuf,
    facts: Option<Facts>,
}
struct Fault {
    stage: Stage,
    relative: PathBuf,
}
#[derive(Default)]
struct Probe {
    events: Vec<Observation>,
    overflow: bool,
    fault: Option<Fault>,
}
thread_local! {
    static PROBE: RefCell<Option<Probe>> = const { RefCell::new(None) };
}

/// Non-decision-making observer. A missing/overflowed
/// observation fails the test; it never changes the filesystem operation.
pub(super) fn record(stage: Stage, relative: &Path, file: &File) {
    PROBE.with(|cell| {
        let mut state = cell.borrow_mut();
        let Some(probe) = state.as_mut() else {
            return;
        };
        if probe.events.len() >= MAX_TRACE {
            probe.overflow = true;
            return;
        }
        probe.events.push(Observation {
            stage,
            relative: relative.to_owned(),
            facts: Facts::of(file),
        });
    });
}

/// Call only AFTER the actual matching sync/rename succeeds.
pub(super) fn after_barrier(stage: Stage, relative: &Path, file: &File) -> Result<()> {
    record(stage, relative, file);
    let fail = PROBE.with(|cell| {
        let mut state = cell.borrow_mut();
        let Some(probe) = state.as_mut() else {
            return false;
        };
        if probe
            .fault
            .as_ref()
            .is_some_and(|fault| fault.stage == stage && fault.relative == relative)
        {
            probe.fault = None;
            true
        } else {
            false
        }
    });
    if fail {
        return Err(
            std::io::Error::other("synthetic failure after an actual export barrier").into(),
        );
    }
    Ok(())
}

struct Scope {
    active: bool,
}
impl Scope {
    fn begin() -> Self {
        PROBE.with(|cell| {
            let mut state = cell.borrow_mut();
            assert!(
                state.is_none(),
                "one export observation scope per test thread"
            );
            *state = Some(Probe::default());
        });
        Self { active: true }
    }
    fn finish(mut self) -> Probe {
        self.active = false;
        PROBE.with(|cell| {
            cell.borrow_mut()
                .take()
                .expect("live export observation scope")
        })
    }
    fn fail_after(stage: Stage, relative: &Path) -> Self {
        let scope = Self::begin();
        PROBE.with(|cell| {
            cell.borrow_mut().as_mut().unwrap().fault = Some(Fault {
                stage,
                relative: relative.to_owned(),
            });
        });
        scope
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        if self.active {
            PROBE.with(|cell| *cell.borrow_mut() = None);
        }
    }
}

struct Fixture {
    _temporary: tempfile::TempDir,
    source_root: PathBuf,
    original: PathBuf,
    parent: PathBuf,
    archive: Vec<u8>,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let source_root = base.join("sources");
        let original = source_root.join("project");
        let parent = base.join("exports");
        for directory in [&source_root, &original, &parent] {
            fs::create_dir(directory).unwrap();
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(original.join("witness.txt"), b"original remains unchanged").unwrap();
        let mut archive = tar::Builder::new(Vec::new());
        append(
            &mut archive,
            "root.txt",
            tar::EntryType::Regular,
            0o644,
            b"root payload",
        );
        // Neither implicit parent appears as a directory header in the archive.
        append(
            &mut archive,
            "implicit/a/tool.bin",
            tar::EntryType::Regular,
            0o755,
            b"synthetic executable payload",
        );
        append(&mut archive, "empty", tar::EntryType::Directory, 0o755, b"");
        Self {
            _temporary: temporary,
            source_root,
            original,
            parent,
            archive: archive.into_inner().unwrap(),
        }
    }
    fn assert_contents(&self, destination: &Path, manifest: &Manifest) {
        assert_eq!(manifest.len(), 2);
        for (relative, expected, mode) in [
            ("root.txt", b"root payload".as_slice(), 0o644),
            (
                "implicit/a/tool.bin",
                b"synthetic executable payload".as_slice(),
                0o644,
            ),
        ] {
            let path = destination.join(relative);
            assert_eq!(fs::read(&path).unwrap(), expected);
            assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, mode);
            assert_eq!(
                manifest[relative].sha256,
                hex::encode(Sha256::digest(expected))
            );
        }
        assert!(destination.join("empty").is_dir());
        assert_eq!(
            fs::read(self.original.join("witness.txt")).unwrap(),
            b"original remains unchanged"
        );
    }
}
fn append(
    builder: &mut tar::Builder<Vec<u8>>,
    path: &str,
    kind: tar::EntryType,
    mode: u32,
    bytes: &[u8],
) {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_mode(mode);
    header.set_size(bytes.len() as u64);
    header.set_mtime(0);
    header.set_cksum();
    builder.append_data(&mut header, path, bytes).unwrap();
}

fn last(probe: &Probe, stage: Stage, relative: &str) -> Option<usize> {
    probe
        .events
        .iter()
        .rposition(|event| event.stage == stage && event.relative == Path::new(relative))
}
fn facts_at(path: &Path) -> Facts {
    Facts::of(&File::open(path).unwrap()).unwrap()
}

fn assert_order(probe: &Probe, destination: &Path, parent: &Path) {
    assert!(!probe.overflow, "bounded export trace overflowed");
    let name = destination.file_name().unwrap().to_str().unwrap();
    let renamed = last(probe, Stage::RenamedExclusive, name)
        .expect("positive control: actual exclusive rename was observed");
    assert_eq!(probe.events[renamed].facts, Some(facts_at(destination)));
    let mut missing = Vec::new();
    for relative in ["root.txt", "implicit/a/tool.bin"] {
        let flushed = last(probe, Stage::FileFlushed, relative)
            .expect("positive control: real file flush observed");
        let mode = last(probe, Stage::FileModeApplied, relative)
            .expect("positive control: final file mode observed");
        assert!(flushed < mode && mode < renamed);
        let expected = facts_at(&destination.join(relative));
        assert_eq!(
            probe.events[flushed].facts.map(Facts::identity),
            Some(expected.identity())
        );
        assert_eq!(probe.events[mode].facts, Some(expected));
        if let Some(synced) = last(probe, Stage::FileSynced, relative) {
            assert!(
                mode < synced && synced < renamed,
                "file sync must follow final mode and precede publication"
            );
            assert_eq!(probe.events[synced].facts, Some(expected));
        } else {
            missing.push(format!("FileSynced({relative})"));
        }
    }
    let mut directories = BTreeMap::new();
    for relative in ["implicit/a", "implicit", "empty"] {
        if let Some(synced) = last(probe, Stage::DirectorySynced, relative) {
            assert!(synced < renamed, "directory sync must precede publication");
            assert_eq!(
                probe.events[synced].facts,
                Some(facts_at(&destination.join(relative)))
            );
            directories.insert(relative, synced);
        } else {
            missing.push(format!("DirectorySynced({relative})"));
        }
    }
    let staging = last(probe, Stage::StagingSynced, "");
    if let Some(staging) = staging {
        assert!(staging < renamed);
        assert_eq!(probe.events[staging].facts, Some(facts_at(destination)));
        for directory in directories.values() {
            assert!(*directory < staging);
        }
    } else {
        missing.push("StagingSynced".into());
    }
    if let (Some(child), Some(parent)) =
        (directories.get("implicit/a"), directories.get("implicit"))
    {
        assert!(
            child < parent,
            "implicit directories must sync from leaves upwards"
        );
    }
    if let (Some(file), Some(directory)) = (
        last(probe, Stage::FileSynced, "implicit/a/tool.bin"),
        directories.get("implicit/a"),
    ) {
        assert!(
            file < *directory,
            "file data and mode precede the containing directory barrier"
        );
    }
    if let (Some(file), Some(staging)) = (last(probe, Stage::FileSynced, "root.txt"), staging) {
        assert!(file < staging);
    }
    if let Some(parent_synced) = last(probe, Stage::ParentSynced, "") {
        assert!(
            renamed < parent_synced,
            "parent publication barrier follows rename and precedes the returned Ok"
        );
        assert_eq!(probe.events[parent_synced].facts, Some(facts_at(parent)));
    } else {
        missing.push("ParentSynced".into());
    }
    assert!(
        missing.is_empty(),
        "export returned Ok without required durable barriers: {missing:?}; actual trace: {:?}",
        probe.events
    );
}

#[test]
fn legacy_export_requires_durable_barriers_before_success() {
    let fixture = Fixture::new();
    // The read-only scanner remains write-free and does not emit sync claims.
    let scan_scope = Scope::begin();
    let scanned = scan_workspace_archive(fixture.archive.as_slice(), Limits::default()).unwrap();
    let scan = scan_scope.finish();
    assert!(scan.events.is_empty() && !scan.overflow);
    let destination = fixture.parent.join("legacy-export");
    let scope = Scope::begin();
    let manifest = extract_workspace(
        fixture.archive.as_slice(),
        &destination,
        &fixture.original,
        Limits::default(),
    )
    .expect("real legacy archive extraction succeeds after durable publication");
    let trace = scope.finish();
    assert_eq!(manifest.len(), scanned.len());
    for (path, exported) in &manifest {
        assert_eq!(exported.sha256, scanned[path].sha256);
        assert_eq!(exported.bytes, scanned[path].bytes);
        assert_eq!(exported.mode, Some(0o644));
    }
    fixture.assert_contents(&destination, &manifest);
    assert_order(&trace, &destination, &fixture.parent);
}

#[test]
fn authorized_export_requires_durable_barriers_before_success() {
    let fixture = Fixture::new();
    let parent = AuthorizedExportParent::open(
        &fixture.parent,
        std::slice::from_ref(&fixture.parent),
        std::slice::from_ref(&fixture.source_root),
    )
    .unwrap();
    let destination = fixture.parent.join("authorized-export");
    let scope = Scope::begin();
    let manifest = parent
        .extract(
            fixture.archive.as_slice(),
            OsStr::new("authorized-export"),
            Limits::default(),
        )
        .expect("real authorized archive extraction succeeds after durable publication");
    let trace = scope.finish();
    fixture.assert_contents(&destination, &manifest);
    assert_order(&trace, &destination, &fixture.parent);
}

#[test]
fn prepublication_barrier_faults_refuse_success_and_clean_only_staging() {
    let fixture = Fixture::new();
    let witness = fixture.parent.join("unrelated.txt");
    fs::write(&witness, b"preserve unrelated entry").unwrap();
    for (stage, relative) in [
        (Stage::FileSynced, "root.txt"),
        (Stage::DirectorySynced, "implicit/a"),
    ] {
        let destination = fixture.parent.join("refused-export");
        let scope = Scope::fail_after(stage, Path::new(relative));
        let error = extract_workspace(
            fixture.archive.as_slice(),
            &destination,
            &fixture.original,
            Limits::default(),
        )
        .unwrap_err();
        let trace = scope.finish();
        assert!(!error.is::<ExportPublicationUnknown>());
        assert!(
            trace.fault.is_none(),
            "fault followed the actual requested barrier"
        );
        assert!(last(&trace, stage, relative).is_some());
        assert!(
            !trace
                .events
                .iter()
                .any(|event| event.stage == Stage::RenamedExclusive)
        );
        assert!(!destination.exists());
        assert_eq!(fs::read(&witness).unwrap(), b"preserve unrelated entry");
        assert_eq!(fs::read_dir(&fixture.parent).unwrap().count(), 1);
        assert_eq!(
            fs::read(fixture.original.join("witness.txt")).unwrap(),
            b"original remains unchanged"
        );
    }
}

#[test]
fn postpublication_barrier_faults_preserve_destination_and_refuse_retry() {
    let fixture = Fixture::new();
    let parent = AuthorizedExportParent::open(
        &fixture.parent,
        std::slice::from_ref(&fixture.parent),
        std::slice::from_ref(&fixture.source_root),
    )
    .unwrap();
    let scanned = scan_workspace_archive(fixture.archive.as_slice(), Limits::default()).unwrap();
    for (stage, name, relative) in [
        (Stage::RenamedExclusive, "rename-unknown", "rename-unknown"),
        (Stage::ParentSynced, "parent-sync-unknown", ""),
    ] {
        let destination = fixture.parent.join(name);
        let scope = Scope::fail_after(stage, Path::new(relative));
        let error = parent
            .extract(
                fixture.archive.as_slice(),
                OsStr::new(name),
                Limits::default(),
            )
            .unwrap_err();
        let trace = scope.finish();
        assert!(
            error.is::<ExportPublicationUnknown>(),
            "postrename error: {error:#}"
        );
        assert!(trace.fault.is_none());
        assert!(last(&trace, stage, relative).is_some());
        assert!(last(&trace, Stage::RenamedExclusive, name).is_some());
        assert_eq!(
            last(&trace, Stage::ParentSynced, "").is_some(),
            stage == Stage::ParentSynced
        );
        // extract has returned and dropped Staging: its old FD must not have
        // cleared any part of the newly published tree.
        fixture.assert_contents(&destination, &scanned);
        let identity = facts_at(&destination).identity();
        let retry_scope = Scope::begin();
        assert!(
            parent
                .extract(
                    fixture.archive.as_slice(),
                    OsStr::new(name),
                    Limits::default()
                )
                .is_err()
        );
        let retry = retry_scope.finish();
        assert!(
            retry.events.is_empty(),
            "retry must refuse before writing or renaming"
        );
        assert_eq!(facts_at(&destination).identity(), identity);
        fixture.assert_contents(&destination, &scanned);
    }
    assert_eq!(fs::read_dir(&fixture.parent).unwrap().count(), 2);
}

#[test]
fn implicit_directory_and_depth_limits_precede_entry_creation() {
    let fixture = Fixture::new();
    let limits = Limits {
        max_files: 2,
        ..Limits::default()
    };
    let mut accepted = tar::Builder::new(Vec::new());
    append(
        &mut accepted,
        "a/b/file",
        tar::EntryType::Regular,
        0o644,
        b"bounded",
    );
    let accepted = accepted.into_inner().unwrap();
    let destination = fixture.parent.join("within-bound");
    let manifest =
        extract_workspace(accepted.as_slice(), &destination, &fixture.original, limits).unwrap();
    assert_eq!(manifest.len(), 1);
    assert_eq!(fs::read(destination.join("a/b/file")).unwrap(), b"bounded");
    for (name, path, limits) in [
        ("too-many-parents", "a/b/c/file".to_owned(), limits),
        (
            "too-deep",
            format!("{}file", "d/".repeat(MAX_EXPORT_DEPTH)),
            Limits {
                max_files: MAX_EXPORT_DEPTH + 1,
                ..Limits::default()
            },
        ),
    ] {
        let mut archive = tar::Builder::new(Vec::new());
        append(
            &mut archive,
            &path,
            tar::EntryType::Regular,
            0o644,
            b"refused",
        );
        let archive = archive.into_inner().unwrap();
        let scope = Scope::begin();
        assert!(scan_workspace_archive(archive.as_slice(), limits).is_err());
        let destination = fixture.parent.join(name);
        assert!(
            extract_workspace(archive.as_slice(), &destination, &fixture.original, limits).is_err()
        );
        let trace = scope.finish();
        assert!(
            trace.events.is_empty(),
            "reject the whole entry before creating implicit parents"
        );
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(&fixture.parent).unwrap().count(), 1);
    }
}
