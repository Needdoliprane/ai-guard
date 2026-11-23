import assert from "node:assert/strict";
import { test } from "node:test";
import { adminUrls, parseHumanGrant, parseMembershipPage } from "../src/admin-model.ts";
import { DelegatedChangeRecovery } from "../src/delegation-model.ts";
import { GrantEditor } from "../src/delegation-views.ts";
import { HumanApiClient, HumanApiError } from "../src/human-api.ts";
import { grantBody, grantReceipt, memberPrincipal, membership, membershipPage } from "./admin-fixtures.mjs";
import { catalogue, context, errorResponse, identity } from "./human-fixtures.mjs";

// The modal uses these node operations. Taking a focused node out of the tree drops its focus.
class Element extends EventTarget {
  className = "";
  classList = { add: (name) => { this.className = `${this.className} ${name}`; } };
  dataset = {};
  children = [];
  attributes = new Map();
  disabled = false;
  parent = null;
  constructor(tag, text = "") { super(); this.tag = tag; this.text = text; }
  get isConnected() { return this === document.body || this.parent?.isConnected === true; }
  get textContent() { return this.text + this.children.map((child) => child.textContent).join(""); }
  set textContent(text) { this.replaceChildren(); this.text = text; }
  contains(node) { return this === node || this.children.some((child) => child.contains(node)); }
  descendants() { return this.children.flatMap((child) => [child, ...child.descendants()]); }
  append(...children) {
    for (const value of children) {
      const child = typeof value === "string" ? new Element("#text", value) : value;
      child.remove(); child.parent = this; this.children.push(child);
    }
  }
  replaceChildren(...children) {
    for (const child of [...this.children]) child.remove();
    this.append(...children);
  }
  remove() {
    if (!this.parent) return;
    if (this.contains(document.activeElement)) document.activeElement = document.body;
    this.parent.children.splice(this.parent.children.indexOf(this), 1); this.parent = null;
  }
  setAttribute(name, value) { this.attributes.set(name, value); }
  matches(selector) { assert.equal(selector, ":disabled"); return this.disabled; }
  querySelector(selector) {
    const key = /^\[data-focus-key="(.*)"\]$/.exec(selector)?.[1];
    if (key) return this.descendants().find((node) => node.dataset.focusKey === key);
    if (selector === ".dg-step-title") return this.descendants().find((node) => node.className === "dg-step-title");
    assert.equal(selector, "input:not(:disabled), button:not(:disabled)");
    return this.descendants().find((node) => (node.tag === "input" || node.tag === "button") && !node.disabled);
  }
  focus() { if (this.isConnected && !this.disabled) document.activeElement = this; }
  click() { if (!this.disabled) { this.focus(); this.dispatchEvent(new Event("click")); } }
  showModal() { this.open = true; }
  close() { this.open = false; this.dispatchEvent(new Event("close")); }
}

function installNodes(t) {
  const body = new Element("body");
  const globals = { Node: Element, HTMLElement: Element, CSS: { escape: (key) => key },
    document: { body, activeElement: body, createElement: (tag) => new Element(tag),
      createElementNS: (_namespace, tag) => new Element(tag), createTextNode: (text) => new Element("#text", text) } };
  for (const [name, value] of Object.entries(globals)) {
    const original = Object.getOwnPropertyDescriptor(globalThis, name);
    Object.defineProperty(globalThis, name, { value, configurable: true });
    t.after(() => original ? Object.defineProperty(globalThis, name, original) : Reflect.deleteProperty(globalThis, name));
  }
}

const adminId = "c".repeat(64);
const resource = { kind: "session", resource_id: "session-a" };
const admin = { ...context(adminId, "administration"), capabilities: ["read_metadata", "read_memberships", "manage_grants"],
  operations: catalogue({ memberships_read: ["granted", "organization", "available"], grants_manage: ["granted", "organization", "available"] }) };
const tick = () => new Promise((resolve) => setImmediate(resolve));
const control = (editor, key) => editor.dialog.querySelector(`[data-focus-key="${key}"]`);
function confirmRemoval(editor) {
  editor.dialog.descendants().find((node) => node.tag === "button" && node.textContent === "Retirer ce partage").click();
  control(editor, "dg-review").click();
  control(editor, "dg-confirm").click();
}

test("Une réponse de partage perdue retire l’ancienne confirmation", async (t) => {
  installNodes(t);
  let posts = 0;
  const api = new HumanApiClient("synthetic-admin-token", { onAuthentication: () => {}, onContextInvalidated: () => {} },
    async (path, options) => {
      if (path === "/api/v1/human/identity") return Response.json({ ...identity(), contexts: [admin] });
      if (path === adminUrls.grant(resource)) return Response.json(grantBody(adminId));
      if (path === adminUrls.memberships(null)) return Response.json(membershipPage(adminId));
      assert.equal(path, adminUrls.grantChange());
      assert.equal(options.method, "POST");
      posts++;
      throw new TypeError("Réponse interrompue après enregistrement.");
    });
  let editor;
  try {
    const authorized = await api.identity();
    const trigger = document.createElement("button");
    document.body.append(trigger); trigger.focus();
    editor = new GrantEditor({ context: api.selectContext(adminId), self: authorized.principal, resource,
      read: (signal) => api.grant(resource, signal), members: (cursor, signal) => api.memberships(cursor, signal),
      send: (change, owner, signal) => api.changeGrant(change, owner, signal),
      onApplied: () => assert.fail("Le contrôle n’a pas envoyé de reçu."),
      onRefused: () => assert.fail("La réponse est inconnue."), onClosed: () => {} });
    editor.open();
    await tick();
    confirmRemoval(editor);
    await tick();
    assert.equal(posts, 1);
    assert.equal(editor.dialog.isConnected, false);
    assert.equal(document.activeElement, trigger);
  } finally { editor?.close(); api.close(); }
});

test("Un 409 retire la confirmation jusqu’à la relecture des partages et des membres", async (t) => {
  installNodes(t);
  const grantHeld = Promise.withResolvers();
  const membersHeld = Promise.withResolvers();
  const posts = [];
  const applied = [];
  let grantReads = 0;
  let memberReads = 0;
  const newMember = membership({ principal: memberPrincipal(3), display_name: "Membre relu" });
  const memberPage = (items) => Response.json(membershipPage(adminId, { items }));
  const grant = (revision) => Response.json(grantBody(adminId, revision === "4" ? {} : { revision, shares: [] }));
  const api = new HumanApiClient("synthetic-admin-token", { onAuthentication: () => {}, onContextInvalidated: () => {} },
    async (path, options) => {
      if (path === "/api/v1/human/identity") return Response.json({ ...identity(), contexts: [admin] });
      assert.equal(options.headers["X-AiGuard-Context"], adminId);
      if (path === adminUrls.grant(resource)) {
        grantReads++;
        if (grantReads === 1) return grant("4");
        return grantReads === 2 ? grantHeld.promise : grant("7");
      }
      if (path === adminUrls.memberships(null)) {
        memberReads++;
        if (memberReads === 1) return memberPage([membership({ principal: memberPrincipal(2) })]);
        return memberReads === 2 ? membersHeld.promise : memberPage([newMember]);
      }
      assert.equal(path, adminUrls.grantChange());
      assert.equal(options.method, "POST");
      const change = JSON.parse(options.body);
      posts.push(change);
      return posts.length === 1 ? errorResponse(409, "conflict", "reread") :
        Response.json(grantReceipt(change.operation_id, resource, { grant_revision: "8" }));
    });

  let editor;
  try {
    const offer = await api.identity();
    const recovery = new DelegatedChangeRecovery(api);
    const selected = api.selectContext(adminId);
    const trigger = document.createElement("button");
    document.body.append(trigger); trigger.focus();
    editor = new GrantEditor({ context: selected, self: offer.principal, resource,
      read: (signal) => api.grant(resource, signal), members: (cursor, signal) => api.memberships(cursor, signal),
      send: (change, owner, signal) => recovery.submitGrant(offer.principal, selected, change, owner, signal),
      onApplied: (receipt) => { applied.push(receipt); editor.close(); }, onRefused: () => editor.close(), onClosed: () => {} });
    const find = (key) => control(editor, key);
    editor.open();
    await tick();
    confirmRemoval(editor);
    await tick();

    find("dg-confirm")?.click();
    assert.equal(posts.length, 1);
    assert.equal(recovery.operationId, null);
    assert.match(editor.dialog.textContent, /Relecture des partages et des membres/);
    assert.equal(document.activeElement.textContent, "Relire les partages et les membres");
    assert.doesNotMatch(editor.dialog.textContent, /Member Two|révision 4/);
    assert.equal(find("dg-review"), undefined);
    assert.equal(grantReads, 2);
    assert.equal(memberReads, 1, "La relecture des membres attend la réponse de la relecture des partages.");

    grantHeld.resolve(grant("6"));
    await tick();
    assert.equal(memberReads, 2);
    assert.doesNotMatch(editor.dialog.textContent, /révision 6/);
    assert.equal(find("dg-review"), undefined);
    assert.equal(find("dg-confirm"), undefined);
    membersHeld.resolve(errorResponse(503, "audit_unavailable", "contact_operator"));
    await tick();
    assert.match(editor.dialog.textContent, /Relecture impossible.*audit_unavailable/);
    assert.equal(document.activeElement.textContent, "Relire les partages et les membres");
    assert.equal(find("dg-confirm"), undefined);
    assert.equal(grantReads, 2);
    assert.equal(memberReads, 2);
    assert.equal(posts.length, 1);

    find("dg-reread").click();
    await tick();
    assert.equal(grantReads, 3);
    assert.equal(memberReads, 3);
    assert.equal(document.activeElement.textContent, "Partages de la ressource");
    assert.match(editor.dialog.textContent, /révision 7/);
    assert.equal(find("dg-review").disabled, true);
    find("dg-add-share").click();
    find("dg-review").click();
    assert.equal(document.activeElement.textContent, "Vérifier le changement");
    assert.equal(posts.length, 1);
    find("dg-confirm").click();
    await tick();
    assert.equal(posts.length, 2);
    assert.equal(posts[0].expected_revision, "4");
    assert.equal(posts[1].expected_revision, "7");
    assert.notEqual(posts[1].operation_id, posts[0].operation_id);
    assert.deepEqual(posts[1].shares, [{ principal: newMember.principal, permissions: ["metadata"] }]);
    assert.equal(applied.length, 1);
    assert.equal(applied[0].operation_id, posts[1].operation_id);
    assert.deepEqual(applied[0].resource, posts[1].resource);
    assert.equal(applied[0].grant_revision, "8");
    assert.equal(document.activeElement, trigger);
  } finally {
    editor?.close();
    grantHeld.resolve(grant("6"));
    membersHeld.resolve(memberPage([]));
    api.close();
  }
});

test("Fermer l’éditeur pendant la lecture des partages empêche la lecture des membres", async (t) => {
  installNodes(t);
  const expected = { contextId: adminId, authorizationVersion: admin.authorization_version };
  const grant = parseHumanGrant(grantBody(adminId), { ...expected, resource });
  const members = parseMembershipPage(membershipPage(adminId), expected);
  const trigger = document.createElement("button");
  document.body.append(trigger); trigger.focus();
  const held = Promise.withResolvers();
  let readSignal;
  let memberReads = 0;
  const editor = new GrantEditor({ context: admin, self: identity().principal, resource,
    read: (signal) => { readSignal = signal; return held.promise; },
    members: async () => { memberReads++; return members; },
    send: async () => assert.fail("La fermeture ne confirme aucun changement."),
    onApplied: () => assert.fail("La fermeture ne fournit aucun reçu."),
    onRefused: () => assert.fail("Une fermeture est un abandon de lecture."), onClosed: () => {} });
  try {
    editor.open();
    assert.equal(readSignal.aborted, false);
    editor.close();
    assert.equal(readSignal.aborted, true);
    held.resolve(grant);
    await tick();
    assert.equal(memberReads, 0);
    assert.equal(editor.dialog.isConnected, false);
    assert.equal(document.activeElement, trigger);
  } finally { editor.close(); held.resolve(grant); }
});

test("Un refus d’accès pendant la relecture ferme l’éditeur et rend le focus", async (t) => {
  installNodes(t);
  const expected = { contextId: adminId, authorizationVersion: admin.authorization_version };
  const grant = parseHumanGrant(grantBody(adminId), { ...expected, resource });
  const members = parseMembershipPage(membershipPage(adminId), expected);
  const trigger = document.createElement("button");
  document.body.append(trigger); trigger.focus();
  const held = Promise.withResolvers();
  let grantReads = 0;
  let memberReads = 0;
  let posts = 0;
  let refused = 0;
  let readSignal;
  const editor = new GrantEditor({ context: admin, self: identity().principal, resource,
    read: async (signal) => {
      if (++grantReads === 1) return grant;
      readSignal = signal;
      return held.promise;
    },
    members: async () => { memberReads++; return members; },
    send: async () => { posts++; throw new HumanApiError(409, "conflict", "Les partages ont changé.", "reread"); },
    onApplied: () => assert.fail("Le changement a été refusé."), onRefused: () => { refused++; editor.close(); }, onClosed: () => {} });
  try {
    editor.open();
    await tick();
    confirmRemoval(editor);
    await tick();
    assert.equal(readSignal.aborted, false);
    held.reject(new HumanApiError(403, "permission_denied", "Lecture refusée.", "review_permissions"));
    await tick();
    assert.equal(refused, 1);
    assert.equal(editor.dialog.isConnected, false);
    assert.equal(readSignal.aborted, true);
    assert.equal(memberReads, 1);
    assert.equal(posts, 1);
    assert.equal(document.activeElement, trigger);
  } finally {
    editor.close();
    held.resolve(grant);
  }
});
