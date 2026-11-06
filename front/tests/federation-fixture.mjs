// Browser fixture only; this module is outside src and never bundled.
export function federationPage(page, registered = null) {
  const association = (authority) => {
    if (page.registry_status === "migration_required") return { status: "registry_unavailable", broker: null,
      runtime_kind: null, runtime_matches: null, in_current_grant: null };
    if (!authority) return { status: "authority_unknown", broker: null, runtime_kind: null,
      runtime_matches: null, in_current_grant: null };
    if (!registered || authority !== registered.authority_id) return {
      status: "unregistered", broker: null, runtime_kind: null, runtime_matches: null, in_current_grant: null,
    };
    return { status: "registered", broker: registered.broker, runtime_kind: "docker",
      runtime_matches: true, in_current_grant: false };
  };
  return { version: 1, source: "tenant_worker_broker_registry", consistency: "snapshot_per_page",
    registry_status: page.registry_status ?? "available",
    evaluated_at: "2026-11-06T01:03:00.000Z", runtime_inventory: "unknown", next_cursor: page.next_cursor,
    items: page.items?.map((worker) => ({ worker, launch_broker: association(worker?.launch?.authority_id),
      observation_broker: association(worker?.last_observation?.inspection?.authority_id) })) };
}
