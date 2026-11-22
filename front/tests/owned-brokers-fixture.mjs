// Exact whitelisted owned wire profile. Test data only; never imported by the application.
import { broker, brokerPage } from "./human-fixtures.mjs";
export const ownedReadProfiles = () => ({ broker_registry: "human_broker_registry_owned_v1", worker_registry: "unavailable", federation_registry: "unavailable" });
/** The owned source profile has no protocol for these reads: the control announces them unavailable in every context, granted or not. */
export const OWNED_UNAVAILABLE = ["workers_read", "federation_read", "quota_read", "usage_read"];
/** The same offer on the owned profile: permissions and scopes kept, availability stated by the profile. */
export const ownedCatalogue = (operations) => operations.map((entry) => OWNED_UNAVAILABLE.includes(entry.operation) ? { ...entry, availability: "profile_unavailable" } : entry);

export const ownedBroker = (extra = {}) => ownedBrokerFromLegacy(broker(), extra);
export function ownedBrokerFromLegacy(item, extra = {}) {
  return { broker_id: item.broker_id, incarnation_id: item.incarnation_id, state: item.state, clock_uncertain: item.state === "clock_uncertain",
    grant: { revision: item.grant_revision, revoked: item.revoked_at !== null, granted_at: item.granted_at, not_after: item.not_after,
      revoked_at: item.revoked_at, lease_seconds: 30, authorities: item.authorities },
    heartbeat: { revision: item.heartbeat_revision, last: item.last_heartbeat,
      current_incarnation: item.last_heartbeat?.incarnation_id === item.incarnation_id },
    checkpoint: { status: "received", received_at: "2026-11-01T12:00:01Z", lease_expires_at: "2026-11-01T12:00:31Z",
      freshness: "current", in_current_grant: true, coverage: "declared_partial", reference_count: 2, has_more: false,
      publication_provenance: "declared", reception_provenance: "observed" }, ...extra };
}
export function noCheckpoint(status = "unknown") {
  return { status, received_at: null, lease_expires_at: null, freshness: null, in_current_grant: null,
    coverage: null, reference_count: null, has_more: null, publication_provenance: null, reception_provenance: null };
}
export const ownedBrokerPage = (items = [ownedBroker()], extra = {}) =>
  ({ ...brokerPage([]), profile: "human_broker_registry_owned_v1", count_scope: "page", items, ...extra });
