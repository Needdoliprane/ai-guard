//! `GET /api/v1/human/events/recent` on the real gateway and dashboard routers
//! over private test SQLite stores (synthetic data, see
//! `support/supervision_seed.rs`). No runtime or model is started.
//!
//! The newest-first read duplicates the selection of the forward read in SQL;
//! the differential test below is what keeps both in agreement.
#[path = "support/supervision_seed.rs"]
mod supervision_seed;
mod support;

use agentsandbox::{
    audit::EventQuery,
    identity::{HumanCapabilityV1, ResourceKindV1},
};
use reqwest::StatusCode;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    time::Duration,
};
use supervision_seed::{Reply, Seed, SeedOptions, Served, http_get, resource_event, serve};

const RECENT: &str = "/api/v1/human/events/recent";
const FORWARD: &str = "/api/v1/human/events";

struct Stack {
    seed: Seed,
    served: Served,
    _files: tempfile::TempDir,
}

impl Stack {
    async fn start(options: SeedOptions) -> Self {
        let files = support::private_tempdir();
        let seed = Seed::build(files.path(), options)
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
    async fn small() -> Self {
        Self::start(SeedOptions {
            workers: 12,
            events: 400,
            ..SeedOptions::default()
        })
        .await
    }
    fn token(&self, account: &str) -> String {
        self.seed.accounts[account].token.clone()
    }
    /// Direct gateway read: exactly the receipts of the route under test.
    async fn get(&self, path: &str, token: &str, context: Option<&str>) -> Reply {
        http_get(&self.served, &self.served.gateway, path, token, context)
            .await
            .unwrap()
    }
    /// Through the dashboard proxy, like the browser.
    async fn proxied(&self, path: &str, token: &str, context: Option<&str>) -> Reply {
        http_get(&self.served, &self.served.dashboard, path, token, context)
            .await
            .unwrap()
    }
    async fn context(&self, account: &str, mode: &str) -> (String, String) {
        let token = self.token(account);
        let reply = self.get("/api/v1/human/identity", &token, None).await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "the local human authenticates"
        );
        let context = reply.body["contexts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|context| context["mode"] == mode)
            .expect("the control offers the requested context")["id"]
            .as_str()
            .unwrap()
            .to_owned();
        (token, context)
    }
}

fn with(path: &str, pairs: &[(&str, &str)]) -> String {
    let mut url = reqwest::Url::parse(&format!("http://localhost{path}")).unwrap();
    url.query_pairs_mut().extend_pairs(pairs);
    format!("{}?{}", url.path(), url.query().unwrap_or_default())
}

fn ids(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["metadata"]["id"].as_str().unwrap().to_owned())
        .collect()
}

/// Account, context mode and query selection of one differential case.
type Case<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>);

/// Follows `next_cursor` to the end of a traversal and returns every id.
async fn traverse(
    stack: &Stack,
    route: &str,
    selection: &[(&str, &str)],
    token: &str,
    context: &str,
) -> Vec<String> {
    let mut all = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..200 {
        let mut pairs = selection.to_vec();
        if let Some(cursor) = &cursor {
            pairs.push(("cursor", cursor));
        }
        let reply = stack.get(&with(route, &pairs), token, Some(context)).await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "{route} {selection:?}: {}",
            reply.body
        );
        all.extend(ids(&reply.body));
        assert_eq!(
            reply.body["has_more"],
            reply.body["next_cursor"].is_string()
        );
        match reply.body["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => return all,
        }
    }
    panic!("the traversal did not end");
}

/// Optional exact-response handoff to the frontend wire tests.
fn write_wire(stack: &Stack, name: &str, bytes: &[u8]) {
    let Some(path) = std::env::var_os("AIGUARD_SUPERVISION_WIRE_DIR") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    assert!(path.is_absolute(), "the wire directory is an absolute path");
    let directory = agentsandbox::control_fs::Directory::open_existing(&path).unwrap();
    for account in stack.seed.accounts.values() {
        assert!(
            !bytes
                .windows(account.token.len())
                .any(|part| part == account.token.as_bytes()),
            "a test credential must never enter a wire artifact"
        );
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory.path().join(name))
        .unwrap();
    assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
    directory.revalidate().unwrap();
}

/// First `human_audit` frame of a stream, or `None` when the stream stays
/// silent for two seconds. Returns the frame page and its SSE id.
async fn first_frame(
    stack: &Stack,
    path: &str,
    token: &str,
    context: &str,
) -> (StatusCode, Option<(Value, String)>) {
    let mut response = stack
        .served
        .client
        .get(format!("{}{path}", stack.served.gateway))
        .bearer_auth(token)
        .header("X-AiGuard-Context", context)
        .header("accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    let status = response.status();
    if status != StatusCode::OK {
        return (status, None);
    }
    let mut text = String::new();
    loop {
        let chunk = tokio::time::timeout(Duration::from_secs(2), response.chunk()).await;
        let Ok(Ok(Some(bytes))) = chunk else {
            return (status, None);
        };
        text.push_str(&String::from_utf8_lossy(&bytes));
        for frame in text.split("\n\n") {
            if !frame.contains("event: human_audit") {
                continue;
            }
            let field = |name: &str| {
                frame
                    .lines()
                    .find_map(|line| line.strip_prefix(name))
                    .map(|value| value.trim().to_owned())
            };
            if let (Some(data), Some(id)) = (field("data:"), field("id:"))
                && let Ok(page) = serde_json::from_str::<Value>(&data)
            {
                return (status, Some((page, id)));
            }
        }
    }
}

#[tokio::test]
async fn recent_returns_newest_authorized_events_first() {
    let stack = Stack::small().await;
    let (token, context) = stack.context("admin_a", "administration").await;
    let reply = stack
        .get(
            &with(RECENT, &[("limit", "100"), ("view", "metadata")]),
            &token,
            Some(&context),
        )
        .await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "the additive route must exist: {}",
        reply.body
    );
    write_wire(&stack, "recent-first.json", &reply.raw);
    let mut keys: Vec<&str> = reply
        .body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "authorization_version",
            "context_id",
            "coverage",
            "has_more",
            "items",
            "live_cursor",
            "next_cursor",
            "order",
            "read_receipt",
            "version",
            "view"
        ],
        "the envelope is closed"
    );
    assert_eq!(reply.body["version"], 1);
    assert_eq!(reply.body["order"], "newest_first");
    assert_eq!(reply.body["coverage"], "authorized_selection");
    assert_eq!(reply.body["view"], "metadata");
    assert_eq!(reply.body["context_id"], context);
    let items = reply.body["items"].as_array().unwrap();
    assert_eq!(items.len(), 100, "a full page of the dense history");
    assert_eq!(reply.body["has_more"], true);
    assert!(reply.body["next_cursor"].is_string());
    assert!(reply.body["live_cursor"].is_string());
    assert_ne!(reply.body["next_cursor"], reply.body["live_cursor"]);
    assert!(items.iter().all(|item| item["content"].is_null()));
    let received: Vec<&str> = items
        .iter()
        .map(|item| item["metadata"]["received_at"].as_str().unwrap())
        .collect();
    let mut descending = received.clone();
    descending.sort_unstable_by(|left, right| right.cmp(left));
    assert_eq!(received, descending, "newest reception first");
    let encoded = reply.body.to_string();
    for hidden in [
        "sandbox-retired",
        "project-elsewhere",
        stack.seed.session_without_grant.as_str(),
        "control.identity.",
        token.as_str(),
    ] {
        assert!(!encoded.contains(hidden), "{hidden} is never returned");
    }

    let older = stack
        .get(
            &with(
                RECENT,
                &[
                    ("limit", "100"),
                    ("view", "metadata"),
                    ("cursor", reply.body["next_cursor"].as_str().unwrap()),
                ],
            ),
            &token,
            Some(&context),
        )
        .await;
    assert_eq!(older.status, StatusCode::OK);
    write_wire(&stack, "recent-older.json", &older.raw);
    assert!(
        older.body["live_cursor"].is_null(),
        "only the first page carries a live position"
    );
    let first: BTreeSet<String> = ids(&reply.body).into_iter().collect();
    assert!(ids(&older.body).iter().all(|id| !first.contains(id)));
}

#[tokio::test]
async fn recent_equals_reversed_forward_traversal_for_owner_share_and_administration() {
    let stack = Stack::small().await;
    let owned = stack.seed.session_owned_by_user_a1.clone();
    let shared = stack.seed.session_shared_with_user_a2.clone();
    let cases: Vec<Case<'_>> = vec![
        ("admin_a", "administration", vec![("view", "metadata")]),
        ("admin_a", "personal", vec![("view", "metadata")]),
        ("user_a1", "personal", vec![("view", "metadata")]),
        ("user_a1", "personal", vec![("view", "content")]),
        ("user_a2", "personal", vec![("view", "metadata")]),
        ("other_org_a", "personal", vec![("view", "metadata")]),
        (
            "admin_a",
            "administration",
            vec![
                ("view", "metadata"),
                ("resource_kind", "session"),
                ("resource_id", &owned),
            ],
        ),
        (
            "user_a2",
            "personal",
            vec![
                ("view", "metadata"),
                ("resource_kind", "session"),
                ("resource_id", &shared),
            ],
        ),
        (
            "user_a1",
            "personal",
            vec![
                ("view", "content"),
                ("resource_kind", "agent"),
                ("resource_id", "agent-planner"),
            ],
        ),
        (
            "admin_a",
            "administration",
            vec![
                ("view", "metadata"),
                ("resource_kind", "project"),
                ("resource_id", "project-atlas"),
            ],
        ),
    ];
    let mut sizes = Vec::new();
    for (account, mode, selection) in &cases {
        let (token, context) = stack.context(account, mode).await;
        let mut pairs = vec![("limit", "37")];
        pairs.extend(selection.iter().copied());
        let mut forward = traverse(&stack, FORWARD, &pairs, &token, &context).await;
        let recent = traverse(&stack, RECENT, &pairs, &token, &context).await;
        // The administrator owns no resource and holds no share: its personal
        // context is a real, empty selection.
        assert_eq!(
            forward.is_empty(),
            (*account, *mode) == ("admin_a", "personal"),
            "{account} {mode} {selection:?}: expected emptiness of the selection"
        );
        forward.reverse();
        assert_eq!(
            recent, forward,
            "{account} {mode} {selection:?}: the newest-first traversal is the exact reverse"
        );
        assert_eq!(
            recent.iter().collect::<BTreeSet<_>>().len(),
            recent.len(),
            "no event repeats across pages"
        );
        sizes.push(recent.len());
    }
    // Non-vacuity: the selections really differ with the rights of each context.
    let (admin, admin_personal, owner, stranger, other) =
        (sizes[0], sizes[1], sizes[2], sizes[4], sizes[5]);
    assert!(
        admin > owner,
        "administration reads the session owned by user A2"
    );
    assert_eq!(
        admin_personal, 0,
        "tenant metadata is not honoured in personal mode"
    );
    assert!(
        owner > stranger && stranger > 0,
        "a metadata share is a strict subset"
    );
    assert!(
        other > 0 && other < stranger,
        "another organization reads only its own project"
    );
    assert!(
        sizes[3] <= owner,
        "content never selects more than metadata"
    );
    assert!(sizes[6] < admin && sizes[7] < stranger && sizes[9] < admin);
}

#[tokio::test]
async fn live_cursor_resumes_stream_after_the_page_without_replay() {
    let stack = Stack::small().await;
    let (token, context) = stack.context("user_a1", "personal").await;
    let selection = [("limit", "100"), ("view", "metadata")];
    let page = stack
        .get(&with(RECENT, &selection), &token, Some(&context))
        .await;
    assert_eq!(
        page.status,
        StatusCode::OK,
        "the additive route must exist: {}",
        page.body
    );
    let live = page.body["live_cursor"].as_str().unwrap().to_owned();
    let known: BTreeSet<String> = ids(&page.body).into_iter().collect();

    // Nothing new yet: the stream does not replay the history it starts after.
    let stream = with(
        "/api/v1/human/stream",
        &[("limit", "100"), ("view", "metadata"), ("cursor", &live)],
    );
    let (status, frame) = first_frame(&stack, &stream, &token, &context).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        frame.is_none(),
        "a stream opened at the live position stays silent"
    );

    let session = stack.seed.session_owned_by_user_a1.clone();
    let mut appended = Vec::new();
    for index in 0..3 {
        let stored = stack
            .seed
            .a
            .append(resource_event(
                ResourceKindV1::Session,
                &session,
                9000 + index,
            ))
            .unwrap();
        appended.push(stored.event.id);
    }
    // Not authorized for this context: it must not appear in the live frame.
    stack
        .seed
        .a
        .append(resource_event(
            ResourceKindV1::Session,
            &stack.seed.session_owned_by_user_a2,
            9100,
        ))
        .unwrap();
    let (status, frame) = first_frame(&stack, &stream, &token, &context).await;
    assert_eq!(status, StatusCode::OK);
    let (frame, resume) = frame.expect("the new authorized events are delivered");
    assert_eq!(
        ids(&frame),
        appended,
        "only the events appended after the page, oldest first"
    );
    assert!(ids(&frame).iter().all(|id| !known.contains(id)));
    assert!(!resume.is_empty() && resume != live);

    // A reconnection from the position of the last delivered batch receives
    // only what was appended since: nothing is delivered twice.
    let later = stack
        .seed
        .a
        .append(resource_event(ResourceKindV1::Session, &session, 9200))
        .unwrap()
        .event
        .id;
    let resumed = with(
        "/api/v1/human/stream",
        &[("limit", "100"), ("view", "metadata"), ("cursor", &resume)],
    );
    let (status, frame) = first_frame(&stack, &resumed, &token, &context).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ids(&frame.expect("the reconnection delivers the newer event").0),
        vec![later],
        "a resumed stream never repeats a delivered event"
    );

    // The live position is bound to its selection: another limit is refused.
    for other in [
        vec![("limit", "25"), ("view", "metadata"), ("cursor", &live)],
        vec![
            ("limit", "100"),
            ("view", "metadata"),
            ("resource_kind", "session"),
            ("resource_id", &session),
            ("cursor", &live),
        ],
    ] {
        let (status, _) = first_frame(
            &stack,
            &with("/api/v1/human/stream", &other),
            &token,
            &context,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    // Opened without the position, the existing stream still replays history.
    let (_, replay) = first_frame(
        &stack,
        &with(
            "/api/v1/human/stream",
            &[("limit", "100"), ("view", "metadata")],
        ),
        &token,
        &context,
    )
    .await;
    let replay = ids(&replay.expect("history is replayed").0);
    let oldest = stack
        .get(
            &with(FORWARD, &[("limit", "100"), ("view", "metadata")]),
            &token,
            Some(&context),
        )
        .await;
    assert_eq!(
        replay,
        ids(&oldest.body),
        "without the position the stream restarts from the oldest authorized event"
    );
    assert!(replay.iter().all(|id| !appended.contains(id)));
}

#[tokio::test]
async fn older_pages_are_stable_while_new_events_arrive() {
    let stack = Stack::small().await;
    let (token, context) = stack.context("admin_a", "administration").await;
    let selection = [("limit", "10"), ("view", "metadata")];
    let mut snapshot = traverse(&stack, FORWARD, &selection, &token, &context).await;
    snapshot.reverse();
    let first = stack
        .get(&with(RECENT, &selection), &token, Some(&context))
        .await;
    assert_eq!(
        first.status,
        StatusCode::OK,
        "the additive route must exist: {}",
        first.body
    );
    for index in 0..25 {
        stack
            .seed
            .a
            .append(resource_event(
                ResourceKindV1::Session,
                &stack.seed.session_owned_by_user_a1,
                9500 + index,
            ))
            .unwrap();
    }
    let mut collected = ids(&first.body);
    let mut cursor = first.body["next_cursor"].as_str().map(str::to_owned);
    for _ in 0..3 {
        let next = cursor.expect("older pages exist");
        let mut pairs = selection.to_vec();
        pairs.push(("cursor", &next));
        let page = stack
            .get(&with(RECENT, &pairs), &token, Some(&context))
            .await;
        assert_eq!(page.status, StatusCode::OK);
        assert!(page.body["live_cursor"].is_null());
        collected.extend(ids(&page.body));
        cursor = page.body["next_cursor"].as_str().map(str::to_owned);
    }
    assert_eq!(
        collected,
        snapshot[..40],
        "older pages continue below the first page: no gap, no repeat, no event appended later"
    );
}

#[tokio::test]
async fn recent_records_one_events_receipt_and_respects_read_budget() {
    let stack = Stack::small().await;
    let (token, context) = stack.context("user_a1", "personal").await;
    let before = stack.seed.a.head().unwrap();
    let page = stack
        .get(
            &with(RECENT, &[("limit", "5"), ("view", "metadata")]),
            &token,
            Some(&context),
        )
        .await;
    assert_eq!(
        page.status,
        StatusCode::OK,
        "the additive route must exist: {}",
        page.body
    );
    assert_eq!(
        stack.seed.a.head().unwrap(),
        before + 1,
        "exactly one durable receipt"
    );
    let receipt = page.body["read_receipt"]["audit_event_id"]
        .as_str()
        .unwrap();
    let recorded = stack
        .seed
        .a
        .events(&EventQuery {
            kind: Some("control.identity.access".into()),
            ..Default::default()
        })
        .unwrap()
        .into_iter()
        .find(|stored| stored.event.id == receipt)
        .expect("the receipt is a recorded access event");
    assert_eq!(
        recorded.event.data["action"], "events",
        "the existing audit action is reused"
    );
    assert_eq!(recorded.event.data["result_count"], 5);
    assert!(stack.seed.a.integrity().unwrap().valid);
    drop(stack);

    // A principal whose cumulative read budget is spent receives no page.
    let stack = Stack::start(SeedOptions {
        workers: 12,
        events: 40,
        human_principal_events: Some(4),
    })
    .await;
    let (token, context) = stack.context("user_a1", "personal").await;
    let path = with(RECENT, &[("limit", "5"), ("view", "metadata")]);
    let mut served = 0;
    let refusal = loop {
        let reply = stack.get(&path, &token, Some(&context)).await;
        if reply.status != StatusCode::OK {
            break reply;
        }
        served += 1;
        assert!(served < 10, "the reduced budget must end the reads");
    };
    assert!(served >= 1, "the budget first allows a read");
    assert_eq!(refusal.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refusal.body["error"]["code"], "audit_read_budget_exhausted");
    assert!(refusal.body.get("items").is_none());
    let head = stack.seed.a.head().unwrap();
    let again = stack.get(&path, &token, Some(&context)).await;
    assert_eq!(again.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        stack.seed.a.head().unwrap(),
        head,
        "a refused read is not recorded as a read"
    );
}

#[tokio::test]
async fn recent_refuses_foreign_resource_other_tenant_and_reused_cursors() {
    let stack = Stack::small().await;
    let (admin, admin_context) = stack.context("admin_a", "administration").await;
    let base = [("limit", "10"), ("view", "metadata")];
    let first = stack
        .get(&with(RECENT, &base), &admin, Some(&admin_context))
        .await;
    assert_eq!(
        first.status,
        StatusCode::OK,
        "the additive route must exist: {}",
        first.body
    );
    let recent_cursor = first.body["next_cursor"].as_str().unwrap().to_owned();
    let live_cursor = first.body["live_cursor"].as_str().unwrap().to_owned();
    let forward = stack
        .get(&with(FORWARD, &base), &admin, Some(&admin_context))
        .await;
    let forward_cursor = forward.body["next_cursor"].as_str().unwrap().to_owned();
    let head = stack.seed.a.head().unwrap();

    let refused = |status: StatusCode, code: &'static str, reply: Reply| {
        assert_eq!(reply.status, status, "{}", reply.body);
        assert_eq!(reply.body["error"]["code"], code);
        assert!(reply.body.get("items").is_none());
    };
    // Cursors are bound to their route, selection and context.
    for (route, pairs) in [
        (
            RECENT,
            vec![
                ("limit", "10"),
                ("view", "metadata"),
                ("cursor", forward_cursor.as_str()),
            ],
        ),
        (
            RECENT,
            vec![
                ("limit", "10"),
                ("view", "metadata"),
                ("cursor", live_cursor.as_str()),
            ],
        ),
        (
            RECENT,
            vec![
                ("limit", "11"),
                ("view", "metadata"),
                ("cursor", recent_cursor.as_str()),
            ],
        ),
        (
            RECENT,
            vec![
                ("limit", "10"),
                ("view", "content"),
                ("cursor", recent_cursor.as_str()),
            ],
        ),
        (
            RECENT,
            vec![
                ("limit", "10"),
                ("view", "metadata"),
                ("cursor", "not-a-sealed-cursor"),
            ],
        ),
        (
            FORWARD,
            vec![
                ("limit", "10"),
                ("view", "metadata"),
                ("cursor", recent_cursor.as_str()),
            ],
        ),
    ] {
        let reply = stack
            .get(&with(route, &pairs), &admin, Some(&admin_context))
            .await;
        refused(StatusCode::BAD_REQUEST, "invalid_cursor", reply);
    }
    // Closed parameters.
    for pairs in [
        vec![("limit", "0")],
        vec![("limit", "101")],
        vec![("order", "asc")],
        vec![("resource_kind", "session")],
        vec![("resource_id", "session-without-kind")],
        vec![("view", "everything")],
    ] {
        let reply = stack
            .get(&with(RECENT, &pairs), &admin, Some(&admin_context))
            .await;
        refused(StatusCode::BAD_REQUEST, "invalid_request", reply);
    }
    // A session nobody was granted is indistinguishable from an absent one.
    let hidden = [
        ("limit", "10"),
        ("view", "metadata"),
        ("resource_kind", "session"),
        ("resource_id", stack.seed.session_without_grant.as_str()),
    ];
    let reply = stack
        .get(&with(RECENT, &hidden), &admin, Some(&admin_context))
        .await;
    write_wire(&stack, "recent-filtered-not-found.json", &reply.raw);
    refused(StatusCode::NOT_FOUND, "not_found", reply);
    assert_eq!(
        stack.seed.a.head().unwrap(),
        head,
        "no refused request is recorded as a read"
    );

    let (stranger, stranger_context) = stack.context("user_a2", "personal").await;
    let foreign = [
        ("limit", "10"),
        ("view", "metadata"),
        ("resource_kind", "session"),
        ("resource_id", stack.seed.session_owned_by_user_a1.as_str()),
    ];
    let reply = stack
        .get(&with(RECENT, &foreign), &stranger, Some(&stranger_context))
        .await;
    refused(StatusCode::NOT_FOUND, "not_found", reply);
    let reply = stack
        .get(
            &with(RECENT, &[("view", "content")]),
            &stranger,
            Some(&stranger_context),
        )
        .await;
    refused(StatusCode::FORBIDDEN, "permission_denied", reply);
    let (registry, registry_context) = stack.context("workers_only_a", "administration").await;
    let reply = stack
        .get(&with(RECENT, &base), &registry, Some(&registry_context))
        .await;
    refused(StatusCode::FORBIDDEN, "permission_denied", reply);
    // A cursor sealed for another context of the same tenant.
    let reply = stack
        .get(
            &with(
                RECENT,
                &[
                    ("limit", "10"),
                    ("view", "metadata"),
                    ("cursor", &recent_cursor),
                ],
            ),
            &stranger,
            Some(&stranger_context),
        )
        .await;
    refused(StatusCode::BAD_REQUEST, "invalid_cursor", reply);

    // Another tenant: its own events only, and no context of A.
    let (admin_b, context_b) = stack.context("admin_b", "administration").await;
    let b = stack
        .get(&with(RECENT, &base), &admin_b, Some(&context_b))
        .await;
    assert_eq!(b.status, StatusCode::OK);
    let known_a: BTreeSet<String> = traverse(&stack, RECENT, &base, &admin, &admin_context)
        .await
        .into_iter()
        .collect();
    assert!(!ids(&b.body).is_empty());
    assert!(ids(&b.body).iter().all(|id| !known_a.contains(id)));
    assert!(b.body["items"].as_array().unwrap().iter().all(|item| {
        item["metadata"]["resource"]["resource_id"] == stack.seed.session_b.as_str()
    }));
    let reply = stack
        .get(&with(RECENT, &base), &admin_b, Some(&admin_context))
        .await;
    refused(StatusCode::FORBIDDEN, "context_invalid", reply);
    let reply = stack.get(&with(RECENT, &base), &admin, None).await;
    refused(StatusCode::FORBIDDEN, "context_required", reply);
    let reply = stack
        .get(
            &with(RECENT, &base),
            "not-a-human-credential",
            Some(&admin_context),
        )
        .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

const HELD_PAGE: [(&str, &str); 2] = [("limit", "5"), ("view", "metadata")];

/// What a client still holds when the control changes its mind: its context,
/// the older-page cursor and the live position of a first page.
struct Held {
    token: String,
    context: String,
    older: String,
    stream: String,
}

/// Opens `/human/stream` and returns the response as it stands: a refusal
/// carries the JSON error contract, an admitted stream stays open.
async fn open_stream(stack: &Stack, path: &str, token: &str, context: &str) -> reqwest::Response {
    stack
        .served
        .client
        .get(format!("{}{path}", stack.served.gateway))
        .bearer_auth(token)
        .header("X-AiGuard-Context", context)
        .header("accept", "text/event-stream")
        .send()
        .await
        .unwrap()
}

/// Reads a first page as `account`, checks that both positions it issues are
/// usable, and leaves a stream open at the live position.
async fn hold(stack: &Stack, account: &str) -> (Held, reqwest::Response) {
    let (token, context) = stack.context(account, "personal").await;
    let first = stack
        .get(&with(RECENT, &HELD_PAGE), &token, Some(&context))
        .await;
    assert_eq!(
        first.status,
        StatusCode::OK,
        "the additive route must exist: {}",
        first.body
    );
    let position = |name: &str| {
        first.body[name]
            .as_str()
            .unwrap_or_else(|| panic!("the first page issues {name}"))
            .to_owned()
    };
    let (next, live) = (position("next_cursor"), position("live_cursor"));
    let older = with(
        RECENT,
        &[HELD_PAGE[0], HELD_PAGE[1], ("cursor", next.as_str())],
    );
    let stream = with(
        "/api/v1/human/stream",
        &[HELD_PAGE[0], HELD_PAGE[1], ("cursor", live.as_str())],
    );
    assert_eq!(
        stack.get(&older, &token, Some(&context)).await.status,
        StatusCode::OK,
        "precondition: the issued older-page cursor is usable"
    );
    let open = open_stream(stack, &stream, &token, &context).await;
    assert_eq!(
        open.status(),
        StatusCode::OK,
        "precondition: the issued live position opens a stream"
    );
    (
        Held {
            token,
            context,
            older,
            stream,
        },
        open,
    )
}

/// The route again, the older-page cursor already issued and a new stream at
/// the live position already issued: each is refused with this contract, and
/// no refusal carries an item.
async fn refuses_what_is_held(
    stack: &Stack,
    held: &Held,
    status: StatusCode,
    code: &str,
    retry_action: &str,
) {
    let refused = |what: &str, got: StatusCode, body: &Value| {
        assert_eq!(got, status, "{what}: {body}");
        assert_eq!(body["error"]["code"], code, "{what}");
        assert_eq!(body["error"]["retry_action"], retry_action, "{what}");
        assert!(body.get("items").is_none(), "{what}: no item");
    };
    let again = stack
        .get(&with(RECENT, &HELD_PAGE), &held.token, Some(&held.context))
        .await;
    refused("the route again", again.status, &again.body);
    let older = stack
        .get(&held.older, &held.token, Some(&held.context))
        .await;
    refused("the issued older-page cursor", older.status, &older.body);
    let stream = open_stream(stack, &held.stream, &held.token, &held.context).await;
    let status = stream.status();
    let body: Value = stream.json().await.unwrap_or(Value::Null);
    refused("a new stream at the issued live position", status, &body);
}

/// The stream that was open is closed by the control within `patience`, after
/// exactly one frame: a terminal error with one of these codes. It delivers
/// no event.
async fn ends_with_error(mut open: reqwest::Response, codes: &[&str], patience: Duration) {
    let deadline = tokio::time::Instant::now() + patience;
    let mut text = String::new();
    let closed = loop {
        match tokio::time::timeout_at(deadline, open.chunk()).await {
            Ok(Ok(Some(bytes))) => text.push_str(&String::from_utf8_lossy(&bytes)),
            Ok(Ok(None)) => break true,
            Ok(Err(error)) => panic!("the open stream failed instead of ending: {error}"),
            Err(_) => break false,
        }
    };
    let frames: Vec<(String, Value)> = text
        .split("\n\n")
        .filter_map(|frame| {
            let field = |name: &str| {
                frame
                    .lines()
                    .find_map(|line| line.strip_prefix(name))
                    .map(str::trim)
            };
            Some((
                field("event:")?.to_owned(),
                serde_json::from_str(field("data:")?).ok()?,
            ))
        })
        .collect();
    assert!(closed, "the control closes the open stream: {frames:?}");
    assert_eq!(frames.len(), 1, "one terminal frame, no event: {frames:?}");
    let (event, data) = &frames[0];
    assert_eq!(event, "error");
    let code = data["error"]["code"].as_str().unwrap_or_default();
    assert!(codes.contains(&code), "terminal frame: {data}");
    assert!(data.get("items").is_none());
}

/// A selection is stable for the life of its context because any identity
/// mutation of the tenant ends that context. The positions a first page
/// issued are then worth nothing: neither the route, nor its `next_cursor`,
/// nor its `live_cursor` returns an item, and an open stream ends.
#[tokio::test]
async fn a_rights_change_refuses_recent_and_the_positions_already_issued() {
    let mut stack = Stack::small().await;
    let session = stack.seed.session_owned_by_user_a1.clone();

    // A mutation that concerns another account raises the authorization
    // revision of the whole tenant.
    let (held, open) = hold(&stack, "user_a1").await;
    stack
        .seed
        .set_capabilities(
            "user_a2",
            &[
                HumanCapabilityV1::ReadMetadata,
                HumanCapabilityV1::ReadContent,
            ],
        )
        .expect("the trusted control changes another membership");
    let head = stack.seed.a.head().unwrap();
    refuses_what_is_held(
        &stack,
        &held,
        StatusCode::FORBIDDEN,
        "context_revoked",
        "select_context",
    )
    .await;
    ends_with_error(open, &["context_revoked"], Duration::from_secs(8)).await;
    assert_eq!(
        stack.seed.a.head().unwrap(),
        head,
        "no refused call is recorded as a read"
    );

    // The rights of the reader itself are reduced while it holds positions
    // of a new context. An event appended afterwards is never delivered
    // under that context.
    let (held, open) = hold(&stack, "user_a1").await;
    stack
        .seed
        .set_capabilities("user_a1", &[HumanCapabilityV1::ReadMetadata])
        .expect("the trusted control reduces the membership");
    let late = stack
        .seed
        .a
        .append(resource_event(ResourceKindV1::Session, &session, 9900))
        .unwrap()
        .event
        .id;
    let head = stack.seed.a.head().unwrap();
    refuses_what_is_held(
        &stack,
        &held,
        StatusCode::FORBIDDEN,
        "context_revoked",
        "select_context",
    )
    .await;
    ends_with_error(open, &["context_revoked"], Duration::from_secs(8)).await;
    assert_eq!(
        stack.seed.a.head().unwrap(),
        head,
        "no refused call is recorded as a read"
    );

    // The control offers a new context for the reduced grant. Nothing issued
    // under the previous one transfers to it, and the previous one stays refused.
    let (token, fresh) = stack.context("user_a1", "personal").await;
    assert_ne!(fresh, held.context, "the reduced grant is another context");
    let reply = stack.get(&held.older, &token, Some(&fresh)).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.body);
    assert_eq!(reply.body["error"]["code"], "invalid_cursor");
    let reply = stack
        .get(
            &with(RECENT, &[("limit", "5"), ("view", "content")]),
            &token,
            Some(&fresh),
        )
        .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.body);
    assert_eq!(reply.body["error"]["code"], "permission_denied");
    assert!(reply.body.get("items").is_none());
    let page = stack
        .get(&with(RECENT, &HELD_PAGE), &token, Some(&fresh))
        .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(
        ids(&page.body).first(),
        Some(&late),
        "the new context reads the event appended after the change"
    );
    let reply = stack
        .get(&with(RECENT, &HELD_PAGE), &token, Some(&held.context))
        .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.body);
    assert_eq!(reply.body["error"]["retry_action"], "select_context");
    assert!(reply.body.get("items").is_none());
}

#[tokio::test]
async fn a_revoked_credential_refuses_recent_and_the_positions_already_issued() {
    let mut stack = Stack::small().await;
    let (held, open) = hold(&stack, "user_a1").await;
    stack
        .seed
        .revoke_credential("user_a1")
        .expect("the trusted control revokes the credential");
    refuses_what_is_held(
        &stack,
        &held,
        StatusCode::UNAUTHORIZED,
        "identity_revoked",
        "authenticate",
    )
    .await;
    ends_with_error(open, &["identity_revoked"], Duration::from_secs(8)).await;
}

/// A context never outlives the proof it was offered for. A credential that
/// expires in a few seconds ends its context with it, without waiting for
/// the 300-second lifetime: the open stream ends on the expired context or
/// on the expired proof, whichever its poll meets first, and every later
/// call is refused at authentication.
#[tokio::test]
async fn an_expired_proof_refuses_recent_and_the_positions_already_issued() {
    let mut stack = Stack::small().await;
    let life = Duration::from_secs(6);
    let expiry = chrono::Utc::now() + life;
    stack
        .seed
        .set_credential_expiry(
            "user_a1",
            &expiry.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )
        .expect("the trusted control bounds the credential");
    let (held, open) = hold(&stack, "user_a1").await;
    assert!(
        chrono::Utc::now() < expiry,
        "precondition: the positions were issued before the expiry"
    );
    ends_with_error(
        open,
        &["context_expired", "credential_expired"],
        life + Duration::from_secs(4),
    )
    .await;
    let remaining = (expiry - chrono::Utc::now()).to_std().unwrap_or_default();
    tokio::time::sleep(remaining + Duration::from_millis(300)).await;
    refuses_what_is_held(
        &stack,
        &held,
        StatusCode::UNAUTHORIZED,
        "credential_expired",
        "authenticate",
    )
    .await;
}

/// The same after the real lifetime of a context, which nothing outside the
/// gateway can shorten: the test waits for it. Run it explicitly with
/// `--ignored`; left out, it is not run.
#[tokio::test]
#[ignore = "waits for the real 300-second lifetime of a context"]
async fn an_expired_context_refuses_recent_and_the_positions_already_issued() {
    let stack = Stack::small().await;
    let issued = tokio::time::Instant::now();
    let (held, open) = hold(&stack, "user_a1").await;
    // The test client bounds every request to 20 seconds: the stream that is
    // observed across the expiry is opened shortly before it.
    drop(open);
    tokio::time::sleep_until(issued + Duration::from_secs(296)).await;
    let open = open_stream(&stack, &held.stream, &held.token, &held.context).await;
    assert_eq!(
        open.status(),
        StatusCode::OK,
        "precondition: the context still stands four seconds before its end"
    );
    ends_with_error(open, &["context_expired"], Duration::from_secs(12)).await;
    refuses_what_is_held(
        &stack,
        &held,
        StatusCode::FORBIDDEN,
        "context_expired",
        "select_context",
    )
    .await;

    // The next identity read forgets the expired context: it is then
    // unknown, still refused, and its cursors do not transfer to the new one.
    let (token, fresh) = stack.context("user_a1", "personal").await;
    assert_ne!(fresh, held.context);
    refuses_what_is_held(
        &stack,
        &held,
        StatusCode::FORBIDDEN,
        "context_invalid",
        "select_context",
    )
    .await;
    let reply = stack.get(&held.older, &token, Some(&fresh)).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.body);
    assert_eq!(reply.body["error"]["code"], "invalid_cursor");
}

#[tokio::test]
async fn dashboard_proxy_admits_only_the_exact_recent_path() {
    let stack = Stack::small().await;
    let (token, context) = stack.context("admin_a", "administration").await;
    let path = with(RECENT, &[("limit", "10"), ("view", "metadata")]);
    let reply = stack.proxied(&path, &token, Some(&context)).await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "the dashboard proxy must admit the additive route: {}",
        reply.body
    );
    assert_eq!(reply.body["order"], "newest_first");
    assert_eq!(reply.body["items"].as_array().unwrap().len(), 10);
    for other in [
        "/api/v1/human/events/recent/x",
        "/api/v1/human/events/recent/",
        "/api/v1/human/events/recently",
        "/api/v1/human/events/RECENT",
    ] {
        let reply = stack.proxied(other, &token, Some(&context)).await;
        assert_eq!(
            reply.status,
            StatusCode::NOT_FOUND,
            "{other} stays outside the closed list"
        );
        assert!(reply.body.get("items").is_none());
    }
}

/// What a gateway that predates the route answers, for the frontend's
/// version handling. Such a gateway routes `/human/events/recent` to the
/// event detail handler, where `recent` is not an event identifier. Any
/// non-identifier path segment reaches that same handler on this gateway.
#[tokio::test]
async fn a_gateway_without_the_route_answers_the_event_not_found_contract() {
    let stack = Stack::small().await;
    let (token, context) = stack.context("admin_a", "administration").await;
    let before = stack.seed.a.head().unwrap();
    let reply = stack
        .get(
            "/api/v1/human/events/recent-on-an-older-gateway?limit=100&view=metadata",
            &token,
            Some(&context),
        )
        .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.body["error"]["code"], "not_found");
    assert_eq!(reply.body["error"]["retry_action"], "review_permissions");
    assert_eq!(stack.seed.a.head().unwrap(), before);
    write_wire(&stack, "recent-absent.json", &reply.raw);
}
