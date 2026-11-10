//! Configuration and lifetime controls for D8's bounded registry. Bindings
//! are unit-test data; actual local identity/Router producers remain separate.
use super::*;

fn binding(tenant: &str, owner: u32, membership: u32) -> ContextBinding {
    ContextBinding {
        digest: Sha256::digest(owner.to_be_bytes()).into(),
        principal_key: format!("{owner:064x}"),
        credential_id: uuid::Uuid::from_u128(u128::from(owner) + 1).to_string(),
        credential_revision: 1,
        authentication_method: "local_credential".into(),
        mapping_revision: None,
        tenant_id: tenant.into(),
        organization_scope: "fixture-organization".into(),
        membership_id: uuid::Uuid::from_u128(u128::from(membership) + 1).to_string(),
        mode: ContextModeV1::Personal,
        membership_revision: 1,
        authorization_revision: 1,
    }
}

fn one_place_per_tenant() -> ContextRegistry {
    let ids = (0..MAX_CONTEXTS)
        .map(|index| format!("tenant-{index:04}"))
        .collect::<Vec<_>>();
    ContextRegistry::for_tenants(ids.iter().map(String::as_str)).unwrap()
}

#[test]
fn configured_shares_are_deterministic_and_configuration_has_explicit_bounds() {
    assert!(ContextRegistry::for_tenants(std::iter::empty::<&str>()).is_err());
    assert!(ContextRegistry::for_tenants(["tenant-a", "tenant-a"]).is_err());
    assert!(ContextRegistry::for_tenants(["tenant-a", "invalid/tenant"]).is_err());
    let single = ContextRegistry::for_tenants(["tenant-a"]).unwrap();
    assert_eq!(single.tenant_limits["tenant-a"], MAX_CONTEXTS);
    let three = ContextRegistry::for_tenants(["tenant-c", "tenant-a", "tenant-b"]).unwrap();
    let reordered = ContextRegistry::for_tenants(["tenant-b", "tenant-c", "tenant-a"]).unwrap();
    assert_eq!(three.tenant_limits, reordered.tenant_limits);
    assert_eq!(three.tenant_limits["tenant-a"], 1366);
    assert_eq!(three.tenant_limits["tenant-b"], 1365);
    assert_eq!(three.tenant_limits["tenant-c"], 1365);
    assert_eq!(three.tenant_limits.values().sum::<usize>(), MAX_CONTEXTS);
    let maximum = one_place_per_tenant();
    assert_eq!(maximum.tenant_limits.len(), MAX_CONTEXTS);
    assert!(maximum.tenant_limits.values().all(|share| *share == 1));
    let too_many = (0..=MAX_CONTEXTS)
        .map(|index| format!("tenant-{index:04}"))
        .collect::<Vec<_>>();
    assert!(ContextRegistry::for_tenants(too_many.iter().map(String::as_str)).is_err());
}

#[test]
fn unknown_or_disabled_tenant_cannot_create_a_share_or_reconcile_a_live_context() {
    let registry = ContextRegistry::for_tenants(["tenant-a"]).unwrap();
    let entry = registry
        .obtain(binding("tenant-a", 1, 1), CONTEXT_TTL, "expiry".into())
        .unwrap();
    assert!(matches!(
        registry.obtain(binding("tenant-b", 2, 1), CONTEXT_TTL, "expiry".into()),
        Err(StateFailure::InvalidContext)
    ));
    assert!(matches!(
        registry.reconcile(
            &entry.binding.digest,
            &entry.binding.principal_key,
            2,
            "tenant-b",
            Some(2),
            &[]
        ),
        Err(StateFailure::InvalidContext)
    ));
    assert!(entry.ensure_valid().is_ok());
    assert_eq!(registry.entries.lock().unwrap().len(), 1);
    assert_eq!(registry.tenant_limits.len(), 1);
    let disabled = ContextRegistry::disabled();
    assert!(matches!(
        disabled.obtain(binding("tenant-a", 1, 1), CONTEXT_TTL, "expiry".into()),
        Err(StateFailure::InvalidContext)
    ));
    assert!(disabled.entries.lock().unwrap().is_empty());
    assert!(disabled.tenant_limits.is_empty());
}

#[test]
fn per_owner_bound_survives_tenant_changes_digest_rotation_and_principal_aliases() {
    let registry = ContextRegistry::for_tenants(["tenant-a", "tenant-b"]).unwrap();
    for membership in 0..MAX_OWNER_CONTEXTS as u32 {
        let tenant = if membership % 2 == 0 {
            "tenant-a"
        } else {
            "tenant-b"
        };
        registry
            .obtain(binding(tenant, 1, membership), CONTEXT_TTL, "expiry".into())
            .unwrap();
    }
    let original = registry
        .obtain(binding("tenant-a", 1, 0), CONTEXT_TTL, "expiry".into())
        .unwrap();
    let mut rotated = binding("tenant-b", 1, 999);
    rotated.digest = [99; 32];
    rotated.credential_id = uuid::Uuid::new_v4().to_string();
    assert!(matches!(
        registry.obtain(rotated, CONTEXT_TTL, "expiry".into()),
        Err(StateFailure::Capacity)
    ));
    let mut alias = binding("tenant-b", 2, 999);
    alias.digest = original.binding.digest;
    assert!(matches!(
        registry.obtain(alias, CONTEXT_TTL, "expiry".into()),
        Err(StateFailure::Capacity)
    ));
    assert!(
        registry
            .obtain(binding("tenant-b", 2, 999), CONTEXT_TTL, "expiry".into())
            .is_ok()
    );
    assert!(original.ensure_valid().is_ok());
}

#[test]
fn a_full_tenant_keeps_its_exact_context_and_cannot_borrow_an_idle_share() {
    let registry = one_place_per_tenant();
    let first = registry
        .obtain(
            binding("tenant-0000", 1, 1),
            CONTEXT_TTL,
            "original-expiry".into(),
        )
        .unwrap();
    let again = registry
        .obtain(
            binding("tenant-0000", 1, 1),
            CONTEXT_TTL,
            "later-expiry".into(),
        )
        .unwrap();
    assert!(Arc::ptr_eq(&first, &again));
    assert_eq!(first.created, again.created);
    assert_eq!(again.expires_at, "original-expiry");
    assert!(matches!(
        registry.obtain(binding("tenant-0000", 2, 2), CONTEXT_TTL, "expiry".into()),
        Err(StateFailure::Capacity)
    ));
    let peer = registry
        .obtain(binding("tenant-0001", 2, 1), CONTEXT_TTL, "expiry".into())
        .unwrap();
    assert!(peer.ensure_valid().is_ok() && first.ensure_valid().is_ok());
    assert_eq!(registry.entries.lock().unwrap().len(), 2);
}

#[test]
fn expiry_and_authoritative_revocation_release_only_the_original_tenant_place() {
    let registry = one_place_per_tenant();
    let original_binding = binding("tenant-0000", 1, 1);
    let mut expired = registry
        .obtain(original_binding.clone(), CONTEXT_TTL, "expiry".into())
        .unwrap();
    // Existing unit-test seam: change only the original elapsed origin, with
    // no sleep or new deadline. A retained Arc must still become unusable.
    registry.entries.lock().unwrap().remove(&expired.id);
    let original_created = expired.created;
    Arc::get_mut(&mut expired).unwrap().created =
        original_created.checked_sub(CONTEXT_TTL).unwrap();
    registry
        .entries
        .lock()
        .unwrap()
        .insert(expired.id.clone(), Arc::clone(&expired));
    let fresh = registry
        .obtain(original_binding.clone(), CONTEXT_TTL, "fresh-expiry".into())
        .unwrap();
    assert_ne!(fresh.id, expired.id);
    assert!(matches!(expired.ensure_valid(), Err(StateFailure::Expired)));
    registry
        .reconcile(
            &fresh.binding.digest,
            &fresh.binding.principal_key,
            1,
            "tenant-0000",
            Some(2),
            &[],
        )
        .unwrap();
    assert!(matches!(fresh.ensure_valid(), Err(StateFailure::Revoked)));
    let mut next = original_binding;
    next.authorization_revision = 2;
    let reacquired = registry
        .obtain(next, CONTEXT_TTL, "next-expiry".into())
        .unwrap();
    assert_ne!(reacquired.id, fresh.id);
    assert!(
        registry
            .obtain(
                binding("tenant-0001", 2, 1),
                CONTEXT_TTL,
                "peer-expiry".into()
            )
            .is_ok()
    );
    assert_eq!(registry.entries.lock().unwrap().len(), 2);
}

#[test]
fn concurrent_allocation_cannot_overbook_a_tenant_or_consume_its_peer_share() {
    let registry = Arc::new(one_place_per_tenant());
    let results = std::thread::scope(|scope| {
        let handles = (0..8)
            .map(|owner| {
                let registry = Arc::clone(&registry);
                scope.spawn(move || {
                    registry.obtain(
                        binding("tenant-0000", owner, 1),
                        CONTEXT_TTL,
                        "expiry".into(),
                    )
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(StateFailure::Capacity)))
            .count(),
        7
    );
    assert!(
        registry
            .obtain(binding("tenant-0001", 9, 1), CONTEXT_TTL, "expiry".into())
            .is_ok()
    );
    assert_eq!(registry.entries.lock().unwrap().len(), 2);
}
