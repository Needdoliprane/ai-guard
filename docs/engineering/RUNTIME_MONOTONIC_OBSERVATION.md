# Runtime monotonic diagnostics / Relevé monotone du runtime

## Scope (English)

This optional, process-local recorder helps attribute a runtime delay. It is
neither durable audit nor an independent engine observation, and grants no
permission. A stopwatch around `execute` cannot distinguish image resolution,
creation, runtime deadline polling, STOP process emission, attach exit, capture
EOF, terminal inspection and cleanup. The recorder marks those actual code
boundaries using one `std::time::Instant` origin and a trace UUID. Each operation
has its UUID; exact immutable CIDs are decoded into 32 bytes. Names, argv,
environment, credentials, captures and user payloads never enter these records.

`Runtime::with_observation` is opt-in. Producers only `try_send` a closed event
into a bounded Tokio channel (capacity 1–4096); no IO, consumer callback, await,
lock shared with a writer, or retention operation is added to STOP. Overflow,
consumer loss and unrepresentable identifiers set a sticky diagnostic-loss bit.
They do not change execution, STOP, cleanup, errors or deadlines. The loss bit
invalidates diagnostic completeness; absence of a phase is INCONCLUSIVE.
`DeadlineArmed` records the existing planned batch deadline after the actual
attach spawn; `DeadlineObservedByPoller` is when the poller handled expiry,
not an independently observed OS alarm. Spawn-request and spawn-return bound the
actual native CLI spawn. `StopInvocationRequested` marks entry to the Rust
capture pipeline; `StopInvocationReturnedOk/Error` reports only that function's
result. Neither says a process was spawned or reaped. `StopCommandExitedOk/Error`
is emitted inside the physical IO owner only when its actual `Child::wait`
returns a status; Error means that known status is unsuccessful, including a
signal termination. `StopCommandWaitFailed` marks a failed wait without claiming
an exit. A capture/preflight/join error never creates a command-exit phase.
The exit event marks handling the wait result, not the engine's receipt of STOP;
even a successful STOP command is not terminal proof.
Capture EOF is recorded only after the real read future returns EOF. There is
no inferred EOF from an attach exit or global await.

The full diagnostic path currently targets legacy **Managed batch** execution.
Prearmed transports, interactive TTY, daemon enforcement, Kubernetes and direct
compatibility paths have different/partial coverage and must not inherit this
qualification. Resolution events precede `Submitted` (the `execute_retained`
API boundary) in the test harness. Diagnostic operation IDs do not establish a
human instruction or durable broker authorization.

## Qualification procedure (English)

The ignored public case remains `runtime_confinement` /
`real_worker_confinement_copy_export_and_timeout`. It requires explicit
`AGENTSANDBOX_TEST_RUNTIME`, `AGENTSANDBOX_TEST_RUNTIME_PROGRAM`,
`AGENTSANDBOX_TEST_RUNTIME_ENDPOINT` and `AGENTSANDBOX_TEST_IMAGE`.
Docker connects through existing `ManagedRuntime`; Podman additionally reopens
`AGENTSANDBOX_TEST_PODMAN_AUTHORITY`, a separately admitted existing private
binding. The case does not bootstrap/replace that marker or remove its volume.
It does not authorize an engine run by its existence: perform the separately
admitted read-only preflight of exact program/endpoint/image and protected IDs.

Preserve timeout **1 second**, worker sleep **60 seconds**, whole-call guard
**30 seconds**, all existing probes, limits and copy/export assertions. The guard
still includes image resolution and native cleanup; no duration is inferred
from engine timestamps. Every operation writes private 0600 intent, actual
created, fresh terminal and native cleanup receipts. Diagnostics persist in
`monotonic.jsonl` through a separate bounded consumer (4096 records, 1 MiB file
cap); its failure leaves the initial INCONCLUSIVE marker. Neither outputs nor
provider secrets enter the trace. Error cleanup requires the actual created
handle and a fresh matching native terminal; unknown/nonterminal resources are
retained. Read-only `inspect`/`ps` probes keep their explicit pinned endpoint and
256/64 KiB capture limits, and use the same retained CLI IO pipeline as ordinary
control commands. Timeout requests termination of only that created CLI Child,
then collects its actual wait and readers; it does not drop a consuming timeout
future. Existing drain bounds report faults without inventing a join: an unknown
wait/blocked reader may keep collection pending. The 30-second case guard and
external cancellation still do not prove a completed IO owner.
Execution/cleanup errors own their original error and secondary causes, the real
created handle, any actual counter-observation, and a successful retained Managed
result when one exists. Unknown/nonterminal observations never authorize `rm`.
After actually awaiting the diagnostic consumer, the case retains simultaneous
execution, inventory, consumer, finish-signal, receipt and timeline failures in
one typed aggregate; a secondary error cannot replace the primary error or its
IO. Consumer errors also retain previously collected diagnostic events. Final
exact-session inventory is read-only. Source copy, new export destination, named
volume and receipts are retained.

The timeout timeline requires resolution, submission/create, exact CID, start
spawn, arm/planned deadline, poller expiry, STOP request, Rust invocation entry,
native spawn, actual successful CLI wait, successful capture-pipeline return,
attach exit, both EOFs, terminal inspection, collected drainage, operation return
and cleanup. Missing phase, trace/operation/CID mismatch, diagnostic loss,
truncation, writer failure or missing `complete.json` is INCONCLUSIVE, never
PASS. A timing/confinement assertion failure remains FAIL even with a complete
trace. Protected-resource pre/post checks remain separate admission evidence.
Build/tests/engines/providers for this source are NOT_RUN until recorded by
Root/Luna. No historical result is transferred and no production gate is closed.

## Portée (français)

Ce collecteur optionnel et local au processus sert à attribuer un délai. Il ne
constitue ni un audit durable, ni une preuve indépendante du moteur, ni une
permission. Un chronomètre autour de `execute` ne distingue pas résolution de
l’image, création, observation de l’échéance par le poller, émission du processus
STOP, fin attach, EOF, inspection terminale et nettoyage. Les événements viennent
de ces frontières réelles, avec une origine `std::time::Instant`, un UUID de
trace, l’UUID d’opération et le CID immuable exact décodé en 32 octets. Aucun nom,
argv, environnement, credential, capture ou contenu utilisateur n’est enregistré.

`Runtime::with_observation` est explicite. Les producteurs emploient uniquement
`try_send` dans un canal Tokio borné (1–4096). Aucun IO, callback consommateur,
attente, verrou partagé avec le writer ou opération de rétention n’entre dans
STOP. Saturation, déconnexion ou identifiant non représentable positionnent une
perte diagnostique irréversible, sans modifier exécution, STOP, nettoyage,
erreurs ou délais. Une phase manquante reste INCONCLUSIVE.
`DeadlineArmed` décrit l’échéance batch existante après le vrai spawn attach ;
`DeadlineObservedByPoller` décrit son traitement par le poller, jamais une alarme
OS indépendante. Les deux événements spawn encadrent la création native du CLI.
`StopInvocationRequested` marque l’entrée dans la collecte Rust et
`StopInvocationReturnedOk/Error` son résultat ; ils ne prouvent ni spawn ni wait.
`StopCommandExitedOk/Error` n’est émis que par le propriétaire IO au retour réel
de `Child::wait` avec un statut connu ; Error désigne ce statut non réussi, signal
compris. `StopCommandWaitFailed` décrit un wait en échec sans prétendre à une
sortie. Refus préalable, capture ou join en erreur ne fabriquent aucune sortie
CLI. L’horodatage marque le traitement du wait, jamais l’acceptation de STOP par
le moteur. Le retour réussi de STOP ne prouve pas le terminal. Un EOF provient
du retour réel du lecteur ; ni la sortie attach ni un await global ne le fabriquent.

Le chemin complet concerne actuellement l’exécution **Managed batch legacy**.
Transports préarmés, TTY, enforcement daemon, Kubernetes et compatibilité directe
ont une couverture différente ou partielle. Le harnais résout l’image avant
`Submitted`, frontière de l’API `execute_retained`. Les UUID diagnostiques ne
prouvent ni instruction humaine ni autorisation broker durable.

## Procédure de qualification (français)

La cible publique ignorée reste `runtime_confinement` /
`real_worker_confinement_copy_export_and_timeout`. Exiger sélection explicite du
moteur, programme, endpoint et image avec les quatre variables précitées. Pour
Podman, rouvrir aussi le binding privé existant et admis séparément ; aucun
bootstrap ou remplacement du marqueur, aucun retrait de son volume. Le préflight
lecture seule du programme, endpoint, image et des ressources protégées reste
une admission distincte ; le code du test n’autorise pas son exécution.

Conserver timeout **1 s**, sleep **60 s**, garde entière **30 s**, probes, limites
et assertions copie/export. La garde inclut encore résolution et nettoyage.
Chaque opération conserve intention, vrai handle créé, terminal frais et reçu
de nettoyage natif en 0600. Un consommateur séparé persiste `monotonic.jsonl`
(4096 records, 1 Mio maximum). Sa panne conserve le marqueur INCONCLUSIVE initial.
Le harnais ne nettoie qu’un terminal frais correspondant au handle réellement
créé ; les ressources inconnues ou non terminales restent conservées. Les probes
`inspect`/`ps` gardent leur endpoint explicite épinglé et leurs captures 256/64 Kio,
avec le propriétaire CLI retenu commun aux commandes de contrôle. Le timeout
demande l’arrêt du seul Child créé puis collecte son vrai wait et ses lecteurs,
sans jeter un futur englobant. Les bornes de drainage existantes signalent un
défaut sans inventer de join : wait inconnu ou lecteur bloqué peuvent garder la
collecte pendante. La garde 30 s et l’annulation externe ne prouvent toujours pas
la fin du propriétaire IO.
Les erreurs d’exécution/nettoyage possèdent la vraie erreur primaire et les
causes secondaires, le handle créé, la contre-observation réelle éventuelle et
le résultat Managed réussi retenu s’il existe. Inconnu/nonterminal n’autorise
jamais `rm`. Après le véritable await du consommateur, un agrégat typé garde
ensemble exécution, inventaire, consommateur, signal de fin, reçu et timeline ;
une erreur secondaire ne remplace ni cause primaire ni IO. Une erreur du
consommateur garde aussi les événements déjà collectés. Inventaire final de la
session en lecture seule, source copiée, destination d’export neuve, volume et
preuves conservés. Les ressources protégées exigent leurs contrôles
pré/post séparés dans l’admission.

Pour le timeout, exiger toutes les phases listées en anglais, même origine,
opération et CID, échéance prévue identique et observation du poller après celle-ci.
Phase absente, divergence, perte, troncature, panne du writer ou absence de
`complete.json` : INCONCLUSIVE, jamais PASS. Un oracle de délai/confinement en
échec reste FAIL même avec une trace complète. Compilation, tests, moteurs et
fournisseurs de cette source restent NOT_RUN avant les preuves Root/Luna.
Aucun PASS historique transféré et aucun gate production clos.
