//! Explicit genuine fresh Legacy spawn and restrictive existing reconnection.
use super::*;

impl EnforcerClient {
    /// An existing directory can never enter the genuine fresh child factory.
    /// EEXIST/failed launch does not fall back to adoption or rekeying.
    pub(crate) async fn connect_or_spawn_legacy_with_budget(
        parent: &Directory,
        name: &str,
        executable: &Path,
        slots: u32,
        recovery: u32,
    ) -> EnforcerResult<Self> {
        if !(1..=MAX_SLOTS).contains(&slots) || recovery > MAX_SLOTS || !executable.is_absolute() {
            return Err(EnforcerError::InvalidInput);
        }
        parent
            .revalidate()
            .map_err(|_| EnforcerError::UnsafeFilesystem)?;
        if let Some(root) = parent
            .persistent_child(name, false)
            .map_err(|_| EnforcerError::UnsafeFilesystem)?
        {
            return Self::connect_existing_legacy_mode(&root, executable, slots, recovery, true)
                .await;
        }
        let nonce = Uuid::new_v4();
        spawn_helper_config(
            parent,
            executable,
            slots,
            HelperMode::LegacyFresh {
                name,
                max_recovery_stop_owners: recovery,
                launch_nonce: nonce,
            },
        )?;
        let expires = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(root) = parent
                .persistent_child(name, false)
                .map_err(|_| EnforcerError::UnsafeFilesystem)?
            {
                if let Some(endpoint) = load_endpoint(&root)? {
                    let secret = capability(&root, false)?;
                    match Self::connect(&root, executable, endpoint, secret, slots, recovery, true)
                        .await
                    {
                        Ok(client) => {
                            let value = client.legacy_capabilities().await?;
                            if value.fresh_launch_nonce != Some(nonce)
                                || value.recovery_generation.is_some()
                                || value.recovery_incomplete
                                || value.known_stop_slots != 0
                                || value.owned_active != Some(0)
                            {
                                return Err(EnforcerError::Conflict);
                            }
                            *client
                                .legacy_transport()?
                                .fresh_startup
                                .lock()
                                .map_err(|_| EnforcerError::Unhealthy)? =
                                Some((value.epoch, nonce));
                            return Ok(client);
                        }
                        Err(ConnectFailure::Absent) => {}
                        Err(ConnectFailure::Other(error)) => return Err(error),
                    }
                }
            }
            if Instant::now() >= expires {
                return Err(EnforcerError::Unavailable);
            }
            tokio::time::sleep(POLL).await;
        }
    }

    pub(in crate::broker::enforcer::client) async fn connect_existing_legacy_mode(
        root: &Directory,
        executable: &Path,
        slots: u32,
        recovery: u32,
        auto_recovery: bool,
    ) -> EnforcerResult<Self> {
        if !(1..=MAX_SLOTS).contains(&slots) || recovery > MAX_SLOTS || !executable.is_absolute() {
            return Err(EnforcerError::InvalidInput);
        }
        root.revalidate()
            .map_err(|_| EnforcerError::UnsafeFilesystem)?;
        // Existing/cold never bootstraps missing auth, lock or namespaces.
        let secret = capability(root, false)?;
        if let Some(endpoint) = load_endpoint(root)? {
            match Self::connect(
                root,
                executable,
                endpoint,
                secret,
                slots,
                recovery,
                auto_recovery,
            )
            .await
            {
                Ok(client) => return Ok(client),
                Err(ConnectFailure::Absent) => {}
                Err(ConnectFailure::Other(error)) => return Err(error),
            }
        }
        let lease = RoleLease::acquire_existing(root)?;
        drop(lease);
        spawn_helper_config(
            root,
            executable,
            slots,
            HelperMode::LegacyExisting {
                max_recovery_stop_owners: recovery,
            },
        )?;
        let expires = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(endpoint) = load_endpoint(root)? {
                match Self::connect(
                    root,
                    executable,
                    endpoint,
                    secret,
                    slots,
                    recovery,
                    auto_recovery,
                )
                .await
                {
                    Ok(client) => return Ok(client),
                    Err(ConnectFailure::Absent) => {}
                    Err(ConnectFailure::Other(error)) => return Err(error),
                }
            }
            if Instant::now() >= expires {
                return Err(EnforcerError::Unavailable);
            }
            tokio::time::sleep(POLL).await;
        }
    }
}
