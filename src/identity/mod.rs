//! Named human principals and explicit tenant/resource permissions.
//!
//! Authentication transports are separate from this domain. A parsed OIDC
//! principal is never proof of token validation, membership, or runner authority.
mod authorization;
mod types;

pub use authorization::AuthorizedScopeV1;
pub(crate) use authorization::authorize_context;
pub use types::*;

#[cfg(test)]
mod tests;
