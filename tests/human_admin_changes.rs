//! Delegated membership changes over the real gateway router and private test
//! SQLite stores of `support/supervision_seed.rs`. The change is posted to the
//! gateway itself; reads go through the dashboard proxy like the browser.
//! These tests prove server contracts, never a runtime or a broker process.
#[path = "support/supervision_seed.rs"]
mod supervision_seed;
mod support;

use agentsandbox::{audit::EventQuery, identity::HumanCapabilityV1 as C};
use reqwest::StatusCode;
use serde_json::{Value, json};
use supervision_seed::{Seed, SeedOptions, Served, serve};

struct Stack {
    seed: Seed,
    served: Served,
    _files: tempfile::TempDir,
}
struct Reply {
    status: StatusCode,
    body: Value,
}
impl Stack {
    async fn start() -> Self {
        let files = support::private_tempdir();
        let seed = Seed::build(
            files.path(),
            SeedOptions {
                workers: 4,
                events: 8,
                human_principal_events: None,
            },
        )
        .unwrap_or_else(|error| panic!("build the supervision seed: {error:#}"));
        let front = files.path().join("front");
        std::fs::create_dir(&front).unwrap();
        std::fs::write(front.join("index.html"), "<title>test frontend</title>").unwrap();
        let served = serve(&seed, front).await.expect("serve the test stack");
        Self {
            seed,
            served,
            _files: files,
        }
    }
    fn token(&self, account: &str) -> String {
        self.seed.accounts[account].token.clone()
    }
    async fn reply(response: reqwest::Response) -> Reply {
        let status = response.status();
        let raw = response.bytes().await.unwrap();
        Reply {
            status,
            body: serde_json::from_slice(&raw).unwrap_or(Value::Null),
        }
    }
    async fn get(&self, path: &str, account: &str, context: Option<&str>) -> Reply {
        let mut request = self
            .served
            .client
            .get(format!("{}{path}", self.served.dashboard))
            .bearer_auth(self.token(account));
        if let Some(context) = context {
            request = request.header("X-AiGuard-Context", context);
        }
        Self::reply(request.send().await.unwrap()).await
    }
    async fn context(&self, account: &str, mode: &str) -> Value {
        let identity = self.get("/api/v1/human/identity", account, None).await;
        assert_eq!(identity.status, StatusCode::OK, "{account} authenticates");
        identity.body["contexts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|context| context["mode"] == mode)
            .unwrap_or_else(|| panic!("the control offers {account} a {mode} context"))
            .clone()
    }
    async fn change(
        &self,
        account: &str,
        context: &str,
        membership_id: &str,
        body: &Value,
    ) -> Reply {
        let request = self
            .served
            .client
            .post(format!(
                "{}/api/v1/human/admin/memberships/{membership_id}",
                self.served.gateway
            ))
            .bearer_auth(self.token(account))
            .header("X-AiGuard-Context", context)
            .json(body);
        Self::reply(request.send().await.unwrap()).await
    }
}

fn body(revision: u64, capabilities: &[&str], active: bool) -> Value {
    json!({
        "operation_id": uuid::Uuid::new_v4().to_string(),
        "expected_revision": revision.to_string(),
        "capabilities": capabilities,
        "active": active,
    })
}

/// Sequence of the tenant's latest identity mutation.
fn last_mutation(stack: &Stack) -> Option<i64> {
    stack
        .seed
        .a
        .events(&EventQuery {
            kind: Some("control.identity.mutation".into()),
            order: Some("desc".into()),
            limit: Some(1),
            ..Default::default()
        })
        .unwrap()
        .first()
        .map(|stored| stored.seq)
}

/// Every refusal leaves the identity chain where it was.
async fn refused(
    stack: &Stack,
    account: &str,
    context: &str,
    membership_id: &str,
    body: &Value,
    status: StatusCode,
    code: &str,
) {
    let before = last_mutation(stack);
    let reply = stack.change(account, context, membership_id, body).await;
    assert_eq!(reply.status, status, "{body} → {}", reply.body);
    assert_eq!(reply.body["error"]["code"], code, "{}", reply.body);
    assert_eq!(last_mutation(stack), before, "a refusal changes nothing");
}

/// The code of a refusal of a context made stale by an earlier change.
async fn refused_context(stack: &Stack, context: &str, membership_id: &str, body: &Value) -> Value {
    let reply = stack.change("admin_a", context, membership_id, body).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.body);
    reply.body["error"]["code"].clone()
}

#[tokio::test]
async fn a_delegated_change_is_durable_attributed_and_idempotent() {
    let mut stack = Stack::start().await;
    stack
        .seed
        .set_capabilities(
            "admin_a",
            &[
                C::ReadMetadata,
                C::ReadContent,
                C::Export,
                C::ReadTenantMetadata,
                C::ReadMemberships,
                C::ManageMemberships,
            ],
        )
        .unwrap();
    let admin = stack.context("admin_a", "administration").await;
    let admin = admin["id"].as_str().unwrap().to_owned();
    let member = stack.seed.accounts["user_a1"].membership.clone();
    let before = stack.context("user_a1", "personal").await;
    let export = |context: &Value| {
        context["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["operation"] == "events_export")
            .unwrap()["permission"]
            .clone()
    };
    assert_eq!(export(&before), "granted");

    // Remove Export, keep the rest: a reduction the administrator may make.
    let change = body(member.revision, &["read_metadata", "read_content"], true);
    let reply = stack
        .change("admin_a", &admin, &member.membership_id, &change)
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert_eq!(reply.body["operation_id"], change["operation_id"]);
    assert_eq!(reply.body["membership_id"], member.membership_id.as_str());
    assert_eq!(
        reply.body["membership_revision"],
        (member.revision + 1).to_string()
    );
    let audit_seq = reply.body["audit_seq"].clone();

    // The mutation is durable and attributed to the authenticated administrator.
    let last = stack
        .seed
        .a
        .events(&EventQuery {
            kind: Some("control.identity.mutation".into()),
            order: Some("desc".into()),
            limit: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].seq.to_string(), audit_seq.as_str().unwrap());
    let recorded = serde_json::to_string(&last[0].event.data).unwrap();
    let agentsandbox::identity::PrincipalRefV1::HumanLocal { account_id, .. } =
        &stack.seed.accounts["admin_a"].principal
    else {
        panic!("seed administrators are local accounts");
    };
    assert!(
        recorded.contains(account_id.as_str()),
        "the actor is the administrator"
    );
    assert!(recorded.contains(member.membership_id.as_str()));

    // The change is a new authorization revision: the administrator selects
    // its context again. An exact replay returns the same receipt.
    let stale = refused_context(&stack, &admin, &member.membership_id, &change).await;
    assert_eq!(stale, "context_revoked");
    let admin = stack.context("admin_a", "administration").await;
    let admin = admin["id"].as_str().unwrap().to_owned();
    let mutation_before_replay = last_mutation(&stack);
    let replay = stack
        .change("admin_a", &admin, &member.membership_id, &change)
        .await;
    assert_eq!(replay.status, StatusCode::OK, "{}", replay.body);
    assert_eq!(replay.body["audit_seq"], audit_seq);
    assert_eq!(
        last_mutation(&stack),
        mutation_before_replay,
        "a replay writes nothing"
    );

    // The same operation identifier for another change is a conflict.
    let mut other = body(member.revision, &["read_metadata"], true);
    other["operation_id"] = change["operation_id"].clone();
    refused(
        &stack,
        "admin_a",
        &admin,
        &member.membership_id,
        &other,
        StatusCode::CONFLICT,
        "conflict",
    )
    .await;
    // A stale revision is a conflict too.
    refused(
        &stack,
        "admin_a",
        &admin,
        &member.membership_id,
        &body(member.revision, &["read_metadata"], true),
        StatusCode::CONFLICT,
        "conflict",
    )
    .await;

    // The member's next identity no longer offers the removed right.
    let after = stack.context("user_a1", "personal").await;
    assert_eq!(export(&after), "not_granted");
    assert_ne!(
        before["authorization_version"], after["authorization_version"],
        "the change is a new authorization revision"
    );
    let page = stack
        .get(
            "/api/v1/human/admin/memberships?limit=100",
            "admin_a",
            Some(&admin),
        )
        .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    let item = page.body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["membership_id"] == member.membership_id.as_str())
        .unwrap()
        .clone();
    assert_eq!(
        item["capabilities"],
        json!(["read_metadata", "read_content"])
    );
    assert_eq!(item["revision"], (member.revision + 1).to_string());
}

#[tokio::test]
async fn a_delegated_change_never_raises_a_right() {
    let mut stack = Stack::start().await;
    // The administrator holds ReadWorkerRegistry but not ReadContent.
    stack
        .seed
        .set_capabilities(
            "admin_a",
            &[
                C::ReadMetadata,
                C::ReadTenantMetadata,
                C::ReadWorkerRegistry,
                C::ReadMemberships,
                C::ManageMemberships,
            ],
        )
        .unwrap();
    let admin = stack.context("admin_a", "administration").await;
    let admin = admin["id"].as_str().unwrap().to_owned();
    let member = stack.seed.accounts["user_a2"].membership.clone();
    let own = stack.seed.accounts["admin_a"].membership.clone();
    let foreign = stack.seed.accounts["other_org_a"].membership.clone();
    let forbidden = StatusCode::FORBIDDEN;

    // A right the administrator does not hold.
    refused(
        &stack,
        "admin_a",
        &admin,
        &member.membership_id,
        &body(member.revision, &["read_metadata", "read_content"], true),
        forbidden,
        "permission_denied",
    )
    .await;
    // A control-only right, even one the administrator holds.
    refused(
        &stack,
        "admin_a",
        &admin,
        &member.membership_id,
        &body(
            member.revision,
            &["read_metadata", "read_worker_registry"],
            true,
        ),
        forbidden,
        "permission_denied",
    )
    .await;
    // The administrator's own membership, even a reduction.
    refused(
        &stack,
        "admin_a",
        &admin,
        &own.membership_id,
        &body(
            own.revision,
            &["read_metadata", "read_memberships", "manage_memberships"],
            true,
        ),
        forbidden,
        "permission_denied",
    )
    .await;
    // Another organization's membership is not disclosed.
    refused(
        &stack,
        "admin_a",
        &admin,
        &foreign.membership_id,
        &body(foreign.revision, &[], false),
        StatusCode::NOT_FOUND,
        "not_found",
    )
    .await;
    // The personal context of the same membership never changes memberships.
    let personal = stack.context("admin_a", "personal").await;
    refused(
        &stack,
        "admin_a",
        personal["id"].as_str().unwrap(),
        &member.membership_id,
        &body(member.revision, &[], false),
        forbidden,
        "permission_denied",
    )
    .await;
    // A closed body: an unknown field or an identity in the body is refused.
    let mut smuggled = body(member.revision, &["read_metadata"], true);
    smuggled["principal"] = json!("someone-else");
    refused(
        &stack,
        "admin_a",
        &admin,
        &member.membership_id,
        &smuggled,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await;
    // Deactivation is a reduction the administrator may make.
    let reply = stack
        .change(
            "admin_a",
            &admin,
            &member.membership_id,
            &body(member.revision, &["read_metadata"], false),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    // Without ManageMemberships nothing is changed.
    stack
        .seed
        .set_capabilities(
            "admin_a",
            &[C::ReadMetadata, C::ReadTenantMetadata, C::ReadMemberships],
        )
        .unwrap();
    let admin = stack.context("admin_a", "administration").await;
    refused(
        &stack,
        "admin_a",
        admin["id"].as_str().unwrap(),
        &member.membership_id,
        &body(member.revision + 1, &["read_metadata"], true),
        forbidden,
        "permission_denied",
    )
    .await;
}

impl Stack {
    async fn change_grant(&self, account: &str, context: &str, body: &Value) -> Reply {
        let request = self
            .served
            .client
            .post(format!("{}/api/v1/human/admin/grants", self.served.gateway))
            .bearer_auth(self.token(account))
            .header("X-AiGuard-Context", context)
            .json(body);
        Self::reply(request.send().await.unwrap()).await
    }
    async fn listed(&self, account: &str, session: &str) -> bool {
        let context = self.context(account, "personal").await;
        let page = self
            .get(
                "/api/v1/human/resources?limit=100",
                account,
                context["id"].as_str(),
            )
            .await;
        assert_eq!(page.status, StatusCode::OK, "{}", page.body);
        page.body["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["resource"]["resource_id"] == session)
    }
}

fn share(principal: &agentsandbox::identity::PrincipalRefV1, permissions: &[&str]) -> Value {
    json!({"principal": principal, "permissions": permissions})
}
fn grant_change(session: &str, revision: &str, shares: Vec<Value>) -> Value {
    json!({
        "operation_id": uuid::Uuid::new_v4().to_string(),
        "resource": {"kind": "session", "resource_id": session},
        "expected_revision": revision,
        "shares": shares,
    })
}

#[tokio::test]
async fn a_delegated_share_change_never_raises_a_right() {
    let mut stack = Stack::start().await;
    // The administrator may share metadata, not content.
    stack
        .seed
        .set_capabilities(
            "admin_a",
            &[
                C::ReadMetadata,
                C::ReadTenantMetadata,
                C::ReadMemberships,
                C::ManageGrants,
            ],
        )
        .unwrap();
    let admin = stack.context("admin_a", "administration").await;
    let admin = admin["id"].as_str().unwrap().to_owned();
    let session = stack.seed.session_shared_with_user_a2.clone();
    let user_a1 = stack.seed.accounts["user_a1"].principal.clone();
    let user_a2 = stack.seed.accounts["user_a2"].principal.clone();
    let own = stack.seed.accounts["admin_a"].principal.clone();
    let reader = stack.seed.accounts["workers_only_a"].principal.clone();

    let read = stack
        .get(
            &format!("/api/v1/human/admin/grants?resource_kind=session&resource_id={session}"),
            "admin_a",
            Some(&admin),
        )
        .await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.body);
    assert_eq!(read.body["owner"], json!(user_a1));
    assert_eq!(
        read.body["shares"],
        json!([{"principal": user_a2, "display_name": "user A2", "permissions": ["metadata"]}])
    );
    assert!(read.body["read_receipt"]["audit_event_id"].is_string());
    let revision = read.body["revision"].as_str().unwrap().to_owned();
    assert!(stack.listed("user_a2", &session).await);

    let forbidden = StatusCode::FORBIDDEN;
    for (body, status, code, why) in [
        (
            grant_change(
                &session,
                &revision,
                vec![share(&user_a2, &["metadata", "content"])],
            ),
            forbidden,
            "permission_denied",
            "content is not held by the administrator",
        ),
        (
            grant_change(&session, &revision, vec![share(&own, &["metadata"])]),
            forbidden,
            "permission_denied",
            "no share to oneself",
        ),
        (
            grant_change(
                &stack.seed.session_without_grant,
                "0",
                vec![share(&user_a2, &["metadata"])],
            ),
            StatusCode::NOT_FOUND,
            "not_found",
            "only an operator binds a resource first",
        ),
        (
            grant_change(&session, &revision, vec![share(&user_a2, &[])]),
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "an empty permission set is not a share",
        ),
        (
            grant_change(&session, "0", vec![share(&user_a2, &["metadata"])]),
            StatusCode::CONFLICT,
            "conflict",
            "a stale revision",
        ),
    ] {
        let before = last_mutation(&stack);
        let reply = stack.change_grant("admin_a", &admin, &body).await;
        assert_eq!(reply.status, status, "{why}: {}", reply.body);
        assert_eq!(reply.body["error"]["code"], code, "{why}: {}", reply.body);
        assert_eq!(last_mutation(&stack), before, "{why}: nothing changes");
    }

    // Add a held permission for another member and keep the existing share.
    let change = grant_change(
        &session,
        &revision,
        vec![
            share(&user_a2, &["metadata"]),
            share(&reader, &["metadata"]),
        ],
    );
    let reply = stack.change_grant("admin_a", &admin, &change).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let next: u64 = revision.parse::<u64>().unwrap() + 1;
    assert_eq!(reply.body["grant_revision"], next.to_string());

    // Remove the user A2 share: the session leaves that member's views.
    let admin = stack.context("admin_a", "administration").await;
    let admin = admin["id"].as_str().unwrap().to_owned();
    let reply = stack
        .change_grant(
            "admin_a",
            &admin,
            &grant_change(
                &session,
                &next.to_string(),
                vec![share(&reader, &["metadata"])],
            ),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert!(!stack.listed("user_a2", &session).await);

    // Without read_memberships the share list is not disclosed.
    stack
        .seed
        .set_capabilities(
            "admin_a",
            &[C::ReadMetadata, C::ReadTenantMetadata, C::ManageGrants],
        )
        .unwrap();
    let admin = stack.context("admin_a", "administration").await;
    let refused = stack
        .get(
            &format!("/api/v1/human/admin/grants?resource_kind=session&resource_id={session}"),
            "admin_a",
            admin["id"].as_str(),
        )
        .await;
    assert_eq!(refused.status, forbidden, "{}", refused.body);
}
