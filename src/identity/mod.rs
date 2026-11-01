//! Named human principals and explicit tenant/resource permissions.
//!
//! Authentication transports are separate from this domain. A parsed OIDC
//! principal is never proof of token validation, membership, or runner authority.
mod access;
mod authorization;
mod entropy;
mod types;

pub use access::*;
pub use authorization::AuthorizedScopeV1;
pub(crate) use authorization::authorize_context;
pub use entropy::generate_local_credential;
pub(crate) use entropy::local_credential_format;
pub(crate) use entropy::random_bytes;
pub use types::*;

#[cfg(test)]
mod tests;
