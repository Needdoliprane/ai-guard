//! Service integration of the proposed owned broker-supervision publisher
//! (`broker_supervision_owned_v2`). **Proposal submitted to Root, not an
//! admitted interface.**
//!
//! Each profile's presence file selects its profile explicitly through
//! `presence::owned::load_selected`; this module never chooses, converts or
//! falls back. A selected owned profile is owned, sampled, stopped and joined
//! by the same `checkpoint_source::Publication` as a legacy one: its
//! checkpoint actor receives only `confirmed_snapshot` values, that is heads
//! already acknowledged by an admitted Full writer, and nothing for a cold,
//! fenced or incomplete service. No writer, job, checkpoint, runtime, remote
//! command, fencing decision, worker authority, quota release or usage fact
//! comes from presence.
//!
//! Before any lock, anchor or journal entry exists, `check_owned_profiles`
//! keeps every owned journal dedicated across the whole broker and refuses
//! two owned profiles publishing the same enrolled broker. A broker without
//! an owned profile keeps its existing preparation exactly.
use super::checkpoint_source::{CheckpointSource, PresencePublisher, Snapshots};
use super::*;
use crate::broker::presence::{PresenceRunOwner, owned::PreparedOwnedPresence};

type PresenceResult<T> = std::result::Result<T, PresenceError>;

impl PresencePublisher for PreparedOwnedPresence {
    fn checkpoint_source(&self) -> Option<CheckpointSource> {
        // The enrolled authority is the only one a sampled head may carry.
        self.checkpoints_enabled().then(|| CheckpointSource {
            authority: self.checkpoint_authority(),
            cadence: self.publication_interval(),
        })
    }

    fn start_owner(
        self,
        snapshots: Option<Snapshots>,
        shutdown: watch::Receiver<bool>,
        attempts: Arc<Semaphore>,
        io: Arc<Semaphore>,
    ) -> PresenceResult<PresenceRunOwner> {
        match snapshots {
            Some(snapshots) => self.start_with_checkpoints(snapshots, shutdown, attempts, io),
            None => self.start(shutdown, attempts, io),
        }
    }
}

/// Dispatch to the explicitly selected profile only.
impl PresencePublisher for SelectedPresence {
    fn checkpoint_source(&self) -> Option<CheckpointSource> {
        match self {
            Self::Legacy(presence) => presence.checkpoint_source(),
            Self::Owned(presence) => presence.checkpoint_source(),
        }
    }

    fn start_owner(
        self,
        snapshots: Option<Snapshots>,
        shutdown: watch::Receiver<bool>,
        attempts: Arc<Semaphore>,
        io: Arc<Semaphore>,
    ) -> PresenceResult<PresenceRunOwner> {
        match self {
            Self::Legacy(presence) => {
                PresencePublisher::start_owner(*presence, snapshots, shutdown, attempts, io)
            }
            Self::Owned(presence) => {
                PresencePublisher::start_owner(*presence, snapshots, shutdown, attempts, io)
            }
        }
    }
}

/// One private journal directory of a prepared profile.
struct JournalRoot {
    profile: usize,
    owned: bool,
    root: Directory,
    identity: (u64, u64),
}

/// Static preparation rules for owned profiles, checked after every profile
/// is loaded read-only and before any protection, lock, anchor or journal
/// entry is created. A refusal is `InvalidConfig` and refuses the startup.
///
/// - Two owned profiles never publish the same enrolled broker, incarnation
///   or credential: a second publisher would diverge from the first.
/// - An owned journal never equals, contains or lies in another profile's
///   journal (lexically, or by physical identity for equality), the broker
///   root, a workspace source root or an export root.
/// - A journal paired with an owned profile never contains a protected file
///   of another profile or of the service (lexically or physically).
///
/// Pairs of legacy profiles are not compared here, so a broker without an
/// owned profile keeps its existing behaviour and diagnostics exactly.
pub(super) fn check_owned_profiles(
    registry: &Registry,
    material: &Material,
    prepared: &[(String, SelectedPresence)],
) -> PresenceResult<()> {
    if !prepared.iter().any(|(_, presence)| presence.is_owned()) {
        return Ok(());
    }
    let enrolled: Vec<_> = prepared
        .iter()
        .filter_map(|(_, presence)| match presence {
            SelectedPresence::Owned(presence) => Some(presence.enrolled()),
            SelectedPresence::Legacy(_) => None,
        })
        .collect();
    for (index, left) in enrolled.iter().enumerate() {
        for right in &enrolled[index + 1..] {
            if left.broker_id == right.broker_id
                || left.incarnation_id == right.incarnation_id
                || left.credential_id == right.credential_id
            {
                return Err(PresenceError::InvalidConfig);
            }
        }
    }
    let mut journals = Vec::new();
    let mut files = Vec::new();
    for (profile, (_, presence)) in prepared.iter().enumerate() {
        let owned = presence.is_owned();
        for root in presence.journal_roots() {
            let identity = root
                .physical_identity()
                .map_err(|_| PresenceError::InvalidConfig)?;
            journals.push(JournalRoot {
                profile,
                owned,
                root,
                identity,
            });
        }
        files.extend(
            presence
                .protected_files()
                .into_iter()
                .map(|file| (owned, file)),
        );
    }
    // Service control files belong to no presence profile.
    files.extend(material.files.iter().map(|file| (false, file.clone())));
    files.push((false, registry.source.clone()));
    let mut fixed = vec![registry.data.root.as_path()];
    for profile in &registry.data.profiles {
        fixed.extend(profile.source_roots.iter().map(|root| root.as_path()));
        fixed.extend(profile.export_roots.iter().map(|root| root.as_path()));
    }
    for journal in &journals {
        for (owned, file) in &files {
            if (journal.owned || *owned) && contains(&journal.root, file)? {
                return Err(PresenceError::InvalidConfig);
            }
        }
        for other in &journals {
            if other.profile != journal.profile
                && (journal.owned || other.owned)
                && (overlaps(journal.root.path(), other.root.path())
                    || journal.identity == other.identity)
            {
                return Err(PresenceError::InvalidConfig);
            }
        }
        if journal.owned
            && fixed
                .iter()
                .any(|root| overlaps(journal.root.path(), root))
        {
            return Err(PresenceError::InvalidConfig);
        }
    }
    Ok(())
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

/// Lexical, then physical, containment of a protected file in a journal.
fn contains(journal: &Directory, file: &PrivateFile) -> PresenceResult<bool> {
    if file.path().starts_with(journal.path()) {
        return Ok(true);
    }
    file.is_within_directory(journal.path())
        .map_err(|_| PresenceError::InvalidConfig)
}
