// Which nodes a publication detaches. A browser takes the keyboard focus and the text selection away from a
// node that leaves its parent, even to be inserted again at once: a row that can stay in place must not move.
// The page, the parent and the rows below are a structural stand-in for the few DOM members dom-sync uses;
// real focus and real selection are observed in Chromium by the `regions` scenario of browser_supervision.mjs.
import assert from "node:assert/strict";
import { test } from "node:test";
import { syncRows } from "../src/dom-sync.ts";

const sync = await import("../src/dom-sync.ts");

class Row {
  parent = null; detached = 0; focusCalls = [];
  constructor(key, signature) { this.key = key; this.signature = signature; }
  get isConnected() { return this.parent !== null; }
  get outerHTML() { return this.signature; }
  get nextElementSibling() { const list = this.parent?.list; return list ? list[list.indexOf(this) + 1] ?? null : null; }
  remove() {
    const parent = this.parent; if (!parent) return;
    parent.list.splice(parent.list.indexOf(this), 1); this.parent = null; this.detached++;
    // Like a browser: a node that leaves the page loses the focus, which nothing gives back by itself.
    if (parent.ownerDocument.activeElement === this) parent.ownerDocument.activeElement = null;
  }
  replaceWith(node) { const parent = this.parent; if (!parent) return; parent.insertBefore(node, this); this.remove(); }
  focus(options) { if (!this.parent) return; this.parent.ownerDocument.activeElement = this; this.focusCalls.push(options ?? null); }
}
class Parent {
  list = []; ownerDocument = { activeElement: null };
  get children() { return this.list; }
  get firstElementChild() { return this.list[0] ?? null; }
  contains(node) { return node === this || this.list.includes(node); }
  insertBefore(node, reference) {
    node.remove();
    const index = reference ? this.list.indexOf(reference) : this.list.length;
    assert.ok(index >= 0, "the reference node is a child of this parent");
    this.list.splice(index, 0, node); node.parent = this; return node;
  }
}
/** One keyed row per entry; `built` collects the nodes this publication created. */
const rows = (entries, built = []) => entries.map(([key, signature = "same"]) => ({ key, signature, build: () => { const node = new Row(key, signature); built.push(node); return node; } }));
const keys = (parent) => parent.list.map((node) => node.key);
const entries = (count) => Array.from({ length: count }, (_, index) => [`row-${String(index).padStart(3, "0")}`]);
/** Length of a longest strictly increasing subsequence, by the quadratic definition. */
function longest(values) {
  const best = values.map(() => 1);
  for (let right = 0; right < values.length; right++) for (let left = 0; left < right; left++) if (values[left] < values[right]) best[right] = Math.max(best[right], best[left] + 1);
  return Math.max(0, ...best);
}

test("une ligne modifiée qui recule dans le tri ne détache aucune des lignes inchangées", () => {
  const parent = new Parent(); const before = entries(100); syncRows(parent, rows(before));
  const nodes = [...parent.list]; assert.deepEqual(keys(parent), before.map(([key]) => key));
  // The first row changed (its worker ended) and now sorts after forty-one rows that did not change.
  const after = [...before.slice(1, 42), [before[0][0], "changed"], ...before.slice(42)]; const built = [];
  syncRows(parent, rows(after, built));
  assert.deepEqual(keys(parent), after.map(([key]) => key), "l’ordre demandé est obtenu");
  assert.equal(built.length, 1, "seule la ligne modifiée est reconstruite"); assert.equal(parent.list[41], built[0]);
  assert.equal(nodes[0].isConnected, false, "l’ancien nœud de la ligne modifiée a quitté la table");
  const moved = nodes.slice(1).filter((node) => node.detached > 0).map((node) => node.key);
  assert.deepEqual(moved, [], "aucune ligne inchangée n’est détachée puis réinsérée : focus et sélection y restent");
  assert.ok(nodes.slice(1).every((node, index) => parent.list.includes(node) && node.key === before[index + 1][0]), "chaque ligne inchangée garde son nœud");
});

test("un changement d’ordre ne déplace que les lignes qui ne peuvent pas rester en place", () => {
  const parent = new Parent(); const before = entries(100); syncRows(parent, rows(before));
  const nodes = [...parent.list];
  // Same rows, same content: one of them moves down, as when the person changes the sort order of one value.
  const after = [...before.slice(1, 42), before[0], ...before.slice(42)]; const built = [];
  syncRows(parent, rows(after, built));
  assert.deepEqual(keys(parent), after.map(([key]) => key)); assert.equal(built.length, 0, "aucune ligne n’est reconstruite");
  assert.deepEqual(nodes.filter((node) => node.detached > 0).map((node) => node.key), [before[0][0]], "seule la ligne qui change de place est déplacée");
  // A full reversal can keep one row only: every other one has to move, once.
  const counts = new Map(nodes.map((node) => [node, node.detached]));
  const reversed = [...after].reverse(); syncRows(parent, rows(reversed));
  assert.deepEqual(keys(parent), reversed.map(([key]) => key));
  const again = nodes.map((node) => node.detached - counts.get(node));
  assert.equal(again.filter((count) => count === 1).length, 99, "une inversion complète déplace chaque ligne une fois, sauf une");
  assert.equal(again.filter((count) => count === 0).length, 1);
});

test("un lot inséré en tête et des lignes retirées ne touchent pas les lignes conservées", () => {
  const parent = new Parent(); const before = entries(50); syncRows(parent, rows(before));
  const nodes = [...parent.list]; const built = [];
  const after = [["new-2"], ["new-1"], ...before.filter((_, index) => index % 7 !== 3).slice(0, 40)];
  syncRows(parent, rows(after, built));
  assert.deepEqual(keys(parent), after.map(([key]) => key)); assert.equal(built.length, 2);
  const kept = new Set(after.map(([key]) => key));
  assert.deepEqual(nodes.filter((node) => kept.has(node.key) && node.detached > 0), [], "aucune ligne conservée n’est détachée");
  assert.ok(nodes.filter((node) => !kept.has(node.key)).every((node) => !node.isConnected), "les lignes retirées ont quitté la table");
  syncRows(parent, []); assert.deepEqual(parent.list, []);
});

test("quel que soit le changement, l’ordre est exact et le nombre de lignes déplacées est le minimum", () => {
  // Deterministic generator: the same sequences on every run.
  let seed = 20261110; const random = (bound) => { seed = (Math.imul(seed, 1103515245) + 12345) & 0x7fffffff; return (seed >>> 16) % bound; };
  for (let round = 0; round < 300; round++) {
    const parent = new Parent(); const size = 1 + random(30);
    const before = Array.from({ length: size }, (_, index) => [`k${index}`, "v0"]); syncRows(parent, rows(before));
    const nodes = new Map(parent.list.map((node) => [node.key, node]));
    // Shuffle (two rounds in three), drop some rows, change some, add some.
    const shuffled = [...before]; for (let index = shuffled.length - 1; index > 0; index--) { const other = random(index + 1); [shuffled[index], shuffled[other]] = [shuffled[other], shuffled[index]]; }
    const after = (random(3) === 0 ? before : shuffled).filter(() => random(5) !== 0).map(([key]) => [key, random(6) === 0 ? "v1" : "v0"]);
    for (let added = random(4); added > 0; added--) after.splice(random(after.length + 1), 0, [`n${round}-${added}`, "v0"]);
    const built = []; syncRows(parent, rows(after, built));
    assert.deepEqual(keys(parent), after.map(([key]) => key), `tour ${round} : ordre demandé`);
    assert.deepEqual(parent.list.map((node) => node.signature), after.map(([, signature]) => signature), `tour ${round} : contenu demandé`);
    const unchanged = after.filter(([key, signature]) => nodes.has(key) && signature === "v0").map(([key]) => key);
    assert.equal(built.length, after.length - unchanged.length, `tour ${round} : seules les lignes nouvelles ou modifiées sont construites`);
    assert.ok(unchanged.every((key) => parent.list.includes(nodes.get(key))), `tour ${round} : une ligne inchangée garde son nœud`);
    // Among the unchanged rows, those already in the right relative order can all stay: the others move once.
    const positions = new Map(after.map(([key], index) => [key, index]));
    const order = before.map(([key]) => key).filter((key) => unchanged.includes(key)).map((key) => positions.get(key));
    const detached = unchanged.filter((key) => nodes.get(key).detached > 0).length;
    assert.equal(detached, unchanged.length - longest(order), `tour ${round} : ${detached} ligne(s) inchangée(s) déplacée(s) pour ${unchanged.length - longest(order)} nécessaire(s)`);
    assert.ok(unchanged.every((key) => nodes.get(key).detached <= 1), `tour ${round} : jamais plus d’un déplacement par ligne`);
  }
});

test("une ligne conservée qui doit changer de place retrouve le focus qu’elle tenait", () => {
  const parent = new Parent(); const before = [["a"], ["b"], ["c"], ["d"]]; syncRows(parent, rows(before));
  const [a, b] = parent.list; const page = parent.ownerDocument;
  // The focused row stays in place: nothing is taken from it, so nothing is given back.
  b.focus(); b.focusCalls.length = 0;
  syncRows(parent, rows([["b"], ["c"], ["a"], ["d"]]));
  assert.equal(page.activeElement, b, "le focus reste sur une ligne qui peut rester en place");
  assert.equal(a.detached, 1); assert.equal(b.detached, 0);
  assert.deepEqual(b.focusCalls, [], "une ligne restée en place n’est pas focalisée de nouveau");
  // The focused row is the one that has to move: the browser took its focus, the publication gives it back without scrolling.
  a.focus(); a.focusCalls.length = 0;
  syncRows(parent, rows([["a"], ["b"], ["c"], ["d"]]));
  assert.deepEqual(keys(parent), ["a", "b", "c", "d"]); assert.equal(a.detached, 2);
  assert.equal(page.activeElement, a, "la ligne déplacée a retrouvé le focus");
  assert.deepEqual(a.focusCalls, [{ preventScroll: true }]);
  // A focused row that changed is rebuilt: its node left the page, and the view decides where the focus goes.
  syncRows(parent, rows([["a", "changed"], ["b"], ["c"], ["d"]]));
  assert.equal(a.isConnected, false); assert.equal(page.activeElement, null, "le nœud retiré n’est pas focalisé de nouveau");
  // Focus held outside the list is never touched.
  const outside = { marker: "outside" }; page.activeElement = outside;
  syncRows(parent, rows([["d"], ["c"], ["b"], ["a", "changed"]]));
  assert.equal(page.activeElement, outside);
});

test("les sections d’un détail gardent leur nœud tant que leur contenu ne change pas", () => {
  assert.equal(typeof sync.syncSections, "function", "dom-sync.ts must export syncSections");
  const parent = new Parent(); const section = (markup) => new Row(markup, markup);
  assert.equal(sync.syncSections(parent, section("facts"), null, section("events"), section("read at 10:00")), true, "le premier rendu change le détail");
  const [facts, events, read] = parent.list;
  assert.equal(sync.syncSections(parent, section("facts"), section("events"), section("read at 10:00")), false, "un rendu identique ne change rien");
  assert.deepEqual(parent.list, [facts, events, read]);
  // Only the dated line differs after a refresh: the rest of the open detail is left alone.
  assert.equal(sync.syncSections(parent, section("facts"), section("events"), section("read at 10:30")), true);
  assert.equal(parent.list[0], facts); assert.equal(parent.list[1], events); assert.notEqual(parent.list[2], read);
  assert.equal(facts.detached + events.detached, 0, "les sections inchangées ne sont ni détachées ni remplacées");
  assert.deepEqual(parent.list.map((node) => node.signature), ["facts", "events", "read at 10:30"]);
  // Two sections with the same markup are two sections; one that disappears leaves the others in place.
  assert.equal(sync.syncSections(parent, section("facts"), section("note"), section("events"), section("note")), true);
  assert.deepEqual(parent.list.map((node) => node.signature), ["facts", "note", "events", "note"]);
  assert.equal(parent.list[0], facts); assert.equal(parent.list[2], events); assert.notEqual(parent.list[1], parent.list[3]);
  assert.equal(facts.detached + events.detached, 0);
  assert.equal(sync.syncSections(parent, section("events")), true);
  assert.deepEqual(parent.list, [events]); assert.equal(events.detached, 0);
  // Sections that swap places: the first one that still matches stays, the other is replaced by its new node.
  sync.syncSections(parent, section("events"), section("facts"));
  const again = parent.list[1];
  assert.equal(sync.syncSections(parent, section("facts"), section("events")), true);
  assert.deepEqual(parent.list.map((node) => node.signature), ["facts", "events"]);
  assert.equal(parent.list[0], again); assert.notEqual(parent.list[1], events); assert.equal(again.detached, 0);
  assert.equal(sync.syncSections(parent), true); assert.deepEqual(parent.list, []);
  assert.equal(sync.syncSections(parent), false);
});

test("une phrase que la région porte déjà est effacée puis réécrite après une courte pause ; une phrase différente est écrite aussitôt", async () => {
  assert.equal(typeof sync.announce, "function", "dom-sync.ts must export announce");
  // A structural stand-in for a live region: every write of its text is recorded, in order.
  const writes = []; let text = "";
  const region = { isConnected: true, get textContent() { return text; }, set textContent(value) { text = value; writes.push(value); } };
  const pause = () => new Promise((resolve) => setTimeout(resolve, sync.SAME_SENTENCE_MS + 80));
  const count = "24 workers affichés sur 24 chargés."; const other = "18 workers affichés sur 24 chargés.";
  sync.announce(region, count);
  assert.deepEqual(writes, [count], "une phrase nouvelle est écrite aussitôt, en une écriture");
  sync.announce(region, count);
  assert.deepEqual(writes, [count, ""], "la même phrase : la région est d’abord vidée, rien n’est encore réécrit");
  await pause();
  assert.deepEqual(writes, [count, "", count], "puis la phrase est écrite une seule fois, après la pause");
  // The latest sentence wins: a different one asked during the pause is written at once, the pending one never.
  sync.announce(region, count); sync.announce(region, other);
  await pause();
  assert.deepEqual(writes.slice(3), ["", other], "une phrase différente demandée pendant la pause remplace celle en attente");
  // The same sentence asked twice during the pause is one announcement.
  sync.announce(region, other); sync.announce(region, other);
  await pause();
  assert.deepEqual(writes.slice(5), ["", other], "la même phrase demandée deux fois de suite n’est écrite qu’une fois");
  // A region that left the page is not written after the pause.
  sync.announce(region, other); region.isConnected = false;
  await pause();
  assert.deepEqual(writes.slice(7), [""], "une région retirée de la page n’est plus écrite");
});
