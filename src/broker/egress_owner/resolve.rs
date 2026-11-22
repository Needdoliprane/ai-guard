//! Trusted host-resolver adapter for broker-owned egress (decision D3).
//!
//! A name is resolved only after its durable allow decision. Every candidate is
//! validated before any connect: one special, local or excluded address
//! refuses the whole answer instead of silently picking a public one. TTL and
//! CNAME chains are not observable through the host resolver and stay unknown.

use crate::egress::{Authority, PolicyLimits, ValidatedResolution};
use std::{
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::Arc,
};
use tokio::{
    sync::Semaphore,
    time::{Instant, timeout_at},
};

/// Upper bound on accepted candidates per answer.
pub(crate) const MAX_ADDRESSES: usize = 16;
/// Concurrent host lookups per broker; getaddrinfo occupies a blocking thread.
pub(crate) const MAX_CONCURRENT_LOOKUPS: usize = 8;

/// Where the validated addresses came from. Recorded with each connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResolutionSource {
    HostResolver,
    InternalService,
}
impl ResolutionSource {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::HostResolver => "host_resolver",
            Self::InternalService => "internal_service",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResolveError {
    /// The resolver did not answer before the connection deadline.
    Timeout,
    /// The resolver failed or returned no address.
    Unavailable,
    /// The answer contained a special, local or excluded address.
    RefusedAddress,
    /// More candidates than the bound, or the lookup slots are exhausted.
    Capacity,
}
impl ResolveError {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Timeout => "resolution_timeout",
            Self::Unavailable => "resolution_unavailable",
            Self::RefusedAddress => "refused_address",
            Self::Capacity => "resolution_capacity",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Resolution {
    pub(crate) addresses: Vec<IpAddr>,
    pub(crate) source: ResolutionSource,
}

/// An exact operator-declared internal service: the only way a loopback or
/// private address can be reached, and never a range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InternalService {
    pub(crate) authority: Authority,
    pub(crate) addresses: Vec<IpAddr>,
}

pub(crate) type LookupFuture = Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send>>;

/// Name lookup port. Production uses the trusted host's resolver.
pub(crate) trait HostLookup: Send + Sync {
    fn lookup(&self, name: String, port: u16) -> LookupFuture;
}

/// getaddrinfo through Tokio's blocking pool; no cache of ours.
pub(crate) struct SystemLookup;
impl HostLookup for SystemLookup {
    fn lookup(&self, name: String, port: u16) -> LookupFuture {
        Box::pin(async move {
            Ok(tokio::net::lookup_host((name.as_str(), port))
                .await?
                .collect())
        })
    }
}

pub(crate) struct Resolver {
    lookup: Arc<dyn HostLookup>,
    internal: Vec<InternalService>,
    exclusions: Vec<IpAddr>,
    slots: Arc<Semaphore>,
}

impl Resolver {
    /// `exclusions` are exact addresses of this installation's control
    /// services (gateway, dashboard, ingress, broker IPC peers). An internal
    /// service may never alias one of them.
    pub(crate) fn new(
        lookup: Arc<dyn HostLookup>,
        internal: Vec<InternalService>,
        exclusions: Vec<IpAddr>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(internal.len() <= 64, "too many internal egress services");
        for (index, service) in internal.iter().enumerate() {
            anyhow::ensure!(
                !service.addresses.is_empty() && service.addresses.len() <= MAX_ADDRESSES,
                "internal egress service needs 1..=16 exact addresses"
            );
            anyhow::ensure!(
                !service
                    .addresses
                    .iter()
                    .any(|address| exclusions.contains(address)),
                "internal egress service aliases a control address"
            );
            anyhow::ensure!(
                !internal[..index]
                    .iter()
                    .any(|other| other.authority == service.authority),
                "duplicate internal egress service"
            );
        }
        Ok(Self {
            lookup,
            internal,
            exclusions,
            slots: Arc::new(Semaphore::new(MAX_CONCURRENT_LOOKUPS)),
        })
    }

    /// Resolve an already-allowed authority before `deadline`.
    pub(crate) async fn resolve(
        &self,
        authority: &Authority,
        deadline: Instant,
    ) -> Result<Resolution, ResolveError> {
        if let Some(service) = self
            .internal
            .iter()
            .find(|service| &service.authority == authority)
        {
            return Ok(Resolution {
                addresses: service.addresses.clone(),
                source: ResolutionSource::InternalService,
            });
        }
        let _slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| ResolveError::Capacity)?;
        let answer = timeout_at(
            deadline,
            self.lookup
                .lookup(authority.name().into(), authority.port()),
        )
        .await
        .map_err(|_| ResolveError::Timeout)?
        .map_err(|_| ResolveError::Unavailable)?;
        let mut addresses: Vec<IpAddr> = answer.iter().map(SocketAddr::ip).collect();
        addresses.sort_unstable();
        addresses.dedup();
        if addresses.is_empty() {
            return Err(ResolveError::Unavailable);
        }
        if addresses.len() > MAX_ADDRESSES {
            return Err(ResolveError::Capacity);
        }
        if addresses
            .iter()
            .any(|address| self.exclusions.contains(address))
        {
            return Err(ResolveError::RefusedAddress);
        }
        let limits = PolicyLimits::new(1, MAX_ADDRESSES).map_err(|_| ResolveError::Capacity)?;
        let validated = ValidatedResolution::new(authority, &addresses, limits)
            .map_err(|_| ResolveError::RefusedAddress)?;
        Ok(Resolution {
            addresses: validated.addresses().to_vec(),
            source: ResolutionSource::HostResolver,
        })
    }
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
