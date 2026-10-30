# Contrat core v2 — synthèse à implémenter

29 octobre 2026. **Conception uniquement ; aucun registry ni moteur exécuté ici.** Architecture existante conservée : gateway Rust et SQLite distinct par tenant ; broker Rust de confiance mono-hôte ; aucun nouveau service/dépendance/stockage. Une commande métier = une instance worker terminale ; les sous-agents ne deviennent pas des workers. L'[annexe détaillée](/tmp/ai-guard-worker-core-contract-detail.md) contient le DDL, transactions et matrice de tests ; cette synthèse fait foi sur les décisions de portée.

## DTO et autorité

Types version1, `deny_unknown_fields`, enums fermés, tailles/UUID/hash bornés. Aucun tenant libre ; la Store vient de l'identité authentifiée. Runner contrôle, viewer lecture ; worker interdit de ces routes.

```text
DecisionReceiptV1 {
  request_id, request_hash, phase: Initial|Approved,
  audit_seq, audit_hash
}
AdmissionV1 { operation_id, original: DecisionRequest, receipt, launch }
LaunchSpecV1 {
  session_id, command, profile, runtime_kind, authority_id,
  image_ref, immutable_image_id, network,
  cpu_millis=2000, memory_bytes=1073741824, pids_limit=256,
  timeout_seconds, fixed_security_preset, restart_policy=no,
  workspace_manifest_digest, import_policy_version,
  provider, capture_output, credential_binding_id?, credential_names
}
MutationV1 { operation_id, worker_id, attempt_id, expected_revision, payload }
MutationReceiptV1 { operation_id, worker_id?, revision?, input_hash, audit_seq, audit_hash }
StartResult { receipt, disposition: EmitOnce|AlreadyConsumed }
```

L'API décision doit ajouter un reçu complet ; `Authorization::Allowed` du runner le garde au lieu de jeter seq/hash/phase. Le hash historique ne change pas. **L'admission produit une nouvelle autorisation centrale complète**, sous politique courante/Jev hors verrou, puis recheck transactionnel. Elle compare l'original au reçu exact dans la base authentifiée et vérifie commande/profil/session, réseau, limites, durée et preset ; un vieux allow seul ne donne jamais un start.

Ce point est obligatoire : le P1 ingestion a prouvé qu'un ancien `security.decision` pouvait être usurpé par un runner. Le correctif interdit les nouvelles écritures libres des namespaces réservés ; l'historique reste conservé et d'origine indiscernable. Les anciennes décisions restent lisibles/idempotentes, sans conversion en droit d'exécution.

Aucune valeur de secret, env complet, chemin secret ou stdout dans le spec/registre. Refuser un spec dont le masquage modifierait un champ nécessaire à l'identité/reprise ; ne pas désactiver le masqueur. Credential binding est une référence privée du broker, pas une preuve publique des valeurs. Images, limites, mount et identité réelle seront recoupés par inspect avant claim.

## Routes et séquence minimale

| Route `/api/v1` | Effet |
|---|---|
| `POST /workers/reservations` | Admission + quota + worker/attempt stables + audit, même reçu au replay |
| `GET /workers/operations/{id}`, `/workers[/{id}]`, `/quota`, `/usage` | Vérité tenant ; inconnues, quota retenu et action reprise explicites |
| `POST /workers/{id}/prepare`, `/create-issued`, `/created` | Helpers/nom avant émission ; identité et spec réellement inspectés après create |
| `POST /workers/{id}/start-claim` | Consommation irréversible avant start ; nouvelle transition seulement = EmitOnce |
| `POST /workers/{id}/observations`, `/unknown` | Observation typée ; terminal complet finalisé atomiquement, sinon InDoubt |
| `POST /workers/{id}/stop`, `/cancel-reserved`, `/cleanup-result` | Intentions et preuves contrôlées ; aucune promesse d'effet moteur depuis gateway |

Même operation+contenu → même reçu ; divergence409. Lookup idempotence avant CAS, mais état courant vérifié avant nouvel effet. Claim rejoué → **AlreadyConsumed**, jamais un ancien EmitOnce remis comme permission. Réponse fraîche perdue → inspection/réconciliation ; pas de retry start. Un verrou SQL/lease ne fence pas un ancien CLI : contrat d'autorité/verrou OS à intégrer avec le propriétaire runtime.

États : `reserved → preparing → created → start_issued → running/stopping → terminal`, avec `in_doubt` conservateur. Un job déjà exited au premier inspect passe directement terminal, avec preuve historique de démarrage distincte, sans Running courant inventé. Annulation limitée à Reserved avant création/claim possibles ; elle interdit centralement tout claim futur. StartIssued ambigu ne devient pas NeverStarted sur un not-found ou TTL.

## SQL, migration et invariant audit

Même connexion/transaction IMMEDIATE pour état, quotas, ledger et audit. Tables v2 proposées :

```text
control_meta(singleton PK, namespace_uuid UNIQUE, genesis_seq FK events, genesis_hash)
workers(worker_id PK, admission_operation_id UNIQUE, request_id UNIQUE,
        receipt, spec/spec_hash, policy_hash, state, quota_held,
        cpu_millis, memory_bytes, revision, snapshot/snapshot_hash, last_event_seq FK)
worker_attempts(attempt_id PK, worker_id UNIQUE FK, authority, stable_name,
        volume_name, CID, create_seq, created_seq, start_seq UNIQUE,
        start_operation_id UNIQUE, start_emitter_id)
worker_observations(observation_id PK, attempt FK, input_hash, payload, event_seq UNIQUE FK)
worker_allocations(attempt PK/FK, terminal_observation UNIQUE FK,
        dates, duration_ns, cpu_millicore_ns, memory_byte_ns, method, billable=false, event_seq FK)
worker_mutations(operation_id PK, worker_id? FK, action, input_hash, event_seq UNIQUE FK)
```

Quota sous transaction : COUNT et SUM CPU/RAM des `quota_held=1`, comparés à limites explicites (0..100 slots, budgets CPU/RAM, durée maximale). Aucun compteur libre ni volume choisi depuis session_id fourni. IDs/noms/volume viennent du contrôle serveur ; les labels servent au recoupement, pas à l'autorité.

Terminal complet exige même autorité/CID/instance, terminal reconnu, running=false, dates/exit présents, restart_count=0 et limites inspectées stables. Transaction unique : observation + allocation unique + worker terminal + libération + audit + mutation ; puis commit checked. Produits u128 contrôlés, décimaux exacts en API, billable=false. Unknown/restart/date/limites incohérents → aucune durée zéro, slot retenu. Cleanup seulement après reçu terminal ; panne conserve conteneur arrêté/preuves. Stop de sûreté possible si audit indisponible, mais sans nouvelle exécution ni terminal fictif.

Chaque événement de contrôle après genesis porte action, operation/input_hash, révision précédente et snapshot après. Startup/intégrité/changement externe : rejouer ces transitions et comparer **ensemble exact des lignes** aux projections ; libération/claim/ledger altéré → latch503. Les CHECK SQL seuls ne suffisent pas. Garder protections schéma exact, inode/nlink, chaîne/quota et commit. Les primitives Store restent privées ; POST /events ne mute jamais le registre.

Migration recommandée explicite, contrôle offline/quiescent : sauvegarde cohérente distincte vérifiée, préflight v1 exact/chaîne saine, IMMEDIATE, DDL v2 exact, genesis audité et namespace, `user_version=2`, validation puis commit. Les anciens événements/seq/hashes sont conservés ; `tenant_binding` conserve sa structure v1. Version future/partielle refusée sans réparation. Pas d'adoption implicite de workers legacy ; pas de restauration qui rendrait un start consommé à nouveau disponible : autorité désactivée et réconciliation avant reprise.

## Petit lot utilisable et preuves

Fondation puis intégration peuvent être séquencées, **sans annoncer livré avant le vrai runner** : reçu complet + plan versionné/reprise ; réservation avant volume/helpers/worker ; create/inspect stable ; claim sous verrou puis une émission ; observations/terminal/ledger central avant cleanup ; CLI status/resume/stop réels. Les helpers fixes restent du contrôle, bornés et sans credentials, pas des workers supplémentaires.

Tests obligatoires : A/B/rôles/spoof ; reçu exact et ancien faux allow refusé par admission fraîche ; concurrence deux connexions/processus aux quotas ; replay/restart/divergence ; un seul claim frais et aucune réémission après réponse perdue ; cancel/claim en course ; terminal+ledger+libération atomiques ; Unknown/restart/limites/dates ; corruption projection↔audit ; migration non vide et rollback avant commit ; vrais crashs create/start/terminal/cleanup et absence de deuxième effet. Docker/Podman et plateformes ont des preuves distinctes ; mocks ne qualifient pas le runtime.

Les décisions root structurantes sont prises. À figer avant code : valeurs initiales des quotas/config ; identité/verrou runtime et source privée credentials avec son propriétaire ; maintenance/restauration et clôture sûre des états non consommés après préparation. Aucune implémentation registry démarrée, aucun exactly-once moteur, HA, prix ou qualification100 workers revendiqués.
