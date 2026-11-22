// Audit retention as the collector reports it with its integrity check.
// An archived prefix is never shown as an absence of activity: every view
// that reads the live journal names what was archived and where it went.

export interface RetentionSegmentSummary {
  segment_index: number;
  archive_id: string;
  manifest_sha256: string;
  first_seq: number;
  last_seq: number;
  reclaimed_events: number;
  reclaimed_bytes: number;
  retention_event_seq: number;
}

export interface RetentionLedger {
  capability: boolean;
  segments: number;
  archived_through_seq: number;
  reclaimed_events: number;
  reclaimed_bytes: number;
  last: RetentionSegmentSummary | null;
}

const count = (value: unknown): number | null =>
  typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;

const text = (value: unknown, pattern: RegExp): string | null =>
  typeof value === "string" && pattern.test(value) ? value : null;

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const SHA256 = /^[0-9a-f]{64}$/;

function parseSegment(value: unknown): RetentionSegmentSummary | null {
  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  const segment_index = count(record.segment_index);
  const archive_id = text(record.archive_id, UUID);
  const manifest_sha256 = text(record.manifest_sha256, SHA256);
  const first_seq = count(record.first_seq);
  const last_seq = count(record.last_seq);
  const reclaimed_events = count(record.reclaimed_events);
  const reclaimed_bytes = count(record.reclaimed_bytes);
  const retention_event_seq = count(record.retention_event_seq);
  if (
    segment_index === null || segment_index < 1 || archive_id === null || manifest_sha256 === null ||
    first_seq === null || last_seq === null || last_seq < first_seq || reclaimed_events === null ||
    reclaimed_bytes === null || retention_event_seq === null || retention_event_seq <= last_seq
  ) return null;
  return {
    segment_index, archive_id, manifest_sha256, first_seq, last_seq,
    reclaimed_events, reclaimed_bytes, retention_event_seq,
  };
}

/** `null` means unknown: an absent or malformed ledger is never read as "nothing archived". */
export function parseRetentionLedger(value: unknown): RetentionLedger | null {
  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  const segments = count(record.segments);
  const archived_through_seq = count(record.archived_through_seq);
  const reclaimed_events = count(record.reclaimed_events);
  const reclaimed_bytes = count(record.reclaimed_bytes);
  if (
    typeof record.capability !== "boolean" || segments === null || archived_through_seq === null ||
    reclaimed_events === null || reclaimed_bytes === null
  ) return null;
  const last = record.last === null ? null : parseSegment(record.last);
  if ((segments === 0) !== (last === null) || (last !== null && last.last_seq !== archived_through_seq)) {
    return null;
  }
  if (segments > 0 && !record.capability) return null;
  return { capability: record.capability, segments, archived_through_seq, reclaimed_events, reclaimed_bytes, last };
}

const integer = new Intl.NumberFormat("fr-FR", { maximumFractionDigits: 0 });

/** The notice every live journal view shows once a prefix is archived. */
export function retentionNotice(ledger: RetentionLedger | null): string | null {
  if (!ledger || ledger.segments === 0 || !ledger.last) return null;
  const segments = ledger.segments === 1 ? "1 segment" : `${integer.format(ledger.segments)} segments`;
  return `Historique archivé hors ligne jusqu’à l’événement n° ${integer.format(ledger.archived_through_seq)} : ` +
    `${segments}, ${integer.format(ledger.reclaimed_events)} événements retirés de la base vivante. ` +
    "Le journal et les exports n’incluent plus ces événements ; leur vérification exige les archives " +
    `(dernière archive ${ledger.last.archive_id.slice(0, 8)}…).`;
}

/** What the integrity card states about the verified chain. */
export function integrityCoverage(
  eventCount: number | null | undefined,
  storedEvents: number | null | undefined,
  ledger: RetentionLedger | null,
): string {
  const positions = count(eventCount);
  if (positions === null) return "Nombre d’événements vérifiés inconnu.";
  if (!ledger || ledger.segments === 0) {
    return `${integer.format(positions)} événements vérifiés par le collecteur.`;
  }
  const stored = count(storedEvents);
  return `${integer.format(positions)} positions de chaîne vérifiées par le collecteur, dont ` +
    `${stored === null ? "un nombre inconnu" : integer.format(stored)} encore dans la base vivante.`;
}
