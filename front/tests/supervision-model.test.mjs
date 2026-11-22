// Pure model of the unified supervision view: joins, filters, coverage and access. No DOM, no network.
import assert from "node:assert/strict";
import { test } from "node:test";
import { HumanContractError, humanBrokersUrl, humanFederationWorkersUrl, humanUrl, humanWorkersUrl } from "../src/human-model.ts";
import { adminContext, adminOperations, broker, brokerId, fleetWorker, namedResource, observation, ownedAdminOperations, personalContext, registered, sessionId, sessionResource, unassociated, workerId } from "./supervision-fixtures.mjs";
import { catalogue, organization, own, tenantScope } from "./human-fixtures.mjs";

// A missing module is reported by the first assertion of each test, not by an import failure of the file.
const model = await import("../src/supervision-model.ts").catch((error) => {
  if (error?.code === "ERR_MODULE_NOT_FOUND") return {};
  throw error;
});
const present = (name) => assert.equal(typeof model[name], "function", `supervision-model.ts must export ${name}`);

test("les lectures de la vue unifiée demandent 100 lignes sans changer le défaut des registres exacts", () => {
  assert.equal(humanFederationWorkersUrl({ workerLimit: 100, brokerLimit: 100 }), "/api/v1/human/federation/workers?worker_limit=100&broker_limit=100");
  assert.equal(humanFederationWorkersUrl({ workerLimit: 100, brokerLimit: 100, workerCursor: "opaque-cursor" }),
    "/api/v1/human/federation/workers?worker_limit=100&broker_limit=100&worker_cursor=opaque-cursor");
  assert.equal(humanWorkersUrl(null, 100), "/api/v1/human/workers?limit=100");
  assert.equal(humanBrokersUrl(null, 100), "/api/v1/human/brokers?limit=100");
  assert.equal(humanUrl("resources", { view: "metadata", limit: 100 }), "/api/v1/human/resources?limit=100");
  assert.equal(humanUrl("events", { view: "metadata", limit: 100, resource: { kind: "session", resource_id: "s" } }),
    "/api/v1/human/events?limit=100&view=metadata&resource_kind=session&resource_id=s");
  // Existing callers keep the published 25-line pages.
  assert.equal(humanFederationWorkersUrl(), "/api/v1/human/federation/workers?worker_limit=25&broker_limit=25");
  assert.equal(humanWorkersUrl(), "/api/v1/human/workers?limit=25");
  assert.equal(humanBrokersUrl(), "/api/v1/human/brokers?limit=25");
  assert.equal(humanUrl("resources"), "/api/v1/human/resources?limit=25");
  for (const limit of [0, 101, 1.5, -1, "100", Number.NaN])
    assert.throws(() => humanWorkersUrl(null, limit), HumanContractError, `la limite ${String(limit)} est refusée avant toute requête`);
});

test("la jointure garde un broker sans worker et un worker sans broker, sans relation déduite", () => {
  present("joinFleet");
  const brokers = [broker({ broker_id: brokerId(1) }), broker({ broker_id: brokerId(2), state: "revoked", revoked_at: "2026-11-01T12:00:02Z" }),
    broker({ broker_id: brokerId(3), state: "unseen", last_heartbeat: null })];
  const workers = [
    fleetWorker(1, { launch_broker: registered(brokerId(1)) }),
    fleetWorker(2, { launch_broker: registered(brokerId(1)), observation_broker: registered(brokerId(2)) }),
    fleetWorker(3),
    fleetWorker(4, { launch_broker: unassociated("authority_unknown") }),
    fleetWorker(5, { launch_broker: registered(brokerId(9), { broker: { state: "stale" } }) }),
  ];
  const groups = model.joinFleet(brokers, workers);
  const of = (key) => groups.find((group) => group.key === key);
  assert.deepEqual(groups.filter((group) => group.kind === "broker").map((group) => group.key), [brokerId(1), brokerId(2), brokerId(3)],
    "chaque broker reçu garde sa ligne, dans l’ordre du contrôle");
  assert.deepEqual(of(brokerId(1)).workers.map((item) => item.worker_id), [workerId(1), workerId(2)]);
  assert.deepEqual(of(brokerId(2)).workers.map((item) => item.worker_id), [workerId(2)], "l’autorité d’inspection est une association distincte");
  assert.deepEqual(of(brokerId(3)).workers, [], "un broker sans association dans la fenêtre reste affiché");
  assert.deepEqual(of("unregistered").workers.map((item) => item.worker_id), [workerId(3)]);
  assert.deepEqual(of("authority_unknown").workers.map((item) => item.worker_id), [workerId(4)]);
  const unlisted = of(brokerId(9));
  assert.equal(unlisted.kind, "unlisted_broker", "un broker référencé mais absent de la page brokers n’est pas inventé comme ligne du registre");
  assert.equal(unlisted.broker, null);
  assert.deepEqual(unlisted.workers.map((item) => item.worker_id), [workerId(5)]);
  const joined = new Set(groups.flatMap((group) => group.workers.map((item) => item.worker_id)));
  assert.equal(joined.size, workers.length, "aucun worker n’est perdu par la jointure");
  // A similar-looking identifier or a shared runtime never creates a relation.
  const lookalike = model.joinFleet([broker({ broker_id: brokerId(1) })], [fleetWorker(6, { launch_broker: registered(brokerId(10)) })]);
  assert.deepEqual(lookalike.find((group) => group.key === brokerId(1)).workers, []);
  // Without a broker page (capability absent or read refused) worker groups still exist.
  const alone = model.joinFleet(null, workers);
  assert.equal(alone.some((group) => group.kind === "broker"), false);
  assert.equal(new Set(alone.flatMap((group) => group.workers.map((item) => item.worker_id))).size, workers.length);
});

test("la recherche et les filtres portent sur les lignes chargées et annoncent n sur N sans total", () => {
  present("filterWorkers"); present("windowSummary"); present("searchTerms");
  const workers = [
    fleetWorker(1, { state: "running", last_observation: observation(5, { status: "running", running: true, finished_at: null, finished_at_status: "absent", exit_code: null }), launch_broker: registered(brokerId(1)) }),
    fleetWorker(2, { state: "reserved", launch_broker: registered(brokerId(1)) }),
    fleetWorker(3, { state: "in_doubt", runtime_kind: "podman" }),
    fleetWorker(4, { state: "terminal", quota_held: false, last_observation: observation(9, { oom_killed: true, exit_code: "137" }) }),
    fleetWorker(5, { state: "cancelled", quota_held: false }),
  ];
  const filter = (extra) => model.filterWorkers(workers, { ...model.defaultFleetFilter(), ...extra }).map((item) => item.worker_id);
  assert.deepEqual(filter({}), workers.map((item) => item.worker_id));
  assert.deepEqual(filter({ activity: "open" }), [workerId(1), workerId(2)]);
  assert.deepEqual(filter({ activity: "uncertain" }), [workerId(3)]);
  assert.deepEqual(filter({ activity: "ended" }), [workerId(4), workerId(5)]);
  assert.deepEqual(filter({ runtime: "podman" }), [workerId(3)]);
  assert.deepEqual(filter({ broker: brokerId(1) }), [workerId(1), workerId(2)]);
  assert.deepEqual(filter({ broker: "unregistered" }), [workerId(3), workerId(4), workerId(5)]);
  assert.deepEqual(filter({ observed: "observed" }), [workerId(1), workerId(4)]);
  assert.deepEqual(filter({ observed: "unknown" }), [workerId(2), workerId(3), workerId(5)]);
  assert.deepEqual(filter({ flagged: true }), [workerId(3), workerId(4)]);
  assert.deepEqual(filter({ text: workerId(4).slice(-6) }), [workerId(4)]);
  assert.deepEqual(filter({ text: `  ${sessionId(2).toUpperCase()}  ` }), [workerId(2)], "identifiant exact insensible à la casse");
  assert.deepEqual(filter({ text: "podman in_doubt" }), [workerId(3)], "les termes se combinent en ET");
  assert.deepEqual(filter({ text: "podman reserved" }), []);
  assert.deepEqual(filter({ text: ".*" }), [], "aucune expression régulière n’est interprétée");
  assert.deepEqual(model.searchTerms("a".repeat(400)).join(" ").length, 128, "la saisie est bornée à 128 caractères");
  assert.equal(model.windowSummary({ shown: 2, loaded: 5, hasMore: true, noun: "workers" }),
    "2 workers affichés sur 5 chargés · d’autres pages existent au contrôle, aucun total du tenant n’est connu");
  assert.equal(model.windowSummary({ shown: 5, loaded: 5, hasMore: false, noun: "workers" }),
    "5 workers affichés sur 5 chargés · fin du registre à cette lecture");
  // Counts agree in number and gender; zero and one are singular.
  assert.equal(model.windowSummary({ shown: 0, loaded: 0, hasMore: false, noun: "workers" }),
    "0 worker affiché sur 0 chargé · fin du registre à cette lecture");
  assert.equal(model.windowSummary({ shown: 1, loaded: 1, hasMore: false, noun: "workers" }),
    "1 worker affiché sur 1 chargé · fin du registre à cette lecture");
  assert.equal(model.windowSummary({ shown: 1, loaded: 100, hasMore: true, noun: "workers" }),
    "1 worker affiché sur 100 chargés · d’autres pages existent au contrôle, aucun total du tenant n’est connu");
  assert.equal(model.windowSummary({ shown: 15, loaded: 15, hasMore: false, noun: "resources" }),
    "15 ressources affichées sur 15 chargées · fin du registre à cette lecture");
  assert.equal(model.windowSummary({ shown: 1, loaded: 15, hasMore: false, noun: "resources" }),
    "1 ressource affichée sur 15 chargées · fin du registre à cette lecture");
  assert.equal(model.windowSummary({ shown: 2, loaded: 3, hasMore: false, noun: "brokers" }),
    "2 brokers affichés sur 3 chargés · fin du registre à cette lecture");
  present("shownOfLoaded");
  assert.equal(model.shownOfLoaded("events", 100, 100), "100 événements affichés sur 100 chargés");
  assert.equal(model.shownOfLoaded("events", 1, 1), "1 événement affiché sur 1 chargé");
});

test("le tri par défaut place les cycles ouverts, puis l’observation la plus récente, puis l’identifiant", () => {
  present("sortWorkers");
  const workers = [
    fleetWorker(1, { state: "terminal", quota_held: false, last_observation: observation(30) }),
    fleetWorker(2, { state: "running", last_observation: observation(10) }),
    fleetWorker(3, { state: "in_doubt" }),
    fleetWorker(4, { state: "running", last_observation: observation(20) }),
    fleetWorker(5, { state: "reserved" }),
    fleetWorker(6, { state: "reserved" }),
  ];
  assert.deepEqual(model.sortWorkers(workers, "activity").map((item) => item.worker_id),
    [workerId(4), workerId(2), workerId(5), workerId(6), workerId(3), workerId(1)]);
  assert.deepEqual(model.sortWorkers(workers, "identifier").map((item) => item.worker_id), workers.map((item) => item.worker_id));
  assert.deepEqual(model.sortWorkers(workers, "observation").map((item) => item.worker_id).slice(0, 3), [workerId(1), workerId(4), workerId(2)]);
  assert.equal(workers[0].worker_id, workerId(1), "le tri ne modifie pas la fenêtre reçue");
});

test("les points d’attention sont des faits du registre, sans gravité inventée", () => {
  present("workerFlags"); present("workerActivity");
  assert.deepEqual(model.workerFlags(fleetWorker(1)), []);
  assert.deepEqual(model.workerFlags(fleetWorker(1, { state: "in_doubt" })), ["in_doubt"]);
  assert.deepEqual(model.workerFlags(fleetWorker(1, { allocation_inconsistent: true })), ["allocation_inconsistent"]);
  assert.deepEqual(model.workerFlags(fleetWorker(1, { last_observation: observation(1, { oom_killed: true }) })), ["oom_killed"]);
  assert.deepEqual(model.workerFlags(fleetWorker(1, { launch_broker: registered(brokerId(1), { runtime_matches: false }) })), ["runtime_mismatch"]);
  assert.deepEqual(model.workerFlags(fleetWorker(1, { observation_broker: registered(brokerId(1), { broker: { state: "revoked" } }) })), ["association_not_current"]);
  assert.deepEqual(model.workerFlags(fleetWorker(1, { launch_broker: registered(brokerId(1)) })), [], "un bail courant n’est pas un point d’attention");
  for (const [state, activity] of [["reserved", "open"], ["preparing", "open"], ["created", "open"], ["start_issued", "open"], ["running", "open"],
    ["stopping", "open"], ["in_doubt", "uncertain"], ["terminal", "ended"], ["cancelled", "ended"]])
    assert.equal(model.workerActivity(state), activity);
});

test("erreur, source non accordée et lecture vide restent trois faits distincts", () => {
  present("fleetCoverage");
  const source = (phase, extra = {}) => ({ phase, loaded: 0, hasMore: false, stale: false, capped: false, ...extra });
  const empty = model.fleetCoverage({ workers: source("ready"), brokers: source("ready"), resources: source("ready"), contextsComplete: true });
  const refused = model.fleetCoverage({ workers: source("error"), brokers: source("error"), resources: source("ready"), contextsComplete: true });
  const absent = model.fleetCoverage({ workers: source("not_granted"), brokers: source("ready"), resources: source("ready"), contextsComplete: true });
  const text = (coverage, key) => coverage.notes.find((note) => note.source === key)?.text;
  assert.equal(text(empty, "workers"), "Workers : aucun worker enregistré à cette lecture réussie.");
  assert.equal(text(refused, "workers"), "Workers : lecture en échec, contenu inconnu. Une erreur n’est pas une absence d’activité.");
  assert.equal(text(absent, "workers"), "Workers : registre non accordé à ce contexte, aucune lecture émise.");
  assert.equal(new Set([text(empty, "workers"), text(refused, "workers"), text(absent, "workers")]).size, 3);
  assert.equal(text(model.fleetCoverage({ workers: source("forbidden"), brokers: source("ready"), resources: source("ready"), contextsComplete: true }), "workers"),
    "Workers : lecture refusée par le contrôle, données retirées.");
  assert.equal(text(model.fleetCoverage({ workers: source("loading"), brokers: source("ready"), resources: source("ready"), contextsComplete: true }), "workers"),
    "Workers : lecture en cours, contenu encore inconnu.");
  // Every composition stays partial: the runtime inventory is never part of these reads.
  for (const coverage of [empty, refused, absent]) {
    assert.equal(coverage.partial, true);
    assert.equal(text(coverage, "runtime"), "Inventaire runtime : inconnu. Ces registres ne prouvent ni exécution, ni arrêt, ni libération de quota.");
  }
  const more = model.fleetCoverage({ workers: source("ready", { loaded: 100, hasMore: true }), brokers: source("ready", { loaded: 3 }), resources: source("ready", { loaded: 300, hasMore: true, capped: true }), contextsComplete: false });
  assert.equal(text(more, "workers"), "Workers : 100 lignes chargées, d’autres pages existent au contrôle.");
  assert.equal(text(more, "brokers"), "Brokers : 3 lignes chargées, fin du registre à cette lecture.");
  assert.equal(text(more, "resources"), "Ressources : 300 lignes chargées, fenêtre de cette vue atteinte ; utilisez la vue Ressources pour la suite.");
  assert.equal(text(more, "contexts"), "Contextes : liste partielle, d’autres périmètres peuvent manquer.");
  assert.equal(text(model.fleetCoverage({ workers: source("ready", { loaded: 4, stale: true }), brokers: source("ready"), resources: source("ready"), contextsComplete: true }), "workers"),
    "Workers : 4 lignes de la dernière lecture réussie, périmées.");
  const single = model.fleetCoverage({ workers: source("ready", { loaded: 1 }), brokers: source("ready", { loaded: 1, stale: true }), resources: source("ready", { loaded: 1, hasMore: true }), contextsComplete: true });
  assert.equal(text(single, "workers"), "Workers : 1 ligne chargée, fin du registre à cette lecture.");
  assert.equal(text(single, "brokers"), "Brokers : 1 ligne de la dernière lecture réussie, périmée.");
  assert.equal(text(single, "resources"), "Ressources : 1 ligne chargée, d’autres pages existent au contrôle.");
  // Rows withdrawn after an unexplained refusal are neither a failed read nor an empty registry.
  assert.equal(text(model.fleetCoverage({ workers: source("forbidden"), brokers: source("forbidden"), resources: source("error", { withdrawn: true }), contextsComplete: true }), "resources"),
    "Ressources : données retirées après un refus sans motif lisible, contenu inconnu tant que vos accès ne sont pas relus.");
});

test("l’annonce d’état est courte, sans heure ni compte, et ne change pas tant que l’état ne change pas", () => {
  present("sourceAnnouncement");
  const source = (phase, extra = {}) => ({ phase, loaded: 3, hasMore: false, stale: false, capped: false, busy: false, ...extra });
  const steady = { workers: source("ready"), brokers: source("ready"), resources: source("ready") };
  assert.equal(model.sourceAnnouncement(steady), "Workers, brokers, ressources : lecture réussie.");
  assert.equal(model.sourceAnnouncement({ ...steady, workers: source("ready", { loaded: 120, hasMore: true }) }), model.sourceAnnouncement(steady), "ni compte ni pagination dans l’annonce");
  // A read in flight announces nothing: a successful cycle leaves the same sentence before and after it.
  assert.equal(model.sourceAnnouncement({ ...steady, workers: source("ready", { busy: true }) }), null);
  assert.equal(model.sourceAnnouncement({ ...steady, resources: source("loading", { loaded: 0 }) }), null);
  assert.equal(model.sourceAnnouncement({ ...steady, resources: source("idle", { loaded: 0 }) }), null);
  assert.equal(model.sourceAnnouncement({ workers: source("ready", { stale: true }), brokers: source("ready", { stale: true }), resources: source("ready") }),
    "Workers, brokers : données périmées, dernière lecture conservée. Ressources : lecture réussie.");
  assert.equal(model.sourceAnnouncement({ workers: source("error", { loaded: 0 }), brokers: source("error", { loaded: 0 }), resources: source("ready") }),
    "Workers, brokers : lecture en échec, contenu inconnu. Ressources : lecture réussie.");
  assert.equal(model.sourceAnnouncement({ workers: source("forbidden", { loaded: 0 }), brokers: source("forbidden", { loaded: 0 }), resources: source("error", { loaded: 0, withdrawn: true }) }),
    "Workers, brokers : lecture refusée, données retirées. Ressources : données retirées, accès à relire.");
  assert.equal(model.sourceAnnouncement({ workers: source("not_granted", { loaded: 0 }), brokers: source("not_granted", { loaded: 0 }), resources: source("ready") }),
    "Ressources : lecture réussie.", "une source non accordée n’est pas annoncée");
  assert.ok(!/\d/.test(model.sourceAnnouncement(steady)), "aucun chiffre : ni heure ni compte");
});

test("un heartbeat conservé d’une incarnation précédente est signalé comme tel", () => {
  present("heartbeatOfPreviousIncarnation");
  assert.equal(model.heartbeatOfPreviousIncarnation(broker({ broker_id: brokerId(1) })), false);
  assert.equal(model.heartbeatOfPreviousIncarnation(broker({ broker_id: brokerId(1), state: "unseen", last_heartbeat: null })), false, "sans heartbeat il n’y a rien à qualifier");
  const reissued = broker({ broker_id: brokerId(1), state: "unseen", incarnation_id: brokerId(77) });
  assert.notEqual(reissued.last_heartbeat.incarnation_id, reissued.incarnation_id);
  assert.equal(model.heartbeatOfPreviousIncarnation(reissued), true);
});

test("la fraîcheur distingue jamais lu, à jour, à actualiser et périmé", () => {
  present("freshness");
  assert.equal(model.freshness({ loadedAt: null, now: 1000, cadence: 30000, stale: false }), "never");
  assert.equal(model.freshness({ loadedAt: 1000, now: 31000, cadence: 30000, stale: false }), "fresh");
  assert.equal(model.freshness({ loadedAt: 1000, now: 61001, cadence: 30000, stale: false }), "aging");
  assert.equal(model.freshness({ loadedAt: 1000, now: 2000, cadence: 30000, stale: true }), "stale");
  // In manual mode the threshold stays below the 300-second life of a context, so the state can be reached.
  assert.ok(model.MANUAL_AGING_MS < 300000);
  assert.equal(model.freshness({ loadedAt: 1000, now: 100000, cadence: 0, stale: false }), "fresh");
  assert.equal(model.freshness({ loadedAt: 1000, now: 121001, cadence: 0, stale: false }), "aging", "en mode manuel le seuil est de deux minutes");
});

test("la session d’un worker n’est liée à une ressource que par identifiant exact", () => {
  present("sessionResource"); present("workersOfSession");
  const resources = [sessionResource(1), sessionResource(2), sessionResource(0xab), namedResource("agent", sessionId(3)), namedResource("sandbox", "sandbox-build")];
  assert.equal(model.sessionResource(resources, sessionId(1))?.resource.resource_id, sessionId(1));
  assert.equal(model.sessionResource(resources, sessionId(3)), null, "un agent de même identifiant n’est pas une session");
  assert.equal(model.sessionResource(resources, sessionId(0xab))?.resource.resource_id, sessionId(0xab));
  assert.notEqual(sessionId(0xab).toUpperCase(), sessionId(0xab));
  assert.equal(model.sessionResource(resources, sessionId(0xab).toUpperCase()), null, "aucune normalisation de casse");
  assert.equal(model.sessionResource(resources, sessionId(1).slice(0, -1)), null, "un préfixe ressemblant n’est pas lié");
  const workers = [fleetWorker(1), fleetWorker(2), fleetWorker(7, { session_id: sessionId(1) })];
  assert.deepEqual(model.workersOfSession(workers, sessionId(1)).map((item) => item.worker_id), [workerId(1), workerId(7)]);
  assert.deepEqual(model.workersOfSession(workers, "session-a"), []);
});

test("les accès affichés suivent exactement le catalogue d’opérations fourni par le contrôle", () => {
  present("supervisionAccess"); present("accessLines");
  const access = (context) => {
    const { workers, workersUnavailable, brokers, composed, resources, journal, content, export: exporting, any } = model.supervisionAccess(context);
    return { workers, brokers, composed, resources, journal, content, export: exporting, any, ...(workersUnavailable ? { workersUnavailable } : {}) };
  };
  const none = { workers: false, brokers: false, composed: false, resources: false, journal: false, content: false, export: false, any: false };
  const metadata = { resources_read: organization, events_read_metadata: organization, events_stream: organization };
  assert.deepEqual(access(null), none);
  assert.deepEqual(access(adminContext()), { workers: true, brokers: true, composed: true, resources: true, journal: true, content: true, export: true, any: true });
  assert.deepEqual(access(adminContext(["read_worker_registry"], { operations: catalogue({ workers_read: tenantScope, quota_read: tenantScope, usage_read: tenantScope }) })),
    { ...none, workers: true, any: true });
  assert.deepEqual(access(adminContext(["read_broker_registry"], { operations: catalogue({ brokers_read: tenantScope }) })), { ...none, brokers: true, any: true });
  assert.deepEqual(access(adminContext(["read_tenant_metadata"], { operations: catalogue(metadata) })), { ...none, resources: true, journal: true, any: true });
  assert.deepEqual(access(personalContext()), { ...none, resources: true, journal: true, content: true, export: true, any: true });
  assert.deepEqual(access(personalContext(["read_metadata"], { operations: catalogue({ resources_read: own, events_read_metadata: own, events_stream: own }) })),
    { ...none, resources: true, journal: true, any: true });
  // A registry the membership holds is offered by its administration context only: the personal one opens nothing.
  assert.deepEqual(access(personalContext(["read_worker_registry", "read_broker_registry", "read_tenant_metadata"], { operations: catalogue({
    brokers_read: ["requires_administration"], workers_read: ["requires_administration"], federation_read: ["requires_administration"] }) })), none);
  assert.deepEqual(access(personalContext([], { operations: catalogue() })), none);
});

test("la vue suit les opérations et non les capacités, y compris quand elles divergent", () => {
  const access = (operations, capabilities = adminContext().capabilities) => model.supervisionAccess(adminContext(capabilities, { operations }));
  // Every registry and metadata capability is present, but the control offers none of these operations.
  const withdrawn = access(catalogue());
  assert.deepEqual([withdrawn.workers, withdrawn.brokers, withdrawn.composed, withdrawn.resources, withdrawn.journal, withdrawn.any], [false, false, false, false, false, false]);
  assert.equal(withdrawn.workersUnavailable, undefined);
  // The worker registry is granted but the adopted source profile has no protocol for it: no read, and a named gap.
  const owned = access(ownedAdminOperations());
  assert.equal(owned.workers, false); assert.equal(owned.workersUnavailable, true); assert.equal(owned.composed, false);
  assert.equal(owned.brokers, true); assert.equal(owned.any, true);
  for (const availability of ["migration_required", "not_in_this_build"]) {
    const fenced = access(adminOperations({ workers_read: ["granted", "tenant", availability] }));
    assert.equal(fenced.workers, false, availability); assert.equal(fenced.workersUnavailable, undefined, availability);
  }
  // No capability at all, yet the control offers the broker registry: the offer is what the view follows.
  const offered = access(catalogue({ brokers_read: tenantScope }), []);
  assert.equal(offered.brokers, true); assert.equal(offered.workers, false); assert.equal(offered.any, true);
  // Content and export follow their own operations, never the metadata one.
  const metadataOnly = access(adminOperations({ events_read_content: ["not_granted"], events_export: ["granted", "own_and_shared", "migration_required"] }));
  assert.equal(metadataOnly.resources, true); assert.equal(metadataOnly.content, false); assert.equal(metadataOnly.export, false);
});

test("chaque ligne d’accès distingue accordé, non accordé, autre contexte et indisponible, avec son motif", () => {
  const lines = (context) => Object.fromEntries(model.accessLines(context).map((line) => [line.key, line]));
  const workersOnly = lines(adminContext(["read_worker_registry"], { operations: catalogue({ workers_read: tenantScope }) }));
  assert.deepEqual(Object.keys(workersOnly), ["workers", "brokers", "resources", "journal", "content", "export", "personal_workers", "agents"]);
  assert.deepEqual(Object.values(workersOnly).filter((line) => line.granted).map((line) => line.key), ["workers"]);
  assert.equal(workersOnly.workers.state, "granted");
  assert.equal(workersOnly.workers.detail, "Accordé par l’opération workers_read (portée : les registres du tenant) : états du contrôle, dernière observation runtime et associations d’autorité.");
  assert.equal(workersOnly.brokers.state, "not_granted");
  assert.equal(workersOnly.brokers.detail, "Opération brokers_read non accordée à ce contexte : aucune requête n’est émise.");
  assert.equal(workersOnly.agents.granted, false, "la relation agent/sous-agent est annoncée comme non fournie");
  assert.match(workersOnly.agents.detail, /Relation non fournie par le contrat nominatif/);
  const personal = lines(personalContext(["read_metadata"], { operations: catalogue({ resources_read: own, events_read_metadata: own, events_stream: own,
    workers_read: ["requires_administration"] }) }));
  assert.equal(personal.workers.state, "elsewhere"); assert.equal(personal.workers.granted, false);
  assert.equal(personal.workers.detail, "Opération workers_read non offerte par ce contexte. Votre appartenance porte ce droit, mais seul son contexte d’administration l’offre : ouvrez le contexte d’administration de la même appartenance.");
  assert.equal(personal.resources.detail, "Accordé par l’opération resources_read (portée : vos ressources et celles partagées avec vous) : ressources listées avec leurs propres permissions de métadonnées, de contenu et d’export.");
  const organizationScope = lines(adminContext());
  assert.match(organizationScope.resources.detail, /portée : les ressources de l’organisation/);
  const owned = lines(adminContext(undefined, { operations: ownedAdminOperations() }));
  assert.equal(owned.workers.state, "unavailable"); assert.equal(owned.workers.granted, false);
  assert.equal(owned.workers.detail, "Opération workers_read accordée (portée : les registres du tenant), mais indisponible. Le profil de sources adopté par le contrôle ne fournit pas cette lecture : aucune route historique n’est appelée, rien n’est déduit et l’activité reste inconnue.");
  const migration = lines(adminContext(undefined, { operations: adminOperations({ events_export: ["granted", "own_and_shared", "migration_required"] }) }));
  assert.equal(migration.export.state, "unavailable");
  assert.match(migration.export.detail, /attend la migration explicite des droits humains par l’opérateur/);
  for (const line of [...Object.values(owned), ...Object.values(migration)].filter((item) => item.state === "unavailable"))
    assert.doesNotMatch(line.detail, /\b(?:vide|aucune activité|zéro)\b/, `${line.key} : une lecture indisponible n’est jamais annoncée vide`);
  assert.ok(model.accessLines(null).every((line) => !line.granted && line.label && line.detail));
});

test("les filtres de brokers, ressources et événements restent des projections des lignes chargées", () => {
  present("filterBrokers"); present("filterResources"); present("filterEvents");
  const brokers = [broker({ broker_id: brokerId(1) }), broker({ broker_id: brokerId(2), state: "revoked" })];
  assert.deepEqual(model.filterBrokers(brokers, "revoked").map((item) => item.broker_id), [brokerId(2)]);
  assert.deepEqual(model.filterBrokers(brokers, brokerId(1)).map((item) => item.broker_id), [brokerId(1)]);
  assert.deepEqual(model.filterBrokers(brokers, "authority-a").length, 2, "une autorité déclarée est cherchable");
  const resources = [sessionResource(1), namedResource("agent", "agent-planner"), namedResource("sandbox", "sandbox-build"), namedResource("project", "project-atlas")];
  assert.deepEqual(model.filterResources(resources, { text: "", kind: "agent" }).map((item) => item.resource.resource_id), ["agent-planner"]);
  assert.deepEqual(model.filterResources(resources, { text: "atlas", kind: "all" }).map((item) => item.resource.resource_id), ["project-atlas"]);
  assert.equal(model.filterResources(resources, { text: "", kind: "all" }).length, 4);
});
