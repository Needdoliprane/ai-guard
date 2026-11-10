//! Finite volatile accounting for already verified native operations.
//! IDs come from the verified durable plan, not a request or Store index.
use super::*;

pub(super) const VOLATILE_START_SLOTS: usize = 256;
pub(super) const ANONYMOUS_START_SLOTS: usize = 224;
pub(super) const NATIVE_START_SLOTS: usize = 32;
pub(super) const NATIVE_STARTS: u8 = 8;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) struct RegistryTenantKey(u8);

/// Internal accounting input, never an authorization proof. Only install's
/// positive membership observations may construct the session snapshot.
pub(super) struct VerifiedRegistryTenants {
    keys: Vec<RegistryTenantKey>,
}
impl VerifiedRegistryTenants {
    pub fn keys(&self) -> impl Iterator<Item = RegistryTenantKey> + '_ {
        self.keys.iter().copied()
    }
}

pub(super) struct RegistryTenantLimits {
    ids: Vec<String>,
    slots: usize,
    starts: u8,
}
impl RegistryTenantLimits {
    pub fn new(tenants: impl IntoIterator<Item = String>) -> anyhow::Result<Self> {
        let mut ids = Vec::new();
        for tenant in tenants {
            crate::tenancy::validate_tenant_id(&tenant)?;
            // A key fits exactly the existing V4 plan's maximum 256 tenants.
            anyhow::ensure!(
                u8::try_from(ids.len()).is_ok(),
                "too many OIDC registry tenants"
            );
            ids.push(tenant);
        }
        anyhow::ensure!(!ids.is_empty(), "OIDC registry tenants are required");
        ids.sort();
        anyhow::ensure!(
            ids.windows(2).all(|pair| pair[0] != pair[1]),
            "duplicate OIDC registry tenant"
        );
        let count = ids.len();
        // Beyond the 32/8 global budgets, minimum 1 is a tenant cap under the
        // shared global bound, not a simultaneous reservation for every ID.
        Ok(Self {
            ids,
            slots: (NATIVE_START_SLOTS / count).max(1),
            starts: u8::try_from((usize::from(NATIVE_STARTS) / count).max(1))?,
        })
    }

    pub fn verified_memberships(&self, tenants: &[String]) -> ApiResult<VerifiedRegistryTenants> {
        let mut keys = Vec::new();
        for tenant in tenants {
            let index = self
                .ids
                .binary_search(tenant)
                .map_err(|_| audit_unavailable())?;
            keys.push(RegistryTenantKey(
                u8::try_from(index).map_err(|_| audit_unavailable())?,
            ));
        }
        keys.sort_by_key(|key| key.0);
        keys.dedup();
        if keys.is_empty() {
            return Err(audit_unavailable());
        }
        Ok(VerifiedRegistryTenants { keys })
    }

    pub fn contains(&self, key: RegistryTenantKey) -> bool {
        usize::from(key.0) < self.ids.len()
    }
    pub fn slots(&self) -> usize {
        self.slots
    }
    pub fn starts(&self) -> u8 {
        self.starts
    }
    #[cfg(test)]
    pub fn tenant_id(&self, key: RegistryTenantKey) -> Option<&str> {
        self.ids.get(usize::from(key.0)).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_keys_are_explicitly_resolved_by_id_in_sorted_plan_order() {
        let limits = RegistryTenantLimits::new(["tenant-b".into(), "tenant-a".into()]).unwrap();
        let scope = limits.verified_memberships(&["tenant-b".into()]).unwrap();
        let key = scope.keys().next().unwrap();
        assert_eq!(limits.tenant_id(key), Some("tenant-b"));
        assert_eq!(
            key.0, 1,
            "gateway index0 for B must not become sorted plan index0 for A"
        );
        assert!(limits.verified_memberships(&["unknown".into()]).is_err());
        assert!(limits.verified_memberships(&[]).is_err());
        assert!(RegistryTenantLimits::new(["tenant-a".into(), "tenant-a".into()]).is_err());
    }

    #[test]
    fn finite_tenant_caps_preserve_global_budgets_and_document_oversubscription() {
        for count in [1, 2, 3, 8, 9, 32, 33, 256] {
            let limits =
                RegistryTenantLimits::new((0..count).map(|id| format!("tenant-{id:03}"))).unwrap();
            assert_eq!(limits.slots(), (32 / count).max(1));
            assert_eq!(usize::from(limits.starts()), (8 / count).max(1));
            if count <= 32 {
                assert!(limits.slots() * count <= NATIVE_START_SLOTS);
            }
            if count <= 8 {
                assert!(usize::from(limits.starts()) * count <= usize::from(NATIVE_STARTS));
            }
        }
        assert!(RegistryTenantLimits::new((0..257).map(|id| format!("tenant-{id}"))).is_err());
    }
}
