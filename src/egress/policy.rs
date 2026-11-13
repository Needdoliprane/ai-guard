use super::types::*;
use sha2::{Digest, Sha256};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// An immutable policy snapshot, not a tenant identity or durable grant. A
/// trusted caller must load tenant and session policies from its own scope.
#[derive(Clone, Debug)]
pub struct EgressPolicyV1 {
    revision: u64,
    not_before_ms: u64,
    expires_at_ms: u64,
    destinations: Vec<Authority>,
    limits: PolicyLimits,
    digest: [u8; 32],
}

impl EgressPolicyV1 {
    pub fn new(
        revision: u64,
        not_before_ms: u64,
        expires_at_ms: u64,
        destinations: &[Authority],
        limits: PolicyLimits,
    ) -> Result<Self, PolicyError> {
        if revision == 0 {
            return Err(PolicyError::InvalidRevision);
        }
        if not_before_ms >= expires_at_ms {
            return Err(PolicyError::InvalidValidity);
        }
        if destinations.len() > limits.max_destinations() {
            return Err(PolicyError::TooManyDestinations);
        }
        let count =
            u16::try_from(destinations.len()).map_err(|_| PolicyError::TooManyDestinations)?;
        // Bound before cloning or allocating. No wildcard/subdomain expansion.
        let mut destinations = destinations.to_vec();
        destinations.sort_unstable();
        if destinations.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(PolicyError::DuplicateDestination);
        }
        let mut hasher = Sha256::new();
        hasher.update(b"ai-guard:egress-policy:v1\0");
        hasher.update(revision.to_be_bytes());
        hasher.update(not_before_ms.to_be_bytes());
        hasher.update(expires_at_ms.to_be_bytes());
        hasher.update(limits.max_destinations.to_be_bytes());
        hasher.update([limits.max_resolution_ips]);
        hasher.update(count.to_be_bytes());
        for authority in &destinations {
            // A NUL delimiter is unambiguous: validated names are ASCII LDH.
            hasher.update(authority.name().as_bytes());
            hasher.update([0]);
            hasher.update(authority.port().to_be_bytes());
            hasher.update([match authority.transport() {
                EgressTransport::ConnectTcpOpaque => 1,
            }]);
        }
        Ok(Self {
            revision,
            not_before_ms,
            expires_at_ms,
            destinations,
            limits,
            digest: hasher.finalize().into(),
        })
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn not_before_ms(&self) -> u64 {
        self.not_before_ms
    }

    pub fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }

    pub fn destinations(&self) -> &[Authority] {
        &self.destinations
    }

    pub fn limits(&self) -> PolicyLimits {
        self.limits
    }

    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }

    /// Pure tenant∩session evaluation. Unknown destinations are denied before
    /// any DNS exists. `now_ms` is caller supplied and is not a trusted-clock
    /// witness. No caller may use this result as permission to connect.
    pub fn evaluate(&self, session: &Self, authority: &Authority, now_ms: u64) -> PolicyDecision {
        if !self.active_at(now_ms) {
            return PolicyDecision::Denied(DenyReason::TenantInactive);
        }
        if !session.active_at(now_ms) {
            return PolicyDecision::Denied(DenyReason::SessionInactive);
        }
        if self.destinations.binary_search(authority).is_err() {
            return PolicyDecision::Denied(DenyReason::DestinationNotAllowedByTenant);
        }
        if session.destinations.binary_search(authority).is_err() {
            return PolicyDecision::Denied(DenyReason::DestinationNotAllowedBySession);
        }
        PolicyDecision::PolicyAllowed(PolicyAllowed {
            authority: authority.clone(),
            tenant_revision: self.revision,
            session_revision: session.revision,
            tenant_digest: self.digest,
            session_digest: session.digest,
            valid_until_ms: self.expires_at_ms.min(session.expires_at_ms),
            limits: self.limits.intersection(session.limits),
        })
    }

    fn active_at(&self, now_ms: u64) -> bool {
        self.not_before_ms <= now_ms && now_ms < self.expires_at_ms
    }
}

/// Validation of supplied numeric candidates only. This value attests neither
/// DNS provenance/TTL nor audit, routing, remote ownership or a connected peer.
#[derive(Debug)]
pub struct ValidatedResolution {
    authority: Authority,
    addresses: Vec<IpAddr>,
}

impl ValidatedResolution {
    pub fn new(
        authority: &Authority,
        addresses: &[IpAddr],
        limits: PolicyLimits,
    ) -> Result<Self, ResolutionError> {
        if addresses.is_empty() {
            return Err(ResolutionError::Empty);
        }
        if addresses.len() > limits.max_resolution_ips() {
            return Err(ResolutionError::TooManyAddresses);
        }
        // Validate every candidate before selection, allocation or deduplication.
        if addresses.iter().any(|address| !public_candidate(*address)) {
            return Err(ResolutionError::NonPublicAddress);
        }
        let mut addresses = addresses.to_vec();
        addresses.sort_unstable();
        addresses.dedup();
        Ok(Self {
            authority: authority.clone(),
            addresses,
        })
    }

    pub fn authority(&self) -> &Authority {
        &self.authority
    }

    pub fn addresses(&self) -> &[IpAddr] {
        &self.addresses
    }
}

/// Conservative source-reviewed classification, not a universal is_global().
/// IANA IPv4/IPv6 Special-Purpose Address Registries, last updated 2025-10-09:
/// https://www.iana.org/assignments/iana-ipv4-special-registry
/// https://www.iana.org/assignments/iana-ipv6-special-registry
/// Reviewed by Root on 2026-11-12 UTC. Globally reachable special exceptions
/// are still denied. A future runtime must ALSO deny its own host/control/
/// metadata/tenant addresses, including public ones, before issuing a permit.
fn public_candidate(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => public_v4_candidate(address),
        IpAddr::V6(address) => public_v6_candidate(address),
    }
}

fn public_v4_candidate(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    // 0/8, RFC1918, shared space, loopback, link-local, multicast, reserved.
    if a == 0
        || a == 10
        || (a == 100 && (64..=127).contains(&b))
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || a >= 224
    {
        return false;
    }
    // IETF assignments, documentation, AS112, AMT, deprecated 6to4 and LAN.
    if a == 192
        && ((b == 0 && (c == 0 || c == 2))
            || (b == 31 && c == 196)
            || (b == 52 && c == 193)
            || (b == 88 && c == 99)
            || b == 168
            || (b == 175 && c == 48))
    {
        return false;
    }
    // Benchmarking and remaining IPv4 documentation prefixes.
    !(a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)) || (a == 203 && b == 0 && c == 113))
}

fn public_v6_candidate(address: Ipv6Addr) -> bool {
    let [a, b, c, _, _, _, _, _] = address.segments();
    // Admit only the current global-unicast envelope 2000::/3. This rejects
    // mapped IPv4, NAT64/translation, ULA, loopback, link-local and multicast.
    if a & 0xe000 != 0x2000 {
        return false;
    }
    // Conservatively reject all IETF 2001::/23, not just its Teredo/ORCHID/
    // benchmark assignments, plus documentation, 6to4 and AS112 special space.
    !((a == 0x2001 && (b < 0x0200 || b == 0x0db8))
        || a == 0x2002
        || (a == 0x2620 && b == 0x004f && c == 0x8000)
        || (a == 0x3fff && b < 0x1000))
}
