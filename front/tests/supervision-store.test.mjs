// Asynchronous state of the unified supervision view, without DOM. Reads are test doubles of the
// named client, except the last tests which drive the real HumanApiClient over a recorded fetch.
import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanApiClient, HumanApiError, HumanRecentUnavailable, HumanSuperseded } from "../src/human-api.ts";
import { catalogue, identity, organization, tenantScope } from "./human-fixtures.mjs";
import { adminContext, broker, brokerId, contextId, eventOf, eventsPage, fleetPage, fleetWorker, personalContext, recentPage, registered, resourcesPage, sessionId, sessionResource, workerId, workersPage, brokersPage } from "./supervision-fixtures.mjs";

const module = await import("../src/supervision-store.ts").catch((error) => {
  if (error?.code === "ERR_MODULE_NOT_FOUND") return {};
  throw error;
});
const present = () => assert.equal(typeof module.SupervisionStore, "function", "supervision-store.ts must export SupervisionStore");

function clock() {
  let now = 1_000_000; let visible = true; let sequence = 0; const timers = new Map();
  return { now: () => now, visible: () => visible, setTimer: (run, ms) => { timers.set(++sequence, { run, ms }); return sequence; },
    clearTimer: (handle) => { timers.delete(handle); }, timers,
    advance(ms) { now += ms; }, hide() { visible = false; }, show() { visible = true; },
    fire() { const pending = [...timers.values()]; timers.clear(); for (const timer of pending) timer.run(); } };
}
const deferred = () => { let resolve; let reject; const promise = new Promise((ok, ko) => { resolve = ok; reject = ko; }); return { promise, resolve, reject }; };
const range = (from, count, extra = () => ({})) => Array.from({ length: count }, (_, index) => fleetWorker(from + index, extra(from + index)));
const brokers = () => [broker({ broker_id: brokerId(1) }), broker({ broker_id: brokerId(2), state: "unseen", last_heartbeat: null })];
/** Read double: every call is recorded with its arguments and abort signal. */
function reads(handlers) {
  const calls = [];
  // Unless a test provides it, the control is an older one without the newest-first read.
  handlers = { recentEvents: async () => { throw new HumanRecentUnavailable(true); }, ...handlers };
  const call = (name) => async (...args) => {
    const signal = args.find((value) => value instanceof AbortSignal); calls.push({ name, args, signal });
    if (!handlers[name]) throw new Error(`unexpected ${name} read`);
    return handlers[name](...args);
  };
  return { calls, readBudgetStopped: false, federationWorkers: call("federationWorkers"), workers: call("workers"), brokers: call("brokers"),
    resources: call("resources"), events: call("events"), recentEvents: call("recentEvents") };
}
function open(handlers, context = adminContext()) {
  const time = clock(); const source = reads(handlers); let changes = 0; let doubts = 0;
  const store = new module.SupervisionStore(source, context, time, () => { changes++; }, () => { doubts++; });
  return { store, source, time, changes: () => changes, doubts: () => doubts };
}
const tick = () => new Promise((resolve) => setImmediate(resolve));
const standard = () => ({
  federationWorkers: async () => fleetPage(range(1, 3, () => ({ launch_broker: registered(brokerId(1)) })), brokers()),
  resources: async () => resourcesPage([sessionResource(1), sessionResource(2)]),
});
const ids = (store) => store.state.workers.value?.items.map((item) => item.worker_id) ?? [];

test("l’ouverture lit une composition et les ressources, puis publie des sources datées", async () => {
  present();
  const { store, source, time } = open(standard());
  assert.equal(store.state.workers.phase, "idle");
  await store.open();
  assert.deepEqual(source.calls.map((call) => call.name).sort(), ["federationWorkers", "resources"]);
  assert.deepEqual(source.calls.find((call) => call.name === "federationWorkers").args[0], { workerLimit: 100, brokerLimit: 100, workerCursor: null });
  assert.deepEqual(source.calls.find((call) => call.name === "resources").args.filter((value) => !(value instanceof AbortSignal)), [null, 100]);
  for (const key of ["workers", "brokers", "resources"]) {
    assert.equal(store.state[key].phase, "ready"); assert.equal(store.state[key].loadedAt, time.now());
    assert.equal(store.state[key].stale, false); assert.equal(store.state[key].error, null);
  }
  assert.deepEqual(ids(store), [workerId(1), workerId(2), workerId(3)]);
  assert.equal(store.state.brokers.value.items.length, 2);
  assert.equal(store.state.workers.value.hasMore, false);
  assert.equal(time.timers.size, 1, "une seule minuterie d’actualisation est armée");
  assert.equal([...time.timers.values()][0].ms, 30000);
  store.close();
});

test("une réponse tardive après fermeture ne repeuple rien", async () => {
  present();
  const held = deferred(); const heldResources = deferred();
  const { store, source, changes } = open({ federationWorkers: () => held.promise, resources: () => heldResources.promise });
  const opening = store.open();
  assert.equal(store.state.workers.phase, "loading");
  store.close();
  const after = changes();
  assert.ok(source.calls.every((call) => call.signal.aborted), "la fermeture annule les lectures en vol");
  held.resolve(fleetPage(range(1, 3), brokers())); heldResources.resolve(resourcesPage([sessionResource(1)]));
  await opening;
  assert.equal(store.state.workers.value, null); assert.equal(store.state.brokers.value, null); assert.equal(store.state.resources.value, null);
  assert.equal(store.state.closed, true);
  assert.equal(changes(), after, "aucune publication après fermeture");
});

test("403 retire la fenêtre et les détails ; une erreur serveur conserve la dernière lecture, périmée", async () => {
  present();
  let mode = "ok";
  const { store, time } = open({
    federationWorkers: async () => {
      if (mode === "down") throw new HumanApiError(503, "audit_unavailable", "The required audit store is unavailable.", "retry");
      if (mode === "denied") throw new HumanApiError(403, "permission_denied", "Consultation refusée par le contrôle (HTTP 403).", "review_permissions");
      return fleetPage(range(1, 3), brokers());
    },
    resources: async () => resourcesPage([sessionResource(1)]),
    events: async () => eventsPage([eventOf(1, { kind: "session", resource_id: sessionId(1) })]),
  });
  await store.open(); const loadedAt = store.state.workers.loadedAt;
  await store.readSessionEvents(sessionId(1));
  assert.equal(store.state.session.phase, "ready");
  mode = "down"; time.advance(5000); await store.refresh(false);
  assert.deepEqual(ids(store), [workerId(1), workerId(2), workerId(3)], "la dernière fenêtre autorisée reste affichable");
  assert.equal(store.state.workers.stale, true); assert.equal(store.state.brokers.stale, true);
  assert.equal(store.state.workers.loadedAt, loadedAt, "la date reste celle de la dernière lecture réussie");
  assert.equal(store.state.workers.error.kind, "server");
  assert.equal(store.state.resources.stale, false, "une source indépendante n’est pas périmée par l’échec d’une autre");
  mode = "denied"; await store.refresh(false);
  assert.equal(store.state.workers.phase, "forbidden"); assert.equal(store.state.brokers.phase, "forbidden");
  assert.equal(store.state.workers.value, null); assert.equal(store.state.brokers.value, null);
  assert.equal(store.state.session, null, "le détail ouvert sur une ligne retirée est fermé");
  store.close();
});

test("un 403 de la lecture composée arrive sans motif : toutes les sources sont retirées et l’hôte relit les accès une fois", async () => {
  present();
  let denied = false;
  const refusal = () => new HumanApiError(403, "permission_denied", "Consultation refusée par le contrôle (HTTP 403).", "review_permissions");
  const { store, source, time, doubts } = open({
    federationWorkers: async () => { if (denied) throw refusal(); return fleetPage(range(1, 3), brokers()); },
    resources: async () => resourcesPage([sessionResource(1)]),
    recentEvents: async () => recentPage([eventOf(1, { kind: "session", resource_id: sessionId(1) })]),
  });
  const count = (name) => source.calls.filter((call) => call.name === name).length;
  await store.open(); await store.readSessionEvents(sessionId(1));
  assert.equal(store.state.resources.value.items.length, 1); assert.equal(store.state.session.phase, "ready");
  // The automatic cycle reads the fleet only: nobody clicked, and the resources are not read again.
  denied = true; time.advance(30000); time.fire(); await tick();
  assert.equal(count("federationWorkers"), 2); assert.equal(count("resources"), 1);
  assert.equal(store.state.workers.phase, "forbidden"); assert.equal(store.state.brokers.phase, "forbidden");
  assert.equal(store.state.resources.value, null, "les ressources lues sous ce contexte ne restent pas affichées");
  assert.equal(store.state.resources.phase, "error"); assert.equal(store.state.resources.error.kind, "withdrawn");
  assert.equal(store.state.resources.loadedAt, null);
  assert.equal(store.state.session, null, "le détail d’événements est retiré avec elles");
  assert.equal(doubts(), 1, "une seule relecture des accès est demandée à l’hôte");
  assert.equal(store.state.autoPaused, "forbidden"); assert.equal(time.timers.size, 0, "aucune minuterie après un refus");
  time.fire(); await tick();
  assert.equal(count("federationWorkers"), 2, "aucune relance automatique d’une source interdite");
  // The host confirmed the context: an explicit refresh reads the resources again and leaves the refused registries alone.
  await store.refresh(false);
  assert.equal(count("federationWorkers"), 2); assert.equal(count("resources"), 2);
  assert.equal(store.state.resources.phase, "ready"); assert.equal(store.state.resources.error, null);
  assert.equal(doubts(), 1);
  store.close();

  // The same unexplained refusal while loading one more page.
  let page = 0;
  const more = open({
    federationWorkers: async ({ workerCursor }) => { if (workerCursor && ++page) throw refusal(); return fleetPage(range(1, 100), brokers(), { has_more: true, next_cursor: "cursor-1" }); },
    resources: async () => resourcesPage([sessionResource(1)]),
  });
  await more.store.open(); await more.store.loadMoreWorkers();
  assert.equal(page, 1); assert.equal(more.store.state.workers.phase, "forbidden");
  assert.equal(more.store.state.resources.value, null); assert.equal(more.doubts(), 1);
  more.store.close();
});

test("un refus motivé d’un registre non composé retire cette source seule et arrête son cycle automatique", async () => {
  present();
  let denied = false;
  const { store, source, time, doubts } = open({
    workers: async () => { if (denied) throw new HumanApiError(403, "permission_denied", "Refus permission_denied", "review_permissions"); return workersPage(range(1, 3)); },
    resources: async () => resourcesPage([sessionResource(1)]),
  }, adminContext(["read_worker_registry", "read_tenant_metadata"], { operations: catalogue({ workers_read: tenantScope, quota_read: tenantScope, usage_read: tenantScope,
    summary_read: tenantScope, resources_read: organization, events_read_metadata: organization, events_stream: organization }) }));
  const count = (name) => source.calls.filter((call) => call.name === name).length;
  await store.open(); assert.equal(time.timers.size, 1);
  denied = true; await store.refresh(false);
  assert.equal(store.state.workers.phase, "forbidden"); assert.equal(store.state.workers.value, null);
  assert.equal(store.state.resources.phase, "ready", "le refus motivé d’un registre ne retire pas une autre source autorisée");
  assert.equal(store.state.resources.value.items.length, 1);
  assert.equal(doubts(), 0, "le motif a été lu : aucune relecture des accès n’est demandée");
  assert.equal(store.state.autoPaused, "forbidden"); assert.equal(time.timers.size, 0);
  const reads = count("workers"); time.fire(); await tick();
  assert.equal(count("workers"), reads, "aucune relance automatique d’une source interdite"); assert.equal(time.timers.size, 0);
  store.close();
});

test("une suspension ne se verrouille pas : une action explicite onglet visible la lève, et un retour manqué n’empêche pas le suivant", async () => {
  present();
  const { store, source, time } = open(standard());
  const count = (name) => source.calls.filter((call) => call.name === name).length;
  await store.open();
  time.hide(); store.suspend();
  assert.equal(store.state.autoPaused, "hidden"); assert.equal(time.timers.size, 0);
  // Back in the foreground, but the host could not confirm the identity: resume() was never called.
  time.show(); await store.refresh(false);
  assert.equal(store.state.autoPaused, "", "une actualisation explicite onglet visible lève la suspension");
  assert.equal(time.timers.size, 1, "le cycle automatique est réarmé");
  time.hide(); store.suspend(); time.show(); store.setCadence(120000);
  assert.equal(store.state.autoPaused, "", "un changement de rythme onglet visible la lève aussi");
  assert.equal([...time.timers.values()][0].ms, 120000);
  // hidden -> visible -> hidden again while the host revalidates: its resume() lands on a hidden tab.
  time.hide(); store.suspend(); store.resume();
  assert.equal(store.state.autoPaused, "hidden"); assert.equal(time.timers.size, 0);
  const reads = count("federationWorkers");
  time.show(); store.resume(); await tick();
  assert.equal(count("federationWorkers"), reads + 1, "le retour suivant relit la flotte");
  assert.equal(store.state.autoPaused, ""); assert.equal(time.timers.size, 1);
  store.close();
});

test("de retour au premier plan, la vue ne dit plus « onglet masqué » pendant la lecture qui reprend", async () => {
  present();
  let gate = null;
  const { store, time } = open({ ...standard(), federationWorkers: async () => { if (gate) await gate.promise; return standard().federationWorkers(); } });
  await store.open();
  // The host resumes the view: the fleet read it starts may last, and the tab is visible meanwhile.
  time.hide(); store.suspend(); assert.equal(store.state.autoPaused, "hidden");
  gate = deferred(); time.show(); store.resume(); await tick();
  assert.equal(store.state.workers.busy, true, "précondition : la lecture de reprise est en vol");
  assert.equal(store.state.autoPaused, "", "une reprise onglet visible ne laisse pas « onglet masqué » affiché pendant sa lecture");
  gate.resolve(); for (let turn = 0; turn < 20 && store.state.workers.busy; turn++) await tick();
  assert.equal(store.state.autoPaused, ""); assert.equal(time.timers.size, 1);
  // The same for an explicit refresh that the person asks for before the host resumed the view.
  time.hide(); store.suspend(); assert.equal(store.state.autoPaused, "hidden");
  gate = deferred(); time.show(); const refreshing = store.refresh(false); await tick();
  assert.equal(store.state.workers.busy, true);
  assert.equal(store.state.autoPaused, "", "une actualisation explicite onglet visible non plus");
  gate.resolve(); await refreshing;
  assert.equal(store.state.autoPaused, ""); assert.equal(time.timers.size, 1);
  // A tab that is hidden again keeps saying so.
  time.hide(); store.suspend(); store.resume();
  assert.equal(store.state.autoPaused, "hidden");
  store.close();
});

test("un contexte sans registre n’annonce aucun refus : rien n’est relu automatiquement, et le rythme n’y change rien", async () => {
  present();
  for (const context of [personalContext(), adminContext(["read_tenant_metadata"], { operations: catalogue({ summary_read: tenantScope,
    resources_read: organization, events_read_metadata: organization, events_stream: organization }) })]) {
    const { store, source, time } = open({ resources: async () => resourcesPage([sessionResource(1)]) }, context);
    await store.open();
    assert.equal(store.state.autoPaused, "not_granted", "« non accordé » n’est pas « refusé par le contrôle »");
    assert.equal(time.timers.size, 0);
    store.setCadence(120000); store.setCadence(0);
    assert.equal(store.state.autoPaused, "not_granted"); assert.equal(time.timers.size, 0);
    assert.deepEqual(source.calls.map((call) => call.name), ["resources"]);
    store.close();
  }
});

test("charger une page de plus ne rajeunit pas les lignes déjà lues", async () => {
  present();
  const { store, time } = open({
    federationWorkers: async ({ workerCursor }) => workerCursor ? fleetPage(range(101, 20), brokers()) : fleetPage(range(1, 100), brokers(), { has_more: true, next_cursor: "cursor-1" }),
    resources: async (cursor) => cursor ? resourcesPage([sessionResource(2)]) : resourcesPage([sessionResource(1)], { has_more: true, next_cursor: "resources-1" }),
  });
  await store.open(); const first = time.now();
  assert.equal(store.state.workers.value.oldestReadAt, first); assert.equal(store.state.resources.value.oldestReadAt, first);
  time.advance(270000); await store.loadMoreWorkers(); await store.loadMoreResources();
  assert.equal(store.state.workers.value.items.length, 120); assert.equal(store.state.resources.value.items.length, 2);
  assert.equal(store.state.workers.loadedAt, first + 270000, "l’heure de la dernière lecture réussie");
  assert.equal(store.state.workers.value.oldestReadAt, first, "la page la plus ancienne de la fenêtre garde son heure de lecture");
  assert.equal(store.state.resources.value.oldestReadAt, first);
  // A refresh reads every page of the window again: the whole window is as old as its first page.
  time.advance(1000); await store.refresh(false);
  assert.equal(store.state.workers.value.oldestReadAt, first + 271000); assert.equal(store.state.resources.value.oldestReadAt, first + 271000);
  store.close();
});

test("l’actualisation publie la fenêtre en bloc ou conserve l’ancienne, jamais un mélange", async () => {
  present();
  let generation = 1; let failSecond = false;
  const { store } = open({
    federationWorkers: async ({ workerCursor }) => {
      const base = generation * 1000;
      if (!workerCursor) return fleetPage(range(base + 1, 100), brokers(), { has_more: true, next_cursor: `cursor-${generation}` });
      if (failSecond) throw new HumanApiError(503, "audit_unavailable", "The required audit store is unavailable.", "retry");
      return fleetPage(range(base + 101, 20), brokers());
    },
    resources: async () => resourcesPage([]),
  });
  await store.open();
  assert.equal(ids(store).length, 100); assert.equal(store.state.workers.value.hasMore, true);
  await store.loadMoreWorkers();
  assert.equal(ids(store).length, 120); assert.equal(store.state.workers.value.pages, 2); assert.equal(store.state.workers.value.hasMore, false);
  const first = ids(store);
  generation = 2; failSecond = true; await store.refresh(false);
  assert.deepEqual(ids(store), first, "un échec en cours de relecture ne publie aucune ligne de la nouvelle génération");
  assert.equal(store.state.workers.stale, true);
  failSecond = false; await store.refresh(false);
  assert.equal(ids(store).length, 120);
  assert.ok(ids(store).every((id) => !first.includes(id)), "une relecture réussie remplace toute la fenêtre");
  assert.equal(store.state.workers.stale, false); assert.equal(store.state.workers.error, null);
  store.close();
});

test("charger plus ajoute une page sans doublon, borne la fenêtre à cinq pages et suspend l’actualisation automatique", async () => {
  present();
  const { store, source, time } = open({
    federationWorkers: async ({ workerCursor }) => {
      const page = workerCursor ? Number(workerCursor.split("-")[1]) : 0;
      // The first row of each later page repeats the last row of the previous one.
      return fleetPage(range(page * 100 + (page ? 0 : 1), 100), brokers(), { has_more: true, next_cursor: `cursor-${page + 1}` });
    },
    resources: async () => resourcesPage([]),
  });
  await store.open();
  for (let page = 2; page <= 6; page++) await store.loadMoreWorkers();
  assert.equal(store.state.workers.value.pages, 5, "la fenêtre est bornée à cinq pages");
  assert.equal(source.calls.filter((call) => call.name === "federationWorkers").length, 5, "aucune sixième page n’est demandée");
  assert.equal(new Set(ids(store)).size, ids(store).length, "aucune ligne en double");
  assert.equal(store.state.workers.value.capped, true);
  assert.equal(store.state.autoPaused, "window", "au-delà de deux pages l’actualisation devient manuelle");
  assert.equal(time.timers.size, 0);
  store.close();
});

test("l’actualisation automatique ne relit que la flotte et s’arrête onglet caché ou budget atteint", async () => {
  present();
  const { store, source, time } = open(standard());
  await store.open();
  const count = (name) => source.calls.filter((call) => call.name === name).length;
  time.advance(30000); time.fire(); await new Promise((resolve) => setImmediate(resolve));
  assert.equal(count("federationWorkers"), 2); assert.equal(count("resources"), 1, "les ressources ne sont relues que sur action explicite");
  assert.equal(time.timers.size, 1, "le cycle suivant est armé après la lecture");
  store.suspend();
  assert.equal(time.timers.size, 0); assert.equal(store.state.autoPaused, "hidden");
  await store.refresh(true);
  assert.equal(count("federationWorkers"), 2, "aucune lecture automatique onglet caché");
  store.resume(); await new Promise((resolve) => setImmediate(resolve));
  assert.equal(count("federationWorkers"), 3, "le retour au premier plan relit la flotte une fois");
  source.readBudgetStopped = true; time.fire(); await new Promise((resolve) => setImmediate(resolve));
  assert.equal(count("federationWorkers"), 3, "aucune lecture automatique après arrêt du budget durable");
  assert.equal(store.state.autoPaused, "budget"); assert.equal(time.timers.size, 0);
  store.setCadence(0);
  assert.equal(store.state.cadence, 0); assert.equal(time.timers.size, 0, "le mode manuel n’arme aucune minuterie");
  assert.throws(() => store.setCadence(1000), /cadence/);
  store.close();
});

test("un cycle automatique ne démarre jamais tant que le précédent est en vol : des réponses plus lentes que la période n’accumulent aucune lecture", async () => {
  present();
  let gate = null;
  const { store, source, time } = open({ ...standard(), federationWorkers: async () => { if (gate) await gate.promise; return standard().federationWorkers(); } });
  const fleet = () => source.calls.filter((call) => call.name === "federationWorkers").length;
  const periods = async (count) => { for (let period = 0; period < count; period++) { time.advance(30000); time.fire(); await tick(); } };
  await store.open();
  assert.equal(fleet(), 1); assert.equal(time.timers.size, 1);
  // The cycle starts and the control takes longer than several periods to answer.
  gate = deferred(); await periods(1);
  assert.equal(fleet(), 2); assert.equal(store.state.workers.busy, true, "précondition : la lecture du cycle est en vol");
  assert.equal(time.timers.size, 0, "aucune minuterie n’est armée pendant la lecture : le cycle suivant ne peut pas partir de lui-même");
  await periods(5);
  assert.equal(fleet(), 2, "cinq périodes écoulées pendant la lecture n’émettent aucune autre lecture");
  // Whatever asks for the fleet meanwhile reads nothing more: a stray cycle, a return to the tab, another
  // period, an explicit refresh. Some of them arm a timer; the cycle it starts finds the read in flight.
  // None of these calls is awaited here: a read they would wrongly start would wait for the same answer.
  void store.refresh(true); await tick();
  time.hide(); store.suspend(); time.show(); store.resume(); await tick();
  store.setCadence(120000); store.setCadence(30000);
  const resources = source.calls.filter((call) => call.name === "resources").length;
  const explicit = store.refresh(false); await tick();
  assert.equal(source.calls.filter((call) => call.name === "resources").length, resources + 1, "précondition : l’actualisation explicite a bien été traitée");
  await periods(5);
  assert.equal(fleet(), 2, "une seule lecture de flotte reste en vol, quoi qu’il soit demandé entre-temps");
  assert.equal(store.state.workers.busy, true);
  // The answer arrives: one timer, one period after the end of that read, then exactly one new read.
  gate.resolve(); await explicit; for (let turn = 0; turn < 20 && store.state.workers.busy; turn++) await tick();
  assert.equal(fleet(), 2, "la réponse tardive ne déclenche aucune lecture de rattrapage");
  assert.equal(time.timers.size, 1, "une seule minuterie après la fin de la lecture");
  assert.equal([...time.timers.values()][0].ms, 30000, "le cycle suivant part une période après la fin du précédent");
  gate = null; await periods(1);
  assert.equal(fleet(), 3, "un cycle, une lecture");
  assert.equal(time.timers.size, 1);
  store.close();
});

test("la fermeture annule lectures, détails et minuteries et vide l’état", async () => {
  present();
  const heldEvents = deferred();
  const { store, source, time } = open({ ...standard(), recentEvents: () => heldEvents.promise });
  await store.open();
  const detail = store.readSessionEvents(sessionId(1));
  assert.equal(store.state.session.phase, "loading");
  store.close();
  assert.equal(time.timers.size, 0);
  assert.ok(source.calls.every((call) => call.signal.aborted));
  heldEvents.reject(new HumanSuperseded()); await detail;
  assert.equal(store.state.workers.value, null); assert.equal(store.state.session, null);
  assert.equal(source.calls.some((call) => call.name === "events"), false, "aucun repli n’est lu après la fermeture");
  const calls = source.calls.length;
  await store.refresh(false); await store.loadMoreWorkers(); await store.readSessionEvents(sessionId(1));
  assert.equal(source.calls.length, calls, "un store fermé n’émet plus aucune lecture");
});

test("un contexte brokers seuls n’émet ni lecture de workers, ni ressources, ni événements", async () => {
  present();
  const { store, source } = open({ brokers: async () => brokersPage(brokers()) }, adminContext(["read_broker_registry"], { operations: catalogue({ brokers_read: tenantScope, summary_read: tenantScope }) }));
  await store.open();
  assert.deepEqual(source.calls.map((call) => call.name), ["brokers"]);
  assert.deepEqual(source.calls[0].args.filter((value) => !(value instanceof AbortSignal)), [null, 100]);
  assert.equal(store.state.workers.phase, "not_granted"); assert.equal(store.state.resources.phase, "not_granted");
  assert.equal(store.state.brokers.phase, "ready");
  await store.readSessionEvents(sessionId(1));
  assert.equal(store.state.session.phase, "not_granted");
  assert.deepEqual(source.calls.map((call) => call.name), ["brokers"], "aucune requête d’événements sans capacité de métadonnées");
  store.close();
  const workersOnly = open({ workers: async () => workersPage(range(1, 2)) }, adminContext(["read_worker_registry"], { operations: catalogue({ workers_read: tenantScope, summary_read: tenantScope }) }));
  await workersOnly.store.open();
  assert.deepEqual(workersOnly.source.calls.map((call) => call.name), ["workers"]);
  assert.equal(workersOnly.store.state.brokers.phase, "not_granted");
  workersOnly.store.close();
  const personal = open({ resources: async () => resourcesPage([sessionResource(1)]) }, personalContext());
  await personal.store.open();
  assert.deepEqual(personal.source.calls.map((call) => call.name), ["resources"]);
  assert.equal(personal.store.state.workers.phase, "not_granted");
  personal.store.close();
});

test("un refus de session reste local au détail et ne purge pas la flotte", async () => {
  present();
  let refusal = new HumanApiError(404, "not_found", "The requested resource is unavailable.", "review_permissions");
  const { store, source } = open({ ...standard(), events: async () => { throw refusal; } });
  await store.open();
  await store.readSessionEvents(sessionId(9));
  assert.equal(store.state.session.phase, "refused");
  assert.match(store.state.session.message, /Aucune ressource autorisée de ce contexte ne correspond/);
  assert.deepEqual(ids(store).length, 3, "la flotte reste intacte");
  assert.equal(store.state.workers.stale, false);
  const selection = source.calls.find((call) => call.name === "events").args[0];
  assert.deepEqual(selection, { view: "metadata", limit: 25, resource: { kind: "session", resource_id: sessionId(9) } });
  refusal = new HumanApiError(403, "permission_denied", "Refus permission_denied", "review_permissions");
  await store.readSessionEvents(sessionId(9));
  assert.equal(store.state.session.phase, "refused");
  refusal = new HumanApiError(503, "audit_unavailable", "The required audit store is unavailable.", "retry");
  await store.readSessionEvents(sessionId(9));
  assert.equal(store.state.session.phase, "error", "une panne n’est pas un refus");
  store.clearSessionEvents();
  assert.equal(store.state.session, null);
  store.close();
});

test("les événements d’une session sont lus les plus récents d’abord lorsque le contrôle sert cette lecture", async () => {
  present();
  let outcome = () => recentPage([eventOf(3, { kind: "session", resource_id: sessionId(1) }), eventOf(2, { kind: "session", resource_id: sessionId(1) })], { has_more: true, next_cursor: "older-opaque" });
  const { store, source } = open({ ...standard(), recentEvents: async () => outcome() });
  await store.open(); await store.readSessionEvents(sessionId(1));
  assert.equal(store.state.session.phase, "ready"); assert.equal(store.state.session.order, "newest_first");
  assert.equal(store.state.session.page.items.length, 2); assert.equal(store.state.session.page.has_more, true);
  assert.deepEqual(source.calls.find((call) => call.name === "recentEvents").args[0], { view: "metadata", limit: 25, resource: { kind: "session", resource_id: sessionId(1) } });
  assert.equal(source.calls.some((call) => call.name === "events"), false, "aucune seconde lecture lorsque la lecture récente répond");
  // Once the route is known, its not-found is the refusal of this resource: no fallback read.
  outcome = () => { throw new HumanApiError(404, "not_found", "The requested resource is unavailable.", "review_permissions"); };
  await store.readSessionEvents(sessionId(9));
  assert.equal(store.state.session.phase, "refused"); assert.equal(source.calls.some((call) => call.name === "events"), false);
  store.close();
  // An older control: the forward read answers, and the detail says in which order.
  const older = open({ ...standard(), events: async () => eventsPage([eventOf(1, { kind: "session", resource_id: sessionId(1) })]) });
  await older.store.open(); await older.store.readSessionEvents(sessionId(1));
  assert.equal(older.store.state.session.phase, "ready"); assert.equal(older.store.state.session.order, "oldest_first");
  assert.deepEqual(older.source.calls.filter((call) => ["recentEvents", "events"].includes(call.name)).map((call) => call.name), ["recentEvents", "events"]);
  older.store.close();
});

test("le résultat d’une lecture d’événements de session s’annonce en une phrase, sans événement ni reçu", async () => {
  present(); assert.equal(typeof module.sessionAnnouncement, "function", "supervision-store.ts must export sessionAnnouncement");
  const ref = { kind: "session", resource_id: sessionId(1) }; const gate = deferred();
  let outcome = () => gate.promise;
  const { store } = open({ ...standard(), recentEvents: async () => outcome() });
  await store.open();
  assert.equal(module.sessionAnnouncement(store.state.session), null, "rien à annoncer avant toute lecture");
  const reading = store.readSessionEvents(sessionId(1));
  assert.equal(store.state.session.phase, "loading"); assert.equal(module.sessionAnnouncement(store.state.session), null, "une lecture en vol n’annonce rien");
  gate.resolve(recentPage([eventOf(3, ref), eventOf(2, ref)], { has_more: true, next_cursor: "older-opaque" })); await reading;
  const sentence = module.sessionAnnouncement(store.state.session);
  assert.equal(sentence, "2 événement(s) reçus pour cette session, du plus récent au plus ancien ; d’autres existent.");
  assert.ok(!sentence.includes(store.state.session.page.read_receipt.audit_event_id) && !sentence.includes("command.completed"), "ni reçu ni contenu d’événement dans l’annonce");
  outcome = () => recentPage([]); await store.readSessionEvents(sessionId(1));
  assert.equal(module.sessionAnnouncement(store.state.session), "Aucun événement autorisé pour cette session à cette lecture.");
  outcome = () => { throw new HumanApiError(404, "not_found", "The requested resource is unavailable.", "review_permissions"); };
  await store.readSessionEvents(sessionId(9));
  assert.equal(module.sessionAnnouncement(store.state.session), store.state.session.message);
  assert.match(module.sessionAnnouncement(store.state.session), /^Aucune ressource autorisée de ce contexte ne correspond/);
  store.close();
  // An older control answers through the forward read: the sentence says that order.
  const older = open({ ...standard(), events: async () => eventsPage([eventOf(1, ref)]) });
  await older.store.open(); await older.store.readSessionEvents(sessionId(1));
  assert.equal(module.sessionAnnouncement(older.store.state.session), "1 événement(s) reçus pour cette session, du plus ancien au plus récent.");
  older.store.close();
});

test("un contexte devenu invalide ou expiré côté serveur retire toutes les données sans les marquer interdites", async () => {
  present();
  for (const code of ["context_invalid", "context_expired", "context_revoked"]) {
    let failing = false;
    const { store, time } = open({
      federationWorkers: async () => {
        if (failing) throw new HumanApiError(403, code, `Refus ${code}`, "select_context");
        return fleetPage(range(1, 3), brokers());
      },
      resources: async () => resourcesPage([sessionResource(1)]),
    });
    await store.open(); failing = true; await store.refresh(false);
    assert.equal(store.state.closed, true, `${code} ferme la vue : le choix du contexte appartient au dashboard`);
    for (const key of ["workers", "brokers", "resources"]) assert.equal(store.state[key].value, null);
    assert.notEqual(store.state.workers.phase, "forbidden");
    assert.equal(time.timers.size, 0);
  }
});

test("saturation temporaire, action opérateur, contrat et panne réseau restent des erreurs distinctes", async () => {
  present();
  const cases = [
    [new HumanApiError(429, "capacity_exhausted", "Live context capacity is full.", "retry"), "capacity"],
    [new HumanApiError(503, "identity_unavailable", "The credential binding is ambiguous.", "contact_operator"), "operator"],
    [new HumanApiError(503, "identity_unavailable", "The human gateway is unavailable.", "retry"), "unreachable"],
    [new HumanApiError(429, "audit_read_budget_exhausted", "The durable read budget is exhausted.", "contact_operator"), "budget"],
    [new TypeError("fetch failed"), "unreachable"],
    [new DOMException("The operation timed out.", "TimeoutError"), "unreachable"],
  ];
  for (const [error, kind] of cases) {
    const { store } = open({ federationWorkers: async () => { throw error; }, resources: async () => resourcesPage([]) });
    await store.open();
    assert.equal(store.state.workers.phase, "error"); assert.equal(store.state.workers.value, null, "aucune ligne inventée après un échec initial");
    assert.equal(store.state.workers.error.kind, kind, String(error.code ?? error.name));
    assert.ok(store.state.workers.error.message.length > 0);
    store.close();
  }
});

// The two tests below use the real named client: URLs, context header and refusal handling are the product's.
function client(responder) {
  const requests = []; const invalidated = [];
  const api = new HumanApiClient("fixture-human-secret-123456789", { onAuthentication: () => {}, onContextInvalidated: (error) => invalidated.push(error.code) },
    async (path, options) => { requests.push({ path, context: options.headers["X-AiGuard-Context"] }); return responder(path); });
  return { api, requests, invalidated };
}
const supervisionIdentity = (context) => ({ ...identity(), contexts: [context] });

test("par le client nominatif réel, la vue demande une composition de 100 lignes et 100 ressources dans son contexte", async () => {
  present();
  const context = adminContext();
  const { api, requests } = client((path) => Response.json(path.endsWith("/identity") ? supervisionIdentity(context) :
    path.includes("/federation/workers") ? fleetPage(range(1, 2), brokers()) : resourcesPage([sessionResource(1)])));
  await api.identity(); const selected = api.selectContext(contextId);
  const store = new module.SupervisionStore(api, selected, clock(), () => {});
  await store.open();
  assert.deepEqual(requests.map((request) => request.path).sort(), ["/api/v1/human/federation/workers?worker_limit=100&broker_limit=100",
    "/api/v1/human/identity", "/api/v1/human/resources?limit=100"]);
  assert.ok(requests.slice(1).every((request) => request.context === contextId));
  assert.equal(store.state.workers.phase, "ready"); assert.equal(store.state.resources.value.items.length, 1);
  store.close(); api.close();
});

const refused = (code) => Response.json({ error: { code, message: `Refus ${code}`, retry_action: "select_context" } }, { status: 403 });

test("par le client nominatif réel, un contexte révoqué vu par le cycle automatique retire tout et fait relire les accès", async (t) => {
  present();
  const context = adminContext(); let mode = "ok"; let doubts = 0;
  const { api, requests, invalidated } = client((path) => {
    if (path.endsWith("/identity")) return Response.json(supervisionIdentity(context));
    if (path.includes("/resources")) return mode === "invalid" ? refused("context_invalid") : Response.json(resourcesPage([sessionResource(1)]));
    // What the gateway answers on every route once the tenant identity changed.
    return mode === "ok" ? Response.json(fleetPage(range(1, 2), brokers())) : refused("context_revoked");
  });
  t.after(() => api.close());
  await api.identity();
  const time = clock();
  const store = new module.SupervisionStore(api, api.selectContext(contextId), time, () => {}, () => { doubts++; });
  await store.open(); assert.equal(store.state.workers.value.items.length, 2); assert.equal(store.state.resources.value.items.length, 1);
  mode = "revoked"; const before = requests.length;
  time.advance(30000); time.fire();
  for (let turn = 0; turn < 20 && store.state.workers.phase !== "forbidden"; turn++) await tick();
  assert.deepEqual(requests.slice(before).map((request) => request.path), ["/api/v1/human/federation/workers?worker_limit=100&broker_limit=100"], "le cycle automatique ne lit que la flotte");
  assert.equal(store.state.workers.phase, "forbidden"); assert.equal(store.state.workers.value, null);
  assert.deepEqual(invalidated, [], "le client ne lit pas le motif d’un 403 de composition : il ne peut pas fermer le contexte lui-même");
  assert.equal(store.state.resources.value, null, "rien de ce qui a été lu sous ce contexte ne reste affiché");
  assert.equal(doubts, 1, "la relecture d’identité de l’hôte tranche entre contexte perdu et registre refusé");
  // Another route, whose refusal is read: the client itself closes the context.
  mode = "invalid"; await store.refresh(false);
  assert.deepEqual(invalidated, ["context_invalid"]);
  assert.equal(store.state.closed, true); assert.equal(store.state.resources.value, null);
  api.close();
});

test("par le client nominatif réel, la relecture après intervention de l’opérateur relit la flotte puis les ressources", async (t) => {
  present();
  const context = adminContext(); let exhausted = false;
  const { api, requests } = client((path) => {
    if (path.endsWith("/identity")) return Response.json(supervisionIdentity(context));
    if (exhausted) return Response.json({ error: { code: "audit_read_budget_exhausted", message: "The durable read budget is exhausted.", retry_action: "contact_operator" } }, { status: 429 });
    return Response.json(path.includes("/federation/workers") ? fleetPage(range(1, 2), brokers()) : resourcesPage([sessionResource(1)]));
  });
  t.after(() => api.close());
  await api.identity();
  const store = new module.SupervisionStore(api, api.selectContext(contextId), clock(), () => {});
  await store.open();
  exhausted = true; await store.refresh(false);
  assert.equal(api.readBudgetStopped, true); assert.equal(store.state.workers.error.kind, "budget");
  // The operator reopened the budget; the person asks for one explicit read.
  exhausted = false; api.allowReadAfterOperatorAction(); const before = requests.length;
  await store.refresh(false);
  assert.equal(api.readBudgetStopped, false, "la lecture réussie rouvre le budget côté client");
  assert.deepEqual(requests.slice(before).map((request) => request.path), ["/api/v1/human/federation/workers?worker_limit=100&broker_limit=100", "/api/v1/human/resources?limit=100"],
    "la flotte consomme la tentative accordée, les ressources ne partent qu’une fois le budget rouvert");
  for (const key of ["workers", "brokers", "resources"]) {
    assert.equal(store.state[key].error, null, `${key} : aucune erreur de budget locale après la reprise`);
    assert.equal(store.state[key].stale, false); assert.equal(store.state[key].phase, "ready");
  }
  // A budget still exhausted keeps the resources as they were, without a second refused request.
  exhausted = true; await store.refresh(false); assert.equal(api.readBudgetStopped, true);
  api.allowReadAfterOperatorAction(); const again = requests.length; await store.refresh(false);
  assert.deepEqual(requests.slice(again).map((request) => request.path), ["/api/v1/human/federation/workers?worker_limit=100&broker_limit=100"]);
  assert.equal(api.readBudgetStopped, true);
  store.close(); api.close();
});
