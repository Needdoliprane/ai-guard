# Unified supervision view

[English](README.md) · [Français](README.fr.md) · [Guides](../README.md)

**Candidate implementation of one component, not a qualified release.** This guide describes two destinations of the named dashboard. **Supervision** is one page that joins the broker registry, the worker registry and the authorized resources of the context you selected. **Journal** lists every event that context may read, newest first, then live. It shows what the control recorded and returned to you. It does not prove that a worker is running, that a broker is reachable, or any capacity. The interface is in French only: this guide quotes every label as displayed, with an English gloss where the meaning is not obvious. Evidence, source attribution and open gaps are in the [integration record](../../engineering/evidence/2026-11-10/unified-control-ui-integration/README.md); the visual and behavioural contract is in [ui.md](../../ui.md).

The view needs a named account: [local](../identity/README.md) or [SSO](../sso/README.md). The technical `viewer` token opens the nine historical views, not this one.

## Who sees what

Navigation, sections and actions follow the capabilities that the control returns for the selected context. The browser never derives a right from an account name; every read is authorized again by the gateway.

| Capability in the selected context | What Supervision shows | Read sent |
|---|---|---|
| `read_worker_registry` and `read_broker_registry`, administration mode | Brokers with their associated workers, dense worker table | `GET /api/v1/human/federation/workers?worker_limit=100&broker_limit=100` |
| `read_worker_registry` only | Worker table; brokers announced as "not granted" | `GET /api/v1/human/workers?limit=100` |
| `read_broker_registry` only | Broker table; worker associations announced as unknown | `GET /api/v1/human/brokers?limit=100` |
| `read_metadata`, or `read_tenant_metadata` in administration mode | Authorized sessions, agents, sandboxes and projects | `GET /api/v1/human/resources?limit=100` |
| Same metadata capabilities | **Journal** tab: authorized events, newest first, then live | `GET /api/v1/human/events/recent?limit=100&view=metadata`, then `GET /api/v1/human/stream?…&cursor=<live_cursor>` |
| None of the above | No Supervision entry for that context | none |

A source that the context does not grant is never requested. The **Accès de ce contexte** panel (access of this context; closed by default, opened with its chevron) lists each capability, what it opens and what is missing. Two items are always listed as unavailable: the personal view of workers (no authenticated initiation chain links a worker to a person yet) and the agent / sub-agent hierarchy (the named contract does not return it, so it stays unknown and is never inferred from a name).

## Open the view

1. Sign in with your named account and wait for **Choisir un contexte.**
2. Each offered context keeps its existing **Ouvrir …** button and gains **Superviser administration · `<tenant>`** or **Superviser personnel · `<tenant>`** when it grants at least one source.
3. The heading is **Supervision du tenant.** for an administration context and **Supervision de mon espace.** for a personal one. The **Supervision** tab stays available while the context is selected.

Leaving the view for another tab aborts its reads and drops its rows; what you typed in the filters is kept while the same context stays selected. Changing context, signing out or losing the context removes everything, including open details.

## Read the screen

- **Freshness.** Each source (Workers, Brokers, Ressources) shows its own state and the time of its last successful read: `Lecture en cours…`, `Lecture réussie`, `Données à actualiser` (older than twice the refresh period, or two minutes in manual mode), `Données périmées · dernière lecture conservée` after a failed refresh, `Contrôle injoignable`, `Lecture refusée`, `Données retirées · accès à relire`, `Non accordé`. A window of several pages is as fresh as its oldest page: it then shows `Pages lues entre … et …` (pages read between … and …) instead of `Lu le …`.
- **Coverage.** The banner always says **Couverture partielle**: the page joins independent registry snapshots and never a runtime inventory. One line per source states what was loaded, whether more pages exist, or why nothing is known. A failed read, a source that is not granted and a successful empty read are three different sentences.
- **Counts.** Every count is "n displayed of N loaded". No tenant total is ever shown; `d’autres pages existent au contrôle` means the registry holds more rows than the loaded window.
- **Control state versus runtime observation.** `Cycle ouvert` groups the non-terminal control states (`reserved` … `stopping`). It is not an observed execution. `Observation inconnue` means no runtime inspection is recorded.
- **Broker lease.** `Bail courant` does not prove a running worker; a stale, expired or revoked lease proves neither a stop nor a released quota.
- **Authority associations.** A worker is listed under a broker only when one of its two authority associations (launch, inspection) names that exact broker. The association is historical and may be later than the worker fact: it never proves which broker ran or inspected the session. Workers whose launch authority is not registered, unknown, or was read while the broker registry was unavailable are grouped under that status instead of being hidden.
- **Points d’attention** (points of attention) are facts returned by the registry (`Allocation incohérente`, `Issue incertaine`, `Arrêt mémoire relevé`, `Runtime différent du broker associé`, `Broker associé sans bail courant`). They carry no severity and no diagnosis.

## Search, filter and load more

Search and filters apply to the loaded rows only, and the result line says so. Search is a case-insensitive literal match of every term (128 characters at most, no pattern syntax) on exact identifiers and state names. Filters: control state group, declared runtime, broker or association status, runtime observation, points of attention; sort by state then latest observation, latest observation, or identifier.

The broker table has its own search, **Rechercher dans les brokers chargés**, over the broker rows of the page that was read (identifier, incarnation, lease state, declared authority); it never filters the workers.

The registry is sorted by identifier on the control side and has no server-side state filter. **Charger 100 lignes de plus** adds one page, up to 500 worker rows; beyond that, use the exact **Workers** registry view. Workers with an open cycle can therefore exist outside the loaded window: an identifier that is absent here is not absent from the tenant.

With several worker pages loaded, each page is its own snapshot and the broker snapshot of the last page read is the one displayed.

## Keyboard and announcements

- Every action is reachable with Tab, Enter, Space and Escape. A detail is a modal dialog; Escape returns to the row that opened it.
- A command that starts a read stays in the tab order while a read is in flight: it is announced as unavailable, ignores another activation, and keeps the focus during the read and after it. This holds for **Actualiser maintenant**, **Relire depuis les plus récents**, **Charger 100 lignes de plus**, **Charger 100 ressources de plus**, **Charger 100 événements plus anciens**, **Lire les événements de cette session**, **Réessayer** and **Reprendre …**, and for **Actualiser** in the destination bar while one of the two views or the context list is open. A page you loaded stays in sight with its button, ready for the next one.
- A command that its own read removes hands the focus to what replaces it, never to the top of the page: the note at the end of a list, or its result line, when there is nothing more to read; the result line of the Journal, or **Actualiser maintenant** in Supervision, once the error that **Réessayer** belonged to is gone; the heading of the page for **Reprendre**. A retry that fails again keeps its error line and the focus on **Réessayer**. **Limiter** and **Retirer la limite** move the focus to the result line at once. **Afficher N lignes de plus** renders loaded rows without any read and continues on the first of them. A stopped read budget disables these commands and removes **Réessayer**; in the exact **Brokers**, **Workers** and **Fédération** views, **Actualiser** is disabled during their own read and the focus moves to the current destination.
- Each view has a single announcement region, in its banner: a short sentence when the state of a source changes (for example `Workers, brokers : données périmées, dernière lecture conservée. Ressources : lecture réussie.`), never on an automatic refresh that changes nothing. An error is announced once, when it appears. The result lines (`24 workers affichés sur 24 chargés · …`) follow every keystroke on screen and are not announcement regions themselves: the banner region says their count.
- In Supervision, the count of a list (`18 workers affichés sur 24 chargés.`) is announced once you paused for half a second after a search or a filter, however many keys you typed; lists changed within the same pause are said together. A read that changes a count says it when it ends, in the same sentence as the state if it changes too: the first read, a page you loaded, a refresh that changes the number of rows. An automatic refresh that changes the content of rows (a worker that changed state, for example) without changing a state or a count announces nothing, and neither does a change of sort order. A read you asked for (**Actualiser maintenant**, **Actualiser**, **Réessayer**, a page you loaded) always says how it ended: when it changed neither a state nor a count, the region says the state of the sources again (`Workers, brokers, ressources : lecture réussie.`), or the error still displayed when it failed the same way as before. **Voir ces workers** moves the focus to the result line, which is read from there. An open detail has its own region and announces the outcome of **Lire les événements de cette session** in one sentence, never the list of events.
- In the Journal, the result line changes with every live batch: live events are announced as a count, at most every 15 seconds, and the result of a search or of a filter once you paused for half a second. A read you asked for (**Charger 100 événements plus anciens**, **Limiter**, **Retirer la limite**, **Relire depuis les plus récents**, **Réessayer**, **Actualiser**) announces its result once, when it has ended and the live stream it reopens has settled, and only if it succeeded; when the state changes with it, one sentence gives the state, then the result. A read you asked for that fails is told by the new state and by its error line; when it fails again the same way, with the same line, the region says that error again. A live batch never announces a result.
- When the sentence to say is the one the region already holds (the same result after the same reload, for example), the region is emptied and the sentence is written again a tenth of a second later, once, so that it is a new announcement and not an unchanged text. An automatic refresh that changes nothing and a live batch never trigger this.
- Rows that did not change are neither rebuilt nor moved by a refresh or a live batch, including when another row changed and took a new place in the sort order: a text selection, the focus and a click in progress survive it. A row whose content changed is rebuilt: the focus returns to the same control of that row, a text selection inside it is lost. When the sort order itself changes, rows that have to change place keep the focus but lose a text selection.
- An open detail is updated section by section: only the sections whose content changed are replaced. A refresh that changes nothing else replaces the line that carries the read time or the read receipt, so a text selection and the focus stay in the rest.
- With many destinations the bar scrolls sideways without a scrollbar; the edge that hides more destinations fades out, and a destination that takes the focus is scrolled fully into view.
- Not verified: screen readers and a WCAG audit (see the lot handoff). The announcements above are observed as writes in the page, with their texts, their region and their order; whether a given screen reader speaks each of them, an emptied and rewritten region included, is not established.

## Correlate

| From | To | Key | What it proves |
|---|---|---|---|
| Broker | Workers | Exact broker identifier in a worker's launch or inspection association | A historical authority association |
| Worker | Session resource | `session_id` equals a granted `session` resource identifier, exactly | A declared session reference; no owner is attributed |
| Session | Events | Server read filtered on that session | Events the control authorizes for this context |
| Agent ↔ session, agent ↔ sub-agent | — | Not returned by the named contract | Unknown |

In a worker or session detail, **Lire les événements de cette session** sends one authorized read (`GET /api/v1/human/events/recent?limit=25&view=metadata&resource_kind=session&resource_id=…`) and lists the 25 most recent events of that session. Nothing is read before you ask. **Ouvrir cette session dans le Journal** continues in the Journal, limited to that session. If the control answers `404 not_found`, the detail shows a refusal and the rest of the page stays as it was: the control does not distinguish an absent resource from one this context may not read. Content and export stay in the **Ressources** and **Événements** views.

From the Journal, **Limiter** re-reads the journal for the resource of that event on the server, and **Workers** opens Supervision with the search set to that session identifier; the refresh period and the other choices you made there are kept.

## Journal and live

The **Journal** tab answers "what just happened" without paging through history.

1. It reads the 100 most recent authorized events (`order: newest_first`). **Charger 100 événements plus anciens** goes further back, up to 1 000 events in the window; beyond that the summary asks you to restart from the newest or to limit the journal to one resource.
2. It then opens the existing stream at the position returned with that page (`live_cursor`). The stream starts strictly after the page: no gap, and no replay of history. New events are inserted on top, without duplicate, and **Direct** shows how many arrived since the last read.
3. Search and the decision, provenance and resource-type filters apply to the loaded events only. The resource limit is the only server-side filter.

| **Direct** state | Meaning | What to do |
|---|---|---|
| `Connexion au direct…` | The stream is being opened | wait |
| `Direct connecté` | New authorized events arrive on top | — |
| `Direct déconnecté · reconnexion` | The connection dropped, or the control answered that it is unavailable for now (`retry`: gateway unreachable from the dashboard, capacity full for a moment). The stream is reopened from the last delivered position with a growing delay (1 to 30 s). The window is marked stale meanwhile | wait, or **Relire depuis les plus récents** |
| `Direct suspendu · onglet masqué` | The tab is hidden: the stream is closed and nothing is read | return to the tab: the journal is read again from the newest and a new stream starts after it. **Relire depuis les plus récents** in the visible tab does the same |
| `Direct refusé` | The stream was refused: too many streams for this account, the read budget stopped, or a refusal (HTTP 403) that does not carry the control's error contract, as an intermediary in front of the dashboard may answer. A refusal is never retried by itself | follow the message, then **Relire depuis les plus récents** |
| `Direct arrêté` | No stream: the last read failed, or the stream could not be reopened after six attempts | **Réessayer**, or **Relire depuis les plus récents** |

The Journal lists metadata of events attached to resources this context may read. Read receipts and events of resources that are not granted never appear. It is neither the complete audit nor a runtime inventory. The **Événements** tab keeps its role: one chosen resource, with content and export according to your rights, read from the oldest event.

## Expiry, new context and resume

A context lasts at most 300 seconds. The control never renews it: it may offer a new one.

1. When the context expires, every row, detail, cursor and stream of the view is removed, and the dashboard reads your identity again.
2. If the control then offers **exactly one** context with the same tenant, organization, mode, membership revision and capabilities, the context list shows **Reprendre la supervision · …** (or **Reprendre le journal · …**) and gives it focus. No data is read before you confirm. After the click the same view opens with the filters you had typed, and data is read again. The offered context has its own life of at most 300 seconds. If the list stays open beyond it, the offer says that the proposed context has expired in turn (`a expiré à son tour`); **Reprendre** then reads your accesses and reopens the view on the equivalent context the control proposes at that moment, without any other click. While the control still proposes the expired context, the dashboard says so and repeats that read as in step 3, up to three times for each such click whatever was read before it; the view reopens by itself as soon as an equivalent context is offered. Once those automatic reads have stopped or one of them has failed, a new click is needed. A hidden tab reopens nothing and starts none of those repeated reads: if the tab is hidden before the view has reopened, the confirmation ends there, even when the control offers a valid context meanwhile. The dashboard says that the resume was interrupted (`Reprise interrompue`) and reads your accesses when you come back; if the control then offers a context with the same rights, the resume is offered again, with the focus, for a new click.
3. If the control still offers the expired context, the dashboard reads the identity at most three more times (after 2, 4 and 8 seconds) and then stops; none of these reads starts while the tab is hidden. **Actualiser** reads it once more whenever you choose; the resume is offered as soon as a new equivalent context exists. This happens when the browser clock runs ahead of the control's own measure of the context lifetime, for example after the host was suspended.

While the tab is hidden the two views start no read: the refresh cycle and the live stream are suspended, and the repeated identity reads of steps 2 and 3 do not start. Two things still happen there. A read that was already in flight ends. And when the context is lost (it expired, or a read in flight was refused), its rows are removed at once and your identity is read once, unless the read budget stopped, so that the context list is current when you come back; a resume offered that way waits for your click like any other.

No resume is offered after a revocation or a change of rights, after a refusal, after a sign-out, when you went back to the context list yourself, when the read budget stopped, for another account, or more than 15 minutes after the context was lost. Through the dashboard, an expiry noticed first by the control appears as an unknown context, a refusal or changed accesses, and also leads to the explicit choice. The resume intention holds the open view, the filters you typed, the refresh period and the identifiers you had selected (the resource the Journal was limited to, the broker filter); it stays in memory and is never stored.

With SSO, each token renewal replaces the dashboard session. The same rule applies: the next context list offers the resume, says that the session was renewed (`Votre session a été renouvelée`) rather than that a context expired, and nothing is read before you confirm.

## Refusals and errors

| Shown | Cause | Action |
|---|---|---|
| `Lecture refusée`, rows removed | `403 permission_denied` on a registry or resource read | **Relire mes accès**, then select a context the control currently offers |
| `Lecture refusée` on Workers and Brokers, `Données retirées · accès à relire` on Ressources | `403` on the composed read of both registries. The client retires its rows as soon as that refusal arrives, without reading its reason: a revoked or expired context cannot be told from a refused registry. Every source and open detail of the view is therefore withdrawn and your accesses are read once | nothing: a context that is revoked or expired sends you back to **Choisir un contexte.**; if the control still offers the context, the resources are read again and only the registries stay refused |
| Back to **Choisir un contexte.** | Context expired, revoked or no longer known to the control | Select an offered context. After an expiry only, confirm the offered resume to get the same view and filters back; no row of the previous context is kept |
| Sign-in form | `401`, credential refused or expired | Sign in again |
| `Contrôle injoignable` | Network failure, timeout, or the dashboard cannot reach the gateway | **Réessayer**; the last successful window stays, dated and marked stale |
| `Données indisponibles` with the server message | Storage or audit unavailable (`503`) | **Réessayer**; if the message asks for the operator, contact them |
| `Capacité temporairement atteinte` | Temporary `429 capacity_exhausted` | Retry shortly |
| Automatic reads stopped | `429 audit_read_budget_exhausted` with `contact_operator` | Ask the operator; then **Relire après intervention de l’opérateur**, which reads the registries first and the resources once that read reopened the budget |
| `Revalidation des accès impossible` after returning to the tab | The identity could not be read when the tab became visible again | nothing: the view leaves its suspension and its own read reports the real state, with **Réessayer** if the control is still unreachable |

An error is never rendered as an empty registry: without a successful read the tables say `Workers inconnus`, not `Aucun worker`.

## Read budget and refresh period

Every authorized read leaves a durable receipt, and the dashboard proxy adds one identity receipt per read. These receipts count against a cumulative per-account budget that neither a new context nor a restart refills.

| Action | Receipts |
|---|---|
| One refresh cycle, both registries, one worker page | 3 |
| Each additional loaded worker page in a cycle | 3 |
| Resources (explicit refresh only) | 2 per page |
| Events of one session | 2 |
| Journal: each page read (newest or older) | 2 |
| Journal limited to a resource, first read answered `404` before the route ever answered: one unfiltered read of one event settles it | 2 |
| Journal: opening the stream | 1 |
| Journal: each non-empty live batch | 1 |

The fleet refreshes every 30 s while the tab is visible; choose 2 min or **Manuel** under **Rythme d’actualisation**. Resources are re-read only on **Actualiser maintenant**. Automatic refresh stops when the tab is hidden, when more than two worker pages are loaded, when a read was refused, and when the budget is exhausted. A context that grants no registry has nothing to refresh automatically: it shows no refresh period and says so (`Aucun registre n’est accordé à ce contexte`). A context lasts at most 300 seconds and is never renewed implicitly, which bounds an unattended page.

## Limits

- No runtime inventory, no usage, quota, user or policy administration: the 360° target of [ui.md](../../ui.md#vues-utilisateur-et-administration-unifiée--cible-à-livrer) remains open.
- One tenant per context. No cross-tenant view.
- No server-side filter or sort of the worker registry; at most 500 worker rows, 100 brokers and 300 resources in this view.
- No agent hierarchy and no personal view of workers.
- The Journal window holds at most 1 000 events; a page cut by that bound is reported as a reached window, never as the beginning of the selection. The **Ressources** and **Événements** views still open their stream from the beginning of the authorized history; only the Journal starts after the page it read.
- A view left open does not stay live indefinitely: the context ends after 300 seconds and the resume is a deliberate click.
- French interface only.

## Contract of `GET /api/v1/human/events/recent`

Additive read of the gateway, also admitted by the dashboard proxy at this exact path. Headers: `Authorization: Bearer …` and one `X-AiGuard-Context`. Closed parameters:

| Parameter | Value |
|---|---|
| `limit` | 1 to 100, default 25 |
| `view` | `metadata` (default) or `content` |
| `resource_kind` and `resource_id` | both or neither; restricts to one authorized resource |
| `cursor` | opaque `next_cursor` of the previous page of the same selection |

```json
{
  "version": 1,
  "context_id": "<context>",
  "authorization_version": "<64 hex>",
  "view": "metadata",
  "order": "newest_first",
  "coverage": "authorized_selection",
  "items": [],
  "has_more": false,
  "next_cursor": null,
  "live_cursor": "<opaque>",
  "read_receipt": { "request_id": "<uuid>", "audit_event_id": "<uuid>", "received_at": "<RFC 3339>" }
}
```

- `items` are the same projections as `/human/events`, from the most recent to the oldest. `has_more` equals `next_cursor != null` and reports older authorized events.
- `live_cursor` is present on the first page only (no `cursor` in the request). `/human/stream` accepts it with exactly the same `limit`, `view` and resource, and then delivers only events recorded after that page. No audit position is exposed. Any other limit or selection answers `400 invalid_cursor`.
- Authorization, masking and the 256 KiB envelope are those of `/human/events`. So is the traversal budget of 1 000 pages and 16 MiB, which applies **per traversal**, that is per chain of cursors, during the life of the context: a request without `cursor` starts a new traversal, and `live_cursor` starts a fresh stream traversal. It is not a ceiling per context. The cumulative bound is the human read receipt budget of the account (see [Read budget and refresh period](#read-budget-and-refresh-period)), together with the read frequency and the read slots of the tenant. A traversal that exceeds its budget answers `429 capacity_exhausted` with `select_context`. A cursor of one route is refused by the other.
- Each page records one durable `control.identity.access` receipt with the existing action `events`, in the same store transaction as its authorization and before anything is returned; it counts in the human read budgets and does not distinguish this read from the forward read. A receipt may remain when a refusal that comes after that transaction (revocation, expiry, traversal budget) prevents the page from being published: a receipt does not prove that a page was delivered.
- After any identity mutation of the tenant, a revoked credential or an expiry, the route, a `next_cursor` already issued and a `live_cursor` already issued are refused without any item (`403 context_revoked`, `context_expired` or `context_invalid` with `select_context`; `401` with `authenticate`), and a stream that was open ends with an error.
- Errors: `400 invalid_request` or `invalid_cursor`; `401`; `403` for a context refusal or `permission_denied`; `404 not_found` when the filtered resource is not granted or does not exist; `429 capacity_exhausted` or `audit_read_budget_exhausted`; `503`.

The route is documented in [docs/api.md](../../api.md). Its integration on `2cc29ee` and the checks still to run are tracked separately from the historical lot in the [integration evidence](../../engineering/evidence/2026-11-10/unified-control-ui-integration/README.md).

## Version compatibility

No schema migration, no new audit kind, no configuration change and no new dependency. Update the gateway and the dashboard together.

| Gateway | Dashboard binary with its `front/dist` | Result |
|---|---|---|
| With the route | With the route | Everything in this guide |
| Without the route | With the route | The Journal shows **Lecture récente indisponible sur ce contrôle** and opens no stream; session events in Supervision fall back to the forward read, oldest first, and say so; every other view works |
| With the route | Older | The route is simply unused |

A gateway without the route answers `404 not_found` for it, exactly like a resource that is not granted. Without a resource filter that answer can only mean the route is absent. With a filter it stays ambiguous until the route has answered once in the session, so the client never turns it into a refused selection by itself: the session events of Supervision use the forward read as the authority, and the Journal settles it with one unfiltered read (answered: the resource is not granted, `Ressource non accordée`; `404` again: the route is absent). A newer `front/dist` behind an older dashboard binary is not a supported combination: both ship together.

## Verify as a contributor

All commands run from the repository root with the pinned toolchain; none starts a container runtime or a model. The browser scenarios also need the `playwright` package (1.64.x) and its Chromium, which are not dependencies of the frontend package: make them resolvable through `NODE_PATH`.

```sh
cargo test --locked --test supervision_stack_api --test human_recent_events
cd front && npm ci && npm test && npm run build
```

One test of `human_recent_events` waits for the real 300-second lifetime of a context and is ignored by default: run it with `cargo test --locked --test human_recent_events -- --ignored`. Left out, it is not run.

`examples/supervision_stack.rs` is a test harness, not a trial path: it serves the real gateway and dashboard routers on loopback over synthetic data built through real control mutations, for the browser scenarios of `front/tests/browser_supervision.mjs`.

```sh
cargo build --locked --example supervision_stack
cd front && AI_GUARD_SUPERVISION_STACK=../target/debug/examples/supervision_stack \
  AI_GUARD_SUPERVISION_SCENARIO=fleet node tests/browser_supervision.mjs
```

Scenarios: `fleet`, `partial`, `correlation`, `rights`, `revoked`, `isolation`, `expiry`, `resume`, `ttl` (waits for a real 300-second expiry), `journal`, `keyboard`, `focus`, `regions`, `details`, `states`, `load`. The script exits with code 2 and `BLOCKED` when Playwright, the binary or `front/dist` is missing, or when the stack does not start in time; such a scenario is `NOT_RUN`. The 120 registry rows of the data set are real admissions without any runtime: they exercise the interface with dense data and qualify no worker capacity.
