// Editors of the delegated membership and share changes. DOM only: what is offered, what a change does and the life
// of its request come from delegation-model.ts; Rust authorizes and applies every change.
// Each editor is a modal detail: a step to edit, a step that lists the exact change, then one request whose
// operation identifier survives retries until the control answers for good.
import { button, el } from "./dom.ts";
import { formatDate } from "./model.ts";
import { HumanApiError, HumanSuperseded } from "./human-api.ts";
import type { Capability, HumanContext, Principal, ResourceRef } from "./human-model.ts";
import { SHARE_PERMISSIONS, type GrantChange, type GrantChangeReceipt, type HumanGrant, type Membership, type MembershipChange,
  type MembershipChangeReceipt, type MembershipPage, type SharePermission } from "./admin-model.ts";
import { CAPABILITY_LABELS, CHANGE_PHASE_TEXT, CHOICE_REASON, ChangeFlow, MEMBERSHIP_PROBLEM_TEXT, PERMISSION_LABELS, SHARE_PROBLEM_TEXT,
  canonical, capabilityChoices, draftHolds, membershipChangeKey, membershipDiff, membershipDraft, membershipProblems, permissionAddable, shareCandidates, shareDiff,
  shareProblems, toggleCapability, type MembershipDraft, type ShareDraft } from "./delegation-model.ts";
import { principalKey } from "./supervision-model.ts";

export function principalLabel(principal: Principal) {
  return principal.kind === "human_local" ? `Compte local ${principal.account_id.slice(0, 8)}` : `OIDC ${principal.subject}`;
}
const list = (items: string[], none: string) => items.length ? items.join(", ") : none;

/** Shared shell: a modal detail with its own status region, closed by Escape, its button or its host. */
abstract class EditorDialog {
  readonly dialog: HTMLDialogElement;
  protected body = el("div", "detail-body dg-body");
  protected status = el("p", "dg-status");
  protected abort = new AbortController();
  protected step: "edit" | "confirm" = "edit";
  #trigger: HTMLElement | null = null; #closed = false; #onClosed: () => void;
  constructor(title: string, subtitle: string, onClosed: () => void) {
    this.#onClosed = onClosed;
    this.dialog = el("dialog", "detail-dialog dg-dialog");
    const heading = el("h2", "", title); heading.id = `dg-title-${Math.random().toString(36).slice(2)}`;
    this.dialog.setAttribute("aria-labelledby", heading.id);
    const close = button("", "icon-button", () => this.close(), "x"); close.setAttribute("aria-label", "Fermer l’éditeur");
    this.status.setAttribute("role", "status"); this.status.setAttribute("aria-live", "polite");
    this.dialog.append(el("div", "detail-header", el("div", "", el("p", "eyebrow", "CHANGEMENT DÉLÉGUÉ"), heading, el("p", "muted", subtitle)), close), this.body);
    this.dialog.addEventListener("close", () => this.close());
  }
  get closed() { return this.#closed; }
  open() {
    this.#trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    document.body.append(this.dialog); this.render(); this.dialog.showModal(); this.focusStep();
  }
  /** Withdraws the editor and its draft, aborts its request, and gives the focus back to what opened it. */
  close() {
    if (this.#closed) return;
    this.#closed = true; this.abort.abort();
    if (this.dialog.open) this.dialog.close();
    this.dialog.remove();
    if (this.#trigger?.isConnected) this.#trigger.focus({ preventScroll: true });
    this.#onClosed();
  }
  protected abstract render(): void;
  /** Rebuilds the body; the control that had the focus keeps it when it still exists. */
  protected paint(...children: (HTMLElement | string | null)[]) {
    if (this.#closed) return;
    const key = (document.activeElement as HTMLElement | null)?.dataset?.focusKey;
    this.body.replaceChildren(...children.filter((child): child is HTMLElement | string => child !== null), this.status);
    const same = key ? this.body.querySelector<HTMLElement>(`[data-focus-key="${CSS.escape(key)}"]`) : null;
    if (same && !same.matches(":disabled")) same.focus({ preventScroll: true });
  }
  protected focusStep() {
    const target = this.body.querySelector<HTMLElement>(".dg-step-title") ?? this.body.querySelector<HTMLElement>("input:not(:disabled), button:not(:disabled)");
    target?.focus({ preventScroll: true });
  }
  protected say(text: string) { if (this.status.textContent !== text) this.status.textContent = text; }
  protected action(label: string, key: string, run: () => void, disabled = false, primary = false) {
    const node = button(label, primary ? "button button-primary" : "button button-secondary", run);
    node.dataset.focusKey = key; node.disabled = disabled; return node;
  }
  protected stepTitle(text: string) { const node = el("h3", "dg-step-title", text); node.tabIndex = -1; return node; }
  protected goTo(step: "edit" | "confirm") { this.step = step; this.render(); this.focusStep(); }
}
function check(label: string, key: string, checked: boolean, disabled: boolean, change: (checked: boolean) => void, hint?: string) {
  const input = el("input"); input.type = "checkbox"; input.checked = checked; input.disabled = disabled; input.dataset.focusKey = key;
  input.addEventListener("change", () => change(input.checked));
  const node = el("label", `dg-check${disabled ? " dg-check-disabled" : ""}`, input, el("span", "", label));
  return hint ? el("div", "dg-choice", node, el("p", "dg-hint", hint)) : node;
}

export interface MembershipEditorHost {
  context: HumanContext; membership: Membership;
  send: (change: MembershipChange, signal: AbortSignal) => Promise<MembershipChangeReceipt>;
  onApplied: (receipt: MembershipChangeReceipt) => void;
  /** 409 or 404: the page is read again; this editor keeps its message and sends nothing more. */
  onStale: () => void;
  /** 403: the host withdraws this editor and reads the accesses again. */
  onRefused: () => void;
  onClosed: () => void;
}
export class MembershipEditor extends EditorDialog {
  #host: MembershipEditorHost;
  #draft: MembershipDraft;
  #flow: ChangeFlow<MembershipChangeReceipt>;
  constructor(host: MembershipEditorHost) {
    super("Modifier l’appartenance", `${host.membership.display_name} · ${principalLabel(host.membership.principal)} · révision ${host.membership.revision}`, host.onClosed);
    this.#host = host;
    this.#draft = membershipDraft(host.membership);
    this.#flow = new ChangeFlow<MembershipChangeReceipt>({
      applied: (receipt) => host.onApplied(receipt),
      conflict: () => host.onStale(), missing: () => host.onStale(),
      refused: () => host.onRefused(),
    });
  }
  #key() { return membershipChangeKey(this.#host.membership, this.#draft); }
  protected render() {
    const { context, membership } = this.#host;
    const phase = this.#flow.phase; const settled = ["conflict", "not_found", "invalid"].includes(phase);
    if (phase !== "ready" && phase !== "applied") this.say(CHANGE_PHASE_TEXT[phase as Exclude<typeof phase, "ready" | "applied">]);
    if (this.step === "edit") {
      const problems = membershipProblems(context, membership, this.#draft);
      const choices = capabilityChoices(context, membership).map((choice) => {
        const checked = draftHolds(this.#draft, choice.capability);
        const disabled = settled || (!choice.current && !choice.addable);
        const hint = choice.reason === "control_only" ? CHOICE_REASON.control_only : !choice.current && choice.reason ? CHOICE_REASON[choice.reason] : undefined;
        return check(choice.label, `dg-cap-${choice.capability}`, checked, disabled, (value) => {
          this.#draft = toggleCapability(this.#draft, choice.capability, value); this.render();
        }, hint);
      });
      this.paint(this.stepTitle("Capacités de cette appartenance"),
        el("p", "muted", "Une capacité se conserve ou se retire. Elle ne s’ajoute que si ce contexte la détient et qu’elle n’est pas réservée au contrôle. Le contrôle vérifie de nouveau chaque règle avant tout effet."),
        el("fieldset", "dg-choices", el("legend", "sr-only", "Capacités"), ...choices),
        check("Appartenance active", "dg-active", this.#draft.active, settled, (value) => { this.#draft.active = value; this.render(); },
          membership.active ? "Désactiver retire tous les accès de ce compte dans cette organisation." : "Réactiver réactive les capacités conservées ; chacune doit pouvoir être ajoutée par ce contexte."),
        problems.length ? el("ul", "dg-problems", ...problems.map((problem) => el("li", "", MEMBERSHIP_PROBLEM_TEXT[problem]))) : null,
        el("div", "dg-actions", this.action("Vérifier le changement", "dg-review", () => this.goTo("confirm"), settled || problems.length > 0, true),
          this.action("Fermer", "dg-cancel", () => this.close())));
      return;
    }
    const diff = membershipDiff(membership, this.#draft);
    const label = (capabilities: Capability[]) => capabilities.map((capability) => CAPABILITY_LABELS[capability]);
    const sending = phase === "sending";
    this.paint(this.stepTitle("Vérifier le changement"),
      el("dl", "dg-diff",
        el("div", "", el("dt", "", "Ajoutées"), el("dd", "", list(label(diff.added), "Aucune"))),
        el("div", "", el("dt", "", "Retirées"), el("dd", "", list(label(diff.removed), "Aucune"))),
        el("div", "", el("dt", "", "Appartenance"), el("dd", "", diff.activation === "deactivated" ? "Désactivée" : diff.activation === "reactivated" ? "Réactivée" : this.#draft.active ? "Reste active" : "Reste inactive"))),
      diff.removesAllAccess ? el("p", "control-warning dg-warning", diff.activation === "deactivated" ?
        "La désactivation retire tous les accès de ce compte dans cette organisation." : "Aucune capacité n’est conservée : ce compte ne garde aucun accès dans cette organisation.") : null,
      el("p", "muted", "Le changement crée une nouvelle révision d’autorisation : vos accès sont relus et ce contexte est choisi de nouveau, puis la page est relue."),
      this.#flow.pendingOperation ? el("p", "mono dg-operation", `Opération ${this.#flow.pendingOperation}`) : null,
      el("div", "dg-actions",
        this.action(phase === "unknown" ? "Réessayer la même opération" : "Confirmer le changement", "dg-confirm", () => { void this.#submit(); }, settled, true),
        this.action("Revenir à la modification", "dg-back", () => this.goTo("edit"), sending),
        this.action("Fermer", "dg-cancel", () => this.close())));
    this.body.querySelector('[data-focus-key="dg-confirm"]')?.setAttribute("aria-disabled", String(sending));
  }
  async #submit() {
    if (this.#flow.phase === "sending" || this.closed) return;
    const { membership } = this.#host;
    const capabilities = canonical(this.#draft.capabilities);
    const request = this.#flow.submit(this.#key(), (operationId) => this.#host.send({ operation_id: operationId,
      expected_revision: membership.revision, capabilities, active: this.#draft.active }, this.abort.signal));
    this.render(); await request; if (!this.closed) this.render();
  }
}

export interface GrantEditorHost {
  context: HumanContext; self: Principal; resource: ResourceRef;
  read: (signal: AbortSignal) => Promise<HumanGrant>;
  members: (cursor: string | null, signal: AbortSignal) => Promise<MembershipPage>;
  send: (change: GrantChange, owner: Principal, signal: AbortSignal) => Promise<GrantChangeReceipt>;
  onApplied: (receipt: GrantChangeReceipt) => void;
  onRefused: () => void;
  onClosed: () => void;
}
export class GrantEditor extends EditorDialog {
  #host: GrantEditorHost;
  #grant: HumanGrant | null = null; #draft: ShareDraft[] = [];
  #members: Membership[] = []; #membersNext: string | null = null; #reading = false; #readError = "";
  #candidate = ""; #candidatePermissions: SharePermission[] = ["metadata"];
  #flow: ChangeFlow<GrantChangeReceipt>;
  constructor(host: GrantEditorHost) {
    super("Partages de la ressource", `${host.resource.kind} · ${host.resource.resource_id}`, host.onClosed);
    this.#host = host;
    this.#flow = this.#freshFlow();
  }
  override open() { super.open(); void this.#read(); }
  async #read() {
    if (this.#reading || this.closed) return;
    const afterConflict = this.#flow.phase === "conflict";
    if (afterConflict) {
      this.#grant = null; this.#draft = [];
      this.#members = []; this.#membersNext = null;
      this.#candidate = ""; this.#candidatePermissions = ["metadata"];
      this.step = "edit";
      this.say("Le contrôle a signalé un conflit sur les partages.");
    }
    this.#reading = true; this.#readError = ""; this.render();
    if (afterConflict) this.focusStep();
    try {
      const [grant, members] = await Promise.all([this.#host.read(this.abort.signal), this.#host.members(null, this.abort.signal)]);
      if (this.closed) return;
      this.#grant = grant; this.#draft = grant.shares.map((share) => ({ principal: share.principal, display_name: share.display_name, permissions: [...share.permissions] }));
      this.#members = members.items; this.#membersNext = members.next_cursor;
      if (afterConflict) {
        this.#flow = this.#freshFlow(); this.step = "edit";
        this.say("Partages et membres relus. Vérifiez la nouvelle révision avant de confirmer.");
      }
    } catch (error) {
      if (this.closed || error instanceof HumanSuperseded) return;
      if (error instanceof HumanApiError && error.status === 403) { this.#host.onRefused(); return; }
      this.#readError = error instanceof Error && error.message ? error.message : "Le contrôle n’a pas fourni les données demandées.";
    } finally { this.#reading = false; if (!this.closed) { this.render(); if (afterConflict) this.focusStep(); } }
  }
  #freshFlow() {
    return new ChangeFlow<GrantChangeReceipt>({ applied: (receipt) => this.#host.onApplied(receipt), conflict: () => { void this.#read(); },
      missing: () => {}, refused: () => this.#host.onRefused() });
  }
  async #moreMembers() {
    if (this.step !== "edit" || !this.#membersNext || this.#reading || this.closed) return;
    this.#reading = true; this.render();
    try {
      const page = await this.#host.members(this.#membersNext, this.abort.signal);
      if (!this.closed) { this.#members = [...this.#members, ...page.items]; this.#membersNext = page.next_cursor; }
    } catch (error) {
      if (!this.closed && !(error instanceof HumanSuperseded)) {
        if (error instanceof HumanApiError && error.status === 403) { this.#host.onRefused(); return; }
        this.#readError = error instanceof Error ? error.message : "Lecture impossible.";
      }
    } finally { this.#reading = false; if (!this.closed) this.render(); }
  }
  #key() {
    const shares = [...this.#draft].sort((left, right) => principalKey(left.principal) < principalKey(right.principal) ? -1 : 1)
      .map((share) => [principalKey(share.principal), SHARE_PERMISSIONS.filter((p) => share.permissions.includes(p))]);
    return JSON.stringify([this.#host.resource.kind, this.#host.resource.resource_id, this.#grant?.revision, shares]);
  }
  protected render() {
    const grant = this.#grant; const { context, self } = this.#host;
    if (!grant) {
      const conflict = this.#flow.phase === "conflict";
      this.paint(this.stepTitle(conflict ? "Relire les partages et les membres" : "Partages de la ressource"),
        this.#reading ? el("p", "muted", conflict ? "Relecture des partages et des membres…" : "Lecture des partages…") : null,
        this.#readError ? el("div", "control-error", el("p", "", `${conflict ? "Relecture impossible." : "Lecture impossible."} ${this.#readError}`),
          this.action("Réessayer", "dg-reread", () => { void this.#read(); })) : null,
        el("div", "dg-actions", this.action("Fermer", "dg-cancel", () => this.close())));
      return;
    }
    const phase = this.#flow.phase; const settled = ["not_found", "invalid"].includes(phase);
    if (phase !== "ready" && phase !== "applied" && phase !== "conflict") this.say(CHANGE_PHASE_TEXT[phase as Exclude<typeof phase, "ready" | "applied">]);
    const provenance = el("p", "ov-provenance", `Lu le ${formatDate(grant.evaluated_at, true)} · révision ${grant.revision} · ${grant.active ? "ressource active" : "ressource inactive"}`);
    if (this.step === "edit") {
      const problems = shareProblems(context, grant, this.#draft, self);
      const rows = this.#draft.map((share) => {
        const before = grant.shares.find((item) => principalKey(item.principal) === principalKey(share.principal));
        const permissions = SHARE_PERMISSIONS.map((permission) => {
          const kept = !!before?.permissions.includes(permission);
          const addable = permissionAddable(context, permission);
          return check(PERMISSION_LABELS[permission], `dg-share-${principalKey(share.principal)}-${permission}`, share.permissions.includes(permission),
            settled || (!kept && !addable), (value) => {
              share.permissions = value ? SHARE_PERMISSIONS.filter((p) => p === permission || share.permissions.includes(p)) : share.permissions.filter((p) => p !== permission);
              this.render();
            });
        });
        return el("li", "dg-share", el("div", "", el("strong", "", share.display_name), el("span", "muted", ` · ${principalLabel(share.principal)}`)),
          el("div", "dg-permissions", ...permissions),
          this.action("Retirer ce partage", `dg-remove-${principalKey(share.principal)}`, () => {
            this.#draft = this.#draft.filter((item) => item !== share); this.render(); }, settled));
      });
      const candidates = shareCandidates(this.#members, grant, self, this.#draft);
      if (!candidates.some((member) => member.membership_id === this.#candidate)) this.#candidate = candidates[0]?.membership_id ?? "";
      const select = el("select", "", ...candidates.map((member) => { const option = el("option", "", `${member.display_name} · ${principalLabel(member.principal)}`); option.value = member.membership_id; option.selected = member.membership_id === this.#candidate; return option; }));
      select.id = "dg-candidate"; select.dataset.focusKey = "dg-candidate"; select.disabled = settled || !candidates.length;
      select.addEventListener("change", () => { this.#candidate = select.value; });
      const newPermissions = SHARE_PERMISSIONS.map((permission) => check(PERMISSION_LABELS[permission], `dg-new-${permission}`,
        this.#candidatePermissions.includes(permission), settled || !permissionAddable(context, permission), (value) => {
          this.#candidatePermissions = value ? SHARE_PERMISSIONS.filter((p) => p === permission || this.#candidatePermissions.includes(p)) : this.#candidatePermissions.filter((p) => p !== permission);
          this.render();
        }));
      const addable = this.#candidatePermissions.filter((permission) => permissionAddable(context, permission));
      const field = el("label", "field", "Membre de l’organisation", select); field.htmlFor = select.id;
      this.paint(this.stepTitle("Partages de la ressource"), provenance,
        el("p", "", `Propriétaire : ${grant.owner_display_name} · ${principalLabel(grant.owner)}. Il ne reçoit pas de partage et ne change pas ici.`),
        rows.length ? el("ul", "dg-shares", ...rows) : el("p", "muted", "Aucun partage : seul le propriétaire et les lecteurs de l’organisation y accèdent selon leurs droits."),
        el("section", "dg-add", el("h3", "", "Ajouter un partage"),
          candidates.length ? field : el("p", "muted", "Aucun autre membre actif de cette page ne peut recevoir de partage. Vous-même et le propriétaire n’apparaissent jamais ici."),
          el("div", "dg-permissions", ...newPermissions),
          el("p", "dg-hint", "Une permission ne s’ajoute que si ce contexte détient sa capacité : métadonnées, contenus ou export."),
          el("div", "dg-actions",
            this.action("Ajouter ce partage", "dg-add-share", () => {
              const member = candidates.find((item) => item.membership_id === this.#candidate); if (!member || !addable.length) return;
              this.#draft = [...this.#draft, { principal: member.principal, display_name: member.display_name, permissions: addable }]; this.render();
            }, settled || !candidates.length || !addable.length),
            this.#membersNext ? this.action("Charger d’autres membres", "dg-more-members", () => { void this.#moreMembers(); }, this.#reading) : null)),
        this.#readError ? el("div", "control-error", el("p", "", this.#readError)) : null,
        problems.length ? el("ul", "dg-problems", ...problems.map((problem) => el("li", "", SHARE_PROBLEM_TEXT[problem]))) : null,
        el("div", "dg-actions", this.action("Vérifier le changement", "dg-review", () => this.goTo("confirm"), settled || problems.length > 0 || this.#reading, true),
          this.action("Fermer", "dg-cancel", () => this.close())));
      return;
    }
    const diff = shareDiff(grant, this.#draft);
    const permissions = (items: SharePermission[]) => items.map((permission) => PERMISSION_LABELS[permission]).join(", ");
    const sending = phase === "sending";
    this.paint(this.stepTitle("Vérifier le changement"), provenance,
      el("ul", "dg-diff-list",
        ...diff.added.map((share) => el("li", "", `Nouveau partage : ${share.display_name} · ${permissions(share.permissions)}`)),
        ...diff.changed.map((share) => el("li", "", `${share.display_name} : ${share.added.length ? `ajout ${permissions(share.added)}` : ""}${share.added.length && share.removed.length ? " · " : ""}${share.removed.length ? `retrait ${permissions(share.removed)}` : ""}`)),
        ...diff.removed.map((share) => el("li", "", `Partage retiré : ${share.display_name} perd l’accès partagé (${permissions(share.permissions)})`))),
      el("p", "muted", "La liste remplace celle de la révision lue. Un partage retiré quitte les vues du membre à sa prochaine lecture. Vos accès sont ensuite relus et ce contexte est choisi de nouveau."),
      this.#flow.pendingOperation ? el("p", "mono dg-operation", `Opération ${this.#flow.pendingOperation}`) : null,
      el("div", "dg-actions",
        this.action("Confirmer le changement", "dg-confirm", () => { void this.#submit(); }, settled, true),
        this.action("Revenir à la modification", "dg-back", () => this.goTo("edit"), sending),
        this.action("Fermer", "dg-cancel", () => this.close())));
    this.body.querySelector('[data-focus-key="dg-confirm"]')?.setAttribute("aria-disabled", String(sending));
  }
  async #submit() {
    const grant = this.#grant;
    const phase = this.#flow.phase;
    if (!grant || this.#reading || this.step !== "confirm" || phase === "sending" || phase === "conflict" || phase === "unknown" || this.closed) return;
    const shares = this.#draft.map((share) => ({ principal: share.principal, permissions: SHARE_PERMISSIONS.filter((p) => share.permissions.includes(p)) }));
    const request = this.#flow.submit(this.#key(), (operationId) => this.#host.send({ operation_id: operationId, resource: this.#host.resource,
      expected_revision: grant.revision, shares }, grant.owner, this.abort.signal));
    this.render();
    const outcome = await request;
    if (this.closed || outcome === "conflict") return;
    // An uncertain send can only be recovered by the dashboard with a current context.
    if (outcome === "unknown") this.close();
    else this.render();
  }
}
