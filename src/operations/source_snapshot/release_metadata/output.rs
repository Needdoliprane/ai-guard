//! A new private destination for the three release documents.
//!
//! The parent must already exist as a private directory (owned by the
//! effective UID, mode exactly 0700, ancestors owned by root or the effective
//! UID without group/other write unless root-owned and sticky). The documents
//! are written into `.<leaf>.release-staging`, synchronised, then the staging
//! directory is renamed to `<leaf>` without replacing anything, and the
//! published directory is listed and read back. Nothing that exists is
//! overwritten, and nothing is removed on any path: a failure keeps what was
//! written where it is and names it.
use super::super::capture::shown;
use crate::control_fs::{self, Directory, publication::PublicationFailure};
use anyhow::{Context, Result, anyhow, bail, ensure};
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

const STAGING_SUFFIX: &str = ".release-staging";
const MAX_LEAF_BYTES: usize = 96;
const PARENT_RULE: &str = "the parent of --output must be an existing directory owned by you with \
    mode exactly 0700, whose ancestors belong to root or to you and are not writable by group or \
    others (create it with mkdir -m 700 <parent>)";

#[cfg(test)]
pub(super) type BeforePublish = Box<dyn FnOnce()>;
#[cfg(test)]
thread_local! {
    /// Runs once, on this thread, after the documents were staged and
    /// synchronised and before the final name is last checked absent.
    pub(super) static BEFORE_FINAL_CHECK: std::cell::RefCell<Option<BeforePublish>> =
        const { std::cell::RefCell::new(None) };
    /// Runs once, on this thread, after the final name was last seen absent
    /// and before the staging directory is renamed to it.
    pub(super) static BEFORE_PUBLISH: std::cell::RefCell<Option<BeforePublish>> =
        const { std::cell::RefCell::new(None) };
}

/// The place a manifest is published at. Nothing has been created.
pub(super) struct Destination {
    parent: Directory,
    leaf: String,
    staging: String,
    /// The output as the operator gave it, for messages.
    given: PathBuf,
}

impl Destination {
    fn final_path(&self) -> PathBuf {
        self.parent.path().join(&self.leaf)
    }

    fn staging_path(&self) -> PathBuf {
        self.parent.path().join(&self.staging)
    }

    /// The output as the operator gave it, escaped.
    pub(super) fn shown(&self) -> String {
        shown(&self.given)
    }

    /// What may exist when the publication step ended without a result: the
    /// rename may or may not have happened, so both names are given and
    /// neither is removed.
    pub(super) fn unknown_publication(&self) -> String {
        format!(
            "the output may exist at {} or at {}; ai-guard removes neither name; run release \
             verify --manifest <that directory> before using it",
            shown(&self.final_path()),
            shown(&self.staging_path())
        )
    }
}

/// `[A-Za-z0-9._-]{1,96}`, not starting with a dot.
fn valid_leaf(leaf: &str) -> bool {
    (1..=MAX_LEAF_BYTES).contains(&leaf.len())
        && !leaf.starts_with('.')
        && leaf
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

/// Opens the private parent of `output` and checks that neither the final
/// name nor the staging name exists, and that the source directory is not the
/// parent or one of its ancestors (compared by device and inode; advisory,
/// since the source is a path). Nothing is created.
pub(super) fn destination(output: &Path, source: &Path) -> Result<Destination> {
    let absolute = control_fs::absolute_path(output)
        .with_context(|| format!("--output is refused: {}", shown(output)))?;
    let leaf = absolute
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|leaf| valid_leaf(leaf))
        .with_context(|| {
            format!(
                "--output must end in a new directory name of 1 to {MAX_LEAF_BYTES} of \
                 [A-Za-z0-9._-], not starting with a dot: {}",
                shown(output)
            )
        })?
        .to_owned();
    let parent_path = absolute
        .parent()
        .with_context(|| format!("--output has no parent directory: {}", shown(output)))?;
    let parent = Directory::open_existing(parent_path)
        .with_context(|| format!("{PARENT_RULE}: {}", shown(parent_path)))?;
    let source_facts = std::fs::metadata(source)
        .with_context(|| format!("the source directory is unavailable: {}", shown(source)))?;
    for ancestor in parent_path.ancestors() {
        let facts = std::fs::symlink_metadata(ancestor).with_context(|| {
            format!(
                "an ancestor of --output is unavailable: {}",
                shown(ancestor)
            )
        })?;
        ensure!(
            (facts.dev(), facts.ino()) != (source_facts.dev(), source_facts.ino()),
            "--output must be outside the source directory: the documents are never written \
             into the distribution they describe ({})",
            shown(output)
        );
    }
    let staging = format!(".{leaf}{STAGING_SUFFIX}");
    parent.require_absent_child(&leaf).with_context(|| {
        format!(
            "--output already exists or cannot be inspected: {}; release manifest writes only \
             into a new directory and never replaces one",
            shown(output)
        )
    })?;
    parent.require_absent_child(&staging).with_context(|| {
        format!(
            "a staging directory of an earlier run exists or cannot be inspected: {}; inspect or \
             remove it first, ai-guard never adopts or removes it",
            shown(&parent.path().join(&staging))
        )
    })?;
    Ok(Destination {
        parent,
        leaf,
        staging,
        given: output.to_owned(),
    })
}

/// Stages `files`, publishes them under the final name without replacing
/// anything, then reads the published directory back.
pub(super) fn publish(destination: &Destination, files: &[(&str, Vec<u8>)]) -> Result<()> {
    let staging_path = destination.staging_path();
    let stage = destination
        .parent
        .exclusive_child(&destination.staging)
        .with_context(|| {
            format!(
                "the staging directory {} could not be created or checked; if it exists, \
                 inspect it and remove it yourself, ai-guard never adopts or removes it; nothing \
                 was published",
                shown(&staging_path)
            )
        })?;
    let partial = || {
        format!(
            "the release documents could not be staged; what was written is kept at {} and \
             nothing was published",
            shown(&staging_path)
        )
    };
    for (name, bytes) in files {
        let file = stage.persistent_create(name, bytes).with_context(partial)?;
        file.persistent_sync().with_context(partial)?;
    }
    stage.sync().with_context(partial)?;
    #[cfg(test)]
    {
        let hook = BEFORE_FINAL_CHECK.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
    }
    // This check inspects the final name between two revalidations of the
    // parent: an entry there, a name that cannot be inspected or a parent that
    // changed all fail it, and its cause follows the message. The staging
    // directory may have moved with the parent, so only where it was created
    // is named.
    destination
        .parent
        .require_absent_child(&destination.leaf)
        .with_context(|| {
            format!(
                "{} was not confirmed absent before the rename, so the staging directory was not \
                 renamed to it; ai-guard replaced and removed nothing and did not move the \
                 staging directory it created at {}, where it wrote and synchronised the \
                 documents",
                destination.shown(),
                shown(&staging_path)
            )
        })?;
    #[cfg(test)]
    {
        let hook = BEFORE_PUBLISH.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
    }
    // `NotPublished` carries no cause. Before the rename it comes from a
    // revalidation of the parent or of the staging directory, or from the
    // synchronisation of the staging directory; then from the rename, for an
    // entry at the final name or any other refusal of the system. The parent
    // is synchronised only after the rename, where a failure is
    // `DurabilityUnknown`; the staging-parent and name checks cannot fail for
    // this destination, and other platforms are refused when the parent is
    // opened. Only ai-guard's own actions are known, so the message states
    // those and no place it did not check.
    match destination
        .parent
        .publish_child_exclusive(&stage, &destination.leaf)
    {
        Ok(()) => {}
        Err(PublicationFailure::NotPublished) => bail!(
            "the staging directory was not renamed to {} (the cause is not reported: an entry \
             may exist there, the parent or the staging directory may have failed a revalidation, \
             the staging directory may have failed a synchronisation, or the system may have \
             refused the rename); ai-guard replaced and removed nothing and did not move the \
             staging directory it created at {}",
            destination.shown(),
            shown(&staging_path)
        ),
        Err(PublicationFailure::DurabilityUnknown) => bail!(
            "the rename was performed but its durability is unknown; ai-guard removes neither \
             name. After a crash the directory may be found at {} or at {}; run release verify \
             --manifest <that directory> before using it",
            shown(&destination.final_path()),
            shown(&staging_path)
        ),
    }
    confirm(destination, files).map_err(|error| {
        anyhow!(
            "the documents were published at {} but not confirmed ({error:#}); the directory is \
             kept, verify it with release verify --manifest before using it",
            shown(&destination.final_path())
        )
    })
}

/// The published directory holds exactly `files`, with the bytes written.
fn confirm(destination: &Destination, files: &[(&str, Vec<u8>)]) -> Result<()> {
    let published = destination
        .parent
        .persistent_child(&destination.leaf, false)?
        .context("the published directory is unavailable")?;
    let limit = u32::try_from(files.len() + 1).unwrap_or(u32::MAX);
    let entries = published.persistent_entries(limit)?;
    let mut listed: Vec<&str> = entries
        .iter()
        .filter(|entry| !entry.directory)
        .map(|entry| entry.name.as_str())
        .collect();
    listed.sort_unstable();
    let mut expected: Vec<&str> = files.iter().map(|(name, _)| *name).collect();
    expected.sort_unstable();
    ensure!(
        entries.len() == files.len() && listed == expected,
        "the published directory does not hold exactly the release documents"
    );
    for (name, bytes) in files {
        let read = published
            .persistent_read(name, control_fs::MAX_PRIVATE_FILE_BYTES, &[])?
            .with_context(|| format!("{name} is missing from the published directory"))?;
        ensure!(
            read.bytes() == bytes.as_slice(),
            "{name} in the published directory differs from what was written"
        );
    }
    published.sync()?;
    destination.parent.sync()
}
