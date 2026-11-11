//! Unified supervision reads over the real gateway and dashboard routers and
//! private test SQLite stores. The data set is synthetic (see
//! `support/supervision_seed.rs`); no runtime, broker process or model runs,
//! so these tests prove contracts and isolation, never a worker capacity.
#[path = "support/supervision_seed.rs"]
mod supervision_seed;
mod support;

use agentsandbox::identity::{HUMAN_MAX_PAGE_BYTES, HumanCapabilityV1};
use reqwest::StatusCode;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
};
use supervision_seed::{Seed, SeedOptions, Served, post_heartbeat, serve};

const FLEET: &str = "/api/v1/human/federation/workers?worker_limit=100&broker_limit=100";

struct Stack {
    seed: Seed,
    served: Served,
    _files: tempfile::TempDir,
}

struct Reply {
    status: StatusCode,
    body: Value,
    raw: Vec<u8>,
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
    fn token(&self, account: &str) -> String {
        self.seed.accounts[account].token.clone()
    }
    /// Every read goes through the dashboard proxy, like the browser.
    async fn get(&self, path: &str, token: &str, context: Option<&str>) -> Reply {
        let mut request = self
            .served
            .client
            .get(format!("{}{path}", self.served.dashboard))
            .bearer_auth(token);
        if let Some(context) = context {
            request = request.header("X-AiGuard-Context", context);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let raw = response.bytes().await.unwrap().to_vec();
        Reply {
            status,
            body: serde_json::from_slice(&raw).unwrap_or(Value::Null),
            raw,
        }
    }
    async fn contexts(&self, token: &str) -> Vec<Value> {
        let reply = self.get("/api/v1/human/identity", token, None).await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "the local human authenticates"
        );
        reply.body["contexts"].as_array().unwrap().clone()
    }
    async fn context(&self, token: &str, mode: &str) -> String {
        self.contexts(token)
            .await
            .iter()
            .find(|context| context["mode"] == mode)
            .expect("the control offers the requested context")["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

/// Optional exact-response handoff to `front/tests/supervision-wire.test.mjs`.
/// Without the private QA directory nothing is written.
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

fn ids(page: &Value, leg: &str, field: &str) -> Vec<String> {
    page[leg]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item[field].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn admin_reads_one_hundred_workers_and_all_brokers_in_one_composition() {
    let mut stack = Stack::start(SeedOptions {
        workers: 120,
        events: 60,
        ..SeedOptions::default()
    })
    .await;
    post_heartbeat(&mut stack.seed, &stack.served)
        .await
        .expect("the authenticated broker heartbeat is accepted");
    let token = stack.token("admin_a");
    let context = stack.context(&token, "administration").await;

    let first = stack.get(FLEET, &token, Some(&context)).await;
    assert_eq!(
        first.status,
        StatusCode::OK,
        "one composed read is authorized"
    );
    assert!(first.raw.len() <= 2 * HUMAN_MAX_PAGE_BYTES + 4096);
    write_wire(&stack, "federation-first.json", &first.raw);
    let page = &first.body;
    assert_eq!(page["source"], "tenant_federation_registry");
    assert_eq!(page["coverage"], "partial");
    assert_eq!(page["runtime_inventory"], "unknown");
    assert!(page.get("total").is_none(), "no tenant total is published");

    let mut seen: BTreeSet<String> = ids(page, "workers", "worker_id").into_iter().collect();
    let first_count = seen.len();
    assert!(
        (1..=100).contains(&first_count),
        "one page carries at most one hundred rows"
    );
    assert_eq!(
        page["workers"]["has_more"],
        page["workers"]["next_cursor"].is_string()
    );
    assert_eq!(
        page["workers"]["has_more"], true,
        "120 registry rows cannot fit one bounded page"
    );

    let brokers = page["brokers"]["items"].as_array().unwrap();
    assert_eq!(brokers.len(), 3, "every registered broker of A is listed");
    assert_eq!(page["brokers"]["has_more"], false);
    for (name, state) in [
        ("current", "current"),
        ("revoked", "revoked"),
        ("unseen", "unseen"),
    ] {
        let expected = &stack.seed.brokers[name];
        let row = brokers
            .iter()
            .find(|row| row["broker_id"] == expected.broker_id)
            .unwrap_or_else(|| panic!("broker {name} is listed"));
        assert_eq!(row["state"], state, "lease state of the {name} broker");
    }

    // Later pages keep the same limits; the cursor is bound to them.
    let mut cursor = page["workers"]["next_cursor"].as_str().map(str::to_owned);
    let mut pages = 1;
    let mut rows = page["workers"]["items"].as_array().unwrap().clone();
    while let Some(next) = cursor {
        assert!(pages < 5, "the traversal ends within the expected window");
        let mut url = reqwest::Url::parse(&format!("http://localhost{FLEET}")).unwrap();
        url.query_pairs_mut().append_pair("worker_cursor", &next);
        let path = format!("{}?{}", url.path(), url.query().unwrap());
        let reply = stack.get(&path, &token, Some(&context)).await;
        assert_eq!(reply.status, StatusCode::OK);
        if pages == 1 {
            write_wire(&stack, "federation-next.json", &reply.raw);
        }
        for id in ids(&reply.body, "workers", "worker_id") {
            assert!(seen.insert(id), "no row repeats across pages");
        }
        rows.extend(reply.body["workers"]["items"].as_array().unwrap().clone());
        cursor = reply.body["workers"]["next_cursor"]
            .as_str()
            .map(str::to_owned);
        pages += 1;
    }
    let expected: BTreeSet<String> = stack.seed.worker_ids_a.iter().cloned().collect();
    assert_eq!(
        seen, expected,
        "the traversal returns exactly the rows of A"
    );

    // The window shows registry variety without any inferred state.
    let states: BTreeSet<&str> = rows
        .iter()
        .map(|row| row["state"].as_str().unwrap())
        .collect();
    for state in ["reserved", "running", "terminal", "in_doubt", "cancelled"] {
        assert!(
            states.contains(state),
            "state {state} is present in the seed"
        );
    }
    assert!(rows.iter().any(|row| row["last_observation"].is_null()));
    assert!(
        rows.iter()
            .any(|row| row["last_observation"]["oom_killed"] == true)
    );
    let association = |status: &str| {
        rows.iter()
            .filter(|row| row["launch_broker"]["status"] == status)
            .count()
    };
    assert!(association("registered") > 0);
    assert!(
        association("unregistered") > 0,
        "an authority without broker grant stays visible as unregistered"
    );
    assert!(rows.iter().any(|row| {
        row["launch_broker"]["broker"]["broker_id"] == stack.seed.brokers["unseen"].broker_id
            && row["launch_broker"]["runtime_matches"] == false
    }));

    // Resources of the same data set, for the worker -> session correlation.
    let resources = stack
        .get("/api/v1/human/resources?limit=100", &token, Some(&context))
        .await;
    assert_eq!(resources.status, StatusCode::OK);
    write_wire(&stack, "resources-fleet.json", &resources.raw);
    let granted: BTreeSet<&str> = resources.body["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["resource"]["kind"] == "session")
        .map(|item| item["resource"]["resource_id"].as_str().unwrap())
        .collect();
    assert_eq!(granted.len(), stack.seed.granted_sessions_a.len());
    assert!(
        rows.iter()
            .any(|row| granted.contains(row["session_id"].as_str().unwrap())),
        "a registry row declares a session that is also a granted resource"
    );

    // Tenant B never appears in a read authorized for A.
    let encoded = format!("{}{}", first.body, Value::Array(rows));
    for foreign in stack
        .seed
        .worker_ids_b
        .iter()
        .chain(&stack.seed.broker_ids_b)
        .chain([&stack.seed.session_b])
    {
        assert!(!encoded.contains(foreign.as_str()));
    }
    assert!(stack.seed.a.integrity().unwrap().valid);
}

#[tokio::test]
async fn session_events_follow_resource_grants_for_admin_owner_and_stranger() {
    let stack = Stack::start(SeedOptions {
        workers: 12,
        events: 90,
        ..SeedOptions::default()
    })
    .await;
    let owned = stack.seed.session_owned_by_user_a1.clone();
    let shared = stack.seed.session_shared_with_user_a2.clone();
    let events = |session: &str, view: &str| {
        format!(
            "/api/v1/human/events?limit=25&view={view}&resource_kind=session&resource_id={session}"
        )
    };

    let admin = stack.token("admin_a");
    let admin_context = stack.context(&admin, "administration").await;
    let resources = stack
        .get(
            "/api/v1/human/resources?limit=100",
            &admin,
            Some(&admin_context),
        )
        .await;
    assert_eq!(resources.status, StatusCode::OK);
    write_wire(&stack, "resources-admin.json", &resources.raw);
    let listed: BTreeSet<(String, String)> = resources.body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["resource"]["kind"].as_str().unwrap().to_owned(),
                item["resource"]["resource_id"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert!(listed.contains(&("session".into(), owned.clone())));
    assert!(listed.contains(&("agent".into(), "agent-planner".into())));
    assert!(listed.contains(&("sandbox".into(), "sandbox-build".into())));
    assert!(listed.contains(&("project".into(), "project-atlas".into())));
    assert!(
        !listed
            .iter()
            .any(|(_, id)| id == "sandbox-retired" || id == "project-elsewhere"),
        "inactive and foreign-organization grants are not listed"
    );

    let reply = stack
        .get(&events(&owned, "metadata"), &admin, Some(&admin_context))
        .await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "administration reads the session"
    );
    write_wire(&stack, "events-session.json", &reply.raw);
    let items = reply.body["items"].as_array().unwrap();
    assert!(
        !items.is_empty(),
        "the session carries real appended events"
    );
    assert!(items.iter().all(|item| {
        item["metadata"]["resource"]["kind"] == "session"
            && item["metadata"]["resource"]["resource_id"] == owned
            && item["content"].is_null()
    }));

    let owner = stack.token("user_a1");
    let owner_context = stack.context(&owner, "personal").await;
    let reply = stack
        .get(&events(&owned, "content"), &owner, Some(&owner_context))
        .await;
    assert_eq!(reply.status, StatusCode::OK, "the owner reads its content");
    assert!(reply.body["items"][0]["content"].is_object());

    let stranger = stack.token("user_a2");
    let stranger_context = stack.context(&stranger, "personal").await;
    let before = stack.seed.a.head().unwrap();
    let reply = stack
        .get(
            &events(&owned, "metadata"),
            &stranger,
            Some(&stranger_context),
        )
        .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.body["error"]["code"], "not_found");
    let reply = stack
        .get(
            &events(&shared, "metadata"),
            &stranger,
            Some(&stranger_context),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK, "a metadata share is honoured");
    let reply = stack
        .get(
            &events(&shared, "content"),
            &stranger,
            Some(&stranger_context),
        )
        .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(reply.body["error"]["code"], "permission_denied");
    assert!(stack.seed.a.head().unwrap() > before);

    // A registry-only reader holds no resource or event authority.
    let registry = stack.token("workers_only_a");
    let registry_context = stack.context(&registry, "administration").await;
    let reply = stack
        .get(
            "/api/v1/human/resources?limit=100",
            &registry,
            Some(&registry_context),
        )
        .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(reply.body["error"]["code"], "permission_denied");
    let reply = stack
        .get(
            "/api/v1/human/workers?limit=100",
            &registry,
            Some(&registry_context),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body["items"].as_array().unwrap().len(), 12);
}

#[tokio::test]
async fn tenants_are_isolated_and_an_empty_registry_is_a_successful_read() {
    let stack = Stack::start(SeedOptions {
        workers: 12,
        events: 12,
        ..SeedOptions::default()
    })
    .await;
    let admin_a = stack.token("admin_a");
    let admin_b = stack.token("admin_b");
    let context_a = stack.context(&admin_a, "administration").await;
    let context_b = stack.context(&admin_b, "administration").await;

    let b = stack.get(FLEET, &admin_b, Some(&context_b)).await;
    assert_eq!(b.status, StatusCode::OK);
    assert_eq!(
        ids(&b.body, "workers", "worker_id"),
        stack.seed.worker_ids_b
    );
    assert_eq!(
        ids(&b.body, "brokers", "broker_id"),
        stack.seed.broker_ids_b
    );
    let encoded = b.body.to_string();
    for foreign in stack
        .seed
        .worker_ids_a
        .iter()
        .chain(stack.seed.brokers.values().map(|broker| &broker.broker_id))
    {
        assert!(
            !encoded.contains(foreign.as_str()),
            "B never receives a row of A"
        );
    }

    // A context issued to another human and tenant selects nothing.
    let heads = (stack.seed.a.head().unwrap(), stack.seed.b.head().unwrap());
    let crossed = stack.get(FLEET, &admin_a, Some(&context_b)).await;
    assert_eq!(crossed.status, StatusCode::FORBIDDEN);
    let crossed = stack.get(FLEET, &admin_b, Some(&context_a)).await;
    assert_eq!(crossed.status, StatusCode::FORBIDDEN);
    assert!(crossed.body.get("workers").is_none());
    // Each refusal leaves only the identity receipt of the dashboard proxy
    // preflight in the caller's own tenant: no registry read is recorded.
    assert_eq!(
        (stack.seed.a.head().unwrap(), stack.seed.b.head().unwrap()),
        (heads.0 + 1, heads.1 + 1),
        "a refused cross-tenant selection records no registry read"
    );

    let admin_c = stack.token("admin_c");
    let context_c = stack.context(&admin_c, "administration").await;
    let empty = stack.get(FLEET, &admin_c, Some(&context_c)).await;
    assert_eq!(
        empty.status,
        StatusCode::OK,
        "an empty registry is a success"
    );
    write_wire(&stack, "federation-empty.json", &empty.raw);
    for leg in ["workers", "brokers"] {
        assert_eq!(empty.body[leg]["items"].as_array().unwrap().len(), 0);
        assert_eq!(empty.body[leg]["has_more"], false);
        assert!(empty.body[leg]["read_receipt"]["audit_event_id"].is_string());
    }
    assert_eq!(empty.body["runtime_inventory"], "unknown");
}

#[tokio::test]
async fn reduced_capabilities_revoke_the_context_and_refuse_the_registry() {
    let mut stack = Stack::start(SeedOptions {
        workers: 12,
        events: 12,
        ..SeedOptions::default()
    })
    .await;
    let token = stack.token("admin_a");
    let context = stack.context(&token, "administration").await;
    assert_eq!(
        stack.get(FLEET, &token, Some(&context)).await.status,
        StatusCode::OK
    );

    stack
        .seed
        .set_capabilities(
            "admin_a",
            &[
                HumanCapabilityV1::ReadMetadata,
                HumanCapabilityV1::ReadTenantMetadata,
                HumanCapabilityV1::ReadBrokerRegistry,
            ],
        )
        .expect("the trusted control reduces the membership");

    let refused = stack.get(FLEET, &token, Some(&context)).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert!(
        refused.body.get("workers").is_none(),
        "no row follows a refusal"
    );

    let contexts = stack.contexts(&token).await;
    let administration = contexts
        .iter()
        .find(|context| context["mode"] == "administration")
        .expect("the reduced membership still administers brokers");
    let capabilities: BTreeSet<&str> = administration["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|capability| capability.as_str().unwrap())
        .collect();
    assert!(!capabilities.contains("read_worker_registry"));
    assert!(capabilities.contains("read_broker_registry"));
    let fresh = administration["id"].as_str().unwrap();
    assert_ne!(fresh, context, "the reduced grant is a different context");

    let reply = stack.get(FLEET, &token, Some(fresh)).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    let reply = stack
        .get("/api/v1/human/workers?limit=100", &token, Some(fresh))
        .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(reply.body["error"]["code"], "permission_denied");
    let reply = stack
        .get("/api/v1/human/brokers?limit=100", &token, Some(fresh))
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body["items"].as_array().unwrap().len(), 3);
}
