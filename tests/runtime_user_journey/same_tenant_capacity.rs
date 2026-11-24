//! Same-tenant pilot for the G10 capacity campaign.
use super::*;
use chrono::{DateTime, FixedOffset};

const PILOT_WORKERS: u32 = 3;

#[tokio::test]
#[ignore = "requires explicit local Docker and cached QA image; starts three same-tenant sleepers"]
async fn real_docker_same_tenant_capacity_pilot() -> Result<()> {
    ensure!(
        std::env::var("AGENTSANDBOX_TEST_CAPACITY_PILOT").as_deref() == Ok("1"),
        "set AGENTSANDBOX_TEST_CAPACITY_PILOT=1 explicitly"
    );
    let selection = RuntimeSelection::read_docker()?;
    selection.preflight().await?;
    // Keep the private checkpoints and volume bindings for targeted recovery.
    let root = support::private_tempdir().keep().canonicalize()?;
    eprintln!("Retained capacity fixture {}", root.display());
    let jobs = ["A1", "A2", "A3"].map(|label| ParallelJob {
        label,
        tenant: "tenant-a",
        seconds: "35",
        root: root.join(label),
        marker: Uuid::new_v4().to_string(),
    });
    let first_config = jobs[0].config();
    jobs[0].prepare(None).await?;
    for job in &jobs[1..] {
        job.prepare(Some(&first_config)).await?;
    }
    let registry = root.join("tenants.json");
    fs::write(
        &registry,
        serde_json::to_vec(&json!({"version":1,"tenants":[{
            "tenant_id":jobs[0].tenant,
            "data":root.join("tenant-a.sqlite"),
            "runner_token_file":first_config.join("secrets/runner"),
            "viewer_token_file":first_config.join("secrets/viewer"),
            "worker_quota":{
                "version":1,"max_active_workers":PILOT_WORKERS,
                "max_cpu_millis":u64::from(PILOT_WORKERS)*WORKER_CPU,
                "max_memory_bytes":u64::from(PILOT_WORKERS)*WORKER_MEMORY,
                "max_timeout_seconds":60
            }
        }]}))?,
    )?;
    let policy_path = root.join("policy.json");
    let policy = Policy {
        allowed_argv: vec![jobs[0].argv()],
        ..Policy::default()
    };
    fs::write(&policy_path, serde_json::to_vec(&policy)?)?;
    let mut gateway = Gateway::start_tenants(&registry, &policy_path).await?;
    let result: Result<Value> = async {
        let token = read_token(&first_config.join("secrets/viewer"))?;
        let viewer = AuditClient::new(&gateway.origin, &token)?;
        ensure!(
            viewer.get_json("api/v1/identity").await?
                == json!({"tenant_id":jobs[0].tenant,"role":"viewer"}),
            "unexpected capacity fixture viewer"
        );
        check_quota(&viewer, PILOT_WORKERS, 0).await?;
        ensure!(
            current_workers(&viewer, PILOT_WORKERS as usize)
                .await?
                .is_empty(),
            "capacity fixture must start with an empty inventory"
        );
        // A failed observation must allow all owned CLI runs to complete cleanup.
        let (first, second, third, peak) = tokio::join!(
            jobs[0].run(&gateway, &selection),
            jobs[1].run(&gateway, &selection),
            jobs[2].run(&gateway, &selection),
            timeout(Duration::from_secs(45), observe_peak(&viewer, &selection)),
        );
        let runs = [first?, second?, third?];
        let peak = peak.context("capacity peak observation timed out")??;
        check_quota(&viewer, PILOT_WORKERS, 0).await?;
        let records = current_workers(&viewer, PILOT_WORKERS as usize).await?;
        ensure!(
            records.len() == runs.len(),
            "terminal inventory is incomplete"
        );
        let mut intervals = Vec::new();
        for fact in &peak {
            let worker = records
                .iter()
                .find(|worker| worker.attempt.container_id.as_deref() == Some(&fact.id))
                .context("observed CID absent from terminal inventory")?;
            let run = runs
                .iter()
                .find(|run| run.session_id.to_string() == fact.session)
                .context("observed session absent from CLI results")?;
            let terminal = worker
                .last_observation
                .as_ref()
                .context("missing terminal inspection")?;
            let started = terminal
                .started_at
                .as_deref()
                .context("missing terminal StartedAt")?;
            let finished = terminal
                .finished_at
                .as_deref()
                .context("missing terminal FinishedAt")?;
            ensure!(
                worker.state == WorkerStateV1::Terminal
                    && !worker.quota_held
                    && worker.cleanup_removed == Some(true)
                    && worker.launch.session_id == fact.session
                    && worker.attempt.volume_name == run.volume
                    && terminal.inspection.container_id == fact.id
                    && terminal.running == Some(false)
                    && terminal.exit_code == Some(0)
                    && terminal.oom_killed == Some(false)
                    && terminal.restart_count == Some(0)
                    && same_started_at(started, &fact.started_at),
                "terminal state differs from the observed sleeper"
            );
            intervals.push((
                DateTime::parse_from_rfc3339(started)?,
                DateTime::parse_from_rfc3339(finished)?,
            ));
            require_removed(&selection, &fact.id).await?;
        }
        let (start, end) =
            common_window(&intervals).context("no common native running interval")?;
        ensure!(
            viewer.get_json("api/v1/integrity").await?["valid"] == true,
            "invalid fixture audit"
        );
        Ok(json!({
            "status":"PASS","scope":"same_tenant_capacity_pilot","observed":peak,
            "held_at_peak":PILOT_WORKERS,"held_after":0,
            "overlap_start":start.to_rfc3339(),"overlap_end":end.to_rfc3339(),
            "worker_containers_removed":true,"workspaces_retained":true
        }))
    }
    .await;
    let closed = timeout(Duration::from_secs(5), gateway.child.kill()).await;
    let report = result?;
    closed??;
    eprintln!("{report}");
    Ok(())
}

async fn observe_peak(
    viewer: &AuditClient,
    selection: &RuntimeSelection,
) -> Result<Vec<LiveDockerSnapshot>> {
    loop {
        let workers = current_workers(viewer, PILOT_WORKERS as usize).await?;
        if workers.len() == PILOT_WORKERS as usize
            && workers
                .iter()
                .all(|worker| worker.state == WorkerStateV1::Running && worker.quota_held)
        {
            for worker in &workers {
                let inspection = &worker
                    .last_observation
                    .as_ref()
                    .context("missing running inspection")?
                    .inspection;
                ensure!(
                    u64::from(inspection.cpu_millis) == WORKER_CPU
                        && inspection.memory_bytes == WORKER_MEMORY
                        && inspection.pids_limit == 256
                        && inspection.network == NetworkV1::None
                        && inspection.security_preset == SecurityPresetV1::FixedSandboxV2
                        && inspection.runtime_kind == RuntimeKindV1::Docker
                        && inspection.spec_hash == worker.spec_hash,
                    "running inspection differs from the reserved confined profile"
                );
            }
            check_quota(viewer, PILOT_WORKERS, PILOT_WORKERS).await?;
            let live = workers
                .iter()
                .map(LiveWorker::read)
                .collect::<Result<Vec<_>>>()?;
            let facts = selection.inspect_live_workers(&live).await?;
            check_quota(viewer, PILOT_WORKERS, PILOT_WORKERS).await?;
            return Ok(facts);
        }
        sleep(PARALLEL_POLL).await;
    }
}

async fn require_removed(selection: &RuntimeSelection, cid: &str) -> Result<()> {
    let mut command = Command::new(&selection.program);
    command
        .args([
            "--host",
            &selection.endpoint,
            "container",
            "ls",
            "--all",
            "--quiet",
            "--no-trunc",
            "--filter",
        ])
        .arg(format!("id={cid}"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let matches = checked_output(
        command,
        "capacity cleanup inventory",
        Duration::from_secs(5),
    )
    .await?;
    ensure!(
        matches.is_empty(),
        "fixture CID remains on the selected Docker endpoint"
    );
    Ok(())
}

fn common_window(
    intervals: &[(DateTime<FixedOffset>, DateTime<FixedOffset>)],
) -> Option<(DateTime<FixedOffset>, DateTime<FixedOffset>)> {
    let start = intervals.iter().map(|(start, _)| *start).max()?;
    let end = intervals.iter().map(|(_, end)| *end).min()?;
    (start < end).then_some((start, end))
}

#[test]
fn capacity_window_requires_a_strict_overlap_across_all_hundred_intervals() {
    let instant = |offset| DateTime::from_timestamp(offset, 0).unwrap().fixed_offset();
    let mut intervals: Vec<_> = (0..100)
        .map(|index| (instant(index), instant(110)))
        .collect();
    assert_eq!(common_window(&intervals), Some((instant(99), instant(110))));
    intervals[0].1 = instant(99);
    assert_eq!(common_window(&intervals), None);
}
