mod support;

use agentsandbox::{
    audit::{EventQuery, Store},
    model::Event,
    redaction::Redactor,
};
use std::sync::Arc;

#[test]
fn eight_concurrent_sources_keep_order_integrity_and_filters() {
    let directory = support::private_tempdir();
    let store = Arc::new(
        Store::open(
            &directory.path().join("load.sqlite"),
            Redactor::new(vec![]),
            2000,
        )
        .unwrap(),
    );
    let start = std::time::Instant::now();
    let workers: Vec<_> = (0..8)
        .map(|worker| {
            let store = store.clone();
            std::thread::spawn(move || {
                for index in 0..125 {
                    let mut event = Event::new("test.load");
                    event.agent_id = Some(format!("fixture-agent-{worker}"));
                    event.project_id = Some("load-fixture".into());
                    event.data = serde_json::json!({"synthetic_test":true,"index":index});
                    store.append(event).unwrap();
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let integrity = store.integrity().unwrap();
    assert!(integrity.valid);
    assert_eq!(integrity.event_count, 1001); // 1000 synthetic events + real control genesis
    let records = store
        .events(&EventQuery {
            agent_id: Some("fixture-agent-3".into()),
            limit: Some(1000),
            order: Some("asc".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(records.len(), 125);
    assert!(records.windows(2).all(|w| w[0].seq < w[1].seq));
    let page = store
        .events(&EventQuery {
            after: Some(990),
            limit: Some(5),
            order: Some("asc".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.len(), 5);
    assert_eq!(page[0].seq, 991);
    eprintln!(
        "Synthetic ingestion: 1000 events / 8 concurrent sources in {:?}",
        start.elapsed()
    );
}
