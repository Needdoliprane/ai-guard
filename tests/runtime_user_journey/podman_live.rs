//! Read-only native proof for a future Podman peak browser canary.

use super::{
    LiveWorker, RuntimeDoctor, RuntimeSelection, WORKER_CPU, WORKER_MEMORY, same_started_at,
    validate_parallel_quota,
};
use agentsandbox::runtime::{Runtime, RuntimeKind};
use anyhow::{Context, Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Component, Path};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::time::timeout;

const OUTPUT_LIMIT: u64 = 4096;
const INSPECT_TIMEOUT: Duration = Duration::from_secs(3);
// Podman's documented Go template exposes `.ID`; full JSON names that key
// `Id`. A narrow template keeps full inspection, including Config.Env, private.
const FORMAT: &str = concat!(
    r#"{"id":{{json .ID}},"name":{{json .Name}},"image":{{json .Image}},"#,
    r#""session":{{json (index .Config.Labels "agentsandbox.session")}},"#,
    r#""purpose":{{json (index .Config.Labels "agentsandbox.purpose")}},"#,
    r#""running":{{json .State.Running}},"paused":{{json .State.Paused}},"#,
    r#""restarting":{{json .State.Restarting}},"status":{{json .State.Status}},"#,
    r#""started_at":{{json .State.StartedAt}},"restart_count":{{json .RestartCount}},"#,
    r#""nano_cpus":{{json .HostConfig.NanoCpus}},"cpu_quota":{{json .HostConfig.CpuQuota}},"#,
    r#""cpu_period":{{json .HostConfig.CpuPeriod}},"memory_bytes":{{json .HostConfig.Memory}},"#,
    r#""pids_limit":{{json .HostConfig.PidsLimit}}}"#,
);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeRow {
    id: String,
    name: String,
    image: String,
    session: String,
    purpose: String,
    running: bool,
    paused: bool,
    restarting: bool,
    status: String,
    started_at: String,
    restart_count: u32,
    nano_cpus: i64,
    cpu_quota: i64,
    cpu_period: i64,
    memory_bytes: u64,
    pids_limit: u64,
}

#[derive(Serialize)]
pub(super) struct LivePodmanSnapshot {
    tenant: &'static str,
    cpu_millis: u64,
    #[serde(flatten)]
    native: NativeRow,
}

impl NativeRow {
    fn matches_limits(&self) -> bool {
        // Compare exact configured ratios. Zero can denote an unused encoding;
        // when both encodings are set they must agree.
        let quota_matches = self.cpu_quota > 0
            && self.cpu_period > 0
            && i128::from(self.cpu_quota) * 1000
                == i128::from(self.cpu_period) * i128::from(WORKER_CPU);
        let nano_matches = i128::from(self.nano_cpus) == i128::from(WORKER_CPU) * 1_000_000;
        let cpu_matches = match (self.nano_cpus, self.cpu_quota) {
            (0, _) => quota_matches,
            (_, 0) => self.cpu_period >= 0 && nano_matches,
            _ => quota_matches && nano_matches,
        };
        cpu_matches && self.memory_bytes == WORKER_MEMORY && self.pids_limit == 256
    }
}

fn is_cid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// `workers` and `quotas_at_peak` come from the same successful A/B polling
/// round. The caller retains the Doctor receipt from that selection and uses
/// this helper only after the matched browser DISPLAYED frame. Tenant names
/// come from the authenticated caller scopes.
pub(super) async fn inspect(
    selection: &RuntimeSelection,
    doctor: &RuntimeDoctor,
    workers: [&[LiveWorker]; 2],
    quotas_at_peak: [Value; 2],
) -> Result<Vec<LivePodmanSnapshot>> {
    ensure!(
        selection.runtime == RuntimeKind::Podman
            && selection.podman_rootless == Some(doctor.rootless)
            && selection.allow_rootful != doctor.rootless,
        "Podman live inspection requires the preflighted mode"
    );
    let socket = Path::new(
        selection
            .endpoint
            .strip_prefix("unix://")
            .context("Podman live inspection requires the selected Unix socket")?,
    );
    ensure!(
        socket.is_absolute()
            && !socket.components().any(|part| part == Component::ParentDir)
            && selection.endpoint.len() <= 4096
            && !selection.endpoint.contains(['?', '#', '@'])
            && !selection.endpoint.chars().any(char::is_control),
        "invalid Podman live Unix endpoint"
    );
    let before = std::fs::metadata(socket).context("Podman live socket unavailable")?;
    ensure!(
        before.file_type().is_socket(),
        "Podman live endpoint is not a socket"
    );
    ensure!(
        workers[0].len() == 2 && workers[1].len() == 1,
        "expected Podman scopes A1/A2 and B1"
    );
    validate_parallel_quota(quotas_at_peak, 2)?;
    let cids: Vec<_> = workers
        .into_iter()
        .flatten()
        .map(|worker| worker.container_id.as_str())
        .collect();
    ensure!(
        cids.iter().all(|cid| is_cid(cid)) && cids.iter().collect::<BTreeSet<_>>().len() == 3,
        "Podman live inspection requires three unique full CIDs"
    );
    let runtime = Runtime::with_program(RuntimeKind::Podman, &selection.program)?;
    let mut child = runtime
        .command()?
        .args([
            "--remote",
            "--url",
            &selection.endpoint,
            "container",
            "inspect",
            "--format",
            FORMAT,
        ])
        .args(cids)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("could not start the bounded Podman live inspection")?;
    let mut stdout = child
        .stdout
        .take()
        .context("Podman live inspection has no captured output")?
        .take(OUTPUT_LIMIT + 1);
    let result = timeout(INSPECT_TIMEOUT, async {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await?;
        ensure!(
            bytes.len() <= OUTPUT_LIMIT as usize,
            "Podman live inspection output exceeded its bound"
        );
        ensure!(
            child.wait().await?.success(),
            "Podman live inspection was refused"
        );
        Ok::<_, anyhow::Error>(bytes)
    })
    .await
    .unwrap_or_else(|_| Err(anyhow!("Podman live inspection timed out")));
    let bytes = match result {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = child.start_kill();
            let _ = timeout(INSPECT_TIMEOUT, child.wait()).await;
            return Err(error);
        }
    };
    let after =
        std::fs::metadata(socket).context("Podman live socket unavailable after inspection")?;
    ensure!(
        after.file_type().is_socket() && (before.dev(), before.ino()) == (after.dev(), after.ino()),
        "Podman live socket changed during inspection"
    );
    let mut native = BTreeMap::new();
    for row in serde_json::Deserializer::from_slice(&bytes).into_iter::<NativeRow>() {
        let row = row.map_err(|_| anyhow!("invalid projected Podman live inspection"))?;
        ensure!(
            native.insert(row.id.clone(), row).is_none(),
            "duplicate CID in Podman live inspection"
        );
    }
    ensure!(
        native.len() == 3,
        "Podman live inspection did not return exactly three rows"
    );

    let mut snapshots = Vec::with_capacity(3);
    for (tenant, group) in ["tenant-a", "tenant-b"].into_iter().zip(workers) {
        for worker in group {
            let mut row = native
                .remove(&worker.container_id)
                .context("Podman live inspection returned a different CID set")?;
            let name = row.name.strip_prefix('/').unwrap_or(&row.name);
            let image = row.image.strip_prefix("sha256:").unwrap_or(&row.image);
            ensure!(
                name == worker.stable_name
                    && is_cid(image)
                    && selection.image.strip_prefix("sha256:") == Some(image)
                    && worker.image_id == selection.image
                    && row.session == worker.session_id
                    && row.purpose == "run"
                    && row.running
                    && !row.paused
                    && !row.restarting
                    && row.status == "running"
                    && row.restart_count == 0
                    && same_started_at(&row.started_at, &worker.started_at)
                    && row.matches_limits(),
                "Podman did not attest the matching running worker and bounded configuration"
            );
            row.name = worker.stable_name.clone();
            row.image = selection.image.clone();
            snapshots.push(LivePodmanSnapshot {
                tenant,
                cpu_millis: WORKER_CPU,
                native: row,
            });
        }
    }
    Ok(snapshots)
}
