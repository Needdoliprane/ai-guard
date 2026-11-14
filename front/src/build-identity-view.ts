import { el, button } from "./dom.ts";
import { BuildIdentityStore, BuildIdentityAnnouncements, identityRows, identitySummary } from "./build-identity.ts";
import type { FrontendIdentity } from "./build-identity.ts";

/** Remains outside authenticated collections, including while those are purged. */
export function buildIdentityFooter(frontend: FrontendIdentity) {
  const store = new BuildIdentityStore();
  const summaryText = el("span", "build-summary-state");
  const summary = el("summary", "build-summary", el("strong", "", `ai-guard ${frontend.version}`),
    el("span", "build-summary-id", `Build : ${frontend.build.id ?? "inconnu"}`), el("span", "", "Diagnostic"), summaryText);
  const rows = el("dl", "build-rows");
  const announcement = el("p", "sr-only");
  announcement.setAttribute("role", "status");
  let busy = true;
  const retry = button("Relire les versions", "button button-secondary build-refresh", () => { if (!busy) void store.refresh(); }, "refresh");
  const details = el("details", "build-details", summary, rows,
    el("p", "build-note", "Identités de build descriptives. Elles ne prouvent ni disponibilité, ni compatibilité, ni qualification."), retry);
  const footer = el("footer", "build-identity", details, announcement);
  footer.setAttribute("aria-label", "Versions d’ai-guard");
  const announcements = new BuildIdentityAnnouncements(frontend, (message) => { announcement.textContent = message; });
  store.subscribe((state) => {
    const comparison = identitySummary(frontend, state);
    summaryText.textContent = comparison.message;
    summaryText.classList.toggle("build-mismatch", comparison.mismatch);
    busy = state.dashboard.kind === "loading" || state.gateway.kind === "loading";
    retry.setAttribute("aria-disabled", String(busy));
    rows.replaceChildren(...identityRows(frontend, state).map((row) => el("div", "build-row",
      el("dt", "", row.label), el("dd", "", el("strong", "", row.version ?? "Version inconnue"),
        el("span", "build-reading", row.message), row.build ? el("span", "build-metadata",
          el("span", "", `Build : ${row.build.id ?? "inconnu"}`), el("span", "", `Révision : ${row.build.revision ?? "inconnue"}`)) : null))));
    announcements.update(state);
  });
  void store.refresh();
  return { node: footer, close: () => store.close() };
}
