// Synthetic broker events and register only. No adapter, provider or runtime.
export const id = (n) => "00000000-0000-4000-8000-" + String(n).padStart(12, "0");
export const digest = (digit) => digit.repeat(64);
export const fixtureProof = {
  version: 1, operation_id: id(1), session_id: id(2), container_id: digest("a"),
  result_audit_seq: 3, cleanup_audit_seq: 4, scan_audit_seq: 5, scan_manifest_sha256: digest("b"),
  export_command_id: id(6), export_request_id: id(6), export_manifest_sha256: digest("c"), export_file_count: 2,
  file_name: "result.txt", file_sha256: digest("d"), client: "codex", auth_mode: "api", client_profile: "fixture",
  model_requested: "gpt-6-luna", runtime: "docker", worker_image_id: "sha256:" + digest("e"),
};

export function brokerBatchFixture() {
  const expected = structuredClone(fixtureProof);
  const workerId = id(7), attempt = id(8), request = id(9);
  const resolution = {
    version: 1, client: expected.client, auth_mode: expected.auth_mode, profile_id: expected.client_profile,
    model: expected.model_requested, transport: "batch",
  };
  const exited = { status: "exited", running: false, exit_code: 0, oom_killed: false,
    started_at: "2026-11-18T00:00:00Z", finished_at: "2026-11-18T00:00:01Z" };
  const event = (seq, kind, source, data) => ({
    seq, kind, source, data, id: id(100 + seq), hash: String(seq).padStart(64, "0"),
    prev_hash: String(seq - 1).padStart(64, "0"), timestamp: "2026-11-18T00:00:10Z", received_at: "2026-11-18T00:00:11Z",
    session_id: expected.session_id, sandbox_id: expected.session_id, correlation_id: id(10), provenance: "observed",
  });
  const decision = { ...event(1, "security.decision", "gateway", {
    request_id: request, request_hash: digest("1"), provider_resolution: resolution,
  }), decision: { outcome: "allow" } };
  const worker = {
    worker_id: workerId, admission_operation_id: expected.operation_id, request_id: request,
    spec_hash: digest("2"), state: "terminal", quota_held: false, revision: 6,
    observed_started_at: exited.started_at, unknown_reason: null, allocation_inconsistent: false,
    cleanup_removed: true, cleanup_error: null, receipt: {
      version: 1, audit_seq: decision.seq, audit_hash: decision.hash, request_id: request, request_hash: digest("1"),
    },
    launch: { version: 1, profile: "strict", provider: expected.client, cpu_millis: 2000, memory_bytes: 1073741824,
      timeout_seconds: 60, pids_limit: 64, network: "none",
      session_id: expected.session_id, runtime_kind: expected.runtime, authority_id: id(11),
      image_id: expected.worker_image_id, capture_output: false, provider_resolution: resolution },
    attempt: { attempt_id: attempt, container_id: expected.container_id, stable_name: "worker-fixture", volume_name: "volume-fixture" },
    last_observation: { ...exited, observed_at: exited.finished_at, restart_count: 0,
      inspection: { runtime_kind: expected.runtime, authority_id: id(11), spec_hash: digest("2"),
      container_id: expected.container_id, image_id: expected.worker_image_id } },
  };
  const result = { ...event(expected.result_audit_seq, "broker.batch.result", "runner", {
    version: 1, worker_id: workerId, attempt_id: attempt,
    observation: { id: expected.container_id, image_id: expected.worker_image_id, process: exited, observed_at: exited.finished_at },
    transport: { exit_code: 23, duration_ms_decimal: "1000",
      stdout: { total_bytes: 12, truncated: false, text_clipped: null, text: null },
      stderr: { total_bytes: 1, truncated: false, text_clipped: null, text: null } },
  }), command: { argv: ["/usr/bin/id"], cwd: "/workspace" } };
  const cleanup = { ...event(expected.cleanup_audit_seq, "control.worker.transition", "gateway", {
    version: 1, action: "cleanup_result", worker: structuredClone(worker),
  }), sandbox_id: null, correlation_id: null };
  const scan = event(expected.scan_audit_seq, "broker.batch.scan", "runner", {
    version: 1, worker_id: workerId, manifest_digest: expected.scan_manifest_sha256, changed_files: 1, truncated: false,
    changes: [{ path: expected.file_name, change: "created", before: null, after: { sha256: expected.file_sha256 } }],
  });
  const policy = event(6, "workspace.export.policy", "runner", {
    version: 1, request_id: expected.export_request_id, manifest_digest: expected.export_manifest_sha256,
  });
  const exported = event(7, "broker.batch.export", "runner", {
    ...policy.data, destination_ref: expected.export_request_id, files: expected.export_file_count,
  });
  return { expected, worker, cleanup, events: [decision, result, scan, policy, exported] };
}
