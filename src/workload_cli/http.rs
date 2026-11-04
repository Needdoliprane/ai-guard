use super::{
    Result, WorkloadCliError as E,
    manifest::{Control, Request},
    storage::{Recovery, validate_receipt},
};
use crate::{
    control::ControlProtocolV1, control_fs::PrivateFile, workload::WorkloadBindingReceiptV1,
};
use futures_util::StreamExt;
use reqwest::{StatusCode, header};
use serde::de::DeserializeOwned;
use std::time::Duration;

pub(super) struct Material {
    source: PrivateFile,
    ca: Option<PrivateFile>,
    token: String,
}

impl Material {
    pub fn load(control: &Control) -> Result<Self> {
        let source =
            PrivateFile::open(&control.runner_token_file, 1024).map_err(|_| E::InvalidControl)?;
        let text = std::str::from_utf8(source.bytes()).map_err(|_| E::InvalidControl)?;
        let token = text.strip_suffix('\n').unwrap_or(text);
        if !(32..=512).contains(&token.len()) || token.chars().any(char::is_whitespace) {
            return Err(E::InvalidControl);
        }
        header::HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| E::InvalidControl)?;
        let token = token.to_owned();
        let ca = control
            .ca_bundle_file
            .as_ref()
            .map(|path| PrivateFile::open(path, 65_536).map_err(|_| E::InvalidControl))
            .transpose()?;
        if let Some(ca) = &ca {
            certificates(ca)?;
        }
        let material = Self { source, ca, token };
        material.revalidate()?;
        Ok(material)
    }
    pub fn token(&self) -> &str {
        &self.token
    }
    pub fn revalidate(&self) -> Result<()> {
        self.source.revalidate().map_err(|_| E::InvalidControl)?;
        if let Some(ca) = &self.ca {
            ca.revalidate().map_err(|_| E::InvalidControl)?;
        }
        Ok(())
    }
}

fn certificates(source: &PrivateFile) -> Result<Vec<reqwest::Certificate>> {
    let certificates = crate::tls::certificates(source.bytes()).map_err(|_| E::InvalidControl)?;
    if certificates.is_empty() || certificates.len() > 8 {
        return Err(E::InvalidControl);
    }
    certificates
        .iter()
        .map(|certificate| {
            reqwest::Certificate::from_der(certificate.as_ref()).map_err(|_| E::InvalidControl)
        })
        .collect()
}

pub(super) struct Client {
    http: reqwest::Client,
    origin: reqwest::Url,
    authorization: header::HeaderValue,
}

impl Client {
    pub fn new(control: &Control, material: &Material) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .https_only(control.gateway.scheme() == "https")
            .http1_only()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(3))
            .read_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(12))
            .pool_max_idle_per_host(0);
        if let Some(ca) = &material.ca {
            if control.gateway.scheme() != "https" {
                return Err(E::InvalidControl);
            }
            builder = builder.tls_built_in_root_certs(false);
            for certificate in certificates(ca)? {
                builder = builder.add_root_certificate(certificate);
            }
        }
        let mut authorization =
            header::HeaderValue::from_str(&format!("Bearer {}", material.token()))
                .map_err(|_| E::InvalidControl)?;
        authorization.set_sensitive(true);
        Ok(Self {
            http: builder.build().map_err(|_| E::InvalidControl)?,
            origin: control.gateway.clone(),
            authorization,
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut url = self.origin.clone();
        url.set_path(path);
        self.http
            .request(method, url)
            .header(header::AUTHORIZATION, self.authorization.clone())
            .header(header::ACCEPT, "application/json")
            .header(header::CACHE_CONTROL, "no-store")
    }

    pub async fn protocol(&self) -> Result<ControlProtocolV1> {
        let response = self
            .request(reqwest::Method::GET, "/api/v1/control/protocol")
            .send()
            .await
            .map_err(|_| E::Transport)?;
        let (status, bytes) = response_bytes(response).await?;
        if status != StatusCode::OK {
            return Err(public_error(status, &bytes));
        }
        let value: ControlProtocolV1 = decode(&bytes)?;
        value.validate().map_err(|_| E::Protocol)?;
        Ok(value)
    }

    pub async fn post(&self, recovery: &Recovery) -> Result<WorkloadBindingReceiptV1> {
        let path = match &recovery.request {
            Request::Issue(_) => "/api/v1/workload-bindings".to_owned(),
            Request::Revoke(value) => {
                format!("/api/v1/workload-bindings/{}/revoke", value.credential_id)
            }
        };
        let bytes = recovery.request.bytes()?;
        if bytes.len() > 16_384 {
            return Err(E::UnsafeState);
        }
        let response = self
            .request(reqwest::Method::POST, &path)
            .header(header::CONTENT_TYPE, "application/json")
            .body(bytes)
            .send()
            .await
            .map_err(|_| E::Transport)?;
        let (status, bytes) = response_bytes(response).await?;
        if !status.is_success() {
            return Err(public_error(status, &bytes));
        }
        let mut receipt: WorkloadBindingReceiptV1 = decode(&bytes)?;
        validate_receipt(recovery, &receipt)?;
        match &recovery.request {
            Request::Issue(_)
                if (status == StatusCode::CREATED && !receipt.replayed)
                    || (status == StatusCode::OK && receipt.replayed) => {}
            Request::Revoke(_) if status == StatusCode::OK => {}
            _ => return Err(E::Protocol),
        }
        // Retain canonical historical evidence; an exact replay changes only this bit.
        receipt.replayed = false;
        Ok(receipt)
    }
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|_| E::Protocol)
}

async fn response_bytes(response: reqwest::Response) -> Result<(StatusCode, Vec<u8>)> {
    let headers = response.headers();
    let header_bytes: usize = headers
        .iter()
        .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
        .sum();
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if headers.len() > 64
        || header_bytes > 32_768
        || response.content_length().is_some_and(|n| n > 16_384)
        || content_type.split(';').next() != Some("application/json")
    {
        return Err(E::Protocol);
    }
    let status = response.status();
    if status.is_redirection() {
        return Err(E::Protocol);
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| E::Transport)?;
        if bytes.len().saturating_add(chunk.len()) > 16_384 {
            return Err(E::Protocol);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((status, bytes))
}

fn public_error(status: StatusCode, bytes: &[u8]) -> E {
    let value = serde_json::from_slice::<serde_json::Value>(bytes).ok();
    let code = value
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(|v| v.get("code"))
        .and_then(|v| v.as_str());
    if code == Some("control_clock_uncertain") {
        return E::Clock;
    }
    match status {
        StatusCode::UNAUTHORIZED => E::Authentication,
        StatusCode::FORBIDDEN => E::Forbidden,
        StatusCode::BAD_REQUEST => E::InvalidInput,
        StatusCode::CONFLICT => E::Conflict,
        StatusCode::TOO_MANY_REQUESTS => E::Capacity,
        StatusCode::SERVICE_UNAVAILABLE => E::Unavailable,
        _ => E::Protocol,
    }
}
