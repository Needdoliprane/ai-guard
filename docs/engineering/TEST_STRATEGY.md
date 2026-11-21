# Stratégie de qualification

Les résultats nouveaux sont dans [VALIDATION.md](VALIDATION.md). Les résultats historiques de [../validation.md](../validation.md) ne qualifient pas ce checkout sur macOS ARM64. Les critères détaillés existants sont dans [../acceptance.md](../acceptance.md) ; leur ancien découpage MVP/V2 ne réduit pas la mission production.

## Identité individuelle, SSO et découverte — à qualifier

Exigences approuvées le 31 octobre, détaillées dans [le contrat canonique d'identité et de configuration](../sso.md). Cette matrice fixe les preuves d'acceptation ; ses libellés ne sont pas des noms de suites déjà disponibles. Les cas sont **NOT_RUN pour ce nouveau périmètre**. Le SSO facultatif à l'installation ne dispense pas le produit de qualifier son connecteur et sa documentation. Les comptes préconfigurés servent à l'environnement explicite de découverte ; les événements d'exécution doivent être réels.

| Cas | Preuve et refus attendus |
|---|---|
| Axes de configuration | Couvrir les [parcours non exhaustifs](../sso.md#parcours-à-couvrir) et les interactions critiques entre topologie, identité locale/OIDC, périmètres, abonnement/API/inférence interne et réseau ; vérifier priorités, contraintes administratives, choix résolus/provenance masquée et refus explicites des combinaisons incompatibles ou non qualifiées |
| Sans Internet | Vrais services internes et confiance TLS configurée ; aucune dépendance publique obligatoire, aucun téléchargement, télémétrie ou repli caché ; refuser le profil exigeant abonnement/API/IdP publics sans connectivité ; tester préparation des images/dépendances client et diagnostics d'indisponibilité |
| Inférence interne | Images construites/importées par le client, versions épinglées ; URL, modèle, format exact compatible OpenAI et adaptation Claude qualifiée, credential minimal et chaîne publique d'AC préparés dans le worker ; tests positifs/négatifs de chaîne et nom TLS, DNS, streaming et outils selon les capacités annoncées, destinations internes approuvées et absence de repli cloud ; aucune clé privée d'AC |
| Sans SSO | Installation locale hors ligne, utilisateur nominatif et attribution conservée ; aucune requête obligatoire vers un IdP, aucun contournement de l'authentification |
| Vrai fournisseur | Compose neuf Keycloak épinglé, realm importé automatiquement, clients publics sans secret ; dashboard et CLI utilisent réellement Authorization Code + PKCE S256 avec `user-1/user-1` et `user-2/user-2` |
| Rôles distincts | `admin/admin` administre le realm sans droit applicatif implicite ; `admin-ai-guard/admin-ai-guard` reçoit le rôle `admin-ai-guard` dans les claims vérifiés et la vue d'administration de son périmètre ; les deux utilisateurs normaux en sont refusés côté Rust, sans credential `runner` distribué |
| Claim administrateur | Mapping configurable des chemins de claims et noms externes, sans dépendance au nom de découverte `admin-ai-guard` ; refus de mapping absent/ambigu, rôle absent, falsifié, révoqué, émis par un autre issuer ou pour un autre client ; un rôle valide n'ouvre pas les organisations/tenants non autorisés, un nom de compte n'accorde aucun rôle |
| Vues selon les permissions | Vrais parcours utilisateur/admin avec données attribuées et configuration effective ; navigation, contenus et commandes adaptés ; refus serveur pour URL/API/export/SSE hors droits, séparation des permissions de consultation/contenu/mutation, thèmes clair/sombre et petit écran conservés |
| Vue administrateur à 360° | Agrégation des seules ressources autorisées de plusieurs utilisateurs/brokers ; filtres et liens vers détails cohérents, sources manquantes/périmées visibles, aucune donnée d'un client tiers, aucun total global extrapolé d'une page ni usage inconnu présenté comme nul ; qualification réelle distincte des fixtures |
| Changement de contexte et révocation | Retrait des données, détails ouverts et caches devenus interdits, arrêt/revalidation des abonnements aux flux concernés et refus des requêtes en cours devenues non autorisées ; pas de résidu de l'ancien périmètre après changement de rôle, utilisateur, tenant ou expiration |
| Réponses adverses | Refus PKCE absent/incorrect, state/nonce incohérents, code rejoué, signature/issuer/audience/expiration invalides, redirection non approuvée et confusion entre ID Token et accès API ; refus côté Rust |
| Sessions et incident | Renouvellement, expiration, déconnexion, révocation et rotation de clés ; panne IdP visible, aucun repli silencieux ni nouvelle autorisation ; politique explicite et bornée pour sessions/identités déjà établies |
| Tenants | Identité humaine et tenant issus du contrôle ; même nom/e-mail chez deux issuers ne fusionne pas les comptes, aucune fuite de session, événement, commande ou résultat A/B |
| Délégation et causalité | Utilisateur → demande → agent → sous-agent → commande → décision → effet réel ; actor/initiator distincts, droits non élargis, falsification du parent/tenant refusée, sous-agent déclaré clairement distinct d'une identité vérifiée |
| Audit et reprise | Demandes/réponses visibles, arguments, décisions et observations liés après concurrence/crash/restart ; audit obligatoire indisponible bloque l'effet, aucune duplication ou fausse attribution ; export et points de contrôle vérifiés |
| Contenus et secrets | Aucun jeton SSO humain ni credential de contrôle dans un worker ; clé API limitée au worker/client/mode autorisé par handoff privé ; aucun de ces secrets dans les argv, logs, événements ou captures ; accès/rétention bornés, masquage/troncature et perte de couverture visibles |
| Découverte chronométrée | Suivre uniquement la documentation depuis un état neuf jusqu'à l'interface puis à une première exécution auditée ; une clé du fournisseur choisi, aucun paramétrage manuel du realm ; distinguer téléchargement/build et démarrage à chaud, publier étapes/versions/durées sans transformer cinq minutes en promesse |
| Préservation et documentation | Activation explicite et loopback, isolation des volumes/configs de démo, credentials existants préservés ; commandes réellement copiables, arrêt/reprise, démo et raccordement entreprise documentés séparément |

Préférer les tests ciblés de contrats pour les nombreux refus et réserver les parcours réels Keycloak/Web/CLI aux jalons et changements d'authentification concernés. Les essais fournisseur restent bornés selon le budget du mainteneur ; un échec de harnais se corrige hors ligne avant répétition. Les quatre comptes de découverte servent aussi à vérifier les séparations de rôles et d'accès entre utilisateurs ; des identités supplémentaires restent propres au harnais si nécessaires. Un IdP simulé ou des transcripts synthétiques ne remplacent pas la qualification réelle. Les nouvelles automatisations du projet restent en Rust, les tests frontend en TypeScript ; les imports JSON et fichiers Compose sont des configurations.

La couverture n'est pas limitée à la liste initiale de profils : maintenir les capacités par axe, les incompatibilités et les interactions à risque, avec configuration/versions/preuves pour chaque parcours qualifié. Ne pas relancer tous les croisements ni les appels fournisseurs à chaque changement ; sélectionner les cas selon le risque et conserver `NOT_RUN` pour les combinaisons sans preuve. L'ajout d'un profil ne qualifie pas automatiquement ses variantes.

## Stockage SQL configurable — à qualifier

SQLite local conserve ses preuves existantes par candidat. Les deux profils MySQL sont obligatoires. Le démarrage du gateway avec chacun de ces profils et le parcours applicatif SQL-06 restent `NOT_RUN`. Faire le prévol des deux profils sur un serveur MySQL réel de recette. Une CA autosignée dédiée à la recette suffit. Le client vérifie la chaîne et le nom du serveur dans les deux profils. Les [configurations de recette](../../tests/e2e/mysql/README.md) sont isolées de l’installation normale.

Sur Docker Desktop ARM64, les prévols passent avec MySQL 8.4.13. Le refus du certificat client signé par une autre CA est corroboré par l’alerte entrante `unknown_ca`. SQL-06 passe côté SQLite sur un gateway Rust archivé. Les preuves restent limitées à ces exécutions. [Prévol MySQL](evidence/2026-11-19/functional/native17-and-local-workers.md#prévol-des-deux-profils-mysql-réels) · [Écriture sur SQLite](evidence/2026-11-19/functional/native17-and-local-workers.md#écriture-dans-la-base-sqlite-configurée)

Le test ciblé `mysql_audit_store` du worktree SQL passe sur MySQL réel avec mot de passe et TLS puis avec mot de passe et mTLS. Il exerce l’audit ordinaire Rust et la transaction de décision, dont le rejeu concurrent, les phases de revue et le rollback si l’insertion du reçu échoue. La recette de capacité vérifie les seuils d’admission par événements et par octets sur les deux profils. La compilation de la composition SQL inclut le test de refus d’une sélection ambiguë par le vrai CLI. Les cas applicatifs par le gateway, le CLI et le navigateur restent à qualifier. [Recette d’audit ordinaire](evidence/2026-11-19/functional/native17-and-local-workers.md#droits-décriture-et-préfixe-confirmé-sur-les-deux-profils-mysql) · [Transaction de décision](evidence/2026-11-19/functional/native17-and-local-workers.md#transaction-de-décision-et-reçu-sur-mysql) · [Réserve des décisions](evidence/2026-11-19/functional/native17-and-local-workers.md#réserve-des-décisions-sur-mysql)

| Cas | Preuve et refus attendus |
|---|---|
| SQL-01 · SQLite local | Ouvrir une base locale existante ou neuve avec la configuration historique ou explicite. Vérifier tenant, audit, reprise après redémarrage et régression des migrations SQLite |
| SQL-02 · MySQL distant avec mot de passe | Réussir une connexion TCP authentifiée avec la CA et le nom attendus, sans certificat client. Refuser identifiant ou mot de passe faux, mauvaise CA ou mauvais nom. Vérifier que le gateway conserve le profil choisi et qu’aucun audit métier ne passe en cas d’échec |
| SQL-03 · MySQL distant avec mot de passe et mTLS | Réussir avec mot de passe valide et certificat client valide. Tester séparément le refus d’un mot de passe faux avec le bon certificat, puis l’absence de certificat, un certificat client expiré ou signé par une autre CA avec le bon mot de passe. La vérification serveur reste requise |
| SQL-04 · Contrats communs | Sur chacun des trois profils, rejouer le vrai gateway et les parcours CLI et Chromium de décision, audit et export. Créer deux tenants, vérifier le cloisonnement des bases et des comptes, les quotas concurrents, la transaction avec audit et le rejeu après perte d’accusé post-commit |
| SQL-05 · Panne et restauration | Provoquer une indisponibilité sur chaque stockage, constater le refus avant effet et reprendre avec la configuration d’origine. Restaurer une copie isolée du même backend avec des claims en cours et vérifier chaîne, quotas et reçus sans réémission |
| SQL-06 · Écriture sur la cible préparée | Préparer l’historique synthétique directement dans la cible de chacun des trois profils. Ouvrir une nouvelle instance avec cette seule cible configurée, ajouter un événement et vérifier la séquence suivante, le lien avec l’ancienne tête et la conservation exacte des lignes historiques. Rejouer le même identifiant sans ajout. Vérifier qu’une autre base témoin reste intacte et qu’un schéma, un tenant ou un namespace incompatible bloque les écritures |

Le scénario SQL-06 commence avec une cible prête à ouvrir et des droits d’exécution limités à cette cible. Le transfert éventuel depuis l’ancien stockage est réalisé par l’opérateur avant le démarrage. Le gateway reçoit uniquement le chemin SQLite local ou les accès MySQL de la cible sélectionnée. Vérifier que l’ajout aboutit même lorsque l’ancien stockage est inaccessible au processus. Les tests de mise à niveau du schéma SQLite restent propres à SQL-01.

Les prérequis Store de SQL-04 et SQL-06 ont leurs oracles propres. Sur les deux profils MySQL physiques, contrôler l'émission, la rotation, le renouvellement et la révocation de l'identité workload, la conservation des empreintes historiques, le rejeu du reçu après réouverture et le rollback après écriture intermédiaire. La preuve du Store couvre les sujets worker et broker dans leurs recettes respectives et vérifie que la seconde base ne change pas. [Recettes d'identité](evidence/2026-11-19/functional/native17-and-local-workers.md#émission-dune-identité-worker-sur-mysql)

La lecture administrative du registre des brokers doit retrouver l'ordre et l'état attendus depuis le journal vérifié, garder les autorités historiques réservées et refuser une divergence de projection, y compris au-delà de la première page. Vérifier le refus de l'ajout en présence d'une divergence, l'intégrité de la base témoin et la compatibilité d'une ancienne cible dont le registre est entièrement absent. Les essais SQLite de rendu partagé et les deux profils MySQL ont des verdicts distincts. La vérification du Store ne couvre pas encore l'authentification et l'autorisation du registre sur la route du gateway.

Le 21 novembre, les deux régressions SQLite de rendu et d'intégrité passent. Le sélecteur de lecture administrative et celui d'émission du sujet Broker passent chacun sur MySQL TLS et MySQL mTLS avec identifiant et mot de passe, soit quatre passages physiques avec nettoyage confirmé. [Registre administratif sur cibles SQL préparées](evidence/2026-11-19/functional/native17-and-local-workers.md#registre-administratif-des-brokers-sur-cibles-sql-préparées)

Dans le worktree d’intégration SQL, la sonde de cible préparée lit la même configuration de registre que le gateway. Depuis ce worktree, choisir explicitement un tenant et un compte de recette avec droits de lecture.

```sh
cargo run --locked --offline --features mysql-storage --example mysql_storage_probe -- \
  --registry "$QA_MYSQL_REGISTRY" --tenant "$QA_MYSQL_TENANT"
```

Rejouer sur les deux profils, avec une liaison préparée et la tête attendue. Un schéma installé sans liaison, un tenant différent et un namespace différent doivent être refusés. Comparer l’historique avant et après. Sur le profil mTLS, le même compte doit refuser séparément l’absence de certificat et un mauvais mot de passe. Ce prévol vérifie la lecture et la liaison de la cible. La recette complète de démarrage et d’ajout SQL-06 garde son propre verdict.

Sur Docker Desktop ARM64, la sonde et le registre courants liés au Store MySQL identité archivé passent les deux profils physiques. Chacun restitue la tête attendue avant et après les refus. Les deux essais mTLS négatifs rendent le code serveur 1045. [Preuve par profil](evidence/2026-11-19/functional/native17-and-local-workers.md#sonde-du-registre-sur-mysql-physique)

Le test `identity::prepared_registry_probe_preserves_bound_target` rejoue
ces contrôles avec le binaire de l’exemple construit par Cargo. Pour chaque
profil, préparer deux schémas de recette neufs et fournir leurs fichiers
privés via `AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A` et
`AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B`. Exécuter un profil à la fois.

```sh
cargo build --locked --offline --features mysql-storage --example mysql_storage_probe
AI_GUARD_MYSQL_REGISTRY_QA_BIN="$PWD/target/debug/examples/mysql_storage_probe" \
  cargo test --locked --offline --features mysql-storage --test mysql_audit_store -- \
  --ignored --exact identity::prepared_registry_probe_preserves_bound_target --test-threads=1
```

Pour chaque profil MySQL, jouer SQL-06 via `POST /api/v1/events` avec un runner A et des comptes SQL distincts pour les deux bases préparées. Vérifier le reçu après redémarrage, le refus d’écriture pour le viewer et l’absence d’effet sur B. Répéter les contrôles d’identité avec une liaison locale inactive ou expirée, un ancien digest workload après rotation ou révocation et une liaison déjà vérifiée chez B lorsque B devient indisponible. Une projection manquante ou incohérente sur la base sélectionnée doit bloquer l’opération avant écriture. Comparer les têtes d’audit avant et après chaque refus.

Le gateway SQLite archivé passe aussi l’ajout et le rejeu après redémarrage avec l’ancienne base illisible pendant l’essai. Il rend le même reçu et les empreintes de la base témoin restent identiques. Sur les deux profils MySQL réels, le test Rust du Store prolonge la chaîne préparée, compare ses lignes historiques octet par octet et vérifie le rejeu ainsi que la réservation concurrente du dernier créneau. SQL-06 via le gateway MySQL reste `NOT_RUN`. L’oracle Rust SQLite et les deux tests de garde du registre passent sur le CLI du gel de la sonde MySQL stricte. Le rejeu retire l’ancien chemin, puis vérifie le reçu complet après redémarrage, le refus du viewer et les lignes intactes de la source conservée et de la base B. [Rejeu Rust du gateway SQLite](evidence/2026-11-19/functional/native17-and-local-workers.md#registre-sql-et-redémarrage-du-gateway-sur-le-gel-strict) [SQLite et rejeu](evidence/2026-11-19/functional/native17-and-local-workers.md#écriture-dans-la-base-sqlite-configurée) · [Store MySQL et réservation](evidence/2026-11-19/functional/native17-and-local-workers.md#écriture-sur-cible-mysql-préparée-et-réservation-de-worker)

Sur les deux profils MySQL réels, la lecture de présence humaine du Store passe sur deux bases distinctes. La recette couvre la réouverture, la liaison révoquée, le refus d’une projection ou d’un marqueur invalide et le préfixe du tenant voisin. Les contrôles côté gateway et les projections MySQL workload restent ouverts. [Présence humaine MySQL](evidence/2026-11-19/functional/native17-and-local-workers.md#présence-dune-liaison-humaine-sur-deux-bases-mysql)

Le test SQL ciblé du Store passe aussi sur les deux profils MySQL pour le cycle de vie V2. Il vérifie un seul succès sur une révision concurrente, le rejeu du claim, la rétention du quota en cas d’observation incomplète, l’annulation sur refus d’allocation et la réutilisation du créneau libéré. Les observations du worker sont synthétiques. Le parcours SQL-04 avec moteur, gateway et navigateur reste à qualifier. [Preuves du cycle V2](evidence/2026-11-19/functional/native17-and-local-workers.md#cycle-de-vie-v2-sur-mysql)

La pagination de l’audit passe sur les deux profils MySQL réels. La recette vérifie les filtres, la continuité des reçus et le refus d’une page précoce si une projection obligatoire diverge plus loin dans le journal. [Lecture paginée de l’audit MySQL](evidence/2026-11-19/functional/native17-and-local-workers.md#lecture-paginée-de-laudit-mysql)

Le bootstrap de supervision sur cible vierge passe sur les deux profils. La recette vérifie le refus du compte d’exécution, le rollback des deux genèses, le marqueur durable et le refus des mutations V2. Une réouverture ou une divergence observée laisse le témoin en mémoire absent. La récupération supervisée après panne et l’autorisation de lancement font l’objet de preuves distinctes. [Origine de supervision MySQL](evidence/2026-11-19/functional/native17-and-local-workers.md#bootstrap-de-lorigine-de-supervision-mysql)

La réservation supervisée du Store passe sur les deux profils réels. Le test couvre le reçu et son rejeu après réouverture, le refus d’une nouvelle admission sans témoin, le refus de quota audité et la reprise du même appel après un rollback SQL confirmé. Le parcours SQL-04 via le gateway reste `NOT_RUN`. [Recette de réservation supervisée](evidence/2026-11-19/functional/native17-and-local-workers.md#réservation-supervisée-mysql-et-reprise-après-rollback)

Le reçu distingue le prévol du serveur et le résultat de la recette ai-guard. Il consigne la révision, les versions ou digests, le transport, le backend résolu et l’audit observé, sans secret. Exécuter un serveur MySQL de recette à la fois sur le poste local.

## Vérifications logicielles

Avec Rust `1.99.1`, fixé dans `rust-toolchain.toml` :

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
CARGO_INCREMENTAL=0 cargo test --locked
cargo build --release --locked
```

Avec Node 24 dans `front/` :

```sh
npm ci
npm test
npm run build
npm run test:browser
```

Consigner les versions réellement utilisées. Le navigateur exige Playwright et Chromium. Un changement Rust de frontière de sécurité conserve un test négatif : identité fausse, lecture/écriture d'un autre tenant, panne d'audit, replay divergent ou substitution de chemin selon le périmètre.

Conformément à la préférence du mainteneur, confier l'exécution des tests aux agents Luna et sélectionner les cibles selon le changement ; ne pas relancer toute la matrice E2E à chaque lot. Les gates obligatoires restent à qualifier. Les [preuves de journalisation moteurv2](evidence/2026-10-30/engine-logs-v2/README.md) illustrent cette sélection : contrats/reprises ciblés, un scénario Docker503 et un run normal sur le candidat, sans nouveau navigateur puisque le frontend est inchangé. Le [vrai Codex batch](evidence/2026-10-30/provider-cli/README.md) garde son FAIL fonctionnel initial ; un exit0 fournisseur sans effet attendu n'est pas un PASS.

`api_auth_boundary` couvre l'ordre identité/rôle avant lecture des corps et extraction des paramètres, les deux alias, l'absence de mutation d'audit, la borne/ambiguïté Bearer et un vrai gateway enfant HTTP. Le client TCP annonce un corps de 1024 octets sans l'envoyer et exige un premier statut401 avant expiration du délai ; ce n'est pas un test de drainage ou de keep-alive. Le harnais Rust possède son child jusqu'à la fin (`kill_on_drop`), sans gateway de fond dépendant d'une session d'outil. Les [preuves RED/GREEN et régressions](evidence/2026-10-30/api-auth/README.md) restent distinctes d'une qualification de charge ou de runtime ; aucun moteur/modèle appelé.

`supervision_api` exerce les neuf routes v1 sur le vrai routeur et des bases SQLite privées A/B : guards avant body, protocole tenant/rôle/namespace, admission/armements/helpers/claims/export, liens au parent réel et maintenance après réouverture. Une sonde TCP loopback comptée vérifie le non-appel de Jev pour rejeu exact et admission fraîche inactive, après contrôle positif avec le vrai client ; aucun modèle ni contournement TLS. `broker_clock` vérifie calculs/UUID et échantillons natifs du macOS courant, dont un enfant Rust du même exécutable via pipe privé. Son entrée enfant ignorée est invoquée par le test parent et ne compte pas comme test produit supplémentaire. [Commandes, gels et limites](evidence/2026-10-30/supervision-api-clock/README.md) ; Linux natif, reboot/suspend et consommateurs d'échéances restent à qualifier séparément.

Le [complément clock Linux ARM64](evidence/2026-10-30/broker-clock-linux/README.md) exécute une fois cette même cible sur le noyau de la VM Docker Desktop, depuis l'archive figée du lot, sans réseau ni montage : cinq PASS, helper non recompté, sources recopiées et revue de preuve. Cette preuve de producteur sous Linux en conteneur ne vaut pas qualification d'un hôte Linux natif/x86_64, de Podman, des workers ou de l'ordonnanceur. Aucun besoin de rejouer cette cible sur chaque modification pure du format d'intention.

`broker_intent` est une cible pure sans moteur, modèle ou IO : projection stable du vrai RunPlan, golden indépendant de bytes/hash, choix fournisseur et prompt cohérents, refus de métadonnées invalides et diagnostics opaques. Le test de bornes utilise des JSON valides paddés pour les limites d'entrée et un prompt non-NUL valide pour dépasser uniquement la limite canonique ; il exige les erreurs typées attendues. [Six fonctions finales et canaris ciblés](evidence/2026-10-30/broker-intent/README.md), avec RED/GREEN de l'écart d'opacité trouvé en revue. Un parsing réussi ne prouve aucun fichier de credential présent, aucune autorisation tenant ou durabilité de journal.

Le [journal privé du broker](evidence/2026-10-30/broker-journal/README.md) se vérifie sans moteur ou modèle avec `cargo test --locked --test broker_journal`, `cargo test --locked --lib broker::journal::tests` et `cargo test --locked --test journal_sqlite`. Les hooks sont uniquement cfg(test), après les vraies opérations ; leurs deux helpers enfants ignorés sont explicitement lancés par les tests parents, sans double comptage. Le probe SQLite garde Journal et Store dans le même PID et observe les locks depuis des enfants indépendants. La preuve Linux en VM est séparée de macOS et des runtimes workers. Le [complément contexte/plancher](evidence/2026-10-30/journal-context/README.md) couvre le batch/generic/None avec Store réel et snapshots synthétiques, corrige l'identité du handle selon le vrai producteur, et distingue le refus de rollback vivant de la lecture permise après reopen. Autres modes et vrais runtimes restent à qualifier ; SIGKILL et caps applicatifs ne prouvent ni panne électrique ni quotas physiques.

`supervision_bootstrap` vérifie la factory exclusivement neuve et le vrai gateway A/B : sauvegarde2, origine3 active, reopen inactif, erreur post-copie, racines de credentials séparées physiquement, défaut2 et refus de réutilisation. Le probe SQLite enfant intervient avant et immédiatement après le refus dans le PID propriétaire, sans requête parent intermédiaire ; les gateways sont possédés par RAII. [Sept fonctions et canaris associés](evidence/2026-10-30/supervision-bootstrap/README.md). Un complément Docker Desktop/LinuxARM64 compare le parent `/run/secrets` non préparé et préparé, avec mêmes binaires release/probe, projections readonly et DATA neuf ; il qualifie ce contrôle conteneurisé, pas des workers ou la chaîne multi-stage complète. Aucune matrice fournisseur ni nouveau navigateur pour ce lot.

`control_cli` teste les commandes de migration sur une base v1 synthétique, puis la récupération du journal sur une copie de la sauvegarde. Les gateways écoutent sur des ports loopback éphémères et s’arrêtent avant la migration ou la fin de la recette. Les résultats `verify-audit` sont comparés par nombre et empreinte. Le registre de la copie doit répondre `503 migration_required`. La procédure de restauration des droits de démarrage d’un registre v2 reste à qualifier. [Rejeu et portée](../../tests/e2e/README.md#vérifier-une-sauvegarde-de-migration-sqlite)

`broker_checkpoint` exerce le format V2 sur de vrais fichiers Journal et le vrai Store SQLite : compatibilité rev0 V1 ancrée au binaire antérieur, CAS/replay historique/plancher partagé, promotion refusée après un ancien managed, admission pending/refusée et graphe des neuf variantes avec Import/ImportVerification/FinalScan/Export. Les faits runtime et horloges sont synthétiques, les reçus Store réels. [Commandes et six fonctions finales](evidence/2026-10-30/broker-checkpoint-v2/README.md). Le graphe est coûteux en debug ; pour ce chantier, cible entière exécutée avec `cargo test --release --locked --test broker_checkpoint` (6PASS,17,59s de test sur macOSARM64), cinq fonctions rapides aussi en debug avec `-- --skip sqlite_store_receipts_cover_all_supervision_operation_variants`. Le filtré ne compte pas PASS et le graphe reste actif par défaut. Les 26canaris pertinents restent distincts, sans compter en plus les helpers ignorés ou les rejeux. Ni IPC/enforcer, ni crash V2 exhaustif/panne électrique, ni autres plateformes ou capacité100 ne sont qualifiés par cette cible.

`broker_submission` vérifie les coutures de préparation/reconstruction du vrai Journal, avec bindings None/manuel/fournisseur, approbation conservée et sources/references legacy retirées au replay. Les assertions portent sur les bytes/SHA du handoff, sa survie après drop des propriétaires initiaux, son altération et les diagnostics opaques. Aucun modèle ou API fournisseur. `runtime_prearmed` exerce le port batch sur des programmes CLI Rust explicitement sélectionnés : garde consommé après autorité, refus/ObserveOnly, reprise possédée, callback en attente face au signal et EOF sans nouvelle invocation. Le signal au processus concerne un child propriétaire du test ; aucun signal au terminal utilisateur. [Preuves, RED/GREEN RPT-01 et limites](evidence/2026-10-31/broker-submission-prearmed/README.md). Le drainage technique10s explique une partie de la durée de la cible ; ces fixtures ne prouvent ni enforcer effectif, ni Docker/Podman, ni échéance conservée après SIGKILL CLI. Aucun lancement de la matrice E2E globale pour ces coutures sans frontend ni déploiement modifié.

Le lot [lifecycle V3 et admission HTTP du 31 octobre](evidence/2026-10-31/broker-lifecycle/README.md) vérifie rejets/replays/partiels/planchers, commandes locales hashées et causales, status/list hors page et compatibilité V1/V2/V3. `broker_lifecycle` (source `a3716314…`, 6/6 release) et le graphe Store `broker_checkpoint` (source `9cb7aed9…`, 6/6 release, 32,23s) couvrent notamment l'approbation en deux commits, plusieurs cancels/grants Export et les faits tardifs. Floor 2/2, client 4/4 sur vrais Axum/SQLite3, journal 12/12 et cinq parents unitaires complètent 35 fonctions uniques ciblées ; contrat API, diagnostics et self-test inclus, deux helpers enfants ignorés non additionnés. BLC-01 à04 ont leurs RED/GREEN ; fmt/Clippy all-targets strict/release PASS. Le test distingue CancelRequested de la transaction Store CancelReserved qui libère la quota centrale avant l'ACK local : une réponse perdue peut laisser un pending local avec quota déjà libérée. Une failure locale ne prouve aucun terminal ; Preparing sans CID et cible jamais démarrée sans terminal admissible peuvent rester NeedsReconciliation/quota tenue. Aucun frontend/moteur/modèle/E2E/cloud exécuté ; service/IPC/enforcer et cutover restent à qualifier. G09 FAIL, 0/12 gates fermés ; QA/schéma2/`billable=false` inchangés.

### Invocation et mises à jour du moteur

Les intégrations `runtime_managed`, `runtime_program` et `runner_recovery` couvrent le choix du programme hors PATH ambiant, l'environnement reconstruit, l'ancrage du répertoire de CLI, le refus des ACL non qualifiées et des fichiers non réguliers/surdimensionnés, ainsi que l'adoption explicite d'une ancienne invocation. Les tests de migration doivent garder le même moteur/worker/CID et les claims consommés, réessayer le même événement d'adoption après un refus d'audit et ne créer aucun événement supplémentaire pour une adoption déjà acquise. Le changement de contexte ou d'ID Docker entre diagnostic et effet est testé séparément pour les commandes opérateur.

Les programmes de substitution de ces tests sont des fixtures Rust natives explicitement sélectionnées, avec métadonnées privées ; ils ne qualifient ni Docker ni Podman. Pour les trois essais réels ci-dessous, `AGENTSANDBOX_TEST_RUNTIME_PROGRAM` permet de choisir explicitement le programme absolu de l'installation à qualifier. Son absence utilise les emplacements connus du produit ; le PATH du harness ne constitue pas une approbation. Consigner programme/politique réellement testés et conserver une preuve distincte pour la reprise d'une session antérieure.

## Qualification par runtime

Docker et Podman, Linux x86_64 et macOS ARM64 exigent quatre rapports distincts. Les trois essais batch ci-dessous et les cas moteurs interactifs sont ignorés par défaut ; leur absence d'exécution vaut `NOT_RUN`, jamais `PASS`.

| Cible Cargo | Prérequis et portée |
|---|---|
| `runtime_smoke` | Image worker et pile de contrôle démarrée, configuration et endpoints QA explicites ; exécution/refus, copie et export selon [../operations.md](../operations.md). |
| `runtime_confinement` | Image worker et moteur explicites ; configuration inspectée, effets de confinement ciblés, timeout, copie/export. Ne qualifie pas les contrats multi-client. |
| `runtime_terminal_persistence` | Image worker et moteur explicites ; crée sa configuration privée et un vrai gateway SQLite loopback. Un middleware refuse uniquement l'accusé terminal HTTP : vérifie conservation du conteneur, cohérence de l'observation, absence de faux terminal et intégrité après réouverture. Ce n'est pas une panne disque ni la réconciliation du produit. |
| `runtime_interactive` | Qualification ciblée Docker Desktop/macOS ARM64 avec image probe Rust : clavier/TTY/resize/exit7, doubleCtrlX et octets raw exacts CR/LF/Ctrl-C/UTF-8/flèche. PTY physique privé, driver maintenu jusqu'à vérification, aucune clé ni modèle. Trois cas réels PASS ; CtrlX rejoué une fois après revue avec motif opérateur durable exact, absence de timeout et délai propre <30s. Résultats, limites et FAIL préalables conservés dans le [dossier](evidence/2026-10-30/interactive-cli/README.md). Entrée `interactive_host_driver` interne ignorée, non comptée comme test produit ; `local_pty_host_driver_help_smoke` qualifie seulement le harnais sans moteur. |

Exemple de lancement ciblé (image déjà construite, aucun credential imprimé) :

```sh
AGENTSANDBOX_TEST_RUNTIME=docker \
AGENTSANDBOX_TEST_IMAGE=mon-image-worker:test \
AGENTSANDBOX_TEST_RUNTIME_ENDPOINT=unix:///chemin/vers/le/socket-local-docker.sock \
cargo test --locked --test runtime_terminal_persistence -- --ignored --nocapture
```

Remplacer le chemin du socket par celui du moteur local choisi. Pour Podman, le test accepte aussi la valeur `local` avec `AGENTSANDBOX_TEST_RUNTIME=podman`. Le programme du moteur peut être précisé avec `AGENTSANDBOX_TEST_RUNTIME_PROGRAM`.

Ajouter `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1` uniquement pour une exception rootful explicitement choisie. Les répertoires de preuve, credentials QA privés, volumes et instances diagnostiques créés par le test terminal restent en place. Les fixtures CLI Rust des tests logiciels d'erreur ne qualifient aucun moteur.

La recette locale standard couvre A1/A2 chez A et B1 chez B. Elle vérifie leur exécution simultanée, la poursuite d’A2 et B1 après l’arrêt d’A1, les données séparées, le grant A1→A2, le refus A1→B1, la révocation, les quotas, l’audit persistant, l’usage idempotent, le redémarrage du contrôle, l’arrêt brutal, le dashboard et le refus d’accès aux secrets et privilèges du contrôle. Trois à cinq vrais workers suffisent sur le poste du mainteneur. La mesure à 100 workers actifs relève d’un essai séparé sur une infrastructure dimensionnée. [Recette locale](LOCAL_CLOUD_TESTS.md)

Kubernetes exige un adaptateur effectif, un cluster autorisé, un CNI appliquant les politiques et des preuves RBAC/identités/volumes/réseau/reprise. Aucune simulation ne qualifie cette plateforme. Les comptes fournisseurs et services distants exigent autorisation et versions réelles.

### Parcours utilisateur avec les fournisseurs réels

Le [guide de rejeu E2E](../../tests/e2e/README.md) fournit une pile isolée, les commandes de lancement et la relecture navigateur d’une session existante.

Une case est identifiée par le candidat, l’hôte, le moteur et son mode rootless, l’image et la version du CLI fournisseur, le client, l’authentification et le transport. Les deux clients demandent respectivement `gpt-6-luna` et `claude-haiku-5-5`. Relever le modèle effectivement servi seulement si une preuve le permet. Les quatre combinaisons client et authentification sont obligatoires pour chaque moteur pris en charge. Exécuter le batch et le TTY à deux tours, puis les refus et états dégradés sur les candidats concernés.

| Profil moteur | Codex API | Codex abonnement | Claude API | Claude abonnement |
|---|---|---|---|---|
| Docker local | Batch + TTY | Batch + TTY | Batch + TTY | Batch + TTY |
| Podman local | Batch + TTY | Batch + TTY | Batch + TTY | Batch + TTY |
| Kubernetes | Batch + TTY | Batch + TTY | Batch + TTY | Batch + TTY |

La matrice indique les cas à exécuter. Le [registre de validation](VALIDATION.md) fixe leurs résultats par candidat. Pour Docker et Podman, distinguer Linux x86_64 et macOS ARM64, ainsi que les essais sous exception rootful. Pour Kubernetes, la disponibilité du serveur et les dry-runs ne prouvent pas qu’une session fournisseur démarre et se termine sous le contrôle d’ai-guard.

Le [harnais Python](../../tests/e2e/run_provider_case.py) prend un binaire, une pile QA loopback, une image versionnée et un profil existant. Il lance une seule session avec un délai borné et vérifie l’original, l’export neuf, le quota libéré et l’intégrité du journal. Le batch exige un fichier créé au contenu exact. Le mode TTY exige une configuration QA réservée à la recette, deux réponses consécutives sur un vrai terminal, une fermeture native avec modes restaurés et un export identique à l’original. Le [harnais navigateur](../../front/tests/browser_provider_live.mjs) lit le journal de cette même session avec le viewer, retrouve les traces affichées et vérifie le téléchargement JSONL. Aucun jeton runner n’est transmis au navigateur. Consulter `--help` pour choisir le moteur, le client, l’authentification et le transport. Les entrées disponibles dans le harnais sont Docker et Podman.

Avant l’appel, construire l’image avec les versions exactes des clients et préparer les références privées du profil dans une configuration QA. Les clés API et identités d’abonnement suivent le [contrat fournisseur](PROVIDER_AUTH.md). La recette actuelle utilise un projet synthétique, `observe` et le réseau `bridge`. Elle ne qualifie pas le filtrage des destinations. Un échec navigateur ou export se diagnostique d’abord sur le même UUID durable. Le rejeu de cette lecture n’a pas besoin d’un nouvel appel au modèle.
