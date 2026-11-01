mod support;

use agentsandbox::{
    api::{GatewayState, gateway_router},
    audit::{EventQuery, Integrity, Store},
    jev::JevClient,
    model::{Event, StoredEvent},
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::{io::ErrorKind, net::TcpListener, sync::Arc, time::Duration};
use tower::ServiceExt;

const CANARY: &str = "Z7Q4M9K2V6R8T3N5P1A0D4H7J9L6C2F8";
const RESPONSE_LIMIT: usize = 16 * 1024;

struct Observations {
    event_id: String,
    history: Vec<StoredEvent>,
    initial_head: i64,
    ingestion_status: StatusCode,
    ingestion_body: Vec<u8>,
    listing_status: StatusCode,
    listing_body: Vec<u8>,
    persisted: Vec<StoredEvent>,
    final_head: i64,
    integrity: Integrity,
}

async fn exchange(app: &Router, request: Request<Body>) -> anyhow::Result<(StatusCode, Vec<u8>)> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let response = app.clone().oneshot(request).await?;
        let status = response.status();
        let body = to_bytes(response.into_body(), RESPONSE_LIMIT).await?;
        Ok((status, body.to_vec()))
    })
    .await?
}

async fn observe(endpoint: &str) -> anyhow::Result<Observations> {
    let directory = support::private_tempdir();
    let store = Arc::new(Store::open(
        &directory.path().join("audit.sqlite"),
        Redactor::new(vec![]),
        100,
    )?);
    let mut historical = Event::new("fixture.history");
    historical.data = json!({"text":"preserved historical entry"});
    store.append(historical)?;
    let query = EventQuery {
        order: Some("asc".into()),
        limit: Some(100),
        ..Default::default()
    };
    let history = store.events(&query)?;
    let initial_head = store.head()?;
    let runner = "fixture-runner-111111111111111111111111111111111111";
    let viewer = "fixture-viewer-222222222222222222222222222222222222";
    let jev = JevClient::new(endpoint, CANARY.into(), "fixture-model".into(), 0.85, true)?;
    let app = gateway_router(GatewayState::new(
        Arc::clone(&store),
        runner.into(),
        viewer.into(),
        Policy::default(),
        Some(jev),
        Redactor::new(vec![]),
    ));
    let mut event = Event::new("fixture.note");
    event.project_id = Some(format!("project:{CANARY}:{CANARY}"));
    event.data = json!({"text":format!("note {CANARY} repeated {CANARY}")});
    let event_id = event.id.clone();
    let (ingestion_status, ingestion_body) = exchange(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/events")
            .header("Authorization", format!("Bearer {runner}"))
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(&event)?))?,
    )
    .await?;
    let (listing_status, listing_body) = exchange(
        &app,
        Request::builder()
            .uri(format!(
                "/api/events?after={initial_head}&order=asc&limit=10"
            ))
            .header("Authorization", format!("Bearer {viewer}"))
            .body(Body::empty())?,
    )
    .await?;
    let persisted = store.events(&query)?;
    let final_head = store.head()?;
    let integrity = store.integrity()?;
    drop(app);
    drop(store);
    drop(directory);
    Ok(Observations {
        event_id,
        history,
        initial_head,
        ingestion_status,
        ingestion_body,
        listing_status,
        listing_body,
        persisted,
        final_head,
        integrity,
    })
}

fn assert_no_canary_fragment(bytes: &[u8]) {
    for fragment in CANARY.as_bytes().windows(4) {
        assert!(
            !bytes
                .windows(fragment.len())
                .any(|window| window == fragment),
            "a known canary fragment survived gateway audit redaction"
        );
    }
}

#[tokio::test]
async fn existing_gateway_constructor_masks_jev_key_in_ingestion_and_original_store() {
    let provider = TcpListener::bind("127.0.0.1:0").expect("loopback fixture listener");
    provider
        .set_nonblocking(true)
        .expect("nonblocking fixture listener");
    let endpoint = format!(
        "http://{}/v1/systemone",
        provider.local_addr().expect("fixture listener address")
    );
    let observed = observe(&endpoint).await;
    let provider_contact = match provider.accept() {
        Ok((connection, _)) => {
            drop(connection);
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(false),
        Err(error) => Err(error),
    };
    drop(provider);

    let observed = observed.expect("collect bounded gateway and audit observations");
    assert!(
        !provider_contact.expect("inspect provider fixture listener"),
        "event ingestion or reading contacted the decision provider"
    );
    assert_eq!(observed.ingestion_status, StatusCode::OK);
    assert_eq!(observed.listing_status, StatusCode::OK);
    let persisted_bytes = serde_json::to_vec(&observed.persisted).unwrap();
    for bytes in [
        observed.ingestion_body.as_slice(),
        observed.listing_body.as_slice(),
        persisted_bytes.as_slice(),
    ] {
        assert_no_canary_fragment(bytes);
    }
    let ingested: StoredEvent = serde_json::from_slice(&observed.ingestion_body).unwrap();
    let listing: Value = serde_json::from_slice(&observed.listing_body).unwrap();
    let listed: Vec<StoredEvent> = serde_json::from_value(listing["items"].clone()).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(ingested.event.id, observed.event_id);
    assert_eq!(ingested.seq, observed.initial_head + 1);
    assert_eq!(observed.final_head, ingested.seq);
    assert_eq!(
        ingested.event.project_id.as_deref(),
        Some("project:[REDACTED]:[REDACTED]")
    );
    assert_eq!(
        ingested.event.data["text"],
        "note [REDACTED] repeated [REDACTED]"
    );
    assert_eq!(
        serde_json::to_value(&listed[0]).unwrap(),
        serde_json::to_value(&ingested).unwrap()
    );
    assert_eq!(observed.persisted.len(), observed.history.len() + 1);
    assert_eq!(
        serde_json::to_value(&observed.persisted[..observed.history.len()]).unwrap(),
        serde_json::to_value(&observed.history).unwrap()
    );
    assert_eq!(
        serde_json::to_value(observed.persisted.last().unwrap()).unwrap(),
        serde_json::to_value(&ingested).unwrap()
    );
    assert!(observed.integrity.valid);
    assert_eq!(
        observed.integrity.event_count,
        observed.persisted.len() as u64
    );
    assert_eq!(observed.integrity.head_hash, ingested.hash);
}
