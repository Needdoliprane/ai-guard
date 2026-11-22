//! Owned-source human rights marker through the operator CLI: a real legacy
//! Store13 union adopted by `human-sources prepare/apply`, then the in-place
//! `human-sources human-rights` maintenance and a real gateway restart.
//! Synthetic private data only; no runtime engine, IdP or model is contacted.
#[allow(dead_code)]
#[path = "support/human_sources_bootstrap.rs"]
mod support;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::Duration,
};
use support::Fixture;

const MARKER_KIND: &str = "control.identity.human_rights.genesis";

fn successful(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn adopted() -> Fixture {
    let f = Fixture::new(false);
    successful(&f.prepare());
    successful(&f.step("apply", true));
    f
}

fn human_rights(f: &Fixture, step: &str, tenant: &str, dir: &Path, stopped: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .env_clear()
        .args(["human-sources", "human-rights", step])
        .arg("--human-sources-activation")
        .arg(f.preparation.join("activation.json"))
        .args(["--tenant-id", tenant, "--maintenance-dir"])
        .arg(dir);
    if stopped {
        command.arg("--confirm-control-stopped");
    }
    command.stdin(Stdio::null()).output().unwrap()
}

fn refusal_state(output: &Output) -> String {
    assert!(!output.status.success(), "the step must be refused");
    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr
        .split(';')
        .find_map(|part| part.trim().strip_prefix("state="))
        .unwrap_or_else(|| panic!("closed refusal expected: {stderr}"))
        .trim()
        .to_owned()
}

struct SourceState {
    format: i64,
    head: (i64, String),
    markers: Vec<(i64, String)>,
}

fn state(path: &Path) -> SourceState {
    let db = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let format = db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    let head = db
        .query_row(
            "SELECT seq,hash FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let markers = db
        .prepare("SELECT seq,hash FROM events WHERE kind=?1 ORDER BY seq")
        .unwrap()
        .query_map([MARKER_KIND], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    SourceState {
        format,
        head,
        markers,
    }
}

fn names(path: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn report(output: &Output) -> Value {
    successful(output);
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn install_appends_the_marker_once_refuses_again_and_resume_is_idempotent() {
    let f = adopted();
    let a = f.sources[0].clone();
    let b = f.sources[1].clone();
    let before = state(&a);
    let neighbor = state(&b);
    assert_eq!(before.format, 14);
    assert!(before.markers.is_empty());

    // Without the stopped-control attestation nothing is created.
    let first = f.root.join("human-rights-a");
    let refused = human_rights(&f, "install", "tenant-a", &first, false);
    assert!(!refused.status.success());
    assert!(!first.exists());
    // Unknown tenant, protected location and absent resume directory.
    assert_eq!(
        refusal_state(&human_rights(&f, "install", "tenant-z", &first, true)),
        "human_rights_source_unknown"
    );
    assert!(!first.exists());
    for protected in [f.preparation.as_path(), b.parent().unwrap()] {
        let inside = protected.join("human-rights");
        assert_eq!(
            refusal_state(&human_rights(&f, "install", "tenant-a", &inside, true)),
            "human_rights_journal_conflict"
        );
        assert!(!inside.exists());
        assert_eq!(
            refusal_state(&human_rights(&f, "resume", "tenant-a", protected, true)),
            "human_rights_journal_conflict"
        );
    }
    assert_eq!(
        refusal_state(&human_rights(&f, "resume", "tenant-a", &first, true)),
        "human_rights_journal_conflict"
    );
    assert_eq!(state(&a).head, before.head);

    let installed = report(&human_rights(&f, "install", "tenant-a", &first, true));
    assert_eq!(installed["state"], "installed");
    assert_eq!(installed["step"], "in_place_identity_markers");
    assert_eq!(installed["tenant_id"], "tenant-a");
    assert_eq!(installed["store_format"], 14);
    assert_eq!(installed["readiness"], "not_checked");
    let markers = installed["markers"].as_array().unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0]["marker"], "human_rights_v2");
    let after = state(&a);
    assert_eq!(after.format, 14);
    assert_eq!(after.markers.len(), 1);
    assert_eq!(after.markers[0].0, before.head.0 + 1);
    assert_eq!(markers[0]["seq"], after.markers[0].0);
    assert_eq!(markers[0]["hash"], after.markers[0].1.as_str());
    // The neighbor tenant is untouched.
    let untouched = state(&b);
    assert_eq!(untouched.head, neighbor.head);
    assert!(untouched.markers.is_empty());
    // Plan, journal records and the verified pre-marker backup are retained.
    assert_eq!(names(&first), ["backups", "journal", "plan.json"]);
    let plan: Value = serde_json::from_slice(&std::fs::read(first.join("plan.json")).unwrap())
        .expect("published maintenance plan");
    assert_eq!(plan["selected_source_id"], installed["source_id"]);
    assert_eq!(
        names(&first.join("journal")),
        [
            "identity-markers-backup-v1",
            "identity-markers-confirmed-v1",
            "identity-markers-intent-v1"
        ]
    );
    let backup = PathBuf::from(installed["backup_path"].as_str().unwrap());
    assert_eq!(backup, first.join("backups/source.sqlite"));
    let backed = state(&backup);
    assert_eq!(backed.head, before.head);
    assert!(backed.markers.is_empty());

    // A second maintenance finds nothing pending: no record, no marker.
    let second = f.root.join("human-rights-a-again");
    assert_eq!(
        refusal_state(&human_rights(&f, "install", "tenant-a", &second, true)),
        "human_rights_already_installed"
    );
    assert!(names(&second.join("journal")).is_empty());
    assert!(names(&second.join("backups")).is_empty());
    assert_eq!(state(&a).head, after.head);

    // Resuming the completed maintenance only re-confirms it.
    let confirmed = report(&human_rights(&f, "resume", "tenant-a", &first, true));
    assert_eq!(confirmed["state"], "confirmed");
    assert_eq!(confirmed["markers"], installed["markers"]);
    assert_eq!(state(&a).markers, after.markers);
    assert_eq!(state(&a).head, after.head);

    // The activation files remain valid for the gateway.
    let check = f.step("check", false);
    successful(&check);
    assert!(String::from_utf8_lossy(&check.stdout).starts_with("activation_files_valid"));
}

fn spawn_gateway(f: &Fixture) -> (tokio::process::Child, String) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .env_clear()
        .args(["gateway", "--human-sources-activation"])
        .arg(f.preparation.join("activation.json"))
        .arg("--listen")
        .arg(address.to_string())
        .args(["--jev-backend", "rust-rules", "--policy"])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/policies/default.json"
        ))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    (child, format!("http://{address}"))
}

async fn read_identity_and_resources(
    f: &Fixture,
    child: &mut tokio::process::Child,
    origin: &str,
) -> String {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    // Generous harness wait under host load; the product deadline is unchanged.
    let identity = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "gateway exited before a real human read"
            );
            if let Ok(response) = client
                .get(format!("{origin}/api/v1/human/identity"))
                .bearer_auth(&f.human_bearer)
                .send()
                .await
                && response.status().is_success()
            {
                break response.json::<Value>().await.unwrap();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let contexts = identity["contexts"].as_array().unwrap();
    assert!(contexts.iter().all(|c| c["tenant_id"] == "tenant-a"));
    let response = client
        .get(format!("{origin}/api/v1/human/resources"))
        .bearer_auth(&f.human_bearer)
        .header("x-aiguard-context", contexts[0]["id"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let resources: Value = response.json().await.unwrap();
    resources["read_receipt"]["audit_event_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn stop(mut child: tokio::process::Child) {
    // SAFETY: child.id() is this test's still-owned gateway; SIGINT requests
    // its normal registered shutdown and no other PID is used.
    unsafe {
        libc::kill(child.id().unwrap() as i32, libc::SIGINT);
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(60), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn a_running_gateway_refuses_the_maintenance_and_restarts_with_the_marker() {
    let f = adopted();
    let a = f.sources[0].clone();
    let (mut child, origin) = spawn_gateway(&f);
    read_identity_and_resources(&f, &mut child, &origin).await;
    let running = state(&a);
    // The gateway holds the owner locks: the guard refuses, SQLite untouched.
    let dir = f.root.join("human-rights-while-running");
    assert_eq!(
        refusal_state(&human_rights(&f, "install", "tenant-a", &dir, true)),
        "human_rights_owner_busy"
    );
    assert_eq!(state(&a).head, running.head);
    assert!(names(&dir.join("journal")).is_empty());
    stop(child).await;

    // An interruption before publishing the intent can leave a partial draft.
    // Resume reconstructs the intent from the admitted plan and source and
    // preserves the unpublished draft.
    let journal = dir.join("journal");
    let draft = journal.join(format!("identity-markers-stage-{}", uuid::Uuid::new_v4()));
    let partial_record = b"{\"version\":1";
    support::private_dir(&draft);
    support::private_file(&draft.join("record-v1.json"), partial_record);
    let plan: Value =
        serde_json::from_slice(&std::fs::read(dir.join("plan.json")).unwrap()).unwrap();
    let stopped = state(&a);
    let resumed = report(&human_rights(&f, "resume", "tenant-a", &dir, true));
    assert_eq!(resumed["state"], "installed");
    assert_eq!(
        std::fs::read(draft.join("record-v1.json")).unwrap(),
        partial_record
    );
    let published = journal.join("identity-markers-intent-v1/record-v1.json");
    let intent: Value = serde_json::from_slice(&std::fs::read(published).unwrap()).unwrap();
    assert_eq!(intent["migration_id"], plan["migration_id"]);
    let after = state(&a);
    assert_eq!(after.markers.len(), 1);
    assert_eq!(after.markers[0].0, stopped.head.0 + 1);

    // A real owner startup replays the marker and still serves the owned
    // reads with a durable receipt recorded after it.
    let (mut child, origin) = spawn_gateway(&f);
    let event_id = read_identity_and_resources(&f, &mut child, &origin).await;
    stop(child).await;
    let db = rusqlite::Connection::open_with_flags(
        &a,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let receipt_seq: i64 = db
        .query_row("SELECT seq FROM events WHERE id=?1", [&event_id], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(receipt_seq > after.markers[0].0);
    assert_eq!(state(&a).markers, after.markers);
}
