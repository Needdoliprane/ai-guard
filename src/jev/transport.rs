use super::{
    BackendFailure, DiagnosticHop, JevError, JevTrust, TransportDiagnostic, TransportKind,
    TransportPhase,
};
use serde_json::Value;
use std::io::Write;

pub(super) const MAX_BODY: usize = 65_536;

#[derive(Clone)]
pub(super) struct JsonTransport {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    key: Option<String>,
    local_http: bool,
    trust: JevTrust,
}
pub fn validate_transport_configuration(
    endpoint: &str,
    allow_local_http: bool,
    trust: &JevTrust,
) -> Result<(), JevError> {
    validate_endpoint(endpoint, allow_local_http, trust).map(|_| ())
}
fn validate_endpoint(
    endpoint: &str,
    local_http: bool,
    trust: &JevTrust,
) -> Result<reqwest::Url, JevError> {
    if endpoint.len() > 4096 || (trust.is_explicit() && local_http) {
        return Err(JevError::InvalidConfiguration);
    }
    let endpoint = reqwest::Url::parse(endpoint).map_err(|_| JevError::InvalidConfiguration)?;
    let loopback = endpoint.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || !(endpoint.scheme() == "https"
            || (local_http && endpoint.scheme() == "http" && loopback && !trust.is_explicit()))
    {
        return Err(JevError::InvalidConfiguration);
    }
    Ok(endpoint)
}
impl JsonTransport {
    pub(super) fn new(
        endpoint: &str,
        key: Option<String>,
        local_http: bool,
        trust: JevTrust,
    ) -> Result<Self, JevError> {
        let key = key.map(|value| value.trim().to_owned());
        let endpoint = validate_endpoint(endpoint, local_http, &trust)?;
        if key.as_ref().is_some_and(|key| {
            key.len() < 4 || key.len() > 4096 || key.chars().any(char::is_control)
        }) {
            return Err(JevError::InvalidConfiguration);
        }
        let client = trust
            .apply_to_client(
                reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(5))
                    .connect_timeout(std::time::Duration::from_secs(2))
                    .redirect(reqwest::redirect::Policy::none())
                    .retry(reqwest::retry::never())
                    .no_proxy(),
            )?
            .build()
            .map_err(|_| JevError::InvalidConfiguration)?;
        Ok(Self {
            client,
            endpoint,
            key,
            local_http,
            trust,
        })
    }
    pub(super) fn configuration(&self) -> Value {
        serde_json::json!({
            "endpoint": self.endpoint.as_str(), "credential_present": self.key.is_some(),
            "allow_loopback_http": self.local_http, "timeout_seconds": 5,
            "connect_timeout_seconds": 2, "request_bytes": MAX_BODY, "response_bytes": MAX_BODY,
            "trust":self.trust.configuration(), "automatic_retry":false
        })
    }
    pub(super) fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    pub(super) async fn post(&self, body: &Value) -> Result<Vec<u8>, BackendFailure> {
        let bytes = bounded_json(body)?;
        let mut request = self
            .client
            .post(self.endpoint.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(bytes);
        if let Some(key) = &self.key {
            request = request.bearer_auth(key);
        }
        let mut response = request
            .send()
            .await
            .map_err(|error| transport_failure(&error, TransportPhase::Send, None))?;
        let status = response.status();
        if !response.status().is_success() {
            return Err(BackendFailure {
                error: JevError::TransportUnavailable,
                diagnostic: Some(TransportDiagnostic {
                    hop: DiagnosticHop::ConfiguredEndpoint,
                    phase: TransportPhase::Send,
                    kind: TransportKind::HttpStatus,
                    http_status: Some(status.as_u16()),
                }),
            });
        }
        if response
            .content_length()
            .is_some_and(|bytes| bytes > MAX_BODY as u64)
        {
            return Err(JevError::ResponseTooLarge.into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            transport_failure(&error, TransportPhase::ResponseBody, Some(status.as_u16()))
        })? {
            if bytes
                .len()
                .checked_add(chunk.len())
                .is_none_or(|length| length > MAX_BODY)
            {
                return Err(JevError::ResponseTooLarge.into());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

fn transport_failure(
    error: &reqwest::Error,
    phase: TransportPhase,
    http_status: Option<u16>,
) -> BackendFailure {
    let kind = if error.is_timeout() {
        TransportKind::Timeout
    } else if error.is_connect() {
        TransportKind::ConnectionError
    } else {
        TransportKind::TransportError
    };
    BackendFailure {
        error: JevError::TransportUnavailable,
        diagnostic: Some(TransportDiagnostic {
            hop: DiagnosticHop::ConfiguredEndpoint,
            phase,
            kind,
            http_status,
        }),
    }
}

pub(super) fn bounded_json(value: &impl serde::Serialize) -> Result<Vec<u8>, JevError> {
    struct Bounded(Vec<u8>);
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self
                .0
                .len()
                .checked_add(bytes.len())
                .is_none_or(|length| length > MAX_BODY)
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "bounded JSON exceeded",
                ));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Bounded(Vec::new());
    serde_json::to_writer(&mut writer, value).map_err(|_| JevError::RequestTooLarge)?;
    Ok(writer.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serialized_byte_limit_includes_json_framing_and_escaping() {
        assert_eq!(
            bounded_json(&"x".repeat(MAX_BODY - 2)).unwrap().len(),
            MAX_BODY
        );
        assert_eq!(
            bounded_json(&"x".repeat(MAX_BODY - 1)),
            Err(JevError::RequestTooLarge)
        );
        assert_eq!(
            bounded_json(&"\n".repeat(MAX_BODY / 2)),
            Err(JevError::RequestTooLarge)
        );
    }
}
