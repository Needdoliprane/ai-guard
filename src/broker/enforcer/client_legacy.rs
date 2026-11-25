//! Legacy prospective process custody. No Full3 ledger or retirement authority.
use super::*;

pub(super) struct LegacyTransport {
    pub(super) recovery_budget: u32,
    prospective: Mutex<BTreeMap<String, Arc<LocalProspective>>>,
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
}
impl EnforcerClient {
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
        target.validate()?;
        let expected = generation.current();
        if expected == u64::MAX {
            return Err(EnforcerError::NotReady.into());
        }
        if !replay_only {
            self.legacy_require_new_prearm_identity()?;
        }
        let digest = target.digest()?;
        let legacy = self.legacy_transport()?;
        let retained = {
            let mut map = legacy
                .prospective
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            if let Some(old) = map.get(target.arm_id()) {
                if old.digest != digest || !Arc::ptr_eq(&old.generation.0, &generation.0) {
                    return Err(EnforcerError::Conflict.into());
                }
                Arc::clone(old)
            } else {
                if map.len() >= self.0.max_slots as usize {
                    return Err(EnforcerError::Capacity.into());
                }
                let retained = Arc::new(LocalProspective {
                    target,
                    digest,
                    generation,
                    reservation: Mutex::new(None),
                    bound: AtomicBool::new(false),
                    original_exchange_nonce: Mutex::new(None),
                });
                map.insert(retained.target.arm_id().into(), Arc::clone(&retained));
                retained
            }
        };
        // The exact target is retained before capability/factory transport.
        // Failure or a dropped waiter cannot erase this unknown admission.
        self.legacy_capabilities().await?;
        // A fresh reservation can already have lost its local generation
        // while capabilities was in flight. Keep the post-ACK check for
        // invalidation after this sample.
        if !replay_only && retained.generation.current() != expected {
            return Err(EnforcerError::NotReady.into());
        }
        let (reply, negotiated) = self
            .0
            .request_reservation(
                Request::ReserveProspective {
                    prospective: Box::new(retained.target.clone()),
                    generation: expected,
                    replay_only,
                },
                REGISTER_TIMEOUT,
            )
            .await?;
        let reservation = match reply {
            Response::ProspectiveReserved { reservation } => reservation,
            Response::ProspectiveRefused { code, motif } => {
                // The closed motif is only meaningful on this negotiated
                // connection, and only for the not_ready refusal sites.
                if !negotiated || code != EnforcerError::NotReady {
                    return Err(EnforcerError::Unavailable.into());
                }
                return Err(LegacyReservationRefusalV1::with_motif(code, motif));
            }
            reply => return Err(response_error(reply).into()),
        };
        reservation.validate_for(
            &retained.target,
            self.0.endpoint.epoch(),
            reservation.cancel_generation,
        )?;
        if (!replay_only && reservation.cancel_generation != expected)
            || (replay_only && reservation.cancel_generation > expected)
        {
            return Err(EnforcerError::Conflict.into());
        }
        {
            let mut previous = retained
                .reservation
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            if let Some(old) = &*previous {
                if serde_json::to_vec(old).map_err(|_| EnforcerError::InvalidInput)?
                    != serde_json::to_vec(&reservation).map_err(|_| EnforcerError::InvalidInput)?
                {
                    return Err(EnforcerError::Conflict.into());
                }
            } else {
                *previous = Some(reservation);
            }
        }
        if retained.generation.current() != expected {
            self.legacy_cancel_prospective(&retained.target, retained.generation.current())
                .await?;
            return Err(EnforcerError::NotReady.into());
        }
        Ok(())
    }
    fn retained_prospective(
        &self,
        target: &ProspectiveRegistrationV1,
        generation: &CancelGeneration,
    ) -> EnforcerResult<Arc<LocalProspective>> {
        let retained = self
            .legacy_transport()?
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .get(target.arm_id())
            .cloned()
            .ok_or(EnforcerError::NotReady)?;
        if retained.digest != target.digest()?
            || !Arc::ptr_eq(&retained.generation.0, &generation.0)
        {
            return Err(EnforcerError::Conflict);
        }
        Ok(retained)
    }
    pub(crate) async fn legacy_begin_prospective_arm_exchange(
        &self,
        target: &ProspectiveRegistrationV1,
        generation: &CancelGeneration,
    ) -> EnforcerResult<ProspectiveArmExchange> {
        if self.0.failed.load(Ordering::SeqCst) {
            return Err(EnforcerError::Unhealthy);
        }
        let expected = generation.current();
        if expected == u64::MAX {
            return Err(EnforcerError::NotReady);
        }
        let retained = self.retained_prospective(target, generation)?;
        let reservation = retained
            .reservation
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .clone()
            .ok_or(EnforcerError::NotReady)?;
        retained
            .original_exchange_nonce
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .take();
        let reply = self
            .0
            .request(
                Request::BeginProspectiveArmExchange {
                    reservation,
                    generation: expected,
                },
                REGISTER_TIMEOUT,
            )
            .await?;
        let Response::ProspectiveArmExchange {
            original_exchange_nonce,
        } = reply
        else {
            return Err(response_error(reply));
        };
        if original_exchange_nonce.is_some_and(|nonce| nonce.is_nil()) {
            return Err(EnforcerError::Unauthorized);
        }
        if generation.current() != expected {
            self.legacy_cancel_prospective(target, generation.current())
                .await?;
            return Err(EnforcerError::NotReady);
        }
        Ok(ProspectiveArmExchange {
            epoch: self.0.endpoint.epoch(),
            arm_id: target.arm_id().into(),
            prospective_hash: retained.digest.clone(),
            generation: expected,
            original_exchange_nonce,
        })
    }
    pub(crate) async fn legacy_bind_prospective(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
        exchange: ProspectiveArmExchange,
    ) -> EnforcerResult<EnforcedSlot> {
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
        let retained = self
            .legacy_transport()?
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .get(target.arm_id())
            .cloned()
            .ok_or(EnforcerError::NotReady)?;
        if retained.digest != target.digest()? {
            return Err(EnforcerError::Conflict);
        }
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
impl Client {
    pub(super) fn legacy_observe_only(
        &self,
        registration: &RegistrationV1,
        requested: bool,
    ) -> EnforcerResult<bool> {
        let Some(legacy) = &self.legacy else {
            return Ok(requested);
        };
        if requested {
            return Ok(true);
        }
        let map = legacy
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        match map.get(registration.arm_id()) {
            Some(retained) if !retained.bound.load(Ordering::SeqCst) => {
                retained.target.matches_registration(registration)?;
                Ok(false)
            }
            _ => Ok(true),
        }
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
        let retained = legacy
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .get(registration.arm_id())
            .cloned();
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
            && let Some(retained) = legacy
                .prospective
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .get(arm_id)
        {
            retained.bound.store(true, Ordering::SeqCst);
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
