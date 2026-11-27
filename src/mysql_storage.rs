//! MySQL transport for the storage port. The probe checks the authenticated
//! database and TLS session before handing any authority to the future Store.
use mysql_async::{
    ClientIdentity, Conn, Opts, OptsBuilder, Row, SslOpts, from_row_opt, prelude::Queryable,
};
use serde::Serialize;
use std::{fmt, time::Duration};
use tokio_rustls::rustls::pki_types::ServerName;

pub mod audit;
mod broker_registry;
mod decision;
mod event_read;
mod identity;
mod owned_origin;
mod supervision;
mod worker;
mod workload;

pub use owned_origin::{
    MysqlOwnedPreparationPreflight, MysqlOwnedPreparationReceipt, MysqlOwnedPreparationRefusal,
    MysqlOwnedPreparationRequest, preflight_owned_preparation, publish_owned_fresh_source,
    verify_owned_fresh_source,
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
pub const MAX_CERTIFICATE_BYTES: usize = 65_536;
pub const MAX_PRIVATE_KEY_BYTES: usize = 16_384;
pub const MAX_PASSWORD_BYTES: usize = 1_024;

pub struct MysqlEndpoint<'a> {
    pub host: &'a str,
    pub port: u16,
    pub database: &'a str,
    pub username: &'a str,
}

pub struct MysqlClientIdentity<'a> {
    pub certificate_chain_pem: &'a [u8],
    pub private_key_pem: &'a [u8],
}

pub struct MysqlCredentials<'a> {
    pub password: &'a str,
    pub ca_pem: &'a [u8],
    pub client_identity: Option<MysqlClientIdentity<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MysqlAuthentication {
    Password,
    PasswordAndMtls,
}

#[derive(Serialize)]
pub struct MysqlProbeReceipt {
    pub configured_authentication: MysqlAuthentication,
    pub tls_version: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MysqlProbeError {
    InvalidConfiguration,
    TimedOut,
    ServerRefused(u16),
    TransportUnavailable,
    SessionMismatch,
}

impl fmt::Display for MysqlProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => f.write_str("invalid MySQL transport configuration"),
            Self::TimedOut => f.write_str("MySQL connection or probe timed out"),
            Self::ServerRefused(code) => write!(f, "MySQL refused the probe (server code {code})"),
            Self::TransportUnavailable => {
                f.write_str("MySQL TLS transport or protocol unavailable")
            }
            Self::SessionMismatch => f.write_str(
                "MySQL session does not match the configured database, account or TLS policy",
            ),
        }
    }
}

impl std::error::Error for MysqlProbeError {}

/// A confirmed transport or timeout failure, as opposed to a Store refusal.
pub fn is_transport_failure(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<MysqlProbeError>(),
        Some(MysqlProbeError::TransportUnavailable | MysqlProbeError::TimedOut)
    )
}

fn driver_error(error: mysql_async::Error) -> MysqlProbeError {
    match error {
        mysql_async::Error::Server(error) => MysqlProbeError::ServerRefused(error.code),
        _ => MysqlProbeError::TransportUnavailable,
    }
}

fn valid_pem_size(bytes: &[u8], maximum: usize) -> bool {
    !bytes.is_empty() && bytes.len() <= maximum && bytes.is_ascii()
}

fn connect_options(
    endpoint: &MysqlEndpoint<'_>,
    credentials: MysqlCredentials<'_>,
) -> Result<(OptsBuilder, MysqlAuthentication), MysqlProbeError> {
    if endpoint.port == 0
        || ServerName::try_from(endpoint.host).is_err()
        || endpoint.username.is_empty()
        || endpoint.database.is_empty()
        || endpoint.username.chars().count() > 32
        || endpoint.database.chars().count() > 64
        || endpoint.username.contains('\0')
        || endpoint.database.contains('\0')
        || credentials.password.is_empty()
        || credentials.password.len() > MAX_PASSWORD_BYTES
        || credentials.password.contains('\0')
        || !valid_pem_size(credentials.ca_pem, MAX_CERTIFICATE_BYTES)
    {
        return Err(MysqlProbeError::InvalidConfiguration);
    }

    let tls = SslOpts::default()
        .with_disable_built_in_roots(true)
        .with_root_certs(vec![credentials.ca_pem.to_vec().into()]);
    let (tls, authentication) = match credentials.client_identity {
        Some(identity) => {
            if !valid_pem_size(identity.certificate_chain_pem, MAX_CERTIFICATE_BYTES)
                || !valid_pem_size(identity.private_key_pem, MAX_PRIVATE_KEY_BYTES)
            {
                return Err(MysqlProbeError::InvalidConfiguration);
            }
            let identity = ClientIdentity::new(
                identity.certificate_chain_pem.to_vec().into(),
                identity.private_key_pem.to_vec().into(),
            );
            (
                tls.with_client_identity(Some(identity)),
                MysqlAuthentication::PasswordAndMtls,
            )
        }
        None => (tls, MysqlAuthentication::Password),
    };
    let options = OptsBuilder::default()
        .ip_or_hostname(endpoint.host)
        .tcp_port(endpoint.port)
        .db_name(Some(endpoint.database))
        .user(Some(endpoint.username))
        .pass(Some(credentials.password))
        .prefer_socket(false)
        .socket(None::<String>)
        .ssl_opts(Some(tls));
    Ok((options, authentication))
}

/// One TLS-authenticated session and fixed read-only queries. Errors drop the
/// driver payload, which can contain server-controlled strings or identifiers.
pub async fn probe(
    endpoint: MysqlEndpoint<'_>,
    credentials: MysqlCredentials<'_>,
) -> Result<MysqlProbeReceipt, MysqlProbeError> {
    let (options, authentication) = connect_options(&endpoint, credentials)?;
    tokio::time::timeout(PROBE_TIMEOUT, async {
        let (connection, tls_version) =
            open_verified(options.into(), endpoint.database, endpoint.username).await?;
        connection.disconnect().await.map_err(driver_error)?;
        Ok(MysqlProbeReceipt {
            configured_authentication: authentication,
            tls_version,
        })
    })
    .await
    .map_err(|_| MysqlProbeError::TimedOut)?
}

async fn open_verified(
    options: Opts,
    database: &str,
    username: &str,
) -> Result<(Conn, String), MysqlProbeError> {
    let mut connection = Conn::new(options).await.map_err(driver_error)?;
    let Some(session) = connection
        .query_first::<Row, _>("SELECT DATABASE(), CURRENT_USER()")
        .await
        .map_err(driver_error)?
    else {
        return Err(MysqlProbeError::SessionMismatch);
    };
    let (selected_database, account): (Option<String>, String) =
        from_row_opt(session).map_err(|_| MysqlProbeError::SessionMismatch)?;
    if selected_database.as_deref() != Some(database)
        || account.rsplit_once('@').map(|(user, _)| user) != Some(username)
    {
        return Err(MysqlProbeError::SessionMismatch);
    }
    let Some(version) = connection
        .query_first::<Row, _>("SHOW SESSION STATUS LIKE 'Ssl_version'")
        .await
        .map_err(driver_error)?
    else {
        return Err(MysqlProbeError::SessionMismatch);
    };
    let (name, tls_version): (String, String) =
        from_row_opt(version).map_err(|_| MysqlProbeError::SessionMismatch)?;
    if name != "Ssl_version" || !matches!(tls_version.as_str(), "TLSv1.2" | "TLSv1.3") {
        return Err(MysqlProbeError::SessionMismatch);
    }
    Ok((connection, tls_version))
}
