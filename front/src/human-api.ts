import { createSseParser } from "./sse.ts";
import { HUMAN_FEDERATION_PAGE_BYTES, OWNED_WORKER_PROFILE, HumanContractError, boundedText, humanBrokersUrl, humanWorkersUrl, humanFederationWorkersUrl, humanRecentEventsUrl, humanUrl, object, opaque, parseHumanBrokerPage, parseHumanWorkerPage, parseHumanFederationWorkerPage, parseHumanEventPage, parseHumanIdentity, parseHumanRecentEventPage, parseResourcePage, uuid } from "./human-model.ts";
import type { HumanContext, HumanEventPage, HumanIdentity, HumanOperation, HumanRecentEventPage, HumanSelection, HumanFederationSelection, Principal, ResourceRef } from "./human-model.ts";
import { humanOwnedBrokersUrl, parseHumanOwnedBrokerPage } from "./human-owned-brokers.ts";
import { SUMMARY_PAGE_BYTES, humanSummaryUrl, parseHumanSummary, type HumanSummary, type SummaryWindow } from "./summary-model.ts";
import { ADMIN_PAGE_BYTES, adminUrls, grantChangeBody, membershipChangeBody, parseGrantChangeReceipt, parseHumanConfiguration, parseHumanGrant, parseHumanQuota,
  parseMembershipChangeReceipt, parseMembershipPage, parseUsagePage, type GrantChange, type GrantChangeReceipt, type HumanConfiguration, type HumanGrant,
  type HumanQuota, type MembershipChange, type MembershipChangeReceipt, type MembershipPage, type UsagePage } from "./admin-model.ts";
import { OPERATION_FACTS, access, availabilityText } from "./access-model.ts";
import { parseSessionLaunchReceipt, parseSessionPrepareReceipt, sessionLaunchBody, sessionLaunchUrls, sessionPrepareBody,
  type SessionLaunchReceipt, type SessionLaunchRequest, type SessionPrepareReceipt, type SessionPrepareRequest } from "./session-launch-model.ts";

export class HumanSuperseded extends Error { constructor() { super("Lecture abandonnée après changement de contexte."); } }
export class HumanApiError extends Error {
  status: number; code: string; retryAction: string;
  constructor(status: number, code: string, message: string, retryAction = "retry") {
    super(message); this.status = status; this.code = code; this.retryAction = retryAction;
  }
}
/**
 * The control did not serve the newest-first read. A gateway that predates the route answers 404 with the
 * ordinary "not found" contract, so the cause is certain only for a read without resource filter (`ambiguous`
 * is false). With a filter, and until this client has seen the route answer, the same 404 may also be a
 * resource this context cannot read: callers fall back to the forward read, which is the authority for that refusal.
 */
export class HumanRecentUnavailable extends Error {
  ambiguous: boolean;
  constructor(ambiguous: boolean) { super("Lecture des événements récents indisponible sur ce contrôle. Mettez à jour gateway et dashboard ensemble."); this.ambiguous = ambiguous; }
}
export const HUMAN_EXPORT_LIMITS = Object.freeze({ pages: 25, bytes: 4 * 1024 * 1024, milliseconds: 60000 });
interface Snapshot { generation: number; context: HumanContext | null; operatorRetry?: boolean }
interface Handlers {
  onAuthentication: (error: HumanApiError) => void;
  onContextInvalidated: (error: HumanApiError, identity?: HumanIdentity) => void;
  onReadBudgetExhausted?: (error: HumanApiError) => void;
}
const codes = new Set(["authentication_required", "credential_invalid", "credential_expired", "identity_revoked", "context_required",
  "context_invalid", "context_expired", "context_revoked", "permission_denied", "not_found", "invalid_request", "invalid_cursor",
  "capacity_exhausted", "stream_admission_unavailable", "audit_read_budget_exhausted", "identity_unavailable", "audit_unavailable", "migration_required", "conflict"]);
const actions = new Set(["authenticate", "select_context", "review_permissions", "reduce_page", "retry", "contact_operator", "close_unused_streams", "reread"]);

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
  #readBudgetStopped = false;
  #operatorReadAllowed = false;
  /** True once this client received a page from the newest-first route. */
  #recentServed = false;
  constructor(token: string, handlers: Handlers, fetcher: typeof fetch = fetch) {
    this.#token = token; this.#handlers = handlers; this.#fetcher = fetcher;
  }
  get context() { return this.#context ? structuredClone(this.#context) : null; }
  get readProfiles() { return this.#identity?.read_profiles ? structuredClone(this.#identity.read_profiles) : undefined; }
  get readBudgetStopped() { return this.#readBudgetStopped; }
  /** One attempt after the user confirms an operator intervention; no server budget is reset. */
  allowReadAfterOperatorAction() { if (this.#readBudgetStopped) this.#operatorReadAllowed = true; }
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
  #reject(error: HumanApiError, withdrawOffers = false): never {
    if (error.status === 429 && error.code === "audit_read_budget_exhausted" && error.retryAction === "contact_operator") {
      this.#readBudgetStopped = true; this.#operatorReadAllowed = false; this.#handlers.onReadBudgetExhausted?.(error);
    }
    if (error.status === 401) { this.close(); this.#handlers.onAuthentication(error); }
    else if (withdrawOffers || ["context_required", "context_invalid", "context_expired", "context_revoked"].includes(error.code)) {
      if (withdrawOffers) {
        if (this.#identity) this.#identity = { ...this.#identity, contexts: [] };
        this.#operatorReadAllowed = false;
      }
      this.clearContext(); this.#handlers.onContextInvalidated(error);
    }
    throw error;
  }
  #parseError(value: unknown, status: number): HumanApiError {
    if (object(value) && object(value.error) && typeof value.error.code === "string" && codes.has(value.error.code) &&
      boundedText(value.error.message, 1024) && typeof value.error.retry_action === "string" && actions.has(value.error.retry_action))
      return new HumanApiError(status, value.error.code, value.error.message, value.error.retry_action);
    // A refusal that does not follow the error contract, as an intermediary may answer, is still a refusal:
    // it is never offered another attempt as an unavailable control is.
    return new HumanApiError(status, status === 401 ? "authentication_required" : status === 403 ? "permission_denied" : "identity_unavailable",
      status === 401 ? "Credential personnel refusé ou expiré. Reconnectez-vous." :
        status === 403 ? "Consultation refusée pour cette ressource. Faites vérifier vos permissions." : `Lecture humaine indisponible (HTTP ${status}). Réessayez ou contactez l’opérateur.`,
      status === 401 ? "authenticate" : status === 403 ? "review_permissions" : "retry");
  }
  #parseText(text: string): unknown {
    // Defense in depth against an echo of this request's bearer, before any UI/export persistence.
    for (const secret of [this.#token, encodeURIComponent(this.#token)]) {
      if (secret) text = text.split(JSON.stringify(secret).slice(1, -1)).join("[REDACTED]");
    }
    try { return JSON.parse(text); } catch { throw new HumanContractError(); }
  }
  async #json(response: Response, snapshot: Snapshot, maxBytes = 256 * 1024): Promise<{ value: unknown; bytes: number }> {
    if (!response.body) throw new HumanContractError();
    const reader = response.body.getReader(); const decoder = new TextDecoder();
    let bytes = 0; let text = "";
    try {
      for (;;) {
        const chunk = await reader.read(); this.#assert(snapshot);
        if (chunk.done) break;
        bytes += chunk.value.byteLength;
        if (bytes > maxBytes) throw new Error("Réponse trop volumineuse. Réduisez la sélection ou contactez l’opérateur.");
        text += decoder.decode(chunk.value, { stream: true });
      }
      text += decoder.decode();
      return { value: this.#parseText(text), bytes };
    } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
  }
  /** `body` makes the request a POST of that JSON text: a delegated change, never a read. */
  async #request(path: string, snapshot: Snapshot, signal?: AbortSignal, stream = false, body?: string): Promise<Response> {
    this.#assert(snapshot);
    if (!/^\/api\/v1\/human\/(identity|resources|events|export|stream|brokers|workers|summary)(?:\/[0-9a-f-]{36})?(?:\?[^#]*)?$/.test(path) &&
      !/^\/api\/v1\/human\/federation\/workers(?:\?[^#]*)?$/.test(path) && !/^\/api\/v1\/human\/events\/recent(?:\?[^#]*)?$/.test(path) &&
      !/^\/api\/v1\/human\/owned\/brokers(?:\?[^#]*)?$/.test(path) &&
      !/^\/api\/v1\/human\/admin\/(memberships|quota|usage|configuration|grants)(?:\?[^#]*)?$/.test(path) &&
      !(body !== undefined && /^\/api\/v1\/human\/admin\/memberships\/[0-9a-fA-F-]{36}$/.test(path)) &&
      !(body !== undefined && /^\/api\/v1\/human\/owned\/sessions\/(prepare|launch)$/.test(path)) &&
      !(body === undefined && /^\/api\/v1\/human\/owned\/sessions\/preparations\/[0-9a-f-]{36}$/.test(path))) throw new HumanContractError();
    // The durable read budget bounds reads; a change is not a read and keeps its own refusals.
    if (this.#readBudgetStopped && body === undefined) {
      if (!this.#operatorReadAllowed || stream) throw new HumanApiError(429, "audit_read_budget_exhausted", "Lecture automatique suspendue : budget durable de lecture atteint. Contactez l’opérateur.", "contact_operator");
      this.#operatorReadAllowed = false; snapshot.operatorRetry = true;
    }
    const headers: Record<string, string> = { Authorization: `Bearer ${this.#token}`, Accept: stream ? "text/event-stream" : "application/json",
      ...(body !== undefined ? { "Content-Type": "application/json" } : {}) };
    if (snapshot.context) headers["X-AiGuard-Context"] = snapshot.context.id;
    const signals = [this.#lifetime.signal, this.#scope.signal];
    if (signal) signals.push(signal);
    if (!stream) signals.push(AbortSignal.timeout(15000));
    const fetcher = this.#fetcher;
    const response = await fetcher(path, { method: body === undefined ? "GET" : "POST", headers, ...(body !== undefined ? { body } : {}),
      credentials: "omit", cache: "no-store", redirect: "error", signal: AbortSignal.any(signals) });
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
    const profiles = this.#identity?.read_profiles;
    const ownedWorkers = profiles?.worker_registry === OWNED_WORKER_PROFILE && path.startsWith("/api/v1/human/workers?");
    const ownedQuotaUsage = profiles !== undefined && body === undefined && /^\/api\/v1\/human\/admin\/(?:quota|usage)(?:\?|$)/.test(path);
    if (response.status === 403 && (ownedWorkers || ownedQuotaUsage || path === "/api/v1/human/identity" ||
        /^\/api\/v1\/human\/(?:federation\/workers|owned\/brokers)(?:\?|$)/.test(path))) {
      // These reads must retire displayed data as soon as refusal headers arrive.
      // No error body or cancellation acknowledgement may delay that refusal.
      // Other routes keep their existing structured context and stream errors.
      const error = new HumanApiError(403, "permission_denied",
        "Consultation refusée par le contrôle (HTTP 403). Le motif détaillé n’a pas été lu. Relisez vos accès ou faites vérifier vos permissions.", "review_permissions");
      void response.body?.cancel().catch(() => {}); this.#reject(error, ownedQuotaUsage);
    }
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
  #finishRead<T>(value: T, snapshot: Snapshot, signal?: AbortSignal): T {
    this.#assert(snapshot);
    if (signal?.aborted) throw new HumanSuperseded();
    // Recovery commits only after the endpoint has validated the complete requested selection.
    if (snapshot.operatorRetry) this.#readBudgetStopped = false;
    return value;
  }
  async identity(): Promise<HumanIdentity> {
    const snapshot = this.#snapshot(false); const request = ++this.#identityRequest;
    const body = await this.#json(await this.#request("/api/v1/human/identity", snapshot), snapshot);
    this.#assert(snapshot); if (request !== this.#identityRequest) throw new HumanSuperseded();
    const identity = parseHumanIdentity(body.value);
    const profilesChanged = this.#identity !== null && JSON.stringify(this.#identity.read_profiles) !== JSON.stringify(identity.read_profiles);
    this.#identity = structuredClone(identity);
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
    const selected = this.#context;
    const current = selected && identity.contexts.find((context) => context.id === selected.id);
    if (selected && (profilesChanged || !current || current.authorization_version !== selected.authorization_version ||
      current.membership_revision !== selected.membership_revision || current.tenant_id !== selected.tenant_id ||
      current.organization_scope !== selected.organization_scope || current.mode !== selected.mode ||
      [...current.capabilities].sort().join(",") !== [...selected.capabilities].sort().join(",") ||
      // The same context can carry a new catalogue (an operator migration changes an availability): its views were built on the old one.
      JSON.stringify(current.operations) !== JSON.stringify(selected.operations) || Date.parse(current.expires_at) <= Date.now())) {
      const error = new HumanApiError(403, "context_revoked", "Les accès ont changé. Choisissez à nouveau un contexte autorisé.", "select_context");
      this.clearContext(); this.#handlers.onContextInvalidated(error, identity);
    }
    if (snapshot.operatorRetry) this.#readBudgetStopped = false;
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
  async resources(cursor: string | null = null, signal?: AbortSignal, limit?: number) {
    const snapshot = this.#snapshot(true);
    const body = await this.#json(await this.#request(humanUrl("resources", { view: "metadata", cursor, limit }), snapshot, signal), snapshot);
    return this.#finishRead(this.#validatePage(parseResourcePage(body.value), snapshot), snapshot, signal);
  }
  async brokers(cursor: string | null = null, signal?: AbortSignal, limit?: number) {
    const snapshot = this.#snapshot(true);
    const owned = this.#identity?.read_profiles?.broker_registry === "human_broker_registry_owned_v1";
    const body = await this.#json(await this.#request(owned ? humanOwnedBrokersUrl(cursor, limit) : humanBrokersUrl(cursor, limit), snapshot, signal), snapshot);
    return this.#finishRead(this.#validatePage(owned ? parseHumanOwnedBrokerPage(body.value) : parseHumanBrokerPage(body.value), snapshot), snapshot, signal);
  }
  /**
   * A read the selected context's catalogue announces as not served (source profile, store marker or build) is refused
   * here, before any request, whatever its permission: no historical route is substituted. A permission is never
   * decided here; a served read goes to the control, which authorizes it.
   */
  #served(snapshot: Snapshot, operation: HumanOperation) {
    const entry = access(snapshot.context, operation);
    if (!entry || entry.availability === "available") return;
    const code = entry.availability === "profile_unavailable" ? "read_profile_unavailable" : entry.availability;
    throw new HumanApiError(503, code, `${OPERATION_FACTS[operation].label} : ${availabilityText[entry.availability]}`, "contact_operator");
  }
  #anchors(snapshot: Snapshot) {
    const context = snapshot.context;
    if (!context) throw new HumanSuperseded();
    return { contextId: context.id, authorizationVersion: context.authorization_version };
  }
  async workers(cursor: string | null = null, signal?: AbortSignal, limit?: number) {
    const snapshot = this.#snapshot(true);
    this.#served(snapshot, "workers_read");
    const body = await this.#json(await this.#request(humanWorkersUrl(cursor, limit), snapshot, signal), snapshot);
    return this.#finishRead(this.#validatePage(parseHumanWorkerPage(body.value), snapshot), snapshot, signal);
  }
  /** One summary of the selected context; its sections follow the control's decision for this context. */
  async summary(window: SummaryWindow, signal?: AbortSignal): Promise<HumanSummary> {
    const snapshot = this.#snapshot(true);
    this.#served(snapshot, "summary_read");
    const body = await this.#json(await this.#request(humanSummaryUrl(window), snapshot, signal), snapshot, SUMMARY_PAGE_BYTES);
    return this.#finishRead(parseHumanSummary(body.value, this.#anchors(snapshot)), snapshot, signal);
  }
  /** Memberships of the administered organization, one page; the control decides what each entry discloses. */
  async memberships(cursor: string | null = null, signal?: AbortSignal): Promise<MembershipPage> {
    const snapshot = this.#snapshot(true);
    this.#served(snapshot, "memberships_read");
    const body = await this.#json(await this.#request(adminUrls.memberships(cursor), snapshot, signal), snapshot, ADMIN_PAGE_BYTES);
    return this.#finishRead(parseMembershipPage(body.value, this.#anchors(snapshot)), snapshot, signal);
  }
  /** The tenant quota policy and its durable reservations. */
  async quota(signal?: AbortSignal): Promise<HumanQuota> {
    const snapshot = this.#snapshot(true);
    this.#served(snapshot, "quota_read");
    const body = await this.#json(await this.#request(adminUrls.quota(), snapshot, signal), snapshot, ADMIN_PAGE_BYTES);
    return this.#finishRead(parseHumanQuota(body.value, this.#anchors(snapshot)), snapshot, signal);
  }
  /** Terminal allocations observed, method 1, never billable. */
  async usage(cursor: string | null = null, signal?: AbortSignal): Promise<UsagePage> {
    const snapshot = this.#snapshot(true);
    this.#served(snapshot, "usage_read");
    const body = await this.#json(await this.#request(adminUrls.usage(cursor), snapshot, signal), snapshot, ADMIN_PAGE_BYTES);
    return this.#finishRead(parseUsagePage(body.value, this.#anchors(snapshot)), snapshot, signal);
  }
  /** One resource of the organization with its shares, for the administrator who may change them. */
  async grant(resource: ResourceRef, signal?: AbortSignal): Promise<HumanGrant> {
    const snapshot = this.#snapshot(true);
    this.#served(snapshot, "grants_manage");
    const body = await this.#json(await this.#request(adminUrls.grant(resource), snapshot, signal), snapshot, ADMIN_PAGE_BYTES);
    return this.#finishRead(parseHumanGrant(body.value, { ...this.#anchors(snapshot), resource }), snapshot, signal);
  }
  /**
   * A delegated change of one membership. The caller keeps `operation_id` across retries until a definitive answer;
   * a 200 means a new authorization revision, so the selected context is refused afterwards and must be chosen again.
   */
  async changeMembership(membershipId: string, change: MembershipChange, signal?: AbortSignal): Promise<MembershipChangeReceipt> {
    const snapshot = this.#snapshot(true);
    this.#served(snapshot, "memberships_manage");
    const text = membershipChangeBody(change);
    const body = await this.#json(await this.#request(adminUrls.membershipChange(membershipId), snapshot, signal, false, text), snapshot, ADMIN_PAGE_BYTES);
    return this.#finishRead(parseMembershipChangeReceipt(body.value, { operationId: change.operation_id, membershipId }), snapshot, signal);
  }
  /** A delegated replacement of the shares of one bound resource. `owner` lets the body refuse a share to the owner. */
  async changeGrant(change: GrantChange, owner?: Principal, signal?: AbortSignal): Promise<GrantChangeReceipt> {
    const snapshot = this.#snapshot(true);
    this.#served(snapshot, "grants_manage");
    const text = grantChangeBody(change, owner);
    const body = await this.#json(await this.#request(adminUrls.grantChange(), snapshot, signal, false, text), snapshot, ADMIN_PAGE_BYTES);
    return this.#finishRead(parseGrantChangeReceipt(body.value, { operationId: change.operation_id, resource: change.resource }), snapshot, signal);
  }
  /** Effective configuration, types and modes only. */
  async configuration(signal?: AbortSignal): Promise<HumanConfiguration> {
    const snapshot = this.#snapshot(true);
    this.#served(snapshot, "configuration_read");
    const body = await this.#json(await this.#request(adminUrls.configuration(), snapshot, signal), snapshot, ADMIN_PAGE_BYTES);
    return this.#finishRead(parseHumanConfiguration(body.value, this.#anchors(snapshot)), snapshot, signal);
  }
  /**
   * Retain one nominative launch preparation on the caller's own owned source. The control revalidates the personal
   * launch context and commits the mandatory audit before the receipt; prepare creates no grant, reservation or start
   * claim. The caller keeps `operation_id` across retries: an exact replay returns the retained receipt, a divergent
   * intent under the same operation is a conflict.
   */
  async sessionPrepare(request: SessionPrepareRequest, signal?: AbortSignal): Promise<SessionPrepareReceipt> {
    const snapshot = this.#snapshot(true);
    this.#served(snapshot, "session_launch_prepare");
    const text = sessionPrepareBody(request);
    const body = await this.#json(await this.#request(sessionLaunchUrls.prepare(), snapshot, signal, false, text), snapshot, ADMIN_PAGE_BYTES);
    return this.#finishRead(parseSessionPrepareReceipt(body.value, { operationId: request.operation_id, launchTargetId: request.launch_target_id }), snapshot, signal);
  }
  /**
   * Commit one retained preparation as a real nominative session launch. The control commits the launch audit, the
   * nominative grant and the retained conversation binding together before the receipt. Accepted launch is not a
   * running worker: the supervised admission and the runtime-observed start remain the broker's separate effect.
   */
  async sessionLaunch(request: SessionLaunchRequest, signal?: AbortSignal): Promise<SessionLaunchReceipt> {
    const snapshot = this.#snapshot(true);
    this.#served(snapshot, "session_launch");
    const text = sessionLaunchBody(request);
    const body = await this.#json(await this.#request(sessionLaunchUrls.launch(), snapshot, signal, false, text), snapshot, ADMIN_PAGE_BYTES);
    return this.#finishRead(parseSessionLaunchReceipt(body.value, { operationId: request.operation_id, launchTargetId: request.launch_target_id }), snapshot, signal);
  }
  /** Read one retained preparation receipt for a still-authorized caller; revocation closes the read. */
  async sessionPreparation(operationId: string, signal?: AbortSignal): Promise<SessionPrepareReceipt> {
    const snapshot = this.#snapshot(true);
    this.#served(snapshot, "session_launch_prepare");
    const body = await this.#json(await this.#request(sessionLaunchUrls.preparation(operationId), snapshot, signal), snapshot, ADMIN_PAGE_BYTES);
    return this.#finishRead(parseSessionPrepareReceipt(body.value, { operationId }), snapshot, signal);
  }
  async federationWorkers(selection: HumanFederationSelection = {}, signal?: AbortSignal) {
    const snapshot = this.#snapshot(true);
    this.#served(snapshot, "federation_read");
    const body = await this.#json(await this.#request(humanFederationWorkersUrl(selection), snapshot, signal), snapshot, HUMAN_FEDERATION_PAGE_BYTES);
    const page = parseHumanFederationWorkerPage(body.value);
    this.#validatePage(page.workers, snapshot); this.#validatePage(page.brokers, snapshot);
    return this.#finishRead(page, snapshot, signal);
  }
  async events(selection: HumanSelection, signal?: AbortSignal): Promise<HumanEventPage> {
    const snapshot = this.#snapshot(true);
    const body = await this.#json(await this.#request(humanUrl("events", selection), snapshot, signal), snapshot);
    const page = this.#validatePage(parseHumanEventPage(body.value), snapshot);
    const ref = selection.resource;
    if (page.view !== selection.view || ref && page.items.some((item) =>
      item.metadata.resource.kind !== ref.kind || item.metadata.resource.resource_id !== ref.resource_id)) throw new HumanContractError();
    return this.#finishRead(page, snapshot, signal);
  }
  /** Newest authorized events first. The first page carries the position from which `listen` continues without replay. */
  async recentEvents(selection: HumanSelection, signal?: AbortSignal): Promise<HumanRecentEventPage> {
    const snapshot = this.#snapshot(true);
    let response: Response;
    try { response = await this.#request(humanRecentEventsUrl(selection), snapshot, signal); }
    catch (error) {
      if (error instanceof HumanApiError && error.status === 404 && (!selection.resource || !this.#recentServed))
        throw new HumanRecentUnavailable(!!selection.resource);
      throw error;
    }
    const body = await this.#json(response, snapshot);
    const page = this.#validatePage(parseHumanRecentEventPage(body.value, selection.cursor == null), snapshot);
    const ref = selection.resource;
    if (page.view !== selection.view || ref && page.items.some((item) =>
      item.metadata.resource.kind !== ref.kind || item.metadata.resource.resource_id !== ref.resource_id)) throw new HumanContractError();
    this.#finishRead(page, snapshot, signal);
    this.#recentServed = true;
    return page;
  }
  async event(id: string, view: HumanSelection["view"], signal?: AbortSignal) {
    if (!uuid(id)) throw new HumanContractError();
    const snapshot = this.#snapshot(true);
    const body = await this.#json(await this.#request(`/api/v1/human/events/${id}?view=${view}`, snapshot, signal), snapshot);
    const page = this.#validatePage(parseHumanEventPage(body.value), snapshot);
    if (page.view !== view || page.items.length !== 1 || page.has_more || page.items[0]?.metadata.id !== id) throw new HumanContractError();
    return this.#finishRead(page, snapshot, signal);
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
        return this.#finishRead(blob, snapshot, cancel);
      }
      cursor = page.next_cursor;
      if (!cursor || cursors.has(cursor)) throw new HumanContractError(); cursors.add(cursor);
    }
    throw new Error("Export arrêté après 25 pages. Aucune sélection partielle n’a été téléchargée. Choisissez une ressource plus précise.");
  }
  /** `onPage` also receives the position after its batch: a caller whose stream was refused for a while can restart exactly there. */
  async listen(selection: HumanSelection, signal: AbortSignal, onPage: (page: HumanEventPage, position: string) => void,
    onStatus: (status: "connecting" | "live" | "retrying" | "closed") => void): Promise<void> {
    // Without a start position the control replays the authorized history from its beginning.
    const snapshot = this.#snapshot(true); let cursor: string | null = selection.cursor ?? null; let attempt = 0;
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
          cursor = message.id; onPage(page, message.id); attempt = 0;
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
