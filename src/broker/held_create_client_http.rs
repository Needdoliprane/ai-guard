use super::*;
use futures_util::StreamExt;
use reqwest::{StatusCode, header};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{collections::BTreeSet, net::IpAddr, time::Duration};

pub(super) fn prepare(
    scope: HeldCreateClientScope,
    token: PrivateFile,
    ca: Option<PrivateFile>,
    known_secrets: Vec<String>,
) -> ClientResult<HeldCreateControlClient> {
    crate::tenancy::validate_tenant_id(&scope.tenant_id)
        .map_err(|_| HeldCreateClientError::InvalidConfiguration)?;
    for value in [
        &scope.control_namespace_uuid,
        &scope.broker_id,
        &scope.credential_id,
        &scope.incarnation_id,
    ] {
        validation::id(value).map_err(|_| HeldCreateClientError::InvalidConfiguration)?;
    }
    if scope.gateway.len() > 2048 {
        return Err(HeldCreateClientError::InvalidConfiguration);
    }
    let origin = reqwest::Url::parse(&scope.gateway)
        .map_err(|_| HeldCreateClientError::InvalidConfiguration)?;
    let local = origin.host_str().is_some_and(|host| {
        host.trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    if origin.host_str().is_none()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
        || !(origin.scheme() == "https" || origin.scheme() == "http" && local)
        || ca.is_some() && origin.scheme() != "https"
    {
        return Err(HeldCreateClientError::InvalidConfiguration);
    }
    let bearer = std::str::from_utf8(token.bytes())
        .map_err(|_| HeldCreateClientError::InvalidConfiguration)?;
    let bearer = bearer.strip_suffix('\n').unwrap_or(bearer);
    if !(32..=4096).contains(&bearer.len())
        || !bearer.is_ascii()
        || bearer
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return Err(HeldCreateClientError::InvalidConfiguration);
    }
    if known_secrets
        .iter()
        .any(|secret| secret.is_empty() || secret.len() > 4096)
    {
        return Err(HeldCreateClientError::InvalidConfiguration);
    }
    let mut secrets: BTreeSet<String> = known_secrets.into_iter().collect();
    secrets.insert(bearer.into());
    let total_bytes = secrets
        .iter()
        .try_fold(0usize, |total, secret| total.checked_add(secret.len()));
    if secrets.len() > 64 || total_bytes.is_none_or(|total| total > 256 * 1024) {
        return Err(HeldCreateClientError::InvalidConfiguration);
    }
    let redactor = Redactor::new(secrets.into_iter().collect());
    for value in [
        &scope.gateway,
        &scope.tenant_id,
        &scope.control_namespace_uuid,
        &scope.broker_id,
        &scope.credential_id,
        &scope.incarnation_id,
    ] {
        if redactor.text(value) != *value {
            return Err(HeldCreateClientError::InvalidConfiguration);
        }
    }
    let mut authorization = header::HeaderValue::from_str(&format!("Bearer {bearer}"))
        .map_err(|_| HeldCreateClientError::InvalidConfiguration)?;
    authorization.set_sensitive(true);
    let mut builder = reqwest::Client::builder()
        .https_only(origin.scheme() == "https")
        .http1_only()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(3))
        .read_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .pool_max_idle_per_host(0);
    if let Some(ca) = &ca {
        if ca.bytes().len() > 65536 {
            return Err(HeldCreateClientError::InvalidConfiguration);
        }
        let certs = crate::tls::certificates(ca.bytes())
            .map_err(|_| HeldCreateClientError::InvalidConfiguration)?;
        if certs.is_empty() || certs.len() > 8 {
            return Err(HeldCreateClientError::InvalidConfiguration);
        }
        builder = builder.tls_built_in_root_certs(false);
        for cert in certs {
            builder = builder.add_root_certificate(
                reqwest::Certificate::from_der(cert.as_ref())
                    .map_err(|_| HeldCreateClientError::InvalidConfiguration)?,
            );
        }
    }
    // Constructor reads retained bytes only; no synchronous filesystem revalidation.
    Ok(HeldCreateControlClient {
        scope,
        token,
        ca,
        redactor,
        http: builder
            .build()
            .map_err(|_| HeldCreateClientError::InvalidConfiguration)?,
        origin,
        authorization,
        material_capacity: Arc::new(Semaphore::new(1)),
        revisions: Mutex::new((0, 0)),
        #[cfg(test)]
        material_gate: Mutex::new(None),
    })
}

impl HeldCreateControlClient {
    pub(super) fn protected_files(&self) -> Vec<PrivateFile> {
        let mut files = vec![self.token.clone()];
        if let Some(ca) = &self.ca {
            files.push(ca.clone());
        }
        files
    }
    async fn revalidate_material(&self) -> ClientResult<()> {
        // Separate logical admission from JournalIo: never acquire its held permit.
        let capacity = Arc::clone(&self.material_capacity)
            .try_acquire_owned()
            .map_err(|_| HeldCreateClientError::Capacity)?;
        let files = self.protected_files();
        #[cfg(test)]
        let mut gate = self
            .material_gate
            .lock()
            .map_err(|_| HeldCreateClientError::Unavailable)?
            .take();
        tokio::task::spawn_blocking(move || {
            let result = (|| {
                #[cfg(test)]
                if let Some(gate) = &mut gate {
                    if let Some(entered) = gate.entered.take() {
                        let _ = entered.send(());
                    }
                    gate.release
                        .recv_timeout(Duration::from_secs(30))
                        .map_err(|_| HeldCreateClientError::Unavailable)?;
                }
                for file in files {
                    file.revalidate()
                        .map_err(|_| HeldCreateClientError::UnsafeFilesystem)?;
                }
                Ok(())
            })();
            drop(capacity);
            #[cfg(test)]
            if let Some(mut gate) = gate {
                if let Some(finished) = gate.finished.take() {
                    let _ = finished.send(());
                }
            }
            result
        })
        .await
        .map_err(|_| HeldCreateClientError::Unavailable)?
    }

    pub(super) fn request_bytes<T: Serialize>(&self, value: &T) -> ClientResult<Vec<u8>> {
        let value =
            serde_json::to_value(value).map_err(|_| HeldCreateClientError::InvalidRequest)?;
        if self.redactor.value_unclipped(&value) != value {
            return Err(HeldCreateClientError::InvalidRequest);
        }
        let bytes =
            serde_json::to_vec(&value).map_err(|_| HeldCreateClientError::InvalidRequest)?;
        if bytes.len() > 64 * 1024 {
            return Err(HeldCreateClientError::InvalidRequest);
        }
        Ok(bytes)
    }

    pub(super) async fn exchange<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> ClientResult<(StatusCode, T)> {
        self.revalidate_material().await?;
        let mut url = self.origin.clone();
        url.set_path(path);
        let mut request = self
            .http
            .request(
                if body.is_some() {
                    reqwest::Method::POST
                } else {
                    reqwest::Method::GET
                },
                url,
            )
            .header(header::AUTHORIZATION, self.authorization.clone())
            .header(header::ACCEPT, "application/json")
            .header(header::CACHE_CONTROL, "no-store");
        if let Some(body) = body {
            request = request
                .header(header::CONTENT_TYPE, "application/json")
                .body(body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| HeldCreateClientError::Transport)?;
        let status = response.status();
        if status.is_redirection() {
            return Err(HeldCreateClientError::Protocol);
        }
        let header_bytes: usize = response
            .headers()
            .iter()
            .map(|(k, v)| k.as_str().len() + v.as_bytes().len())
            .sum();
        if response.headers().len() > 64
            || header_bytes > 32768
            || response.content_length().is_some_and(|n| n > 128 * 1024)
            || response
                .headers()
                .get_all(header::CONTENT_TYPE)
                .iter()
                .count()
                != 1
            || !response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(';').next())
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
        {
            return Err(HeldCreateClientError::Protocol);
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| HeldCreateClientError::Transport)?;
            if bytes.len().saturating_add(chunk.len()) > 128 * 1024 {
                return Err(HeldCreateClientError::Protocol);
            }
            bytes.extend_from_slice(&chunk);
        }
        let value =
            crate::json::decode_unique(&bytes, 64).map_err(|_| HeldCreateClientError::Protocol)?;
        if !matches!(status, StatusCode::OK | StatusCode::CREATED) {
            return Err(remote_error(status, &value));
        }
        if self.redactor.value_unclipped(&value) != value {
            return Err(HeldCreateClientError::Protocol);
        }
        let decoded = serde_json::from_value(value).map_err(|_| HeldCreateClientError::Protocol)?;
        Ok((status, decoded))
    }

    pub(super) async fn handshake(
        &self,
    ) -> ClientResult<(BrokerIdentityViewV1, KubernetesHeldCreateProtocolV1)> {
        let (status, identity) = self.exchange("/api/v1/brokers/identity", None).await?;
        if status != StatusCode::OK {
            return Err(HeldCreateClientError::Protocol);
        }
        let (status, protocol) = self
            .exchange("/api/v1/brokers/kubernetes/held-create-protocol", None)
            .await?;
        if status != StatusCode::OK {
            return Err(HeldCreateClientError::Protocol);
        }
        validation::protocol(&self.scope, &identity, &protocol)?;
        let mut revisions = self
            .revisions
            .lock()
            .map_err(|_| HeldCreateClientError::Unavailable)?;
        if identity.binding_revision < revisions.0 || identity.grant_revision < revisions.1 {
            return Err(HeldCreateClientError::InvalidContext);
        }
        *revisions = (identity.binding_revision, identity.grant_revision);
        Ok((identity, protocol))
    }
}

fn remote_error(status: StatusCode, value: &Value) -> HeldCreateClientError {
    let code = value
        .get("error")
        .and_then(|v| v.get("code"))
        .and_then(Value::as_str);
    if code == Some("control_clock_uncertain") {
        return HeldCreateClientError::Clock;
    }
    match status {
        StatusCode::BAD_REQUEST => HeldCreateClientError::ControlRejected,
        StatusCode::UNAUTHORIZED => HeldCreateClientError::Authentication,
        StatusCode::FORBIDDEN => HeldCreateClientError::Forbidden,
        StatusCode::NOT_FOUND => HeldCreateClientError::NotFound,
        StatusCode::CONFLICT => match code {
            Some("stale_epoch") => HeldCreateClientError::StaleEpoch,
            Some("stale_revision") => HeldCreateClientError::StaleRevision,
            _ => HeldCreateClientError::Conflict,
        },
        StatusCode::TOO_MANY_REQUESTS => HeldCreateClientError::Capacity,
        _ if status.is_server_error() => HeldCreateClientError::Unavailable,
        _ => HeldCreateClientError::Protocol,
    }
}

#[cfg(test)]
pub(super) struct MaterialTestGate {
    entered: Option<tokio::sync::oneshot::Sender<()>>,
    release: std::sync::mpsc::Receiver<()>,
    finished: Option<tokio::sync::oneshot::Sender<()>>,
}
#[cfg(test)]
impl HeldCreateControlClient {
    pub(crate) fn pause_next_material_validation_for_test(
        &self,
        entered: tokio::sync::oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
        finished: tokio::sync::oneshot::Sender<()>,
    ) -> ClientResult<()> {
        let mut slot = self
            .material_gate
            .lock()
            .map_err(|_| HeldCreateClientError::Unavailable)?;
        if slot.is_some() {
            return Err(HeldCreateClientError::InvalidContext);
        }
        *slot = Some(MaterialTestGate {
            entered: Some(entered),
            release,
            finished: Some(finished),
        });
        Ok(())
    }
}
