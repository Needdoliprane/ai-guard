import { consumeAuditStream } from "./sse.ts";
import { isControlReadPath } from "./control.ts";
import type { AuditEvent, EventFilters, ViewerIdentity } from "./types.ts";

export class IdentityError extends Error {
  constructor() {
    super("Identité de lecture invalide ou incompatible. Vérifiez les versions du dashboard et du collecteur.");
  }
}

export class ApiError extends Error {
  status: number;
  constructor(status: number) {
    super(
      status === 401 || status === 403
        ? "Jeton refusé. Vérifiez le jeton de lecture."
        : `Le collecteur a répondu avec une erreur HTTP ${status}.`,
    );
    this.status = status;
  }
}

export type StreamStatus = "connecting" | "live" | "retrying" | "closed";
export class ApiClient {
  #token: string;
  #fetcher: typeof fetch;
  #onAuthError: (() => void) | undefined;
  #controller = new AbortController();
  constructor(token: string, fetcher: typeof fetch = fetch, onAuthError?: () => void) {
    this.#token = token;
    this.#fetcher = fetcher;
    this.#onAuthError = onAuthError;
  }
  close() {
    this.#controller.abort();
    this.#token = "";
  }
  async #request(path: string, stream = false, cursor = 0, onAuthError = this.#onAuthError): Promise<Response> {
    if (!this.#token || this.#controller.signal.aborted)
      throw new Error("Session fermée.");
    if (
      path !== "/api/v1/identity" && !isControlReadPath(path) && !/^\/api\/(overview|agents|sandboxes|events|stream|export|integrity)(?:\?[^#]*)?$/.test(
        path,
      )
    )
      throw new Error("Route de lecture non autorisée.");
    const headers: Record<string, string> = {
      Authorization: `Bearer ${this.#token}`,
      Accept: stream ? "text/event-stream" : "application/json",
    };
    if (stream && cursor > 0) headers["Last-Event-ID"] = String(cursor);
    const fetcher = this.#fetcher;
    const response = await fetcher(path, {
      method: "GET",
      headers,
      credentials: "omit",
      redirect: "error",
      cache: "no-store",
      signal: stream
        ? this.#controller.signal
        : AbortSignal.any([
            this.#controller.signal,
            AbortSignal.timeout(15000),
          ]),
    });
    if (!response.ok) {
      if ((response.status === 401 || response.status === 403) && !this.#controller.signal.aborted) {
        this.close();
        onAuthError?.();
      }
      try {
        await response.body?.cancel();
      } catch {
        // Closing the session may already have aborted the response body.
      }
      throw new ApiError(response.status);
    }
    return response;
  }
  async get<T>(path: string): Promise<T> {
    return (await this.#request(path)).json() as Promise<T>;
  }
  async identity(): Promise<ViewerIdentity> {
    const value = await this.get<unknown>("/api/v1/identity");
    if (!value || typeof value !== "object" || Array.isArray(value)) throw new IdentityError();
    const identity = value as Record<string, unknown>;
    if (identity.role !== "viewer") throw new ApiError(403);
    if (typeof identity.tenant_id !== "string" || !/^[A-Za-z0-9_-]{1,128}$/.test(identity.tenant_id))
      throw new IdentityError();
    return { tenant_id: identity.tenant_id, role: "viewer" };
  }
  async export(filters: EventFilters = {}): Promise<Blob> {
    const query = new URLSearchParams({ format: "jsonl" });
    for (const name of [
      "q",
      "agent_id",
      "sandbox_id",
      "project_id",
      "kind",
      "decision",
      "from",
      "to",
    ] as const) {
      const value = filters[name];
      if (value) query.set(name, value);
    }
    return (await this.#request(`/api/export?${query.toString()}`)).blob();
  }
  async listen(
    cursor: number,
    onEvent: (event: AuditEvent) => void,
    onStatus: (status: StreamStatus) => void,
    onAuthError: () => void,
  ): Promise<void> {
    let attempt = 0;
    while (!this.#controller.signal.aborted) {
      onStatus(attempt ? "retrying" : "connecting");
      try {
        const response = await this.#request("/api/stream", true, cursor, this.#onAuthError ?? onAuthError);
        if (
          !response.body ||
          !response.headers.get("content-type")?.includes("text/event-stream")
        )
          throw new Error("Flux temps réel indisponible.");
        onStatus("live");
        await consumeAuditStream(response.body, cursor, (event) => {
          onEvent(event);
          cursor = event.seq;
          attempt = 0;
        });
      } catch (error) {
        if (
          error instanceof ApiError &&
          (error.status === 401 || error.status === 403)
        ) {
          return;
        }
        if (this.#controller.signal.aborted) break;
      }
      if (this.#controller.signal.aborted) break;
      attempt++;
      onStatus("retrying");
      await new Promise<void>((resolve) => {
        const finish = () => {
          clearTimeout(timer);
          this.#controller.signal.removeEventListener("abort", finish);
          resolve();
        };
        const timer = setTimeout(
          finish,
          Math.min(30000, 1000 * 2 ** Math.min(attempt - 1, 5)),
        );
        this.#controller.signal.addEventListener("abort", finish, {
          once: true,
        });
      });
    }
    onStatus("closed");
  }
}

export function eventUrl(
  filters: EventFilters,
  before?: number | null,
): string {
  const query = new URLSearchParams({ limit: "100" });
  for (const [key, value] of Object.entries(filters))
    if (value) query.set(key, value);
  if (before !== undefined && before !== null)
    query.set("before", String(before));
  return `/api/events?${query.toString()}`;
}
