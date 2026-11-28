//! Legacy prospective process custody. No Full3 ledger or retirement authority.
use super::super::legacy_named_stop::NamedImportVerificationArmBirthV2;
use super::*;
use crate::{
    broker::{
        journal::{AcceptedJobV1, canonical},
        lifecycle::LocalCommandRequestV1,
    },
    control::{ArmResultV1, session_control::LegacyNamedImportArmRequestV1},
};

/// Result of one exact native attempt. A native refusal can report a site
/// after installation. Only BeforeSend proves the business frame was not sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LegacyNamedProspectiveFailure {
    BeforeSend(EnforcerError),
    Refused(LegacyReservationRefusalV1),
    StaleAcknowledged(EnforcerError),
    OutcomeUnknown(EnforcerError),
}

impl LegacyNamedProspectiveFailure {
    fn refusal(self) -> LegacyReservationRefusalV1 {
        match self {
            Self::Refused(refusal) => refusal,
            Self::BeforeSend(code) | Self::StaleAcknowledged(code) | Self::OutcomeUnknown(code) => {
                code.into()
            }
        }
    }

    pub(super) fn code(self) -> EnforcerError {
        self.refusal().code()
    }
}

/// Live ACK of this client's exact reservation and guardian custody. The
/// original job arbiter remains in the retained target.
pub(crate) struct LegacyNamedProspectiveReservation {
    client: EnforcerClient,
    retained: Arc<LocalProspective>,
    expected: u64,
    custody_nonce: Uuid,
}

#[allow(
    dead_code,
    reason = "the named Legacy service caller is composed separately"
)]
pub(crate) enum LegacyNamedProspectiveBind {
    Bound(Box<LegacyNamedProspectiveBindAck>),
    Replayed,
}

/// Exact native ACK data. This never provides a LocalSlot or a claim right.
#[allow(
    dead_code,
    reason = "the named Legacy service caller is composed separately"
)]
pub(crate) struct LegacyNamedProspectiveBindAck {
    registration: RegistrationV1,
    status: EnforcerSlotStatusV1,
    generation: CancelGeneration,
}

#[allow(
    dead_code,
    reason = "the named Legacy service caller is composed separately"
)]
impl LegacyNamedProspectiveBindAck {
    pub(crate) fn registration(&self) -> &RegistrationV1 {
        &self.registration
    }

    pub(crate) fn status(&self) -> &EnforcerSlotStatusV1 {
        &self.status
    }

    pub(crate) fn generation(&self) -> (&CancelGeneration, u64) {
        (&self.generation, self.status.generation)
    }
}

struct LegacyProspectiveEntry {
    retained: Arc<LocalProspective>,
    // Historical entries have no named custody. A named nonce is selected
    // only on first insertion, before any request can leave this client.
    custody_nonce: Option<Uuid>,
    can_remove_before_send: bool,
    named_bind_ack: Option<EnforcerSlotStatusV1>,
}

impl LegacyProspectiveEntry {
    fn require_custody(&self, custody_nonce: Option<Uuid>) -> EnforcerResult<()> {
        if self.custody_nonce == custody_nonce {
            Ok(())
        } else {
            Err(EnforcerError::Conflict)
        }
    }

    fn share(&mut self) -> Arc<LocalProspective> {
        // A second attempt may already have sent and dropped its reference.
        // The current Arc count alone cannot recover that history.
        self.can_remove_before_send = false;
        Arc::clone(&self.retained)
    }
}

pub(super) struct LegacyTransport {
    pub(super) recovery_budget: u32,
    prospective: Mutex<BTreeMap<String, LegacyProspectiveEntry>>,
    prearm_write_unknown: AtomicBool,
    // Exists only in the process that actually spawned this genuine fresh
    // child and matched its native epoch/nonce. Reconnection never creates it.
    fresh_startup: Mutex<Option<(Uuid, Uuid)>>,
    #[cfg(test)]
    pub(super) lose_next_bind_reply: AtomicBool,
    #[cfg(test)]
    pub(super) lose_next_begin_reply: AtomicBool,
    #[cfg(test)]
    pub(super) lose_next_reserve_reply: AtomicBool,
    #[cfg(test)]
    pub(super) fail_next_bind_before_connect: AtomicBool,
}
impl LegacyTransport {
    pub(super) fn new(recovery_budget: u32) -> Self {
        Self {
            recovery_budget,
            prospective: Mutex::new(BTreeMap::new()),
            prearm_write_unknown: AtomicBool::new(false),
            fresh_startup: Mutex::new(None),
            #[cfg(test)]
            lose_next_bind_reply: AtomicBool::new(false),
            #[cfg(test)]
            lose_next_begin_reply: AtomicBool::new(false),
            #[cfg(test)]
            lose_next_reserve_reply: AtomicBool::new(false),
            #[cfg(test)]
            fail_next_bind_before_connect: AtomicBool::new(false),
        }
    }

    fn remove_unsent_prospective(&self, retained: &Arc<LocalProspective>) -> EnforcerResult<()> {
        let mut entries = self
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        let arm = retained.target.arm_id();
        let exclusive = entries.get(arm).is_some_and(|entry| {
            entry.custody_nonce.is_some()
                && entry.can_remove_before_send
                && Arc::ptr_eq(&entry.retained, retained)
                // The map and the failed attempt are the only owners.
                && Arc::strong_count(retained) == 2
                && !retained.bound.load(Ordering::SeqCst)
        });
        if exclusive
            && retained
                .reservation
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .is_none()
        {
            entries.remove(arm);
        }
        Ok(())
    }
}
impl EnforcerClient {
    pub(in crate::broker) async fn deliver_named_import_stop(
        &self,
        projection: &crate::broker::journal::CancelStopProjectionV1,
    ) -> EnforcerResult<()> {
        self.legacy_transport()?;
        let endpoint = super::super::legacy_named_stop_ipc::NamedImportStopEndpoint::pin(
            &self.0.root,
            Some(&self.0.endpoint),
        )?;
        endpoint.deliver(projection, &self.0.capability).await
    }

    fn legacy_transport(&self) -> EnforcerResult<&LegacyTransport> {
        self.0.legacy.as_deref().ok_or(EnforcerError::Unauthorized)
    }
    pub(crate) fn legacy_require_new_prearm_identity(&self) -> EnforcerResult<()> {
        if self.0.failed.load(Ordering::SeqCst)
            || self
                .legacy_transport()?
                .prearm_write_unknown
                .load(Ordering::SeqCst)
        {
            return Err(EnforcerError::NotReady);
        }
        Ok(())
    }
    pub(crate) fn legacy_fence_unknown_prearm_write(&self) -> EnforcerResult<()> {
        self.legacy_transport()?
            .prearm_write_unknown
            .store(true, Ordering::SeqCst);
        Ok(())
    }
    pub(crate) async fn legacy_capabilities(
        &self,
    ) -> EnforcerResult<LegacyProspectiveCapabilitiesV1> {
        let legacy = self.legacy_transport()?;
        let Response::ProspectiveCapabilities { value } = self
            .0
            .request(Request::ProspectiveCapabilities {}, IO_TIMEOUT)
            .await?
        else {
            return Err(EnforcerError::Unavailable);
        };
        value.validate(
            self.0.endpoint.epoch(),
            self.0.max_slots,
            legacy.recovery_budget,
        )?;
        Ok(value)
    }
    pub(crate) async fn legacy_fresh_startup(&self) -> EnforcerResult<bool> {
        let retained = *self
            .legacy_transport()?
            .fresh_startup
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        let Some((epoch, nonce)) = retained else {
            return Ok(false);
        };
        let value = self.legacy_capabilities().await?;
        Ok(value.epoch == epoch
            && value.fresh_launch_nonce == Some(nonce)
            && value.recovery_generation.is_none()
            && !value.recovery_incomplete
            && value.known_stop_slots == 0
            && value.owned_active == Some(0))
    }
    pub(crate) async fn legacy_begin_recovery(&self) -> EnforcerResult<u64> {
        let legacy = self.legacy_transport()?;
        // Forget only startup provenance, never a prospective target/grant.
        legacy
            .fresh_startup
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .take();
        let Response::RecoveryBegun { generation } = self
            .0
            .request(Request::BeginRecovery {}, IO_TIMEOUT)
            .await?
        else {
            return Err(EnforcerError::Unavailable);
        };
        if generation == 0 || generation == u64::MAX {
            return Err(EnforcerError::Unauthorized);
        }
        let value = self.legacy_capabilities().await?;
        if value.recovery_generation != Some(generation) {
            return Err(EnforcerError::Unauthorized);
        }
        Ok(generation)
    }
    /// Historical code-only mapping kept for the existing regressions; the
    /// production path retains the closed refusal through the diagnosed call.
    #[cfg(test)]
    pub(crate) async fn legacy_reserve_prospective(
        &self,
        target: ProspectiveRegistrationV1,
        generation: CancelGeneration,
        replay_only: bool,
    ) -> EnforcerResult<()> {
        self.legacy_reserve_prospective_diagnosed(target, generation, replay_only)
            .await
            .map_err(|refusal| refusal.code())
    }
    /// Same single reservation attempt, retaining the closed refusal motif
    /// carried by this attempt's own negotiated response. The motif stays
    /// absent when the peer is historical or the response was lost, unknown
    /// or incomplete; no resend or fallback follows such a response.
    pub(crate) async fn legacy_reserve_prospective_diagnosed(
        &self,
        target: ProspectiveRegistrationV1,
        generation: CancelGeneration,
        replay_only: bool,
    ) -> Result<(), LegacyReservationRefusalV1> {
        self.reserve_legacy_prospective(target, &generation, replay_only, None)
            .await
            .map(|_| ())
            .map_err(LegacyNamedProspectiveFailure::refusal)
    }

    /// Reserve one exact named target. A stale ACK is retained for the later
    /// durable STOP path and never produces a reservation capability.
    #[allow(
        dead_code,
        reason = "the named Legacy service caller is composed separately"
    )]
    pub(crate) async fn legacy_reserve_named_prospective(
        &self,
        target: ProspectiveRegistrationV1,
        birth: &NamedImportArmBirthV1,
        generation: &CancelGeneration,
        expected: u64,
    ) -> Result<LegacyNamedProspectiveReservation, LegacyNamedProspectiveFailure> {
        let (retained, custody_nonce) = self
            .reserve_legacy_prospective(target, generation, false, Some((expected, birth)))
            .await?;
        let custody_nonce = custody_nonce.ok_or(LegacyNamedProspectiveFailure::OutcomeUnknown(
            EnforcerError::Conflict,
        ))?;
        generation
            .with_current_generation(expected, || LegacyNamedProspectiveReservation {
                client: self.clone(),
                retained,
                expected,
                custody_nonce,
            })
            .map_err(LegacyNamedProspectiveFailure::StaleAcknowledged)
    }

    /// Check one verified pending against the original named Import custody.
    /// The response carries no reservation or permission.
    #[allow(
        dead_code,
        reason = "the named verification service caller is composed separately"
    )]
    pub(in crate::broker) async fn legacy_check_named_verification_prospective(
        &self,
        birth: NamedImportVerificationArmBirthV2,
    ) -> Result<(), LegacyNamedProspectiveFailure> {
        use LegacyNamedProspectiveFailure::{
            BeforeSend, OutcomeUnknown, Refused, StaleAcknowledged,
        };

        self.legacy_require_new_prearm_identity()
            .map_err(BeforeSend)?;
        let (facts, prospective, (cancel, generation)) =
            birth.into_prospective_facts().map_err(BeforeSend)?;
        let (_, import_arm) = facts.import_predecessor().map_err(BeforeSend)?;
        let (retained, import_custody_nonce) = {
            let entries = self
                .legacy_transport()
                .map_err(BeforeSend)?
                .prospective
                .lock()
                .map_err(|_| BeforeSend(EnforcerError::Unhealthy))?;
            let entry = entries
                .get(&import_arm.arm_id)
                .ok_or(BeforeSend(EnforcerError::NotReady))?;
            let custody_nonce = entry
                .custody_nonce
                .ok_or(BeforeSend(EnforcerError::Conflict))?;
            if !Arc::ptr_eq(&entry.retained.generation.0, &cancel.0)
                || entry
                    .named_bind_ack
                    .as_ref()
                    .is_none_or(|ack| ack.generation != generation)
            {
                return Err(BeforeSend(EnforcerError::NotReady));
            }
            (Arc::clone(&entry.retained), custody_nonce)
        };
        import_arm
            .validate_for_arm(retained.target.arm())
            .map_err(|_| BeforeSend(EnforcerError::Conflict))?;
        let import_reservation = retained
            .reservation
            .lock()
            .map_err(|_| BeforeSend(EnforcerError::Unhealthy))?
            .clone()
            .ok_or(BeforeSend(EnforcerError::NotReady))?;
        let epoch = self.0.endpoint.epoch();
        import_reservation
            .validate_for(&retained.target, epoch, generation)
            .map_err(BeforeSend)?;

        let reply = self
            .0
            .request_in_generation(
                Request::CheckNamedVerificationProspective {
                    request: Box::new(NamedVerificationProspectiveCheckV2 {
                        import_reservation,
                        import_custody_nonce,
                        generation,
                        facts,
                        prospective,
                    }),
                },
                IO_TIMEOUT,
                Some((&cancel, generation)),
            )
            .await?;
        match reply {
            Response::NamedVerificationProspectiveChecked {
                epoch: checked_epoch,
                generation: checked_generation,
            } if checked_epoch == epoch && checked_generation == generation => cancel
                .with_current_generation(generation, || ())
                .map_err(StaleAcknowledged),
            Response::NamedVerificationProspectiveChecked { .. } => {
                Err(OutcomeUnknown(EnforcerError::Conflict))
            }
            Response::Error { code } => Err(Refused(code.into())),
            _ => Err(OutcomeUnknown(EnforcerError::Unavailable)),
        }
    }

    async fn reserve_legacy_prospective(
        &self,
        target: ProspectiveRegistrationV1,
        generation: &CancelGeneration,
        replay_only: bool,
        named_birth: Option<(u64, &NamedImportArmBirthV1)>,
    ) -> Result<(Arc<LocalProspective>, Option<Uuid>), LegacyNamedProspectiveFailure> {
        use LegacyNamedProspectiveFailure::{
            BeforeSend, OutcomeUnknown, Refused, StaleAcknowledged,
        };

        target.validate().map_err(BeforeSend)?;
        let named = named_birth.is_some();
        let expected = named_birth.map_or_else(|| generation.current(), |(expected, _)| expected);
        if expected == u64::MAX {
            return Err(BeforeSend(EnforcerError::NotReady));
        }
        if named {
            generation
                .with_current_generation(expected, || ())
                .map_err(BeforeSend)?;
        }
        if !replay_only {
            self.legacy_require_new_prearm_identity()
                .map_err(BeforeSend)?;
        }
        let digest = target.digest().map_err(BeforeSend)?;
        let legacy = self.legacy_transport().map_err(BeforeSend)?;
        let (retained, custody_nonce) = {
            // A generic registration retains its LocalSlot under the same
            // slots-then-prospective order. Neither can adopt the other's ID.
            let _slots = if named {
                let slots = self
                    .0
                    .slots
                    .lock()
                    .map_err(|_| BeforeSend(EnforcerError::Unhealthy))?;
                if slots.contains_key(target.arm_id()) {
                    return Err(BeforeSend(EnforcerError::Conflict));
                }
                Some(slots)
            } else {
                None
            };
            let mut map = legacy
                .prospective
                .lock()
                .map_err(|_| BeforeSend(EnforcerError::Unhealthy))?;
            if let Some(old) = map.get_mut(target.arm_id()) {
                if old.custody_nonce.is_some() != named
                    || old.retained.digest != digest
                    || !Arc::ptr_eq(&old.retained.generation.0, &generation.0)
                {
                    return Err(BeforeSend(EnforcerError::Conflict));
                }
                (old.share(), old.custody_nonce)
            } else {
                if map.len() >= self.0.max_slots as usize {
                    return Err(BeforeSend(EnforcerError::Capacity));
                }
                let retained = Arc::new(LocalProspective {
                    target,
                    digest,
                    generation: generation.clone(),
                    reservation: Mutex::new(None),
                    bound: AtomicBool::new(false),
                    original_exchange_nonce: Mutex::new(None),
                });
                let custody_nonce = named.then(Uuid::new_v4);
                map.insert(
                    retained.target.arm_id().into(),
                    LegacyProspectiveEntry {
                        retained: Arc::clone(&retained),
                        custody_nonce,
                        can_remove_before_send: named,
                        named_bind_ack: None,
                    },
                );
                (retained, custody_nonce)
            }
        };
        // Retain the exact target before transport. Only a proven BeforeSend
        // can release an exclusive named entry.
        let attempt = async {
            self.legacy_capabilities().await.map_err(BeforeSend)?;
            // Generation may have changed during the capabilities request.
            if !replay_only && retained.generation.current() != expected {
                return Err(BeforeSend(EnforcerError::NotReady));
            }
            let prospective = Box::new(retained.target.clone());
            let request = if let Some((_, birth)) = named_birth {
                Request::ReserveNamedProspective {
                    prospective,
                    generation: expected,
                    custody_nonce: custody_nonce.ok_or(BeforeSend(EnforcerError::Conflict))?,
                    birth: Box::new(birth.clone()),
                }
            } else {
                Request::ReserveProspective {
                    prospective,
                    generation: expected,
                    replay_only,
                }
            };
            self.0
                .request_reservation(
                    request,
                    REGISTER_TIMEOUT,
                    named_birth.map(|(expected, _)| (&retained.generation, expected)),
                )
                .await
        }
        .await;
        if named && matches!(&attempt, Err(BeforeSend(_))) {
            legacy
                .remove_unsent_prospective(&retained)
                .map_err(BeforeSend)?;
        }
        let (reply, negotiated) = attempt?;
        let reservation = match reply {
            Response::ProspectiveReserved { reservation } => reservation,
            Response::ProspectiveRefused { code, motif } => {
                // The closed motif is only meaningful on this negotiated
                // connection, and only for the not_ready refusal sites.
                if !negotiated || code != EnforcerError::NotReady {
                    return Err(OutcomeUnknown(EnforcerError::Unavailable));
                }
                return Err(Refused(LegacyReservationRefusalV1::with_motif(code, motif)));
            }
            Response::Error { code } => return Err(Refused(code.into())),
            reply => return Err(OutcomeUnknown(response_error(reply))),
        };
        reservation
            .validate_for(
                &retained.target,
                self.0.endpoint.epoch(),
                reservation.cancel_generation,
            )
            .map_err(OutcomeUnknown)?;
        if (!replay_only && reservation.cancel_generation != expected)
            || (replay_only && reservation.cancel_generation > expected)
        {
            return Err(OutcomeUnknown(EnforcerError::Conflict));
        }
        {
            let mut previous = retained
                .reservation
                .lock()
                .map_err(|_| OutcomeUnknown(EnforcerError::Unhealthy))?;
            if let Some(old) = &*previous {
                if serde_json::to_vec(old)
                    .map_err(|_| OutcomeUnknown(EnforcerError::InvalidInput))?
                    != serde_json::to_vec(&reservation)
                        .map_err(|_| OutcomeUnknown(EnforcerError::InvalidInput))?
                {
                    return Err(OutcomeUnknown(EnforcerError::Conflict));
                }
            } else {
                *previous = Some(reservation);
            }
        }
        if !named && retained.generation.current() != expected {
            self.legacy_cancel_prospective(&retained.target, retained.generation.current())
                .await
                .map_err(OutcomeUnknown)?;
            return Err(StaleAcknowledged(EnforcerError::NotReady));
        }
        Ok((retained, custody_nonce))
    }
    fn retained_prospective(
        &self,
        target: &ProspectiveRegistrationV1,
        generation: &CancelGeneration,
    ) -> EnforcerResult<Arc<LocalProspective>> {
        self.retained_prospective_with_custody(target, generation, None)
    }

    fn retained_prospective_with_custody(
        &self,
        target: &ProspectiveRegistrationV1,
        generation: &CancelGeneration,
        custody_nonce: Option<Uuid>,
    ) -> EnforcerResult<Arc<LocalProspective>> {
        let digest = target.digest()?;
        let mut entries = self
            .legacy_transport()?
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        let entry = entries
            .get_mut(target.arm_id())
            .ok_or(EnforcerError::NotReady)?;
        entry.require_custody(custody_nonce)?;
        if entry.retained.digest != digest
            || !Arc::ptr_eq(&entry.retained.generation.0, &generation.0)
        {
            return Err(EnforcerError::Conflict);
        }
        Ok(entry.share())
    }
    pub(crate) async fn legacy_begin_prospective_arm_exchange(
        &self,
        target: &ProspectiveRegistrationV1,
        generation: &CancelGeneration,
    ) -> EnforcerResult<ProspectiveArmExchange> {
        self.begin_legacy_prospective_arm_exchange(target, generation, None)
            .await
            .map_err(LegacyNamedProspectiveFailure::code)
    }

    /// Consume the original named reservation and its exact arbiter. A stale
    /// ACK has no exchange capability and leaves native STOP to the journal.
    #[allow(
        dead_code,
        reason = "the named Legacy service caller is composed separately"
    )]
    pub(crate) async fn legacy_begin_named_prospective_arm_exchange(
        &self,
        reserved: LegacyNamedProspectiveReservation,
    ) -> Result<ProspectiveArmExchange, LegacyNamedProspectiveFailure> {
        if !Arc::ptr_eq(&self.0, &reserved.client.0) {
            return Err(LegacyNamedProspectiveFailure::BeforeSend(
                EnforcerError::Conflict,
            ));
        }
        let retained = Arc::clone(&reserved.retained);
        self.begin_legacy_prospective_arm_exchange(
            &retained.target,
            &retained.generation,
            Some(reserved),
        )
        .await
    }

    async fn begin_legacy_prospective_arm_exchange(
        &self,
        target: &ProspectiveRegistrationV1,
        generation: &CancelGeneration,
        named: Option<LegacyNamedProspectiveReservation>,
    ) -> Result<ProspectiveArmExchange, LegacyNamedProspectiveFailure> {
        use LegacyNamedProspectiveFailure::{
            BeforeSend, OutcomeUnknown, Refused, StaleAcknowledged,
        };

        if self.0.failed.load(Ordering::SeqCst) {
            return Err(BeforeSend(EnforcerError::Unhealthy));
        }
        let named_expected = named.as_ref().map(|owner| owner.expected);
        let expected = named_expected.unwrap_or_else(|| generation.current());
        if expected == u64::MAX {
            return Err(BeforeSend(EnforcerError::NotReady));
        }
        if named_expected.is_some() {
            generation
                .with_current_generation(expected, || ())
                .map_err(BeforeSend)?;
        }
        let custody_nonce = named.as_ref().map(|owner| owner.custody_nonce);
        let retained = self
            .retained_prospective_with_custody(target, generation, custody_nonce)
            .map_err(BeforeSend)?;
        let reservation = retained
            .reservation
            .lock()
            .map_err(|_| BeforeSend(EnforcerError::Unhealthy))?
            .clone()
            .ok_or(BeforeSend(EnforcerError::NotReady))?;
        let request = if let Some(owner) = &named {
            if !Arc::ptr_eq(&retained, &owner.retained) || reservation.cancel_generation != expected
            {
                return Err(BeforeSend(EnforcerError::Conflict));
            }
            Request::BeginNamedProspectiveArmExchange {
                reservation,
                generation: expected,
                custody_nonce: owner.custody_nonce,
            }
        } else {
            retained
                .original_exchange_nonce
                .lock()
                .map_err(|_| BeforeSend(EnforcerError::Unhealthy))?
                .take();
            Request::BeginProspectiveArmExchange {
                reservation,
                generation: expected,
            }
        };
        let reply = self
            .0
            .request_in_generation(
                request,
                REGISTER_TIMEOUT,
                named_expected.map(|expected| (generation, expected)),
            )
            .await?;
        let original_exchange_nonce = match reply {
            Response::ProspectiveArmExchange {
                original_exchange_nonce,
            } => original_exchange_nonce,
            Response::Error { code } => return Err(Refused(code.into())),
            reply => return Err(OutcomeUnknown(response_error(reply))),
        };
        if original_exchange_nonce.is_some_and(|nonce| nonce.is_nil()) {
            return Err(OutcomeUnknown(EnforcerError::Unauthorized));
        }
        let exchange = || ProspectiveArmExchange {
            epoch: self.0.endpoint.epoch(),
            arm_id: target.arm_id().into(),
            prospective_hash: retained.digest.clone(),
            generation: expected,
            original_exchange_nonce,
            legacy_named_reservation: named,
        };
        if named_expected.is_some() {
            return generation
                .with_current_generation(expected, exchange)
                .map_err(StaleAcknowledged);
        }
        if generation.current() != expected {
            self.legacy_cancel_prospective(target, generation.current())
                .await
                .map_err(OutcomeUnknown)?;
            return Err(StaleAcknowledged(EnforcerError::NotReady));
        }
        Ok(exchange())
    }

    /// Bind one original named exchange to the exact Writer-acknowledged
    /// Arm. The native ACK remains data for the separate durable STOP path.
    #[allow(
        dead_code,
        reason = "the named Legacy service caller is composed separately"
    )]
    pub(crate) async fn legacy_bind_named_prospective(
        &self,
        job: &AcceptedJobV1,
        arm_ack: &ArmResultV1,
        exchange: ProspectiveArmExchange,
    ) -> Result<LegacyNamedProspectiveBind, LegacyNamedProspectiveFailure> {
        use LegacyNamedProspectiveFailure::{BeforeSend, OutcomeUnknown, StaleAcknowledged};

        let owner = exchange
            .legacy_named_reservation
            .as_ref()
            .ok_or(BeforeSend(EnforcerError::Unauthorized))?;
        let original_nonce = exchange
            .original_exchange_nonce
            .ok_or(BeforeSend(EnforcerError::NotReady))?;
        let expected = owner.expected;
        if !Arc::ptr_eq(&self.0, &owner.client.0)
            || exchange.epoch != self.0.endpoint.epoch()
            || exchange.arm_id != owner.retained.target.arm_id()
            || exchange.prospective_hash != owner.retained.digest
            || exchange.generation != expected
        {
            return Err(BeforeSend(EnforcerError::Conflict));
        }
        if arm_ack.replayed {
            return Err(BeforeSend(EnforcerError::NotReady));
        }
        if self.0.failed.load(Ordering::SeqCst) {
            return Err(BeforeSend(EnforcerError::Unhealthy));
        }
        let generation = &owner.retained.generation;
        generation
            .with_current_generation(expected, || ())
            .map_err(BeforeSend)?;
        let retained = self
            .retained_prospective_with_custody(
                &owner.retained.target,
                generation,
                Some(owner.custody_nonce),
            )
            .map_err(BeforeSend)?;
        if !Arc::ptr_eq(&retained, &owner.retained) || retained.bound.load(Ordering::SeqCst) {
            return Err(BeforeSend(EnforcerError::Conflict));
        }
        let registration =
            registration_for_named_bind(job, arm_ack, &retained.target).map_err(BeforeSend)?;
        let digest = registration.digest().map_err(BeforeSend)?;
        let reservation = retained
            .reservation
            .lock()
            .map_err(|_| BeforeSend(EnforcerError::Unhealthy))?
            .clone()
            .ok_or(BeforeSend(EnforcerError::NotReady))?;
        reservation
            .validate_for(&retained.target, exchange.epoch, expected)
            .map_err(BeforeSend)?;

        let reply = self
            .0
            .request_in_generation(
                Request::BindNamedProspective {
                    reservation,
                    registration: Box::new(registration.clone()),
                    generation: expected,
                    original_exchange_nonce: original_nonce,
                    custody_nonce: owner.custody_nonce,
                },
                REGISTER_TIMEOUT,
                Some((generation, expected)),
            )
            .await?;
        let (status, replayed) = match reply {
            Response::Registered { value, replayed } => (*value, replayed),
            // The native bind can install its actor before reporting a
            // readiness error. Only the correlated Registered ACK settles it.
            reply => return Err(OutcomeUnknown(response_error(reply))),
        };
        if status.arm_id != registration.arm_id()
            || status.registration_hash != digest
            || status.generation < expected
        {
            return Err(OutcomeUnknown(EnforcerError::Conflict));
        }
        // Preserve the correlated ACK even if invalidation or replay prevents
        // a handoff. This map has no generic LocalSlot or recovery task.
        {
            let mut entries = self
                .legacy_transport()
                .map_err(OutcomeUnknown)?
                .prospective
                .lock()
                .map_err(|_| OutcomeUnknown(EnforcerError::Unhealthy))?;
            let entry = entries
                .get_mut(registration.arm_id())
                .ok_or(OutcomeUnknown(EnforcerError::Conflict))?;
            entry
                .require_custody(Some(owner.custody_nonce))
                .map_err(OutcomeUnknown)?;
            if !Arc::ptr_eq(&entry.retained, &retained) || entry.named_bind_ack.is_some() {
                return Err(OutcomeUnknown(EnforcerError::Conflict));
            }
            entry.named_bind_ack = Some(status.clone());
            entry.retained.bound.store(true, Ordering::SeqCst);
        }
        generation
            .with_current_generation(expected, || {
                if status.generation != expected {
                    return Err(StaleAcknowledged(EnforcerError::NotReady));
                }
                if self.0.failed.load(Ordering::SeqCst) {
                    return Err(StaleAcknowledged(EnforcerError::Unhealthy));
                }
                if replayed {
                    return Ok(LegacyNamedProspectiveBind::Replayed);
                }
                if status.phase != SlotPhaseV1::Ready || status.prepared_once {
                    return Err(StaleAcknowledged(EnforcerError::NotReady));
                }
                Ok(LegacyNamedProspectiveBind::Bound(Box::new(
                    LegacyNamedProspectiveBindAck {
                        registration,
                        status,
                        generation: generation.clone(),
                    },
                )))
            })
            .map_err(StaleAcknowledged)?
    }

    pub(crate) async fn legacy_bind_prospective(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
        exchange: ProspectiveArmExchange,
    ) -> EnforcerResult<EnforcedSlot> {
        if exchange.legacy_named_reservation.is_some() {
            return Err(EnforcerError::Unauthorized);
        }
        let observe_only = !exchange.permits_original();
        let retained = self.retained_prospective(&registration.prospective()?, &generation)?;
        if !observe_only && self.0.failed.load(Ordering::SeqCst) {
            self.legacy_cancel_prospective(&retained.target, generation.current())
                .await?;
            return Err(EnforcerError::Unhealthy);
        }
        if exchange.epoch != self.0.endpoint.epoch()
            || exchange.arm_id != registration.arm_id()
            || exchange.prospective_hash != retained.digest
        {
            return Err(EnforcerError::Conflict);
        }
        if exchange.generation != generation.current() {
            self.legacy_cancel_prospective(&retained.target, generation.current())
                .await?;
            return Err(EnforcerError::NotReady);
        }
        if retained
            .reservation
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .is_none()
        {
            return Err(EnforcerError::NotReady);
        }
        *retained
            .original_exchange_nonce
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)? = exchange.original_exchange_nonce;
        let expected = generation.current();
        let result = self
            .register_current(registration, generation.clone(), observe_only)
            .await;
        if generation.current() != expected {
            self.legacy_cancel_prospective(&retained.target, generation.current())
                .await?;
            return Err(EnforcerError::NotReady);
        }
        result
    }
    pub(crate) async fn legacy_cancel_prospective(
        &self,
        target: &ProspectiveRegistrationV1,
        generation: u64,
    ) -> EnforcerResult<()> {
        let digest = target.digest()?;
        let retained = {
            let mut entries = self
                .legacy_transport()?
                .prospective
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            let entry = entries
                .get_mut(target.arm_id())
                .ok_or(EnforcerError::NotReady)?;
            entry.require_custody(None)?;
            if entry.retained.digest != digest {
                return Err(EnforcerError::Conflict);
            }
            entry.share()
        };
        retained
            .original_exchange_nonce
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .take();
        let reservation = retained
            .reservation
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .clone()
            .ok_or(EnforcerError::NotReady)?;
        match self
            .0
            .request(
                Request::CancelProspective {
                    reservation,
                    generation,
                },
                IO_TIMEOUT,
            )
            .await?
        {
            Response::Acknowledged {} => Ok(()),
            reply => Err(response_error(reply)),
        }
    }
}

fn registration_for_named_bind(
    job: &AcceptedJobV1,
    arm_ack: &ArmResultV1,
    target: &ProspectiveRegistrationV1,
) -> EnforcerResult<RegistrationV1> {
    let checkpoint = job.broker_checkpoint_v3().ok_or(EnforcerError::Conflict)?;
    if checkpoint
        .commands()
        .iter()
        .any(|command| matches!(command.request(), LocalCommandRequestV1::CancelRequested))
    {
        return Err(EnforcerError::NotReady);
    }
    let named = checkpoint
        .broker()
        .supervision()
        .legacy_named_admission()
        .ok_or(EnforcerError::Conflict)?;
    let plan = named.plan();
    let request = named.import_arm_request().ok_or(EnforcerError::Conflict)?;
    let receipt = named
        .import_arm()
        .and_then(|arm| arm.receipt())
        .ok_or(EnforcerError::Conflict)?;
    let owned = named
        .import_creation()
        .and_then(|creation| creation.owned())
        .ok_or(EnforcerError::Conflict)?;
    let worker = named
        .admission_result()
        .and_then(|result| result.worker.as_ref())
        .ok_or(EnforcerError::Conflict)?;
    let scope = job.scope();
    let namespace = scope.control_namespace_uuid().to_string();
    let attempt =
        Uuid::parse_str(&worker.attempt.attempt_id).map_err(|_| EnforcerError::InvalidInput)?;
    let exact_id = LegacyNamedImportArmRequestV1::operation_id(plan.binding_ref, attempt);
    let invalid = |_| EnforcerError::InvalidInput;
    let arm = canonical(&request.arm).map_err(invalid)?;
    if job.operation_id() != plan.operation_id
        || job.intent().legacy_named_plan() != Some(plan)
        || plan.broker_profile_id != scope.profile_id()
        || request.binding_ref != plan.binding_ref
        || request.namespace != namespace
        || receipt.control_namespace_uuid != namespace
        || request.arm.operation_id != exact_id
        || request.arm.worker_id != worker.worker_id
        || request.arm.attempt_id != worker.attempt.attempt_id
        || target.tenant_id() != scope.tenant_id()
        || target.control_origin_hash()
            != hex::encode(Sha256::digest(scope.gateway_origin().as_bytes()))
        || arm != canonical(target.arm()).map_err(invalid)?
        || arm != canonical(&arm_ack.arm).map_err(invalid)?
        || canonical(receipt).map_err(invalid)? != canonical(&arm_ack.receipt).map_err(invalid)?
        || canonical(owned).map_err(invalid)? != canonical(target.owned()).map_err(invalid)?
    {
        return Err(EnforcerError::Conflict);
    }
    named
        .require_import_creation_binding(target.authority())
        .map_err(|_| EnforcerError::Conflict)?;
    RegistrationV1::new(
        target.control_origin_hash().into(),
        target.tenant_id().into(),
        request.arm,
        receipt.clone(),
        target.authority().clone(),
        target.owned().clone(),
    )
}

impl Client {
    pub(super) fn require_historical_legacy_registration(
        &self,
        arm_id: &str,
    ) -> EnforcerResult<()> {
        if let Some(legacy) = &self.legacy
            && let Some(entry) = legacy
                .prospective
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .get(arm_id)
        {
            entry.require_custody(None)?;
        }
        Ok(())
    }

    pub(super) fn legacy_observe_only(
        &self,
        registration: &RegistrationV1,
        requested: bool,
    ) -> EnforcerResult<bool> {
        let Some(legacy) = &self.legacy else {
            return Ok(requested);
        };
        let map = legacy
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        if let Some(entry) = map.get(registration.arm_id()) {
            entry.require_custody(None)?;
            if !requested && !entry.retained.bound.load(Ordering::SeqCst) {
                entry.retained.target.matches_registration(registration)?;
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub(super) fn adapt_legacy_request(&self, request: Request) -> EnforcerResult<Request> {
        let Request::Register {
            registration,
            generation,
            observe_only,
        } = request
        else {
            return Ok(request);
        };
        let Some(legacy) = &self.legacy else {
            return Err(EnforcerError::Unauthorized);
        };
        let retained = {
            let mut entries = legacy
                .prospective
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            if let Some(entry) = entries.get_mut(registration.arm_id()) {
                entry.require_custody(None)?;
                Some(entry.share())
            } else {
                None
            }
        };
        let Some(retained) = retained.filter(|retained| !retained.bound.load(Ordering::SeqCst))
        else {
            // This also constrains an older live daemon: receipt-only generic
            // Register is observation/STOP, even if its old implementation
            // would otherwise accept a new Original from that receipt.
            return Ok(Request::Register {
                registration,
                generation,
                observe_only: true,
            });
        };
        retained.target.matches_registration(&registration)?;
        let reservation = retained
            .reservation
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .clone()
            .ok_or(EnforcerError::NotReady)?;
        reservation.validate_for(
            &retained.target,
            self.endpoint.epoch(),
            reservation.cancel_generation,
        )?;
        if (!observe_only && generation != reservation.cancel_generation)
            || (observe_only && generation < reservation.cancel_generation)
        {
            return Err(EnforcerError::Conflict);
        }
        // This destructive take precedes every connect/write. Neither a lost
        // Bind notification nor generic Register can export it a second time.
        let original = retained
            .original_exchange_nonce
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .take();
        Ok(Request::BindProspective {
            reservation,
            registration,
            generation,
            observe_only,
            original_exchange_nonce: if observe_only { None } else { original },
        })
    }
    pub(super) fn legacy_known_slots(
        &self,
        slots: &BTreeMap<String, Arc<LocalSlot>>,
    ) -> EnforcerResult<usize> {
        let legacy = self.legacy.as_ref().ok_or(EnforcerError::Unauthorized)?;
        let prospective = legacy
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        if prospective.values().any(|entry| {
            entry
                .retained
                .reservation
                .lock()
                .map_or(true, |value| value.is_none())
        }) {
            return Err(EnforcerError::NotReady);
        }
        Ok(slots.len()
            + prospective
                .keys()
                .filter(|arm| !slots.contains_key(*arm))
                .count())
    }
    pub(super) fn legacy_bound(&self, arm_id: &str) -> EnforcerResult<()> {
        if let Some(legacy) = &self.legacy
            && let Some(entry) = legacy
                .prospective
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .get(arm_id)
        {
            entry.require_custody(None)?;
            entry.retained.bound.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[path = "client_legacy_launch.rs"]
mod launch;

#[cfg(test)]
#[path = "client_legacy_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "reservation_motif_tests.rs"]
mod reservation_motif_tests;

#[cfg(test)]
pub(in crate::broker) async fn connect_named_legacy_fixture(
    fixture: &super::super::server::test_support::LegacyIpcFixture,
) -> EnforcerResult<EnforcerClient> {
    let root = fixture.root();
    EnforcerClient::connect(
        root,
        &std::env::current_exe().map_err(|_| EnforcerError::Unavailable)?,
        load_endpoint(root)?.ok_or(EnforcerError::NotReady)?,
        capability(root, false)?,
        1,
        0,
        false,
    )
    .await
    .map_err(|failure| match failure {
        ConnectFailure::Absent => EnforcerError::Unavailable,
        ConnectFailure::Other(error) => error,
    })
}

#[cfg(test)]
impl LegacyNamedProspectiveBindAck {
    /// Send the fixture's retained control claim through the real Legacy
    /// Prepare transport. The fixture receipt is not a Service start permit.
    pub(in crate::broker) async fn prepare_named_import_for_fixture(
        self: Box<Self>,
        claim: LegacyNamedImportClaimRequestV1,
        receipt: MutationReceiptV1,
    ) -> anyhow::Result<()> {
        self.require_live_custody()?;
        let (generation, expected) = self.generation();
        let reservation = self
            .owner
            .retained
            .reservation
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .clone()
            .ok_or(EnforcerError::NotReady)?;
        let clock = NativeHostClock::new()?;
        let before_ns = clock.sample()?.continuous_ns()?;
        let reply = self
            .owner
            .client
            .0
            .request_in_generation(
                Request::PrepareNamedImport {
                    request: Box::new(NamedImportPrepareV1 {
                        reservation,
                        registration_hash: self.status.registration_hash.clone(),
                        generation: expected,
                        custody_nonce: self.owner.custody_nonce,
                        original_exchange_nonce: self.original_exchange_nonce,
                        claim,
                        receipt,
                    }),
                },
                IO_TIMEOUT,
                Some((generation, expected)),
            )
            .await
            .map_err(LegacyNamedProspectiveFailure::code)?;
        let Response::Prepared { value } = reply else {
            return Err(response_error(reply).into());
        };
        require_named_prepare_ack(
            &self.status,
            self.registration.arm(),
            &value,
            before_ns,
            &clock.sample()?,
        )
        .map_err(LegacyNamedProspectiveFailure::code)?;
        Ok(())
    }
}

#[cfg(test)]
impl EnforcerClient {
    pub(in crate::broker) async fn check_recorded_named_verification_for_fixture(
        &self,
        fixture: &super::super::server::test_support::LegacyIpcFixture,
        import_arm_id: &str,
        recorded: Box<crate::broker::writer::RecordedLegacyNamedImportVerificationArm>,
        target: ProspectiveRegistrationV1,
        after_stop: bool,
    ) -> anyhow::Result<()> {
        let verification_arm_id = target.arm_id().to_owned();
        let original = fixture
            .pending_reservation(import_arm_id)
            .ok_or(EnforcerError::NotReady)?;
        let before = self.legacy_capabilities().await?;
        assert_eq!(fixture.reserve_requests(), 1);
        assert_eq!(before.known_stop_slots, 1);
        assert_eq!(before.owned_active, Some(1));
        assert!(!before.recovery_incomplete);

        let arm = if after_stop {
            let response = self
                .0
                .request(
                    Request::Slots {
                        arm_ids: vec![import_arm_id.to_owned()],
                    },
                    IO_TIMEOUT,
                )
                .await?;
            let Response::Slots { values } = response else {
                return Err(response_error(response).into());
            };
            assert!(matches!(
                values.as_slice(),
                [value] if value.arm_id == import_arm_id
                    && value.generation == recorded.generation().1
                    && value.phase == SlotPhaseV1::Stopping && value.prepared_once
            ));
            Some(target.arm().clone())
        } else {
            None
        };
        let birth = NamedImportVerificationArmBirthV2::from_recorded(recorded, target)?;
        let checked = self
            .legacy_check_named_verification_prospective(birth)
            .await;
        if let Some(arm) = arm {
            assert_eq!(
                checked,
                Err(LegacyNamedProspectiveFailure::Refused(
                    EnforcerError::NotReady.into()
                ))
            );
            arm.validate_clock_check(&NativeHostClock::new()?.sample()?)?;
        } else {
            checked.map_err(LegacyNamedProspectiveFailure::code)?;
        }

        let after = self.legacy_capabilities().await?;
        assert_eq!(after.epoch, before.epoch);
        assert_eq!(after.known_stop_slots, before.known_stop_slots);
        assert_eq!(after.owned_active, before.owned_active);
        assert!(!after.recovery_incomplete);
        assert_eq!(fixture.reserve_requests(), 1);
        let retained = fixture
            .pending_reservation(import_arm_id)
            .ok_or(EnforcerError::NotReady)?;
        assert_eq!(canonical(&retained)?, canonical(&original)?);
        assert!(fixture.pending_reservation(&verification_arm_id).is_none());
        assert!(matches!(
            self.0
                .request(
                    Request::Slots {
                        arm_ids: vec![verification_arm_id],
                    },
                    IO_TIMEOUT,
                )
                .await?,
            Response::Error {
                code: EnforcerError::Conflict
            }
        ));
        let local_slots = self.0.slots.lock().map_err(|_| EnforcerError::Unhealthy)?;
        assert!(local_slots.is_empty());
        assert_eq!(self.0.legacy_known_slots(&local_slots)?, 1);
        Ok(())
    }
}

#[cfg(test)]
#[tokio::test]
async fn named_reservation_retains_stale_ack_without_issuing_stop() -> anyhow::Result<()> {
    use super::super::server::test_support::{LegacyIpcFixture, named_import_registration};

    let fixture = LegacyIpcFixture::start(1, 0).await;
    let root = fixture.root();
    let client = connect_named_legacy_fixture(&fixture).await?;
    let (registration, birth) = named_import_registration(root.path());
    let target = registration.prospective()?;
    let generation = CancelGeneration::new();
    let expected = generation.current();
    let observation = async {
        let (unsent, unsent_birth) = named_import_registration(root.path());
        let unsent = unsent.prospective()?;
        let unsent_generation = CancelGeneration::new();
        let (held, resume) = fixture.pause_capabilities_reply();
        let reserve_never_sent = client.legacy_reserve_named_prospective(
            unsent.clone(),
            &unsent_birth,
            &unsent_generation,
            unsent_generation.current(),
        );
        tokio::pin!(reserve_never_sent);
        tokio::select! {
            _ = &mut reserve_never_sent => anyhow::bail!("reservation did not reach the capabilities barrier"),
            held = tokio::time::timeout(IO_TIMEOUT, held) => held??,
        }
        unsent_generation.invalidate();
        resume.send(()).map_err(|_| EnforcerError::Unavailable)?;
        let before_send = tokio::time::timeout(REGISTER_TIMEOUT, reserve_never_sent).await?.err();
        anyhow::ensure!(
            before_send == Some(LegacyNamedProspectiveFailure::BeforeSend(EnforcerError::NotReady)),
            "the revoked generation did not stop before the reservation write"
        );
        anyhow::ensure!(
            fixture.reserve_requests() == 0 && fixture.pending_reservation(unsent.arm_id()).is_none(),
            "the unsent attempt left a native reservation"
        );
        let local_slots = {
            let slots = client.0.slots.lock().map_err(|_| EnforcerError::Unhealthy)?;
            client.0.legacy_known_slots(&slots)?
        };
        anyhow::ensure!(local_slots == 0, "the unsent attempt retained local capacity");

        // The sole slot is available to the next exact request. Once its ACK
        // exists, invalidation must preserve the reservation for durable STOP.
        let (held, resume) = fixture.pause_reserve_ack();
        let reserve =
            client.legacy_reserve_named_prospective(target.clone(), &birth, &generation, expected);
        tokio::pin!(reserve);
        tokio::select! {
            _ = &mut reserve => anyhow::bail!("reservation did not reach the native ACK barrier"),
            held = tokio::time::timeout(IO_TIMEOUT, held) => held??,
        }
        generation.invalidate();
        resume.send(()).map_err(|_| EnforcerError::Unavailable)?;
        let failure = tokio::time::timeout(REGISTER_TIMEOUT, reserve).await?.err();
        let (retained, custody_nonce) = {
            let entries = client
                .legacy_transport()?
                .prospective
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            let entry = entries.get(target.arm_id()).ok_or(EnforcerError::NotReady)?;
            (
                Arc::clone(&entry.retained),
                entry.custody_nonce.ok_or(EnforcerError::NotReady)?,
            )
        };
        let reservation = retained
            .reservation
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .clone()
            .ok_or(EnforcerError::NotReady)?;
        // Inspect the native actor at the retained generation. A premature
        // CancelProspective would have closed its original exchange.
        let reply = client
            .0
            .request(
                Request::BeginNamedProspectiveArmExchange {
                    reservation,
                    generation: expected,
                    custody_nonce,
                },
                IO_TIMEOUT,
            )
            .await?;
        Ok::<_, anyhow::Error>((
            failure,
            matches!(
                reply,
                Response::ProspectiveArmExchange {
                    original_exchange_nonce: Some(_)
                }
            ),
        ))
    }
    .await;
    drop(client);
    fixture.finish().await;
    let (failure, original_native_exchange) = observation?;
    assert_eq!(
        failure,
        Some(LegacyNamedProspectiveFailure::StaleAcknowledged(
            EnforcerError::NotReady
        ))
    );
    assert!(original_native_exchange);
    Ok(())
}

#[cfg(test)]
#[tokio::test]
async fn named_exchange_never_enters_the_historical_bind() -> anyhow::Result<()> {
    use super::super::server::test_support::{LegacyIpcFixture, named_import_registration};

    let fixture = LegacyIpcFixture::start(1, 0).await;
    let client = connect_named_legacy_fixture(&fixture).await?;
    let observation = async {
        let (registration, birth) = named_import_registration(fixture.root().path());
        let target = registration.prospective()?;
        let generation = CancelGeneration::new();
        let reserved = client
            .legacy_reserve_named_prospective(
                target.clone(),
                &birth,
                &generation,
                generation.current(),
            )
            .await
            .map_err(LegacyNamedProspectiveFailure::code)?;

        for replay_only in [false, true] {
            let historical = client
                .legacy_reserve_prospective_diagnosed(target.clone(), generation.clone(), replay_only)
                .await;
            anyhow::ensure!(
                matches!(historical, Err(refusal) if refusal.code() == EnforcerError::Conflict && refusal.motif.is_none()),
                "historical reservation must refuse the retained named target"
            );
        }
        anyhow::ensure!(
            fixture.reserve_requests() == 1,
            "cross-rail Reserve reached the wire"
        );
        let historical_begin = client.legacy_begin_prospective_arm_exchange(&target, &generation).await;
        anyhow::ensure!(
            matches!(historical_begin, Err(EnforcerError::Conflict)),
            "historical Begin must refuse the retained named target"
        );
        let automatic_stop = client.legacy_cancel_prospective(&target, generation.current()).await;
        anyhow::ensure!(
            automatic_stop == Err(EnforcerError::Conflict),
            "historical automatic STOP must refuse the retained named target"
        );
        let observe = client.register_current(registration.clone(), generation.clone(), true).await;
        anyhow::ensure!(
            matches!(observe, Err(EnforcerError::Conflict)),
            "historical ObserveOnly must refuse the retained named target"
        );
        let retained_stop = client.cancel_retained(registration.clone(), generation.clone()).await;
        anyhow::ensure!(
            retained_stop == Err(EnforcerError::Conflict),
            "historical retained Cancel must refuse the named rail before retaining a slot"
        );
        let exchange = client
            .legacy_begin_named_prospective_arm_exchange(reserved)
            .await
            .map_err(LegacyNamedProspectiveFailure::code)?;
        anyhow::ensure!(
            exchange.permits_original(),
            "the test requires an original exchange"
        );
        let refusal = client
            .legacy_bind_prospective(registration, generation, exchange)
            .await
            .err();
        let slots = client
            .0
            .slots
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .len();
        Ok::<_, anyhow::Error>((refusal, slots))
    }
    .await;
    drop(client);
    fixture.finish().await;
    assert_eq!(observation?, (Some(EnforcerError::Unauthorized), 0));
    Ok(())
}
