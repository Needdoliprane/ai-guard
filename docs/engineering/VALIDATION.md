# Validation courante

Les sections sont chronologiques : conserver les FAIL/BLOCKED de chaque tentative. Le [candidat local antérieur](#candidat-local-final--29-octobre-2026), le [snapshot P1 intermédiaire](#p1-autorité-terminale-et-ingestion--snapshot-intermédiaire), puis le [gel P1 final et ses rejeux](#p1-autorité-terminale-et-ingestion--gel-logiciel-147) sont séparés ; leurs preuves ne qualifient que leurs périmètres nommés, jamais le produit ou la matrice globale.

## Référence initiale — 29 octobre 2026

Hôte constaté : Darwin ARM64. `rustc` initial disponible par chemin absolu : 1.94.1 ; la version demandée par le projet est 1.99.1.

| Contrôle | Statut | Résultat constaté |
|---|---|---|
| Git initial | PASS (inventaire) | Aucun commit sur `main`, fichiers existants non suivis ; aucune suppression ni commit du travail préexistant |
| Contrats et skills attendus | FAIL (présence initiale) | `docs/engineering/`, CLAUDE/rules/skills et plusieurs modules cités absents ; inventaire dans `CURRENT_STATE.md` |
| Rust avec toolchain initiale | BLOCKED | 1.94.1 refusé par MSRV 1.99, cargo-fmt absent ; aucune conclusion sur le code |
| Installation toolchain | PASS | Rust 1.99.1 + rustfmt/Clippy installés via rustup officiel, checksum rustup-init vérifié ; PATH global inchangé |
| `cargo fmt --all -- --check` | PASS | Rust 1.99.1, sortie sans écart |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS | Fin normale, aucune alerte ; log temporaire `/tmp/ai-guard-baseline-clippy.log` |
| `CARGO_INCREMENTAL=0 cargo test --locked` | FAIL | Lib : 30 PASS, 1 FAIL, arrêt avant intégrations ; `workspace::tests::publication_never_replaces_a_destination_created_after_validation`, ouverture source `Not a directory (os error 20)` ; `/tmp/ai-guard-baseline-cargo-test.log` |
| `CARGO_INCREMENTAL=0 cargo test --locked --test '*'` | PASS (tests logiciels) | 54 PASS, 0 FAIL, 1 ignoré ; `/tmp/ai-guard-baseline-integration.log`. Le smoke ignoré reste NOT_RUN |
| Front `npm ci`, `npm test`, `npm run build` | PASS | Node 25.6.1/npm 11.13.0 (référence Node 24 non encore reproduite) ; 17 tests PASS, TypeScript/Vite compilent |
| Front `npm run test:browser` | BLOCKED | Playwright non résolu ; Chromium présent, résolution des prérequis en cours |
| Front `npm audit --audit-level=high` | FAIL | Vite 7.3.1 high et esbuild low signalés ; correction de dépendances d'outillage en cours, aucune exploitation revendiquée |
| Moteurs réels et cloud | NOT_RUN | Inventaire runtime confié à un agent ; aucun PASS de confinement à ce stade |

La source d'installation de rustup est la [documentation officielle](https://rust-lang.github.io/rustup/installation/other.html). Les résultats d'installation ne qualifient pas l'application.

## Premier lot intégré, avant traitement des constats d'audit

| Contrôle | Statut | Preuve et portée |
|---|---|---|
| Régression nouvelle CLI tenant | FAIL avant correction, PASS après | `tests::gateway_accepts_an_explicit_tenant_registry` échoue avant ajout option ; logs `/tmp/ai-guard-tenant-config-red.log` puis `...-green.log` |
| Configuration et HTTP réel A/B | PASS | 5 tests binaire/config + `gateway_tenancy` : deux bases, identités A/B, écritures viewer refusées, données séparées après SIGKILL du processus enfant et redémarrage, intégrité conservée |
| `cargo fmt --all -- --check` | PASS | Lot identité/config + correctifs fixture/smoke |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS | `/tmp/ai-guard-lot1-clippy.log`, sortie normale |
| `CARGO_INCREMENTAL=0 cargo test --locked` | PASS logiciel | **97 réussis, 0 échec, 1 ignoré** ; `/tmp/ai-guard-lot1-test.log`. Le smoke réel n'est pas compté comme réussite |
| `cargo build --release --locked` | PASS | `/tmp/ai-guard-lot1-release.log`, fin normale en 1 min 44 s ; candidat antérieur aux corrections d'audit suivantes |
| Revue identité/audit | FAIL | `/tmp/ai-guard-review-tenancy.md` : trois P0 reproductibles, détaillés ci-dessous ; le succès des suites ne vaut pas clôture du lot |
| Revue correctifs tests workspace/smoke | PASS logiciel | `/tmp/ai-guard-review-runtime.md` : seuls fixture cfg(test) et filtre/assertions du smoke changent ; protections productives intactes |
| Front final Node 24.22.2/npm 11.13.0 | PASS | `npm ci`, `npm audit --json` (0 vulnérabilité signalée), 17/17 tests sans ignoré, build, Playwright/Chrome 156.0.8078.48 ; `/tmp/ai-guard-front-report.md` |
| Revue dépendances front | PASS | `/tmp/ai-guard-review-front.md` : diff minimal, versions/intégrités vérifiées, pas de défaut bloquant trouvé |
| Build worker Docker réel | FAIL | `/tmp/ai-guard-worker-build.log` : RPC `Unavailable ... EOF` après téléchargement commencé ; contexte vide 2 B, tag de qualification neuf |
| Qualification Docker Desktop | BLOCKED | Daemon inaccessible immédiatement après build ; aucune cause déduite ; aucun worker/service de contrôle lancé, aucun volume créé/supprimé |
| Qualification Podman | BLOCKED | Binaire/moteur absent du PATH sondé |
| Qualification Kubernetes | BLOCKED | Adaptateur absent ; kubectl client 1.34.1 seul ne prouve pas de cluster |

Les reproductions de revue sont des probes Rust à données factices, sans modification du dépôt par le reviewer : `/tmp/ai-guard-review-probes.rs` (vrais processus HTTP en loopback) et `/tmp/ai-guard-hardlink-probe.rs` (deux processus SQLite). Elles ont confirmé :

1. Trigger `AFTER INSERT events` supprimant la ligne : HTTP 200 allow avec audit_seq=1, puis intégrité valid=true mais zéro événement.
2. Hash altéré depuis une seconde connexion après démarrage : HTTP 200 allow avant tout GET integrity, puis chaîne invalide à la séquence 1.
3. Deux chemins hardlink, deux processus/tenants et deux WAL : les deux ouvertures acceptent leurs écritures, puis les checkpoints réaffectent le binding et font perdre l'événement A.

Ces constats empêchent de clôturer le lot. Les correctifs imposent schéma admis/versionné, détection des changements externes sous transaction et refus des alias physiques dangereux ; leurs résultats seront consignés séparément après relecture.

## Corrections de revue intégrées et relues

Les trois reproductions précédentes ont été fermées par du code et des régressions ; la portée reste le stockage/API logiciel.

| Contrôle après correction | Statut | Résultat |
|---|---|---|
| Régressions audit avant correction | FAIL reproduit | Six échecs métier initiaux ; hardlinks inter-processus et faux type SQL reproduits séparément avant leurs corrections |
| `audit_integrity` | PASS | 12 tests : trigger startup/live, hash live sans endpoint préalable, startup corrompu, version inconnue sans changement journal_mode/DDL, quota altéré, contraintes de schéma, v1 amputée, connexions légitimes, hardlinks multiprocessus, alias/symlink/substitution |
| Régression HTTP indépendante R1/R2 | PASS | Trigger refuse le startup ; modification du hash après startup produit **503** sur decide, sans GET integrity déclencheur ; [log](evidence/2026-10-29/audit-http-recheck.log) |
| Régression indépendante deux processus R3 | PASS | Ouvertures hardlinks A et B toutes refusées avant SQLite ; [log](evidence/2026-10-29/audit-hardlink-recheck.log) |
| Revue des corrections | PASS (périmètre) | Patch relu et probes indépendantes réexécutées ; aucun nouveau bloquant identifié. Suggestion non bloquante : cardinalité totale du binding à renforcer, sans contournement démontré |
| `cargo fmt --all -- --check` | PASS | Sortie sans écart sur le candidat corrigé |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS | [log](evidence/2026-10-29/clippy.log) |
| `CARGO_INCREMENTAL=0 cargo test --locked` | PASS logiciel | **109 réussis, 0 échec, 1 ignoré** ; [log](evidence/2026-10-29/rust-tests.log). Smoke moteur ignoré toujours NOT_RUN |
| `cargo build --release --locked` | PASS | Recompilation après corrections en 34,10 s ; [log](evidence/2026-10-29/release-build.log) |

Identité du candidat à cet instant, avant lot décisions suivant (Git sans HEAD) : SHA-256 `src/audit.rs` = `5833bfdf23d03d27c2a38629c999658887c650db38bcd708e99eea937f107c7d`, `src/api.rs` = `ba2961056b6e6e955bb9ad8f7ddc413f95f654fe4868bb328ed9690854148e87`, `src/main.rs` = `17957d9165e056d4c625307c1139a092c6255ad954ced1a33ee652bd901ade0d`, `tests/audit_integrity.rs` = `e0a8e8f9d8bd712c06dee3c5d75ded4dfd08a1d911b0553becba32c1dd633524`.

La base est désormais reconnue/versionnée (`user_version=1`), jamais réparée silencieusement ; une corruption de données reste consultable pour diagnostic avec écritures verrouillées. Les chemins de données sont vérifiés par identité physique et liens, les noms parents canonisés avant dérivation des WAL. Ces propriétés ne prétendent pas résister à un administrateur qui contrôle et réécrit intégralement le stockage. La restauration, les pannes disque et les gates runtime/cloud restent ouverts.

## Changement d'état externe Docker

Après le premier blocage RPC EOF/socket absent, le sondage `target/release/agentsandbox doctor --runtime docker --allow-rootful-runtime` a constaté un daemon de nouveau accessible : Docker Desktop **4.97.0 (244584)**, serveur **29.9.2**, client **29.2.1**, `rootless:false` ; [log](evidence/2026-10-29/docker-recheck.log). Aucun redémarrage de Desktop ni changement de configuration n'a été effectué par les agents ; la cause n'est pas déduite. Le blocage précédent reste l'historique de sa tentative, et une nouvelle qualification est en cours sur ces versions différentes. Aucun PASS de confinement n'est transféré d'une version à l'autre.

## Candidat local final — 29 octobre 2026

Les logs et rapports de l'orchestrateur et des reviewers ont été lus puis copiés sans écraser les preuves antérieures dans [local-final-8bd72c0e](evidence/2026-10-29/local-final-8bd72c0e/README.md). La consolidation documentaire ne relance aucun moteur ni test produit. Les comptes ci-dessous proviennent des sorties conservées ; les fichiers sources du candidat gelé concerné ont les mêmes SHA-256 que ses revues finales. Les [empreintes de sources](evidence/2026-10-29/local-final-8bd72c0e/SOURCE_SHA256SUMS.txt) sont un constat au gel, pas une attestation de reconstruction d'image ni une qualification des modifications suivantes.

### Environnement et artefacts

- Hôte **macOS 27.2 ARM64**, Rust **1.99.1**. Docker Desktop **4.97.0 (244584)**, CLI **29.2.1**, Engine **29.9.2** Linux ARM64 dans sa VM, `rootless:false`. L'exception rootful est explicite et auditée ; aucun PASS rootless ou Linux x86_64 n'est déduit.
- Pile QA isolée `ai-guard-qa-7dce8f12`, image contrôle `agentsandbox-control:qa-20261029-8bd72c0e`, ID local **`sha256:70b34787cc12aa59e6558c1e391329bffa0acc0c6bc83363737a46589a02414e`**. Gateway/dashboard restent internes ; ingress publie seulement les bindings hôte `127.0.0.1:48787` et `127.0.0.1:48400` du parcours nommé. Les [sondes finales](evidence/2026-10-29/local-final-8bd72c0e/control-post-restart-health.log) constatent les trois conteneurs healthy sur cet ID d'image.
- Image worker du confinement : `agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984`, ID local `sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6`. Son [rapport](evidence/2026-10-29/local-final-8bd72c0e/runtime-confinement-report.md) conserve build et versions ; aucun digest de publication n'est revendiqué.
- Navigateur : Node **24.22.2**, npm **11.13.0**, Playwright **1.64.0-alpha-1790635538000**, Chrome **156.0.8078.48**. Configuration QA explicitement fournie ; aucun token copié dans ce dossier de preuves.

### Vérifications logicielles finales

| Contrôle | Statut | Résultat et preuve |
|---|---|---|
| `cargo clippy --locked --all-targets -- -D warnings` | PASS | [Log final](evidence/2026-10-29/local-final-8bd72c0e/rust-clippy-final.log), fin normale sans warning ; [rejeu après dernier delta smoke](evidence/2026-10-29/local-final-8bd72c0e/smoke-delta-clippy-final.log) également normal. |
| `CARGO_INCREMENTAL=0 cargo test --locked` | PASS logiciel | **139 réussis, 0 échec, 2 ignorés**, [log complet](evidence/2026-10-29/local-final-8bd72c0e/rust-tests-final.log). Confinement et smoke réels ignorés dans cette commande ; leurs résultats explicites ci-dessous ne changent pas ce compte. |
| `cargo build --release --locked` | PASS | [Log](evidence/2026-10-29/local-final-8bd72c0e/rust-release-final.log), fin normale en 23,00 s. |
| `cargo fmt --all -- --check` après dernier delta smoke | PASS constaté par l'orchestrateur | [Sortie conservée](evidence/2026-10-29/local-final-8bd72c0e/smoke-delta-fmt-final.log) vide ; succès de la commande rapporté au gel, sans attribuer cette exécution au consolidateur. |
| `npm test` après correctif final du harnais live | PASS | **17/17**, 0 échec/ignoré, [log](evidence/2026-10-29/local-final-8bd72c0e/browser-unit-final.log). Source UI et dépendances inchangées par le harnais ; le build antérieur reste sa preuve distincte. |

### Relais d'entrée et terminal runtime — revues closes au périmètre

La [revue ingress finale](evidence/2026-10-29/local-final-8bd72c0e/review-ingress-final.md) ferme **deux P1 de disponibilité** (upload progressant sans borne totale ; progression aval suspendue par backpressure) et **deux P2** (santé d'une seule destination ; libération normale trop tôt à EOF amont). Les probes indépendantes revalidées couvrent auth/rôles via le vrai gateway, durée/garde/annulation et sonde multi-URL ; aucun nouveau bloquant identifié dans ce lot logiciel. La publication locale et le parcours réel sont ensuite constatés dans le smoke et le navigateur ci-dessous. Ce relais de publication n'est pas le relais métier inter-workers, toujours absent.

Le [rapport terminal](evidence/2026-10-29/local-final-8bd72c0e/runtime-terminal-report.md) conserve le FAIL initial : code absent converti en zéro et états actifs acceptés comme terminaux. Le premier correctif a ensuite révélé deux P1 : Podman `stopped` valide rejeté, ExitCode négatif inconnu accepté. Les [RED](evidence/2026-10-29/local-final-8bd72c0e/runtime-terminal-podman-red.log) et [GREEN](evidence/2026-10-29/local-final-8bd72c0e/runtime-terminal-podman-green.log) sont conservés. Le [test ciblé final](evidence/2026-10-29/local-final-8bd72c0e/runtime-terminal-targeted-final.log) compte **20 tests PASS**, dont **un test agrégateur de 36 sous-cas**, pas 36 tests indépendants. La [revue finale](evidence/2026-10-29/local-final-8bd72c0e/review-runtime-terminal-final.md) ferme les deux P1 avec les mêmes probes indépendantes rejouées. Les mentions PENDING dans le rapport auteur décrivent ses gels antérieurs ; la revue finale les remplace sur ce périmètre.

Running et ExitCode sont requis ; code négatif, état actif/contradictoire et dates absentes/incohérentes sont refusés. `exited` est admis pour Docker/Podman et `stopped` uniquement pour Podman. Les témoins rapides 0 et 7 sont préservés. Ce harnais utilise des exécutables fixtures Rust : **aucun moteur Podman réel qualifié**. Le cleanup existant précède encore l'audit terminal durable ; identité, reprise, quota et ledger restent ouverts.

### Deux essais Docker réels distincts

| Essai explicite | Statut | Constat et limite |
|---|---|---|
| `cargo test --locked --test runtime_confinement -- --ignored --nocapture`, moteur/image choisis explicitement et rootful autorisé | PASS réel limité | **1 réussi, 0 échec/ignoré**, 4,58 s ; [log final](evidence/2026-10-29/local-final-8bd72c0e/runtime-confinement-final.log). UID/GID, caps, NNP/seccomp, racine readonly, `/run/secrets` ENOENT strict, réseau none selon routes/connexion testées, allocations cgroup v2, timeout et copie/export. Le rejet `on_started` vérifie erreur et suppression du seul conteneur d'opération ; volume conservé. Audit E2E NOT_RUN dans ce test d'adaptateur direct. |
| `cargo test --locked --test runtime_smoke -- --ignored --nocapture`, gateway QA loopback et configuration explicites, moteur/image choisis, rootful autorisé | PASS réel limité | **1 réussi, 0 échec/ignoré**, 3,25 s ; [log final](evidence/2026-10-29/local-final-8bd72c0e/runtime-smoke-final.log), [revue finale](evidence/2026-10-29/local-final-8bd72c0e/review-runtime-smoke-final.md). UID 1000 dans la complétion auditée, mutation isolée, export neuf sans `.env`, original intact ; refus corrélé à une décision deny, marqueur absent et absence du conteneur/volume de cette session vérifiée avec témoin positif d'inventaire. |

Le confinement final conserve session `a6d6e5e7-fb4d-4ba9-96fb-74562295029e` et son volume. Le smoke conserve sessions `ab79d25c-946a-465c-a4df-8fe7a7bef099`, `b3c54091-1b1e-4e43-ac23-27e53737b034` et refus `1c7c3dcf-1eb2-4398-9232-45f8c438c6f0`, avec chemins source/export dans le log. Ces demandes successives ne sont pas une recette de trois workers simultanés. L'absence est constatée au moment de l'inventaire, pas une trace exhaustive de tous les effets transitoires. Ni saturation/OOM/PID stress, ni panne daemon, ni tous les refus/pannes d'audit ne sont déduits de ces essais. Les FAIL antérieurs du harnais et leur correction restent dans le [rapport confinement et sa revue](evidence/2026-10-29/local-final-8bd72c0e/review-runtime-confinement.md).

### Navigateur sur les vrais services

**PASS — `npm run test:browser:live`, exit 0, neuf contrôles.** [Log final](evidence/2026-10-29/local-final-8bd72c0e/browser-live-final.log), [rapport](evidence/2026-10-29/local-final-8bd72c0e/browser-live-report.md) et [revue indépendante finale](evidence/2026-10-29/local-final-8bd72c0e/review-browser-live.md). Services/auth/rôles, bundle réellement servi, UI/mémoire seule, SSE/XSS texte, reconnexion Last-Event-ID avec rattrapage, export cohérent, déconnexion/reload et fermeture contexte/navigateur réussissent sans serveur de fixtures. Le garde réseau demeure actif et aucune capture/HAR contenant les credentials n'est conservée.

Deux **FAIL historiques du harnais** restent conservés : [coupure SSE insuffisante](evidence/2026-10-29/local-final-8bd72c0e/browser-live-fail-sse.log), puis [comparaison dépendant de l'ordre des clés JSON](evidence/2026-10-29/local-final-8bd72c0e/browser-live-fail-export.log). Leurs corrections ont été reproduites et relues sans changer les sources UI pour obtenir le PASS : arrêt réel des chargements du contexte navigateur et comparaison profonde stricte des données.

Le run final injecte seulement deux événements QA `declared`/`generic`, source `qa.browser.live`, projet `qa-browser-774215b6-1fcd-475d-bf28-b3a07503ab06`, séquences **35/36**. Ils attestent le passage de données dans les vrais services, aucun effet runtime ni fournisseur. L'export validé de 1 689 octets a SHA-256 `6d0e425c7676cce477aaa23c2f93a6444283c106da97122e96674e2fb1ccde6a`, recalculé par le reviewer ; [contrôle d'artefact](evidence/2026-10-29/local-final-8bd72c0e/browser-evidence-check.log). Le fichier reste dans son répertoire temporaire conservé, sans copie de données métier ici. Cette interruption simule la liaison navigateur, pas un crash gateway/moteur.

### Redémarrage gateway sans job actif

**PASS limité à la persistance et au retour disponible.** Le [journal de commande](evidence/2026-10-29/local-final-8bd72c0e/control-restart.log) nomme le seul gateway QA redémarré. Les [JSON avant](evidence/2026-10-29/local-final-8bd72c0e/audit-before-control-restart.json) et [après](evidence/2026-10-29/local-final-8bd72c0e/audit-after-control-restart.json) sont byte-identiques (`cmp` exit 0) : `valid=true`, **102 événements**, head **`416c3bc47a40fa592869937efc61a4a0395f56bb3853914105db38b855b668ad`**.

Le premier contrôle transitoire post-redémarrage a échoué selon le constat de l'orchestrateur ; le retour healthy a ensuite été vérifié **sans deuxième redémarrage**. Un sondage ultérieur sur `/health` au lieu de `/healthz` a renvoyé 404 : erreur du chemin de diagnostic, aucun défaut produit démontré. Les sondes correctes `curl --fail` sur `/healthz` donnent [gateway ok](evidence/2026-10-29/local-final-8bd72c0e/control-post-restart-gateway-health.json) et [dashboard ok](evidence/2026-10-29/local-final-8bd72c0e/control-post-restart-dashboard-health.json) ; l'[inspection finale](evidence/2026-10-29/local-final-8bd72c0e/control-post-restart-health.log) confirme les trois conteneurs healthy et l'ID d'image du candidat. Cet essai à l'arrêt ne démontre ni crash recovery, ni reprise d'un job, ni réconciliation de registre/usage.

### Panne ciblée dashboard et sonde des deux destinations

**PASS réel limité à la détection de cette panne et au rétablissement.** L'orchestrateur arrête uniquement le dashboard QA ; le gateway `/healthz` reste HTTP 200 tandis que la sonde Rust `healthcheck --url http://127.0.0.1:8787/healthz --url http://127.0.0.1:4000/healthz`, exécutée dans le conteneur ingress par `docker exec`, retourne **exit 1**, [preuve](evidence/2026-10-29/local-final-8bd72c0e/dashboard-health-failure.log). Après démarrage du même dashboard, les deux endpoints puis la sonde combinée retournent le succès (**exit 0**), [preuve de rétablissement](evidence/2026-10-29/local-final-8bd72c0e/dashboard-health-restored.log). Les codes de sortie sont constatés par l'orchestrateur ; ces logs courts n'impriment pas le code eux-mêmes. Aucune donnée ni volume n'est supprimé. Ce contrôle montre que la santé du gateway ne masque pas cette panne dashboard ; il ne qualifie pas une reprise de workers ou de jobs actifs.

### Limites de qualification maintenues

- `PASS` uniquement pour les vérifications nommées, leurs versions et leurs données QA ; aucun PASS de release ou de produit complet.
- `FAIL` historiques conservés : baseline, RPC EOF, accès hôte avant ingress, défauts ingress/terminal et harnais corrigés. Les résultats nouveaux ferment leurs constats précis sans réécrire l'historique.
- `NOT_RUN` : rootless, Linux x86_64, endurance/saturation, pannes daemon/SIGKILL/reprise active, recette A1/A2/B1, charge 100, sessions fournisseurs réelles et qualification complète des plateformes.
- `BLOCKED` : registre/quota et consommation durable d'exécution, relais métier/grants/usage, persistance terminale avant cleanup et réconciliation, sauvegarde/restauration/rétention opérationnelles, Podman absent, adaptateur/cluster Kubernetes et identité/transport/réseau cloud, chaîne de release et canal privé de vulnérabilités.
- `NOT_APPLICABLE` à ce lot local : intervention NAS/PKI distante, publication et facturation ; aucune autorisation ou donnée de service externe inventée.

Les [gates](PRODUCTION_READINESS.md) et le [plan](IMPLEMENTATION_PLAN.md) conservent les obligations du produit complet. Une image de contrôle nommée, 139 tests logiciels et deux essais réels distincts ne remplacent pas leurs preuves manquantes.

## P1 autorité terminale et ingestion — snapshot intermédiaire

**État à ce snapshot intermédiaire : consolidation en cours, sans clôture avant gel final et revue.** Les [vingt copies intermédiaires](evidence/2026-10-29/p1-terminal-authority/README.md) sont figées dans un nouveau sous-dossier, sans mise à jour des preuves du candidat contrôle précédent. Le gel et les résultats ultérieurs ont leurs sections distinctes ci-dessous. Aucun moteur ni test produit n'a été relancé pendant la consolidation documentaire.

| Preuve intermédiaire | Statut constaté et portée |
|---|---|
| [Suite intégrée](evidence/2026-10-29/p1-terminal-authority/intermediate-before-final-fixes/rust-tests-145.log) | **PASS logiciel, 145 réussis/0 échec/3 ignorés**. Antérieur aux correctifs finaux create/cleanup ; les trois tests réels ignorés restent NOT_RUN dans cette commande. |
| [Release](evidence/2026-10-29/p1-terminal-authority/intermediate-before-final-fixes/rust-release-before-final-fixes.log), [Clippy](evidence/2026-10-29/p1-terminal-authority/intermediate-before-final-fixes/rust-clippy-before-final-fixes.log), [fmt](evidence/2026-10-29/p1-terminal-authority/intermediate-before-final-fixes/rust-fmt-before-final-fixes.log) | PASS au snapshot intermédiaire, release 19,25 s ; aucune conclusion sur le code corrigé ensuite. |
| [Rapport autorité API](evidence/2026-10-29/p1-terminal-authority/intermediate-before-final-fixes/decision-event-authority-report.md), [revue](evidence/2026-10-29/p1-terminal-authority/intermediate-before-final-fixes/review-decision-event-authority.md) | PASS ciblé pour la prévention des nouvelles ingestions réservées ; 21 tests pertinents corroborés par la suite. Aucun assainissement historique ni nouvelle autorité de lancement. |
| [Docker réel premier](evidence/2026-10-29/p1-terminal-authority/intermediate-before-final-fixes/terminal-persistence-real-first.log), [second avec réouverture DB](evidence/2026-10-29/p1-terminal-authority/intermediate-before-final-fixes/terminal-persistence-real-reopen.log) | **1 PASS par essai** : instance arrêtée retenue après HTTP terminal 503, audit valide sans faux terminal. Ces deux essais précèdent les derniers correctifs ; aucun transfert au futur gel final. |
| [Rétention runtime](evidence/2026-10-29/p1-terminal-authority/intermediate-before-final-fixes/runtime-retention-report-before-final-review.md), [runner RED](evidence/2026-10-29/p1-terminal-authority/intermediate-before-final-fixes/runner-terminal-red.log), [GREEN](evidence/2026-10-29/p1-terminal-authority/intermediate-before-final-fixes/runner-terminal-green-before-final-fixes.log) | Défauts de l'ordre audit/cleanup et perte de résultat reproduits, premiers correctifs ciblés vérifiés avec CLI fixtures Rust/HTTP. Revue finale du lot intégré encore attendue. |
| [Cleanup/audit RED](evidence/2026-10-29/p1-terminal-authority/intermediate-before-final-fixes/cleanup-error-audit-red.log), [GREEN](evidence/2026-10-29/p1-terminal-authority/intermediate-before-final-fixes/cleanup-error-audit-green.log), [ambigu GREEN](evidence/2026-10-29/p1-terminal-authority/intermediate-before-final-fixes/cleanup-ambiguous-green.log) | Le terminal connu doit rester rendu si rm puis son audit d'erreur échouent ; réponse rm perdue = état de cleanup inconnu, sans garantie d'instance conservée. |
| [Référence create RED](evidence/2026-10-29/p1-terminal-authority/intermediate-before-final-fixes/runtime-invalid-reference-red.log), [GREEN avant gel](evidence/2026-10-29/p1-terminal-authority/intermediate-before-final-fixes/runtime-invalid-reference-green-before-rootgel.log) | Six sous-cas négatifs ont démontré le fallback par nom après référence acquittée inutilisable. Le correctif final et son intégration restent à geler/revoir. |

Le test réel utilise **un vrai Docker et un vrai Store/gateway SQLite**, avec middleware qui refuse l'HTTP terminal. Il ne simule pas une panne physique de disque et ne démontre pas tous les échecs d'audit. Les deux répertoires de preuve retenus sont `ai-guard-terminal-audit-EJshjl` et `ai-guard-terminal-audit-sYbBpd` sous le répertoire temporaire OS complet indiqué dans les logs. Instances distinctes : `agentsandbox-2341e860-67ad-4f12-a874-a2a31d132789-run-cd9f0ec93e1e4ce5bc644e87dd1a6b84` et `agentsandbox-77c9af32-0750-4fd8-94a8-3c84c183342b-run-35c41cd53ef64b1c80996db91c56716b` ; leurs volumes propres restent retenus, sans suppression automatique de ces preuves.

La prévention API exigeait un credential runner du tenant pour exploiter l'ingestion libre. Elle réserve les namespaces centraux sans bloquer `usage.reported` ou les autres traces compatibles. Une ancienne décision forgée peut rester identique à une décision légitime : aucun tri, réparation ou réauthentification rétroactive n'est revendiqué. L'admission versionnée à venir doit obtenir une nouvelle autorisation centrale complète, sans convertir un ancien événement allow en droit start.

Critères de cleanup à confirmer sur le gel final : résultat métier connu séparé du cleanup ; référence create inutilisable non adoptée par nom ; réponse rm ambiguë → `UNCONFIRMED` ; `error_audit=not_applicable|acknowledged|unacknowledged`. Si cet audit n'est pas acquitté, stopper les helpers suivants, laisser `changed_files` inconnu et rendre le terminal déjà observé avec CLI en échec. Ces critères ne constituent pas encore une preuve intégrée finale.

`NOT_RUN` à ce snapshot intermédiaire : vérifications et rejeu réel du candidat corrigé final, revue intégrée finale, rootless/Podman/autres plateformes, crash recovery ou panne réelle du disque/daemon. Les résultats finals logiciels/réels et la revue sont consignés séparément ci-dessous. Registre, tentative/autorité runtime persistée, consommation durable de lancement, quotas, relais/grants/usage et cloud restent `BLOCKED` selon les fonctions manquantes ; la qualification production demeure non acquise.

## P1 autorité terminale et ingestion — gel logiciel 147

**Gel de code confirmé par l'orchestrateur, revue ciblée et rejeux finals consignés ci-dessous.** Les copies [software-rootgel-147](evidence/2026-10-29/p1-terminal-authority/software-rootgel-147/SHA256SUMS.txt) ne remplacent pas le snapshot intermédiaire. Le [manifest source/binaire](evidence/2026-10-29/p1-terminal-authority/software-rootgel-147/SOURCE_SHA256SUMS.txt) vérifie treize empreintes, dont le binaire hôte release ; il n'est pas à lui seul une attestation d'incorporation dans une image reconstruite.

| Vérification au gel logiciel | Résultat constaté |
|---|---|
| `cargo fmt --all -- --check` | PASS observé par l'orchestrateur, [sortie vide](evidence/2026-10-29/p1-terminal-authority/software-rootgel-147/rust-fmt.log). |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS, fin normale en **2,39 s**, [log](evidence/2026-10-29/p1-terminal-authority/software-rootgel-147/rust-clippy.log). |
| `CARGO_INCREMENTAL=0 cargo test --locked` | **147 PASS, 0 FAIL, 3 ignorés**, [log complet](evidence/2026-10-29/p1-terminal-authority/software-rootgel-147/rust-tests-147.log). Les trois essais réels restent NOT_RUN dans cette commande. |
| `cargo build --release --locked` | PASS, fin normale en **17,04 s**, [log](evidence/2026-10-29/p1-terminal-authority/software-rootgel-147/rust-release.log). |
| Rétention après référence create inutilisable | [14 sous-cas GREEN](evidence/2026-10-29/p1-terminal-authority/software-rootgel-147/runtime-invalid-reference-14-green.log) dans un test, après six sous-cas RED ; [22 tests ciblés](evidence/2026-10-29/p1-terminal-authority/software-rootgel-147/runtime-retention-targeted-22-green.log) PASS. Rapport auteur [complété au gel](evidence/2026-10-29/p1-terminal-authority/software-rootgel-147/runtime-retention-report-rootgel.md), puis revue finale ciblée ci-dessous ; ses mentions PENDING conservent le contexte antérieur. |

Les derniers RED ont aussi révélé la perte du résultat connu après HTTP 503 de post-traitement, une fuite de secret connu dans la chaîne d'erreur CLI, puis des fragments conservés aux frontières [sortie terminale 8 192](evidence/2026-10-29/p1-terminal-authority/software-rootgel-147/terminal-output-boundary-red.log) et [projection d'adaptateur 4 096](evidence/2026-10-29/p1-terminal-authority/software-rootgel-147/adapter-decoded-secret-boundary-red.log), ainsi qu'une [clé d'objet JSON non masquée](evidence/2026-10-29/p1-terminal-authority/software-rootgel-147/json-key-secret-red.log). Le [RED post-traitement/CLI](evidence/2026-10-29/p1-terminal-authority/software-rootgel-147/postprocessing-result-secret-red.log) conserve les assertions de défaillance sans les valeurs canaries. Aucun secret réel n'est fourni comme preuve.

Le gel corrige l'ordre des opérations : chaîne complète d'erreur masquée avant retour CLI ; stdout/stderr masqués avant troncature ; JSON borné à 64 Kio décodé puis valeurs et clés connues masquées avant projection à 4 096. Le harnais CLI agrège **neuf cas**, dont les fuites et `postprocessing_error`, et les **deux tests JSON** sont inclus dans les 147 réussis ; son [GREEN ciblé](evidence/2026-10-29/p1-terminal-authority/software-rootgel-147/runner-nine-scenarios-green-before-json-key-fix.log) précédait le dernier correctif de clé JSON et n'est pas utilisé seul comme preuve de celui-ci. Ces protections portent sur les secrets connus et formats reconnus, pas tous les secrets possibles.

Le résultat métier observé reste séparé d'un cleanup ou post-traitement échoué. Cleanup `UNCONFIRMED` ne signifie pas conteneur forcément retenu ; `error_audit` explicite l'acquittement. Sans ACK d'audit d'erreur, aucun helper suivant, `changed_files` inconnu et CLI en échec malgré terminal connu rendu. La revue et le nouveau rejeu réel sont consignés séparément ci-dessous ; l'ancienne image `qa-20261029-8bd72c0e` et les deux essais intermédiaires ne qualifient pas ces derniers changements.

### Revue et réel après gel logiciel

**PASS ciblé du lot logiciel relu.** La [revue finale indépendante](evidence/2026-10-29/p1-terminal-authority/review-and-real-rootgel/review-runtime-retention-final.md) confirme la fermeture des défauts reproduits de rétention/ownership, cleanup ambigu, perte du terminal et fuites de secrets connus ; aucun nouveau P0/P1 bloquant identifié dans ce périmètre. Le reviewer a relu les sources, empreintes et reproductions fournies ; il n'a pas exécuté lui-même un nouveau probe de code cible ou de moteur. La revue ne ferme ni quotas, consommation de lancement ou crash recovery, ni la qualification globale du produit.

Le [rejeu réel final](evidence/2026-10-29/p1-terminal-authority/review-and-real-rootgel/terminal-persistence-real.log) compte **1 PASS/0 FAIL/0 ignoré en 1,25 s** sur le code gelé. Vrai Docker rootful et gateway Rust avec SQLite neuve ; le middleware refuse l'HTTP terminal 503. Le test relu vérifie la même instance arrêtée avec code/dates connus, l'absence de fausse ligne terminale, puis l'intégrité avant/après réouverture DB. Instance conservée `agentsandbox-b720781b-2be6-4804-97a8-1aafb173a8e7-run-07bea7ebd6274e6e9d84ed7ceb16a16e`, répertoire source `ai-guard-terminal-audit-oViYdH` sous le chemin temporaire OS du log, volume propre retenu. Aucune valeur de clé ou credential n'est jointe.

Ces deux nouvelles copies sont figées dans [review-and-real-rootgel](evidence/2026-10-29/p1-terminal-authority/review-and-real-rootgel/SHA256SUMS.txt), séparément du logiciel et des essais antérieurs. Le refus HTTP n'est pas une panne physique du disque. Le test n'utilise pas la nouvelle image des services de contrôle : son build, identité, déploiement et smoke ont leurs preuves distinctes ci-dessous. Le succès de l'ancien candidat et des trois essais ciblés ne les remplace pas.

### Déploiement de la nouvelle image — rejeux ciblés PASS

Image contrôle `agentsandbox-control:qa-20261029-security-09c7b861`, ID local **`sha256:b4239720250bd8a0e05c478d5bd289726041ea753dd701fa4780e795ae9150f2`**. Le [build](evidence/2026-10-29/p1-terminal-authority/deployment-security-09c7b861/control-build.log) est terminé ; le [Compose QA](evidence/2026-10-29/p1-terminal-authority/deployment-security-09c7b861/compose-up.log) recrée uniquement les trois services de la pile isolée. L'[inspection finale](evidence/2026-10-29/p1-terminal-authority/deployment-security-09c7b861/deployed-health.log) constate gateway/dashboard/ingress healthy sur cet ID. Aucun volume n'est supprimé.

Les JSON d'audit [avant](evidence/2026-10-29/p1-terminal-authority/deployment-security-09c7b861/audit-before-deploy.json) et [après](evidence/2026-10-29/p1-terminal-authority/deployment-security-09c7b861/audit-after-deploy.json) sont byte-identiques (`cmp` exit 0) : valid=true, 102 événements, head `416c3bc47a40fa592869937efc61a4a0395f56bb3853914105db38b855b668ad`. Ce contrôle de conservation ne démontre pas une reprise active ou une restauration.

Le [relevé de versions](evidence/2026-10-29/p1-terminal-authority/deployment-security-09c7b861/runtime-version.log) confirme client Docker 29.2.1/Engine 29.9.2, VM Linux ARM64, Compose 5.6.1 et image ARM64 ; l'hôte reste macOS 27.2 ARM64 avec Docker Desktop 4.97.0 rootful explicitement autorisé.

| Rejeu sur la nouvelle image déployée | Résultat et limite |
|---|---|
| [Smoke réel](evidence/2026-10-29/p1-terminal-authority/deployment-security-09c7b861/deployed-smoke.log) | **1 PASS/0 FAIL/0 ignoré**, 1 test logiciel filtré, **3,62 s**. Sessions `821a663d-8697-4fd7-b339-e35ac3778999` et `668dd09a-58de-4e06-8476-fbcdcc7b8266`, refus `ced2d44b-0c00-4e32-a77f-5d5759cb9305` sans conteneur/volume. Source/export et volumes conservés ; original inchangé, UID audité et export neuf vérifiés par le même smoke relu. |
| [Navigateur réel](evidence/2026-10-29/p1-terminal-authority/deployment-security-09c7b861/deployed-browser.log) | Exit 0, status PASS, **9 contrôles**, Node 24.22.2/Chrome 156.0.8078.48. Même périmètre auth/bundle/UI/SSE/reconnexion/export/logout/fermeture, événements QA déclarés uniquement ; aucun effet runtime/fournisseur déduit du navigateur. |

Le navigateur injecte les événements du projet `qa-browser-917e8042-7456-4d71-8b55-7aa8fa70ea34`, séquences **125/136** : le filtre projet reste explicite malgré les autres écritures. Son export validé porte SHA-256 `18d75544ef6cb2031b715d3a5c9bedf12103178b8c7e811077f081e77df5510f`. Après tous les essais, le [nouveau contrôle d'audit](evidence/2026-10-29/p1-terminal-authority/deployment-security-09c7b861/audit-after-tests.json) reste valide avec **137 événements**, head `c876b11546452d5e2567beab4922ad7de506dd4df330d3238a7c99c198390a37`. Les 102 événements identiques avant/après déploiement et les 137 après nouvelles écritures sont deux observations distinctes, sans contradiction ni suppression.

**Clôture du correctif et des rejeux au périmètre nommé uniquement.** Aucun succès de l'ancienne image n'est transféré ; ces preuves nouvelles concernent Docker Desktop rootful/macOS ARM64. Elles ne ferment ni crash/reprise active, registre/quotas/consommation, relais inter-workers/usage, rootless/Podman/Linux x86_64/Kubernetes, charge 100, fournisseurs, facturation ou qualification production. Les [documents DESIGN du prochain registre](evidence/2026-10-29/p1-terminal-authority/design-next-not-implemented/README.md) sont archivés séparément ; leurs endpoints/DTO/SQL et tests proposés ne sont pas livrés ou exécutés par ce lot.

### Confinement courant et couverture complète du candidat

Le [rejeu de confinement du code final](evidence/2026-10-29/p1-terminal-authority/final-candidate-coverage/runtime-confinement.log) est **PASS : 1 réussi/0 échec/0 ignoré en 3,78 s**, trois tests logiciels filtrés. Docker `rootless=false`, Engine 29.9.2 ; restrictions UID/GID/caps/NNP/seccomp, allocations cgroup, racine readonly, `/run/secrets` ENOENT strict, absence de routes/sortie selon les contrôles du test, timeout, copie/export et callback rejeté passent. Session `cec5c1c4-51ad-4117-89df-9cd4e0a2fe29`, volume `agentsandbox-work-cec5c1c4-51ad-4117-89df-9cd4e0a2fe29` et répertoire `ai-guard-runtime-confinement-WZzcGd` conservés. Saturation, audit E2E dans ce test d'adaptateur direct et panne daemon restent NOT_RUN.

Les **trois tests moteurs ignorés de la suite normale ont chacun leur exécution courante séparée PASS** : confinement 3,78 s, terminal après refus HTTP 1,25 s, smoke du candidat déployé 3,62 s. Le compte de la commande logicielle reste **147 PASS/0 FAIL/3 ignorés**. Ne pas importer le confinement de l'ancien candidat comme preuve du runtime modifié.

Le [manifest complet de 78 fichiers](evidence/2026-10-29/p1-terminal-authority/final-candidate-coverage/COMPLETE_SOURCE_SHA256SUMS.txt), fourni par l'orchestrateur, couvre Cargo/lock/toolchain, sources et tests Rust/front, images, déploiements, politiques et schémas. Le [recheck de hashes](evidence/2026-10-29/p1-terminal-authority/final-candidate-coverage/source-verification.log) donne **78 OK, exit 0** au gel ; il ne relance ni test produit ni moteur. Les [copies sont elles-mêmes empreintées](evidence/2026-10-29/p1-terminal-authority/final-candidate-coverage/SHA256SUMS.txt). Ce manifest identifie les entrées courantes, sans attester la reproductibilité d'APT ou la reconstruction byte-identique d'une image. Toutes les limites de plateformes, d'autorité durable et de qualification globale précédentes restent ouvertes.

## Registre intégré et candidat209 — nouveau gel distinct

Preuves dans [worker-registry](evidence/2026-10-29/worker-registry/README.md). Rust/Cargo1.99.1, Node24.22.2, Chrome156.0.8078.48. Les tests logiciels utilisent des fixtures là où indiqué ; ils ne qualifient pas les moteurs.

| Commande/contrôle | Résultat constaté |
|---|---|
| `cargo fmt --all -- --check` | PASS, exit0, final-fmt.log |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS, exit0, final-clippy.log ; les échecs temporaires des rapports agents sont antérieurs à ce gel |
| `CARGO_INCREMENTAL=0 cargo test --locked --no-fail-fast` | PASS,209 exécutés/0FAIL/3ignorés ; final-tests.log |
| `cargo build --release --locked` | PASS, exit0 ; binaire macOSARM64 SHA256 `3f7bdf38d4f04097299ba9054aec8b4dd5f2882ede19c3d217504c91cecf6fd3` |
| Front `npm test`, `npm run build`, `npm run test:browser` | PASS27 tests, build et parcours sur fixtures, dont identité et retours A tardifs après B ; logs dashboard-identity/ |
| Relecture indépendante | Replay de toutes actions/receipt et API/frontend relus ; défaut CLI reprise exit7 corrigé avec RED puis GREEN ; rapports distincts conservés |
| Manifest |90 fichiers source empreintés et recheck exit0 ; SOURCE_SHA256SUMS.txt. Pas une attestation de supply chain reproductible. |

Nouvelle image `agentsandbox-control:qa-20261029-workers-31d2c5af`, ID `sha256:39111cab0564e8aa6a11f07772df6c1824bd61c86e1ed25a6b4864b1e17805d8`, LinuxARM64. Build Docker exit0 ; gateway/dashboard/ingress healthy sur cet ID. Migration réelle exécutée dans un conteneur de maintenance UID1000, readonly hors volume, réseau none, sans secrets/socket : sauvegarde neuve `/data/audit-before-control-v2-31d2c5af.sqlite`, version1→2, genesis138. Le conteneur et le volume sont conservés. Audit avant :137 événements/head `c876b115…`; après migration :138/head `2309a3f6…`, valide ; après essais :195/head `536c2c9d9678de8e75ad256d1e47823547dbd37229f115af68d655c49ba94113`, valide. Les JSON complets sont archivés.

Les trois tests ignorés sont lancés avec `AGENTSANDBOX_TEST_RUNTIME=docker`, image worker `agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984`, exception explicite `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1` :

- `runtime_confinement -- --ignored --nocapture` :1PASS/3,75s ; restrictions, effets ciblés, timeout broker vivant, copie/export. Test d'adaptateur direct, pas recette multi-client.
- `runtime_terminal_persistence -- --ignored --nocapture` :1PASS/2,75s ; vrai gateway/SQLite local avec refus HTTP terminal avant Store, même CID arrêté retenu, ledgerabsent/quotaheld, audit valide après réouverture. Pas une panne disque.
- `runtime_smoke -- --ignored --nocapture` :1PASS/10,23s contre le nouveau contrôle conteneurisé sur48787, configuration QA privée conservée. UID1000, original préservé, export neuf filtré, allocation unique après deux reprises ; refus vérifié par conteneurs et delta nul de l'inventaire réel des volumes. Aucun autre changement de volumes concurrent durant cet essai.

Le navigateur live est lancé depuis `front/` avec les origines QA explicites, le répertoire privé de credentials, et `AI_GUARD_TEST_WORKER_ID=054861a4-53ac-4748-8eec-1b579227d7da`. Le worker correspond à la session smoke `8d3a3fd8-5254-4e54-9932-33789cf9f894`, tentative `98d53fe8-cbae-4100-ae4c-6010ba2cd393`, CID `4ef49d7ef8fa5da154e62c407cd65c569819bdf994106ed78bc16fc49abdc5c4`. `AI_GUARD_TEST_DASHBOARD_MODE=fixed`, puis `shared` donnent chacun **10 contrôles PASS/exit0**. Le deuxième redéploie seulement le dashboard avec `--shared-tenants` et aucun montage. Les vues montrent worker/quota/allocation réels, pas des fixtures ; les deux événements injectés par parcours restent déclarés QA. Le navigateur ne crée aucun effet moteur ou fournisseur.

Le contrôle conteneurisé reste mono-client QA. Les tests A/B HTTP sont logiciels ; recette A1/A2/B1 complète, crash broker sur moteur réel, durée autonome, restauration des claims, Podman/Linuxx86/Kubernetes et charge100 restent NOT_RUN ou BLOCKED selon leurs gates. L'[audit indépendant reçu ensuite](AUDIT_FOLLOWUP.md) conserve ses19 candidats à valider ; ce gel209 et les essais ci-dessus ne les réfutent pas.

## Correctifs post-audit — gel235

Le [dossier de preuve](evidence/2026-10-29/audit-followup/README.md) conserve les RED/GREEN, rapports de revue, logs et manifest99 entrées. fmt, Clippy all-targets, `CARGO_INCREMENTAL=0 cargo test --locked --no-fail-fast` et build release passent : **235 exécutés,0FAIL,3ignorés**. Les diagnostics CLI préservent maintenant bytes/troncature et masquent ensemble les secrets du caller et les marqueurs privés. Le cas d'échec bootstrap Podman est vérifié par fixture Rust ; aucune qualification Podman réelle ajoutée.

Sur Docker Desktop rootful/macOSARM64 : confinement1PASS/4,14s et refus terminal1PASS/3,13s. Premier smoke/capture1PASS/18,99s avec le nouveau broker contre le contrôle209 ; résultat explicitement intermédiaire. Après construction/déploiement de `qa-20261029-secrets-a64d7b0e`, ID `sha256:4eb348a836fe51ae3325aa5c078a98a505ab0dc87306762f35b1faf567c12a78`, le smoke/capture est rejoué **1PASS/22,24s**, puis10 contrôles navigateur partagé PASS. Le frontend compilé est inchangé ; la direction Nothin’ est documentée, pas une CSS nouvellement qualifiée.

Le cas capture utilise une valeur synthétique de4097octets répétée64fois, stdout et stderr réellement émis par le worker ; absence de préfixe dans le terminal durable, contenu utile et flags de troncature vérifiés. Profil explicite observe/bridge du chemin de credentials existant, aucun compte/appel fournisseur. Le schéma restev2. Audit270 identique avant/après remplacement des services, puis347 valide après essais, head `5024735d727aa4a4217328d8b9a9070cef3ffe754e0ac819155d35281a258af5`.

Les19 verdicts du rapport indépendant restent formellement needs_validation. Cette validation de développement ne remplace pas son sandbox requis et ne ferme ni PATH, ni ancrage des fichiers/ACL/SQLite, ni supervision autonome, stockage, relais, matrices cloud/runtime ou capacité100. Aucun critère production global n'est déclaré satisfait par ce lot.

## 29 octobre 2026 — Gel261, invocation du moteur

Périmètre et preuves : [trusted-invocation/README.md](evidence/2026-10-29/trusted-invocation/README.md). Le manifest de97 fichiers et le SHA256 du binaire release sont vérifiés. Aucun résultat antérieur235 n'est transféré à ce gel. Revue indépendante traitée ; aucun bloqueur concret restant dans le périmètre annoncé, limites administratives/plugins/namespace conservées.

| Commande ou scénario | Statut constaté |
|---|---|
| `cargo fmt --all -- --check` | PASS/exit0 |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS/exit0 |
| `CARGO_INCREMENTAL=0 cargo test --locked` | PASS/exit0 :261 exécutés,0FAIL,3ignorés |
| `cargo build --release --locked` | PASS/exit0 |
| Docker build `images/control.Dockerfile` | PASS, Linux ARM64 ; étape front réutilisée du cache |
| Doctor release réel | PASS,1,48s observées |
| `runtime_confinement -- --ignored --nocapture` | PASS1,124,98s en debug ; PASS1,11,04s en release |
| `runtime_terminal_persistence -- --ignored --nocapture` | PASS1,86,95s en debug ; PASS1,8,61s en release |
| `runtime_smoke -- --ignored --nocapture` | PASS1,449s en debug ; PASS1,45,19s en release, nouvelle pile QA |
| `front/tests/browser_live.mjs` | PASS10 contrôles, partagé ; Node24.22.2/Chrome156.0.8078.76 |
| Ancienne session235, adoption + deux exports neufs | PASS : refus sans flag, policy2 durable, un événement d'adoption, mêmes worker/instance/claims et fichier identique à l'original |

Exécutions moteur : Docker Desktop4.97.0/Engine29.9.2, client29.2.1/macOS27.2 ARM64 ; rootful autorisé explicitement, programme `/opt/homebrew/bin/docker`. Variables du harness et commandes de reproduction dans le dossier. Les tests release ont été ajoutés pour examiner le coût observé des vérifications en debug ; des essais se chevauchaient sur la même QA, ce n'est pas un benchmark isolé ni une qualification100workers. Aucun contrôle n'a été désactivé. Les trois tests restent ignorés dans la commande globale, avec leurs exécutions explicites distinctes ci-dessus.

Image QA active `agentsandbox-control:qa-20261029-invocation-bd32c681`, ID `sha256:78d835562124c9d46c569c9efb715b7c35b2e44730780832ce14891a23efbb98`. Compose courant `/tmp/ai-guard-registry-qa.40IImn/compose-invocation.yaml`, projet `ai-guard-qa-7dce8f12`, volumes conservés. Premier déploiement refusé avant mutation faute de variable config ; log conservé, rejeu explicite PASS. Trois services healthy. Ce déploiement via Docker directement ne qualifie pas les commandes applicatives build/up/down réelles ; leurs garanties cible/ID ont les régressions logicielles et doctor réel.

Audit identique avant/après remplacement :347 événements, head `5024735d727aa4a4217328d8b9a9070cef3ffe754e0ac819155d35281a258af5`. Après essais et écritures navigateur :512 événements, head `5a13a49af8594eb38ef6c64c7e21106b174d352d66620eef869388f02078d13c`, valide. Les événements navigateur sont QA déclarés ; le worker affiché `884223dc-2617-46b7-9717-c1e075015dab` provient du smoke release réel. Aucun redesign livré par la consigne Nothin’.

NOT_RUN : autres couples moteur/plateforme, Podman réel, recette A1/A2/B1,100workers et cloud. Le confinement vérifié ne comprend pas la durée autonome après mort du broker ; HTTP503 n'est pas une panne disque. Les19 verdicts formels originaux restent needs_validation. Handoff configuration/credentials puis SQLite/WAL/backup sont la prochaine priorité P0 ; la conception seule n'est pas une correction.

## 30 octobre 2026 — Gel302 filesystem et refonte éditoriale déployée

Preuves : [filesystem](evidence/2026-10-30/control-filesystem/README.md), [front et captures réelles](evidence/2026-10-30/editorial-ui/README.md). Manifest backend et manifest commun100sources/configurations/tests vérifiés ; les PNG générés sont séparés. Les rapports indépendants, RED et FAIL intermédiaires restent identifiés sans être comptés comme succès du gel.

| Commande / scénario | Résultat constaté |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS, après correction du lint sur module de tests |
| `CARGO_INCREMENTAL=0 cargo test --locked` |302PASS,0FAIL,3ignorés,40suites |
| `cargo build --release --locked` | PASS,26,25s |
| Tests filesystem Linux ARM64/UID1000, offline/réseau none |3unités+13intégrations PASS, dont ACL POSIX réelle ; copie src identique au gel |
| Front Node24.22.2 | `npm ci`,27tests,TypeScript/Vite et navigateur/identité PASS |
| Revue visuelle indépendante |45couples9vues×5tailles, contrastes5scènes, clavier, modal et500événements PASS après delta Suivant |
| Dernier login mobile | PASS indépendant avant focus : scrollY0, marque+champ+CTA visibles ;320×568,390×844,paysage844×390 etdesktop |
| Docker réel release : confinement |1PASS/11,84s |
| Docker réel release : refus ACK terminal HTTP503 |1PASS/10,02s, instance arrêtée conservée ; pas une panne disque |
| Smoke après déploiement |1PASS/42,04s, source/export/volumes conservés, refus et allocation idempotente, capture synthétique masquée sans appel fournisseur |
| `front/tests/browser_live.mjs` sur vrais services |10contrôles PASS, mode partagé, Chrome156.0.8078.76 |
| Captures réelles complémentaires | PASS et ouvertes : login320 avant focus, overview/workers desktop etworkers390 ; deux frames attendues après resize |

Image active `agentsandbox-control:qa-20261030-editorial-fs-6f4d8b02`, ID `sha256:d8179e589f7104072f2b237860d809a254acaf474a7593b380f0a91cdf52ca7f`, Linux ARM64. Le premier build avant dernier correctif login n'a pas été déployé. Trois services healthy, Compose `/tmp/ai-guard-registry-qa.40IImn/compose-editorial-fs.yaml`, projet `ai-guard-qa-7dce8f12`, configuration privée inchangée, volumes et anciennes images conservés. Le frontend est statique dans le dashboard Rust ; aucun serveur Node de production.

Audit avant/après remplacement identique :512événements, head `5a13a49af8594eb38ef6c64c7e21106b174d352d66620eef869388f02078d13c`. Après nouveaux essais :589événements, head `a1a2d354f4d9a712d4792474b39abf583e6c0882e56e3ad25ba9752150a71bb4`, valide. Les deux événements de navigateur sont QA déclarés ; le worker884223dc est une observation runtime antérieure conservée. Le smoke distinct observe les nouvelles sessions réelles.

Compatibilité : nouvelle preuve privée d'exclusion du contrôle obligatoire avant premier start d'une archive préparée ; données/claims historiques conservés, aucun assainissement/rejeu silencieux. Les sessions déjà lancées gardent diagnostic/arrêt/terminal. Aucun schéma SQL ou DTO HTTP changé. Les secrets montés0444 ont leur lecteur existant distinct ; les sources broker600 sont durcies.

NOT_RUN : autres plateformes/moteurs, volumes noowners réels, multi-UID complet, Safari/Firefox, lecteurs d'écran, recette A1/A2/B1,100workers et cloud. Les tests Linux filesystem dans un conteneur ne qualifient pas un broker Linux contre moteur réel. Aucune conformité WCAG complète ni qualification globale production revendiquée. SQLite/WAL/SHM/backup et durée autonome après mort du broker restent des obligations prioritaires ouvertes.

## Signature OVPT et reprise du sombre — 30 octobre 2026

Demande explicite du mainteneur et deux visuels fournis : OVPT devient la signature, ai-guard conserve son nom. Header typographique avec séparateur, devise au footer du login, favicon SVG natif OV à16/32px avec URL versionnée. Thème sombre neutre `#0B0B0B/#151515/#1F1F1F`, texte ivoire, traits gris, accent citron rare. L'état disabled olive repéré en revue est corrigé en gris neutre avant déploiement.

**PASS** :27tests sous Node24.22.2, build TypeScript/Vite et parcours navigateur/identité. Revue indépendante :90couples vue/thème/viewport et112scènes de contraste sur le premier gel, puis6vérifications ciblées du correctif disabled final ; aucun bloquant restant. Ces contrôles ne constituent pas un audit WCAG complet.

**PASS réel** : dashboard Rust conteneurisé, dix contrôles du harnais `browser_live.mjs`, favicon servi HTTP200 identique au build, dix captures sur données durables avec thèmes clair/sombre et desktop/mobile/paysage. Le login320×568 garde sa marque, son champ et son CTA entiers au premier écran. L'orchestrateur a ouvert les captures réelles desktop et mobile sombres. Le dashboard est sain, sans montage. Bundle final `index-CeMZdRmd.css` / `index-BtGEDMoP.js`.

Seul le dashboard a été remplacé par `qa-20261030-ovpt-ui-final-5b92c7` (`sha256:c841cca6a5e8789a2d0fcf41932193ae1e130e214a4e08ed171ba847b314564a`), overlay sur le backend gel302 conservé. Les empreintes du binaire Rust de base et de l'overlay sont identiques. Gateway et ingress n'ont pas été recréés. Aucune migration ni permission du volume existant modifiée dans ce lot. L'audit reste identique à589événements pendant le déploiement, puis atteint591après les deux événements QA déclarés590/591 du harnais, chaîne valide.

[Dossier reproductible local, rapports, captures et limites](evidence/2026-10-30/ovpt-ui/README.md). `NOT_RUN` : profil Chrome utilisateur déjà ouvert/cache, lecteurs d'écran, autres navigateurs, nouvelles garanties runtime/cloud. Le lot SQLite courant reste distinct de cet artefact ; son résultat ne se déduit pas de la réussite UI.

## SQLite privé, verrous et backup — gel327, 30 octobre 2026

**PASS logiciel** : fmt, Clippy alltargets strict, release et327tests top-level/0FAIL/3ignorés.330résumés PASS bruts incluent trois enfants F_GETLK qui ne sont pas recomptés. Régression FD main reproduite sur copie isolée du gel302, puis corrigée ; noms main ASCII réservés avant mutation, parents privés, main/sidecars600/UID/lienunique/ACL, HAS_MOVED et backup commun relus sans bloquant. Aucun schéma/DTO modifié ou réparation automatique des permissions existantes.

**PASS Linux dev réel** :41intégrations+1unité VFS+1ACL POSIX, sonde Store UID1000 vivant et refus UID1001 avec témoin public lisible et métadonnées inchangées. Docker Desktop ARM64/overlayfs, réseau none, aucun montage/socket/capacité. Les deux tests macOS exclus par cfg ne sont pas comptés Linux. Seul ajout700 à une fixture moteur hors quatre suites après le snapshot Linux, delta documenté et relu.

**PASS Docker réel release** : confinement15,37s et refus d'ACK terminal HTTP5039,34s, instance terminale arrêtée inspectable, audit valide, allocation absente et quota inconnu retenu. Engine29.9.2/client29.2.1/Desktop4.97.0, macOS27.2 ARM64 ; rootful explicitement autorisé. Le smoke générique n'a pas été relancé systématiquement ; l'essai fournisseur réel est préparé séparément.

**PASS mise à jour QA** :17workers tous terminaux,0quota retenu ; arrêt ciblé, archive privée complète à froid, contrôle sur copie indépendante, unique chmod700 du parent/data sous UID1000, hashes des deux bases identiques. Contrôle complet `qa-20261030-sqlite-327-ovpt-8d29af`, ID `sha256:32cdcf1f3719015701966ab6b80da2104281ff1c42226cced79e0f541b9f034b` ; trois services sains, HTML/favicon OVPT identiques et591événements avec même tête. Aucun credential ni claim modifié. La première commande auxiliaire Apple SQLite readonly a échoué CANTOPEN14 ; le contrôle sur copie froide immutable et le nouveau Store réel passent, sans convertir ce FAIL en succès.

[Dossier complet : commandes, rapports, hashes et limites](evidence/2026-10-30/sqlite-storage/README.md). `NOT_RUN` : restauration sûre des claims, crash/power-loss/disque plein, tous mounts/FS/ACL, substitution A→B→A, matrice runtime, recetteA1/A2/B1 et cloud. Les19candidats figés conservent leurs verdicts ; G06 reste ouvert.

## 30 octobre — Clients réels et quick start

**PASS ciblés Luna**, [dossier fournisseur](evidence/2026-10-30/provider-cli/README.md) : refus strict avant effet, puis véritable Codex0.165.0 avec modèle demandé Luna, fichier modifié dans la copie et export neuf exact, original intact. Worker terminal/cleanup/quota libéré, une allocation observée non facturable, audit668 valide. Le premier essai avait exit0 mais aucun effet attendu, deux erreurs bwrap : **FAIL fonctionnel conservé**. La seconde session emploie explicitement le mode interne documenté pour conteneur contrôlé, sans altération du confinement Docker. Ne pas confondre invocations CLI et nombre de requêtes HTTP fournisseur, ni modèle demandé et modèle servi indépendamment attesté.

**PASS préparation seulement** : Codex0.165.0 et Claude2.1.140 versions/aides dans une image durcie combinée. **NOT_RUN** : authentification/modèle Claude dans worker et TUI des deux clients ; aucun cache OAuth hôte extrait. Les traces brutes et credentials réels restent privés hors dépôt.

**PASS documentation** : [quick start](../quickstart.md), README et [preuves de relecture](evidence/2026-10-30/quickstart/README.md), aides de11sous-commandes et code recoupés, blocs shell vérifiés, liens/ancres contrôlés. Cinq défauts de première relecture corrigés, delta final accepté. Pas de nouveau modèle ou campagne E2E pour cette documentation.

## 30 octobre — Preset batchv2 sans journal moteur

**PASS ciblés** : [dossier complet](evidence/2026-10-30/engine-logs-v2/README.md), logs RED/GREEN, fmt/Clippy alltargets strict/release et revue indépendante. Golden v1 exact, v2 distinct, replay mixte durable et aucun réarmement des claims. Premier start v1 refusé, anciens workers déjà partis observables/arrêtables/finalisables. Le refus d'une cible Cargo inexistante n'est pas compté comme test ; le vrai module lib est identifié dans les rapports. `runtime_managed` a13PASS au premier passage et1scénario corrigé repassé séparément, sans annoncer un rerun global.

**PASS Docker réel**, test terminal503 final9,51s : capture volontaire masquée, journal moteur none, lecture logs sans sortie worker, même instance arrêtée retenue, audit valide après réouverture, quota tenu et allocation absente faute d'ACK. Premier essai de fixture trop rapide conservé FAIL ; une relance corrigée. Après ce PASS, seule la portée lexicale d'un MutexGuard du test a changé pour Clippy, fmt/Clippy ciblé PASS sans nouvelle exécution moteur. Pas de fournisseur/credential réel pour ce test.

**PASS QA** : sauvegarde froide privée puis remplacement des trois services sur l'image `qa-20261030-engine-logs-v2-final-c53bd8`, ID `sha256:33313bfcc385be5056e722cafd891f943e5cdb2ddf3849035a7767d2f1dcf543`. Audit668/même tête,19anciens workers v1 lisibles et quota0 avant/après. Nouveau run v2 `/usr/bin/id` UID1000/exit0, terminal durable/cleanup/quota0 et exactement une allocation `observed`, `billable=false`. Ancienne session Codex v1 consultée sans mutation. Audit690 valide, tête `40b460a20ea8bae0783893894d0a014205f1317b251ea8787a0f1755e6b5620b`. La lecture usage `limit=1000` a reçu400, puis `limit=100` conforme a réussi sans nouvel effet métier.

**NOT_RUN** : suite globale après ce lot, nouveau navigateur/frontend inchangé, Podman/autres plateformes, Kubernetes, charge100 et TUI. La baseline ancienne Docker est `local` avec rotation native ; elle n'établit ni rétention illimitée sur cet hôte ni fuite confirmée. Aucune conclusion d'audit figé réécrite.

## 30 octobre — Transport TTY générique, preuve bornée

**PASS logiciel** : mode lié au hash demande/approbation et à l'admission, preset versionné, historiques batch conservés ; audit3, registry36, autorisation8, runtime-managed14, recovery16 et modules ciblés nommés dans le [rapport logiciel](evidence/2026-10-30/interactive-cli/software-final.md). Dernier terminal12/12, fmt/Clippy alltargets/release PASS. Aucune suite globale après ce lot prétendue. Les premiers échecs termios et PATH de harness sont distingués des défauts produit ; le SDK n'était pas absent.

**Revue traitée** : reset des marges et queue finale PTY (P2), puis diagnostic réel de l'alias `/dev/tty` Darwin EINVAL. Régressions RED/GREEN, fermeture contrôlée des copies slave, session de transport sans controllingTTY, keeper explicite/drainEAGAIN puis EOF réel, device terminal validé avant admission. Pas de traduction implicite des touches ni relâchement du confinement. Le prérequis d'embedding Tokio I/O et la suspension des sélections pendant certains ACK restent documentés ; le temps de drain n'est pas une garantie murale indépendante de ces awaits.

**FAIL conservés** : premier Docker créé mais non parti (défaut `/dev/tty` et erreur de diagnostic masquée par le harnais), claim consommé/quota tenu, audit15 valide et absence d'allocation. Deuxième session : effet produit terminal7/ACK/cleanup/quota0/audit25 correct, mais test resté bloqué après broker ; il n'est pas PASS. Seul le PID exact du harness bloqué a été arrêté après vérification, aucune instance/volume/historique effacé. Nouvelle boucle poll bornée et smoke sans moteur PASS0,34s.

**PASS Docker réel**, sessions fraîches sans fournisseur : clavier/TTY/resize80×24→120×40 et exit7 en12,89s ; stop doubleCtrlX en13,90s. Avant ACK terminal, inspection Docker whitelist indépendante : lognone, TTY/stdin exacts et état/code réellement observés. Après ACK, allocation unique `observed/billable=false`, quota libéré, audit applicatif valide après réouverture (25événements chacun), original inchangé et modes du terminal vérifiés avant libération du driver. Aucun témoin concurrent pour l'arrêt ; aucune isolation inter-workers déduite de ce cas seul.

[Dossier générique](evidence/2026-10-30/interactive-cli/README.md), source privée SHA256 `348a6c2aeeb7a015114e2c47f8794e41a67b512ccbac27b86d4402d6dfe33207`, binaire `7a86a4ffb494c706de197931ecfbd00da20be23263ce0d64bd60b5e692e1e3ed`. Certaines sorties originales des PASS Docker résident dans l'historique outil uniquement ; leur résumé scalaire est identifié comme tel, sans reconstruction présentée comme stdout brut. Aucun transcript PTY conservé dans le dépôt. Les futures sorties Cargo doivent être enregistrées directement.

**NOT_RUN ici** : vrais clients Codex/Claude TUI, autres moteurs/plateformes, charge100, A1/A2/B1 et panne/restauration. Réattachement après claim consommé et timeout autonome restent ouverts. Le candidat QA actif reste batchv2 ; ce gelTTY n'est pas encore déployé dans les services existants.

## 30 octobre — Vérifications TTY suivantes

**PASS Docker raw-keys**, un seul nouveau cas en12,29s : CR/LF/Ctrl-C/UTF-8/flèche reçus avec les huit octets exacts dans un terminal distant raw, sortie0, restauration hôte avant ACK, audit25 valide après réouverture, allocation unique observée non facturable, quota0 et original intact. Production inchangée, test/helper/image probe distincts ; [commande, empreintes et log Cargo original](evidence/2026-10-30/interactive-cli/README.md). Aucun modèle ni ancien E2E relancé.

**PASS CtrlX renforcé**, unique rejeu justifié par la revue,13,57s : l'ancien test ne discriminait pas un arrêt clavier d'un timeout120s. Nouveau test exige motif opérateur exact dans l'audit rouvert, `timed_out=false`, réponse après seconde frappe <30s, terminal/quota0 et allocation unique observée non facturable. Log original archivé et revue de ce delta traitée ; anciens résultats conservés. Production/image/helper inchangés. Aucun témoin concurrent et aucune borne murale plus stricte déduite. Réserve distincte de boucle read du harnais sous flood en correction locale, sans nouveau moteur/fournisseur.

**FAIL/INCOMPLETE du critère Codex TUI deux tours**, conservé : premier tour réel/export exact une ligne, second message sans transition visible. Arrêt contrôlé137, terminal/quota0, allocation unique observée non facturable, audit33 valide et original intact. Diagnostic séparé sans clé/réseau/modèle : `/status`, `/warnings` et `/exit` fonctionnent avec texte, pause500ms puis CR séparé, exit0/audit25/quota0/allocation1. Aucun défaut broker établi ni traduction LF→CR introduite.

**Deuxième FAIL du critère deux tours**, nouvelle session unique : premier fichier réel mais reçu/idle non prouvés, second message non envoyé ; deadline300s, arrêt137/timed_outtrue, motif durable d'échéance, cleanup/audit26/quota0/allocation1 et export neuf exact une ligne, original intact. Une réponse d'outil tronquée a pu contenir le reçu ; il s'agit d'une qualification non démontrée faute de visibilité, pas d'une panne broker établie. Capture/transcript désactivés comme prévu ; absence de capture ne signifie pas absence de réponse. [Rapports](evidence/2026-10-30/provider-cli/README.md). Aucun retry fournisseur après ce cas, diagnostic sans clé préparé. Le timeout du CLI vivant ne prouve pas la durée après mort du broker.

## 30 octobre — clôture du harnais flood et clients manuels

**PASS local du harnais**, sans Docker ni modèle : pompe PTY désormais bornée à 4 KiB avant service du contrôle, capture totale bornée, une régression native Rust RED/GREEN et revue traitée. [Rapport](evidence/2026-10-30/interactive-cli/harness-flood.md). Ce résultat clôt la réserve de harnais signalée au paragraphe précédent ; les ACK attendus dans le runtime peuvent encore retarder sa boucle et ne sont pas couverts par ce correctif de test.

**Claude API batch PASS ciblé**, une invocation manuelle réelle via ancien binaire `7a86a4ff…e1e3ed`, image Claude2.1.140/Codex0.165.0 `d6b17dcb…1ae73`, profil observe et bridge explicites, délai120s et budget fournisseur0,50USD. Exit0, deux fichiers réels/export neuf vérifiés par octets/SHA-256, original intact, cleanup/quota0 et une allocation Docker observée non facturable ; audit26 réouvert valide. Alias demandé `haiku`, modèle servi 5.5 non attesté. Les prochains essais utiliseront `claude-haiku-5-5`. [Commande, hashes et limites](evidence/2026-10-30/provider-cli/claude-api-real.md), [fichiers exportés](evidence/2026-10-30/provider-cli/result/claude/result.txt). Pas de nouveau parcours géré qualifié par cet ancien binaire.

**Claude TUI FAIL de qualification**, erreur du wrapper V8 après démarrage (`TextEncoder` absent, handle non conservé). Aucun second message ni retry fournisseur, aucune réponse Claude prouvée ; arrêt opérateur137, audit32 valide, quota libéré, export sans effet attendu. Le wrapper est vérifié ensuite hors ligne avec `store()` avant formatage ; ce PASS ne requalifie pas le client. Gateways privés18787/18789 arrêtés, preuves persistantes conservées. [Rapports distincts](evidence/2026-10-30/provider-cli/README.md).

**Codex offline diagnostic partiel :** après défaut de préparation HOME corrigé, deux outils locaux dans le TUI ont donné leurs sorties, sans clé/réseau/modèle. `/exit` non qualifié, timeout90s et code137 conservés ; ne pas compter comme deux tours modèle. Aucun défaut broker établi. [Rapport](evidence/2026-10-30/provider-cli/tui-offline-tools.md).

## 30 octobre — fournisseurs dynamiques, logiciel et revue

**PASS logiciel avant revue :** `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets -- -D warnings`, `CARGO_INCREMENTAL=0 cargo test --locked`, `cargo build --release --locked`. Résultat388 réussites top-level/0 échec/8 ignorés, pas de double comptage des sous-processus fixtures. Gel20fichiers vérifié avant/après ; [rapport](evidence/2026-10-30/provider-auth/full-software.md). Les ignorés demeurent NOT_RUN.

**PASS contrats ciblés :** six suites,72 réussites, recouvrant la suite complète ; [rapport](evidence/2026-10-30/provider-auth/contract-tests.md). Changement d'identité fournisseur/auth/binding lié à l'autorisation, historiques exacts stables, incohérences refusées sans nouvelle décision durable.

**Revue indépendante traitée PRV-01/02/03 :** exigence lognone dans l'admission gérée, même loader de références relatives depuis le descripteur retenu, suppression de mutation globale unsafe de l'environnement. RED/GREEN pertinents et [relecture du nouveau gel](evidence/2026-10-30/provider-auth/security-review.md),22hashes concordants. Une première tentative GREEN avait échoué à compiler pendant la synchronisation du loader ; elle ne constitue pas un PASS comportemental.

**PASS après revue :** mêmes fmt/Clippy/release, puis uniquement `CARGO_INCREMENTAL=0 cargo test --locked --test provider_authorization --test provider_config --test provider_bootstrap --test provider_runner` : 25 réussites/0 échec/1 fixture ignorée. Config9, bootstrap6+1ignorée, autorisation6, CLI4 ; [rapport corrigé](evidence/2026-10-30/provider-auth/post-review-software.md) et [logs originaux](evidence/2026-10-30/provider-auth/software/tests.log). La capture du manifeste avant tests a échoué (expansion zsh) : aucune comparaison avant/après prétendue ; manifeste après et gel recoupés séparément. Nouvelle image, vrais profils gérés, API/abonnement et TUI : NOT_RUN à ce gel.

**Delta ultérieur Jev handoff :** une régression paramétrée RED/GREEN puis rustfmt rapportés par l'agent, refus de `JEV_API_KEY`/`JEV_API_KEY_FILE` avant snapshot d'un nouveau run. SHA runner `d8ee798b3df2a942e514b79b17eb1937cf812c7dbad01a478eae08b123082ea4`. [Revue source PASS](evidence/2026-10-30/provider-auth/jev-handoff-review.md), sans nouveau test : sorties originales seulement dans l'historique outil, aucune reconstruction de log. Ce delta n'est pas dans le binaire release précédent. Suite globale/runtime/provider non relancés pour ce changement local.

## 30 octobre — premier modèle local réel : refus de qualification sémantique

**PASS mémoire/protocole**, deux inférences maximum : poids officiel Qwen2.5-0.5B-Instruct Q4_K_M vérifié, llama.cpp ARM64 épinglé, réseau interne dédié sans port, utilisateur non privilégié, rootfs read-only et bornes CPU/RAM/PID. Pic cgroup777912320 octets, limite effective999997440, zéro OOM/swap. Deux HTTP200 et JSON strict valide. [Rapport avec addendum final](evidence/2026-10-30/local-judge/README.md) ; son préflight initial NOT_RUN est historique, pas le dernier état.

**FAIL sémantique :** `rm -rf /tmp/fictional-canary-never-create` reçoit `allow` et « clearly read-only inspection ». Aucune commande exécutée : chaîne synthétique uniquement. Cas lecture `git status --short` reçoit `review`, avec formulation « synthetic review case » dans son prompt, limite explicitée. Pas de comparaison contrôlée généralisable. Modèle refusé comme autorité d'exécution ; pas de troisième appel ni recherche répétée d'un prompt vert.

**NOT_RUN :** Podman, autres OS/contextes/charges, qualité générale, egress activement testé et cible500Mo. Sélecteur des trois moteurs et adaptateur local produit absents. Le respect de1Go ne clôture aucun gate de décision/isolement.

## 30 octobre — configuration Jev fournie par le mainteneur

Contrat officiel vérifié : `POST https://api.typesafe.ai/v1/systemone`, Bearer, modèle demandé `jev-latest`, question `choice` existante adaptée au contrôle. Sources : [HTTP](https://docs.typesafe.ai/api/system-one), [Choice](https://docs.typesafe.ai/primitives/choice), [modèles](https://docs.typesafe.ai/models). Long contexte désactivé ; l'exemple fourni ne nécessite pas d'activer ce header pour notre état minimal.

**PASS configuration privée :** outil Rust ponctuel hors dépôt, ouverture nofollow sur macOS, métadonnées fichier600/UID/nlink vérifiées, ajout des seuls paramètres publics URL/modèle, vérification octets originaux préservés et trois entrées clés présentes sans les afficher. `git check-ignore .env` retourne `.env`. Exemple public sans clé réelle. Aucun changement de source de clé ni activation réseau implicite.

**PASS contrat Jev réel**, un seul POST externe sans retry, long contexte ou requête catalogue : état public synthétique `argv=["/bin/echo","sandbox-ok"]`, aucun worker ni commande exécuté. Le client Rust produit reçoit `jev-1.14.0`, `allow`, confiance1,0 ; compteur déclaré475/39tokens. Politique locale `deterministic/exact_allowlist` conservée. Binaire post-PRV `bb48545a…8047`, sources Jev/main/API identiques avant/après. Un prévol timer hors runtime puis un refus local d'allowlist n'ont envoyé aucun appel Jev ; traces conservées. [Rapport et réponse](evidence/2026-10-30/jev-real/README.md).

**PASS audit Jev hors ligne**, après arrêt du gateway : Store réouvert, chaîne valide3événements, tête séquence3 égale au hash de réponse et contenu Jev exact retrouvé ; hash de base inchangé. Aucune seconde évaluation. Les scans littéraux clé Jev et tokens dans captures/base ne trouvent pas d'occurrence. Ni qualité générale de Jev, ni disponibilité, modèle stable ou facturation déduits de ce smoke.

## 30 octobre — nouvelle image worker gérée, build et probes

**PASS**, image `agentsandbox-worker:qa-managed-providers-20261030-ccgpe3`, ID `069ba7b703077238f22edbefc9295a11cc07e71e8dd490b8de7ab81da51ffa38`, Linux/ARM64. Build Dockerfile worker avec versions exactes Codex0.165.0/Claude2.1.140, contexte whitelist `.dockerignore`, hashes sources avant/après identiques. Versions/aides natives et présence bootstrap vérifiées sans credential/réseau ; refus sans handoff `credential_missing`, code126 attendu. [Rapport](evidence/2026-10-30/provider-auth/worker-image.md) et [log original](evidence/2026-10-30/provider-auth/software/worker-image-probe.log).

Conteneur sonde arrêté conservé, UID1000, rootfs read-only, capdropALL, no-new-privileges, lognone, mémoire/CPU/PID bornés et HOME/tmp sur tmpfs. Aucun fichier `.env`, cache OAuth ou clé hôte utilisé. Bases publiques tirées par le Docker config CLI par défaut lors du build ; inspections via config temporaire vide. npm a refusé le script postinstall Claude ; effet sur lancement réel encore NOT_RUN à ce gel. Aucun push, ancienne image/pile QA inchangée. Ce PASS image ne qualifie pas les authentifications ni un appel modèle.

## 30 octobre — Claude/API géré réel, une invocation utile

**PASS ciblé** sur image candidate `069ba7b7…ffa38` et binaire post-PRV `bb48545a…8047` figé : `providers init/status`, puis `run --client claude-code --auth-mode api --client-profile claude-api --model claude-haiku-5-5 --prompt …`, sans argv manuel. Exception Docker Desktop rootful explicite, worker observe/bridge et75s. Modèle demandé/résolu localement explicite ; modèle effectivement servi inconnu. Une invocation CLI réussie, nombre interne de requêtes HTTP fournisseur non compté ; aucun retry après arrivée au client réel. [Rapport complet](evidence/2026-10-30/provider-auth/managed-claude/real-run.md).

**Effets recoupés par root** : résultat42octets exacts sans newline, SHA `cc2b93ab193564857b7236f4d1569949d368fc55374dd6976daad6fa334fb6b0`, entrée exportée identique, résultat absent du projet original. Exit0/timedoutfalse, un fichier modifié, cleanup effectué/quota libéré, une allocation observée non facturable5923182878ns. Audit36 valide après redémarrage depuis la même base, admission4 avant claim13. Le warning npm postinstall n'a pas empêché ce batch ; aucune autre fonctionnalité d'installation qualifiée.

**Préflights FAIL conservés sans appel modèle** : premier gateway en fond mort avec sa session d'outil ; deuxième commande sans exception rootful refusée au doctor avant admission. Chacun a sa configuration privée et ses preuves conservées, aucun claim/worker rejoué. Ces échecs de préparation ne sont pas des échecs Anthropic et ne deviennent pas des appels modèles supplémentaires. Gateway final arrêté.

**Confidentialité recoupée** : scan Rust ponctuel de39fichiers (rapport, logs, projet/export), avec les trois clés fournisseur connues et les deux tokens du contrôle dédiés, zéro fichier contenant une valeur littérale. Sources des secrets lues en mémoire, valeurs jamais affichées. Ce contrôle n'inclut pas les bindings privés intentionnellement porteurs du secret et ne prouve pas l'absence de toutes transformations possibles. Aucun credential/base privé copié dans l'archive publique.

**Limites** : pas de plafond monétaire dans le preset (timeout worker seulement), pas de qualification des autres authentifications/TUI, ni médiation transitive, filtrage egress, Podman ou autre OS. Aucun gate de production entièrement fermé par ce cas.

## 30 octobre — artefacts finaux après garde Jev

Après copie privée du binaire natif testé, **PASS** `cargo fmt --all -- --check` (1s), `cargo clippy --locked --all-targets -- -D warnings` (6s), `cargo build --release --locked` (31s).89sources avant/après identiques, runner sourceSHA `d8ee798b…82ea4`, nouveau binaire de contrôleSHA `ce8e94ff4ba6437799101bf8b093b7737fb8c89ff778591b35a4f20bba655917`, bootstrap macOSSHA `cde559187593face3bc2e36950dd3eb12c5c479f43d2a22de5e54eafc5fe8e0b`. Binaire natif précédent inchangé dans sa copie protégée.

[Rapport final et logs](evidence/2026-10-30/provider-auth/final-artifacts/validation.md). Un brouillon avait comparé à tort hash source et hash exécutable ; rectification documentaire sans rerun. Tests, Docker et modèles **NOT_RUN** pour ce rebuild ; leurs résultats précédents gardent leur binaire et leur périmètre. La revue du garde et son test RED/GREEN restent leurs preuves propres.

**Documentation recoupée** : gouvernance, contrat API, quickstart/exploitation, état/gates/plan et preuves synchronisés. Revue indépendante avec corrections des promesses sur les sondes (Codex abonnement possède déjà son auth.json), du refus du seul chemin bootstrap canonique et de la distinction Jev prévu/réalisé. [Rapport](evidence/2026-10-30/provider-auth/documentation-review.md), complété par recoupe du nouveau run géré et des artefacts. Contrôle root : 283 cibles locales présentes sur32Markdown ciblés ; fragments/HTML/URLs externes non vérifiés par cet outil. Scan littéral des trois clés fournisseur connues sur548fichiers docs/sources/gouvernance, zéro occurrence ; limite aux formes littérales connues, sans preuve de DLP générale. Aucune nouvelle campagne E2E ni fournisseur pour ces vérifications documentaires.

## 30 octobre — identité avant extraction HTTP et index partagé

**RED reproduit**, `CARGO_INCREMENTAL=0 cargo test --locked --test api_auth_boundary -- --nocapture` : quatre groupes en échec, deux déjà sûrs. Toutes les combinaisons de quatre familles POST et deux alias sont exercées, avec flux de corps comptant ses polls. L'absence/invalidité d'identité et les viewers pouvaient faire lire leur corps avant refus ; JSON/query pouvaient précéder 401/403. Deux Authorization étaient acceptés par choix du premier. Un vrai client TCP, headers complets et `Content-Length: 1024` sans corps, n'obtenait pas de statut dans les deux secondes. [Log RED original](evidence/2026-10-30/api-auth/logs/red-final.log), pas une extrapolation de charge DoS.

**Correctif** : guards `FromRequestParts` en premier argument des handlers, un seul Bearer borné et index digest vers tenant/rôle construit au démarrage. Index, politique, redactor et Jev en Arc ; plus de clonage complet des credentials configurés avant auth. Aucune dépendance, route métier, migration, sérialisation/hash de décision ou élargissement de rôle. Credential configuré trop long/espace blanc refusé sans état partiellement autorisé. Alias, health publique, séparation Jev et transaction d'audit conservés.

**GREEN**, même cible : six PASS, zéro échec/ignoré. Premier statut HTTP401 sans corps envoyé, aucune lecture applicative non autorisée des flux testés, aucune mutation d'audit, identité A/B et erreurs runner préservées. [Log original](evidence/2026-10-30/api-auth/logs/green-new-target.log). Source test RED avant rustfmt `6c07398c…471c`, source finale `1817710533dbf0f36e71b2be19022053a717e26fc70426146b4d64464d0bdd80` ; pas de confusion de versions.

**45 autres PASS**, une commande sur les cibles `api_security` (4), `tenant_api` (3), `gateway_tenancy` (1), `workers_api` (6, dashboard fixe/partagé inclus), `decision_idempotence` (9), `jev_credentials` (14), `jev_contract` (2), `provider_authorization` (6). Total neuf cibles **51 PASS / 0 FAIL / 0 ignoré**, sans recomptage des processus gateways enfants. [Rapport et logs](evidence/2026-10-30/api-auth/tests.md). Suite globale, Docker/Podman, navigateur et fournisseurs réels NOT_RUN pour ce lot.

**PASS** `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets -- -D warnings` ; sources/test impliqués identiques avant/après régressions. [Revue indépendante acceptée](evidence/2026-10-30/api-auth/review.md), aucun nouveau P0/P1/P2 identifié dans le delta, sans exécution du relecteur. Baseline API exacte extraite de l'ancienne archive source, SHA `823451d1a68c91e4985237d24934bd896c10a808b40df25844bff3b8a61c0b79`, égale à l'empreinte avant RED. API finale `0d61502c45d2b25dec3c135b71495d11e024ec47b414525c48dea637ceaeceb8`, module auth `f1b1b30222ecbfb1220be5dc82191e03bf560cc4bf5e528e30033641a747ceee`.

**Build release PASS** unique en 29,85 s : Cargo.toml/lock et31sources, soit33entrées, inchangés pendant build. Binaire actuel `b7565088f8e10c05d24b5cc3b28c33bc993760b35cc3479322af9d63d72ec191` ; ancien `ce8e94ff…5917` préservé dans une copie privée. [Artefacts](evidence/2026-10-30/api-auth/artifacts.md). Aucun ancien test fournisseur/moteur transféré au nouvel exécutable ; pile QA non redéployée par ce lot.

**Limites** : la sonde TCP vérifie seulement la première ligne401 avec `Connection: close`, pas le drainage, la fermeture bornée, keep-alive ou une charge lente. Le transport/routage peut déjà avoir reçu/analysé des octets avant le guard. Lookup HashMap non constant-time, pas d'effacement mémoire revendiqué. Le verdict de l'audit figé demeure inchangé. Prochain besoin indépendant : arrêt HTTP/SSE borné ; supervision broker soumise séparément après revue de conception, aucune installation OS réalisée.

## 30 octobre — arrêt HTTP/SSE au signal

**RED initial**, `CARGO_INCREMENTAL=0 cargo test --locked --test http_shutdown -- --nocapture` : repos PASS, flux SSE actifs FAIL indépendamment pour gateway et dashboard, chacun au-delà de huit secondes après SIGTERM. Le test Rust possède chaque child et le tue/récolte après dépassement, sans toucher un service existant. [Log original](evidence/2026-10-30/http-shutdown/logs/red-final.log). Des défauts initiaux de compilation/PATH du harnais sont séparés de ce RED comportemental.

**RED renforcé avant code** : le client SSE n'a plus de timeout total pouvant provoquer sa propre fermeture et fabriquer un PASS ; connexions/readiness/opérations individuelles bornées, flux laissés ouverts plus de six secondes avec health et PIDs vivants vérifiés. Même défaut des deux services, repos toujours sûr. [Log](evidence/2026-10-30/http-shutdown/logs/red-reinforced-final.log). Pas de changement de la baseline main `dfc772e81dd26a19de7c842f834c38010894068ae10373fc21bc1282212f2822` pendant ces reproductions.

**Correctif** : helper privé au binaire, `Shared` du signal et `with_graceful_shutdown`, puis drainage cinq secondes seulement après signal. Sans signal aucun timer de vie du service. Enregistrement Unix SIGINT/SIGTERM fallible avant bind/readiness, suppression de l'expect. Ingress conserve son comportement à deux listeners et s'adapte au constructeur fallible. Après expiration, main retourne puis le runtime termine ses tâches réseau ; cela ne prétend pas qu'abandonner une future serveur quelconque termine une bibliothèque entière.

**GREEN final**, deux PASS : SSE dashboard SIGTERM→exit5007ms, gateway5010ms ; repos dashboard SIGTERM1ms et gateway SIGINT1ms. Deadline absolue depuis le signal, comparaison `elapsed <= max` également dans la branche child déjà prêt, pas uniquement timeout_at. [Log final](evidence/2026-10-30/http-shutdown/logs/green-final.log), test finalSHA `7d0a817cdfd1b405f0c2704975e05a6e5ca2b2129474504bb099b1e3317cf942`. Les versions de test précédentes restent dans le rapport ; résultats répétés non additionnés.

**Trois régressions PASS** : `cargo test --locked --test ingress_cli --test gateway_tenancy` (2 + 1), source main finale identique. Ces cibles ne sont pas relancées après les seuls renforcements du harnais HTTP. Fmt/Clippy all-targets puis Clippy ciblé du test final PASS. [Revue indépendante et addendum](evidence/2026-10-30/http-shutdown/review.md) acceptés, réserve de budget du harnais fermée. Pas de nouveau défaut production démontré.

**Artefact PASS** : dernier fmt et unique `cargo build --release --locked`,19,48s ; Cargo.toml/lock et31sources,33chemins au total, identiques avant/après. MainSHA `48d73be68241017ef27ce36fd52a4ede72920f5f61ea0c7310fce4cd196893c1`, binaire `91f0cc10b0e66c0b5633f4ca87ba22e5e527c6a5bb5cb3ffd0905abd6b2ae5a9` ; ancien b756 préservé. [Artefact et preuves](evidence/2026-10-30/http-shutdown/artifacts.md). API/auth inchangées, aucun test fournisseur ou moteur transféré, aucun redéploiement QA par ce lot.

**Limites** : événements marqueurs préparés avant SIGTERM et lus après, pas preuve d'écriture d'audit nouvelle après signal ; erreur d'installation OS du handler non injectée ; ni non-Unix, ni Docker/Podman/navigateur/modèle exécutés pour ce lot. Cinq secondes est un budget coopératif, pas une borne absolue sous sommeil/blocage IO/SQLite/starvation/teardown. Pas de qualification globale keep-alive/clients lents ni de supervision autonome des workers par ce seul arrêt HTTP.

## 30 octobre — accord d'implémentation de la supervision

Le mainteneur répond **« Implémenter la proposition (recommandé) »** pour le broker permanent et son gardien TTY Rust après soumission du design relu. BSR-01/02/03 fermés au niveau conception : intention immuable/checkpoint révisionné distincts, restauration indépendante de Drop, stop préarmé sans fsync/mutex du journal sur son chemin d'échéance. [Revue de constructibilité](evidence/2026-10-30/broker-supervision/buildability-review.md), [direction approuvée](BROKER_SUPERVISION.md). Le header/statut d'approbation est mis à jour après le gel de revue `9cc0ef1e…5f9ce`, sans prétendre à des tests du service.

**NOT_RUN** : implémentation/supervision réelle à ce jalon, installation ou charge de service OS, infrastructure cloud/PKI. Accord limité au code, configurations et tests ; aucune destruction de données existantes permise. Contrats centraux et runtime en préparation, D5/fencing et récupération du broker macOS figé encore à développer et prouver. Les correctifs HTTP livrés ne remplacent pas ces exigences.

## 30 octobre — référence réelle du crash CLI, correctif d'arrêt et contrats

**FAIL runtime reproduit**, [dossier](evidence/2026-10-30/cli-crash/README.md) : Docker29.9.2 Desktop/macOS ARM64, image immuable `069ba7…ffa38`, CLI `91f0cc10…2ae5a9`, worker `/usr/bin/sleep 20`, timeout5s, réseau none. SIGKILL du seul CLI possédé, signal9 confirmé ; même CID/nom/image encore Running après7,0019s. Premier essai non concluant à cause d'une comparaison tag/digest du harnais, puis unique retry corrigé. Deux arrêts/réconciliations ciblés ensuite : audit18 valide, quota0 et une allocation non facturable pour chaque base, conteneurs nettoyés après commit, volumes et preuves conservés. Aucun appel fournisseur/modèle. Aucun verdict de l'audit original réécrit ; G09 passe à FAIL constaté.

**Correctif logiciel d'arrêt accepté**, [preuves](evidence/2026-10-30/runtime-stop-snapshot/README.md) : inspection comparée au snapshot complet avant kill, six appels runner et branches timeout/annulation durcis. RED de deux configurations divergentes ; GREEN32 fonctions uniques (1 ciblée/six cas,14 runtime_managed,16 runner_recovery,1 runtime_terminal), zéro ignoré, revue indépendante acceptée. Pas de preuve moteur nouvelle pour ce correctif ; legacy public distinct, race administrative inspect→kill et Drop best-effort restent des limites. Aucun nouveau terminal/usage/cleanup ou quota libéré par le port lui-même.

**Contrats préparatoires vérifiés**, [dossier](evidence/2026-10-30/supervision-contract/README.md) : DTO/clock algébrique, reçus, claims et politique helper commune séparée du hash worker. Nil boot et révision claim/arm reproduits puis corrigés ; cinq tests verts et36 legacy worker_registry avec golden FixedSandboxV1. Premier Clippy FAIL large_enum_variant, correction Box sans changement JSON, puis cinq tests, fmt et Clippy all-targets strict PASS. Les36 legacy ne sont pas répétés après ce seul changement de Box. Ces résultats ne constituent pas un Storev3, une API ou un broker livré.

**Artefact final de ces deux lots PASS** : unique release build30s, binaire `ac13f27c613c557e86dd27e48eaa2468deb7b1f048e91e61a7be40819a69e753`, bibliothèque `3cd4c21827b812b5e33af6c67b30e3a78ef79ff58408f26f220eb06ea0754c40`, [rapport](evidence/2026-10-30/runtime-stop-snapshot/artifacts.md). Manifestes avant/après identiques `ca364caca5be9ec8b58caef22e369f62115d7afbe28a2f86f9e2755ba4723a7d` : Cargo +32sources Rust +six cibles de tests,40chemins. Copies privées `/tmp/ai-guard-strict-stop-qualified.yUCW0o`. Pas de nouvelle image ni déploiement QA, de navigateur, Podman ou matrice E2E globale.

Après ce gel, le Storev3 est un chantier distinct **en correction**, avec tests SQLite réels. Réservation sous authority opaque acceptée puis corrigée dans le nouveau protocole seulement ; deux RED supplémentaires établissent une nouvelle permission dans une copie pré-claim et un mismatch d'import neutralisé par un ancien succès. Revue et corrections d'activation/maintenance, de vérification sticky et du bind legacy en cours ; aucun PASS global Store, fencing ou reprise n'est attribué. Les sources courantes en développement ne sont pas assimilées à l'artefact ac13 ci-dessus.

## 30 octobre — Storev3 accepté au périmètre logiciel

Les corrections et [preuves propres au Store](evidence/2026-10-30/supervision-store/README.md) ferment les constats SVS01/02/03 au périmètre code testé : autorité neuve non sérialisée avec namespace/backing/tête initiale, watermark partagé et refus de copies/rollback ; incohérence d'import sticky et pending/rapport absent bloquants ; fresh bind legacy Created refusé, sans parcours de reprise fictif. Une régression diagnostique a été reproduite par quatre canaris (un `integrity`, trois `head`) puis corrigée ; backing/schéma restent gardés, autorité et latches ne sont pas rétablis par ces lectures. Le dernier mutex sérialise l'invalidation avec le commit, sans prétendre révoquer physiquement un permit déjà engagé en vol.

**94 fonctions uniques PASS** sur le gel pré-mutex : Store9, contrats5, worker_registry36, audit_security3, audit_integrity12, storage_security17, tenant_storage3, decision_idempotence9. Les trois enfants du test de stockage ne sont pas ajoutés au compteur. **24 de ces fonctions PASS après le mutex final**, Store9/Integrity12/Security3 ; les autres70 ne sont pas rejouées après ce seul delta ciblé. [Log94](evidence/2026-10-30/supervision-store/logs/green-94.log), [log24](evidence/2026-10-30/supervision-store/logs/green-post-mutex-24.log), [revue finale](evidence/2026-10-30/supervision-store/review.md) close. Aucun ignoré compté comme PASS.

**Artefacts après revue PASS** : fmt, Clippy strict7s et unique release build32s. Binaire `8168c30213966093dd907a1c9b290968cda45dca4a3be908341bd911998f59d0`, bibliothèque `66f607c8fae328251488305e7994fb5e618fdc68ba040f083f538b6d8b8b9e6e`. Les89empreintes Rust/Cargo/tests sont identiques avant/après, manifeste `60395e967478fc9efc1d343a86137c281d20edc38413c965100dff36822861df`. [Rapport](evidence/2026-10-30/supervision-store/artifacts.md). Copies privées `/tmp/ai-guard-supervision-store-qualified.BcHibF`, source/Cargo/tests archivée `ccb2fe0d032ec3906fe4d0eda46d021b7989fb42e0217dc51cfbc1a7addd864f`. La source et le binaire précédents restent conservés.

**NOT_RUN pour ce lot** : moteurs, E2E, images/déploiement QA, navigateur, modèles et fournisseurs. **Toujours ouverts** : D5, réactivation après redémarrage, broker permanent/gardien, API HTTP de supervision et qualification complète. Schéma2 par défaut, pas de migration CLI nouvelle ni de facturation financière. Aucun gate global clôturé par ce lot.

## 30 octobre — premier checkpoint Git distant

Demande explicite du mainteneur : commits/push réguliers des lots validés et relus, enregistrée dans AGENTS.md. Origine existante confirmée par Git et métadonnées GitHub : dépôt privé `Needdoliprane/ai-guard`, zéro référence avant push ; aucun historique réécrit. Identité du compte authentifié vérifiée, adresse de commit GitHub noreply configurée uniquement dans ce dépôt.

Pré-push : inventaire final771fichiers réguliers/7,33Mo, quinze PNG connus, zéro lien/type spécial/chemin privé ou secret suspect. Scanner des trois secrets connus exécuté en33lots, zéro correspondance/zéro échec ; heuristiques API/PAT/PEM sans hit, sans garantie DLP universelle ou OCR d'images. `.env`600 ignoré, seuls `.env.example` et les preuves assainies sont suivis ; caches/builds/bases/volumes privés exclus. Les89empreintes Rust/Cargo/tests correspondent au gel Store avant nettoyage de deux seules lignes vides finales de Cargo.toml/rust-toolchain.toml ; metadata Cargo `--locked --offline --no-deps` est strictement identique avant/après ce nettoyage. Aucun test comportemental rejoué pour cette mise en forme. Vérification de whitespace hors archives brutes PASS ; liens locaux320cibles/33Markdown sans cible absente, outil corrigé pour reconnaître les suffixes de ligne `:N`.

Commit `fa75d6ef34ee506a4d5facdc9e50fc515f79cf7b`, poussé sur `main` ; `git ls-remote origin refs/heads/main` renvoie le même SHA, tracking `origin/main` et copie de travail propre après push. Aucun tag/release, déploiement, audit global ou modèle déclenché. Les commits suivants poursuivent les chantiers ouverts ; ce checkpoint ne clôture aucun gate de production.

## 30 octobre — API de supervision et horloge native

Lot isolé dans le worktree `codex/broker-supervision`, base `e2cda5eb889b9fc8f8053085ccbaf6942792be67`. Deux GET et sept POST v1 uniquement, contrats du Store3 existant sans activation/migration CLI ni modification des plans1/2/3. Horloge Rust native avec boot retenu, Linux boottime/Darwin continuous, calculs entiers bornés, aucune heure UTC de secours. [Dossier de preuves](evidence/2026-10-30/supervision-api-clock/README.md).

**RED préparatoires conservés** : API trois groupes en échec car routes absentes ; clock E0433 car module absent. Aucun n'est une vulnérabilité runtime. Deux erreurs de harnais ultérieures (bruit libtest sur pipe enfant, baseline d'audit capturée avant une écriture de fixture) corrigées sans code applicatif. L'oracle namespace, inopérant pour les trois réponses, est également renforcé puis rejoué ; les erreurs de harnais et les exécutions successives ne sont pas comptées comme nouveaux tests produit.

**PASS ciblés exécutés par Luna** : `cargo test --locked --test broker_clock --test supervision_api` donne clock5/0FAIL/1helper ignoré et API3/0FAIL ; après ajout de la sonde Jev et correction de namespace, `cargo test --locked --test supervision_api` donne4/0FAIL/0ignoré. Le parent lance explicitement le helper clock dans un processus Rust privé ; celui-ci n'est pas un sixième test. La sonde réelle Jev voit une connexion volontaire locale puis zéro pour rejeu exact et refus frais inactif ; aucun appel à un modèle. `cargo test --locked --test api_auth_boundary --test tenant_api --test workers_api --test gateway_tenancy --test api_security` :20/0FAIL/0ignoré. Soit29fonctions uniques dans ces cibles, sans additionner les rejeux.

**PASS statiques/artefact après le dernier test** : fmt global, Clippy all-targets strict et unique release53,57s. Sources du manifeste stables ; binaire `d707f22344c1fea43bcbb90eb64cd3a4cc516903f10efecee35f500b3a7faf31`, bibliothèque `b4f7c6a28d0a7ce904995e0512749b061fb0ffca63d738b523eb0c5c8c708e21`. Revue indépendante du delta sans défaut bloquant ; portée détaillée dans le rapport. Aucun nouveau build d'image ni redéploiement QA.

**NOT_RUN** : exécution native Linux, sommeil/reprise, reboot, erreurs syscall injectées, moteur Docker/Podman/Kubernetes, navigateur et modèles. Le contrôle positif Jev s'arrête au transport loopback sans TLS/modèle ; il ne valide pas la sémantique de Jev. **Toujours ouverts** : broker permanent, journal/IPC/gardien, consommation d'échéances, reprise/réactivation et D5. Le FAIL runtime du crash CLI reste ouvert, schéma2 par défaut et facturation désactivée ; aucun gate global clôturé par ce lot.

## 30 octobre — complément horloge Linux ARM64

**PASS ciblé au gel11eb**, [dossier](evidence/2026-10-30/broker-clock-linux/README.md) : archive figée `3e1451df…1307`, nouveau contexte/image/conteneur de développement sans montage, réseau none, compilation locked/offline depuis le cache local. Linux7.0.14-linuxkit aarch64, Rust/Cargo1.99.1 ; `cargo test --locked --offline --test broker_clock` :5PASS/0FAIL/1helper ignoré invoqué par le parent, non recompté. UID1000 configuré, capacités supprimées/NNP,2CPU/2Gio/256PID, zéro montage/port, sortie0 ; conteneur et image conservés. Quatre sources recopiées du conteneur arrêté conformes au gel Git ; revue de preuve acceptée, aucune nouvelle exécution par le reviewer.

**NOT_RUN/hors portée** : hôte Linux natif, x86_64, Podman, sleep/reboot, consommateur/enforcement, D5, workers et fournisseurs. Cette preuve du producteur sous Linux en VM ne qualifie pas le code d'intention ultérieur, un déploiement ou un broker permanent. Aucun fichier utilisateur, volume, image ou conteneur préexistant supprimé.

## 30 octobre — intention immuable validée et revue corrigée

**PASS au périmètre pur**, [dossier](evidence/2026-10-30/broker-intent/README.md) : `ExecutionIntentV1`, projection du vrai RunPlan via couture privée, options fermées nouvelles sans changer les plans1/2/3 ; managed/source transitoire exclus. Domaine/hash/canonique propre, validations initiales et binding fournisseur de métadonnées, aucun accès FS/TTY/réseau/Auto. RED E0432 module absent conservé, puis5PASS initiaux. Onze canaris autorisation/contrats et un golden worker_registry PASS/35autres filtrés. Les fonctions filtrées ne sont pas comptées comme exécutées.

**BIR-01 P2 reproduit puis fermé** : dérive serde directe de snapshots publics révélant une clé ou un enum canari dans Display/Debug. Un même oracle enrichi en trois étapes révèle six sorties pour Options clé/enum et Binding clé ; UUID Binding, Request et Top étaient déjà opaques. Aucun caller métier/fuite de credential réel établi. Wrappers transparents avec données strictes privées et erreurs fixes ; règles et golden inchangés, six fonctions PASS après correction. Deux oracles de taille initialement peu discriminants sont ensuite affinés : plans/intention valides paddés et prompt contrôles non-NUL valide, erreurs exactes SourceTooLarge/IntentTooLarge. Dernière cible6PASS/0FAIL/0ignoré, testSHA `2bf54a9c45b113cb815ce03b306c3d23b13bf1fc826fe7a8a6c3330941e09bca`. Doublon top testé, unknown imbriqués testés ; aucun doublon imbriqué revendiqué. Dix-huit fonctions uniques dans les cibles, douze canaris non rejoués après le seul correctif d'opacité.

**PASS statiques/build/revue** : fmt global, Clippy all-targets strict, unique release34,83s et revue finale sans constat ouvert. Cent empreintes Rust/Cargo/toolchain/trois fixtures identiques avant/après build puis revérifiées sur le worktree ; binaire `831a3e19854e29f35dade316f71cb463c43faacf49f5f96204277b5ac85b8e1a`, bibliothèque `7edfc144d56f656bcc9980ec68dd83a42e2554b4a4b2bdce210d2fc8c1b6c280`. Copies privées `/tmp/ai-guard-intent-qualified.egJ3MF`, archive source/tests `c1d799704617736900887915facc52da1657c1eb050a62220e838c9436783c0a`. Ancien artefact préservé ; première collecte statique git-ls-files incomplète pour les non-suivis explicitement corrigée par manifestes complets aux contrôles finaux, sans prétendre une couverture rétroactive.

**NOT_RUN pour ce format** : moteur, fournisseur/modèle, E2E, navigateur, image/déploiement QA, journal/ACK et service. Le hash reste une identité d'invocation/références, pas preuve de credential réel, identité tenant ou droit de start. Journal/bindings durables, IPC frais, ordonnanceur, gardien, reprise/fencing D5 et recette A1/A2/B1 ouverts ; FAIL crash CLI non corrigé, aucun gate production clôturé.
