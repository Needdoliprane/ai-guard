# Stratégie de qualification

Les résultats nouveaux sont dans [VALIDATION.md](VALIDATION.md). Les résultats historiques de [../validation.md](../validation.md) ne qualifient pas ce checkout sur macOS ARM64. Les critères détaillés existants sont dans [../acceptance.md](../acceptance.md) ; leur ancien découpage MVP/V2 ne réduit pas la mission production.

## Identité individuelle, SSO et découverte — à qualifier

Exigences approuvées le 31 octobre, détaillées dans [le cahier produit SSO et traçabilité](../sso.md) et sa [déclinaison technique OIDC/PKCE](SSO_OIDC.md). Les cas ci-dessous sont **NOT_RUN pour ce nouveau périmètre** : ils ne décrivent pas des suites déjà disponibles. Le SSO facultatif à l'installation ne dispense pas le produit de qualifier son connecteur et sa documentation. Les comptes préconfigurés servent à l'environnement explicite de découverte ; les événements d'exécution doivent être réels.

| Cas | Preuve et refus attendus |
|---|---|
| Sans SSO | Installation locale hors ligne, utilisateur nominatif et attribution conservée ; aucune requête obligatoire vers un IdP, aucun contournement de l'authentification |
| Vrai fournisseur | Compose neuf Keycloak épinglé, realm importé automatiquement, clients publics sans secret ; dashboard et CLI utilisent réellement Authorization Code + PKCE S256 avec `user/user` |
| Rôles distincts | `admin/admin` administre Keycloak ; seules les permissions applicatives explicitement accordées ouvrent les actions ai-guard, sans accès implicite au credential `runner` |
| Réponses adverses | Refus PKCE absent/incorrect, state/nonce incohérents, code rejoué, signature/issuer/audience/expiration invalides, redirection non approuvée et confusion entre ID Token et accès API ; refus côté Rust |
| Sessions et incident | Renouvellement, expiration, déconnexion, révocation et rotation de clés ; panne IdP visible, aucun repli silencieux ni nouvelle autorisation ; politique explicite et bornée pour sessions/identités déjà établies |
| Tenants | Identité humaine et tenant issus du contrôle ; même nom/e-mail chez deux issuers ne fusionne pas les comptes, aucune fuite de session, événement, commande ou résultat A/B |
| Délégation et causalité | Utilisateur → demande → agent → sous-agent → commande → décision → effet réel ; actor/initiator distincts, droits non élargis, falsification du parent/tenant refusée, sous-agent déclaré clairement distinct d'une identité vérifiée |
| Audit et reprise | Demandes/réponses visibles, arguments, décisions et observations liés après concurrence/crash/restart ; audit obligatoire indisponible bloque l'effet, aucune duplication ou fausse attribution ; export et points de contrôle vérifiés |
| Contenus et secrets | Clé API/jeton SSO/credential contrôle absents des argv, workers non concernés, logs, événements et captures ; accès/rétention bornés, masquage/troncature et perte de couverture visibles |
| Découverte chronométrée | Suivre uniquement la documentation depuis un état neuf jusqu'à l'interface puis à une première exécution auditée ; une clé du fournisseur choisi, aucun paramétrage manuel du realm ; distinguer téléchargement/build et démarrage à chaud, publier étapes/versions/durées sans transformer cinq minutes en promesse |
| Préservation et documentation | Activation explicite et loopback, isolation des volumes/configs de démo, credentials existants préservés ; commandes réellement copiables, arrêt/reprise, démo et raccordement entreprise documentés séparément |

Préférer les tests ciblés de contrats pour les nombreux refus et réserver les parcours réels Keycloak/Web/CLI aux jalons et changements d'authentification concernés. Les essais fournisseur restent bornés selon le budget du mainteneur ; un échec de harnais se corrige hors ligne avant répétition. Le second utilisateur/tenant nécessaire aux tests négatifs est créé par le harnais ; il ne complique pas les deux comptes du parcours de découverte. Un IdP simulé ou des transcripts synthétiques ne remplacent pas la qualification réelle. Les nouvelles automatisations du projet restent en Rust, les tests frontend en TypeScript ; les imports JSON et fichiers Compose sont des configurations.

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

`broker_checkpoint` exerce le format V2 sur de vrais fichiers Journal et le vrai Store SQLite : compatibilité rev0 V1 ancrée au binaire antérieur, CAS/replay historique/plancher partagé, promotion refusée après un ancien managed, admission pending/refusée et graphe des neuf variantes avec Import/ImportVerification/FinalScan/Export. Les faits runtime et horloges sont synthétiques, les reçus Store réels. [Commandes et six fonctions finales](evidence/2026-10-30/broker-checkpoint-v2/README.md). Le graphe est coûteux en debug ; pour ce chantier, cible entière exécutée avec `cargo test --release --locked --test broker_checkpoint` (6PASS,17,59s de test sur macOSARM64), cinq fonctions rapides aussi en debug avec `-- --skip sqlite_store_receipts_cover_all_supervision_operation_variants`. Le filtré ne compte pas PASS et le graphe reste actif par défaut. Les 26canaris pertinents restent distincts, sans compter en plus les helpers ignorés ou les rejeux. Ni IPC/enforcer, ni crash V2 exhaustif/panne électrique, ni autres plateformes ou capacité100 ne sont qualifiés par cette cible.

`broker_submission` vérifie les coutures de préparation/reconstruction du vrai Journal, avec bindings None/manuel/fournisseur, approbation conservée et sources/references legacy retirées au replay. Les assertions portent sur les bytes/SHA du handoff, sa survie après drop des propriétaires initiaux, son altération et les diagnostics opaques. Aucun modèle ou API fournisseur. `runtime_prearmed` exerce le port batch sur des programmes CLI Rust explicitement sélectionnés : garde consommé après autorité, refus/ObserveOnly, reprise possédée, callback en attente face au signal et EOF sans nouvelle invocation. Le signal au processus concerne un child propriétaire du test ; aucun signal au terminal utilisateur. [Preuves, RED/GREEN RPT-01 et limites](evidence/2026-10-31/broker-submission-prearmed/README.md). Le drainage technique10s explique une partie de la durée de la cible ; ces fixtures ne prouvent ni enforcer effectif, ni Docker/Podman, ni échéance conservée après SIGKILL CLI. Aucun lancement de la matrice E2E globale pour ces coutures sans frontend ni déploiement modifié.

Le lot [lifecycle V3 et admission HTTP du 31 octobre](evidence/2026-10-31/broker-lifecycle/README.md) vérifie rejets/replays/partiels/planchers, commandes locales hashées et causales, status/list hors page et compatibilité V1/V2/V3. `broker_lifecycle` (source `a3716314…`, 6/6 release) et le graphe Store `broker_checkpoint` (source `9cb7aed9…`, 6/6 release, 32,23s) couvrent notamment l'approbation en deux commits, plusieurs cancels/grants Export et les faits tardifs. Floor 2/2, client 4/4 sur vrais Axum/SQLite3, journal 12/12 et cinq parents unitaires complètent 35 fonctions uniques ciblées ; contrat API, diagnostics et self-test inclus, deux helpers enfants ignorés non additionnés. BLC-01 à04 ont leurs RED/GREEN ; fmt/Clippy all-targets strict/release PASS. Le test distingue CancelRequested de la transaction Store CancelReserved qui libère la quota centrale avant l'ACK local : une réponse perdue peut laisser un pending local avec quota déjà libérée. Une failure locale ne prouve aucun terminal ; Preparing sans CID et cible jamais démarrée sans terminal admissible peuvent rester NeedsReconciliation/quota tenue. Aucun frontend/moteur/modèle/E2E/cloud exécuté ; service/IPC/enforcer et cutover restent à qualifier. G09 FAIL, 0/12 gates fermés ; QA/schéma2/`billable=false` inchangés.

### Service permanent et reprise locale

Le [checkpoint du service permanent](evidence/2026-10-31/broker-service/README.md) documente 40 fonctions ciblées : IPC et queue réels, vrai child gardien vide gelé/repris, états/identités du service, capacités FD source/export, outbox500, reader STOP, bindings et canaris du journal. Le graphe Store/checkpoint et l'outbox sont exécutés en release ; les quatre capacités workspace sont exécutées en debug. Les premiers échecs de fixtures et la correction d'un manifeste auto-référent restent consignés. Les deux helpers unitaires ignorés ne sont pas comptés en plus des parents. Ces preuves ne couvrent ni bootstrap parent, ni slots moteur non vides, ni service complet authentifié A/B, ni deadlines de workers après panne. Après corrections Clippy mécaniques, seuls IPC/service/outbox ont été rejoués. Aucun fournisseur, frontend ou E2E général pour ce checkpoint ; G09 reste FAIL.

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
cargo test --locked --test runtime_terminal_persistence -- --ignored --nocapture
```

Ajouter `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1` uniquement pour une exception rootful explicitement choisie. Les répertoires de preuve, credentials QA privés, volumes et instances diagnostiques créés par le test terminal restent en place. Les fixtures CLI Rust des tests logiciels d'erreur ne qualifient aucun moteur.

La recette obligatoire à trois workers doit couvrir A1/A2 chez A et B1 chez B : démarrage/arrêt, données séparées, grant A1→A2, refus A1→B1, révocation, quotas, audit persistant, usage idempotent, redémarrage contrôle, arrêt brutal, dashboard et refus d'accès aux secrets/privilèges du contrôle. Ce scénario n'existe pas encore dans le code initial. La charge 100 workers est un essai séparé sur de vrais workers.

Kubernetes exige un adaptateur effectif, un cluster autorisé, un CNI appliquant les politiques et des preuves RBAC/identités/volumes/réseau/reprise. Aucune simulation ne qualifie cette plateforme. Les comptes fournisseurs et services distants exigent autorisation et versions réelles.
