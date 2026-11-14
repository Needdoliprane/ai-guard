//! Owned broker-supervision publisher of the proposed
//! `broker_supervision_owned_v2` profile, on the existing protected presence
//! transport. **Proposal submitted to Root, not an admitted interface.**
//!
//! Selection is explicit: only a configuration naming that exact profile loads
//! this module. There is no fallback to, adoption of or migration from a
//! legacy configuration, journal or remote head. The public enrollment
//! handoff yields the journal pins; the separate private bearer is sent only
//! as the transport credential and never enters a journal, log or error.
//!
//! A heartbeat stays a declaration whose reception the control plane
//! observes. A checkpoint stays a `DeclaredPartial` list of heads already
//! acknowledged by an admitted Full writer. Neither is a runtime inventory, a
//! terminal observation, a fencing decision, a quota release, a worker
//! authority or a usage fact, and an absent source declares nothing.
use super::{
    PreparedPresence, PresenceError, PresenceRunOwner, PresenceScope, RECORD_BYTES, Result,
    admit_scope, http, normalized, private_bearer, run_owner, screen_text_with, screen_with,
};
use crate::{
    broker_registry::{
        BrokerAuthorityV1, BrokerCheckpointSnapshotV1,
        owned_supervision::{
            MAX_OWNED_ENROLLMENT_BYTES, OWNED_SUPERVISION_PROFILE_V2, OWNED_SUPERVISION_VERSION_V2,
            OwnedAckRejectionV2, OwnedBrokerEnrollmentV2, OwnedBrokerIdentityViewV2,
            OwnedBrokerSubjectV2, OwnedHandshakeRejectionV2, OwnedIncompatibilityV2,
            OwnedRemoteFailureV2, OwnedSupervisionErrorCodeV2, OwnedSupervisionPinsV2,
        },
    },
    control_fs::{Directory, PrivateFile},
    redaction::Redactor,
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    fmt,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, watch};

mod checkpoint;
mod heartbeat;
mod journal;
mod schedule;
mod transport;

/// Closed, secret-free state in which an owned publisher waits or stops. None
/// of them rewrites the retained Pending, an anchor or a journal record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnedPresenceBlock {
    /// Classified non-200 control response; its remote prose is never kept.
    Remote(OwnedRemoteFailureV2),
    /// The authenticated self view refused the pinned enrollment handshake.
    Handshake(OwnedHandshakeRejectionV2),
    /// The engaged identity cannot replay or progress this journal.
    Identity(OwnedIncompatibilityV2),
    /// A positive response or self-read does not acknowledge the Pending.
    Acknowledgement(OwnedAckRejectionV2),
    /// The shared contract refused to prepare or revalidate the operation.
    Contract(OwnedSupervisionErrorCodeV2),
    /// The control already holds a head of this incarnation that this journal
    /// never acknowledged; a remote head is never adopted.
    RemoteHeadWithoutHistory,
    /// The control head differs from this journal's last acknowledgement.
    RemoteHeadDiverged,
    /// The directory belongs to another profile, journal, enrollment or scope.
    ForeignJournal,
}
impl OwnedPresenceBlock {
    fn category(self) -> &'static str {
        match self {
            Self::Remote(_) => "control_response",
            Self::Handshake(_) => "handshake",
            Self::Identity(_) => "identity",
            Self::Acknowledgement(_) => "acknowledgement",
            Self::Contract(_) => "contract",
            Self::RemoteHeadWithoutHistory | Self::RemoteHeadDiverged => "remote_head",
            Self::ForeignJournal => "journal",
        }
    }

    fn detail(self) -> &'static str {
        use OwnedAckRejectionV2 as Ack;
        use OwnedHandshakeRejectionV2 as Handshake;
        use OwnedRemoteFailureV2 as Remote;
        match self {
            Self::Remote(Remote::Owned(code)) | Self::Contract(code) => code.code(),
            Self::Remote(Remote::TransportOutcomeUnknown) => "transport_outcome_unknown",
            Self::Remote(Remote::GatewayCapacityBusy) => "gateway_capacity_busy",
            Self::Remote(Remote::GatewayNotDispatched) => "gateway_not_dispatched",
            Self::Remote(Remote::GatewayRefused) => "gateway_refused",
            Self::Remote(Remote::CoverageUnavailable) => "source_coverage_unavailable",
            Self::Remote(Remote::AuditBudgetExhausted) => "audit_budget_exhausted",
            Self::Remote(Remote::Unrecognized) => "unrecognized",
            Self::Handshake(Handshake::Incompatible(reason))
            | Self::Identity(reason)
            | Self::Acknowledgement(Ack::Incompatible(reason)) => reason.code(),
            Self::Handshake(Handshake::Protocol) | Self::Acknowledgement(Ack::Protocol) => {
                "protocol"
            }
            Self::Handshake(Handshake::Forbidden) => "forbidden",
            Self::Handshake(Handshake::Clock) => "clock_uncertain",
            Self::Acknowledgement(Ack::Commitment) => "commitment",
            Self::Acknowledgement(Ack::Receipt) => "receipt",
            Self::Acknowledgement(Ack::Snapshot) => "snapshot",
            Self::Acknowledgement(Ack::Conflict) => "conflict",
            Self::RemoteHeadWithoutHistory => "without_local_history",
            Self::RemoteHeadDiverged => "diverged",
            Self::ForeignJournal => "foreign",
        }
    }

    /// Fixed operator guidance; never a request value, remote prose or secret.
    fn operator_action(self) -> &'static str {
        use OwnedHandshakeRejectionV2 as Handshake;
        use OwnedRemoteFailureV2 as Remote;
        match self {
            Self::Remote(Remote::Owned(code)) | Self::Contract(code) => code.message(),
            Self::Remote(Remote::TransportOutcomeUnknown) => {
                "The control transport lost the exchange; the same operation is \
                 reconciled by an exact self-read."
            }
            Self::Remote(Remote::GatewayCapacityBusy | Remote::GatewayNotDispatched) => {
                "The control gateway did not dispatch the request; the same operation \
                 is retried."
            }
            Self::Remote(Remote::GatewayRefused) => {
                "The control gateway refused the owned route, method or size; verify \
                 that the control plane admits the owned profile."
            }
            Self::Remote(Remote::CoverageUnavailable) => {
                "The owned source coverage of the control plane is closing or must be \
                 rebuilt; the control operator must restore it."
            }
            Self::Remote(Remote::AuditBudgetExhausted) => {
                "The control audit budget is exhausted; contact the control operator."
            }
            Self::Remote(Remote::Unrecognized) => {
                "The control response is not part of the owned profile; contact the \
                 control operator."
            }
            Self::Handshake(Handshake::Incompatible(reason)) | Self::Identity(reason) => {
                reason.operator_action()
            }
            Self::Handshake(Handshake::Protocol) => {
                "The control self view failed the owned profile validation."
            }
            Self::Handshake(Handshake::Forbidden) => {
                "The broker grant is revoked or expired, or its lease is shorter than \
                 twice the publication interval."
            }
            Self::Handshake(Handshake::Clock) => {
                "The control clock precedes the durable broker state; reconcile the \
                 control clock."
            }
            Self::Acknowledgement(_) => {
                "The control response does not acknowledge the retained Pending; the \
                 journal is kept unchanged for reconciliation with the control operator."
            }
            Self::RemoteHeadWithoutHistory => {
                "The control holds a head of this incarnation that this journal never \
                 acknowledged; it is not adopted. Restore the original journal or \
                 install a new enrollment with a new empty journal."
            }
            Self::RemoteHeadDiverged => {
                "The control head differs from the last acknowledgement of this journal, \
                 for example after a control restore or with a second publisher; \
                 reconcile before resuming."
            }
            Self::ForeignJournal => {
                "The journal directory belongs to another profile, journal, enrollment \
                 or scope; restore the matching original enrollment/journal or enroll a new \
                 incarnation with new journals. The existing evidence is kept untouched."
            }
        }
    }
}
impl fmt::Display for OwnedPresenceBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "owned broker supervision is blocked ({}: {}); {}",
            self.category(),
            self.detail(),
            self.operator_action()
        )
    }
}

fn block(reason: OwnedPresenceBlock) -> PresenceError {
    PresenceError::Owned(reason)
}
fn identity(reason: OwnedIncompatibilityV2) -> PresenceError {
    block(OwnedPresenceBlock::Identity(reason))
}
fn handshake(reason: OwnedHandshakeRejectionV2) -> PresenceError {
    block(OwnedPresenceBlock::Handshake(reason))
}
fn acknowledgement(reason: OwnedAckRejectionV2) -> PresenceError {
    block(OwnedPresenceBlock::Acknowledgement(reason))
}
fn contract(code: OwnedSupervisionErrorCodeV2) -> PresenceError {
    block(OwnedPresenceBlock::Contract(code))
}

/// Explicit owned presence configuration. The exact profile and version are
/// required; a legacy configuration cannot carry `profile` because the legacy
/// loader refuses unknown keys. Identity fields come only from the enrollment.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedPresenceConfigV2 {
    profile: String,
    version: u32,
    enrollment_file: PathBuf,
    bearer_file: PathBuf,
    #[serde(default)]
    ca_bundle_file: Option<PathBuf>,
    #[serde(default = "super::interval_default")]
    interval_seconds: u32,
    heartbeat_journal: OwnedJournalConfigV2,
    #[serde(default)]
    checkpoint_journal: Option<OwnedJournalConfigV2>,
}
/// Bounds of one private owned journal. No capacity is assumed by default.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedJournalConfigV2 {
    state_dir: PathBuf,
    max_operations: u32,
    max_state_bytes: u64,
}
impl OwnedJournalConfigV2 {
    fn limits<K: journal::Kind>(&self) -> Result<journal::JournalLimits> {
        if !(1..=journal::MAX_JOURNAL_OPERATIONS).contains(&self.max_operations)
            || self.max_state_bytes < journal::minimum_bytes::<K>()
            || self.max_state_bytes > journal::MAX_JOURNAL_BYTES
        {
            return Err(PresenceError::InvalidConfig);
        }
        Ok(journal::JournalLimits {
            operations: self.max_operations,
            bytes: self.max_state_bytes,
        })
    }
}
impl OwnedPresenceConfigV2 {
    fn validate(&self) -> Result<()> {
        if self.profile != OWNED_SUPERVISION_PROFILE_V2
            || self.version != OWNED_SUPERVISION_VERSION_V2
            || !(5..=150).contains(&self.interval_seconds)
        {
            return Err(PresenceError::InvalidConfig);
        }
        self.heartbeat_journal
            .limits::<heartbeat::HeartbeatJournal>()?;
        if let Some(journal) = &self.checkpoint_journal {
            // The checkpoint journal has its own larger record bound.
            journal.limits::<checkpoint::CheckpointJournal>()?;
        }
        Ok(())
    }
}

pub(crate) struct PreparedOwnedPresence {
    scope: PresenceScope,
    config: OwnedPresenceConfigV2,
    pins: OwnedSupervisionPinsV2,
    source: PrivateFile,
    enrollment: PrivateFile,
    files: Vec<PrivateFile>,
    roots: Vec<Directory>,
    heartbeat_state: Directory,
    checkpoint_state: Option<Directory>,
    bearer: String,
    secrets: Vec<String>,
    ca: Option<PrivateFile>,
    /// Process-wide maxima, initialized from the durable journal observations.
    closure_floor: AtomicU64,
    binding_floor: AtomicU64,
    heartbeat: Option<journal::Journal<heartbeat::HeartbeatJournal>>,
    checkpoint: Option<journal::Journal<checkpoint::CheckpointJournal>>,
}
impl fmt::Debug for PreparedOwnedPresence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PreparedOwnedPresence(<private>)")
    }
}
impl PreparedOwnedPresence {
    /// Strictly read-only, like the legacy loader: no lock, anchor or journal
    /// entry is created until every profile's protected material is admitted.
    pub(crate) fn load(
        path: &Path,
        mut scope: PresenceScope,
        known_secrets: Vec<String>,
    ) -> Result<Self> {
        normalized(path)?;
        let source =
            PrivateFile::open(path, RECORD_BYTES).map_err(|_| PresenceError::InvalidConfig)?;
        let config: OwnedPresenceConfigV2 =
            serde_json::from_slice(source.bytes()).map_err(|_| PresenceError::InvalidConfig)?;
        config.validate()?;
        let origin = admit_scope(&mut scope)?;
        let mut named = vec![
            path,
            config.enrollment_file.as_path(),
            config.bearer_file.as_path(),
        ];
        named.extend(config.ca_bundle_file.as_deref());
        for (index, candidate) in named.iter().enumerate() {
            normalized(candidate)?;
            if named[..index].contains(candidate) {
                return Err(PresenceError::InvalidConfig);
            }
        }
        normalized(&config.heartbeat_journal.state_dir)?;
        if let Some(journal) = &config.checkpoint_journal {
            normalized(&journal.state_dir)?;
        }
        let enrollment_file =
            PrivateFile::open(&config.enrollment_file, MAX_OWNED_ENROLLMENT_BYTES as u64)
                .map_err(|_| PresenceError::InvalidConfig)?;
        let enrollment: OwnedBrokerEnrollmentV2 = serde_json::from_slice(enrollment_file.bytes())
            .map_err(|_| PresenceError::InvalidConfig)?;
        let pins = OwnedSupervisionPinsV2::from_enrollment(&enrollment)
            .map_err(|_| PresenceError::InvalidConfig)?;
        // `scope.installation_id` is the broker's installation; the subject's
        // installation is the control closure's. They are never compared.
        if pins.gateway_origin != scope.gateway
            || pins.enrolled.tenant_id != scope.expected_tenant_id
            || pins.enrolled.control_namespace_uuid != scope.control_namespace_uuid.to_string()
            || pins.authority.runtime_kind != scope.runtime_kind
            || enrollment.lease_seconds < config.interval_seconds * 2
        {
            return Err(PresenceError::InvalidConfig);
        }
        let bearer_file = PrivateFile::open(&config.bearer_file, 4097)
            .map_err(|_| PresenceError::InvalidConfig)?;
        let bearer = private_bearer(&bearer_file)?;
        let ca = config
            .ca_bundle_file
            .as_ref()
            .map(|ca_path| {
                if origin.scheme() != "https" {
                    return Err(PresenceError::InvalidConfig);
                }
                let file = PrivateFile::open(ca_path, 65_536)
                    .map_err(|_| PresenceError::InvalidConfig)?;
                http::certificates(&file)?;
                Ok(file)
            })
            .transpose()?;
        let heartbeat_state = Directory::open_existing(&config.heartbeat_journal.state_dir)
            .map_err(|_| PresenceError::InvalidConfig)?;
        let checkpoint_state = config
            .checkpoint_journal
            .as_ref()
            .map(|journal| {
                Directory::open_existing(&journal.state_dir)
                    .map_err(|_| PresenceError::InvalidConfig)
            })
            .transpose()?;
        let mut files = vec![source.clone(), enrollment_file.clone(), bearer_file];
        files.extend(ca.iter().cloned());
        distinct_journals(&files, &heartbeat_state, checkpoint_state.as_ref())?;
        let mut roots = vec![heartbeat_state.clone()];
        roots.extend(checkpoint_state.iter().cloned());
        for file in &files {
            let parent = file.path().parent().ok_or(PresenceError::InvalidConfig)?;
            if !roots.iter().any(|root| root.path() == parent) {
                roots.push(
                    Directory::open_existing(parent).map_err(|_| PresenceError::InvalidConfig)?,
                );
            }
        }
        let closure_floor = AtomicU64::new(pins.minimum_closure_revision);
        let binding_floor = AtomicU64::new(pins.enrolled.binding_revision);
        let mut prepared = Self {
            scope,
            config,
            pins,
            source,
            enrollment: enrollment_file,
            files,
            roots,
            heartbeat_state,
            checkpoint_state,
            bearer,
            secrets: known_secrets,
            ca,
            closure_floor,
            binding_floor,
            heartbeat: None,
            checkpoint: None,
        };
        prepared.secrets.push(prepared.bearer.clone());
        prepared.screen_material()?;
        prepared.revalidate()?;
        Ok(prepared)
    }

    pub(crate) fn protected_files(&self) -> Vec<PrivateFile> {
        self.files.clone()
    }
    pub(crate) fn protected_roots(&self) -> Vec<Directory> {
        self.roots.clone()
    }
    pub(crate) fn secret_literals(&self) -> Vec<String> {
        vec![self.bearer.clone()]
    }
    /// The configured journal directories, opened read-only by `load`. The
    /// service keeps them apart from every other profile's journals and
    /// protected files; nothing is created here.
    pub(crate) fn journal_roots(&self) -> Vec<Directory> {
        let mut roots = vec![self.heartbeat_state.clone()];
        roots.extend(self.checkpoint_state.iter().cloned());
        roots
    }

    /// Open the private journals after the cumulative protections of every
    /// profile are known. A journal is never repaired, reset or adopted.
    pub(crate) fn finalize(&mut self, values: &[String]) -> Result<()> {
        if self.heartbeat.is_some() || self.checkpoint.is_some() {
            return Err(PresenceError::UnsafeState);
        }
        self.secrets.extend_from_slice(values);
        self.secrets.sort();
        self.secrets.dedup();
        if self.secrets.len() > 64 || self.secrets.iter().any(|s| s.is_empty() || s.len() > 4096) {
            return Err(PresenceError::InvalidConfig);
        }
        self.screen_material()
            .map_err(|_| PresenceError::InvalidConfig)?;
        self.revalidate()?;
        let this: &Self = self;
        let heartbeat = journal::Journal::<heartbeat::HeartbeatJournal>::open(
            this,
            &this.heartbeat_state,
            this.config
                .heartbeat_journal
                .limits::<heartbeat::HeartbeatJournal>()?,
        )?;
        let checkpoint = match (&this.checkpoint_state, &this.config.checkpoint_journal) {
            (Some(root), Some(config)) => {
                Some(journal::Journal::<checkpoint::CheckpointJournal>::open(
                    this,
                    root,
                    config.limits::<checkpoint::CheckpointJournal>()?,
                )?)
            }
            (None, None) => None,
            _ => return Err(PresenceError::InvalidConfig),
        };
        self.heartbeat = Some(heartbeat);
        self.checkpoint = checkpoint;
        let head = self.heartbeat_journal()?.cached_head()?;
        self.binding_floor.fetch_max(head.binding_floor, Ordering::SeqCst);
        self.closure_floor.fetch_max(head.closure_floor, Ordering::SeqCst);
        if let Some(journal) = &self.checkpoint {
            let head = journal.cached_head()?;
            self.binding_floor.fetch_max(head.binding_floor, Ordering::SeqCst);
            self.closure_floor.fetch_max(head.closure_floor, Ordering::SeqCst);
        }
        Ok(())
    }

    pub(crate) fn revalidate(&self) -> Result<()> {
        for file in &self.files {
            file.revalidate()
                .map_err(|_| PresenceError::UnsafeFilesystem)?;
        }
        for root in &self.roots {
            root.revalidate()
                .map_err(|_| PresenceError::UnsafeFilesystem)?;
        }
        Ok(())
    }

    /// Heartbeats only. A configured checkpoint journal never runs without
    /// its real source, exactly like the legacy V2 profile.
    pub(crate) fn start(
        self,
        shutdown: watch::Receiver<bool>,
        attempts: Arc<Semaphore>,
        io: Arc<Semaphore>,
    ) -> Result<PresenceRunOwner> {
        if self.config.checkpoint_journal.is_some() || self.checkpoint.is_some() {
            return Err(PresenceError::InvalidConfig);
        }
        self.spawn(None, shutdown, attempts, io)
    }

    /// Heartbeats and checkpoints sampled from the service's acknowledged
    /// Full-writer heads; `None` from the source declares nothing.
    pub(crate) fn start_with_checkpoints(
        self,
        snapshots: watch::Receiver<Option<BrokerCheckpointSnapshotV1>>,
        shutdown: watch::Receiver<bool>,
        attempts: Arc<Semaphore>,
        io: Arc<Semaphore>,
    ) -> Result<PresenceRunOwner> {
        if self.checkpoint.is_none() {
            return Err(PresenceError::InvalidConfig);
        }
        self.spawn(Some(snapshots), shutdown, attempts, io)
    }

    fn spawn(
        self,
        snapshots: Option<watch::Receiver<Option<BrokerCheckpointSnapshotV1>>>,
        shutdown: watch::Receiver<bool>,
        attempts: Arc<Semaphore>,
        io: Arc<Semaphore>,
    ) -> Result<PresenceRunOwner> {
        if self.heartbeat.is_none() {
            return Err(PresenceError::UnsafeState);
        }
        let checkpoints = snapshots.is_some();
        let p = Arc::new(self);
        let reporter = Arc::clone(&p);
        let heartbeat_p = Arc::clone(&p);
        let heartbeat_attempts = Arc::clone(&attempts);
        let heartbeat_io = Arc::clone(&io);
        let checkpoint = snapshots.map(move |snapshots| {
            move |stop: watch::Receiver<bool>| async move {
                let mut stop = stop;
                checkpoint::run(p, snapshots, &mut stop, attempts, io).await
            }
        });
        let owner = run_owner::supervise(
            shutdown,
            move |stop: watch::Receiver<bool>| async move {
                let mut stop = stop;
                heartbeat::run(heartbeat_p, &mut stop, heartbeat_attempts, heartbeat_io).await
            },
            checkpoint,
        )?;
        // Diagnostic only: the owned profile was selected and its actors run.
        // It states nothing about the control plane, a runtime or a worker.
        reporter.report(
            <heartbeat::HeartbeatJournal as journal::Kind>::NAME,
            "publication_started",
            None,
            None,
        );
        if checkpoints {
            reporter.report(
                <checkpoint::CheckpointJournal as journal::Kind>::NAME,
                "publication_started",
                None,
                None,
            );
        }
        Ok(owner)
    }

    pub(crate) fn checkpoints_enabled(&self) -> bool {
        self.config.checkpoint_journal.is_some()
    }
    /// The single enrolled authority; a snapshot for another one is refused.
    pub(crate) fn checkpoint_authority(&self) -> BrokerAuthorityV1 {
        self.pins.authority.clone()
    }
    /// The enrolled public subject, so the service can refuse two local
    /// publishers of one broker. It is never a credential or an authority.
    pub(crate) fn enrolled(&self) -> &OwnedBrokerSubjectV2 {
        &self.pins.enrolled
    }
    pub(crate) fn publication_interval(&self) -> Duration {
        Duration::from_secs(u64::from(self.config.interval_seconds))
    }

    fn screen_material(&self) -> Result<()> {
        for file in [&self.source, &self.enrollment] {
            let value = serde_json::from_slice::<Value>(file.bytes())
                .map_err(|_| PresenceError::InvalidConfig)?;
            self.screen(&value)?;
        }
        for value in [
            serde_json::to_value(&self.scope),
            serde_json::to_value(&self.pins),
        ] {
            self.screen(&value.map_err(|_| PresenceError::InvalidConfig)?)?;
        }
        for file in &self.files {
            self.screen_text(file.path().to_str().ok_or(PresenceError::InvalidConfig)?)?;
        }
        for root in &self.roots {
            self.screen_text(root.path().to_str().ok_or(PresenceError::InvalidConfig)?)?;
        }
        Ok(())
    }
    fn screen(&self, value: &Value) -> Result<()> {
        screen_with(&self.secrets, value)
    }
    fn screen_text(&self, text: &str) -> Result<()> {
        screen_text_with(&self.secrets, text)
    }

    fn heartbeat_journal(&self) -> Result<&journal::Journal<heartbeat::HeartbeatJournal>> {
        self.heartbeat.as_ref().ok_or(PresenceError::UnsafeState)
    }
    fn checkpoint_journal(&self) -> Result<&journal::Journal<checkpoint::CheckpointJournal>> {
        self.checkpoint.as_ref().ok_or(PresenceError::UnsafeState)
    }
    /// Same lease rule as the legacy handshake: at least two intervals.
    fn min_lease(&self) -> u32 {
        self.config.interval_seconds * 2
    }

    /// Closure revisions are observed: they may advance, never regress, during
    /// this run. Journal records enforce the same rule across restarts.
    fn observe_closure(&self, view: &OwnedBrokerIdentityViewV2) -> Result<()> {
        let binding = self.binding_floor.fetch_max(view.subject.binding_revision, Ordering::SeqCst);
        if view.subject.binding_revision < binding {
            return Err(identity(OwnedIncompatibilityV2::BindingRegression));
        }
        let floor = self
            .closure_floor
            .fetch_max(view.closure_revision, Ordering::SeqCst);
        if view.closure_revision < floor {
            return Err(identity(OwnedIncompatibilityV2::ClosureRegression));
        }
        Ok(())
    }

    /// One closed operator event. The profile label is redacted; the reason is
    /// fixed prose of a closed state, never remote prose or a secret.
    fn report(
        &self,
        journal: &'static str,
        event: &'static str,
        error: Option<PresenceError>,
        retry_action: Option<&'static str>,
    ) {
        let profile = Redactor::new(self.secrets.clone()).text(&self.scope.profile_id);
        eprintln!(
            "{}",
            serde_json::json!({
                "component": "broker_owned_presence",
                "event": event,
                "profile": profile,
                "journal": journal,
                "reason": error.map(|error| error.to_string()),
                "retry_action": retry_action,
                "runtime_state": "unknown",
            })
        );
    }
}

/// A presence profile chosen by its explicit configuration key.
pub(crate) enum SelectedPresence {
    Legacy(Box<PreparedPresence>),
    Owned(Box<PreparedOwnedPresence>),
}
/// The service-facing preparation contract shared by both profiles. Each call
/// reaches exactly the selected profile; none converts one into the other.
impl SelectedPresence {
    pub(crate) fn protected_files(&self) -> Vec<PrivateFile> {
        match self {
            Self::Legacy(presence) => presence.protected_files(),
            Self::Owned(presence) => presence.protected_files(),
        }
    }
    pub(crate) fn protected_roots(&self) -> Vec<Directory> {
        match self {
            Self::Legacy(presence) => presence.protected_roots(),
            Self::Owned(presence) => presence.protected_roots(),
        }
    }
    pub(crate) fn secret_literals(&self) -> Vec<String> {
        match self {
            Self::Legacy(presence) => presence.secret_literals(),
            Self::Owned(presence) => presence.secret_literals(),
        }
    }
    pub(crate) fn journal_roots(&self) -> Vec<Directory> {
        match self {
            Self::Legacy(presence) => presence.journal_roots(),
            Self::Owned(presence) => presence.journal_roots(),
        }
    }
    pub(crate) fn finalize(&mut self, values: &[String]) -> Result<()> {
        match self {
            Self::Legacy(presence) => presence.finalize(values),
            Self::Owned(presence) => presence.finalize(values),
        }
    }
    pub(crate) fn revalidate(&self) -> Result<()> {
        match self {
            Self::Legacy(presence) => presence.revalidate(),
            Self::Owned(presence) => presence.revalidate(),
        }
    }
    pub(crate) fn is_owned(&self) -> bool {
        matches!(self, Self::Owned(_))
    }
}

/// Explicit, closed selection: a configuration whose top-level object names a
/// `profile` must name exactly the owned profile and is loaded only by the
/// owned loader; one without `profile` is loaded only by the legacy loader,
/// which itself refuses that key. No other content is inspected to choose, and
/// a refused owned configuration never falls back to the legacy profile.
pub(crate) fn load_selected(
    path: &Path,
    scope: PresenceScope,
    known_secrets: Vec<String>,
) -> Result<SelectedPresence> {
    normalized(path)?;
    let source = PrivateFile::open(path, RECORD_BYTES).map_err(|_| PresenceError::InvalidConfig)?;
    let value: Value =
        serde_json::from_slice(source.bytes()).map_err(|_| PresenceError::InvalidConfig)?;
    match value.get("profile") {
        None => PreparedPresence::load(path, scope, known_secrets)
            .map(|prepared| SelectedPresence::Legacy(Box::new(prepared))),
        Some(Value::String(profile)) if profile == OWNED_SUPERVISION_PROFILE_V2 => {
            PreparedOwnedPresence::load(path, scope, known_secrets)
                .map(|prepared| SelectedPresence::Owned(Box::new(prepared)))
        }
        Some(_) => Err(PresenceError::InvalidConfig),
    }
}

/// Journals are distinct from each other and from every protected file: no
/// equality, no lexical or physical nesting of a private file in a journal.
fn distinct_journals(
    files: &[PrivateFile],
    heartbeat: &Directory,
    checkpoint: Option<&Directory>,
) -> Result<()> {
    for journal in std::iter::once(heartbeat).chain(checkpoint) {
        for file in files {
            if file.path().starts_with(journal.path())
                || file
                    .is_within_directory(journal.path())
                    .map_err(|_| PresenceError::InvalidConfig)?
            {
                return Err(PresenceError::InvalidConfig);
            }
        }
    }
    if let Some(checkpoint) = checkpoint {
        let (left, right) = (heartbeat.path(), checkpoint.path());
        if left.starts_with(right)
            || right.starts_with(left)
            || physical(heartbeat)? == physical(checkpoint)?
        {
            return Err(PresenceError::InvalidConfig);
        }
    }
    Ok(())
}
fn physical(directory: &Directory) -> Result<(u64, u64)> {
    let facts = directory
        .try_clone_file()
        .and_then(|file| file.metadata().map_err(Into::into))
        .map_err(|_| PresenceError::InvalidConfig)?;
    Ok((facts.dev(), facts.ino()))
}
