# Broker — revue indépendante des contrats avant code

30 octobre 2026. Direction structurelle déjà approuvée ; aucune nouvelle permission demandée. Revue limitée aux deux propositions et à leurs coutures dans le code actuel. Aucun code/doc du dépôt édité, test/service/runtime/API lancé ou secret réel lu. Les lots API/auth et HTTP restent clos ; aucun résultat n'est transféré à la supervision.

## Identités et verdict

- Contrôle `/tmp/ai-guard-broker-control-contract.md` : `48428b22390ffbe42bb36abfd51a586ca54bd8eccd482c6bdc55bd4d1ef4b220` (121 lignes).
- Runtime `/tmp/ai-guard-broker-runtime-contract.md` : `8f6e141daa2eb85eefda0fdd6690b0334059cce8441e1aec3af2017b913365de` (140 lignes).
- Sources recoupées : control `7f2422c1…5b6`, audit/control `7ed543d6…e9e7`, audit `a6e967bc…86c7`, runner `d8ee798b…2ea4`, runner/managed `a1a4bea2…0619`, runtime `7e878ebc…4a42`, runtime/managed `411dd20c…028a` ; hashes complets disponibles par les fichiers courants identifiés, pas de source produite ici.

**Avis : contrats proches d'une base implémentable, mais pas encore gelables tels quels.** Deux incompatibilités démontrées avec les chemins actuels et une règle d'authentification insuffisamment définie demandent une correction coordonnée. Ce sont des constats de conception, pas des vulnérabilités d'un broker déjà livré. D5/restauration et supervision OS restent des obligations ouvertes, non des risques acceptés par l'accord structurel.

## BPR-01 — P1 potentiel de conception : identité IPC validée au startup, fraîcheur et credential effectif non liés

Références : runtime lignes 32–38, 44–55, 74 ; contrôle 18–20. Code à ne pas réutiliser sans adaptation : `runner.rs:1011` et `runner/managed.rs:2105–2130` relisent le runner du configRoot ; `check_identity` à 498 authentifie ensuite **ce** client.

Le runtime propose un index `digest(token) → Principal` confirmé au startup, puis un profil contenant une référence runner. Il interdit justement de substituer le runner à un viewer, mais ne définit ni invalidation du cache après révocation/changement de rôle, ni identité du credential porté par `AuthenticatedCaller`/`PreparedRun` vers les RPC. Le contrôle exige pour sa part une authentification du credential présenté avant accès au profil.

Séquence possible si ces interfaces sont implémentées littéralement : T est runner au démarrage ; le gateway révoque T (ou le passe viewer), et le profil tourne vers T2 ; le broker continue à reconnaître T depuis son index ; `prepare_once` ouvre le profil/source, puis un adaptateur partagé relit T2 et fait les RPC avec T2. T peut alors emprunter un credential courant qu'il n'a jamais présenté. Même sans relecture de T2, accéder aux données privées via le cache périmé avant le refus HTTP ne respecte pas l'ordre annoncé.

**Statut : risque conditionnel démontré par la combinaison des interfaces, pas exploitation observée.** Choix root requis : l'index ne sert qu'au routage vers l'origin approuvé ; authentifier le credential présenté avec rôle/tenant/namespace actuels avant nouvelle action et accès aux profils/jobs, et lier le client RPC à cette identité/credential validé. Pas de relecture d'une autre clé dans l'adaptateur. Définir invalidation/rotation et indisponibilité fail-closed. Les actions autonomes d'un job déjà accepté et son stop préarmé ont une autorité distincte ; elles ne doivent pas rendre un nouveau demandeur autorisé.

Preuve nécessaire : T runner→viewer/révoqué pendant la vie du broker, profil T2 valide ; T refuse submit/status/export/approve avant source et sans RPC signé T2. T2 valide continue selon son propre périmètre. Gateway indisponible ne transforme pas l'ancien cache en nouveau grant.

## BPR-02 — P2 confirmé : tous les scans exigent un parent terminal, mais la reprise d'import en a besoin avant start

Références : contrôle lignes 40, 82–86 ; runtime 76–81. `runner/managed.rs:2842–2953`, particulièrement 2947 : `recover_import` appelle `managed_scan` si tar est terminal mais qu'aucun writer ACK ne prouve que toute l'archive est arrivée. Le parent est encore en préparation, sans create/start métier, et le scan compare le volume à `checkpoint.before`.

Reproduction logique : import helper termine → broker meurt avant confirmation de transfert → reprise retrouve le helper terminal → comparaison indépendante du volume requise → le nouveau contrat refuse Scan car le parent n'est pas terminal. La reprise légitime ne peut plus atteindre le premier start. Supprimer ce scan, déduire la copie complète du seul exit0 ou utiliser un helper legacy créerait une régression de sûreté.

Choix recommandé : ajouter une finalité centrale fermée de vérification d'import (ou variante de Scan explicitement discriminée), liée à l'import terminal exact, parent Preparing/quota held/non consommé et manifeste admis. Commande/policy immuables, armement/claim propres, deadline et observation/cleanup centraux comme tous les helpers. Le scan après exécution et l'export conservent leur parent terminal. Revalider les préconditions parent/permission au **claim**, pas seulement à l'armement.

Preuve nécessaire : crash entre tar terminal et ACK de transfert, reprise sans nouveau start import, verification helper armé une fois, manifeste exact permettant la suite ; divergence refuse la suite et conserve les preuves. Aucune exemption à l'armement des helpers.

## BPR-03 — P2 confirmé : port stop proposé valide le snapshot après l'effet

Références : runtime lignes 110–116 ; design approuvé `BROKER_SUPERVISION.md:46`. `runtime/managed.rs:746–764` : `stop_owned(handle)` n'accepte pas le snapshot Created. `observe` vérifie le handle/autorité/nom, puis `kill_target` peut être appelé ; la comparaison complète n'intervient qu'ensuite dans la proposition. `runtime.rs:1232–1264` contient cette comparaison fingerprint/image/allocation/restart.

Séquence : armement C créé avec configuration F → configuration/allocation de C change tout en conservant CID/nom/autorité → expiration → `stop_owned` constate running et émet kill → la validation au retour détecte F différent trop tard. Cela contredit le contrat approuvé de refus d'effet sur cible divergente ; le seul CID exact ne prouve pas toute la cible armée.

Choix recommandé : un port de stop préarmé recevant le snapshot complet, qui inspecte/compare les faits exigés **avant** tout kill et revalide l'autorité avant l'effet. Inspection manquante/divergente → Unknown sans kill sous ce contrat ; observation post-effet comparée pour finalisation. Ne pas dupliquer tout le runtime : extraire la primitive commune de validation/stop. La course d'une mutation administrative après inspection doit rester explicitement bornée par la frontière de confiance, pas présentée comme une attestation atomique moteur.

Preuve nécessaire : snapshot validé puis fingerprint/ressources/nom/image divergents à l'inspection de stop → aucune émission kill ; cible exacte → arrêt ciblé ; erreur/absence → Unknown/quota retenu. Un Created arrêté reste non allouable sans preuve terminale suffisante/D5.

## Choix techniques à figer avec la correction, sans nouvelle permission

1. **Stop sans blocage du tick.** Ne pas confondre absence de lecture de PlanState avec absence de FS dans les primitives existantes : `Runtime::command` (`runtime.rs:568`) appelle `InvocationAnchor::verify`, qui ouvre/relit/hash l'exécutable (`runtime/invocation.rs:215–224, 342–378`) synchroniquement avant le timeout CLI. Conserver cette protection, mais placer préparation/inspection/stop dans la capacité d'effet isolée du tick, avec saturation bornée et stratégie pour appels non annulables. Ne pas publier « aucun FS sur tout le stop » ni une borne garantie par `timeout` seul. Un effet bloqué pour A ne doit pas immobiliser le tick et le slot de B. C'est une couture à préciser, pas une nouvelle preuve de panne réelle.
2. **Dernière lecture de clock.** Runtime 116 est correct : après on_created, fsync, HTTP/claim et vérifications d'autorité. Préciser aussi après la construction du Command et ses vérifications FS, et après préparation TTY ; entre validation du permit non clonable et émission, aucun nouvel await/IO de préparation pouvant consommer toute l'échéance. L'horloge centrale n'est qu'une valeur déclarée algébriquement validée. Échec après claim = claim consommé et enforcement, jamais nouveau permit par retry.
3. **Restauration/clone.** Contrôle 115 et runtime 136 exigent correctement fencing, mais ne donnent pas encore de procédure activable. Cas à traiter explicitement : backup avant claim → start réel et claim postérieurs → restauration de l'ancien backup valide. La ligne claim n'y existe plus ; `AlreadyConsumed` ne peut pas protéger ce rollback. L'état restauré/cloné doit rester maintenance sans nouveau claim/admission jusqu'au fencing conservateur de l'ancienne autorité/CID et fermeture des émetteurs/RPC en vol. Définir les preuves et le point d'activation ; sans capacité D5 prouvée, la restauration avec reprise d'exécution reste bloquée. Flock ou audit local valide ne suffisent pas à distinguer deux racines clonées.
4. **Helper schema/API.** Figer ensemble la policy compilée, les permissions reçues, la nouvelle finalité d'import, les clés uniques et replay. Les routes helper `{id}` doivent comparer ce path au worker du reçu/arm chargé dans le Store authentifié, même si le body n'a pas de `worker_id` direct. Une permission est vérifiée contre l'opération durable, pas acceptée parce que son JSON ressemble à un reçu. ExportRequest doit lier son UUID opaque à une intention privée de destination immuable avant ACK ; rotation de chemin interdite sous le même UUID.
5. **Cutover unique.** Schéma3 doit refuser les premiers claims/admissions legacy et conserver seulement les replays/maintenance explicitement permis. Ne pas activer automatiquement sur DB neuve ni au seul succès des tests Store. Batch, TTY/gardien et tous helpers doivent être opérationnels avant cutover. L'étape batch de développement peut exister sous mode explicite et refuser Interactive avant freeze ; elle ne constitue pas la qualification du produit complet.

## Éléments cohérents, à préserver

- La version2 proposée est celle d'un **record de contrôle privé/audité**, pas une modification implicite de `WorkerV1`, des réponses legacy ou de RunPlan3. Le replay doit dispatcher fermé par kind/version, reconstruire arm+transition, conserver les anciens hashes et vérifier toutes les projections ; la proposition le demande correctement.
- DDL exact/auto-index counts sont des propositions à confirmer par SQLite, non une preuve ; `require_v2`, schéma, genèses, tables attendues et loader doivent évoluer ensemble. `tenant_binding.schema_version=1` reste distinct de `PRAGMA user_version=3`.
- Migration explicite v2→v3 avec backup neuf, fsync, contrôle du head et quiescence ; v1 passe d'abord par sa migration existante. Defaults actuels conservés pendant réalisation. Ajouter l'armement dans une seconde transaction après le claim est explicitement interdit.
- Arms/claims ont domaines séparés, IDs globaux via worker_mutations, receipt exact, CAS de révision et replay sans nouvelle échéance. u128 décimal canonique/checked arithmetic, boot/méthode et UTC diagnostic répondent au besoin ; aucun hash historique n'est réinterprété.
- Runtime 79/116 ferme explicitement la fenêtre HTTP/fsync avant émission et conserve la deadline absolue. Sink d'observation non bloquant et writer séparé ne prétendent pas rendre durable un événement seulement en mémoire.
- L'armement worker n'est pas présenté comme couverture des helpers ; admission/permission/claim des helpers et export autorisé sont des exigences obligatoires. Les allocations worker ne reçoivent pas les timestamps helper ; usage inconnu reste inconnu, billable=false.
- Les bindings fournisseurs figés sont réouverts par digest, pas par nouvelle sélection Auto/source tournée. Stop sans clé fournisseur ; frames/records ne contiennent ni token/Env ni contenu privé de binding.
- IPC/guardian, adaptation clock native, services OS, A1/A2/B1, D5, macOS vivant figé et charge100 restent à implémenter/qualifier, avec preuves distinctes. Aucune installation n'est déduite de l'approbation du code.

## Décision recommandée à root

Faire corriger BPR-01/02/03 dans une seule révision coordonnée des deux contrats, arrêter les coutures ci-dessus puis relire ce delta avant de figer les DTO/ports transverses. Les travaux préparatoires sans effet peuvent avancer ; aucun stub, fallback legacy ou helper non armé ne doit rendre le parcours prétendument fonctionnel. Statut présent : revue **effectuée**, contrat **À CORRIGER**, implémentation/tests/qualification **NOT_RUN**. Aucun audit global ou nouveau consentement utilisateur requis.
