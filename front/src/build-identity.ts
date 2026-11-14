/** Public, descriptive build identity. It conveys no readiness or permission. */
export interface BuildMetadata { id: string | null; revision: string | null }
export interface FrontendIdentity { version: string; build: BuildMetadata }
export type ServiceComponent = "dashboard" | "gateway";
export interface BuildStatus extends FrontendIdentity {
  schema_version: 1;
  product: "ai-guard";
  component: ServiceComponent;
}
export type IdentityReading = { kind: "loading" } | { kind: "available"; identity: BuildStatus } |
  { kind: "unavailable"; reason: "unsupported" | "invalid" | "timeout" | "network" | "http" };
export interface ServiceIdentities { dashboard: IdentityReading; gateway: IdentityReading }

const MAX_BYTES = 4096;
const TIMEOUT_MS = 4000;
const paths: Record<ServiceComponent, string> = {
  dashboard: "/api/v1/status", gateway: "/api/v1/gateway/status",
};
const buildId = (value: unknown): value is string => typeof value === "string" &&
  value.length >= 1 && value.length <= 64 && !/[^A-Za-z0-9._+-]/.test(value);
const revision = (value: unknown): value is string => typeof value === "string" &&
  (value.length === 40 || value.length === 64) && !/[^A-Fa-f0-9]/.test(value);
function version(value: unknown): value is string {
  if (typeof value !== "string" || value.length > 64 || /[^\x21-\x7e]/.test(value)) return false;
  const match = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.exec(value);
  return match !== null && (!match[4] || match[4].split(".").every((part) => !/^[0-9]+$/.test(part) || part === "0" || !part.startsWith("0")));
}
function record(value: unknown, keys: string[]): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Invalid build identity");
  const object = value as Record<string, unknown>;
  if (Object.keys(object).length !== keys.length || !keys.every((key) => Object.hasOwn(object, key))) throw new Error("Invalid build identity");
  return object;
}
export function frontendIdentity(value: string, id?: unknown, rev?: unknown, expectedVersion?: string): FrontendIdentity {
  if (!version(value)) throw new Error("Invalid frontend package version");
  if (expectedVersion !== undefined && expectedVersion !== "" && expectedVersion !== value) throw new Error("AI_GUARD_VERSION must match the frontend package version");
  return { version: value, build: { id: buildId(id) ? id : null, revision: revision(rev) ? rev : null } };
}
export function parseBuildStatus(value: unknown, component: ServiceComponent): BuildStatus {
  const dto = record(value, ["schema_version", "product", "version", "component", "build"]);
  const build = record(dto.build, ["id", "revision"]);
  if (dto.schema_version !== 1 || dto.product !== "ai-guard" || dto.component !== component || !version(dto.version) ||
      !(build.id === null || buildId(build.id)) || !(build.revision === null || revision(build.revision))) throw new Error("Invalid build identity");
  return { schema_version: 1, product: "ai-guard", version: dto.version, component,
    build: { id: build.id, revision: build.revision } };
}

function cancel(body: ReadableStream<Uint8Array> | null) {
  if (body) void body.cancel().catch(() => {});
}
async function readBody(response: Response, signal: AbortSignal): Promise<unknown> {
  const contentType = response.headers.get("content-type")?.split(";")[0]?.trim().toLowerCase();
  const length = response.headers.get("content-length");
  if (contentType !== "application/json" || length !== null && (!/^[0-9]+$/.test(length) || Number(length) > MAX_BYTES) || !response.body) {
    cancel(response.body); throw new Error("Invalid build response");
  }
  const reader = response.body.getReader();
  // Never wait for cancellation of a broken source to settle.
  const abort = () => { void reader.cancel().catch(() => {}); };
  signal.addEventListener("abort", abort, { once: true });
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    if (signal.aborted) { abort(); throw new Error("Aborted build response"); }
    while (true) {
      const { value, done } = await reader.read();
      if (signal.aborted) throw new Error("Aborted build response");
      if (done) break;
      size += value.byteLength;
      if (size > MAX_BYTES || chunks.length >= 128) { abort(); throw new Error("Oversized build response"); }
      chunks.push(value);
    }
    const bytes = new Uint8Array(size);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } finally { signal.removeEventListener("abort", abort); reader.releaseLock(); }
}
export async function readBuildStatus(component: ServiceComponent, request: typeof fetch = fetch, timeoutMs = TIMEOUT_MS,
  parentSignal?: AbortSignal): Promise<IdentityReading> {
  const controller = new AbortController();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let expired = false;
  const abort = () => controller.abort();
  parentSignal?.addEventListener("abort", abort, { once: true });
  if (parentSignal?.aborted) controller.abort();
  const operation = async (): Promise<IdentityReading> => {
    let response: Response;
    try {
      response = await request(paths[component], { method: "GET", credentials: "omit", cache: "no-store", redirect: "error",
        referrerPolicy: "no-referrer", headers: { Accept: "application/json" }, signal: controller.signal });
    } catch { return { kind: "unavailable", reason: expired ? "timeout" : "network" }; }
    if (controller.signal.aborted) { cancel(response.body); return { kind: "unavailable", reason: expired ? "timeout" : "network" }; }
    if (response.status !== 200 || response.redirected) {
      cancel(response.body);
      return { kind: "unavailable", reason: response.status === 404 ? "unsupported" : "http" };
    }
    try { return { kind: "available", identity: parseBuildStatus(await readBody(response, controller.signal), component) }; }
    catch { return { kind: "unavailable", reason: expired ? "timeout" : "invalid" }; }
  };
  const deadline = new Promise<IdentityReading>((resolve) => {
    timer = setTimeout(() => { expired = true; controller.abort(); resolve({ kind: "unavailable", reason: "timeout" }); },
      Number.isFinite(timeoutMs) && timeoutMs > 0 ? Math.min(timeoutMs, TIMEOUT_MS) : TIMEOUT_MS);
  });
  try { return await Promise.race([operation(), deadline]); }
  finally { clearTimeout(timer); parentSignal?.removeEventListener("abort", abort); }
}

const unavailableLabels = {
  unsupported: "Indisponible · route absente (version ancienne possible)", invalid: "Indisponible · réponse incompatible",
  timeout: "Indisponible · délai dépassé", network: "Indisponible · connexion impossible", http: "Indisponible · service en erreur",
};
export function identityRows(frontend: FrontendIdentity, state: ServiceIdentities) {
  const rows: { label: string; version: string | null; build: BuildMetadata | null; message: string }[] = [
    { label: "Interface", version: frontend.version, build: frontend.build, message: "Bundle chargé dans ce navigateur" },
  ];
  for (const component of ["dashboard", "gateway"] as const) {
    const reading = state[component];
    rows.push({ label: component === "dashboard" ? "Dashboard" : "Gateway", version: reading.kind === "available" ? reading.identity.version : null,
      build: reading.kind === "available" ? reading.identity.build : null,
      message: reading.kind === "available" ? "Identité déclarée par le service" : reading.kind === "loading" ? "Lecture en cours…" : unavailableLabels[reading.reason] });
  }
  return rows;
}
export function identitySummary(frontend: FrontendIdentity, state: ServiceIdentities) {
  const readings = [state.dashboard, state.gateway];
  const identities = [frontend, ...readings.flatMap((reading) => reading.kind === "available" ? [reading.identity] : [])];
  const versionMismatch = identities.some((identity) => identity.version !== frontend.version);
  const buildMismatch = (["id", "revision"] as const).some((field) =>
    new Set(identities.flatMap((identity) => {
      const value = identity.build[field];
      return value === null ? [] : [field === "revision" ? value.toLowerCase() : value];
    })).size > 1);
  const mismatch = versionMismatch || buildMismatch;
  const complete = readings.every((reading) => reading.kind === "available");
  const missingBuild = identities.some((identity) => identity.build.id === null);
  return { mismatch, complete, message: versionMismatch ? "Versions différentes" : buildMismatch ? "Builds différents" :
    readings.some((reading) => reading.kind === "loading") ? "Lecture des identités…" : complete ?
      missingBuild ? "Version concordante · build inconnu" : "Identités concordantes" : "Identité des services incomplète" };
}

export class BuildIdentityStore {
  #request: typeof fetch;
  #state: ServiceIdentities = { dashboard: { kind: "loading" }, gateway: { kind: "loading" } };
  #listeners = new Set<(state: ServiceIdentities) => void>();
  #inFlight: Promise<void> | null = null;
  #closed = false;
  #abort = new AbortController();
  constructor(request: typeof fetch = fetch) { this.#request = request; }
  subscribe(listener: (state: ServiceIdentities) => void) {
    if (this.#closed) return () => {};
    this.#listeners.add(listener); listener(this.#state);
    return () => { this.#listeners.delete(listener); };
  }
  refresh(): Promise<void> {
    if (this.#closed) return Promise.resolve();
    if (this.#inFlight) return this.#inFlight;
    this.#state = { dashboard: { kind: "loading" }, gateway: { kind: "loading" } };
    this.#publish();
    const request = this.#request;
    this.#inFlight = Promise.all((["dashboard", "gateway"] as const).map(async (component) => {
      const reading = await readBuildStatus(component, request, TIMEOUT_MS, this.#abort.signal);
      if (this.#closed) return;
      this.#state = { ...this.#state, [component]: reading }; this.#publish();
    })).then(() => {}).finally(() => { this.#inFlight = null; });
    return this.#inFlight;
  }
  #publish() { for (const listener of this.#listeners) listener(this.#state); }
  close() { this.#closed = true; this.#listeners.clear(); this.#abort.abort(); }
}
