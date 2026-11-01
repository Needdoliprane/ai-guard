use super::{ConnectionArgs, EventsArgs, ResourcesArgs, SelectionArgs, View};
use crate::{
    control_fs::{PrivateFile, absolute_path},
    identity::{
        HumanEventItemV1, HumanIdentityV1, HumanPageV1, HumanReadViewV1, HumanResourceItemV1,
    },
    redaction::Redactor,
};
use anyhow::{Context, Result, ensure};
use futures_util::StreamExt;
use reqwest::{Url, header};
use serde::{Deserialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{net::IpAddr, time::Duration};

pub(super) const MAX_PAGE_BYTES: usize = 262_144;

pub(super) struct Client {
    http: reqwest::Client,
    origin: Url,
    credential: PrivateFile,
    authorization: header::HeaderValue,
    redactor: Redactor,
    pub credential_digest: String,
}

pub(super) fn credential_text(bytes: &[u8]) -> Result<&str> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("private credential file has an invalid format"))?;
    let token = text.strip_suffix('\n').unwrap_or(text);
    ensure!(
        token.len() == 70
            && token.starts_with("aigh1_")
            && token[6..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "private credential file must contain one local human credential, optionally followed by one newline"
    );
    Ok(token)
}

impl Client {
    pub fn new(args: &ConnectionArgs) -> Result<Self> {
        let origin = validate_origin(&args.gateway)?;
        let credential = PrivateFile::open(&absolute_path(&args.credential_file)?, 128)?;
        let token = credential_text(credential.bytes())?;
        let mut authorization = header::HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| anyhow::anyhow!("private credential cannot be used for authentication"))?;
        authorization.set_sensitive(true);
        let redactor = Redactor::new(vec![token.to_owned()]);
        let credential_digest = hex::encode(Sha256::digest(token.as_bytes()));
        let http = reqwest::Client::builder()
            .https_only(origin.scheme() == "https")
            .http1_only()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(3))
            .read_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(12))
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|_| anyhow::anyhow!("human HTTP client could not be initialized"))?;
        Ok(Self {
            http,
            origin,
            credential,
            authorization,
            redactor,
            credential_digest,
        })
    }

    pub fn origin(&self) -> &str {
        self.origin.as_str()
    }

    pub fn revalidate(&self) -> Result<()> {
        self.credential.revalidate()
    }

    pub fn sanitized<T: serde::Serialize>(&self, value: &T) -> Result<serde_json::Value> {
        self.revalidate()?;
        Ok(self.redactor.value_unclipped(&serde_json::to_value(value)?))
    }

    pub async fn get<T: DeserializeOwned>(
        &self,
        route: &'static str,
        context: Option<&str>,
        query: &[(String, String)],
    ) -> Result<(T, usize)> {
        ensure!(
            matches!(route, "identity" | "resources" | "events" | "export"),
            "unsupported human read route"
        );
        self.revalidate()?;
        let mut url = self.origin.clone();
        url.set_path(&format!("/api/v1/human/{route}"));
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        }
        let mut request = self
            .http
            .get(url)
            .header(header::AUTHORIZATION, self.authorization.clone())
            .header(header::ACCEPT, "application/json")
            .header(header::CACHE_CONTROL, "no-store");
        if let Some(context) = context {
            validate_opaque(context)?;
            let value = header::HeaderValue::from_str(context).map_err(|_| {
                anyhow::anyhow!("invalid context; select an exact ID offered by human identity")
            })?;
            request = request.header("X-AiGuard-Context", value);
        }
        let response = request.send().await.map_err(|_| {
            anyhow::anyhow!("human request failed: connection, TLS or timeout; retain the selected context and retry explicitly")
        })?;
        let status = response.status();
        let header_bytes: usize = response
            .headers()
            .iter()
            .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
            .sum();
        ensure!(
            response.headers().len() <= 64 && header_bytes <= 32_768,
            "human response headers exceed the supported bound"
        );
        ensure!(
            response
                .content_length()
                .is_none_or(|len| len <= MAX_PAGE_BYTES as u64),
            "human response exceeds the page byte bound; reduce the page size"
        );
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        ensure!(
            content_type.split(';').next() == Some("application/json"),
            "human endpoint returned an unsupported response format"
        );
        let no_store = response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(',')
                    .any(|part| part.trim().eq_ignore_ascii_case("no-store"))
            });
        ensure!(
            no_store,
            "human endpoint did not confirm private no-store delivery"
        );
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .map_err(|_| anyhow::anyhow!("human response was interrupted; retry explicitly"))?;
            ensure!(
                bytes.len().saturating_add(chunk.len()) <= MAX_PAGE_BYTES,
                "human response grew beyond the page byte bound; reduce the page size"
            );
            bytes.extend_from_slice(&chunk);
        }
        self.revalidate()?;
        if !status.is_success() {
            // Never echo an error body, URL or remote text. Even a compromised
            // peer cannot cause a supplied bearer to reach the user's terminal.
            return Err(public_error(status.as_u16(), &bytes));
        }
        ensure!(
            status == reqwest::StatusCode::OK,
            "human endpoint returned an unsupported success status"
        );
        let value = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("human endpoint returned an invalid typed response"))?;
        Ok((value, bytes.len()))
    }
}

fn validate_origin(value: &str) -> Result<Url> {
    ensure!(
        value.len() <= 2048,
        "gateway origin exceeds its length bound"
    );
    let url = Url::parse(value).map_err(|_| anyhow::anyhow!("invalid gateway origin"))?;
    let local = url.host_str().is_some_and(|host| {
        host.trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    ensure!(
        url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/"
            && (url.scheme() == "https" || (url.scheme() == "http" && local)),
        "gateway must be a verified HTTPS origin or HTTP loopback origin, without userinfo, path, query or fragment"
    );
    Ok(url)
}

pub(super) fn validate_opaque(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= 4096 && value.bytes().all(|b| b.is_ascii_graphic()),
        "invalid or oversized opaque context/cursor; select a current context"
    );
    Ok(())
}

fn decimal(value: &str) -> Result<()> {
    let number = value.parse::<u64>().ok();
    ensure!(
        number.is_some_and(|n| n <= i64::MAX as u64 && n.to_string() == value),
        "human response contains a noncanonical decimal revision"
    );
    Ok(())
}

fn authorization_version(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "human response contains an invalid opaque authorization version"
    );
    Ok(())
}

pub(super) fn validate_page<T>(page: &HumanPageV1<T>, context: &str) -> Result<()> {
    ensure!(
        page.version == 1
            && page.context_id == context
            && page.items.len() <= 100
            && page.coverage == "authorized_selection",
        "human page version, context, item bound or coverage differs from the selected request"
    );
    authorization_version(&page.authorization_version)?;
    ensure!(
        page.has_more == page.next_cursor.is_some(),
        "human page pagination state is inconsistent"
    );
    if let Some(cursor) = &page.next_cursor {
        validate_opaque(cursor)?;
    }
    uuid::Uuid::parse_str(&page.read_receipt.request_id)
        .context("human page has an invalid read receipt")?;
    uuid::Uuid::parse_str(&page.read_receipt.audit_event_id)
        .context("human page has an invalid read receipt")?;
    chrono::DateTime::parse_from_rfc3339(&page.read_receipt.received_at)
        .context("human page has an invalid read receipt time")?;
    Ok(())
}

pub(super) fn validate_event_page(
    page: &HumanPageV1<HumanEventItemV1>,
    context: &str,
    view: View,
) -> Result<()> {
    validate_page(page, context)?;
    let expected = match view {
        View::Metadata => HumanReadViewV1::Metadata,
        View::Content => HumanReadViewV1::Content,
    };
    ensure!(
        page.view == Some(expected),
        "human event response view differs from the explicit selection"
    );
    if view == View::Metadata {
        ensure!(
            page.items.iter().all(|item| item.content.is_none()),
            "metadata response unexpectedly contained event content"
        );
    }
    Ok(())
}

pub(super) fn selection_query(selection: &SelectionArgs) -> Result<Vec<(String, String)>> {
    ensure!(
        !selection.resource_id.trim().is_empty()
            && selection.resource_id.len() <= 256
            && !selection.resource_id.chars().any(char::is_control),
        "resource ID is invalid"
    );
    Ok(vec![
        ("view".into(), selection.view.wire().into()),
        (
            "resource_kind".into(),
            selection.resource_kind.wire().into(),
        ),
        ("resource_id".into(), selection.resource_id.clone()),
    ])
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ErrorDetail,
}
#[derive(Deserialize)]
struct ErrorDetail {
    code: String,
    retry_action: String,
}

fn public_error(status: u16, bytes: &[u8]) -> anyhow::Error {
    let parsed = serde_json::from_slice::<ErrorEnvelope>(bytes).ok();
    let (code, action) = parsed
        .as_ref()
        .map_or(("protocol_error", "contact_operator"), |e| {
            let code = match e.error.code.as_str() {
                "authentication_required" => "authentication_required",
                "credential_invalid" => "credential_invalid",
                "credential_expired" => "credential_expired",
                "identity_revoked" => "identity_revoked",
                "context_required" => "context_required",
                "context_invalid" => "context_invalid",
                "context_expired" => "context_expired",
                "context_revoked" => "context_revoked",
                "permission_denied" => "permission_denied",
                "not_found" => "not_found",
                "invalid_request" => "invalid_request",
                "invalid_cursor" => "invalid_cursor",
                "capacity_exhausted" => "capacity_exhausted",
                "identity_unavailable" => "identity_unavailable",
                "audit_unavailable" => "audit_unavailable",
                "migration_required" => "migration_required",
                _ => "protocol_error",
            };
            let action = match e.error.retry_action.as_str() {
                "authenticate" => "authenticate",
                "select_context" => "select_context",
                "review_permissions" => "review_permissions",
                "reduce_page" => "reduce_page",
                "retry" => "retry",
                _ => "contact_operator",
            };
            (
                code,
                if code == "migration_required" {
                    "contact_operator"
                } else {
                    action
                },
            )
        });
    let guidance = if code == "migration_required" {
        "contact the operator to stop the controls and run the explicit schema4 to schema5 migration with a new verified backup"
    } else {
        match action {
            "authenticate" => "check the private human credential with the operator",
            "select_context" => {
                "run human identity, then explicitly select a currently offered context"
            }
            "review_permissions" => {
                "review the selected context and resource permissions with the operator"
            }
            "reduce_page" => "reduce the page size or review the explicit selection",
            "retry" => "retry explicitly after control availability is restored",
            _ => "contact the operator; preserve existing credentials and data",
        }
    };
    anyhow::anyhow!(
        "human access refused: HTTP {status}, {code}, retry_action={action}; {guidance}"
    )
}

pub(super) async fn identity(args: ConnectionArgs) -> Result<()> {
    let client = Client::new(&args)?;
    let (identity, _): (HumanIdentityV1, _) = client.get("identity", None, &[]).await?;
    validate_identity(&identity)?;
    super::offline::print_json(&client.sanitized(&identity)?)
}

fn validate_identity(identity: &HumanIdentityV1) -> Result<()> {
    ensure!(
        identity.version == 1
            && identity.principal.is_human()
            && identity.contexts.len() <= 128
            && identity.authentication.method == "local_credential",
        "unsupported human identity response"
    );
    identity.principal.validate()?;
    for context in &identity.contexts {
        validate_opaque(&context.id)?;
        decimal(&context.membership_revision)?;
        authorization_version(&context.authorization_version)?;
        crate::tenancy::validate_tenant_id(&context.tenant_id)?;
        chrono::DateTime::parse_from_rfc3339(&context.expires_at)
            .context("human context expiry is invalid")?;
    }
    Ok(())
}

pub(super) async fn resources(args: ResourcesArgs) -> Result<()> {
    let Some(context) = args.context else {
        ensure!(
            args.cursor.is_none(),
            "a resource cursor requires its explicit context"
        );
        let client = Client::new(&args.connection)?;
        let (identity, _): (HumanIdentityV1, _) = client.get("identity", None, &[]).await?;
        validate_identity(&identity)?;
        return super::offline::print_json(&client.sanitized(&serde_json::json!({"version":1,"status":"context_selection_required","identity":identity,"next_action":"select an offered context ID with --context"}))?);
    };
    ensure!((1..=100).contains(&args.limit), "page limit must be 1..100");
    let client = Client::new(&args.connection)?;
    let mut query = vec![("limit".into(), args.limit.to_string())];
    if let Some(cursor) = args.cursor {
        validate_opaque(&cursor)?;
        query.push(("cursor".into(), cursor));
    }
    let (page, _): (HumanPageV1<HumanResourceItemV1>, _) =
        client.get("resources", Some(&context), &query).await?;
    validate_page(&page, &context)?;
    for item in &page.items {
        decimal(&item.revision)?;
    }
    super::offline::print_json(&client.sanitized(&page)?)
}

pub(super) async fn events(args: EventsArgs) -> Result<()> {
    ensure!((1..=100).contains(&args.limit), "page limit must be 1..100");
    let client = Client::new(&args.access.connection)?;
    let mut query = selection_query(&args.selection)?;
    query.push(("limit".into(), args.limit.to_string()));
    if let Some(cursor) = args.cursor {
        validate_opaque(&cursor)?;
        query.push(("cursor".into(), cursor));
    }
    let (page, _): (HumanPageV1<HumanEventItemV1>, _) = client
        .get("events", Some(&args.access.context), &query)
        .await?;
    validate_event_page(&page, &args.access.context, args.selection.view)?;
    super::offline::print_json(&client.sanitized(&page)?)
}
