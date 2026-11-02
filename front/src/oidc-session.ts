/** Browser OIDC transport. All tokens are private, ephemeral memory values. */
export interface OidcStatus { version: 1; enabled: boolean; issuer?: string; provider_label?: string }
interface Tokens {
  version: 1; authentication_id: string; generation: number; token_type: "Bearer";
  access_token: string; refresh_token?: string; expires_at: string; expires_in_seconds: number;
}
export interface OidcLogout {
  version: 1; local_session: "closed" | "unknown";
  provider_logout: "accepted" | "failed" | "unavailable" | "unsupported" | "not_attempted";
  audit: "recorded" | "unavailable";
}
interface SessionEvents { onChange?: () => void; onClose?: (message: string) => void }
const errors: Record<string, string> = {
  oidc_not_configured: "La connexion SSO n’est pas activée sur ce contrôle.",
  authentication_required: "Connectez-vous de nouveau au fournisseur d’identité.",
  credential_invalid: "La preuve d’accès a été refusée. Connectez-vous de nouveau.",
  credential_expired: "La session a expiré. Connectez-vous de nouveau.",
  permission_denied: "Ce compte ne dispose d’aucun accès autorisé. Contactez l’opérateur.",
  origin_denied: "L’origine de connexion n’est pas autorisée. Vérifiez l’adresse du dashboard.",
  session_conflict: "Cette session a déjà changé. Connectez-vous de nouveau.",
  capacity_exhausted: "La capacité de connexion est atteinte. Réessayez après expiration des sessions.",
  audit_unavailable: "L’audit obligatoire est indisponible. Contactez l’opérateur.",
  identity_unavailable: "Le contrôle d’identité est indisponible. Réessayez.",
  provider_unavailable: "Le fournisseur d’identité est indisponible. Connectez-vous de nouveau.",
  login_cancelled: "La connexion a été annulée auprès du fournisseur.",
  invalid_request: "La réponse de connexion est incompatible. Vérifiez les versions du contrôle et du dashboard.",
  session_closed: "La session locale est fermée.",
};
export class OidcSessionError extends Error {
  readonly code: string;
  constructor(code: string) {
    super(errors[code] ?? errors.identity_unavailable);
    this.name = "OidcSessionError"; this.code = Object.hasOwn(errors, code) ? code : "identity_unavailable";
  }
}
const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown, max: number): value is string => typeof value === "string" && value.length > 0 && value.length <= max && !/[\u0000-\u0020\u007f-\u009f]/.test(value);
const uuid = (value: unknown) => typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);
const keys = (value: Record<string, unknown>, allowed: string[]) => Object.keys(value).every((key) => allowed.includes(key));
function tokenDelivery(value: unknown): Tokens {
  if (!object(value) || !keys(value, ["version", "authentication_id", "generation", "token_type", "access_token", "refresh_token", "expires_at", "expires_in_seconds"]) ||
    value.version !== 1 || !uuid(value.authentication_id) || !Number.isInteger(value.generation) || Number(value.generation) < 1 || Number(value.generation) > 4294967295 ||
    value.token_type !== "Bearer" || !text(value.access_token, 16384) || (value.refresh_token !== undefined && !text(value.refresh_token, 16384)) ||
    typeof value.expires_at !== "string" || !/^\d{4}-\d\d-\d\dT.+(?:Z|[+-]\d\d:\d\d)$/.test(value.expires_at) ||
    !Number.isInteger(value.expires_in_seconds) || Number(value.expires_in_seconds) < 1 || Number(value.expires_in_seconds) > 86400 ||
    !Number.isFinite(Date.parse(value.expires_at)) || Date.parse(value.expires_at) <= Date.now() ||
    Date.parse(value.expires_at) > Date.now() + Number(value.expires_in_seconds) * 1000 + 5000) throw new OidcSessionError("invalid_request");
  return value as unknown as Tokens;
}

export class OidcSession {
  #fetch: typeof fetch;
  #events: SessionEvents;
  #tokens: Tokens | null = null;
  #closed = false;
  #generation = 0;
  #scope = new AbortController();
  #renewal: Promise<void> | null = null;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #status: OidcStatus | null = null;
  constructor(fetcher: typeof fetch = fetch, events: SessionEvents = {}) { this.#fetch = fetcher; this.#events = events; }
  get accessToken(): string | null { return this.#tokens?.access_token ?? null; }
  async #request(path: string, method: "GET" | "POST", body?: object, bearer?: string, logout = false): Promise<unknown> {
    const timeout = new AbortController(); const timer = setTimeout(() => timeout.abort(), 8000);
    try {
      const headers: Record<string, string> = { Accept: "application/json" };
      if (body) headers["Content-Type"] = "application/json";
      if (bearer) headers.Authorization = `Bearer ${bearer}`;
      const response = await this.#fetch(`/auth/oidc/${path}`, {
        method, headers, ...(body ? { body: JSON.stringify(body) } : {}), cache: "no-store", redirect: "error", credentials: "same-origin",
        signal: logout ? timeout.signal : AbortSignal.any([timeout.signal, this.#scope.signal]),
      });
      const reader = response.body?.getReader(); const chunks: Uint8Array[] = []; let size = 0;
      if (!reader) throw new OidcSessionError("invalid_request");
      try {
        while (true) { const part = await reader.read(); if (part.done) break;
          size += part.value.byteLength; if (size > 65536) throw new OidcSessionError("invalid_request"); chunks.push(part.value); }
      } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
      const bytes = new Uint8Array(size); let offset = 0; for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
      let value: unknown; try { value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)); } catch { throw new OidcSessionError("invalid_request"); }
      if (!response.ok) {
        const code = object(value) && object(value.error) && typeof value.error.code === "string" ? value.error.code : "identity_unavailable";
        throw new OidcSessionError(code);
      }
      return value;
    } catch (error) { throw error instanceof OidcSessionError ? error : new OidcSessionError("identity_unavailable"); }
    finally { clearTimeout(timer); }
  }
  async status(): Promise<OidcStatus> {
    const value = await this.#request("status", "GET");
    if (!object(value) || value.version !== 1 || typeof value.enabled !== "boolean" ||
      !keys(value, value.enabled ? ["version", "enabled", "issuer", "provider_label"] : ["version", "enabled"]) ||
      (value.enabled && (!text(value.issuer, 2048) || typeof value.provider_label !== "string" || value.provider_label.length > 128))) throw new OidcSessionError("invalid_request");
    this.#status = value as unknown as OidcStatus; return this.#status;
  }
  async start(): Promise<string> {
    if (this.#closed) throw new OidcSessionError("session_closed");
    const status = this.#status ?? await this.status(); if (!status.enabled) throw new OidcSessionError("oidc_not_configured");
    const value = await this.#request("start", "POST", { version: 1 });
    if (!object(value) || value.version !== 1 || !keys(value, ["version", "authorization_url"]) || !text(value.authorization_url, 8192)) throw new OidcSessionError("invalid_request");
    let url: URL; let issuer: URL;
    try { url = new URL(value.authorization_url); issuer = new URL(status.issuer!); } catch { throw new OidcSessionError("invalid_request"); }
    if ((url.protocol !== "https:" && !(url.protocol === "http:" && issuer.protocol === "http:" && url.origin === issuer.origin)) || url.username || url.password || url.hash) throw new OidcSessionError("invalid_request");
    return value.authorization_url;
  }
  async redeem(): Promise<void> {
    if (this.#closed || this.#tokens) throw new OidcSessionError("session_closed");
    const generation = this.#generation;
    try { const tokens = tokenDelivery(await this.#request("tokens", "POST", { version: 1 }));
      if (this.#closed || generation !== this.#generation || tokens.generation !== 1) throw new OidcSessionError("session_closed"); this.#install(tokens);
    } catch (error) { this.close(error instanceof OidcSessionError ? error.message : undefined); throw error; }
  }
  #install(tokens: Tokens): void {
    this.#tokens = tokens; this.#generation++; clearTimeout(this.#timer);
    const remaining = Math.min(Date.parse(tokens.expires_at) - Date.now(), tokens.expires_in_seconds * 1000);
    const delay = tokens.refresh_token ? Math.max(0, remaining - Math.min(15000, remaining / 4)) : remaining;
    this.#timer = setTimeout(() => { if (tokens.refresh_token) void this.refresh().catch(() => {}); else this.close(errors.credential_expired); }, delay);
    this.#events.onChange?.();
  }
  refresh(): Promise<void> {
    if (this.#renewal) return this.#renewal;
    const current = this.#tokens; const generation = this.#generation;
    if (this.#closed || !current?.refresh_token || current.generation >= 4294967295 || Date.parse(current.expires_at) <= Date.now()) {
      this.close(errors.credential_expired); return Promise.reject(new OidcSessionError("credential_expired"));
    }
    this.#renewal = (async () => {
      try {
        const next = tokenDelivery(await this.#request("refresh", "POST", { version: 1, authentication_id: current.authentication_id,
          generation: current.generation, refresh_token: current.refresh_token }, current.access_token));
        if (this.#closed || generation !== this.#generation) throw new OidcSessionError("session_closed");
        if (next.authentication_id !== current.authentication_id || next.generation !== current.generation + 1) throw new OidcSessionError("session_conflict");
        this.#install(next);
      } catch (error) { this.close(error instanceof OidcSessionError ? error.message : undefined); throw error; }
      finally { this.#renewal = null; }
    })(); return this.#renewal;
  }
  async logout(): Promise<OidcLogout> {
    const current = this.#tokens; this.close();
    if (!current) return { version: 1, local_session: "unknown", provider_logout: "not_attempted", audit: "unavailable" };
    const value = await this.#request("logout", "POST", { version: 1, authentication_id: current.authentication_id, generation: current.generation }, current.access_token, true);
    if (!object(value) || value.version !== 1 || !keys(value, ["version", "local_session", "provider_logout", "audit"]) ||
      !["closed", "unknown"].includes(String(value.local_session)) || !["accepted", "failed", "unavailable", "unsupported", "not_attempted"].includes(String(value.provider_logout)) ||
      !["recorded", "unavailable"].includes(String(value.audit))) throw new OidcSessionError("invalid_request");
    return value as unknown as OidcLogout;
  }
  close(message = errors.session_closed): void {
    if (this.#closed) return; this.#closed = true; this.#tokens = null; this.#generation++; clearTimeout(this.#timer); this.#scope.abort(); this.#events.onClose?.(message!);
  }
}
