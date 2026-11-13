use std::fmt;

pub(super) const MAX_AUTHORITY_BYTES: usize = 512;
const MAX_DNS_NAME_BYTES: usize = 253;
const MAX_DNS_LABEL_BYTES: usize = 63;
const MAX_POLICY_DESTINATIONS: usize = 128;
const MAX_RESOLUTION_IPS: usize = 16;

/// Only an opaque TCP CONNECT transport is described. A port or name does not
/// establish HTTPS, certificate validation or the absence of a nested tunnel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EgressTransport {
    ConnectTcpOpaque,
}

/// A validated exact DNS authority. No IP literal, URL or alias heuristics.
/// Deliberately lacks Deserialize, Display and access to mutable fields.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Authority {
    name: String,
    port: u16,
    transport: EgressTransport,
}

impl Authority {
    /// Parse an untrusted CONNECT authority with an explicit canonical decimal
    /// port. The raw input is never retained in an error or diagnostic.
    pub fn parse_connect(raw: &str) -> Result<Self, AuthorityError> {
        if raw.len() > MAX_AUTHORITY_BYTES {
            return Err(AuthorityError::TooLong);
        }
        let (name, port) = raw
            .split_once(':')
            .ok_or(AuthorityError::InvalidAuthority)?;
        if port.is_empty()
            || port.len() > 5
            || port.starts_with('0')
            || !port.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(AuthorityError::InvalidPort);
        }
        let port = port
            .parse::<u16>()
            .map_err(|_| AuthorityError::InvalidPort)?;
        Self::new(name, port, EgressTransport::ConnectTcpOpaque)
    }

    pub fn new(name: &str, port: u16, transport: EgressTransport) -> Result<Self, AuthorityError> {
        if name.len() > MAX_DNS_NAME_BYTES {
            return Err(AuthorityError::TooLong);
        }
        if port == 0 {
            return Err(AuthorityError::InvalidPort);
        }
        if !name.is_ascii() || name.is_empty() {
            return Err(AuthorityError::InvalidHost);
        }
        let mut label_count = 0;
        let mut all_numeric_pieces = true;
        let mut final_label_numeric = false;
        for label in name.split('.') {
            if label.is_empty()
                || label.len() > MAX_DNS_LABEL_BYTES
                || !label.as_bytes()[0].is_ascii_alphanumeric()
                || !label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return Err(AuthorityError::InvalidHost);
            }
            label_count += 1;
            final_label_numeric = label.bytes().all(|byte| byte.is_ascii_digit());
            // Legacy inet_aton forms include hexadecimal and mixed-radix dotted
            // components. Do not pass these to a future DNS resolver as a name.
            let hex_piece = label
                .strip_prefix("0x")
                .or_else(|| label.strip_prefix("0X"))
                .is_some_and(|rest| {
                    !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_hexdigit())
                });
            all_numeric_pieces &= final_label_numeric || hex_piece;
        }
        if label_count < 2 || final_label_numeric || all_numeric_pieces {
            return Err(AuthorityError::InvalidHost);
        }
        Ok(Self {
            name: name.to_ascii_lowercase(),
            port,
            transport,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn transport(&self) -> EgressTransport {
        self.transport
    }
}

impl fmt::Debug for Authority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Authority([private exact name], ConnectTcpOpaque)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorityError {
    TooLong,
    InvalidAuthority,
    InvalidHost,
    InvalidPort,
}

impl fmt::Display for AuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TooLong => "egress authority exceeds the permitted length",
            Self::InvalidAuthority => "egress requires an exact name and explicit port",
            Self::InvalidHost => "egress requires an unambiguous ASCII DNS name",
            Self::InvalidPort => "egress requires a nonzero canonical decimal port",
        })
    }
}

impl std::error::Error for AuthorityError {}

/// Fixed ceilings protect the software contract. A session can only reduce
/// these bounds; they do not reserve DNS, flow, memory or audit capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PolicyLimits {
    pub(super) max_destinations: u16,
    pub(super) max_resolution_ips: u8,
}

impl PolicyLimits {
    pub fn new(max_destinations: usize, max_resolution_ips: usize) -> Result<Self, PolicyError> {
        if !(1..=MAX_POLICY_DESTINATIONS).contains(&max_destinations)
            || !(1..=MAX_RESOLUTION_IPS).contains(&max_resolution_ips)
        {
            return Err(PolicyError::InvalidLimits);
        }
        Ok(Self {
            max_destinations: u16::try_from(max_destinations)
                .map_err(|_| PolicyError::InvalidLimits)?,
            max_resolution_ips: u8::try_from(max_resolution_ips)
                .map_err(|_| PolicyError::InvalidLimits)?,
        })
    }

    pub fn max_destinations(self) -> usize {
        usize::from(self.max_destinations)
    }

    pub fn max_resolution_ips(self) -> usize {
        usize::from(self.max_resolution_ips)
    }

    pub(super) fn intersection(self, other: Self) -> Self {
        Self {
            max_destinations: self.max_destinations.min(other.max_destinations),
            max_resolution_ips: self.max_resolution_ips.min(other.max_resolution_ips),
        }
    }
}

impl Default for PolicyLimits {
    fn default() -> Self {
        Self {
            max_destinations: 128,
            max_resolution_ips: 16,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyError {
    InvalidLimits,
    InvalidRevision,
    InvalidValidity,
    TooManyDestinations,
    DuplicateDestination,
}

impl fmt::Display for PolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidLimits => "egress policy limits are outside the permitted envelope",
            Self::InvalidRevision => "egress policy requires a nonzero revision",
            Self::InvalidValidity => "egress policy requires a nonempty validity interval",
            Self::TooManyDestinations => "egress policy exceeds its destination limit",
            Self::DuplicateDestination => "egress policy contains a duplicate destination",
        })
    }
}

impl std::error::Error for PolicyError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DenyReason {
    TenantInactive,
    SessionInactive,
    DestinationNotAllowedByTenant,
    DestinationNotAllowedBySession,
}

#[derive(Debug)]
pub enum PolicyDecision {
    PolicyAllowed(PolicyAllowed),
    Denied(DenyReason),
}

/// Descriptive result of pure policy evaluation, NOT a connection capability.
/// It lacks a durable receipt, authenticated binding, trusted clock, grant and
/// single-use consumption. Callers cannot construct or deserialize it.
#[derive(Debug)]
pub struct PolicyAllowed {
    pub(super) authority: Authority,
    pub(super) tenant_revision: u64,
    pub(super) session_revision: u64,
    pub(super) tenant_digest: [u8; 32],
    pub(super) session_digest: [u8; 32],
    pub(super) valid_until_ms: u64,
    pub(super) limits: PolicyLimits,
}

impl PolicyAllowed {
    pub fn authority(&self) -> &Authority {
        &self.authority
    }

    pub fn tenant_revision(&self) -> u64 {
        self.tenant_revision
    }

    pub fn session_revision(&self) -> u64 {
        self.session_revision
    }

    pub fn tenant_digest(&self) -> [u8; 32] {
        self.tenant_digest
    }

    pub fn session_digest(&self) -> [u8; 32] {
        self.session_digest
    }

    pub fn valid_until_ms(&self) -> u64 {
        self.valid_until_ms
    }

    pub fn limits(&self) -> PolicyLimits {
        self.limits
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolutionError {
    Empty,
    TooManyAddresses,
    NonPublicAddress,
}

impl fmt::Display for ResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "egress candidate address set is empty",
            Self::TooManyAddresses => "egress candidate address set exceeds its limit",
            Self::NonPublicAddress => "egress candidate address set contains a forbidden address",
        })
    }
}

impl std::error::Error for ResolutionError {}
