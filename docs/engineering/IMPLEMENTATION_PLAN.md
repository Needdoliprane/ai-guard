# Plan d'industrialisation

> Exécution autonome demandée par le mainteneur, avec agents spécialisés, périmètres exclusifs et revue indépendante. Les exigences sont dans AGENTS.md et ARCHITECTURE_CONTRACT.md ; aucune validation intermédiaire de plan n'est requise par la mission.

Objectif : satisfaire les exigences complètes local-first/cloud et produire des preuves reproductibles, sans assimiler un lot à une qualification du produit. Rust/SQLite et frontend TypeScript existants sont conservés. Les décisions structurelles restent soumises au mainteneur.

## Cycle actif : établir puis sécuriser la référence

### Lot P1 livré — authentification avant traitement HTTP

Besoin constaté le 30 octobre dans `src/api.rs` : les extracteurs JSON/query s'exécutent avant l'autorisation placée dans le corps du handler ; une requête non authentifiée peut donc imposer lecture/désérialisation. Le balayage des tenants recalcule également les digests de la même entrée et des credentials configurés à chaque comparaison. Il s'agit de défauts à reproduire et corriger, pas d'une reproduction rétroactive du rapport de sécurité figé.

Conception retenue dans les frontières existantes : index privé des digests construit au démarrage, identité typée obtenue par `FromRequestParts` avant Path/Query/Json, rôle runner requis avant lecture de tout corps de mutation. L'identité choisit toujours tenant et Store ; aucun identifiant client ne les sélectionne. Pas de middleware avec deuxième authentification, endpoint nouveau, dépendance ou changement de stockage. `token_matches` public reste compatible pour le dashboard.

Propriétaires : agent core pour `src/api.rs` et un éventuel module `src/api/auth.rs` ; Luna pour `tests/api_auth_boundary.rs` et exécution des tests ; root pour contrats/documentation/intégration ; revue indépendante après delta gelé. Aucun accès à une clé réelle, moteur ou fournisseur n'est nécessaire.

- [x] Conserver un RED sur le code inchangé : corps non autorisé non consommé, rôle viewer refusé avant JSON, query invalide non analysée avant auth ; cas déjà sûrs conservés comme PASS.
- [x] Vérifier sur un vrai HTTP local que des headers non authentifiés obtiennent 401 sans attendre le corps annoncé ; un seul gateway enfant possédé par le test Rust, aucun processus en fond hors supervision du harnais.
- [x] Implémenter index et guards typés, borne de token 4096 octets, refus des headers Authorization ambigus, sans prétendre à un lookup constant-time ou à une résistance globale au DoS.
- [x] Préserver `/healthz` public, aliases `/api` et `/api/v1`, séparation A/B, rôles et quotas, 503 en configuration d'identité invalide et erreurs habituelles des entrées authentifiées invalides. Aucune mutation d'audit sur ces refus.
- [x] Exécuter la nouvelle cible et les régressions pertinentes : 6 + 45 PASS, fmt/Clippy/build et revue acceptés ; aucune campagne E2E/moteur/fournisseur systématique.
- [x] Synchroniser contrats, suivi des candidats d'audit et preuves ; prochain P1 indépendant : arrêt HTTP/SSE borné. Aucun gate global clôturé par ce lot.

En parallèle, la proposition de supervision permanente dans `BROKER_SUPERVISION.md` a été actualisée pour les plans fournisseurs v3, relue puis approuvée par le mainteneur. Cette direction est maintenant autorisée à l'implémentation. Aucune installation de service OS ni migration existante implicite.

### Lot P1 livré — arrêt HTTP/SSE borné

Diagnostic : gateway et dashboard attendent sans limite `with_graceful_shutdown` dans `main.rs`, alors qu'un SSE valide peut rester ouvert indéfiniment. L'ingress possède déjà une borne de drainage de 5 s. Reprendre ce comportement pour les deux services, dans le même binaire et sans changer la donnée d'audit ou le flux SSE en fonctionnement normal.

Propriétaires : core pour `src/main.rs` uniquement (helper privé si nécessaire), Luna pour `tests/http_shutdown.rs` et ses exécutions, root pour documentation et intégration. API/auth reste gelé. Baseline main et release auth préservées dans `/tmp/ai-guard-http-shutdown-baseline` avant édition.

- [x] RED avant modification : vrais gateway/dashboard avec SSE authentifiés effectivement ouverts, SIGTERM ciblé, sortie bornée absente ; processus possédés par le test Rust, arrêt forcé du seul child en dépassement et échecs conservés. Contrôle sans flux déjà passant.
- [x] Démarrer le délai de 5 s au signal, jamais au démarrage. Laisser finir les requêtes ordinaires puis sortir même si le SSE reste ouvert ; nommer la fermeture après drainage expiré sans promettre un arrêt temps réel sous blocage OS/IO global.
- [x] Préserver les signaux/arrêts rapides au repos, health publique, accès viewer et requêtes normales ; pas de nouvel endpoint, paramètre tenant, dépendance ou changement de stockage. Setup SIGINT/SIGTERM fallible avant readiness, plus d'expect dans ce chemin.
- [x] GREEN : deux tests HTTP/SSE, trois régressions ingress/tenants, fmt/Clippy/build et revue acceptés. Aucun Docker/Podman, navigateur ou modèle pour ce lot.
- [x] Documenter fenêtre de drainage, flux interrompus et reprise par curseur ; conserver séparément les limites keep-alive/clients lents/audit bloqué et la supervision runtime après mort du broker.

### Prochain chantier autorisé — supervision durable

Le mainteneur a approuvé l'implémentation de `BROKER_SUPERVISION.md` (broker permanent et gardien TTY du même binaire Rust). Les conditions de constructibilité BSR-01/02/03 sont traitées au niveau design. Installation/chargement OS, cloud/PKI et destruction de données ne sont pas autorisés par cet accord. Les exigences D5/fencing et récupération du service figé demeurent ouvertes jusqu'à preuves.

- [ ] Verrouiller le contrat central ArmTarget worker/helper, reçu/CAS/claim, migration sauvegardée et compatibilité legacy, avec interfaces existantes réellement identifiées dans le code.
- [ ] Verrouiller IPC/profils, identité avant sélection de profil, Intent/Checkpoint, horloge continue, ownership runtime/output et ordonnanceur indépendant avant édition transversale.
- [ ] Implémenter puis qualifier la verticale complète broker batch, y compris import/scan/export supervisés, perte du client, crash/restart et absence de double start ; ne pas activer un nouveau schéma obligatoire avant disponibilité de ses clients.
- [ ] Implémenter et qualifier le gardien et le parcours TTY complet sans fallback batch ou régression finale du transport actuel.
- [ ] Traiter fencing des starts ambigus, panne/gel du service, mises à jour/migrations et recette A1/A2/B1 ; conserver les limites et gates tant que non prouvés. Aucun composant simulé ou simple timer local ne remplace ces obligations.

Premier sous-lot immédiatement applicable : arrêt sur snapshot vérifié. La revue BPR-03 a établi que `stop_owned(handle)` ne compare pas le Created complet avant kill. Le nouveau port `stop_snapshot_owned(&OwnedContainerSnapshot)` doit valider CID, nom, image, fingerprint/version, restart et allocation avant effet, revérifier l'autorité moteur, puis comparer l'observation résultante. Une inspection manquante ou divergente refuse l'effet et conserve l'incertitude ; un Created non running ne devient pas une allocation terminale.

- [x] Luna : reproduction ciblée de l'ancien port, puis six cas de divergences/snapshot exact/erreur d'inspection avec fixture moteur Rust explicitement identifiée ; un test de fixture ne qualifie aucun moteur.
- [x] Runtime : `src/runtime.rs` et `src/runtime/managed.rs`, arrêt strict pour les chemins possédant une baseline, y compris timeout et annulation interactive. Contrat des anciens chemins sans snapshot distinct ; aucune garantie de supervision ajoutée.
- [x] Root : six arrêts de `src/runner/managed.rs` raccordés aux snapshots complets ; terminal/audit/usage/cleanup inchangés.
- [x] Luna : défaut initial reproduit sur Docker réel, CLI tué après Running avec worker de courte durée. Premier essai non concluant (comparaison erronée tag/image immuable), puis unique retry corrigé : Running après7,0019s malgré limite5s. Aucun modèle ni test global A1/A2/B1. Deux arrêts/réconciliations ciblés, audit/quota/usage vérifiés, preuves et volumes conservés ; [résultats](evidence/2026-10-30/cli-crash/README.md).
- [x] Revue indépendante acceptée, 32 tests ciblés, formatage/Clippy/release PASS ; [preuves et limites](evidence/2026-10-30/runtime-stop-snapshot/README.md). Ce sous-lot ne ferme pas la supervision permanente ni les gates de crash/reprise.

Préparation centrale : [DTO et helper policy](evidence/2026-10-30/supervision-contract/README.md) compilés/testés/relus, initialement sans API ni activation par défaut. Nil boot et lien révision claim/arm corrigés par RED/GREEN, hashes legacy conservés. Le [Store transactionnel v3](evidence/2026-10-30/supervision-store/README.md) est ensuite validé indépendamment : 94 tests uniques,24 concernés rejoués après mutex final, revue close et fmt/Clippy/release PASS. Avant tout premier start v3, import exact, manifeste concordant et autorité fraîche privée sont requis. Clone/reopen restent maintenance ; legacy Created n'est pas réactivable.

Sous-lot suivant [API et horloge native](evidence/2026-10-30/supervision-api-clock/README.md) : neuf routes v1 uniquement, guards avant corps, parent helper retrouvé dans le Store authentifié ; producteur d'horloge continue Rust avec boot retenu et arithmétique vérifiée. Le journal immuable, l'IPC, l'ordonnanceur et les consommateurs de cette horloge restent à intégrer. Aucun cutover avant batch/helpers/TTY qualifiés, aucune réactivation implicite du schéma3 ; les résultats et limites de validation restent dans le dossier du lot.

### Lot livré — intention d'exécution immuable

Spécification : [intention/checkpoint du design approuvé](BROKER_SUPERVISION.md#intention-immuable-checkpoints-et-secrets). Base `11eb871db40ca9893a4492547561ec604247257c`, worktree `codex/broker-supervision`. Besoin : le hash d'une exécution ne peut porter sur `RunPlan.managed`, modifié après chaque transition ; il doit conserver le choix fournisseur et la demande initiaux. Aucun nouveau service ni changement structurel hors de la direction déjà approuvée.

Fichiers : core possède `src/broker/intent.rs`, l'export de `src/broker/mod.rs` et une couture privée bornée dans `src/runner.rs`. Un helper pur limité à la comparaison du prompt dans `src/provider.rs` peut réutiliser le parseur bootstrap existant, sans modifier ses appels historiques. Luna possède `tests/broker_intent.rs` et ses éventuels JSON de référence dans `tests/fixtures/`. Root possède suivi, intégration et preuves. Les autres modules et les plans historiques restent inchangés.

Contrat public du composant : `ExecutionIntentV1::from_plan_bytes(&[u8])`, `from_bytes(&[u8])`, `validate()`, `canonical_bytes()` et `intent_hash()`, avec résultat et erreur typés. `from_plan_bytes` projette le vrai `RunPlan` via une fonction privée au crate, sans exposer son type ou ses credentials. Les données figées sont privées, sans setter ; getters de métadonnées et snapshots typés pour le futur journal. Aucun appel réseau, accès au disque, résolution Auto ou vérification supposée du contenu d'un binding par ces fonctions.

Le format1, distinct des plans1/2/3, contient version/plan_version/operation_id/session/options et demande initiales, provider_input et référence de binding facultatives. Projection d'options fermée, sans modifier rétroactivement le serde de `RunOptions`. Pas de `managed`, de FD, de bytes de credential ni d'approbation déjà appliquée. UUID non nil, versions connues, empreintes hexadécimales64, forme/cohérence commande/session/profil/transport/fournisseur vérifiées ; une entrée ancienne sans identité initiale suffisante est refusée, jamais réparée ou enrichie implicitement. Ce refus du nouveau composant ne modifie pas la lecture des anciens plans.

Limites : plan source32Mio au plus, intention canonique256Kio au plus ; commandes et métadonnées soumises aux bornes existantes du domaine, chaînes privées restantes bornées. Aucun path canonicalize ni lecture de source pour fabriquer le hash. Le JSON canonique trie récursivement les clés, conserve ordre des tableaux et texte ; le SHA256 porte sur `ai-guard.execution-intent.v1`, octet NUL, puis ces octets UTF-8. Les UUID typés ont leur représentation serde canonique explicite. Les hashes historiques de décision/admission/provider ne changent pas. Debug/Display/erreurs du composant restent opaques pour les valeurs privées.

Points de revue : options inconnues dans le nouveau format ; session/argv/provider incohérents ; approval préexistante ; binding tourné ou absent ; références privées révélées par diagnostics. Le binding désigne seulement des métadonnées à ce stade : sa présence/digest réels seront exigés avant l'ACK du journal, sans substitution ni extraction OAuth.

Portée précise du hash : identité de l'invocation et des références présentes dans le plan, sans attester les bytes d'un credential manuel. Le journal doit engager avant ACK un enregistrement privé immuable liant `intent_hash`, payload Submit exact, scope authentifié et bindings des handoffs manuel/fournisseur (identité, nom privé, digest, noms autorisés). En mode manuel, persister depuis le `PrivateFile` retenu initial, synchroniser fichier/parent, puis engager le marqueur d'acceptation ; un Snapshot éphémère unlink-on-drop ne suffit pas. Le rejeu relit cet engagement et le même binding, jamais `secret_env_file` ou Auto ; binding absent/altéré bloque admission/start sans nouvelle source. Ce contrat du journal est à implémenter/tester séparément ; il n'est pas livré par cette projection pure et ne change pas les plans1/2/3.

- [x] Luna : RED ciblé avant code, avec golden indépendant du domaine/octet NUL et JSON canonique ; ce RED préparatoire n'est pas une vulnérabilité du broker existant.
- [x] Core : format/validation/hash et projection privée ; mêmes octets/hash après mutations managed/pending/résultat ; changement de champs stables et de binding couvert.
- [x] Luna : roundtrip strict, champs/version/UUID/borne/cohérences refusés, source de credential inexistante non consultée, diagnostics opaques et canaris historiques ciblés. Six fonctions finales et douze canaris PASS, sans additionner les rejeux ; les canaris ne sont pas répétés après le seul delta d'opacité.
- [x] Revue indépendante : BIR-01 sur désérialisation publique des snapshots reproduit puis fermé ; oracles de bornes affinés. Fmt/Clippy strict et unique release PASS, [preuves](evidence/2026-10-30/broker-intent/README.md).
- [x] Root : preuves, limites et lot relu préparés pour commit/push ; intégration suivie par Git. Le [complément clock Linux ARM64](evidence/2026-10-30/broker-clock-linux/README.md) concerne le gel précédent et conserve sa portée distincte.

Prochaine priorité : journal privé réellement durable, avec bindings des handoffs manuel/fournisseur et rejet de toute acceptation incomplète ; puis IPC/service/ordonnanceur/gardien. Aucun ACK accepté, effet runtime, migration ou réactivation n'est délivré par le seul format d'intention.

### Lot validé pour intégration — journal privé durable et bindings

Base `3372150fdb99d8bc71700a33dd1d49c0e75c45b5`. Le [contrat du journal](BROKER_JOURNAL.md) précise la persistance requise avant ACK : scope, Submit exact, intention immuable, checkpoint typé et bindings des credentials retenus. Ce composant ne choisit pas l'identité authentifiée et n'accorde aucun lancement ; IPC, ordonnanceur, gardien et qualification du crash restent les étapes obligatoires suivantes.

Propriétaires : core pour `src/broker/journal.rs`, ses types, la couture checkpoint dans `src/runner.rs` et l'extraction pure des parseurs existants ; agent filesystem pour `src/control_fs/persistent.rs` ; Luna pour `tests/broker_journal.rs`, les hooks `cfg(test)` et la preuve SQLite séparée `tests/journal_sqlite.rs`. Root conserve contrat, dépendance, preuves et intégration ; revue indépendante sur sources gelées.

- [x] Implémenter création exclusive, fsync fichier/parents, verrou conservé, marqueur accepté final, révisions immuables et CAS du pointeur ; toute erreur conserve un état relisible ou explicitement interrompu.
- [x] Vérifier None/manuel/fournisseur, rejeu sans relecture de source, rotation/conflit, secrets littéraux décodés, plafonds logiques et diagnostics opaques.
- [x] Reproduire puis corriger le rejet d'un champ connu nullable dans le nouveau constructeur de checkpoint ; conserver les lecteurs historiques et refuser champs inconnus et JSON surnuméraire.
- [x] Exercer les vraies opérations filesystem sous erreurs injectées et neuf interruptions SIGKILL ; distinguer hooks du harnais, tests produit et intervalle rename/fsync non testé.
- [x] Fermer les écarts de versions imbriquées trouvés en revue, avec RED/GREEN sur un vrai checkpoint managed valide ; préserver les versions historiques connues.
- [x] Vérifier dans le même processus que Store le refus d'un hardlink SQLite avant ouverture, avec observation des locks par un enfant indépendant ; corriger et conserver les échecs du harnais.
- [x] Reproduire la course de première création du verrou avec deux vrais processus, puis rendre Busy sur AlreadyExists après une unique relecture contrôlée ; aucun retry des refus de sûreté.
- [x] Exécuter les26canaris pertinents des parseurs partagés, fmt/Clippy strict et release ; macOS et Linux ARM64 en VM ont leurs preuves distinctes, sans qualification de worker.
- [x] Traiter la revue finale, publier preuves et limites, préparer le lot cohérent pour commit/push suivi par Git, sans modifier le déploiement QA ni activer le schéma3.

Les plafonds d'admission du journal ne sont pas des quotas physiques bytes/inodes. SIGKILL ne prouve pas la résistance à la panne électrique ; aucun de ces tests ne clôture G09 ou la recette A1/A2/B1.

Suite obligatoire : le complément ci-dessous vérifie contexte managed et plancher mémoire sur rollback, puis le raccord IPC/service/ordonnanceur doit compléter la verticale batch/helpers. Le simple composant de stockage ne ferme pas les critères d'activation et ne justifie pas un cutover du CLI.

### Lot validé pour intégration — contexte produit par le runner et plancher du journal

Base `537f8d2c072139bb4ced0f3b7bd385f15fb1e9dd`. Luna possède les deux nouvelles fonctions dans `tests/broker_journal.rs` ; root possède le correctif éventuel de `src/runner.rs`, les preuves et l'intégration. Un agent Rust cartographie en lecture seule le futur service/IPC et ses coutures réelles ; aucun service n'est activé par ce travail.

Les premiers tests managed/plancher ont passé avec une fixture Store-backed. La cartographie a ensuite identifié une divergence : `worker_spec` place `plan.operation_id` dans le spec et le handle runtime, tandis que le nouveau validateur et sa fixture attendaient `worker.attempt.attempt_id`. Ces UUID ont des rôles distincts. Le défaut a été reproduit puis le consommateur corrigé selon le producteur, sans modifier les exécutions historiques ni accepter indistinctement les deux identités.

- [x] Vérifier initial rev0 obligatoire puis advance/load/reopen avec le checkpoint managed cohérent et les refus tenant/origine/spec/handoff/worker/identité physique ; pas de managed accepté directement en rev0.
- [x] Vérifier rollback du pointeur valide contre le writer vivant et ses clones, puis lecture conservée après drop/reopen sans prétendre à un anti-restore.
- [x] Conserver les sorties brutes et gels de chaque étape,12tests ciblés/fmt/Clippy/release PASS ; correctif et oracles relus indépendamment.
- [x] Mettre à jour les limites, fermer la revue et préparer le lot pour intégration/push suivi par Git ; poursuivre la verticale service/batch/helpers, sans modifier les JSON RunPlan1/2/3.

### Lot P1 validé — bootstrap explicite d'un contrôle de supervision frais

Base `f46a23ecb8d17f098d325c017536e7c8a146ed83`, worktree `broker-service`. Couture nécessaire à la verticale déjà approuvée : le gateway actuel ne crée que le schéma2 et une réouverture du schéma3 reste en maintenance. Ajouter une création exclusivement neuve avec sauvegarde vérifiée et activation dans cette seule origine ; ne pas changer le défaut2 ni réactiver une base existante/restaurée.

Core possède `src/audit.rs`, `src/main.rs` et la couture minimale de références de chemins dans `src/gateway_config.rs` ; Luna possède `tests/supervision_bootstrap.rs`, la correction privée du clone dans `tests/supervision_store.rs` et toutes les exécutions. Root garde `images/control.Dockerfile`, contrat, suivi, preuves et intégration. Factory implémentée : `Store::create_supervised_for_tenant(path, tenant_id, redactor, max_events, backup_path) -> Result<Store>`, via le vrai `StoreStorage::create_new` exclusif, puis la migration3 existante. Initialisation et garde réutilisés sans refonte du VFS. Le backup existant ne peut être écrasé et la cible DB préexistante est refusée avant ouverture SQLite.

Option gateway implémentée : `--initialize-supervised-stores BACKUP_ROOT`, explicitement demandée, utilisable en local ou avec registre A/B. Chaque backup reste privé, dans un fichier distinct dérivé du SHA256 du tenant exact ; ce nom ne constitue aucune identité authentifiée. Une erreur empêche serve ; les nouveaux partiels éventuels restent conservés et les stores existants intacts. Aucun bootstrap multi-base globalement atomique n'est promis.

- [x] Capturer RED de fonctionnalité absente avant code ; ce RED ne constitue pas une vulnérabilité.
- [x] Vérifier création neuve3 active, sauvegarde2 intègre, reopen maintenance et défaut2 inchangé ; barrières d'ancêtres ajoutées en revue.
- [x] Vérifier refus des cibles/backups existants sans écrasement, erreurs d'entrée et préservation des locks SQLite existants avant toute réacquisition possible par le parent.
- [x] Exercer le vrai gateway loopback sur A/B, identités/namespaces distincts, restart sans flag en maintenance, flag réutilisé refusé ; aucun worker ou modèle.
- [x] Vérifications natives ciblées7+39fonctions, fmt/Clippy/release ; fixture clone600 indépendante de l'umask. Complément Docker du contrôle A/B avec parent `/run/secrets` privé, sans qualifier worker ou build multi-stage complet.
- [x] Fermer la revue documentaire et préparer l'intégration du lot testé ; aucun constat bloquant résiduel.
- [ ] Poursuivre checkpoint V2/IPC/ordonnanceur/batch/helpers, puis démontrer l'arrêt autonome sur un vrai moteur après mort du CLI.

Ce lot ne lance aucun worker supervisé et ne corrige pas seul G09. Pas d'installation OS, modification QA, opération cloud ou nouvelle dépendance d'infrastructure.

### Choix complémentaires du mainteneur — 30 octobre

- [x] Enregistrer les trois moteurs Jev : réel, Rust sans modèle avec limites annoncées, et petit modèle local Jev-like Docker/Podman ; choix/configuration explicites, aucun remplacement du confinement.
- [ ] Définir le contrat commun et les limites de chaque moteur, implémenter le moteur Rust et l'adaptateur local sans réduire les règles impératives, puis qualifier un essai réel borné. Pour le local : au plus4B, cible500Mo–1Go de RAM, modèle/quantification/contexte/limites configurables, mémoire réellement mesurée. Modèle d'embeddings suggéré à évaluer, sélection déléguée à l'orchestrateur.
- [x] Conserver la nouvelle clé Jev privée dans le `.env` ignoré0600, sans altérer les deux autres clés ; source dédiée également privée. Endpoint `https://api.typesafe.ai/v1/systemone` et modèle demandé `jev-latest` ensuite confirmés par le mainteneur. Aucun accès NAS supposé.
- [x] Renseigner uniquement les deux paramètres publics Jev dans le `.env` et l'exemple, sans activation réseau implicite ; vérifier les contrats officiels, garder `choice` et le long contexte désactivé.
- [x] Effectuer un premier essai local réel borné : mémoire/protocole PASS, sémantique FAIL pour Qwen2.5-0.5B Q4_K_M ; candidat refusé pour autoriser des actions. Conserver les deux requêtes et le pic réel, aucun appel répété pour chercher un PASS.
- [x] Vérifier le contrat Jev réel sur un unique petit état synthétique sans worker : réponse `jev-1.14.0`, audit réouvert valide, aucun retry/catalogue/projet. Limites sémantiques explicites ; ne clôture pas la qualification générale Jev.
- [x] Enregistrer la restriction tests : futurs essais Claude explicitement Haiku5.5 (`claude-haiku-5-5`), tests ciblés, aucun retry fournisseur automatique après défaut du harnais.

- [x] Inscrire dans AGENTS.md et les contrats les quatre parcours Codex/Claude Code × abonnement/API, avec résolution dynamique, sélection explicite prioritaire et absence de repli silencieux vers une API facturée.
- [x] Créer le `.env` local autorisé en mode0600, ignoré par Git ; vérifier la liste blanche du contexte Docker et l'exclusion `.env` de l'import. Les fichiers de credentials antérieurs sont préservés. Ce fichier n'est pas encore une preuve de chargement dynamique et ne doit jamais être transmis intégralement.
- [x] Implémenter configuration/résolution Rust, sous-ensemble minimal de credentials, bootstrap et binding aux autorisations avec compatibilité historique ; revue PRV-01/02/03 traitée, vérifications logicielles ciblées et complètes archivées.
- [x] Construire la nouvelle image worker bootstrap et contrôler versions/aides/refus sans credential hors réseau. Ce build ne qualifie pas les authentifications.
- [ ] Compléter les diagnostics de disponibilité avant lancement : status décrit la configuration ; probes clients seulement dans le worker après admission. Qualifier ses vrais lancements et réglages adversariaux.
- [x] Exercer un premier parcours géré Claude/API batch réel, modèle demandé `claude-haiku-5-5`, fichier/export exacts, audit après restart et quota libéré ; preuve limitée à ce binaire/image/compte. Préflights échoués sans modèle conservés.
- [ ] Qualifier les quatre parcours gérés complets, dont conversations interactives. Codex et Claude API batch manuels gardent leurs gels anciens ; le nouveau PASS Claude/API géré ne qualifie ni Codex géré, ni abonnements, ni TUI, ni paramètres adverses. Les abonnements requièrent une connexion officielle autorisée, sans extraction implicite du cache hôte.
- [x] Inscrire le dashboard cloud unique par client sur l'ensemble de ses brokers/hôtes/runtimes, sans dashboard par worker.
- [ ] Livrer cette agrégation avec identité distante, fraîcheur/états partiels, quotas et comptage globaux du tenant ; recette A/B et mesure100workers. Le mode partagé actuel ne constitue pas cette preuve.

- [x] Inventorier les fichiers et l'état Git ; constater les contradictions documentation/code.
- [x] Répartir audits indépendants Rust core, runtime/sécurité et frontend.
- [x] Installer la toolchain exacte manquante et lancer les vérifications de référence.
- [x] Achever la référence et conserver les défauts reproductibles.
- [x] Implémenter le premier lot P0 : identité authentifiée, base SQLite liée au tenant et lectures/écritures API isolées, avec compatibilité locale explicite.
- [x] Tester A/B, spoof d'identité, credentials ambigus et réouverture durable ; aucun worker n'obtient de rôle de contrôle.
- [x] Traiter les trois P0 reproduits par revue : schéma/trigger qui efface l'audit, corruption externe avant append, hardlinks multiprocessus/WAL divergents. Conserver les régressions et vérifier absence de mutation d'un schéma incompatible.
- [x] Faire relire indépendamment les changements, traiter les défauts bloquants, puis format/Clippy/tests/build et preuves courantes : 109 PASS, 1 smoke ignoré, re-probes HTTP et deux processus PASS.
- [x] Aligner la gouvernance persistante sur un vrai produit open source complet et soigné, sans réduction en MVP/V2 ; préserver les preuves historiques. Vingt documents relus, 93 liens/234 chemins vérifiés, copies synchronisées.

## Ordre des lots suivants

1. Corriger les autres P0/P1 reproduits par les audits (audit, autorisations, import/export, lancement).
2. Registre durable des workers, quotas/réservations et transitions atomiques avec audit ; intégration broker et réconciliation.
3. Relais Rust authentifié avec grants dirigés/révocation, limites/durée/rétention, ledger d'allocation `billable=false` idempotent.
4. Recette réelle trois workers, reprises/pannes et visibilité dashboard ; qualification séparée Docker/Podman par plateforme.
5. Versionnement API et schéma, migrations admises, sauvegarde/restauration, exploitation et chaîne de livraison.
6. Adaptateur Kubernetes et identité/réseau/stockage cloud, charge 100 workers réellement mesurée et intégrations fournisseurs natives selon leurs canaux démontrés.
7. Revue de release G01–G12, licence/canal privé décidés, matrice complète et aucun blocage obligatoire ouvert.

Chaque lot précise avant édition : fichiers propriétaires, besoin, risques, tests et critère d'acceptation. Des dépendances externes manquantes bloquent leurs preuves, sans interrompre les travaux indépendants.

## Lot P1 : décisions durables et idempotentes — clos au périmètre logiciel

Défaut initial confirmé par inspection : chaque POST decide créait un nouvel événement pour le même request_id ; le contenu n'était pas lié durablement à cette identité. Ce comportement a été reproduit puis corrigé.

Périmètre traité : `src/audit.rs`, `src/api.rs`, types de décision directement concernés et `tests/decision_idempotence.rs`. Aucun changement de moteur de stockage ni assouplissement des trois correctifs P0. Les chemins append et décision partagent les mêmes contrôles de schéma, chaîne, quota, inode et changement externe.

Critères :

- Même identité de demande, même contenu et même phase initiale/approbation : même décision et même audit_seq, sans nouvel événement, y compris concurrence et redémarrage.
- Même identité avec contenu différent : conflit explicite 409, aucune nouvelle autorisation.
- La phase approuvée exige une revue initiale durable et le hash exact ; un hash faux ne peut pas récupérer une autorisation en cache. Un refus initial ne peut être promu.
- Les événements et l'identité d'idempotence sont engagés dans la même transaction IMMEDIATE ; une panne d'audit ou une altération bloque aussi une reprise.
- Une réponse répétée de décision ne constitue pas une nouvelle preuve de démarrage ni un ticket de lancement consommé. La réservation/cycle de vie des workers et la consommation d'exécution durable restent le lot obligatoire suivant.

Tests : répétition, divergence, concurrence de deux connexions, redémarrage, progression review→approved, approbation sans revue/hash faux, refus non promu et panne/corruption d'audit. Revue indépendante et vérifications globales après intégration.

Statut de ce lot décisions : **terminé au périmètre logiciel**, neuf régressions HTTP et cinq probes indépendantes PASS, fmt/Clippy/tests/build intégrés. Le relevé global après ajout des trois helpers de confinement est 121 PASS/2 ignorés, avec une exécution réelle distincte du test de confinement PASS. Aucun ticket d'exécution consommé n'est livré par ce lot.

## Lot P1 : publication locale par relais Rust — clos pour le candidat local

Défaut reproduit : services Docker sains exclusivement sur `internal`, demandes de ports présentes dans HostConfig mais NetworkSettings.Ports vide et accès hôte impossible. Le mainteneur a **autorisé** le relais Rust distinct sans secrets/volumes, afin de conserver gateway/dashboard internes (décision motivée dans DECISIONS.md).

- [x] Implémenter destination fixe, streaming/auth/statuts, filtres hop-by-hop, limites de corps/URI/flux et délais, avec tests HTTP.
- [x] Ajouter CLI deux listeners et arrêt SIGTERM borné avec un client SSE actif ; intégrer les deux Compose sans toucher les réseaux workers.
- [x] Préparer le parcours navigateur réel avec garde réseau testé et sans captures de secrets ; conserver le BLOCKED initial avant publication effective.
- [x] Clôturer les constats de revue : deadline totale upload, progression aval indépendante du polling Body, garde normale jusqu'à EOF/drop aval ; sonde santé des deux destinations.
- [x] Revue finale, fmt/Clippy/suite/build ; candidat contrôle `qa-20261029-8bd72c0e`, ID `sha256:70b34787cc12aa59e6558c1e391329bffa0acc0c6bc83363737a46589a02414e`. Suite intégrée finale : 139 PASS/0 FAIL/2 ignorés ; les deux essais réels sont exécutés séparément.
- [x] Déployer uniquement la pile QA isolée et observer les bindings `127.0.0.1:48787`/`127.0.0.1:48400` ; conserver les observations réseau et leurs limites de preuve.
- [x] Exécuter le smoke réel (1 PASS) et le navigateur (9 contrôles PASS) sur cette pile ; corriger et relire les défauts du harnais, conserver les FAIL antérieurs, audit/volumes/preuves.
- [x] Redémarrer le seul gateway sans job actif ; audit avant/après identique (102 événements/même head), retour healthy puis `/healthz` final PASS sans second redémarrage. Aucune reprise de job/crash/daemon déduite.
- [x] Arrêter le seul dashboard QA : gateway encore 200, sonde combinée en échec ; redémarrer le dashboard et vérifier endpoints/sonde combinée en succès, pile rétablie sans suppression de données.

Preuves : [dossier final](evidence/2026-10-29/local-final-8bd72c0e/README.md) et [validation](VALIDATION.md). Le PASS du confinement est une exécution réelle distincte du smoke ; les deux restent ignorés dans la suite globale.

## Lot P1 : terminal runtime observé — clos au périmètre logiciel

- [x] Reproduire le résultat terminal inventé (ExitCode absent donnant zéro, état encore actif accepté), sans casser les commandes rapides déjà terminées.
- [x] Exiger Running/ExitCode, code non négatif, état terminal adapté au RuntimeKind et dates cohérentes ; conserver les codes métier non nuls.
- [x] Traiter les deux P1 de revue : Podman `stopped` valide et sentinelle ExitCode négative ; RED puis GREEN et probes indépendantes conservés.
- [x] Exécuter un test agrégateur de 36 sous-cas, 20 tests ciblés, Clippy/rustfmt et revue finale ; intégrer la suite globale 139 PASS.

Le harnais utilise des CLI Rust fixtures pour les cas Docker/Podman ; il ne qualifie pas un moteur Podman réel. Au gel logiciel 139, le cleanup n'avait pas encore été déplacé : la conservation jusqu'à l'audit terminal fait l'objet du lot suivant ; la reprise reste à traiter.

## Prochain chantier vertical obligatoire

### P1 autorité terminale/ingestion — clos au périmètre logiciel et Docker QA nommé

Avant le registre, les correctifs reproduits sont traités en périmètres exclusifs. Le [dossier de preuve](evidence/2026-10-29/p1-terminal-authority/README.md) distingue l'intermédiaire 145/3 ignorés et deux essais réels antérieurs, puis le gel logiciel **147 PASS/0 FAIL/3 ignorés**, fmt/Clippy/release, revue finale et rejeu réel ciblé **1 PASS en 1,25 s**. Les résultats intermédiaires ne qualifient pas le code corrigé ensuite ; le nouveau déploiement reste distinct.

- [x] API : refuser les nouvelles ingestions réservées aux décisions/contrôle/workers/allocations centrales ; conserver la compatibilité des traces libres. Revue ciblée et 21 tests pertinents corroborés par la suite intermédiaire. L'identité runner du même tenant était nécessaire au défaut ; aucun assainissement des décisions historiques ni droit de lancement rétroactif.
- [x] Runtime/runner : reproduire le cleanup avant ACK terminal, la perte de résultat après cleanup/audit d'erreur échoués et la confusion terminal inconnu ; conserver les RED/GREEN de leurs harnais Rust/HTTP.
- [x] Effectuer deux essais Docker indépendants avec gateway/SQLite réels et refus HTTP terminal par middleware ; le second vérifie aussi la réouverture DB. Résultats limités au code antérieur aux dernières corrections, aucun test de disque physiquement défaillant.
- [x] Reproduire les six cas de référence create acquittée inutilisable, où le fallback par nom pouvait agir sur un successeur non possédé.
- [x] Geler les corrections sans fallback par nom en ownership inconnu ; quatorze sous-cas dont les six cas RED corrigés, 22 tests ciblés et source gelée conservés.
- [x] Intégrer `error_audit=not_applicable|acknowledged|unacknowledged`, cleanup `UNCONFIRMED`, arrêt des helpers sans ACK et `changed_files` inconnu ; préserver le terminal connu, et séparer `postprocessing_error` du résultat métier.
- [x] Reproduire puis corriger les fuites de secrets connus dans chaîne d'erreur CLI, frontières stdout/stderr et projection JSON ; masquer après décodage JSON avant projection, y compris les clés. Conserver les RED sans valeur canary dans les journaux.
- [x] Recevoir le gel de l'orchestrateur et consigner fmt/Clippy/suite/release : 147 PASS/0 FAIL/3 ignorés, binaire et sources identifiés par SHA-256.
- [x] Consigner le rejeu réel du code final : HTTP terminal 503 par middleware, instance arrêtée conservée, audit valide avant/après réouverture SQLite ; pas une panne physique de disque.
- [x] Faire relire le lot de code gelé et ses preuves finales, traiter les P0/P1 et archiver le verdict ciblé PASS sans remplacer les snapshots antérieurs. Le reviewer ne revendique pas d'exécution personnelle des probes ou moteurs.
- [x] Identifier la nouvelle image `qa-20261029-security-09c7b861`, ID `sha256:b4239720250bd8a0e05c478d5bd289726041ea753dd701fa4780e795ae9150f2` ; trois services QA healthy, smoke 1 PASS en 3,62 s et navigateur 9 contrôles PASS sur ce candidat. Aucun transfert de l'ancienne image ou du service SQLite local de test.
- [x] Conserver audit avant/après déploiement identique (102 événements), puis audit après nouvelles écritures de tests valide (137 événements) ; volumes et preuves conservés. Clôture limitée à Docker Desktop rootful/macOS ARM64, sans recette trois workers ni qualification production.
- [x] Rejouer aussi le confinement du code final : 1 PASS en 3,78 s. Les trois essais réels courants restent séparés des trois ignorés de la suite normale ; conserver et vérifier le manifest complet de 78 fichiers sans promettre la reproductibilité des paquets/images.

Ce relais de publication n'est pas le relais métier inter-workers. Registre, quotas/réservations durables, identité runtime stable/fencing, état inconnu et ledger d'allocation non facturable restent le chantier vertical obligatoire suivant.

### Conceptions pour le prochain lot — non implémentées

Les [archives DESIGN](evidence/2026-10-29/p1-terminal-authority/design-next-not-implemented/README.md) conservent la synthèse core, son détail et le contrat runtime préparatoire. Elles ne livrent aucun nouvel endpoint, table, quota ou ledger ; leurs tests proposés restent NOT_RUN.

Concepts à transformer en lots avec critères propres : admission fraîche liant spécification complète et reçu de décision, sans faire confiance à un ancien allow ; état/quotas/audit dans la même transaction et projections vérifiées ; tentative/autorité/identité runtime stables et start consommé non rejouable ; observations inconnues conservatrices, terminal/allocation/libération atomiques `billable=false` et reprise sans second effet. Migration explicite, restauration sûre et tests de concurrence/pannes réels restent à implémenter et qualifier. Une fondation SQL ou un mock ne suffit pas à annoncer un registre utilisable.

### Cycle actif : registre et parcours du vrai broker

- [x] Implémenter contrats et projections SQLite v2, quotas/CAS, claim irréversible, mesure terminale et migration sauvegardée ; tests de stockage et relecture ciblée.
- [x] Reproduire puis fermer le contournement de schéma par triggers réservés, avec preuves quota et double claim.
- [x] Ajouter les premières API versionnées et leurs tests HTTP A/B/rôles/quota/pagination ; configuration des limites issue du contrôle.
- [x] Ajouter l'adaptateur à autorité liée, image immuable, inspections et callbacks avant start ; tests sur fixtures, aucun PASS moteur transféré.
- [x] Ajouter les vues Workers/Quotas/Allocations ; tests Node/build/navigateur sur fixtures et relecture du correctif de focus.
- [x] Intégrer `run`, checkpoints privés et verrou par tentative, reprises/arrêts/export sur le même worker et terminal durable avant cleanup ; parcours réels ciblés et reprises sur fixtures, limites ci-dessous.
- [x] Exécuter format/Clippy/suite intégrée après adaptation des tests au genesis v2 :209 PASS/0 FAIL/3 ignorés, release PASS ; corriger les défauts de revue.
- [ ] Faire relire les frontières d'intégration et rejouer le nouveau parcours sur Docker réel, y compris panne et réponse perdue, sans double start ni mesure.
- [x] Construire/déployer le candidat QA209, vérifier migration sauvegardée137→138, audit195 valide et dashboard réel fixe/partagé ; préserver image147, images intermédiaires, volumes et preuves.

Le garde-durée après crash du broker et la restauration sûre d'un état de claims restent des obligations explicites. Un état incertain retient son quota jusqu'à preuve suffisante ; il ne clôture pas à lui seul un parcours opérateur de reprise. Les étapes cochées concernent leur portée logicielle nommée, pas la recette A1/A2/B1, Podman/Kubernetes, charge 100 ou production.

### Priorités révisées après réception de l'audit externe

Les 19 candidats de l'instantané indépendant restent formellement à valider. [AUDIT_FOLLOWUP.md](AUDIT_FOLLOWUP.md) porte le rapprochement avec le checkout courant et les preuves applicables. Traiter d'abord les frontières potentiellement P0 : programme moteur choisi via PATH, ancrage configuration/credentials/SQLite et secrets. La supervision durable Rust du broker reste le prochain chantier P1, avec son design préparé ; aucun daemon ni mécanisme de durée autonome n'est encore livré. La qualification complète des reprises réelles et la recette à trois workers restent ouvertes, sans déplacement sous une étiquette de version future.

- [x] Rapprocher les19 candidats du checkout et conserver leurs verdicts originaux sans réécriture.
- [x] Corriger et faire relire collision Jev/contrôle et captures littérales tronquées, y compris bibliothèque, erreurs CLI et marqueurs privés ; gel235, essais Docker ciblés et nouvelle image QA nommée.
- [x] Supprimer la recherche PATH ambiante sur les parcours livrés et lier l'invocation aux reprises : politique2 ancrée par FD, migrations explicites auditables, gel261 et essais Docker Desktop/macOS ARM64 nommés. Revue et limites dans le [dossier de preuve](evidence/2026-10-29/trusted-invocation/README.md).
- [ ] Qualifier séparément les autres couples moteur/plateforme et compléter la liaison opérateur Podman ; traiter les limites d'installation/plugins/fencing sans transférer le PASS Docker.
- [x] Ancrer configuration, checkpoints broker et handoff de credentials, avec owner/modes/ACL, snapshots indépendants de TMPDIR, exclusion physique du contrôle de l'import et compatibilité des claims ; [gel302](evidence/2026-10-30/control-filesystem/README.md), revue et preuves ciblées.
- [x] Appliquer la frontière privée à SQLite/WAL/SHM/backup, supprimer le FD brut qui annulait des locks POSIX, contrôler noms/permissions/ACL et relire la migration sauvegardée ; [gel327 et QA avec591événements conservés](evidence/2026-10-30/sqlite-storage/README.md), Linux dev et deux UID réels compris. Le gel302 antérieur n'incluait pas ce sous-lot.
- [ ] Compléter la qualification de panne et restauration sûre des claims, les autres filesystems/plateformes et scénarios adverses restants ; le PASS du lot SQLite privé ne clôture pas G06.
- [ ] Poursuivre la supervision autonome, le relais et les qualifications obligatoires ; les étapes logicielles cochées ne remplacent pas leurs preuves.

### Lots locaux livrés et parcours CLI suivant — 30 octobre

- [x] Désactiver le journal brut des nouvelles créations moteur, inspecter avant claim/start et conserver les hashes/reprises historiques : [presetv2](evidence/2026-10-30/engine-logs-v2/README.md), tests ciblés Luna, Docker ACK503/capture masquée et nouveau run v2 PASS, revue indépendante traitée. Pas de qualification Podman ou disque déduite.
- [x] Exécuter un vrai Codex0.165.0 avec modèle demandé Luna : refus strict, premier FAIL bwrap explicite, seconde session avec fichier réel/export exact et original intact. Préparer l'image Claude2.1.140 sans transporter l'OAuth hôte ; [preuves et limites](evidence/2026-10-30/provider-cli/README.md).
- [x] Livrer [quick start](../quickstart.md), prérequis, commandes et configuration, relus contre le CLI/code ; pas de promesse TUI dans les recettes batch.
- [x] Livrer le transport générique `run --interactive` : mode lié à décision/admission, preset versionné, filtre/masquage streaming et gardes, JSON/hashes batch conservés. Régressions ciblées, terminal12/12, auth8/8, fmt/Clippy/release et revue traitée ; vrais Docker clavier/resize/exit7 puis doubleCtrlX PASS. [Preuves et FAIL conservés](evidence/2026-10-30/interactive-cli/README.md). Ce lot ne qualifie pas encore les clients fournisseurs ni une release.
- [ ] Qualifier Codex TUI avec une vraie authentification, deux tours modestes et effet/export réels. Claude exige un handoff autorisé dans le worker ; méthode demandée au mainteneur, ne pas extraire le cache ou le keychain hôte. Les versions/aides seules ne clôturent pas ce parcours.
- [ ] Compléter reprise attach sans second start, crash/enforcement autonome, puis les autres exigences ouvertes. Un terminal interactif ne fournit ni supervision autonome après SIGKILL ni autorisation indépendante de chaque commande transitive.

Le mainteneur reporte Daybreak après la nuit de travail ; aucune campagne globale ni planification automatique cette nuit. Les revues ciblées nécessaires aux lots restent exécutées.

### Direction UX retenue pour les chantiers visuels

- [x] Inscrire la référence Nothin’ demandée par le mainteneur dans AGENTS.md et [docs/ui.md](../ui.md), après consultation de sa page publique.
- [x] Décliner le guide détaillé du30oct dans les neuf vues : header clair, navigation sombre fixe, numéros, typographie éditoriale, citron réservé aux actions/sélection et login accessible dès320px. Node27/build/navigateur,45couples de vues/tailles et revue indépendante PASS ; image QA déployée,10contrôles navigateur réels PASS. Les [captures réelles et limites](evidence/2026-10-30/editorial-ui/README.md) distinguent cette réalisation de l'ancienne mention documentaire.
- [x] Adapter les deux visuels OVPT fournis au logo et favicon16/32, conserver ai-guard comme produit et reprendre le sombre en noirs neutres. Corriger aussi Suivant disabled devenu olive.27tests Node24/build/navigateur/revue puis dix contrôles réels PASS ; [lot OVPT livré et captures](evidence/2026-10-30/ovpt-ui/README.md). Backend ancien explicitement conservé pendant la qualification SQLite.
