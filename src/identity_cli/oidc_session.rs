//! Foreground human session; the control remains the authority for every scope and read.
use super::{
    SelectionArgs, export,
    http::{self, Client},
    oidc::{AuthHttp, Cancellation, Input, NativeIntent, print_json},
};
use crate::{
    identity::{
        HumanEventItemV1, HumanIdentityV1, HumanPageV1, HumanResourceItemV1, PrincipalRefV1,
    },
    oidc::{
        OidcClientKind, OidcError, OidcLogoutContext, OidcProvider, VerifiedAccess, VerifiedLogin,
    },
};
use anyhow::{Result, ensure};
use clap::{Args, Parser, Subcommand};
use serde::Deserialize;
use serde_json::json;
use std::{collections::VecDeque, path::PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Admission {
    version: u32,
    authentication_id: String,
    generation: u32,
    expires_at: String,
}
impl Admission {
    fn validate_reference(&self) -> Result<()> {
        ensure!(
            self.version == 1 && self.generation > 0,
            "OIDC admission version or generation is invalid"
        );
        let id = uuid::Uuid::parse_str(&self.authentication_id)
            .map_err(|_| anyhow::anyhow!("OIDC admission reference is invalid"))?;
        ensure!(
            id.to_string() == self.authentication_id,
            "OIDC admission reference is invalid"
        );
        Ok(())
    }
    /// A correctly bound acknowledgement remains usable for a targeted close
    /// after its business expiry; it must never authorize a read at that point.
    fn validate_binding(&self, access: &VerifiedAccess, expected: Option<&Self>) -> Result<i64> {
        self.validate_reference()?;
        if let Some(previous) = expected {
            ensure!(
                self.authentication_id == previous.authentication_id
                    && Some(self.generation) == previous.generation.checked_add(1),
                "OIDC admission rotation conflicts with this session"
            );
        } else {
            ensure!(
                self.generation == 1,
                "OIDC admission must begin at generation 1"
            );
        }
        let expires = chrono::DateTime::parse_from_rfc3339(&self.expires_at)
            .map_err(|_| anyhow::anyhow!("OIDC admission expiry is invalid"))?
            .timestamp();
        ensure!(
            expires > 0 && expires <= access.expires_at(),
            "OIDC admission expiry is outside the verified access proof"
        );
        Ok(expires)
    }
    fn validate(&self, access: &VerifiedAccess, expected: Option<&Self>) -> Result<i64> {
        let expires = self.validate_binding(access, expected)?;
        ensure!(
            expires > chrono::Utc::now().timestamp(),
            "OIDC admission expiry is outside the verified access proof"
        );
        Ok(expires)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Logout {
    version: u32,
    local_session: String,
    provider_logout: String,
    audit: String,
}

pub(super) struct Session {
    gateway: String,
    provider: OidcProvider,
    control: AuthHttp,
    access: VerifiedAccess,
    access_token: String,
    refresh_token: Option<String>,
    logout_context: Option<OidcLogoutContext>,
    admission: Admission,
    client: Client,
    context: Option<String>,
    closed: bool,
    pending_native: Option<NativeIntent>,
    native_outcome: &'static str,
    native_request_ambiguous: bool,
    queued_input: VecDeque<String>,
}
impl Session {
    pub(super) async fn admit(
        gateway: String,
        provider: OidcProvider,
        control: AuthHttp,
        login: VerifiedLogin,
        intent: NativeIntent,
        input: &mut tokio::sync::mpsc::Receiver<Input>,
        cancellation: &mut Cancellation,
    ) -> Result<Option<Self>> {
        let (access, tokens, logout_context) = login.into_session_parts();
        let mut queued_input = VecDeque::new();
        let mut cancelled = cancellation.observed();
        let mut input_error = None;
        while !cancelled {
            let item = match input.try_recv() {
                Ok(item) => Some(item),
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => None,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
            };
            match queue_or_cancel(item, &mut queued_input) {
                Ok(stop) => cancelled = stop,
                Err(error) => {
                    input_error = Some(error);
                    cancelled = true;
                }
            }
        }
        if cancelled {
            let finish = control
                .finish_native(
                    None,
                    &intent,
                    if input_error.is_some() {
                        "failed"
                    } else {
                        "cancelled"
                    },
                )
                .await;
            print_json(
                &json!({"version":1,"status":"session_closed","admission":"not_started",
                "native_intent_audit":if finish.is_ok(){"recorded"}else{"unavailable"}}),
            )?;
            if let Some(error) = input_error {
                return Err(error);
            }
            return finish.map(|()| None);
        }
        // Complete every local step independent of the admission reference
        // before asking the gateway to create a session. In particular, a short
        // refresh literal must fail before any authority is granted.
        let private = tokens
            .refresh_token()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let prepared = match Client::prepare_in_memory(
            &gateway,
            tokens.access_token(),
            access.expires_at(),
            private,
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                let finish = control.finish_native(None, &intent, "failed").await;
                return Err(if finish.is_ok() {
                    error
                } else {
                    error.context("OIDC native outcome audit is unavailable; no automatic retry")
                });
            }
        };
        let returned: Result<Admission> = {
            let operation = async {
                let admission: Admission = tokio::time::timeout(
                    intent.remaining()?,
                    control.post(
                        "admit",
                        tokens.access_token(),
                        json!({"version":1,"intent_id":intent.id()}),
                        reqwest::StatusCode::CREATED,
                    ),
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "OIDC admission exceeded its native intent deadline; no automatic retry"
                    )
                })??;
                Ok(admission)
            };
            tokio::pin!(operation);
            loop {
                tokio::select! {
                    biased;
                    _ = cancellation.wait(), if !cancelled => cancelled = true,
                    item = input.recv(), if !cancelled => match queue_or_cancel(item,&mut queued_input) {
                        Ok(stop) => cancelled = stop,
                        Err(error) => {input_error = Some(error);cancelled = true;}
                    },
                    // Cancellation never drops or retries the original request.
                    // Its original HTTP/intent deadlines bound resolution; a known
                    // accepted result below is closed before any ready/read.
                    result = &mut operation => break result,
                }
            }
        };
        let admission = match returned {
            Ok(admission) => admission,
            Err(error) => {
                let finish = control
                    .finish_native(
                        None,
                        &intent,
                        if cancelled { "cancelled" } else { "failed" },
                    )
                    .await;
                if cancelled {
                    print_json(
                        &json!({"version":1,"status":"session_closed","admission":"unresolved",
                        "local_session":"unknown","native_intent_audit":if finish.is_ok(){"recorded"}else{"unavailable"}}),
                    )?;
                    return Err(error.context("OIDC admission was cancelled but its remote outcome is unresolved; no automatic retry"));
                }
                return Err(if finish.is_ok() {
                    error
                } else {
                    error.context("OIDC native outcome audit is unavailable; no automatic retry")
                });
            }
        };
        // An invalid reference, generation or proof boundary is not authority
        // for a targeted logout, even when the response claimed acceptance.
        let expires = match admission.validate_binding(&access, None) {
            Ok(expires) => expires,
            Err(error) => {
                let finish = control.finish_native(None, &intent, "failed").await;
                return Err(if finish.is_ok() {
                    error
                } else {
                    error.context("OIDC native outcome audit is unavailable; no automatic retry")
                });
            }
        };
        if cancelled || cancellation.observed() {
            // Closing uses the known reference and original admitted bearer,
            // even if its business expiry elapsed while the reply was held.
            // A cancelled login never constructs a usable read Client.
            report_logout(
                &control,
                &provider,
                tokens.access_token(),
                &admission,
                logout_context,
                "not_required",
            )
            .await?;
            if let Some(error) = input_error {
                return Err(error);
            }
            return Ok(None);
        }
        let client = match prepared.bind_admission(
            &admission.authentication_id,
            admission.generation,
            expires,
        ) {
            Ok(client) => client,
            Err(error) => {
                // The intent was consumed by admission. Close the known tuple
                // once with its original bearer, using the bounded non-retrying
                // logout path even if expiry elapsed while the ACK was held.
                let close = report_logout(
                    &control,
                    &provider,
                    tokens.access_token(),
                    &admission,
                    logout_context,
                    "not_required",
                )
                .await;
                return Err(if close.is_ok() {
                    error
                } else {
                    error.context(
                        "OIDC admitted session close report is unavailable; no automatic retry",
                    )
                });
            }
        };
        let mut session = Self {
            gateway,
            provider,
            control,
            access,
            access_token: tokens.access_token().to_owned(),
            refresh_token: tokens.refresh_token().map(str::to_owned),
            logout_context,
            admission,
            client,
            context: None,
            closed: false,
            pending_native: None,
            native_outcome: "failed",
            native_request_ambiguous: false,
            queued_input,
        };
        if cancelled || cancellation.observed() {
            session.close().await?;
            if let Some(error) = input_error {
                return Err(error);
            }
            Ok(None)
        } else {
            Ok(Some(session))
        }
    }
    pub(super) async fn run(
        &mut self,
        input: &mut tokio::sync::mpsc::Receiver<Input>,
        cancellation: &mut Cancellation,
    ) -> Result<()> {
        let result = self.run_inner(input, cancellation).await;
        // Local credentials are discarded on every exit, regardless of network,
        // IdP or mandatory audit availability. Those outcomes remain separate.
        let close = self.close().await;
        result.and(close)
    }
    async fn run_inner(
        &mut self,
        input: &mut tokio::sync::mpsc::Receiver<Input>,
        cancellation: &mut Cancellation,
    ) -> Result<()> {
        let mut pending = std::mem::take(&mut self.queued_input);
        if cancellation.observed() {
            return Ok(());
        }
        loop {
            let item = match input.try_recv() {
                Ok(item) => Some(item),
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => None,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
            };
            if queue_or_cancel(item, &mut pending)? {
                return Ok(());
            }
        }
        if cancellation.observed() {
            return Ok(());
        }
        self.client.revalidate()?;
        print_json(
            &json!({"version":1,"status":"session_ready","authentication_id":self.admission.authentication_id,
            "generation":self.admission.generation,"expires_at":self.admission.expires_at,
            "next_action":"identity; context <offered ID>; resources; events; export; refresh; logout. Type help for options.",
            "storage":"foreground_memory_only","provider_authentication":"independent of Codex and Claude"}),
        )?;
        loop {
            if cancellation.observed() {
                break;
            }
            let remaining = self.remaining()?;
            let deadline = tokio::time::Instant::now() + remaining;
            let line = if let Some(line) = pending.pop_front() {
                line
            } else {
                tokio::select! {
                    biased;
                    _ = cancellation.wait() => break,
                    _ = tokio::time::sleep_until(deadline) => anyhow::bail!("OIDC foreground session expired or reached its 15 minute idle limit; sign in again"),
                    item = input.recv() => match item {
                        Some(Input::Line(line)) => line,
                        Some(Input::Invalid) => anyhow::bail!("OIDC foreground input is invalid or exceeds 8192 bytes"),
                        _ => break,
                    }
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            if matches!(line.trim(), "logout" | "quit" | "exit") {
                break;
            }
            if line.trim() == "help" {
                help()?;
                continue;
            }
            let command = words(&line).and_then(|words| Prompt::try_parse_from(std::iter::once("human-sso".to_owned()).chain(words))
                .map(|p|p.command).map_err(|_|anyhow::anyhow!("invalid foreground command; type help for supported commands and options")));
            let command = match command {
                Ok(command) => command,
                Err(_) => {
                    print_json(
                        &json!({"version":1,"status":"command_refused","reason":"invalid command or options; type help"}),
                    )?;
                    continue;
                }
            };
            let refreshing = matches!(&command, PromptCommand::Refresh);
            // A queued logout/EOF supersedes an in-flight read, export or refresh.
            // Dropping the future prevents a late response from reopening state.
            let outcome = {
                let operation = self.command(command);
                tokio::pin!(operation);
                loop {
                    tokio::select! {
                        biased;
                        _ = cancellation.wait() => break None,
                        _ = tokio::time::sleep_until(deadline) => break Some(Err(anyhow::anyhow!("OIDC foreground session expired; sign in again"))),
                        item = input.recv() => match item {
                            Some(Input::Line(line)) if !matches!(line.trim(), "logout" | "quit" | "exit") => {
                                if pending.len() >= 8 { break Some(Err(anyhow::anyhow!("foreground command queue exceeded its bound"))); }
                                pending.push_back(line);
                            }
                            Some(Input::Invalid) => break Some(Err(anyhow::anyhow!("OIDC foreground input is invalid or exceeds 8192 bytes"))),
                            _ => break None,
                        },
                        result = &mut operation => break Some(result),
                    }
                }
            };
            match outcome {
                None => {
                    self.native_outcome = "cancelled";
                    break;
                }
                Some(Ok(())) => {}
                Some(Err(error)) => {
                    self.context = None;
                    if refreshing {
                        return Err(error.context("OIDC refresh is unresolved; this foreground session is closed, sign in again; no automatic refresh retry"));
                    }
                    print_json(
                        &json!({"version":1,"status":"command_refused","reason":error.to_string(),
                        "next_action":"Select a current context from identity, or logout and sign in again"}),
                    )?;
                }
            }
        }
        Ok(())
    }
    async fn identity(&self) -> Result<HumanIdentityV1> {
        let (identity, _): (HumanIdentityV1, _) = self.client.get("identity", None, &[]).await?;
        self.client.validate_identity(&identity)?;
        ensure!(
            matches!(&identity.principal, PrincipalRefV1::HumanOidc { issuer, subject }
            if issuer == self.access.issuer() && subject == self.access.subject())
                && identity.authentication.credential_id == self.admission.authentication_id,
            "OIDC control identity differs from this verified foreground session"
        );
        let expires = identity
            .authentication
            .expires_at
            .as_deref()
            .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
            .map(|v| v.timestamp());
        ensure!(
            expires.is_some_and(
                |exp| exp <= self.access.expires_at() && exp > chrono::Utc::now().timestamp()
            ),
            "OIDC control identity expiry is invalid"
        );
        Ok(identity)
    }
    async fn command(&mut self, command: PromptCommand) -> Result<()> {
        match command {
            PromptCommand::Identity => {
                let identity = self.identity().await?;
                print_json(&self.client.sanitized(&identity)?)
            }
            PromptCommand::Context { id } => {
                http::validate_opaque(&id)?;
                let identity = self.identity().await?;
                let context = identity
                    .contexts
                    .iter()
                    .find(|c| c.id == id)
                    .ok_or_else(|| {
                        anyhow::anyhow!("context was not offered by the control for this identity")
                    })?;
                ensure!(
                    chrono::DateTime::parse_from_rfc3339(&context.expires_at)?.timestamp()
                        > chrono::Utc::now().timestamp(),
                    "offered context has expired; request identity again"
                );
                self.context = Some(id);
                print_json(&self.client.sanitized(
                    &json!({"version":1,"status":"context_selected","context":context}),
                )?)
            }
            PromptCommand::Resources(page) => {
                let context = self.selected()?;
                let query = page.query()?;
                let (page, _): (HumanPageV1<HumanResourceItemV1>, _) =
                    self.client.get("resources", Some(context), &query).await?;
                http::validate_page(&page, context)?;
                print_json(&self.client.sanitized(&page)?)
            }
            PromptCommand::Events(args) => {
                let context = self.selected()?;
                let mut query = http::selection_query(&args.selection)?;
                query.extend(args.page.query()?);
                let (page, _): (HumanPageV1<HumanEventItemV1>, _) =
                    self.client.get("events", Some(context), &query).await?;
                http::validate_event_page(&page, context, args.selection.view)?;
                print_json(&self.client.sanitized(&page)?)
            }
            PromptCommand::Export(args) => {
                export::start_with_client(
                    &self.client,
                    self.selected()?,
                    args.selection,
                    args.output_dir,
                )
                .await
            }
            PromptCommand::ResumeExport { output_dir } => {
                export::resume_with_client(&self.client, self.selected()?, output_dir).await
            }
            PromptCommand::Refresh => self.refresh().await,
        }
    }
    fn selected(&self) -> Result<&str> {
        self.context.as_deref().ok_or_else(|| {
            anyhow::anyhow!("context selection required; run identity, then context <offered ID>")
        })
    }
    fn remaining(&self) -> Result<std::time::Duration> {
        let expiry = chrono::DateTime::parse_from_rfc3339(&self.admission.expires_at)
            .map_err(|_| anyhow::anyhow!("OIDC admission expiry is invalid"))?
            .timestamp_millis();
        let remaining = expiry.saturating_sub(chrono::Utc::now().timestamp_millis());
        ensure!(
            remaining > 0,
            "OIDC foreground session expired; sign in again"
        );
        Ok(std::time::Duration::from_millis(
            (remaining as u64).min(900_000),
        ))
    }
    async fn refresh(&mut self) -> Result<()> {
        self.context = None;
        let next_generation = self.admission.generation.checked_add(1).ok_or_else(|| {
            anyhow::anyhow!("OIDC session generation is exhausted; sign in again")
        })?;
        let refresh = self.refresh_token.as_deref().ok_or_else(|| {
            anyhow::anyhow!("OIDC provider did not supply a refresh token; sign in again")
        })?;
        self.native_outcome = "failed";
        self.native_request_ambiguous = true;
        let intent = self.control.native_intent(Some(&self.access_token),
            json!({"kind":"refresh","authentication_id":self.admission.authentication_id,"generation":self.admission.generation}),
            std::time::Duration::from_secs(15)).await?;
        self.native_request_ambiguous = false;
        self.pending_native = Some(intent);
        let intent = self
            .pending_native
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("OIDC native refresh intent is unavailable"))?;
        let login = tokio::time::timeout(
            intent.remaining()?,
            self.provider
                .refresh(OidcClientKind::Cli, refresh, &self.access),
        )
        .await
        .map_err(|_| anyhow::anyhow!("OIDC native refresh intent expired; no automatic retry"))??;
        let (access, tokens, logout_context) = login.into_session_parts();
        let refresh_token = tokens
            .refresh_token()
            .map(str::to_owned)
            .or_else(|| self.refresh_token.clone());
        let private = refresh_token.iter().cloned().collect();
        let mut client = Client::in_memory(
            &self.gateway,
            tokens.access_token(),
            &self.admission.authentication_id,
            next_generation,
            access.expires_at(),
            private,
        )?;
        // Retain every known literal before rotating server authority. A full
        // dictionary fails while the original admission can still be closed.
        client.inherit_memory_redaction(&self.client)?;
        let admission: Admission = tokio::time::timeout(intent.remaining()?,self.control.post(
                "rotate",
                &self.access_token,
                json!({"version":1,"authentication_id":self.admission.authentication_id,
                "generation":self.admission.generation,"intent_id":intent.id(),"access_token":tokens.access_token()}),
                reqwest::StatusCode::OK,
            )).await.map_err(|_|anyhow::anyhow!("OIDC rotation exceeded its native intent deadline; no automatic retry"))??;
        let expires = admission.validate(&access, Some(&self.admission))?;
        client.bound_memory_expiry(expires)?;
        self.access = access;
        self.access_token = tokens.access_token().to_owned();
        self.refresh_token = refresh_token;
        self.admission = admission;
        self.client = client;
        self.pending_native = None;
        if logout_context.is_some() {
            self.logout_context = logout_context;
        }
        print_json(
            &json!({"version":1,"status":"session_refreshed","authentication_id":self.admission.authentication_id,
            "generation":self.admission.generation,"expires_at":self.admission.expires_at,
            "next_action":"Run identity and select a newly offered context; old contexts/cursors and export generation are invalid"}),
        )
    }
    async fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.context = None;
        self.refresh_token = None;
        // Keep only the exact old admitted bearer long enough for this one
        // declared finish RPC. An ambiguous exchange never triggers a retry.
        let native_audit = if let Some(intent) = self.pending_native.take() {
            if self
                .control
                .finish_native(Some(&self.access_token), &intent, self.native_outcome)
                .await
                .is_ok()
            {
                "recorded"
            } else {
                "unavailable"
            }
        } else if self.native_request_ambiguous {
            "unavailable"
        } else {
            "not_required"
        };
        let context = self.logout_context.take();
        let result = report_logout(
            &self.control,
            &self.provider,
            &self.access_token,
            &self.admission,
            context,
            native_audit,
        )
        .await;
        self.access_token.clear();
        result
    }
}

async fn report_logout(
    control: &AuthHttp,
    provider: &OidcProvider,
    bearer: &str,
    admission: &Admission,
    context: Option<OidcLogoutContext>,
    native_audit: &str,
) -> Result<()> {
    let result: Result<Logout> = control.post("logout",bearer,
        json!({"version":1,"authentication_id":admission.authentication_id,"generation":admission.generation}),reqwest::StatusCode::OK).await;
    let (local_session, audit, control_provider) = match result {
        Ok(value)
            if value.version == 1
                && matches!(value.local_session.as_str(), "closed" | "unknown")
                && matches!(value.audit.as_str(), "recorded" | "unavailable")
                && matches!(
                    value.provider_logout.as_str(),
                    "accepted" | "failed" | "unavailable" | "unsupported" | "not_attempted"
                ) =>
        {
            (value.local_session, value.audit, value.provider_logout)
        }
        _ => (
            "unknown".to_owned(),
            "unavailable".to_owned(),
            "not_attempted".to_owned(),
        ),
    };
    let provider_outcome = if let Some(context) = context {
        match provider.logout_idp(context).await {
            Ok(()) => "accepted",
            Err(OidcError::NotConfigured) => "unsupported",
            Err(OidcError::ProviderRejected) => "failed",
            Err(_) => "unavailable",
        }
    } else {
        "not_attempted"
    };
    print_json(
        &json!({"version":1,"status":"session_closed","local_session":local_session,
            "provider_logout":provider_outcome,"control_provider_logout":control_provider,"audit":audit,
            "native_intent_audit":native_audit,
            "scope":"this foreground ai-guard session; this does not assert global JWT revocation"}),
    )
}

fn queue_or_cancel(item: Option<Input>, queue: &mut VecDeque<String>) -> Result<bool> {
    match item {
        None | Some(Input::Eof) => Ok(true),
        Some(Input::Invalid) => {
            anyhow::bail!("OIDC foreground input is invalid or exceeds 8192 bytes")
        }
        Some(Input::Line(line)) if matches!(line.trim(), "logout" | "quit" | "exit") => Ok(true),
        Some(Input::Line(line)) => {
            ensure!(
                queue.len() < 8,
                "foreground command queue exceeded its bound"
            );
            queue.push_back(line);
            Ok(false)
        }
    }
}

#[derive(Parser)]
struct Prompt {
    #[command(subcommand)]
    command: PromptCommand,
}
#[derive(Subcommand)]
enum PromptCommand {
    Identity,
    Context {
        id: String,
    },
    Resources(Page),
    Events(EventArgs),
    Export(ExportArgs),
    ResumeExport {
        #[arg(long)]
        output_dir: PathBuf,
    },
    Refresh,
}
#[derive(Args)]
struct Page {
    #[arg(long,default_value_t=25,value_parser=clap::value_parser!(u16).range(1..=100))]
    limit: u16,
    #[arg(long)]
    cursor: Option<String>,
}
impl Page {
    fn query(self) -> Result<Vec<(String, String)>> {
        let mut query = vec![("limit".into(), self.limit.to_string())];
        if let Some(cursor) = self.cursor {
            http::validate_opaque(&cursor)?;
            query.push(("cursor".into(), cursor));
        }
        Ok(query)
    }
}
#[derive(Args)]
struct EventArgs {
    #[command(flatten)]
    selection: SelectionArgs,
    #[command(flatten)]
    page: Page,
}
#[derive(Args)]
struct ExportArgs {
    #[command(flatten)]
    selection: SelectionArgs,
    #[arg(long)]
    output_dir: PathBuf,
}
fn help() -> Result<()> {
    print_json(
        &json!({"version":1,"status":"commands","commands":["identity","context <offered ID>",
        "resources [--limit 1..100] [--cursor <opaque cursor>]",
        "events --view metadata|content --resource-kind session|sandbox|agent|project --resource-id <ID> [--limit 1..100] [--cursor <cursor>]",
        "export --view metadata|content --resource-kind session|sandbox|agent|project --resource-id <ID> --output-dir <new private destination>",
        "resume-export --output-dir <same destination, foreground session and generation>","refresh","logout"],
        "input":"Quote arguments containing spaces. No shell expansion is performed. EOF and Ctrl-C close the session."}),
    )
}
/// Command words only. No shell, variable expansion, substitution or execution.
fn words(line: &str) -> Result<Vec<String>> {
    ensure!(
        line.len() <= 8192 && !line.chars().any(|c| c.is_control() && c != '\t'),
        "invalid foreground command"
    );
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for c in line.chars() {
        if escaped {
            word.push(c);
            escaped = false;
            started = true;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escaped = true;
            started = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                word.push(c);
            }
            started = true;
        } else if matches!(c, '\'' | '"') {
            quote = Some(c);
            started = true;
        } else if c.is_whitespace() {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
        } else {
            word.push(c);
            started = true;
        }
        ensure!(
            words.len() <= 32,
            "foreground command exceeds its argument bound"
        );
    }
    ensure!(
        quote.is_none() && !escaped,
        "foreground command has unmatched quoting"
    );
    if started {
        words.push(word);
    }
    ensure!(
        !words.is_empty() && words.len() <= 32,
        "foreground command is empty or oversized"
    );
    Ok(words)
}
