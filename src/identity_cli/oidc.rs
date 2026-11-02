//! Native human OIDC entry point. Provider and control tokens stay in this process.
use super::{
    http,
    oidc_callback::{Callback, CallbackListener},
    oidc_session::Session,
};
use crate::oidc::{OidcClientKind, OidcConfigV1, OidcProvider, VerifiedLogin};
use anyhow::{Result, ensure};
use clap::{Args, ValueEnum};
use futures_util::{FutureExt, StreamExt};
use reqwest::{Url, header};
use serde::{Deserialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    process::Stdio,
    time::Duration,
};

#[derive(Args)]
pub struct SsoArgs {
    /// Trusted gateway origin. This is ai-guard login, independent of AI provider credentials.
    #[arg(long)]
    pub gateway: String,
    /// External browser launch. Manual prints only the ephemeral authorization URL.
    #[arg(long, value_enum, default_value = "system")]
    pub browser: Browser,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum Browser {
    System,
    Manual,
}

/// Foreground output is one bounded JSON value per line. Standalone local
/// commands retain their established pretty-printed output.
pub(super) fn print_json(value: &serde_json::Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    ensure!(
        bytes.len() <= 524_288,
        "OIDC foreground output exceeds its bound"
    );
    bytes.push(b'\n');
    std::io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|_| anyhow::anyhow!("OIDC foreground output is unavailable"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    version: u32,
    enabled: bool,
    provider: Option<OidcConfigV1>,
    ca_bundle_pem: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentReply {
    version: u32,
    intent_id: String,
    expires_at: String,
}
/// A bounded correlation ticket, never a business credential or a PKCE proof.
/// It is kept only in memory and is consumed once by the control's durable CAS.
pub(super) struct NativeIntent {
    id: String,
    deadline: tokio::time::Instant,
}
impl NativeIntent {
    pub(super) fn id(&self) -> &str {
        &self.id
    }
    pub(super) fn remaining(&self) -> Result<Duration> {
        self.deadline
            .checked_duration_since(tokio::time::Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| {
                anyhow::anyhow!("OIDC native intent expired; start a new explicit login")
            })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FinishReply {
    version: u32,
    status: String,
}

/// Only predefined routes are available; there is no caller-selected IdP or tenant.
pub(super) struct AuthHttp {
    http: reqwest::Client,
    origin: Url,
}
impl AuthHttp {
    pub(super) fn new(gateway: &str) -> Result<Self> {
        let origin = http::validate_origin(gateway)?;
        Ok(Self {
            http: http::build_http(&origin)?,
            origin,
        })
    }
    async fn profile(&self) -> Result<Profile> {
        let mut url = self.origin.clone();
        url.set_path("/auth/oidc/cli-profile");
        let request = self
            .http
            .get(url)
            .header(header::ACCEPT, "application/json")
            .header(header::CACHE_CONTROL, "no-store");
        self.response(request, reqwest::StatusCode::OK).await
    }
    pub(super) async fn post<T: DeserializeOwned>(
        &self,
        route: &'static str,
        bearer: &str,
        value: serde_json::Value,
        expected: reqwest::StatusCode,
    ) -> Result<T> {
        ensure!(
            matches!(route, "admit" | "rotate" | "logout"),
            "unsupported OIDC control route"
        );
        self.post_optional(route, Some(bearer), value, expected)
            .await
    }
    pub(super) async fn native_intent(
        &self,
        bearer: Option<&str>,
        action: serde_json::Value,
        maximum: Duration,
    ) -> Result<NativeIntent> {
        let reply: IntentReply = self
            .post_optional(
                "native-intent",
                bearer,
                serde_json::json!({"version":1,"action":action}),
                reqwest::StatusCode::OK,
            )
            .await?;
        ensure!(
            reply.version == 1,
            "OIDC native intent acknowledgement is invalid"
        );
        let id = uuid::Uuid::parse_str(&reply.intent_id)
            .map_err(|_| anyhow::anyhow!("OIDC native intent acknowledgement is invalid"))?;
        ensure!(
            id.to_string() == reply.intent_id,
            "OIDC native intent acknowledgement is invalid"
        );
        let expires = chrono::DateTime::parse_from_rfc3339(&reply.expires_at)
            .map_err(|_| anyhow::anyhow!("OIDC native intent expiry is invalid"))?;
        let duration = expires
            .signed_duration_since(chrono::Utc::now())
            .to_std()
            .map_err(|_| anyhow::anyhow!("OIDC native intent is already expired"))?
            .min(maximum);
        ensure!(!duration.is_zero(), "OIDC native intent is already expired");
        Ok(NativeIntent {
            id: reply.intent_id,
            deadline: tokio::time::Instant::now() + duration,
        })
    }
    /// Only a fixed declared terminal outcome is sent; no provider response is
    /// promoted into an independently observed exchange or PKCE fact.
    pub(super) async fn finish_native(
        &self,
        bearer: Option<&str>,
        intent: &NativeIntent,
        outcome: &'static str,
    ) -> Result<()> {
        ensure!(
            matches!(outcome, "cancelled" | "failed"),
            "invalid native intent outcome"
        );
        let reply: FinishReply = self.post_optional("native-intent",bearer,
            serde_json::json!({"version":1,"action":{"kind":"finish","intent_id":intent.id(),"outcome":outcome}}),
            reqwest::StatusCode::OK).await?;
        ensure!(
            reply.version == 1 && reply.status == "finished",
            "OIDC native outcome acknowledgement is invalid"
        );
        Ok(())
    }
    async fn post_optional<T: DeserializeOwned>(
        &self,
        route: &'static str,
        bearer: Option<&str>,
        value: serde_json::Value,
        expected: reqwest::StatusCode,
    ) -> Result<T> {
        ensure!(
            matches!(route, "admit" | "rotate" | "logout" | "native-intent"),
            "unsupported OIDC control route"
        );
        let mut url = self.origin.clone();
        url.set_path(&format!("/auth/oidc/{route}"));
        let mut request = self
            .http
            .post(url)
            .header(header::ACCEPT, "application/json")
            .header(header::CACHE_CONTROL, "no-store")
            .json(&value);
        if let Some(bearer) = bearer {
            ensure!(
                !bearer.is_empty()
                    && bearer.len() <= 65_536
                    && bearer.bytes().all(|b| b.is_ascii_graphic()),
                "OIDC bearer format is invalid"
            );
            let mut authorization = header::HeaderValue::from_str(&format!("Bearer {bearer}"))
                .map_err(|_| anyhow::anyhow!("OIDC bearer format is invalid"))?;
            authorization.set_sensitive(true);
            request = request.header(header::AUTHORIZATION, authorization);
        }
        self.response(request, expected).await
    }
    async fn response<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        expected: reqwest::StatusCode,
    ) -> Result<T> {
        let response = request.send().await.map_err(|_| anyhow::anyhow!(
            "OIDC control request failed: connection, TLS or timeout; no exchange is retried automatically"
        ))?;
        let status = response.status();
        let header_bytes: usize = response
            .headers()
            .iter()
            .map(|(k, v)| k.as_str().len() + v.as_bytes().len())
            .sum();
        ensure!(
            response.headers().len() <= 64 && header_bytes <= 32_768,
            "OIDC control response headers exceed their bound"
        );
        ensure!(
            response.content_length().is_none_or(|n| n <= 131_072),
            "OIDC control response exceeds its bound"
        );
        ensure!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.split(';').next() == Some("application/json")),
            "OIDC control response format is invalid"
        );
        ensure!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v
                    .split(',')
                    .any(|p| p.trim().eq_ignore_ascii_case("no-store"))),
            "OIDC control did not confirm private no-store delivery"
        );
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| {
                anyhow::anyhow!("OIDC control response was interrupted; no automatic retry")
            })?;
            ensure!(
                bytes.len().saturating_add(chunk.len()) <= 131_072,
                "OIDC control response exceeds its bound"
            );
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            return Err(http::public_error(status.as_u16(), &bytes));
        }
        ensure!(
            status == expected,
            "OIDC control returned an unsupported success status"
        );
        serde_json::from_slice(&bytes).map_err(|_| {
            anyhow::anyhow!("OIDC CLI profile is invalid or control response is incompatible")
        })
    }
}

pub(super) enum Input {
    Line(String),
    Eof,
    Invalid,
}

/// One registered, sticky signal observer survives every native-login phase.
/// Dropping a temporary wait future does not drop this Signal subscription.
pub(super) struct Cancellation {
    signal: tokio::signal::unix::Signal,
    observed: bool,
}
impl Cancellation {
    fn new() -> Result<Self> {
        let signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .map_err(|_| anyhow::anyhow!("OIDC foreground cancellation observer is unavailable"))?;
        Ok(Self {
            signal,
            observed: false,
        })
    }
    pub(super) fn observed(&mut self) -> bool {
        if !self.observed && self.signal.recv().now_or_never().is_some() {
            self.observed = true;
        }
        self.observed
    }
    pub(super) async fn wait(&mut self) {
        if !self.observed {
            let _ = self.signal.recv().await;
            self.observed = true;
        }
    }
}
pub(super) fn input() -> tokio::sync::mpsc::Receiver<Input> {
    let (sender, receiver) = tokio::sync::mpsc::channel(8);
    // A plain owned reader thread does not keep Tokio's blocking pool alive on
    // cancellation. No line or error is logged; allocation and queue are bounded.
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut stdin = stdin.lock();
        let mut line = Vec::with_capacity(256);
        let mut byte = [0_u8; 1];
        loop {
            match stdin.read(&mut byte) {
                Ok(0) => {
                    if !line.is_empty() {
                        let item = String::from_utf8(line).map_or(Input::Invalid, Input::Line);
                        if sender.blocking_send(item).is_err() {
                            return;
                        }
                    }
                    let _ = sender.blocking_send(Input::Eof);
                    return;
                }
                Ok(_) if byte[0] == b'\n' => {
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    let item = String::from_utf8(std::mem::take(&mut line))
                        .map_or(Input::Invalid, Input::Line);
                    if sender.blocking_send(item).is_err() {
                        return;
                    }
                }
                Ok(_) if line.len() < 8192 && byte[0] != 0 => line.push(byte[0]),
                _ => {
                    let _ = sender.blocking_send(Input::Invalid);
                    return;
                }
            }
        }
    });
    receiver
}
fn closed(audit: &str) -> Result<()> {
    print_json(
        &serde_json::json!({"version":1,"status":"session_closed","native_intent_audit":audit,"scope":"this foreground ai-guard session"}),
    )
}
pub async fn execute(args: SsoArgs) -> Result<()> {
    let mut cancellation = Cancellation::new()?;
    let control = AuthHttp::new(&args.gateway)?;
    let profile = control.profile().await?;
    if cancellation.observed() {
        return closed("not_required");
    }
    ensure!(profile.version == 1, "OIDC CLI profile is invalid");
    if !profile.enabled {
        ensure!(
            profile.provider.is_none() && profile.ca_bundle_pem.is_none(),
            "OIDC CLI profile is invalid"
        );
        anyhow::bail!(
            "OIDC is disabled; ask the operator to configure ai-guard SSO, or use the separate local human credential commands"
        );
    }
    let config = profile
        .provider
        .ok_or_else(|| anyhow::anyhow!("OIDC CLI profile is invalid"))?;
    ensure!(
        config.cli_callback_path == "/oidc/callback",
        "OIDC CLI profile is invalid"
    );
    let timeout = Duration::from_secs(config.limits.pkce_ttl_seconds.min(300));
    let issuer = config.issuer.clone();
    let provider = if let Some(ca) = profile.ca_bundle_pem {
        ensure!(
            !ca.is_empty() && ca.len() <= 65_536 && !ca.contains("PRIVATE KEY"),
            "OIDC CLI public CA bundle is invalid"
        );
        OidcProvider::new_with_ca(config, ca.as_bytes())?
    } else {
        OidcProvider::new(config)?
    };
    // A durable ACK precedes all provider discovery, browser navigation and
    // exchange. It records this client's intent, not gateway-observed PKCE.
    let intent = control
        .native_intent(
            None,
            serde_json::json!({"kind":"login"}),
            Duration::from_secs(300),
        )
        .await?;
    let mut input = input();
    let result = if let Ok(remaining) = intent.remaining() {
        let attempt = native_login(&provider, &issuer, args.browser);
        tokio::pin!(attempt);
        tokio::select! {
            biased;
            _ = cancellation.wait() => Ok(Attempt::Cancelled),
            item = input.recv() => match item {
                Some(Input::Invalid) => Err(anyhow::anyhow!("OIDC foreground input is invalid or exceeds 8192 bytes")),
                Some(Input::Line(line)) if !matches!(line.trim(),"logout"|"quit"|"exit") =>
                    Err(anyhow::anyhow!("OIDC login is pending; finish in the browser or cancel with logout or EOF")),
                _ => Ok(Attempt::Cancelled),
            },
            result = tokio::time::timeout(remaining.min(timeout), &mut attempt) => result
                .map_err(|_|anyhow::anyhow!("OIDC login expired; start a new explicit login")).and_then(|r|r),
        }
    } else {
        Err(anyhow::anyhow!(
            "OIDC native intent expired; start a new explicit login"
        ))
    };
    let login = match result {
        Ok(Attempt::LoggedIn(login)) => *login,
        Ok(Attempt::Cancelled) => {
            let finish = control.finish_native(None, &intent, "cancelled").await;
            closed(if finish.is_ok() {
                "recorded"
            } else {
                "unavailable"
            })?;
            return finish;
        }
        Ok(Attempt::Refused) => {
            let finish = control.finish_native(None, &intent, "cancelled").await;
            if finish.is_err() {
                anyhow::bail!(
                    "OIDC sign in was cancelled or refused; native outcome audit is unavailable; no automatic retry"
                );
            }
            anyhow::bail!("OIDC sign in was cancelled or refused; start a new explicit login");
        }
        Err(error) => {
            let finish = control.finish_native(None, &intent, "failed").await;
            return Err(if finish.is_ok() {
                error
            } else {
                error.context("OIDC native outcome audit is unavailable; no automatic retry")
            });
        }
    };
    // The gateway observes the signed access admission and consumes the prior
    // intent. It did not independently observe this process's exchange/nonce.
    if let Some(mut session) = Session::admit(
        args.gateway,
        provider,
        control,
        login,
        intent,
        &mut input,
        &mut cancellation,
    )
    .await?
    {
        session.run(&mut input, &mut cancellation).await
    } else {
        Ok(())
    }
}

enum Attempt {
    LoggedIn(Box<VerifiedLogin>),
    Cancelled,
    Refused,
}
async fn native_login(provider: &OidcProvider, issuer: &str, browser: Browser) -> Result<Attempt> {
    provider.discover().await?;
    let listener = CallbackListener::bind().await?;
    let pending = provider
        .begin_login(OidcClientKind::Cli, &listener.redirect())
        .await?;
    let authorization_url = pending.authorization_url();
    let authorization = Url::parse(authorization_url)
        .map_err(|_| anyhow::anyhow!("OIDC authorization request is invalid"))?;
    let expected_state = authorization
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .ok_or_else(|| anyhow::anyhow!("OIDC authorization request is invalid"))?;
    match browser {
        Browser::Manual => print_json(
            &serde_json::json!({"version":1,"status":"browser_authorization_required",
            "authorization_url":authorization_url,"next_action":"Open this ephemeral URL in an external browser; return to this terminal. EOF or logout cancels."}),
        )?,
        Browser::System => {
            open_browser(authorization_url).await?;
            print_json(&serde_json::json!({"version":1,"status":"browser_opened",
                "next_action":"Complete ai-guard sign in in the external browser. EOF or logout cancels."}))?;
        }
    }
    let callback = listener.receive(issuer).await?;
    let (code, state) = match callback {
        Callback::Code { code, state } => (code, state),
        Callback::Refused { state } => {
            ensure!(
                Sha256::digest(state.as_bytes()) == Sha256::digest(expected_state.as_bytes()),
                "OIDC callback is invalid"
            );
            return Ok(Attempt::Refused);
        }
    };
    Ok(Attempt::LoggedIn(Box::new(
        provider.complete_login(pending, &code, &state).await?,
    )))
}

async fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let program = "/usr/bin/open";
    #[cfg(target_os = "linux")]
    let program = "/usr/bin/gio";
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let program = "/unsupported";
    let metadata = std::fs::symlink_metadata(program).map_err(|_| {
        anyhow::anyhow!(
            "approved system browser opener is unavailable; restart with --browser manual"
        )
    })?;
    ensure!(
        metadata.is_file() && metadata.uid() == 0 && metadata.mode() & 0o022 == 0,
        "approved system browser opener is not a protected regular file; restart with --browser manual"
    );
    let mut command = tokio::process::Command::new(program);
    #[cfg(target_os = "linux")]
    command.arg("open");
    command.arg("--").arg(url).env_clear();
    for key in [
        "HOME",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "LANG",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| {
        anyhow::anyhow!("system browser could not be opened; restart with --browser manual")
    })?;
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "system browser opener exceeded its deadline; restart with --browser manual"
            )
        })?
        .map_err(|_| {
            anyhow::anyhow!("system browser opener failed; restart with --browser manual")
        })?;
    ensure!(
        status.success(),
        "system browser opener refused the request; restart with --browser manual"
    );
    Ok(())
}
