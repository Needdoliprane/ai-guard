//! Single-use native callback listener. It never logs or returns callback parameters.
use anyhow::{Result, ensure};
use std::{collections::BTreeMap, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const PATH: &str = "/oidc/callback";
const MAX_HEADERS: usize = 8192;
const MAX_QUERY: usize = 4096;

pub(super) struct CallbackListener {
    listener: TcpListener,
    authority: String,
}
pub(super) enum Callback {
    Code { code: String, state: String },
    Refused { state: String },
}
impl CallbackListener {
    pub(super) async fn bind() -> Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| anyhow::anyhow!("OIDC loopback callback could not be bound"))?;
        let authority = listener
            .local_addr()
            .map_err(|_| anyhow::anyhow!("OIDC loopback callback address is unavailable"))?
            .to_string();
        Ok(Self {
            listener,
            authority,
        })
    }
    pub(super) fn redirect(&self) -> String {
        format!("http://{}{PATH}", self.authority)
    }

    /// This consumes and closes the listener even when parsing or writing fails.
    pub(super) async fn receive(self, issuer: &str) -> Result<Callback> {
        let (mut stream, peer) = self
            .listener
            .accept()
            .await
            .map_err(|_| anyhow::anyhow!("OIDC callback connection is unavailable"))?;
        ensure!(peer.ip().is_loopback(), "OIDC callback is invalid");
        let result = tokio::time::timeout(Duration::from_secs(2), self.parse(&mut stream, issuer))
            .await
            .map_err(|_| anyhow::anyhow!("OIDC callback exceeded its deadline"))
            .and_then(|r| r);
        let success = matches!(&result, Ok(Callback::Code { .. }));
        // Fixed local text: no URI, code, state or IdP error reflection.
        let (status, body) = if success {
            (
                "200 OK",
                "Authorization received. Return to the ai-guard terminal.\n",
            )
        } else {
            (
                "400 Bad Request",
                "Authorization was refused or invalid. Return to the ai-guard terminal.\n",
            )
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            stream.write_all(response.as_bytes()),
        )
        .await;
        let _ = stream.shutdown().await;
        result
    }
    async fn parse(&self, stream: &mut TcpStream, issuer: &str) -> Result<Callback> {
        let invalid = || anyhow::anyhow!("OIDC callback is invalid");
        let mut bytes = Vec::with_capacity(2048);
        let mut block = [0_u8; 1024];
        loop {
            let count = stream.read(&mut block).await.map_err(|_| invalid())?;
            ensure!(
                count > 0 && bytes.len().saturating_add(count) <= MAX_HEADERS,
                "OIDC callback is invalid"
            );
            bytes.extend_from_slice(&block[..count]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                ensure!(end + 4 == bytes.len(), "OIDC callback is invalid");
                break;
            }
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
        ensure!(
            text.is_ascii() && !text.contains('\0'),
            "OIDC callback is invalid"
        );
        let mut lines = text[..text.len() - 4].split("\r\n");
        let request: Vec<_> = lines.next().ok_or_else(invalid)?.split(' ').collect();
        ensure!(
            request.len() == 3
                && request[0] == "GET"
                && matches!(request[2], "HTTP/1.1" | "HTTP/1.0"),
            "OIDC callback is invalid"
        );
        let target = request[1];
        ensure!(
            target.len() <= MAX_QUERY + PATH.len() + 1 && !target.contains('#'),
            "OIDC callback is invalid"
        );
        let (path, query) = target.split_once('?').ok_or_else(invalid)?;
        ensure!(
            path == PATH && query.len() <= MAX_QUERY && !query.is_empty(),
            "OIDC callback is invalid"
        );
        let mut headers = BTreeMap::new();
        for line in lines {
            ensure!(
                headers.len() < 32 && !line.starts_with([' ', '\t']),
                "OIDC callback is invalid"
            );
            let (name, value) = line.split_once(':').ok_or_else(invalid)?;
            ensure!(
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)),
                "OIDC callback is invalid"
            );
            ensure!(
                value.bytes().all(|b| b == b'\t' || (32..=126).contains(&b)),
                "OIDC callback is invalid"
            );
            ensure!(
                headers
                    .insert(name.to_ascii_lowercase(), value.trim())
                    .is_none(),
                "OIDC callback is invalid"
            );
        }
        ensure!(
            headers.get("host") == Some(&self.authority.as_str())
                && !headers.contains_key("transfer-encoding")
                && headers.get("content-length").is_none_or(|v| *v == "0"),
            "OIDC callback is invalid"
        );
        // form_urlencoded otherwise silently accepts malformed percent escapes.
        for pair in query.split('&') {
            ensure!(
                !pair.is_empty() && pair.contains('='),
                "OIDC callback is invalid"
            );
            let raw = pair.as_bytes();
            let mut at = 0;
            while at < raw.len() {
                if raw[at] == b'%' {
                    ensure!(
                        at + 2 < raw.len()
                            && raw[at + 1].is_ascii_hexdigit()
                            && raw[at + 2].is_ascii_hexdigit(),
                        "OIDC callback is invalid"
                    );
                    at += 3;
                } else {
                    at += 1;
                }
            }
        }
        let url = reqwest::Url::parse(&format!("http://{}{target}", self.authority))
            .map_err(|_| invalid())?;
        let mut parameters = BTreeMap::new();
        for (key, value) in url.query_pairs() {
            ensure!(
                matches!(
                    key.as_ref(),
                    "state"
                        | "code"
                        | "error"
                        | "error_description"
                        | "error_uri"
                        | "session_state"
                        | "iss"
                ) && value.len() <= 2048
                    && !value.contains('\u{fffd}')
                    && !value.chars().any(char::is_control),
                "OIDC callback is invalid"
            );
            ensure!(
                parameters
                    .insert(key.into_owned(), value.into_owned())
                    .is_none(),
                "OIDC callback is invalid"
            );
        }
        let state = parameters.remove("state").ok_or_else(invalid)?;
        ensure!(
            !state.is_empty() && state.len() <= 256,
            "OIDC callback is invalid"
        );
        ensure!(
            parameters.get("iss").is_none_or(|value| value == issuer),
            "OIDC callback is invalid"
        );
        match (parameters.remove("code"), parameters.remove("error")) {
            (Some(code), None) if !code.is_empty() => Ok(Callback::Code { code, state }),
            (None, Some(error)) if !error.is_empty() => Ok(Callback::Refused { state }),
            _ => Err(invalid()),
        }
    }
}
