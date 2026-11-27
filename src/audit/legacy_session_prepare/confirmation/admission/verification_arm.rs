//! Checked authority for the separate named Import verification Arm.
use super::*;
use crate::control::session_control::LegacyNamedImportObservationPhaseV1;

fn verification_predecessor(
    db: &Connection,
    binding_ref: Uuid,
    namespace: &str,
    arm: &ArmV1,
    before_seq: i64,
) -> anyhow::Result<(WorkerV1, DateTime<Utc>)> {
    let ArmSubjectV1::Helper { permission, .. } = &arm.subject else {
        return Err(ControlFailure::InvalidRequest.into());
    };
    let HelperPermissionV1::ImportVerification {
        import_helper_id,
        import_arm,
        import_terminal,
        ..
    } = permission.as_ref()
    else {
        return Err(ControlFailure::InvalidRequest.into());
    };
    let (first_event, first_record) =
        supervision::load_record(db, &import_arm.arm_id)?.ok_or(ControlFailure::NotFound)?;
    let admission_codec::Input::LegacyNamedArm(first) = first_record.input else {
        return Err(ControlFailure::Conflict.into());
    };
    let (terminal_event, terminal_record) =
        supervision::load_record(db, &import_terminal.operation_id)?
            .ok_or(ControlFailure::NotFound)?;
    let admission_codec::Input::LegacyNamedImportObservation(terminal) = &terminal_record.input
    else {
        return Err(ControlFailure::Conflict.into());
    };
    anyhow::ensure!(
        first.binding_ref == binding_ref
            && first.namespace == namespace
            && terminal.request.binding_ref == binding_ref
            && terminal.request.namespace == namespace,
        ControlFailure::NotFound
    );
    let exact_arm = supervision::arm_receipt(&first.arm, namespace, &first_event)?;
    let exact_terminal = admission_codec::receipt_for(&terminal_event, &terminal_record)?;
    conflict(
        first_event.seq < terminal_event.seq
            && terminal_event.seq < before_seq
            && serde_json::to_value(import_arm)? == serde_json::to_value(&exact_arm)?
            && &exact_terminal == import_terminal
            && terminal.session_id == first.session_id
            && terminal.request.phase == LegacyNamedImportObservationPhaseV1::Terminal
            && &terminal.request.observation.helper_id == import_helper_id
            && serde_json::to_value(&terminal.request.observation.arm_receipt)?
                == serde_json::to_value(&exact_arm)?
            && terminal.request.observation.observation.exit_code == Some(0)
            && arm.authority_binding_hash == first.arm.authority_binding_hash
            && arm.inspection.container_id != first.arm.inspection.container_id
            && arm.configuration_fingerprint != first.arm.configuration_fingerprint,
    )?;
    let outcome = first_record.worker_record.ok_or(ControlFailure::Conflict)?;
    conflict(outcome.action == "prepare" && outcome.operation_id == first.arm.operation_id)?;
    let prepared = outcome.worker.ok_or(ControlFailure::Conflict)?;
    arm.validate_for_parent(&prepared)?;
    Ok((prepared, terminal.authorized_at))
}

struct VerificationArmLookup {
    subject: AuthorizedNamedArmSubject,
    prepared: WorkerV1,
    broker_deadline: DateTime<Utc>,
    before_seq: i64,
}

fn authorize_verification_arm_lookup(
    tx: &CheckedTransaction<'_>,
    broker: &BrokerIdentityV1,
    request: &LegacyNamedImportVerificationArmRequestV2,
) -> anyhow::Result<VerificationArmLookup> {
    let subject = authorize_named_arm_subject(
        tx,
        broker,
        request.binding_ref,
        &request.namespace,
        &request.arm,
    )?;
    let broker_deadline =
        workload::broker_identity_deadline_checked(tx, broker, subject.clock.at())?;
    let before_seq = audit_head(tx)?
        .0
        .checked_add(1)
        .context("audit sequence exhausted")?;
    let (prepared, _) = verification_predecessor(
        tx,
        request.binding_ref,
        &request.namespace,
        &request.arm,
        before_seq,
    )?;
    conflict(subject.admitted_revision.checked_add(1) == Some(prepared.revision))?;
    Ok(VerificationArmLookup {
        subject,
        prepared,
        broker_deadline,
        before_seq,
    })
}

fn retained_verification_arm(
    db: &Connection,
    request: &LegacyNamedImportVerificationArmRequestV2,
) -> anyhow::Result<Option<ArmResultV1>> {
    let Some((stored, record)) = supervision::load_record(db, &request.arm.operation_id)? else {
        return Ok(None);
    };
    let admission_codec::Input::LegacyNamedImportVerificationArm(named) = record.input else {
        return Err(ControlFailure::Conflict.into());
    };
    anyhow::ensure!(
        named.binding_ref == request.binding_ref && named.namespace == request.namespace,
        ControlFailure::NotFound
    );
    arm_admission(db, &named, stored.seq)?;
    conflict(serde_json::to_value(&named.arm)? == serde_json::to_value(&request.arm)?)?;
    let receipt = supervision::arm_receipt(&named.arm, &named.namespace, &stored)?;
    Ok(Some(ArmResultV1 {
        replayed: true,
        arm: named.arm,
        receipt,
    }))
}

fn release_verification_receipt(
    human_deadline: Option<DateTime<Utc>>,
    broker_deadline: DateTime<Utc>,
) -> anyhow::Result<()> {
    let now = Utc::now();
    anyhow::ensure!(
        human_deadline.is_none_or(|deadline| now < deadline),
        ControlFailure::NotFound
    );
    anyhow::ensure!(
        workload::canonical_control_time(now)? < broker_deadline,
        WorkloadFailureV1::Unauthorized
    );
    Ok(())
}

impl Store {
    pub(in crate::audit) fn validate_named_import_verification_arm_predecessor(
        db: &Connection,
        stored: &StoredEvent,
        named: &LegacyNamedArmV1,
        parent: &WorkerV1,
    ) -> anyhow::Result<()> {
        validate_named_arm_authority(db, named, parent, stored.seq, |_, admitted_revision| {
            conflict(admitted_revision.checked_add(1) == Some(parent.revision))
        })?;
        let (prepared, terminal_at) = verification_predecessor(
            db,
            named.binding_ref,
            &named.namespace,
            &named.arm,
            stored.seq,
        )?;
        conflict(
            serde_json::to_value(parent)? == serde_json::to_value(prepared)?
                && stored.event.session_id.as_deref() == Some(named.session_id.as_str())
                && stored.event.coverage == "observed"
                && named.authorized_at >= terminal_at
                && named.authorized_at <= crate::broker_registry::parse_time(&stored.received_at)?,
        )
    }

    /// Commit one verification Arm for the exact authorized named attempt.
    #[allow(
        dead_code,
        reason = "private verification Arm transport is composed separately"
    )]
    pub(crate) fn arm_legacy_named_import_verification(
        &self,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedImportVerificationArmRequestV2,
    ) -> anyhow::Result<ArmResultV1> {
        request.validate()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
        let tx = self.begin_checked(&mut db)?;
        let lookup = authorize_verification_arm_lookup(&tx, broker, request)?;
        let clock = &lookup.subject.clock;
        if let Some(retained) = retained_verification_arm(&tx, request)? {
            self.finish_named_import_lookup(tx, broker, clock)?;
            release_verification_receipt(None, lookup.broker_deadline)?;
            return Ok(retained);
        }
        anyhow::ensure!(tx.lease.active, ControlFailure::SupervisionInactive);
        let arm = &request.arm;
        conflict(!named_operation(&tx, canonical_uuid(&arm.operation_id)?)?)?;
        let parent = control_store::worker_at(&tx, &arm.worker_id, lookup.before_seq)?
            .ok_or(ControlFailure::NotFound)?;
        let parent_value = serde_json::to_value(&parent)?;
        conflict(parent_value == serde_json::to_value(&lookup.prepared)?)?;
        let (human_deadline, broker_deadline) = self.require_legacy_named_execution(
            &tx,
            broker,
            request.binding_ref,
            &request.namespace,
            &parent,
            clock,
        )?;
        let admission = &lookup.subject.admission;
        let input =
            admission_codec::Input::LegacyNamedImportVerificationArm(Box::new(LegacyNamedArmV1 {
                version: 1,
                binding_ref: request.binding_ref,
                admission_fact_seq: admission.seq,
                admission_fact_hash: admission.hash.clone(),
                namespace: request.namespace.clone(),
                session_id: parent.launch.session_id.clone(),
                authorization_revision: identity::authorization_revision(&tx)?,
                authorized_at: clock.at(),
                arm: arm.clone(),
            }));
        admission_codec::validate_input(&input, &self.redactor)?;
        let record = admission_codec::Record::new(input, None);
        let stored = self.persist_supervision_record(&tx, &record)?;
        self.validate_supervision_write(&tx)?;
        let receipt = supervision::arm_receipt(arm, &request.namespace, &stored)?;
        self.commit_legacy_named_control(tx, clock, |tx, current| {
            anyhow::ensure!(tx.lease.active, ControlFailure::SupervisionInactive);
            let (seq, hash) = audit_head(tx)?;
            conflict(seq == stored.seq && hash == stored.hash)?;
            let before_seq = seq.checked_add(1).context("audit sequence exhausted")?;
            let actual = control_store::worker_at(tx, &arm.worker_id, before_seq)?
                .ok_or(ControlFailure::NotFound)?;
            conflict(serde_json::to_value(actual)? == parent_value)?;
            let (human, broker) = self.require_legacy_named_execution(
                tx,
                broker,
                request.binding_ref,
                &request.namespace,
                &parent,
                current,
            )?;
            Ok((Some(human), broker))
        })?;
        release_verification_receipt(Some(human_deadline), broker_deadline)?;
        Ok(ArmResultV1 {
            replayed: false,
            arm: arm.clone(),
            receipt,
        })
    }

    /// Reconcile the exact verification Arm under its original broker identity.
    #[allow(
        dead_code,
        reason = "private verification Arm transport is composed separately"
    )]
    pub(crate) fn read_legacy_named_import_verification_arm(
        &self,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedImportVerificationArmRequestV2,
    ) -> anyhow::Result<Option<ArmResultV1>> {
        request.validate()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
        let tx = self.begin_checked(&mut db)?;
        let lookup = authorize_verification_arm_lookup(&tx, broker, request)?;
        let retained = retained_verification_arm(&tx, request)?;
        self.finish_named_import_lookup(tx, broker, &lookup.subject.clock)?;
        release_verification_receipt(None, lookup.broker_deadline)?;
        Ok(retained)
    }
}
