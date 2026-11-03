import { createSseParser } from "./sse.ts";
import { HumanContractError, boundedText, humanUrl, object, opaque, parseHumanEventPage, parseHumanIdentity, parseResourcePage, uuid } from "./human-model.ts";
import type { HumanContext, HumanEventPage, HumanIdentity, HumanSelection, ResourceRef } from "./human-model.ts";

export class HumanSuperseded extends Error { constructor() { super("Lecture abandonnée après changement de contexte."); } }
export class HumanApiError extends Error {
  status: number; code: string; retryAction: string;
  constructor(status: number, code: string, message: string, retryAction = "retry") {
    super(message); this.status = status; this.code = code; this.retryAction = retryAction;
  }
}
export const HUMAN_EXPORT_LIMITS = Object.freeze({ pages: 25, bytes: 4 * 1024 * 1024, milliseconds: 60000 });
interface Snapshot { generation: number; context: HumanContext | null }
interface Handlers {
  onAuthentication: (error: HumanApiError) => void;
  onContextInvalidated: (error: HumanApiError) => void;
}
const codes = new Set(["authentication_required", "credential_invalid", "credential_expired", "identity_revoked", "context_required",
  "context_invalid", "context_expired", "context_revoked", "permission_denied", "not_found", "invalid_request", "invalid_cursor",
  "capacity_exhausted", "stream_admission_unavailable", "audit_read_budget_exhausted", "identity_unavailable", "audit_unavailable", "migration_required"]);
const actions = new Set(["authenticate", "select_context", "review_permissions", "reduce_page", "retry", "contact_operator", "close_unused_streams"]);

/** Separate trust boundary: a local human credential can never fall back to the viewer API. */
export class HumanApiClient {
  #token: string;
  #fetcher: typeof fetch;
  #handlers: Handlers;
  #lifetime = new AbortController();
  #scope = new AbortController();
  #generation = 0;
  #identityRequest = 0;
  #identity: HumanIdentity | null = null;
  #context: HumanContext | null = null;
  #expiry: ReturnType<typeof setTimeout> | undefined;
  #credentialExpiry: ReturnType<typeof setTimeout> | undefined;
  constructor(token: string, handlers: Handlers, fetcher: typeof fetch = fetch) {
    this.#token = token; this.#handlers = handlers; this.#fetcher = fetcher;
  }
  get context() { return this.#context ? structuredClone(this.#context) : null; }
  close() {
    this.#token = ""; this.#identity = null;
    this.clearContext(); this.#lifetime.abort(); clearTimeout(this.#credentialExpiry);
  }
  clearContext() {
    this.#generation++; this.#scope.abort(); this.#scope = new AbortController();
    this.#context = null; clearTimeout(this.#expiry);
  }
  #snapshot(contextRequired: boolean): Snapshot {
    if (!this.#token || this.#lifetime.signal.aborted) throw new HumanSuperseded();
    if (contextRequired && !this.#context) throw new HumanApiError(403, "context_required", "Choisissez un contexte autorisé.", "select_context");
    if (this.#context && Date.parse(this.#context.expires_at) <= Date.now())
      this.#reject(new HumanApiError(403, "context_expired", "Le contexte a expiré. Choisissez un nouveau contexte.", "select_context"));
    return { generation: this.#generation, context: this.#context };
  }
  #current(snapshot: Snapshot) {
    return !!this.#token && !this.#lifetime.signal.aborted && snapshot.generation === this.#generation && snapshot.context?.id === this.#context?.id;
  }
  #assert(snapshot: Snapshot) { if (!this.#current(snapshot)) throw new HumanSuperseded(); }
  #reject(error: HumanApiError): never {
    if (error.status === 401) { this.close(); this.#handlers.onAuthentication(error); }
    else if (["context_required", "context_invalid", "context_expired", "context_revoked"].includes(error.code)) {
      this.clearContext(); this.#handlers.onContextInvalidated(error);
    }
    throw error;
  }
  #parseError(value: unknown, status: number): HumanApiError {
    if (object(value) && object(value.error) && typeof value.error.code === "string" && codes.has(value.error.code) &&
      boundedText(value.error.message, 1024) && typeof value.error.retry_action === "string" && actions.has(value.error.retry_action))
      return new HumanApiError(status, value.error.code, value.error.message, value.error.retry_action);
    return new HumanApiError(status, status === 401 ? "authentication_required" : status === 403 ? "permission_denied" : "identity_unavailable",
      status === 401 ? "Credential personnel refusé ou expiré. Reconnectez-vous." :
        status === 403 ? "Consultation refusée pour cette ressource. Faites vérifier vos permissions." : `Lecture humaine indisponible (HTTP ${status}). Réessayez ou contactez l’opérateur.`);
  }
  #parseText(text: string): unknown {
    // Defense in depth against an echo of this request's bearer, before any UI/export persistence.
    for (const secret of [this.#token, encodeURIComponent(this.#token)]) {
      if (secret) text = text.split(JSON.stringify(secret).slice(1, -1)).join("[REDACTED]");
    }
    try { return JSON.parse(text); } catch { throw new HumanContractError(); }
  }
  async #json(response: Response, snapshot: Snapshot): Promise<{ value: unknown; bytes: number }> {
    if (!response.body) throw new HumanContractError();
    const reader = response.body.getReader(); const decoder = new TextDecoder();
    let bytes = 0; let text = "";
    try {
      for (;;) {
        const chunk = await reader.read(); this.#assert(snapshot);
        if (chunk.done) break;
        bytes += chunk.value.byteLength;
        if (bytes > 256 * 1024) throw new Error("Réponse trop volumineuse. Réduisez la sélection ou contactez l’opérateur.");
        text += decoder.decode(chunk.value, { stream: true });
      }
      text += decoder.decode();
      return { value: this.#parseText(text), bytes };
    } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
  }
  async #request(path: string, snapshot: Snapshot, signal?: AbortSignal, stream = false): Promise<Response> {
    this.#assert(snapshot);
    if (!/^\/api\/v1\/human\/(identity|resources|events|export|stream)(?:\/[0-9a-f-]{36})?(?:\?[^#]*)?$/.test(path)) throw new HumanContractError();
    const headers: Record<string, string> = { Authorization: `Bearer ${this.#token}`, Accept: stream ? "text/event-stream" : "application/json" };
    if (snapshot.context) headers["X-AiGuard-Context"] = snapshot.context.id;
    const signals = [this.#lifetime.signal, this.#scope.signal];
    if (signal) signals.push(signal);
    if (!stream) signals.push(AbortSignal.timeout(15000));
    const fetcher = this.#fetcher;
    const response = await fetcher(path, { method: "GET", headers, credentials: "omit", cache: "no-store", redirect: "error", signal: AbortSignal.any(signals) });
    if (!this.#token || this.#lifetime.signal.aborted) {
      void response.body?.cancel().catch(() => {}); throw new HumanSuperseded();
    }
    // Credential refusal applies to this client even when its context changed during the request.
    // Purge before touching the body; neither its bytes nor its cancellation may delay logout.
    if (response.status === 401) {
      const error = new HumanApiError(401, "authentication_required", "Credential personnel refusé ou expiré. Reconnectez-vous.", "authenticate");
      this.close(); this.#handlers.onAuthentication(error);
      void response.body?.cancel().catch(() => {}); throw error;
    }
    if (!this.#current(snapshot)) { void response.body?.cancel().catch(() => {}); throw new HumanSuperseded(); }
    if (!response.ok) {
      let value: unknown = null;
      try { value = (await this.#json(response, snapshot)).value; } catch { this.#assert(snapshot); }
      this.#reject(this.#parseError(value, response.status));
    }
    return response;
  }
  #validatePage<T extends { context_id: string; authorization_version: string }>(page: T, snapshot: Snapshot): T {
    this.#assert(snapshot);
    const context = snapshot.context;
    if (!context || page.context_id !== context.id || page.authorization_version !== context.authorization_version)
      this.#reject(new HumanApiError(403, "context_revoked", "Les autorisations ont changé. Les données du contexte ont été retirées.", "select_context"));
    return page;
  }
  async identity(): Promise<HumanIdentity> {
    const snapshot = this.#snapshot(false); const request = ++this.#identityRequest;
    const body = await this.#json(await this.#request("/api/v1/human/identity", snapshot), snapshot);
    this.#assert(snapshot); if (request !== this.#identityRequest) throw new HumanSuperseded();
    const identity = parseHumanIdentity(body.value); this.#identity = structuredClone(identity);
    clearTimeout(this.#credentialExpiry);
    const expires = identity.authentication.expires_at;
    if (expires && Date.parse(expires) <= Date.now())
      this.#reject(new HumanApiError(401, "credential_expired", "Votre credential personnel a expiré. Reconnectez-vous.", "authenticate"));
    const checkExpiry = () => {
      if (!this.#token || !expires) return;
      const remaining = Date.parse(expires) - Date.now();
      if (remaining <= 0) {
        const error = new HumanApiError(401, "credential_expired", "Votre credential personnel a expiré. Reconnectez-vous.", "authenticate");
        this.close(); this.#handlers.onAuthentication(error);
      } else this.#credentialExpiry = setTimeout(checkExpiry, Math.min(remaining, 2147483647));
    };
    if (expires) checkExpiry();
    if (!this.#token) throw new HumanSuperseded();
    return identity;
  }
  selectContext(id: string): HumanContext {
    const selected = this.#identity?.contexts.find((context) => context.id === id);
    if (!selected) throw new HumanApiError(403, "context_invalid", "Ce contexte n’a pas été proposé par le contrôle.", "select_context");
    if (Date.parse(selected.expires_at) <= Date.now())
      throw new HumanApiError(403, "context_expired", "Ce contexte a expiré. Relisez vos accès.", "select_context");
    this.clearContext(); this.#context = structuredClone(selected);
    const checkExpiry = () => {
      if (this.#context?.id !== selected.id) return;
      const remaining = Date.parse(selected.expires_at) - Date.now();
      if (remaining > 0) this.#expiry = setTimeout(checkExpiry, Math.min(remaining, 2147483647));
      else {
        const error = new HumanApiError(403, "context_expired", "Le contexte a expiré. Relisez vos accès pour continuer.", "select_context");
        this.clearContext(); this.#handlers.onContextInvalidated(error);
      }
    };
    checkExpiry();
    return structuredClone(selected);
  }
  async resources(cursor: string | null = null, signal?: AbortSignal) {
    const snapshot = this.#snapshot(true);
    const body = await this.#json(await this.#request(humanUrl("resources", { view: "metadata", cursor }), snapshot, signal), snapshot);
    return this.#validatePage(parseResourcePage(body.value), snapshot);
  }
  async events(selection: HumanSelection, signal?: AbortSignal): Promise<HumanEventPage> {
    const snapshot = this.#snapshot(true);
    const body = await this.#json(await this.#request(humanUrl("events", selection), snapshot, signal), snapshot);
    const page = this.#validatePage(parseHumanEventPage(body.value), snapshot);
    const ref = selection.resource;
    if (page.view !== selection.view || ref && page.items.some((item) =>
      item.metadata.resource.kind !== ref.kind || item.metadata.resource.resource_id !== ref.resource_id)) throw new HumanContractError();
    return page;
  }
  async event(id: string, view: HumanSelection["view"], signal?: AbortSignal) {
    if (!uuid(id)) throw new HumanContractError();
    const snapshot = this.#snapshot(true);
    const body = await this.#json(await this.#request(`/api/v1/human/events/${id}?view=${view}`, snapshot, signal), snapshot);
    const page = this.#validatePage(parseHumanEventPage(body.value), snapshot);
    if (page.view !== view || page.items.length !== 1 || page.has_more || page.items[0]?.metadata.id !== id) throw new HumanContractError();
    return page;
  }
  async exportSelection(selection: { view: HumanSelection["view"]; resource: ResourceRef }, signal: AbortSignal,
    onProgress: (pages: number, bytes: number) => void): Promise<Blob> {
    const snapshot = this.#snapshot(true);
    const timeout = AbortSignal.timeout(HUMAN_EXPORT_LIMITS.milliseconds);
    const cancel = AbortSignal.any([signal, timeout]);
    const pages: { items: HumanEventPage["items"]; read_receipt: HumanEventPage["read_receipt"] }[] = [];
    const cursors = new Set<string>(); let cursor: string | null = null; let bytes = 0;
    for (let index = 0; index < HUMAN_EXPORT_LIMITS.pages; index++) {
      cancel.throwIfAborted(); this.#assert(snapshot);
      const body = await this.#json(await this.#request(humanUrl("export", { ...selection, cursor }), snapshot, cancel), snapshot);
      const page = this.#validatePage(parseHumanEventPage(body.value, true), snapshot);
      if (page.view !== selection.view || page.items.some((item) => item.metadata.resource.kind !== selection.resource.kind ||
        item.metadata.resource.resource_id !== selection.resource.resource_id)) throw new HumanContractError();
      bytes += body.bytes;
      if (bytes > HUMAN_EXPORT_LIMITS.bytes) throw new Error("Export arrêté à 4 Mio. Aucune sélection partielle n’a été téléchargée.");
      pages.push({ items: page.items, read_receipt: page.read_receipt }); onProgress(pages.length, bytes);
      cancel.throwIfAborted(); this.#assert(snapshot);
      if (page.selection_complete) {
        const blob = new Blob([JSON.stringify({ format: "ai-guard-human-export-v1", selection: { ...selection,
          tenant_id: snapshot.context?.tenant_id, organization_scope: snapshot.context?.organization_scope, mode: snapshot.context?.mode },
          authorization_version: page.authorization_version, coverage: "authorized_selection", selection_complete: true, pages })], { type: "application/json" });
        if (blob.size > HUMAN_EXPORT_LIMITS.bytes) throw new Error("Export arrêté à 4 Mio. Réduisez la sélection.");
        return blob;
      }
      cursor = page.next_cursor;
      if (!cursor || cursors.has(cursor)) throw new HumanContractError(); cursors.add(cursor);
    }
    throw new Error("Export arrêté après 25 pages. Aucune sélection partielle n’a été téléchargée. Choisissez une ressource plus précise.");
  }
  async listen(selection: HumanSelection, signal: AbortSignal, onPage: (page: HumanEventPage) => void,
    onStatus: (status: "connecting" | "live" | "retrying" | "closed") => void): Promise<void> {
    const snapshot = this.#snapshot(true); let cursor: string | null = null; let attempt = 0;
    const cancel = AbortSignal.any([this.#lifetime.signal, this.#scope.signal, signal]);
    while (!cancel.aborted && this.#current(snapshot)) {
      onStatus(attempt ? "retrying" : "connecting");
      try {
        const response = await this.#request(humanUrl("stream", { ...selection, cursor }), snapshot, cancel, true);
        if (!response.body || !response.headers.get("content-type")?.includes("text/event-stream")) throw new HumanContractError();
        onStatus("live");
        const reader = response.body.getReader(); const decoder = new TextDecoder();
        const parser = createSseParser((message) => {
          this.#assert(snapshot);
          if (new TextEncoder().encode(message.data).byteLength > 256 * 1024) throw new HumanContractError();
          if (message.event === "error") {
            const value = this.#parseText(message.data);
            const code = object(value) && object(value.error) ? value.error.code : "identity_unavailable";
            this.#reject(this.#parseError(value, ["authentication_required", "credential_invalid", "credential_expired", "identity_revoked"].includes(String(code)) ? 401 : 403));
          }
          if (message.event !== "human_audit") return;
          const page = this.#validatePage(parseHumanEventPage(this.#parseText(message.data), false, "stream"), snapshot);
          if (page.view !== selection.view || !opaque(message.id)) throw new HumanContractError();
          cursor = message.id; onPage(page); attempt = 0;
        }, 256 * 1024);
        try {
          for (;;) {
            const chunk = await reader.read(); this.#assert(snapshot);
            if (chunk.done) { parser.feed(decoder.decode()); break; }
            parser.feed(decoder.decode(chunk.value, { stream: true }));
          }
        } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
      } catch (error) {
        if (cancel.aborted || !this.#current(snapshot)) return;
        if (error instanceof HumanApiError || error instanceof HumanContractError) throw error;
      }
      if (cancel.aborted || !this.#current(snapshot)) return;
      onStatus("retrying"); attempt++;
      await new Promise<void>((resolve) => {
        const finish = () => { clearTimeout(timer); cancel.removeEventListener("abort", finish); resolve(); };
        const timer = setTimeout(finish, Math.min(30000, 1000 * 2 ** Math.min(attempt - 1, 5)));
        cancel.addEventListener("abort", finish, { once: true });
      });
    }
  }
}
