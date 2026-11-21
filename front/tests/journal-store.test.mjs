// State of the Journal without DOM: newest-first window, live stream from the position of the page, bounded history.
import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiClient, HumanApiError, HumanRecentUnavailable, HumanSuperseded } from "../src/human-api.ts";
import { identity } from "./human-fixtures.mjs";
import { adminContext, contextId, eventOf, eventsPage, recentPage, sessionId } from "./supervision-fixtures.mjs";

const module = await import("../src/journal-store.ts").catch((error) => {
  if (error?.code === "ERR_MODULE_NOT_FOUND") return {};
  throw error;
});
const present = () => assert.equal(typeof module.JournalStore, "function", "journal-store.ts must export JournalStore");
const session = { kind: "session", resource_id: sessionId(1) };
/** Events numbered from `from` down to `from - count + 1`: newest first, like the control returns them. */
const newestFirst = (from, count) => Array.from({ length: count }, (_, index) => eventOf(from - index, session));
const numbers = (store) => store.state.items.map((item) => Number.parseInt(item.metadata.id.slice(-12), 16));
function clock() {
  let visible = true; let sequence = 0; const timers = new Map();
  return { now: () => 5_000_000, visible: () => visible, setTimer: (run, ms) => { timers.set(++sequence, { run, ms }); return sequence; },
    clearTimer: (handle) => { timers.delete(handle); }, timers, hide() { visible = false; }, show() { visible = true; },
    fire() { const pending = [...timers.values()]; timers.clear(); for (const timer of pending) timer.run(); } };
}
/** Read double. `listen` stays open until its signal aborts and exposes the callbacks of the product. */
function reads(recent) {
  const calls = []; const streams = [];
  return { calls, streams, readBudgetStopped: false,
    recentEvents: async (selection, signal) => { calls.push({ name: "recentEvents", selection, signal }); return recent(selection); },
    listen: (selection, signal, onPage, onStatus) => new Promise((resolve, reject) => {
      const stream = { selection, signal, onPage, onStatus, reject }; streams.push(stream); calls.push({ name: "listen", selection, signal });
      signal.addEventListener("abort", () => resolve(), { once: true });
    }) };
}
function open(recent, resource = null) {
  const time = clock(); const source = reads(recent); let changes = 0;
  const store = new module.JournalStore(source, time, () => { changes++; }, resource);
  return { store, source, time, changes: () => changes };
}
const tick = () => new Promise((resolve) => setImmediate(resolve));
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };

test("l’ouverture lit les plus récents puis ouvre le direct à la position de cette page", async () => {
  present();
  const { store, source } = open(() => recentPage(newestFirst(300, 100), { has_more: true, next_cursor: "older-1" }));
  assert.equal(store.state.phase, "idle");
  const opening = store.open(); assert.equal(store.state.phase, "loading"); await opening;
  assert.deepEqual(source.calls.map((call) => call.name), ["recentEvents", "listen"]);
  assert.deepEqual(source.calls[0].selection, { view: "metadata", limit: 100 });
  assert.deepEqual(source.calls[1].selection, { view: "metadata", limit: 100, cursor: "live-cursor-opaque" }, "même limite et même sélection que la page, à sa position");
  assert.equal(store.state.phase, "ready"); assert.equal(store.state.items.length, 100); assert.equal(store.state.hasOlder, true);
  assert.equal(store.state.live, "connecting"); assert.equal(store.state.loadedAt, 5_000_000);
  source.streams[0].onStatus("live"); assert.equal(store.state.live, "live"); assert.equal(store.state.stale, false);
  store.close();
});

test("un lot en direct s’insère en tête sans doublon", async () => {
  present();
  const { store, source } = open(() => recentPage(newestFirst(10, 5)));
  await store.open(); const stream = source.streams[0]; stream.onStatus("live");
  // The stream delivers a batch oldest first; one event was already in the window.
  stream.onPage(eventsPage([eventOf(10, session), eventOf(11, session), eventOf(12, session)]));
  assert.deepEqual(numbers(store), [12, 11, 10, 9, 8, 7, 6]);
  assert.equal(store.state.received, 2, "seuls les événements nouveaux sont comptés");
  stream.onPage(eventsPage([eventOf(12, session), eventOf(13, session)]));
  assert.deepEqual(numbers(store).slice(0, 3), [13, 12, 11]); assert.equal(new Set(numbers(store)).size, numbers(store).length);
  store.close();
});

test("une panne de pagination conserve son curseur et n’interrompt pas le direct", async () => {
  const { store, source } = open((selection) => {
    if (!selection.cursor) return recentPage(newestFirst(10, 5), { has_more: true, next_cursor: "older-1" });
    if (source.calls.filter((call) => call.selection.cursor === "older-1").length === 1)
      throw new HumanApiError(503, "identity_unavailable", "The cursor control is unavailable.", "retry");
    return recentPage(newestFirst(5, 5), { live_cursor: null });
  });
  await store.open();
  const stream = source.streams[0]; stream.onStatus("live");
  await store.loadOlder();
  assert.equal(store.state.error, null);
  assert.equal(store.state.olderError?.retry, "same_page");
  assert.equal(store.state.hasOlder, true);
  assert.deepEqual(numbers(store), [10, 9, 8, 7, 6]);
  stream.onPage(eventsPage([eventOf(11, session)]), "position-11");
  assert.equal(store.state.olderError?.kind, "unreachable");
  assert.equal(module.journalAnnouncement(store.state), "Journal : page plus ancienne indisponible, dernières lignes conservées. Direct connecté.");
  await store.loadOlder();
  assert.equal(store.state.olderError, null);
  assert.deepEqual(numbers(store), [11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1]);
  assert.deepEqual(source.calls.filter((call) => call.name === "recentEvents").map((call) => call.selection.cursor),
    [undefined, "older-1", "older-1"]);
  assert.equal(source.streams.length, 1);
  assert.equal(stream.signal.aborted, false);
  store.close();
});

test("un curseur invalide, un refus et un snapshot injoignable ne rejouent pas la page", async () => {
  const cursorError = new HumanApiError(400, "invalid_cursor", "Cursor invalid.", "select_context");
  const denied = new HumanApiError(403, "permission_denied", "Page refused.", "review_permissions");
  const unavailable = new HumanApiError(503, "identity_unavailable", "The cursor control is unavailable.", "retry");
  for (const [issue, profile, phase] of [[cursorError, {}, "ready"], [denied, {}, "forbidden"],
    [unavailable, { stream_mode: "snapshot_only", live_cursor: null }, "error"]]) {
    let newestUnavailable = false;
    const { store, source } = open((selection) => {
      if (selection.cursor) throw issue;
      if (newestUnavailable) throw unavailable;
      return recentPage(newestFirst(10, 5), { has_more: true, next_cursor: "older-1", ...profile });
    });
    await store.open(); await store.loadOlder();
    assert.equal(store.state.phase, phase);
    assert.equal(store.state.items.length, phase === "ready" ? 5 : 0);
    if (issue === cursorError) {
      assert.equal(store.state.olderError?.retry, "from_newest");
      assert.equal(store.state.hasOlder, true, "le curseur refusé n’invalide pas le constat sur la page lue");
      const before = source.calls.length;
      await store.loadOlder();
      assert.equal(source.calls.length, before, "le curseur refusé n’est jamais demandé de nouveau");
      newestUnavailable = true;
      await store.reload();
      assert.equal(store.state.olderError?.retry, "from_newest", "une relecture en échec ne rétablit pas la position");
      await store.loadOlder();
      assert.equal(source.calls.length, before + 1);
      newestUnavailable = false;
      await store.reload();
      assert.equal(store.state.olderError, null);
      assert.equal(source.calls.at(-2).selection.cursor, undefined);
    } else assert.equal(store.state.olderError, null);
    if (phase === "forbidden") assert.equal(source.streams[0].signal.aborted, true);
    if (profile.stream_mode) assert.equal(source.streams.length, 0);
    store.close();
  }
});

test("le direct peut atteindre exactement la borne pendant la reprise d’une page plus ancienne", async () => {
  let read = 0;
  const delayed = deferred();
  const { store, source } = open((selection) => {
    if (selection.cursor === "older-9") {
      if (read === 10) { read++; throw new HumanApiError(503, "identity_unavailable", "Cursor control unavailable.", "retry"); }
      return delayed.promise;
    }
    const index = read++;
    return recentPage(newestFirst(2000 - index * 100, index === 9 ? 99 : 100), {
      has_more: true, next_cursor: index === 9 ? "older-9" : `older-${index}`, ...(selection.cursor ? { live_cursor: null } : {}),
    });
  });
  await store.open();
  for (let page = 0; page < 9; page++) await store.loadOlder();
  assert.equal(store.state.items.length, 999);
  await store.loadOlder();
  assert.equal(store.state.olderError?.retry, "same_page");
  const pending = store.loadOlder();
  source.streams[0].onStatus("live");
  source.streams[0].onPage(eventsPage([eventOf(2001, session)]), "position-2001");
  assert.equal(store.state.items.length, 1000);
  assert.equal(store.state.trimmed, true);
  assert.equal(store.state.hasOlder, false);
  delayed.resolve(recentPage(newestFirst(1001, 100), { live_cursor: null }));
  await pending;
  assert.equal(store.state.olderError, null);
  assert.equal(numbers(store).at(-1), 1002);
  const before = source.calls.length;
  await store.loadOlder();
  assert.equal(source.calls.length, before);
  store.close();
});

test("au-delà de 1 000 événements les plus anciens sortent et la lecture plus ancienne est annulée", async () => {
  present();
  let page = 0;
  const { store, source } = open((selection) => {
    const base = 100000 - page * 100; page++;
    return recentPage(newestFirst(base, 100), { has_more: true, next_cursor: `older-${page}`, ...(selection.cursor ? { live_cursor: null } : {}) });
  });
  await store.open();
  for (let index = 0; index < 12; index++) await store.loadOlder();
  assert.equal(store.state.items.length, 1000, "la fenêtre est bornée");
  assert.equal(source.calls.filter((call) => call.name === "recentEvents").length, 10, "aucune page n’est lue au-delà de la borne");
  assert.equal(store.state.hasOlder, false); assert.equal(store.state.trimmed, true);
  const stream = source.streams[0]; stream.onStatus("live");
  stream.onPage(eventsPage([eventOf(100001, session), eventOf(100002, session)]));
  assert.equal(store.state.items.length, 1000); assert.deepEqual(numbers(store).slice(0, 2), [100002, 100001]);
  assert.equal(numbers(store).at(-1), 99003, "les deux plus anciens sont sortis");
  assert.equal(store.state.trimmed, true);
  const before = source.calls.length; await store.loadOlder();
  assert.equal(source.calls.length, before, "la position plus ancienne ne suit plus la fenêtre : aucune lecture");
  store.close();
});

test("route absente : état indisponible, aucun flux ouvert, aucun rejeu d’historique", async () => {
  present();
  const { store, source } = open(() => { throw new HumanRecentUnavailable(false); });
  await store.open();
  assert.equal(store.state.phase, "unavailable"); assert.equal(store.state.items.length, 0);
  assert.match(store.state.error.message, /Mettez à jour gateway et dashboard ensemble/);
  assert.deepEqual(source.calls.map((call) => call.name), ["recentEvents"], "le flux n’est jamais ouvert sans position de départ");
  assert.equal(store.state.live, "off");
  store.close();
});

test("refus, ressource non accordée et panne restent distincts ; une panne conserve la fenêtre périmée", async () => {
  present();
  let failure = null;
  const { store, source } = open(() => { if (failure) throw failure; return recentPage(newestFirst(50, 20)); });
  await store.open(); source.streams[0].onStatus("live");
  failure = new HumanApiError(503, "audit_unavailable", "The required audit store is unavailable.", "retry");
  await store.reload();
  assert.equal(store.state.phase, "ready"); assert.equal(store.state.items.length, 20); assert.equal(store.state.stale, true);
  assert.equal(store.state.error.kind, "server"); assert.equal(store.state.live, "off", "aucun direct sans page relue");
  assert.equal(source.streams[0].signal.aborted, true);
  failure = new HumanApiError(404, "not_found", "The requested resource is unavailable.", "review_permissions");
  await store.setResource({ kind: "session", resource_id: sessionId(9) });
  assert.equal(store.state.phase, "refused"); assert.equal(store.state.items.length, 0);
  assert.deepEqual(source.calls.at(-1).selection, { view: "metadata", limit: 100, resource: { kind: "session", resource_id: sessionId(9) } });
  failure = null; await store.setResource(null);
  assert.equal(store.state.phase, "ready"); assert.equal(store.state.resource, null);
  failure = new HumanApiError(403, "permission_denied", "Refus permission_denied", "review_permissions");
  await store.reload();
  assert.equal(store.state.phase, "forbidden"); assert.equal(store.state.items.length, 0); assert.equal(store.state.loadedAt, null);
  for (const code of ["context_invalid", "context_expired", "context_revoked"]) {
    let failing = false;
    const closing = open(() => { if (failing) throw new HumanApiError(403, code, `Refus ${code}`, "select_context"); return recentPage(newestFirst(5, 5)); });
    await closing.store.open(); failing = true; await closing.store.reload();
    assert.equal(closing.store.state.closed, true); assert.equal(closing.store.state.items.length, 0);
    assert.ok(closing.source.streams.every((stream) => stream.signal.aborted));
  }
  store.close();
});

test("le direct signale reconnexion et refus ; un onglet masqué ferme le flux et son retour relit les plus récents", async () => {
  present();
  let newest = 10;
  const { store, source, time } = open(() => recentPage(newestFirst(newest, 5)));
  await store.open(); const first = source.streams[0];
  first.onStatus("live"); first.onStatus("retrying");
  assert.equal(store.state.live, "retrying"); assert.equal(store.state.stale, true, "une coupure peut masquer des événements");
  first.onStatus("live"); assert.equal(store.state.stale, true, "la connexion seule ne confirme pas le rattrapage");
  time.hide(); store.suspend();
  assert.equal(store.state.live, "suspended"); assert.equal(first.signal.aborted, true);
  const reads = () => source.calls.filter((call) => call.name === "recentEvents").length;
  assert.equal(reads(), 1);
  newest = 14; time.show(); store.resume(); await tick();
  assert.equal(reads(), 2, "le retour au premier plan relit depuis les plus récents");
  assert.deepEqual(numbers(store), [14, 13, 12, 11, 10]); assert.equal(source.streams.length, 2); assert.equal(store.state.live, "connecting");
  source.streams[1].reject(new HumanApiError(429, "capacity_exhausted", "Concurrent stream capacity is full.", "close_unused_streams")); await tick();
  assert.equal(store.state.live, "refused"); assert.match(store.state.liveMessage, /Trop de flux ouverts/);
  assert.equal(store.state.items.length, 5, "le refus du direct ne retire pas la page lue");
  // With the durable read budget stopped, no stream is opened at all.
  source.readBudgetStopped = true; await store.reload();
  assert.equal(source.streams.length, 2); assert.equal(store.state.live, "refused");
  store.close();
});

test("la fin d’une tranche commencée avant la coupure ne confirme pas la fraîcheur", async () => {
  const { store, source } = open(() => recentPage(newestFirst(10, 5)));
  await store.open();
  const stream = source.streams[0];
  stream.onStatus("live");
  stream.onPage(eventsPage([eventOf(11, session)], { has_more: true }), "position-11");
  stream.onStatus("retrying"); stream.onStatus("live");
  stream.onPage(eventsPage([eventOf(12, session)]), "position-12");
  assert.equal(store.state.stale, true, "cette tranche était bornée avant la coupure");
  stream.onPage(eventsPage([]), "position-sans-lot");
  assert.equal(store.state.stale, true, "une trame vide ne prouve pas le rattrapage");
  stream.onPage(eventsPage([eventOf(13, session)], { has_more: true }), "position-13");
  assert.equal(store.state.stale, true, "la tranche suivant la coupure a encore des événements");
  stream.onPage(eventsPage([eventOf(14, session)]), "position-14");
  assert.equal(store.state.stale, false);
  assert.deepEqual(numbers(store), [14, 13, 12, 11, 10, 9, 8, 7, 6]);
  assert.equal(source.calls.filter((call) => call.name === "recentEvents").length, 1);
  store.close();
});

test("la fermeture annule la lecture et le flux ; une réponse tardive ne repeuple rien", async () => {
  present();
  let release;
  const { store, source, changes } = open(() => new Promise((resolve) => { release = () => resolve(recentPage(newestFirst(5, 5))); }));
  const opening = store.open(); await tick();
  store.close(); const after = changes();
  assert.equal(source.calls[0].signal.aborted, true);
  release(); await opening;
  assert.equal(store.state.items.length, 0); assert.equal(store.state.closed, true); assert.equal(source.streams.length, 0, "aucun flux après fermeture");
  assert.equal(changes(), after);
  await store.reload(); await store.loadOlder(); await store.setResource(session);
  assert.equal(source.calls.length, 1, "un journal fermé n’émet plus aucune lecture");
  const late = open(() => recentPage(newestFirst(5, 5)));
  await late.store.open(); const stream = late.source.streams[0]; late.store.close();
  stream.onPage(eventsPage([eventOf(99, session)])); stream.onStatus("live"); stream.reject(new HumanSuperseded());
  assert.equal(late.store.state.items.length, 0, "un lot tardif du flux est ignoré");
});

test("la fermeture annule une relance du direct déjà armée et ignore son callback tardif", async () => {
  present();
  const { store, source, time, changes } = open(() => recentPage(newestFirst(5, 5)));
  await store.open();
  source.streams[0].reject(new HumanApiError(503, "identity_unavailable", "The human gateway is unavailable.", "retry")); await tick();
  assert.equal(time.timers.size, 1, "une relance est armée avant la fermeture");
  const pending = [...time.timers.values()][0];
  const calls = source.calls.length;
  store.close(); const after = changes();
  assert.equal(time.timers.size, 0, "la fermeture libère immédiatement la minuterie et sa closure");
  assert.ok(source.calls.every((call) => call.signal.aborted), "lectures et flux sont annulés");
  assert.equal(store.state.closed, true); assert.equal(store.state.items.length, 0);
  // A callback already queued by the clock still cannot read or publish after the close.
  pending.run(); await tick();
  assert.equal(source.calls.length, calls, "aucune lecture ni réouverture du direct après fermeture");
  assert.equal(changes(), after, "aucune publication après fermeture");
  assert.equal(store.state.closed, true); assert.equal(store.state.items.length, 0);
});

test("une dernière page plus ancienne coupée par la borne n’annonce pas le début de la sélection", async () => {
  present();
  let page = 0;
  const { store, source } = open((selection) => {
    if (!selection.cursor) return recentPage(newestFirst(2000, 100), { has_more: true, next_cursor: "older-1" });
    page++;
    // Eight full older pages, then a last one of 80 events: the control has nothing older after it.
    return page <= 8 ? recentPage(newestFirst(2000 - page * 100, 100), { has_more: true, next_cursor: `older-${page + 1}`, live_cursor: null }) :
      recentPage(newestFirst(1100, 80), { live_cursor: null });
  });
  await store.open();
  for (let index = 0; index < 8; index++) await store.loadOlder();
  assert.equal(store.state.items.length, 900);
  const stream = source.streams[0]; stream.onStatus("live");
  stream.onPage(eventsPage(Array.from({ length: 50 }, (_, index) => eventOf(2001 + index, session))), "live-position-1");
  assert.equal(store.state.items.length, 950); assert.equal(store.state.hasOlder, true);
  await store.loadOlder();
  assert.equal(store.state.items.length, 1000, "la fenêtre reste bornée");
  assert.equal(numbers(store).at(-1), 1051, "trente événements autorisés de la dernière page ne sont pas affichés");
  assert.equal(store.state.trimmed, true, "une page coupée par la borne est une couverture partielle, même si elle était la dernière");
  assert.equal(store.state.hasOlder, false);
  store.close();
});

test("une action explicite onglet visible rouvre le direct sans attendre resume, et un retour manqué n’empêche pas le suivant", async () => {
  present();
  const { store, source, time } = open(() => recentPage(newestFirst(10, 5)));
  await store.open(); source.streams[0].onStatus("live");
  time.hide(); store.suspend();
  assert.equal(store.state.live, "suspended"); assert.equal(source.streams[0].signal.aborted, true);
  // Back in the foreground, but the host could not confirm the identity: resume() was never called.
  time.show(); await store.reload();
  assert.equal(source.streams.length, 2, "« Relire depuis les plus récents » rouvre le flux onglet visible");
  assert.equal(store.state.live, "connecting");
  // hidden -> visible -> hidden again while the host revalidates: its resume() lands on a hidden tab.
  time.hide(); store.suspend(); store.resume(); await tick();
  assert.equal(store.state.live, "suspended"); assert.equal(source.streams.length, 2, "rien n’est lu onglet masqué");
  time.show(); store.resume(); await tick();
  assert.equal(source.streams.length, 3, "le retour suivant relit et rouvre le flux");
  assert.equal(store.state.live, "connecting");
  // With the durable read budget stopped, the foreground state names the real reason instead of a hidden tab.
  time.hide(); store.suspend(); source.readBudgetStopped = true; time.show(); store.resume(); await tick();
  assert.equal(store.state.live, "refused"); assert.match(store.state.liveMessage, /Budget durable de lecture atteint/);
  store.close();
});

test("un refus temporaire du direct est une déconnexion : relances bornées depuis la dernière position livrée, puis arrêt explicite", async () => {
  present();
  const { store, source, time } = open(() => recentPage(newestFirst(10, 5)));
  await store.open(); let stream = source.streams[0]; stream.onStatus("live");
  stream.onPage(eventsPage([eventOf(11, session)]), "position-after-11");
  const unavailable = () => new HumanApiError(503, "identity_unavailable", "The human gateway is unavailable.", "retry");
  stream.reject(unavailable()); await tick();
  assert.equal(store.state.live, "retrying", "un 503 « retry » n’est pas un refus du contrôle");
  assert.equal(store.state.stale, true); assert.equal(store.state.items.length, 6, "la fenêtre lue reste affichée");
  assert.equal(time.timers.size, 1); assert.equal([...time.timers.values()][0].ms, 1000);
  time.fire();
  assert.equal(source.streams.length, 2);
  assert.deepEqual(source.streams[1].selection, { view: "metadata", limit: 100, cursor: "position-after-11" }, "la reprise part de la dernière position livrée, jamais du début");
  // A reconnection that delivers again resets the bound.
  stream = source.streams[1]; stream.onStatus("live");
  assert.equal(store.state.live, "live"); assert.equal(store.state.stale, true);
  stream.onPage(eventsPage([eventOf(12, session)]), "position-after-12");
  assert.equal(store.state.stale, false);
  const delays = [];
  for (let attempt = 0; attempt < 20 && store.state.live !== "off"; attempt++) {
    source.streams.at(-1).reject(new HumanApiError(429, "capacity_exhausted", "Live stream capacity is temporarily full.", "retry")); await tick();
    if (time.timers.size) { delays.push([...time.timers.values()][0].ms); time.fire(); }
  }
  assert.deepEqual(delays, [1000, 2000, 4000, 8000, 16000, 30000], "délais croissants et bornés");
  assert.equal(store.state.live, "off", "après la dernière tentative le direct s’arrête et le dit");
  assert.match(store.state.liveMessage, /n’a pas pu être rétabli/); assert.equal(store.state.stale, true); assert.equal(time.timers.size, 0);
  assert.equal(source.calls.filter((call) => call.name === "recentEvents").length, 1, "aucune page n’est relue par ces relances");
  // The explicit action restarts from the newest; a real refusal still stops at once.
  await store.reload(); assert.equal(store.state.live, "connecting"); assert.equal(store.state.liveMessage, "");
  source.streams.at(-1).reject(new HumanApiError(429, "capacity_exhausted", "Concurrent stream capacity is full.", "close_unused_streams")); await tick();
  assert.equal(store.state.live, "refused"); assert.equal(time.timers.size, 0);
  // A pending retry never survives a hidden tab or a closed journal.
  await store.reload(); source.streams.at(-1).reject(unavailable()); await tick(); assert.equal(time.timers.size, 1);
  time.hide(); store.suspend(); assert.equal(time.timers.size, 0); assert.equal(store.state.live, "suspended");
  store.close();
});

test("le Journal ne relit rien par période : une relecture remplace celle en vol, une page plus ancienne n’est pas doublée, un seul flux reste ouvert", async () => {
  present();
  const answers = [];
  const { store, source, time } = open(() => { const answer = deferred(); answers.push(answer); return answer.promise; });
  const recent = () => source.calls.filter((call) => call.name === "recentEvents");
  const openStreams = () => source.streams.filter((stream) => !stream.signal.aborted).length;
  const first = store.open(); await tick();
  assert.equal(time.timers.size, 0, "aucune minuterie de relecture : seul le direct suit l’ouverture");
  // Two reloads while the first read is still in flight, as a slow control allows: each replaces the previous one.
  const second = store.reload(); const third = store.reload(); await tick();
  assert.deepEqual(recent().map((call) => call.signal.aborted), [true, true, false], "une relecture annule la lecture qu’elle remplace");
  answers[1].resolve(recentPage(newestFirst(20, 5))); answers[0].resolve(recentPage(newestFirst(10, 5))); await tick();
  assert.equal(store.state.phase, "loading", "une réponse remplacée n’est pas publiée");
  assert.equal(source.streams.length, 0, "une réponse remplacée n’ouvre aucun flux");
  answers[2].resolve(recentPage(newestFirst(30, 5), { has_more: true, next_cursor: "older-1" })); await Promise.all([first, second, third]);
  assert.deepEqual(numbers(store), [30, 29, 28, 27, 26]); assert.equal(source.streams.length, 1, "un seul flux pour trois relectures");
  assert.equal(time.timers.size, 0);
  // An older page in flight is not asked twice.
  const older = [store.loadOlder(), store.loadOlder(), store.loadOlder()]; await tick();
  assert.equal(recent().length, 4, "une page plus ancienne en vol n’est demandée qu’une fois");
  answers[3].resolve(recentPage(newestFirst(25, 5), { live_cursor: null })); await Promise.all(older);
  assert.deepEqual(numbers(store), [30, 29, 28, 27, 26, 25, 24, 23, 22, 21]); assert.equal(recent().length, 4);
  // The stream is reopened only after it ended, by one timer, and the previous one is closed first.
  source.streams[0].onStatus("live");
  source.streams[0].reject(new HumanApiError(503, "identity_unavailable", "The human gateway is unavailable.", "retry")); await tick();
  assert.equal(time.timers.size, 1, "une seule relance armée, après la fin du flux");
  assert.equal(source.streams.length, 1, "aucun flux n’est rouvert avant l’échéance");
  time.fire();
  assert.equal(source.streams.length, 2); assert.equal(openStreams(), 1, "le flux précédent est terminé avant d’en ouvrir un autre");
  assert.equal(time.timers.size, 0); assert.equal(recent().length, 4, "une relance du direct ne relit aucune page");
  store.close();
});

test("par le client nominatif réel, un refus du direct sans corps conforme est un refus, jamais une déconnexion relancée", async (t) => {
  present();
  // What an intermediary answers in place of the control: a 403 without the error contract, as a response or as a stream frame.
  const refusals = [["réponse 403 sans corps conforme", () => Response.json({}, { status: 403 })],
    ["trame d’erreur non conforme", () => new Response("event: error\ndata: {}\n\n", { headers: { "content-type": "text/event-stream" } })]];
  for (const [name, refuse] of refusals) {
    let streams = 0;
    const api = new HumanApiClient("fixture-human-secret-123456789", { onAuthentication: () => {}, onContextInvalidated: () => {} }, async (path) => {
      if (path.endsWith("/identity")) return Response.json({ ...identity(), contexts: [adminContext()] });
      if (path.includes("/events/recent")) return Response.json(recentPage(newestFirst(10, 5)));
      streams++; return refuse();
    });
    t.after(() => api.close());
    await api.identity(); api.selectContext(contextId);
    const time = clock(); const store = new module.JournalStore(api, time, () => {});
    await store.open();
    for (let turn = 0; turn < 50 && store.state.live === "connecting"; turn++) await tick();
    assert.equal(store.state.live, "refused", `${name} : un refus n’est pas une déconnexion`);
    assert.equal(time.timers.size, 0, `${name} : aucune relance n’est programmée`);
    assert.equal(streams, 1, `${name} : le flux n’est demandé qu’une fois`);
    assert.match(store.state.liveMessage, /^Consultation refusée/, `${name} : le motif dit un refus`);
    assert.equal(store.state.items.length, 5, `${name} : la page lue reste affichée`);
    store.close(); api.close();
  }
});

test("un 404 filtré avant que la route ait répondu est levé par une lecture sans filtre", async () => {
  present();
  const resource = { kind: "session", resource_id: sessionId(9) };
  // The route exists: the unfiltered probe answers, so the filtered 404 was the refusal of this resource.
  const granted = open((selection) => { if (selection.resource) throw new HumanRecentUnavailable(true); return recentPage(newestFirst(3, 1)); }, resource);
  await granted.store.open();
  assert.deepEqual(granted.source.calls.map((call) => call.selection), [{ view: "metadata", limit: 100, resource }, { view: "metadata", limit: 1 }]);
  assert.equal(granted.store.state.phase, "refused", "la route existe : c’est la ressource qui n’est pas accordée");
  assert.equal(granted.store.state.items.length, 0, "la page de la lecture sans filtre n’est jamais affichée sous la limite demandée");
  assert.equal(granted.source.streams.length, 0);
  granted.store.close();
  // An older gateway: the probe gets the same 404, and only then the route is declared absent.
  const absent = open((selection) => { throw new HumanRecentUnavailable(!!selection.resource); }, resource);
  await absent.store.open();
  assert.equal(absent.store.state.phase, "unavailable"); assert.equal(absent.source.calls.length, 2);
  absent.store.close();
  // A probe that fails otherwise decides nothing: the failure is reported as such.
  const down = open((selection) => { if (selection.resource) throw new HumanRecentUnavailable(true); throw new HumanApiError(503, "audit_unavailable", "The required audit store is unavailable.", "retry"); }, resource);
  await down.store.open();
  assert.equal(down.store.state.phase, "error"); assert.equal(down.store.state.error.kind, "server");
  down.store.close();
});

test("l’annonce du Journal est une phrase sans heure ni compte, muette pendant une lecture ou une connexion", async () => {
  present();
  assert.equal(typeof module.journalAnnouncement, "function");
  const { store, source, time } = open(() => recentPage(newestFirst(10, 5)));
  assert.equal(module.journalAnnouncement(store.state), null, "rien à annoncer avant la première lecture");
  await store.open();
  assert.equal(module.journalAnnouncement(store.state), null, "la connexion du direct est une transition");
  const stream = source.streams[0]; stream.onStatus("live");
  const steady = module.journalAnnouncement(store.state);
  assert.equal(steady, "Journal : lecture réussie. Direct connecté.");
  stream.onPage(eventsPage([eventOf(11, session), eventOf(12, session)]), "position-after-12");
  assert.equal(module.journalAnnouncement(store.state), steady, "un lot en direct ne change pas l’annonce");
  stream.onStatus("retrying");
  assert.equal(module.journalAnnouncement(store.state), "Journal : données périmées, dernière lecture conservée. Direct déconnecté, reconnexion en cours.");
  stream.onStatus("live");
  assert.equal(module.journalAnnouncement(store.state), "Journal : données périmées, dernière lecture conservée. Direct connecté.");
  time.hide(); store.suspend();
  assert.equal(module.journalAnnouncement(store.state), "Journal : données périmées, dernière lecture conservée. Direct suspendu, onglet masqué.");
  store.close();
  assert.equal(module.journalAnnouncement(store.state), null);
  const refused = open(() => { throw new HumanApiError(403, "permission_denied", "Refus permission_denied", "review_permissions"); });
  await refused.store.open();
  assert.equal(module.journalAnnouncement(refused.store.state), "Journal : lecture refusée, événements retirés.");
  refused.store.close();
});
