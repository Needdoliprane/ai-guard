# Plan d'industrialisation

> Exécution autonome demandée par le mainteneur, avec agents spécialisés, périmètres exclusifs et revue indépendante. Les exigences sont dans AGENTS.md et ARCHITECTURE_CONTRACT.md ; aucune validation intermédiaire de plan n'est requise par la mission.

Objectif : satisfaire les exigences complètes local-first/cloud et produire des preuves reproductibles, sans assimiler un lot à une qualification du produit. Rust/SQLite et frontend TypeScript existants sont conservés. Les décisions structurelles restent soumises au mainteneur.

Lecture du plan : le [cycle broker permanent](#cycle-actif--broker-permanent-et-premier-batch-réel) reste un chantier applicatif distinct des [exigences d'identité et de configuration](#fonctionnalités-demandées--identité-sso-et-découverte-31-octobre). Les autres lots conservent leurs dates, gels et propriétaires historiques. Leurs chemins temporaires, noms d'agents et commandes exécutées ne sont pas des exigences durables ni des preuves d'un autre checkout. Le contrat produit canonique est [sso.md](../sso.md), les vues sont définies dans [ui.md](../ui.md) et l'avancement constaté dans [CURRENT_STATE.md](CURRENT_STATE.md).

## Point de reprise — raccord permanent du 31 octobre, en cours

- [x] Générateur local PKI en une commande Rust : chaîne neuve, protections de fichiers/programme, refus et TLS réellement testés ; [preuves et limites](evidence/2026-11-01/demo-pki/README.md). Ce lot ne qualifie pas G11 ni le raccord HTTPS des services.
- [ ] Poursuivre indépendamment le profil Docker/reprise STOP et le raccord identité/SSO ; ni la PKI de test ni le profil Keycloak de découverte ne livrent ces parcours complets.

Base publiée `c4c4d2a7560d31768da08d8ad9335f4f08667344`, travail isolé dans `.worktrees/broker-service`. Service/CLI explicite, IPC, writer, outbox V4, capacités de source/export et gardien natif sont raccordés. Les premiers tests ciblés passent ; la nouvelle verticale n'est pas encore qualifiée sur moteur et aucun cutover du CLI n'est autorisé par ces résultats.

- [x] Compilation all-targets d'un gel intermédiaire sans avertissement ; tests IPC/CLI/writer, ownership de la queue et états/identités du service exécutés. Les échecs de compilation de fixtures sont conservés.
- [x] Vrai processus gardien vide : refus pendant SIGSTOP, reprise SIGCONT sur la même epoch, arrêt propre et absence de child résiduel. Le harnais crée sa seule capability privée neuve ; aucune clé réelle ni moteur.
- [x] Corriger en source et faire relire la reprise STOP indépendante des blobs/du chargement global, la causalité Cancel/Export, la santé avant admission et l'exclusion des identités pendant publication de l'input Export. Les tests de concurrence du verrou et les quatre capacités workspace passent ; revue source seule distincte des preuves runtime.
- [x] Tests V4 et de récupération de chaîne, dont sibling corrompu, blob perdu, ancien managed V1 et annulations multiples ; canaris, Clippy et build exécutés. [40 fonctions uniques, gels et limites](evidence/2026-10-31/broker-service/README.md). Le périmètre de validation demeure ciblé et non une qualification de la verticale runtime.
- [x] Qualifier le bootstrap public du gardien vide : vraie API `connect_or_spawn`, racine neuve sans capability préinstallée, état prêt puis arrêt/refus endpoint/libération de lease. [PASS ciblé du lot export](evidence/2026-10-31/export-publication/README.md), distinct du premier child directement lancé par le harnais.
- [ ] Qualifier le service complet avec un vrai contrôle3 local : authentification/rôle/tenant avant FS, perte du client, erreurs d'audit, replays et reprise des faits. Le bootstrap vide ne qualifie aucun worker ni échéance runtime.
- [x] Corriger et vérifier le retry d'une annulation durable sur writer vivant : frontière par commande exacte, anciennes Arms avec ACK tardif incluses, exports ultérieurs exclus, reçu historique et génération/gate conservés ; projection sans blob et port STOP/natif NotReady. [14 fonctions ciblées et revues traitées](evidence/2026-10-31/cancel-retry/README.md), sans preuve d'arrêt moteur.
- [x] Traiter en logiciel le premier Cancel sans blob, le writer strict indisponible et les résultats append inconnus par une capacité STOP durable indépendante du chargement métier. Admission unique et lease/floors/rejets conservés, diagnostic explicite et tests d'authentification avec vrai gateway/Store ; [33 fonctions ciblées, revues et limites](evidence/2026-10-31/stop-only-actor/README.md).
- [ ] Vérifier leur convergence réelle sur moteur, les chemins authentifiés de connexion, l'arrêt/reprise après perte du client/broker et la clôture du cycle du gardien ; ni les fixtures ni le canari Unix de refus vide ne ferment ces exigences. Docker est joignable avec images locales ; une nouvelle session isolée doit servir de preuve, sans réutiliser les ressources historiques.
- [ ] Vérifier les slots non vides, les générations restaurées, la panne/reprise du child et la course de publication du moniteur par des régressions contrôlées ; puis la verticale batch/helpers sur vrai Docker. Préserver le FAIL G09 historique tant que les vrais workers n'ont pas démontré la deadline après perte du CLI/broker.
- [ ] Gardien TTY, fencing/disarm D5, rétention/caps physiques et récupération métier restent obligatoires avant cutover. Le rappel Daybreak Blue attend une version stable et les scénarios réels d'arrêt/reprise de workers.
- [x] Conditionner le succès d'export aux synchronisations fichiers/modes, répertoires en postordre, staging et parent après rename ; conserver la destination lors d'une erreur post-publication ; valider les droits des ancêtres par capacités FD. RED/GREEN et revues traitées, [17 fonctions ciblées](evidence/2026-10-31/export-publication/README.md).
- [ ] Qualifier les pannes de stockage/power loss, vrais autres UID et matrice de plateformes ; harmoniser la limite64 de scan/export avec la collecte/import et borner le travail d'énumération total incluant les entrées exclues. Les seuls syscalls réussis ne clôturent pas ces garanties.

### P1 ajouté le 31 octobre — SSO et documentation d'installation

- [x] Enregistrer le [cahier produit canonique](../sso.md) et sa [déclinaison technique OIDC/PKCE](SSO_OIDC.md), sans annoncer une implémentation. Le suivi détaillé unique est la section [fonctionnalités demandées](#fonctionnalités-demandées--identité-sso-et-découverte-31-octobre) ; elle conserve les critères d'identité, traçabilité, découverte et qualification.

### Recette active — première verticale Docker du broker permanent

**Objectif :** vérifier l'arrêt d'un vrai worker après SIGKILL du broker propriétaire, puis la reprise des mêmes faits. Base logicielle `e7cc0bb`, sans appel modèle. Le lot précédent est un progrès vérifié (code, revue, tests et push) ; cette recette ne réinterprète pas ses preuves comme des effets runtime.

**Architecture :** vrais processus locaux gateway schéma3 neuf, broker et gardien existants ; seul le worker et ses helpers utilisent Docker. Aucun service OS installé, aucune infrastructure distante. Ce contrôle natif de test ne qualifie pas le déploiement conteneurisé du produit. Rust stable pour le harnais, frontend inchangé, aucune dépendance ajoutée.

**Fichiers et propriétaires :** agent runtime pour `tests/broker_runtime.rs` et petits helpers de test nécessaires ; root pour contrats/documentation/intégration ; Luna pour toutes les exécutions ; revue indépendante avant effet puis après résultat. Les corrections applicatives éventuellement révélées recevront un périmètre exclusif avant édition.

- [x] Revalider en lecture seule endpoint, version, image exacte et inventaire préexistant. Capture Luna `/private/tmp/ai-guard-runtime-preflight.9CD3lD` : Docker Desktop29.9.2 Linux ARM64, image locale `sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6`. Les services QA antérieurs sont conservés.
- [x] Écrire une entrée Cargo ignorée par défaut, opt-in explicite pour programme/endpoint/image/rootful ; racine neuve courte privée et canonique, source jetable disjointe, credentials de contrôle neufs, environnement nettoyé et preuves privées. Politique dédiée autorisant uniquement la commande bornée de l'essai, jamais modification de la politique utilisateur. H2 compilé avec `--no-run`, H4 compilé puis exécuté ;155 sources vérifiées pour chaque gel.
- [x] Relire effets, bornes, processus possédés et oracles. Aucun kill par nom, prune, réutilisation de volume, suppression de claim ou affaiblissement d'authentification. La revue H4 fait corriger signal réel du broker, OOM/exit/runtime distinguant sleep20 et projection Mounts explicite. Un échec avant Ready/Running est un FAIL de parcours, pas un test moteur réussi.
- [ ] Exécuter une seule session `/bin/sleep 20`, timeout5, réseau none, image locale épinglée. Exiger vraie identité et admission, Arm d'origine, slot Ready/prepared et CID Running avant de tuer uniquement le processus broker possédé. Distinguer échéance native et sortie naturelle de20s ; consigner l'écart de temps observé sans inventer une borne absolue.
- [ ] Garder gateway/gardien vivants, vérifier arrêt du même CID, redémarrer le même broker et vérifier aucune nouvelle Arm/claim/start/deadline ; contrôler audit intègre, quota et usage idempotent seulement après faits terminaux admissibles. Une absence de lecture ou une réconciliation manquante reste inconnue/FAIL.
- [ ] Préserver la racine et le gardien non vide tant que D5/disarm n'est pas qualifié. Nettoyage uniquement par voie runtime contrôlée sur ressources exactes avec preuve terminale ; aucun effacement pour obtenir un PASS. Vérifier source d'origine inchangée et absence de mutation des ressources QA préexistantes.
- [ ] Reproduire/corriger le premier défaut réel s'il apparaît, conserver les échecs et rejouer seulement ce qui est nécessaire, puis revue, preuves, commit et push. G09 reste FAIL tant que ses critères ne sont pas établis ; A1/A2/B1, Podman/Linux, capacité100, TTY, SSO et autres gates restent obligatoires.

Résultats : **H4 FAIL réel**,47,39s, et **H5 FAIL réel**,46,59s, tous deux avant faute. Worker `StartIssued`, CID `Created`, Arm/claim durables mais aucun Running. Les15 opérations de supervision de chaque demande sont ACKées, échec `worker/unavailable` après le claim. H5 retire le polling RecoveryStatus concurrent du writer, sans résoudre le défaut. Le probe natif H4 constate ensuite `stopping/prepared_once/created` ; la restauration peut elle-même établir prepared_once, donc cette observation ne localise pas le premier refus. Conserver les deux FAIL et leurs ressources. Aucun réarmement ou relèvement du timeout pour faire passer le test.

- [x] Implémenter et relire des diagnostics typés Prepare/garde/runtime, sans sérialiser la chaîne d'erreur arbitraire, sans changer les codes IPC ni déplacer le dernier garde. Raccorder `broker.batch.failure` à l'outbox V4 existante, effet explicitement inconnu.
- [x] Vérifier huit fonctions logicielles uniques, fmt, Clippy strict et release ; conserver les échecs Clippy intermédiaires et les deltas mécaniques. [Preuves H4/H5/D1–D3](evidence/2026-10-31/broker-runtime/README.md).
- [x] Exécuter H6 sur le diagnostic : FAIL d'observation45s pendant ImportVerification, aucun refus produit ni Arm worker établi. Recouper9/9 ACK, helper réellement terminé exit0 et teardown des enfants du test ; corriger seulement l'observation120s/global240s après revue, sans changer les5s du worker. La livraison du diagnostic reste non exercée, pas qualifiée par cet essai.
- [ ] Exécuter une seule tentative réelle instrumentée sur le nouveau binaire ; vérifier la livraison réelle du diagnostic, identifier le point précis de refus puis corriger avec régression. Ni la fraîcheur750ms ni l'expiration5s ne sont une cause démontrée à ce stade.
- [ ] Après démarrage obtenu, reprendre les critères SIGKILL/deadline/reprise ci-dessus, puis cycle terminal/disarm D5 et recette multi-client. SSO/Keycloak/PKCE et documentation découverte/entreprise demeurent un chantier obligatoire distinct, sans statut livré par ces preuves.

### Lot actif après H7 : frontière préclaim et lecture redondante

H7 a atteint WorkerArm ACKrev33 puis FailureObserved34, sans WorkerClaim ni événement typé. Le registre/Ready préclaim ne possède pas encore le diagnostic D3. L'intervalle mtime pendingArm→ACK atteint4,897s ; sans sample natif de l'erreur, cela reste compatible avec une expiration, pas une preuve de sa cause. Ne plus relancer le moteur à l'identique.

Deux tâches internes indépendantes, sans changement d'architecture ni de contrat d'autorité :

- Agent native : `enforcer/client.rs` et `enforcer/diagnostics.rs`, corps Ready commun avec wrapper public gardant les codes, factory de refus Register appelée seulement sur erreur avec métadonnées validées. Root : `executor/helpers.rs`, `executor/journal.rs` et `executor/diagnostics.rs`, contextes d'étapes fermés sans texte arbitraire ; mêmes vérifications et samples, aucun nouveau travail externe sur succès.
- Agent core : `broker/writer.rs`, `broker/journal.rs`, régression outbox existante. Déplacer l'adaptation V3→V4 sous le mutex du préload déjà obligatoire pour éliminer le `Journal.load` complet redondant du writer. Garder tous les contrôles du préload/postload, CAS, barrières, floors, refus de downgrade et règle V4 expected==head ; aucun cache de confiance ni changement à load_locked/persistent_sync.

Critères : conservation byte-à-byte des événements/ACK V4 lors d'une mutation lifecycle, conflit obsolète et downgrade refusés sans mutation, recontrôle des bindings après publication toujours fail-closed ; diagnostics Registration/Ready et étapes génériques sans secret/attribution inventée. Luna exécute les seules régressions pertinentes et fmt/Clippy/release après gel commun ; revue indépendante avant intégration. Une réduction de lectures vérifiée en source ne constitue pas une mesure de performance ni une correction démontrée de G09. La prochaine tentative réelle attend ce lot vérifié.

- [x] Lot logiciel vérifié :16 fonctions uniques PASS, fmt/Clippy strict/release RC0. Constat de revue sur l'observation d'une réservation héritée reproduit RED puis corrigé GREEN ; nouveau port réutilise status_locked comme Journal.load, sans réintroduire un Fullload Accepted. Renommage Exchange pour ne pas présenter une frontière comme un ACK. Revues traitées,158 sources/artefacts vérifiés ; [preuves](evidence/2026-10-31/broker-runtime/README.md).
- [x] Exécuter H8 sur ce gel, observation120s/global240s, Arm worker5s inchangée : FAIL d'observation, sans refus précis disponible. Aucun gain ou correction de startup déduit des tests logiciels. Conserver les racines/gardiens/ressources et ne pas contourner D5. Après Running seulement, reprendre la faute SIGKILL, deadline et reprise idempotente, puis le cycle terminal/disarm.

### H8 : attendre un résultat explicite dans le budget global existant

H8 exécuté sur `af4a868` : FAIL d'observation120s, head33 WorkerArm apparaît vers118,303s après Submit, seulement2,234s avant publication du FAIL. Aucun FailureObserved, diagnostic ou WorkerClaim ; le retour du postload/register n'est pas établi. Ce résultat ne justifie pas de correctif applicatif causal ni de nouvelle deadline.

Conception du seul harnais : retirer la coupure préRunning120s, garder la borne globale240s déjà existante et toutes les limites/oracles du worker5s. Pendant l'attente, lire au plus une fois par seconde le premier événement `broker.batch.failure` via le filtre HTTP existant `events?kind=broker.batch.failure&limit=1&order=asc`. Une erreur réellement rapportée fait échouer immédiatement la recette nominale ; elle ne constitue ni terminal, ni preuve d'absence, ni libération de quota. Capturer uniquement seq positive, UUID canonique et labels fermés validés ; jamais data/cause/commande arbitraire. Un ACK local d'outbox peut encore être pending au moment de ce FAIL. Une expiration globale reste indéterminée, sans succès ni refus produit inventé.

- [x] Root : modifier seulement `tests/broker_runtime.rs` et `tests/support/broker_runtime.rs`, parseur de marqueur fermé et polling HTTP borné, aucune lecture du writer ni modification applicative. Tests purs de page vide, marqueur réel sélectionné, labels/UUID/seq invalides et canari privé exclu.
- [x] Luna : compiler/exécuter ces seuls tests sans moteur, fmt/Clippy ; pas de rebuild applicatif car sources produit identiques à PR4. Revue indépendante du harnais avant effet.
- [x] Une tentative H9 sur ce harnais validé, avec budget global240s et Arm worker5s, sans toucher les cinq racines/gardiens H4–H8. Lire ensuite le diagnostic exact avant correction. Le diagnostic a été obtenu avant le global ; une expiration sans diagnostic aurait demandé un profilage ciblé, pas une conclusion causale.

H9 a atteint un résultat explicite : FAIL codé après232,70s, `readiness_before_claim / before_claim / slot_phase_mismatch`, sans sample local, aucun WorkerClaim/start. Diagnostic SQLite seq17 durable et naissance V4 rev35, ACK local encore pending lors de la coupure du test. Cela établit l'état nonReady du snapshot, pas sa cause ni une expiration. Trois tests purs du marqueur, fmt/Clippy et revue du harnais passent ; aucune réponse API simulée ne qualifie le moteur.

### Suite P1 : données acquittées et preuve de snapshot

Le mainteneur précise ensuite que la borne globale du test n'a pas besoin d'être aussi stricte : le scénario passe à600s pour sa préparation/reprise. L'Arm et les oracles du worker restent5s, et le premier diagnostic réel reste fail-fast. Cette marge de test ne change ni durée/quota du produit ni garantie temps réel. La précédente consigne interne de ne plus ajuster cette borne ne s'oppose pas à cette instruction récente.

- [x] Agent native, `enforcer/client.rs` et `enforcer/diagnostics.rs` : sur les seules erreurs de prédicat, copier phase/prepared/génération et clock du snapshot déjà lu, source fermée `helper_slot`. Valider la clock avant copie ; invalide omise avec flag, manquante inconnue. Champ optionnel boxé, absent des anciens payloads ; native_sample garde son sens local et reste absent avant sa lecture. Aucun nouveau garde/sample/RPC ou changement de code/seuil.
- [x] Agent core, `executor.rs` et `executor/{journal.rs,helpers.rs,journal_tests.rs}` : un Draft privé attaché à son scope/opération/head peut être réutilisé uniquement après Receipt writer OK, pour supprimer le Fullload du caller après Register. C'est une projection de données au head engagé, pas une autorité/fraîcheur. Wrappers RPC existants, fastpath ACK→freshfalse, génération/Ready/clock, pending/ACK durables et EmitOnce restent inchangés. Chaque mutation conserve CAS et Full pré/post du Journal ; conflit recharge et préserve Cancel/outbox ; erreur ou Unknown ne confirme aucune projection. Pas de fusion des autres loads sans preuve de concurrence/freshness.
- [x] Root : fixture `AuthenticatedBatchContext::for_executor_test` seulement `cfg(test)`, avec vraie vérification gateway active/schema3/runner/tenant/namespace avant construction ; aucun port non vérifié en production. Tests utilisent vrai Gateway/Store/Journal/Writer et un gardien IPC vide, jamais un moteur/modèle ; admission registry/FS reste une qualification séparée.
- [x] Luna après gel :20 fonctions ciblées PASS S1. Clippy refuse le seul `.err().expect()` du nouveau test ; S2 le convertit en `expect_err()` sans autre changement, fmt/Clippy strict/release RC0. Aucun test rejoué pour cet idiome.159 fichiers source et deux copies binaires vérifiés ; revue indépendante du Draft et du delta d'assertion. Aucun gain temporel ni correction du démarrage déduit de ces résultats ; [preuves et FAIL conservés](evidence/2026-10-31/broker-runtime/README.md).
- [x] Exécuter H10 sur le nouveau binaire, global600s/Arm worker5s : FAIL d'observation à600,04s, sans Arm worker ni diagnostic public ; ownership29 puis FailureObserved30 conservés. Intervention de nettoyage de cache concomitante et inventaire37→3, sans causalité établie. Ne pas qualifier cet essai comme reproduction isolée, expiration du worker ou preuve de D5 ; aucune relance à l'identique.

### Après H10 : rendre visible tout échec local sans inventer sa cause

H10 expire RC101 après600,04s, sans `broker.batch.failure` visible. La lecture des faits conservés trouve pourtant une FailureObserved locale worker/unavailable rev30, après ownership rev29, avant Created central/Arm/claim. Aucun démarrage qualifié. L'inventaire moteur37→3 est une anomalie distincte à expliquer ; aucune cause de ce refus ne lui est attribuée. Ne pas ajouter un essai à l'identique ni prolonger encore la borne pour ce défaut de visibilité.

**Contrat du lot :** `Execution.failure` doit tenter de publier un événement typé pour tout échec qu'elle observe, via le masquage/outbox/HTTP/ACK existant. Si aucune étape native/runtime/broker n'est connue, `broker_step=failure_observed` signifie seulement qu'un échec a été observé dans la phase indiquée. Texte arbitraire, secrets, cause supposée, effet/terminal/fencing et libération de quota restent exclus. Conserver les anciens labels, le payload v1 et les champs d'événement existants. Une livraison impossible reste signalée opaque ; elle ne devient pas un succès ou une preuve d'absence. La classification spécialisée peut rester partielle ; son rapport public a ce repli explicite.

**Frontières ciblées :** contexte fermé autour des seules opérations déjà présentes du callback ownership : commit, relecture, construction inspection Created et échange Created. Le label d'échange ne prouve ni POST ni ACK. Aucune nouvelle lecture, attente, horloge, barrière ou opération moteur ; aucune permission/start/Arm/claim/durée modifiée. Renommer le type interne PreclaimStep en BrokerStep s'il porte ces étapes plus générales, sans changer les valeurs historiques.

- [x] Agent Core, propriétaire exclusif `src/runner/broker/executor.rs`, `executor/{diagnostics.rs,journal.rs,journal_tests.rs,helpers.rs}` : écrire d'abord une régression sur les vrais Gateway/Store/Journal/Writer et le gardien IPC vide existants : une erreur privée inconnue produit actuellement FailureObserved mais aucun événement public. Attendre le RED Luna, puis implémenter le rapport fermé et les contextes ; réutiliser les fixtures, aucun nouveau service/moteur/mock permanent.
- [x] Root, propriétaire de `tests/broker_runtime.rs` et `tests/support/broker_runtime.rs` : test de marqueur `failure_observed` avant prise en charge, RED attendu sur label non accepté ; puis allowlist des nouveaux labels uniquement, toujours huit champs et aucun corps libre. Les règles de UUID/seq/origine/source, deadline globale600 et worker5 restent identiques.
- [x] Luna seulement : deux RED puis8 fonctions ciblées GREEN, fmt/Clippy strict/build release RC0.159 sources et copies binaires vérifiées ; aucun ancien20tests, moteur, fournisseur ou E2E rejoué. Le complément frontend est vérifié séparément ci-dessous.
- [x] Revue indépendante : source/RED/GREEN et artefacts recoupés, aucun P0/P1 restant identifié dans le delta ; données réelles, cause inconnue, audit/durabilité et ordre des appels conservés. [Preuves et limitations](evidence/2026-10-31/broker-runtime/README.md#h10--échec-local-durable-sans-diagnostic-public). G09 et D5 restent ouverts ; coûts du journal/cycle des gardiens seront mesurés et corrigés séparément, sans suppression opportuniste de ressources.

**Complément de présentation du même parcours :** la revue du front montre que ce kind est conservé mais affiché comme une ligne générique, sa phase/portée restant dans le JSON ; il est absent d'Actions sans command/decision. Corriger uniquement l'affichage avec les composants existants : libellé « Échec signalé », phase/étape fermées et effet non confirmé dans le résumé détaillé. Inclure ce kind dans Actions, sans le transformer en incident de sécurité, état terminal ou compteur. Les valeurs inconnues restent inconnues et aucun texte d'erreur libre n'est repris. Conserver palettes, thèmes, logo, navigation et permissions. `src/main.rs` sert un répertoire statique ; aucune dépendance du build Rust aux sources/dist du frontend n'a été trouvée, donc ces fichiers disjoints n'altèrent pas le gel Rust S4.

- [x] Agent frontend : seuls `front/src/model.ts`, `front/src/main.ts`, `front/tests/model.test.mjs`, sans CSS/config/dépendance. Trois RED puis formateur/détail et inclusion Actions ; inconnus/canari couverts, aucun reclassement automatique sécurité, pas de données fictives dans le produit.
- [x] Luna après gel frontend : modèle11/11 et build statique RC0, dépendances installées hors ligne et lock inchangé, Node25.6.1 consigné. Revue indépendante traitée,24 sources et4 assets dist vérifiés séparément des159 sources Rust. Navigateur/QA visuelle et Node24 NOT_RUN, aucune qualification graphique annoncée.
- [x] Complément final : un RED ciblé reproduit l'étape native connue affichée inconnue ; tables fermées enforcer/runtime et priorité broker connue → enforcer → runtime. Modèle complet12/12, build RC0 et revue indépendante finale ;24 sources et4 assets du gel final vérifiés. Aucun effet confirmé ni nouvelle qualification navigateur/moteur.
- [ ] Prochain P1 : mesurer les opérations du chemin durable sur un journal réel borné et qualifier une fermeture sûre des slots ; aucune nouvelle tentative équivalente ni retrait de gardien sans les preuves nécessaires.

## Cycle actif : établir puis sécuriser la référence

### Mesure active du coût durable — 1er novembre, base b32711d

Complément actif sur base `f6f93b3` : le petit graphe ne reproduit pas H10. Un second diagnostic opt-in doit copier uniquement ses enregistrements JSON autorisés vers une racine privée neuve, sans lock/sync/écriture métier sur l'origine ni ouverture des chemins référencés. Scope et reçus importés sont des données de recherche, jamais une identité ou un permis. La copie et les bytes restent privés, hors Git ; l'origine est relue/revalidée sans prétendre à un snapshot atomique.

- [x] Conception ciblée :31révisions/866540octets, bindings None strict, whitelist/bornes/FD, destination disjointe et conservée ; mesurer deux load+status du vrai Writer puis le vrai ChainVerifier sur bytes en RAM, sans mutation/replay/start.
- [x] Agent Core : second probe uniquement dans `src/broker/journal/tests.rs`, JSON fermé sans path/identité/contenu, compteurs source zéro écriture/sync et segment mémoire zéro FS ; refuse contexte divergent plutôt que copier un secret ou réparer.
- [x] Revue indépendante de l'import de données privées avant première exécution ; Luna exécute une fois la cible exacte sur la QA H10 explicitement sélectionnée, en debug,600s coopératif/caller60s, sans moteur/modèle/navigateur. S7 PASS,71,42s ; deux loads16,53/17,70s et status16,92s, validation RAM13,57s sans IO. Origine sans demande de sync/création/publication,160 sources avant/après et archive vérifiées ; [preuves et limites](evidence/2026-11-01/journal-cost/README.md#s7--mesure-sur-copie-privée-du-graphe-riche).
- [x] Supprimer le doublon canonique local avec contrôles/erreurs conservés, goldens observés avant code, neuf fonctions uniques PASS, fmt/Clippy/release et revue indépendante. Les deux FAIL et le seul ajustement de harnais fonctionnel restent documentés.
- [ ] Identifier et qualifier le coût principal restant et la latence release représentative. La comparaison S7 ne démontre aucun gain ; ne pas poursuivre des optimisations supposées ni supprimer des barrières/cache de confiance pour obtenir un succès.

Factorisation bornée vérifiée : les checkpoints V2/V3/V4 réutilisent les bytes déjà validés, sans cache ni changement de wire, domaines, ordre d'erreur, caps ou contrôles. Luna a observé trois empreintes publiques sur l'ancien code, figées avant modification ; régressions, probe, checks et revue passent. La mémoire reste13,96s contre13,57s sur ces échantillons debug : aucun gain de latence ni cause H10 démontré. [Résultats et limites](evidence/2026-11-01/journal-cost/README.md#résultats-finaux-du-lot).

Premier contrôle après correction : six fonctions PASS, puis canari outbox512 sur `Io`. Reproduction à budgets inchangés : branche timeout du caller, puis résultat backend réussi et send refusé ; deux FAIL conservés. Marqueurs retirés exactement, seul test fonctionnel512 à60s déjà admises par Writer, autres fixtures10s et produit inchangés. Les deux coutures outbox, le canari timeout/ownership, S7 et les checks passent sur le gel final. Latence release, cause H10 et capacité100 restent à qualifier ; aucun nouveau moteur. [Preuves et limites](evidence/2026-11-01/journal-cost/README.md#diagnostic-à-budgets-inchangés).

Besoin : obtenir une mesure du chemin Journal/Writer avant toute optimisation ou répétition équivalente de H10. La lecture source trouve des relectures/validations/synchronisations répétées ; elle ne prouve pas leur coût ni la cause de H10. Les résultats n'autorisent aucun retrait de slot, suppression de barrière ou extension de deadline.

Architecture inchangée : instrumentation **uniquement `cfg(test)`**, vraie copie neuve de journal privé, Writer réel et mêmes fonctions d'IO. Aucun gateway, enforcer ou moteur nouveau. Les opérations d'IO, leur ordre, leurs erreurs, CAS, floors et synchronisations restent conservés. Un collecteur de mesure activé explicitement traverse le thread Writer ; ses compteurs fermés n'enregistrent aucun contenu, path, hash, identité ou credential. Distinguer temps inclusifs imbriqués et durée exacte des appels `File::sync_all`, sans nommer un syscall non observé.

- [x] Diagnostic source : cartographier Full load/status, pré/post de mutation, relecture post-reçu et publication du pointeur ; choisir les fixtures existantes plutôt qu'une simulation du stockage.
- [x] Agent Core, propriétaire exclusif de `src/control_fs.rs`, `src/control_fs/persistent.rs`, nouveau `src/control_fs/cost_probe.rs` et ajout ciblé à `src/broker/journal/tests.rs` : probes compilés seulement pour les tests, jamais activés implicitement ; réutiliser `intent_for`/`open` et un journal réel sans binding secret.
- [x] Ajouter une seule mesure opt-in ignorée par défaut, `broker::journal::tests::measure_durable_journal_cost_without_runtime`, lancée seule : deux lectures Writer au même head, status et mutation lifecycle acquittée aux heads0 et8. Vérifier head/chaîne/reçu et compteurs, sans seuil arbitraire de rapidité ni conclusion sur capacité100. Les temps d'installation du journal restent séparés des opérations mesurées.
- [x] Luna : exécuter la cible exacte une fois après gel et conserver commandes, versions, RC, mesures et manifeste avant/après ; sélectionner les régressions FS pertinentes, fmt/Clippy/build, sans E2E général, modèle ou moteur. Une erreur/timeout conserve les preuves et n'autorise aucun restart automatique du même essai.
- [x] Observation parallèle passive du gardien H10 retenu : attribution PID par FD de verrou et socket exacts, puis deux snapshots scalaires20s, sans argv/contenu/IPC/signal ; absence ou ambiguïté n'est pas zéro coût. Conserver les biais de collecte des enfants et ne pas en déduire un coût par Arm.
- [x] Revue indépendante des probes, chemins inchangés et interprétation traitée ;7 fonctions et fmt/Clippy/release RC0,160 sources et binaires vérifiés. [Mesures, erreurs de harnais et portée](evidence/2026-11-01/journal-cost/README.md). D5 et G09 restent ouverts.
- [ ] Prochain critère runtime : implémenter et qualifier le [profil Docker engagé au parent accepté](evidence/2026-11-01/docker-launch-profile/README.md), puis un témoin vivant de fin normale lié à l'émetteur exact, terminal/ACK et retrait durable. Le contrat du profil est relu ; premier RED attendu constaté dans le checkout frère docker-launch-profile après V1 positif, aucune implémentation disponible. La recherche primaire Docker reste conditionnelle, sans attestation des binaires ni témoin construit ; aucune réécriture des configurations/credentials ou retrait d'anciens slots sur cette base.

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

### Lot P1 validé — checkpoint de supervision distinct V2

Base `3e2cffa86c0d0c4784507e78f289419a9e39890f`, worktree `broker-service` propre après push vérifié. Besoin : le payload runner V1 ne conserve pas les armements/claims/helpers du protocole3. Le futur service doit pouvoir réessayer une requête exacte sans régénérer son identité ou recharger un droit de start. Ce lot livre ce stockage typé ; IPC, ordonnanceur et effets runtime restent le chantier immédiatement suivant, sans clôturer G09.

Choix dans l'architecture autorisée : révisions V2 au sein du même journal privé et de sa publication durable, enveloppant le payload runner V1 et un état de supervision. Conserver tous les bytes/domaines des révisions V1, le rev0 initial V1, les plans1/2/3, bindings, scope, accepted/current et limites persistantes. Un champ JSON libre dans le payload historique cacherait le changement de contrat ; un fichier parallèle non transactionnel désynchroniserait les faits. Aucun des deux n'est retenu.

Core possède `src/broker/checkpoint.rs` nouveau et son sous-module privé `checkpoint/validation.rs`, l'export `src/broker/mod.rs`, `src/broker/journal.rs`, `src/broker/journal/types.rs`, une couture pure limitée de contexte dans `src/runner.rs`, puis le raccord craté au reducer existant dans `src/audit/control.rs` et son réexport `src/audit.rs` autorisés après revue. Luna possède `tests/broker_checkpoint.rs`, les éventuelles fixtures et toutes les exécutions Cargo. Root possède ce contrat, les décisions, preuves et intégration. Les autres sources, tests existants, défaut2, Dockerfiles et QA restent gelés. Revue indépendante avant intégration.

API fixée : `BrokerCheckpointV2::new(CheckpointPayloadV1, SupervisionCheckpointV1)`, `SupervisionCheckpointV1::new(Uuid, Vec<HelperPreparationV1>, Vec<SupervisionOperationV1>)`, `HelperPreparationV1::new(HelperExecutionPolicyV1, HelperPermissionV1, Option<OwnedContainerSnapshot>)` et `DecisionAnswerV1::new(Decision, DecisionReceiptV1)` retournent `JournalResult<Self>`. Versions2/1 implicites, champs wire privés et Debug opaque. Getters étroits `runner()`, `supervision()`, `control_namespace_uuid()`, `helpers()`, `operations()`, plus sérialisation canonique/from_bytes fermées pour l'enveloppe. `Journal::advance_supervised(Uuid, RevisionRefV1, &BrokerCheckpointV2)` retourne le reçu de checkpoint existant ; `AcceptedJobV1::broker_checkpoint_v2()` donne l'option typée, `checkpoint()` garde la vue runner V1.

L'état contient les vrais DTO pour décision, admission, mutation worker, armement avec parent historique/snapshot possédé, claim worker, claim helper, observation helper, cleanup helper et export. Réponse absente signifie inconnue, sans affirmer que le RPC n'a pas été envoyé. Maximum128opérations,32helpers,64réponses inconnues ; admission unique et décision limitée aux phases initiale/approuvée. Aucun booléen de permis, rôle restaurable, erreur HTTP libre ou nouveau `Value`. Helper préparé avec politique déterministe avant create ; `owned` progresse seulement de None à un snapshot immuable. Toute authenticité centrale reste à recouper par le service.

Invariants à appliquer : lecture stricte par version de chaque révision et son domaine de hash, chaîne `V1+ V2*`, jamais V2→V1. Promotion refusée si **une quelconque révision V1 confirmée** contient managed, même si la tête paraît vide ; pas d'adoption implicite d'un ancien worker. Première admission pending possible avec managed=None, sans inventer Worker/attempt. Recouper namespace/scope, intention, demande, launch, binding et les liens requête/réponse disponibles. Dès tout helper préparé, Arm ou owned, exiger le vrai `managed.binding=Some` ; aucune copie d'autorité supplémentaire. Une vue de contexte en références, limitée à la crate et sans IO, peut relier ces champs sans rendre ManagedCheckpoint public. Arm.parent doit correspondre intégralement à un Worker d'une réponse antérieure acquittée : il conserve cette révision historique, pas la dernière révision du worker. Les références causales viennent des opérations précédentes acquittées.

Requêtes et réponses connues sont immuables, listes append-only. Toute nouvelle opération apparaît avec response=None ; elle ne peut passer à Some que si cette même requête figurait dans la révision précédente. Tout nouveau helper apparaît avec owned=None puis acquiert un snapshot dans une révision suivante. Valider ces règles à la première V2 et sur toute la chaîne chargée, pas seulement lors d'un nouvel advance. Une décision initiale et sa phase approuvée restent distinctes même si leur request_hash est identique ; comparer chaque demande historique à l'intention normalisée, sans la forcer égale au current_request approuvé ultérieur. Dans V2, les anciens pending/uncertain/import du runner restent vides. Ces invariants rendent l'ordre de persistance vérifiable ; ils ne prouvent pas que l'appelant a réellement attendu le fsync avant d'émettre un RPC.

Précisions issues de la revue du premier gel : l'admission unique, pending ou acquittée, porte toujours la demande **exactement égale à runner.current_request**, même sans managed. Les anciennes opérations Decision restent normalisées et distinctes ; cette règle ne les force pas à changer de phase. Les payloads WorkerMutation suivent les formes/bornes réelles du Store (motif Unknown, inspection Created, observation Observe, cohérence CleanupResult). Les résultats complets sont dérivés du vrai parent historique et de la transition via le reducer pur existant, aussi pour le StartClaim interne de WorkerClaim, avec vérification séparée de son receipt sous le bon domaine. Couture limitée et cratée autorisée dans `audit/control.rs` et réexport `audit.rs` ; aucun SQL, effet, hash ou contrat historique changé, aucune duplication de machine d'états.

Les révisions worker acquittées sont uniques par worker/attempt/révision parmi Admission, WorkerMutation et WorkerClaim. Plusieurs requêtes pending concurrentes restent représentables ; deux ACK contradictoires issus du même parent ne deviennent pas deux branches valides. Les parents historiques d'Arm restent de simples références et ne créent pas de nouvel ACK. Vérifier aussi le refus des champs inconnus imbriqués derrière les enums internally tagged, par les voies publiques de désérialisation et par le loader. Ces vérifications ne présentent toujours pas le journal comme une autorisation de mutation ou une preuve d'authenticité centrale.

Le raccord au vrai runner conserve son état préparatoire : manifest/archive existent avant RPC d'admission. `managed=Some` avec `worker=None` est donc représentable pour une Admission enregistrée pending ou refusée, aussi bien que `managed=None`. Distinguer la requête demandée de l'admission acceptée : `managed.admission` égale exactement la requête persistée, alors que worker, helpers, owned, Arm et claims nécessitent toujours le vrai ACK accepté. Aucun second fichier d'archive, Worker fictif ou droit d'exécution n'est créé pour contourner cette étape.

CAS/replay : nouveau hash couvre tout V2. Rejeu exact confirmé retourne son reçu original même après des révisions suivantes ; vérifier ce cas avant la transition contre la tête actuelle. Après promotion, `advance` V1 refuse même un ancien replay V1 ; aucune donnée V1 réécrite. Publication exclusive/fsync/pointer/plancher et caps communs, sans doubler le protocole d'écriture. Orphelins conservés, aucune promotion automatique. Une réponse EmitOnce persistée n'est jamais un droit rechargeable ; ce stockage n'autorise ni start, ni terminal, ni libération de quota.

- [x] Capturer un RED minimal sur API absente : compilation ciblée RC101, module checkpoint/advance_supervised/getter absents ; aucun test exécuté et aucune vulnérabilité déduite.
- [x] Implémenter formats fermés, contexte/transition et journal mixte ; traiter BCP-01 à06, avec raccord limité au reducer pur existant et contrôle Owned réinstallé après buffering Serde.
- [x] Vérifier bytes/hash rev0 inchangés, pending/refus d'admission avec managed préparé, vraie chaîne/reopen, CAS/replay historique après V2 ultérieure et plancher via clones ; liens issus du vrai Store SQLite, faits runtime synthétiques explicites.
- [x] Vérifier ancien managed caché dans le préfixe V1, phases, versions/champs inconnus et divergences d'identité/reçus ; six fonctions release PASS et26canaris pertinents, sans matrice complète nouvelle de crash V2 ni moteur.
- [x] Fmt, Clippy strict, release et revue source acceptés ; sources/test115 et artefacts natifs préservés hors target. Benchmarks intermédiaires observés avec limite d'archive du test e0 non reconstruit, qualification finale sur source complète GEL5.
- [x] Fermer la revue documentaire : verdict indépendant CLOSE, aucun constat bloquant résiduel du lot ; préparer son intégration/push après vérifications des manifestes et du delta.
- [ ] Poursuivre IPC privé/ordonnanceur/batch/helpers jusqu'au test réel de mort CLI ; aucune TUI ou portée fournisseur qualifiée par le seul format, aucun test provider/navigateur/E2E général systématique.

### Cycle actif — broker permanent et premier batch réel

Base `7098e204a91182229c1a67bc8017271cb3d20918`, worktree `broker-service` propre après push vérifié. Objectif du cycle : véritable service du même binaire Rust, IPC Unix privé authentifié, Submit idempotent durable, exécution v3 avec tous les helpers et arrêt préarmé après mort du CLI. Le cycle n'est pas clos par ses sous-composants ; aucun cutover du run par défaut avant batch/helpers/TTY qualifiés. Pas d'installation de service OS ou d'action cloud. Le [contrat de supervision](BROKER_SUPERVISION.md) et ses obligations de cycle de vie avant admission restent impératifs.

Suivi mainteneur,31octobre2026 : rappeler le lancement de l'audit **Daybreak Blue** après stabilisation du broker permanent et de ses tests réels d'arrêt/reprise. Le mainteneur préfère encore attendre ce jalon ; aucune campagne globale n'est déclenchée implicitement. Les revues ciblées de chaque lot continuent. Ce rappel est lié au jalon de développement, sans planification horaire créée.

Découpage dans l'architecture déjà approuvée : préparation neuve/reconstruction sans relecture au replay ; transport runtime daemon séparé des signaux/timers CLI ; cycle de vie local durable (annulation/approbation/échecs, y compris préparation sans intention valide) ; writer, enforcer indépendant et IPC/CLI ; enfin raccord batch réel et preuves de panne. Aucun faux Worker, faux receipt ou statut en mémoire seulement ne remplace les faits locaux manquants. Toute extension du journal/checkpoint est explicitement versionnée et compatible avec les formats existants. Les commandes de service ne sont pas publiées comme fonctionnelles avant le raccord réel.

Propriétaires : core possède `src/runner.rs`, nouveau `src/runner/broker.rs` et, après contrat d'exécution, les coutures runner/managed nécessaires. Root possède runtime.rs/runtime/managed.rs, nouveaux modules broker de contrôle/service/IPC/enforcer, main/operations et intégration. Luna possède les nouveaux tests ciblés et toutes les commandes Cargo ; revue indépendante sans modification de code. Aucun chevauchement de fichiers. Le cache Cargo n'appartient qu'à Luna ; gels source avant chaque validation.

#### Première couture — préparation et reconstruction réelles

API fixée dans `runner::broker` : `prepare_broker_submission(options: RunOptions, operation_id: Uuid, control_literals: &[String]) -> Result<PreparedSubmission>`. Objet opaque non Serde avec `intent() -> &ExecutionIntentV1`, `binding_inputs() -> BindingInputs<'_>`, `initial_checkpoint() -> JournalResult<CheckpointPayloadV1>` et `config() -> &Directory`. UUID nil et approve_hash refusés avant IO. Le caller a déjà authentifié/configuré le profil et obtenu ReserveResult::New ; la méthode ne s'authentifie pas elle-même et n'accorde aucun droit. Elle impose l'UUID avant projection, conserve les vrais FD et réutilise les parseurs/choix existants, sans lire une autre clé de contrôle. Wrapper legacy choose_plan inchangé dans ses comportements.

`BrokerRunState::from_job(&AcceptedJobV1) -> JournalResult<Self>` reconstitue purement le RunPlan depuis intention et checkpoint courant, avec getters opération/session/demande/options ; aucun FS, résolution Auto, reset d'approbation ou écriture sessions.json. `credentials_from_job(job, config) -> Result<Option<BrokerCredentials>>` ne lit que le binding durable du job déjà validé et crée le Snapshot minimal existant ; `BrokerCredentials` opaque expose path/names/secrets/validate aux callers de confiance, pas de Serde/Debug de valeurs. Config doit correspondre à l'intention ; aucun accès au chemin source tourné ni aux credentials legacy de secours. Aucun runtime, policy RPC ou modèle dans cette couture.

- [x] Luna capture un RED d'API absente (compilation ciblée RC101,E0432), puis root autorise le code de cette couture. Aucun test exécuté par ce RED préparatoire.
- [x] Core implémente la factorisation étroite, la conservation FD au freeze fournisseur et la reconstruction pure ; gel1 qualifié, pas de Cargo par core.
- [x] Tests réels de fichiers/Journal : UUID imposé, nil/approbation/interactif avant IO refusés, bindingNone/manuel/fournisseur figé, source tournée et références legacy retirées après accept sans relecture, approbation conservée et diagnostics opaques. Cinq fonctions PASS ; handoff bytes/SHA/tamper et durée de vie FD vérifiés, aucune API modèle.

#### Deuxième couture — transport batch préarmé

Root ajoute un port runtime sans Ctrl-C global ni deadline métier reconstruite. `BeforeSpawnGuard: Send` est un garde synchrone consommable `check_before_spawn(self: Box<Self>, target: &str) -> Result<()>`, non Serde/non Clone. Le runtime prépare toute commande/vérification d'autorité avant de le consommer, puis appelle spawn sans await/IO/log intermédiaire. Le raccord futur doit fournir le garde concret relisant l'horloge native et l'état du contrôleur, sans writer, HTTP ou secret fournisseur ; ce garde/enforcer n'est pas encore livré par le port. Ce dernier contrôle n'est pas une atomicité moteur et ne ferme pas D5.

Le callback du nouveau port rend ObserveOnly ou un contrôle préarmé frais avec garde et signal indépendant de drainage du transport. Les captures et le terminal observé sont conservés ; résultat daemon sans booléen timed_out hérité du timer CLI. Expiration observée, stop demandé et terminal sont des faits distincts du contrôleur. Le chemin CLI actuel garde son comportement ; le port batch refuse l'interactif avant création. L'enforcer reste armé après erreur/cancellation/EOF et son retrait exige la preuve terminale/centrale admissible. Le transport est borné après signal indépendant, sans réinitialiser la durée métier.

API du port fixée pour code/tests : `PrearmedStartControl::new(Box<dyn BeforeSpawnGuard>, tokio::sync::watch::Receiver<bool>)` non Clone/non Serde, Debug opaque ; `PrearmedStartDirective::{ObserveOnly, EmitOnce(PrearmedStartControl)}`. Le signal est produit par le contrôleur indépendant, pas par une nouvelle durée locale ; vrai ou canal fermé avant émission interdit spawn. `ManagedRuntime::execute_prearmed_retained(spec, input, stdout_limit, on_created, on_started)` et `start_owned_prearmed_retained(spec, owned, input, stdout_limit, on_ready, on_started)` réutilisent les validations/inspections actuelles ; callback créé/ready retourne la nouvelle directive. Pas d'argument duration métier. `ManagedPrearmedExecution` opaque donne result/created/terminal/handle ; `PrearmedExecutionResult` expose stdout/stderr Capture, transport_exit_code Option<i32>, duration_ms u128, sans timed_out. Le terminal runtime distinct est l'autorité sur le résultat métier. Après un signal indépendant de stop, attente de fin du client bornée10s puis arrêt du seul transport et erreur si le terminal manque ; le port n'invente aucun résultat de timeout et ne libère jamais quota. Les délais techniques existants pour commandes/inspections/pipes restent bornés.

- [x] Figer les signatures du port et le RED avant édition runtime (compilation ciblée RC101,E0432) ; root implémente hors fichiers core. RPT-01 callback/EOF reproduit au gel1 puis corrigé au gel2 en conservant le même futur ; aucune qualification moteur.
- [x] Vérifier refus/dernier garde/ObserveOnly et absence de signal global sur fixtures techniques Rust : quatre fonctions PASS, puis15canaris sélectionnés, fmt/Clippy strict/release PASS et revue logiciel CLOSE. [Preuves et limites](evidence/2026-10-31/broker-submission-prearmed/README.md),118entrées source/artefacts hors target ; échecs du harnais conservés séparément.
- [ ] Qualifier ce transport sur vrai moteur avec le service entier. Aucune fixture ne compte qualification runtime.
- [ ] Définir/implémenter faits locaux durables et canaux privés, contrôle authentifié avant projet/fournisseur/job, writer et cible préarmée hors writer/HTTP ; tests pertinents et revue avant exposition CLI.
- [ ] Raccorder les étapes v3 réelles, observer un worker poursuivant son effet après SIGKILL du client puis arrêté à l'échéance originale, témoin intact, audit/usage idempotents et aucune reprise avec nouveau claim/durée.
- [ ] Compléter panne du broker/writer/HTTP, D5, gardien TTY et recette A1/A2/B1 ; conserver les écarts tant que leurs preuves manquent.

#### Troisième couture — cycle de vie local durable

Base `4890df13f56f2a61d9ee773f9d1dbd7cca929e72`. Le [contrat canonique du journal](BROKER_JOURNAL.md#extension-de-cycle-de-vie--contrat-du-lot-suivant31octobre2026) fixe reject avant intention valide, commandes locales typées et checkpoint V3, approbation sur deux commits après vrai Review, cancel monotone et export postmortem sous nouveau grant, statut/liste bornés et planchers. Les plans1/2/3 et bytes/domaines V1/V2 restent conservés. La V3 réutilise le même journal/CAS, sans fichier parallèle de statut ni nouveau stockage.

Core possède lifecycle.rs/sous-modules, journal.rs/types.rs, coutures checkpoint.rs/validation.rs et export broker/mod.rs ; aucune source runner/runtime/API/audit modifiée. Luna possède broker_lifecycle.rs, journal_lifecycle_floor.rs, fixtures et exécutions ; root contrats/intégration et raccord service indépendant, reviewer lecture seulement. Toujours trois agents d'implémentation au plus et aucun Cargo concurrent.

- [x] Reproduire le défaut existant : après accept confirmé, disparition du seul accepted.json faisait revenir reserve avec Ok(Interrupted) malgré le plancher vivant ; marqueur restauré avant assertion, source/logs RED conservés.
- [x] Capturer l'API manquante par compilation ciblée RC101 avant code ; puis core livre un GEL1 archivé. App compile après corrections test-only et régression du plancher 1/1 GREEN.
- [x] Qualifier rejets/status/list, chaîne V3/replays, vrais Review/approbation, Admission inconnue/cancel et refus d'actions co-nées ; rejets/failures ne sont ni terminal runtime ni libération locale. La transaction Store CancelReserved libère le quota avant son ACK local, pas la commande CancelRequested.
- [x] Traiter les écarts de revue GEL1 : ancien grant Export avant cancel ne permet plus de nouveau start après cancel ; jobs Reserved/Interrupted déjà observés revalidés hors page. RED puis GREEN, frontière historique du dernier cancel dérivée sans wire nouveau. Diagnostic enum public également fermé après RED.
- [x] Revue logicielle et canaris/fmt/Clippy/release au gel final ; [preuves ciblées](evidence/2026-10-31/broker-lifecycle/README.md). Aucun gate global fermé.
- [ ] Raccorder le vrai service et sa cible préarmée. Annulation de Preparing sans CID/absence de terminal admissible reste un écart Store opérateur explicite, pas un faux Cancelled.

#### Raccord HTTP en parallèle — refus d'admission durables

Défaut de raccord constaté dans AuditClient::response : toute réponse non2xx est rejetée avant lecture du DTO, alors que la route v1 de réservation supervisée rend de vrais AdmissionResultV1 avec reçu durable sur403Policy/429Quota. Root possède uniquement src/runner.rs pour la nouvelle méthode typée `AuditClient::admit_supervised(&SupervisedAdmissionV1) -> Future<Result<AdmissionResultV1>>` ; aucune ancienne méthode ni handler n'est remplacé. Luna possède tests/broker_admission_client.rs. Ce client ne persiste rien et ne donne aucun start ; le service doit enregistrer sa requête pending avant émission et publier l'ACK via le journal validé.

Contrat : endpoint déjà existant api/v1/workers/supervised-reservations, credential explicite du même AuditClient sans fallback, client HTTP existant (timeouts15s/connect3s, pas de proxy/redirect). Corps requête borné64KiB et réponse512KiB, parsing DTO fermé/EOF, erreurs opaques ; auth/503/autre réponse sans DTO ne sont jamais un refus métier durable. Statut201 accepté neuf/200 replay accepté ;403 exige refusalPolicy et429refusalQuota, accepted=false,worker=None,reçu sans worker/révision. Vérifier opération/input_hash/audit_seq/hash, cohérence état initial/révision/quota/identités/launch/spec/receipt du Worker accepté. Les validations de scope/contexte/chaîne du journal restent requises avant action.

- [x] RED absent E0599 par compilation ciblée avant code, log conservé ; root produit un premier gel sans Cargo.
- [x] Vérifier sur vrai gateway/Store loopback acceptation/replay et refus Policy/Quota/replay, auth et Worker initial altéré conservés en erreur. BLC-04 reproduit par mutation technique d'un vrai DTO201 puis corrigé ; aucun moteur/modèle ou endpoint inventé. Les bornes sont implémentées et relues, sans campagne exhaustive de mutations HTTP revendiquée.
- [x] Revue indépendante et validation au même gel applicatif que lifecycle, [preuves](evidence/2026-10-31/broker-lifecycle/README.md). L'identité/namespace attendus doivent encore être recoupés par le service avant effet ; le client seul ne fournit ni durabilité ni autorité de start.

#### Plan initial du raccord — base c4c4d2a, 31 octobre, conservé pour historique

État de cette liste au premier RED, avant les raccords et validations décrits dans le point de reprise en tête du document. Les cases ci-dessous ne sont pas le statut courant ; source/IPC/CLI sont maintenant raccordés, les qualifications runtime/TTY/D5 demeurent ouvertes.

Le lot lifecycle est intégré et poussé, sources122 vérifiées après fast-forward. Le [contrat de raccord concret](BROKER_SUPERVISION.md#raccord-en-cours--interfaces-dimplémentation-du-31-octobre) fixe les propriétaires et interfaces du vrai service. Revue de constructibilité du child interne stop-only : direction approuvée conservée, aucun service OS installé ; Ready exige capacité réelle et disarm exige un témoin non ambigu, pas un Ok générique. Les notes privées d'implémentation ne remplacent pas ce contrat.

- [x] Capturer RED comportement du vrai binaire : `broker --help` retourne exit2, sous-commande absente ; aucun service/moteur démarré.
- [x] Capturer RED API locale absente puis premier transport sur vrais sockets Unix ; trois fonctions ciblées passent. La revue demande portabilité du répertoire temporaire et oracles discriminants avant clôture des preuves.
- [ ] Finaliser transport/découverte/leases et thread writer borné, avec tests utiles et revue ; UID seul ne choisit aucun tenant.
- [ ] Raccorder profils de contrôle privés, identité/protocole frais, commandes IPC et CLI réelles, sans paramètre de contrôle libre ni fallback de credential.
- [ ] Raccorder stop child, capacité réservée, dernier garde et reprise aux mêmes cibles/échéances, sans déduire D5 d'un simple retour transport.
- [ ] Raccorder le moteur batch et tous ses helpers, approbation/cancel/export séparés et reprise des pending, puis qualifier la mort CLI sur moteur réel. Aucun cutover par défaut avant batch/helpers/TTY qualifiés.

Luna reste propriétaire des tests/Cargo ciblés ; aucun E2E global, modèle fournisseur ou frontend systématique. Les deux RED sont des preuves de manque actuel, pas une vulnérabilité ou une recette fonctionnelle. G09 reste FAIL et aucun gate global n'est fermé par cette préparation.

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

## Fonctionnalités demandées — identité, SSO et découverte, 31 octobre

Le [contrat canonique](../sso.md) fixe intégralement les exigences ; ce tableau organise leur livraison et leurs critères de preuve. **Tous ces comportements restent à implémenter et qualifier.** Le lot documentaire ne livre aucun d'entre eux et ne remplace pas le chantier de supervision en cours. Le SSO est facultatif à l'installation, mais son support fait partie du produit ; les exemples de parcours ne limitent pas les axes de configuration.

| Identifiant | Fonctionnalité à livrer | Critère d'acceptation |
|---|---|---|
| CFG-01 | Configuration par axes indépendants et profils de parcours non exhaustifs | Topologie, identité, périmètres, client/authentification IA et réseau combinables selon le [contrat](../sso.md#configuration-par-axes-indépendants) ; configuration typée, priorités/provenance visibles, contraintes administratives appliquées et incompatibilités refusées sans repli ; interactions critiques qualifiées sans assimiler une combinaison non testée à une garantie |
| INF-01 | Images construites/distribuées par le client et inférence interne | Dockerfiles et procédures sans image publique obligatoire ; URL, modèle, format compatible OpenAI précis/adaptation Claude, référence de credential et chaîne publique d'AC préparés dans le worker et liés à l'autorisation ; TLS actif, destinations internes approuvées, aucun secret de contrôle ou clé privée d'AC, aucun repli cloud ; vrais parcours sans Internet avec clients épinglés |
| IDN-01 | Utilisateurs individuels en local et via fournisseur OIDC configurable | Identité locale ou `(iss, sub)` vérifiée, appartenances tenant et rôles côté Rust ; compatibilité des anciennes clés sans réattribution fictive |
| SSO-01 | Clients publics Web et CLI, Authorization Code + PKCE S256 | Aucun secret client distribué ; connexion réelle, expiration/renouvellement, déconnexion/révocation, refus d'issuer/audience/signature/nonce et replay |
| SSO-02 | Administration applicative avec mapping de rôles/claims configurable | Rôle externe `admin-ai-guard` pour la découverte, noms et chemins configurables ailleurs ; claims du jeton d'accès validés côté Rust, mapping et périmètre explicites ; vue d'administration autorisée, refus utilisateurs normaux/autres tenants/claims falsifiés ou retirés, aucune promotion en administrateur du realm ou `runner` |
| UI-01 | Vues utilisateur et administrateur pilotées par les permissions effectives | Contrat typé Rust/frontend, navigation et actions adaptées aux droits/configuration, contexte personnel distinct, contrôles serveur sur lecture/contenu/export/mutation ; invalidation après révocation/changement de périmètre, refus aussi par URL directe et SSE |
| UI-02 | Synthèse administrateur unifiée à 360° | [Vue globale](../ui.md#vues-utilisateur-et-administration-unifiée--cible-à-livrer) des ressources, utilisateurs, incidents, décisions, quotas et configurations autorisés sur tous les brokers concernés ; filtres et causalité vers les détails, sources/fraîcheur/couverture partielle explicites, aucun accès inter-client implicite ni faux total de pagination ; parcours réel qualifié avec thèmes et mobile |
| TRC-01 | Identités d'exécution et délégations authentifiées | Parent, initiateur, acteur, mission et droits liés durablement ; pas d'élévation, d'usurpation ou de jeton SSO humain dans un worker |
| TRC-02 | Chaîne demandes/réponses visibles → outils/commandes → décisions → effets | Causalité conservée sous concurrence/reprise, messages masqués/bornés, couverture partielle explicite, audit obligatoire avant effet |
| TRC-03 | Recherche et export de preuves par utilisateur/session/agent/commande | Navigation des parents et résultats selon les droits ; audit indépendant, chaîné et points de contrôle signés, vérification et limites documentées |
| DEM-01 | Premier essai Compose avec une clé API du fournisseur choisi | Prérequis diagnostiqués, interface disponible et première exécution réelle auditée, sans autre clé fournisseur/SSO obligatoire ; temps et étapes manuelles mesurés |
| DEM-02 | Profil Keycloak de découverte prêt à lancer | Version épinglée, realm/clients/PKCE préconfigurés ; quatre comptes : `admin/admin` pour le realm, `admin-ai-guard/admin-ai-guard` avec le rôle `admin-ai-guard`, `user-1/user-1` et `user-2/user-2` sans ce rôle ; loopback et activation explicite, aucune donnée existante remplacée |
| DOC-01 | Documentation découverte et raccordement entreprise | Quick start, configuration, Compose/realm, commandes CLI, diagnostic, arrêt/reprise et migration cohérents avec les comportements réellement testés |

- [ ] Définir les contrats typés, migrations et règles de rattachement/revocation sans changer les frontières de confiance existantes.
- [ ] Implémenter les parcours individuels, le connecteur OIDC et les délégations/audits avec les refus et états incomplets.
- [ ] Livrer le packaging et les configurations de découverte ; aucune commande future n'est présentée comme exécutable avant validation.
- [ ] Exécuter la [matrice ciblée](TEST_STRATEGY.md#identité-individuelle-sso-et-découverte--à-qualifier) avec vrai Keycloak, vrais Web/CLI et trace d'exécution, puis mesurer la cible d'essai rapide. Aucune qualification globale par fixture ou par simple démarrage du fournisseur.
- [ ] Mettre à jour documentation et preuves au même gel, faire relire les changements de sécurité et traiter les écarts des gates G04/G06/G07/G08/G09/G12.

## Ordre de dépendance des lots

Cette séquence décrit les dépendances du chantier global. Les sous-lots déjà vérifiés ci-dessus ne sont pas à refaire systématiquement ; compléter leurs exigences et preuves encore ouvertes, ainsi que les fonctionnalités CFG/INF/IDN/SSO/UI/TRC/DEM/DOC, avant la revue de release.

1. Corriger les autres P0/P1 reproduits par les audits (audit, autorisations, import/export, lancement).
2. Compléter le registre durable, les quotas/réservations et transitions atomiques avec audit ; intégration broker, supervision et réconciliation qualifiées.
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
