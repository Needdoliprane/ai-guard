// The owned human registry is a distinct, closed wire profile. This adapter validates it in full before
// sharing the existing broker presentation model; it never retries another route or infers runtime facts.
import { HumanContractError, object, pageLimit, parseHumanBrokerPage } from "./human-model.ts";
import type { HumanBroker, HumanBrokerPage, HumanOwnedBrokerCheckpoint } from "./human-model.ts";

export const OWNED_BROKER_PROFILE = "human_broker_registry_owned_v1" as const;
export const OWNED_BROKER_PAGE_BYTES = 256 * 1024;
const cursorPattern = /^[0-9a-f]{1082}$/;
function requireContract(condition: unknown): asserts condition { if (!condition) throw new HumanContractError(); }
function closed(value: unknown, fields: readonly string[]): asserts value is Record<string, unknown> {
  requireContract(object(value) && Object.keys(value).length === fields.length && fields.every((key) => Object.hasOwn(value, key)));
}
/** RFC3339 dates emitted by the registry are bounded and strictly after the Unix epoch. */
function instant(value: unknown): bigint | null {
  if (typeof value !== "string" || value.length > 64) return null;
  const parts = /^(\d{4})-(\d\d)-(\d\d)T(\d\d):(\d\d):(\d\d)(?:\.(\d+))?(Z|[+-]\d\d:\d\d)$/i.exec(value);
  if (!parts) return null;
  const [, year, month, day, hour, minute, second, fraction = "", zone] = parts;
  if (!zone) return null;
  const y = Number(year); const m = Number(month); const d = Number(day);
  const days = [31, y % 4 === 0 && (y % 100 !== 0 || y % 400 === 0) ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  if (m < 1 || m > 12 || d < 1 || d > days[m - 1]! || Number(hour) > 23 || Number(minute) > 59 || Number(second) > 59 ||
    zone !== "Z" && zone !== "z" && (Number(zone.slice(1, 3)) > 23 || Number(zone.slice(4, 6)) > 59)) return null;
  const milliseconds = Date.parse(`${year}-${month}-${day}T${hour}:${minute}:${second}${zone.toUpperCase()}`);
  // Match the server's timestamp() > 0 rule (fractional positive values in the first second are rejected).
  if (!Number.isFinite(milliseconds) || milliseconds < 1000) return null;
  return BigInt(milliseconds) * 1000000n + BigInt(fraction.slice(0, 9).padEnd(9, "0"));
}
function time(value: unknown): value is string { return instant(value) !== null; }
function ordered(start: string, end: string): boolean { return instant(end)! >= instant(start)!; }
const checkpointFields = ["status", "received_at", "lease_expires_at", "freshness", "in_current_grant", "coverage", "reference_count", "has_more", "publication_provenance", "reception_provenance"] as const;
function checkpoint(value: unknown): HumanOwnedBrokerCheckpoint {
  closed(value, checkpointFields);
  requireContract(["received", "absent", "unknown"].includes(String(value.status)));
  if (value.status !== "received") {
    requireContract(checkpointFields.slice(1).every((key) => value[key] === null));
    return { status: value.status as "absent" | "unknown", received_at: null, lease_expires_at: null, freshness: null,
      in_current_grant: null, coverage: null, reference_count: null, has_more: null, publication_provenance: null, reception_provenance: null };
  }
  requireContract(time(value.received_at) && time(value.lease_expires_at) && ordered(value.received_at, value.lease_expires_at) &&
    ["current", "stale", "historical", "clock_uncertain"].includes(String(value.freshness)) && typeof value.in_current_grant === "boolean" &&
    value.coverage === "declared_partial" && typeof value.reference_count === "number" && Number.isInteger(value.reference_count) &&
    value.reference_count >= 0 && value.reference_count <= 100 && typeof value.has_more === "boolean" &&
    value.publication_provenance === "declared" && value.reception_provenance === "observed");
  return { status: "received", received_at: value.received_at, lease_expires_at: value.lease_expires_at,
    freshness: value.freshness as "current" | "stale" | "historical" | "clock_uncertain", in_current_grant: value.in_current_grant,
    coverage: "declared_partial", reference_count: value.reference_count, has_more: value.has_more,
    publication_provenance: "declared", reception_provenance: "observed" };
}
export function parseHumanOwnedBrokerPage(value: unknown): HumanBrokerPage {
  closed(value, ["profile", "version", "context_id", "authorization_version", "source", "consistency", "evaluated_at", "runtime_inventory", "count_scope", "items", "has_more", "next_cursor", "read_receipt"]);
  requireContract(value.profile === OWNED_BROKER_PROFILE && value.count_scope === "page" && Array.isArray(value.items) && value.items.length <= 100 &&
    (value.next_cursor === null || typeof value.next_cursor === "string" && cursorPattern.test(value.next_cursor)));
  requireContract(time(value.evaluated_at) && object(value.read_receipt) && time(value.read_receipt.received_at));
  // The wire limit applies before normalization, including every nested summary and receipt.
  requireContract(new TextEncoder().encode(JSON.stringify(value)).byteLength <= OWNED_BROKER_PAGE_BYTES);
  const summaries: NonNullable<HumanBroker["owned"]>[] = [];
  const items = value.items.map((item) => {
    closed(item, ["broker_id", "incarnation_id", "state", "clock_uncertain", "grant", "heartbeat", "checkpoint"]);
    closed(item.grant, ["revision", "revoked", "granted_at", "not_after", "revoked_at", "lease_seconds", "authorities"]);
    closed(item.heartbeat, ["revision", "last", "current_incarnation"]);
    const grant = item.grant; const heartbeat = item.heartbeat; const checked = checkpoint(item.checkpoint);
    requireContract(typeof grant.revoked === "boolean" && grant.revoked === (grant.revoked_at !== null) &&
      time(grant.granted_at) && time(grant.not_after) && (grant.revoked_at === null || time(grant.revoked_at)) &&
      typeof grant.lease_seconds === "number" && Number.isInteger(grant.lease_seconds) && grant.lease_seconds >= 1 && grant.lease_seconds <= 300 &&
      typeof heartbeat.current_incarnation === "boolean" && typeof item.clock_uncertain === "boolean" &&
      item.clock_uncertain === (item.state === "clock_uncertain" || checked.freshness === "clock_uncertain"));
    if (heartbeat.last === null) requireContract(heartbeat.revision === "0" && heartbeat.current_incarnation === false);
    else {
      requireContract(object(heartbeat.last) && time(heartbeat.last.received_at) && time(heartbeat.last.lease_expires_at) &&
        ordered(heartbeat.last.received_at, heartbeat.last.lease_expires_at) && heartbeat.revision !== "0" &&
        heartbeat.current_incarnation === (heartbeat.last.incarnation_id === item.incarnation_id));
    }
    summaries.push({ clock_uncertain: item.clock_uncertain, lease_seconds: grant.lease_seconds, checkpoint: checked });
    return { broker_id: item.broker_id, incarnation_id: item.incarnation_id, state: item.state, grant_revision: grant.revision,
      heartbeat_revision: heartbeat.revision, authorities: grant.authorities, granted_at: grant.granted_at, not_after: grant.not_after,
      revoked_at: grant.revoked_at, last_heartbeat: heartbeat.last };
  });
  // Reuse the closed validator of the common fields. Owned cursors are fixed hex, including all-digit values;
  // the legacy `opaque` helper has a different contract, so only this local validation frame uses null.
  const base = parseHumanBrokerPage({ version: value.version, context_id: value.context_id, authorization_version: value.authorization_version,
    source: value.source, consistency: value.consistency, evaluated_at: value.evaluated_at, runtime_inventory: value.runtime_inventory,
    items, has_more: false, next_cursor: null, read_receipt: value.read_receipt });
  requireContract(typeof value.has_more === "boolean" && value.has_more === (value.next_cursor !== null));
  return { ...base, profile: OWNED_BROKER_PROFILE, count_scope: "page", has_more: value.has_more, next_cursor: value.next_cursor as string | null,
    items: base.items.map((item, index) => ({ ...item, owned: summaries[index]! })) };
}
export function humanOwnedBrokersUrl(cursor: string | null = null, limit?: number): string {
  const query = new URLSearchParams({ limit: pageLimit(limit) });
  if (cursor !== null) { requireContract(cursorPattern.test(cursor)); query.set("cursor", cursor); }
  return `/api/v1/human/owned/brokers?${query}`;
}

const freshnessLabel = { current: "Checkpoint courant", stale: "Checkpoint périmé", historical: "Checkpoint historique", clock_uncertain: "Horloge incertaine du checkpoint" } as const;
/** Heartbeat and checkpoint are independent declarations. A missing summary is never invented for legacy. */
export function checkpointLabel(item: HumanBroker): string {
  const value = item.owned?.checkpoint;
  return !value ? "Checkpoint non fourni par ce profil" : value.status === "received" ? freshnessLabel[value.freshness] :
    value.status === "unknown" ? "Checkpoint inconnu" : "Aucune publication de checkpoint";
}
export function checkpointFacts(item: HumanBroker): [string, string][] {
  const value = item.owned?.checkpoint;
  if (!value) return [];
  if (value.status !== "received") return [["Checkpoint", checkpointLabel(item)], ["Portée", value.status === "unknown" ?
    "Recherche bornée : publication inconnue. L’activité reste inconnue." : "Aucune publication dans l’historique complet du broker. L’activité reste inconnue."]];
  return [["Checkpoint", checkpointLabel(item)], ["Réception du checkpoint", value.received_at], ["Expiration du checkpoint", value.lease_expires_at],
    ["Autorisation du checkpoint", value.in_current_grant ? "Dans l’autorisation courante" : "Hors de l’autorisation courante"],
    ["Couverture du checkpoint", "Publication déclarée · réception observée · couverture partielle"],
    ["Références du checkpoint", `${value.reference_count} référence(s) déclarée(s) dans cette publication${value.has_more ? " · d’autres références existent" : ""} ; ce compte ne mesure pas l’activité ni les workers du tenant.`]];
}
