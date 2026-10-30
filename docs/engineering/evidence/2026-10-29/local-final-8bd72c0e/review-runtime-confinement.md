# Revue indépendante — test de confinement réel

Date : 2026-10-29. Périmètre : nouveau `tests/runtime_confinement.rs`, absent de la copie initiale `/tmp/ai-guard-baseline.KqN9Hi/tests/`. Lecture seule du dépôt ; aucun test moteur relancé par le réviseur, aucune ressource créée ou supprimée. Consignes AGENTS/RTK et contrats engineering relus. Skills projet absents de `.agents/` et `.claude/` : chargement automatique non confirmé, aucun skill absent prétendu appliqué.

## Verdict final

**PASS conformité et qualité pour ce lot de test, après traitement des deux constats ci-dessous. Aucun défaut P0/P1 ni défaut bloquant restant identifié.**

Les effets inspectés sont recevables pour **Docker Desktop macOS ARM64, daemon Linux ARM64, rootful explicitement autorisé**, avec l'image locale identifiée dans le rapport. Aucun résultat audit/autorisation E2E, trois workers, Podman, Kubernetes, rootless ou production globale n'en découle.

Source revue : SHA-256 `4ca3796f2f29835b3ee73c7d46c92ae848e3b94c39d135977cfed3fd84fa3e8a`. Empreinte recalculée après les corrections. `diff -q` confirme `src/runtime.rs` identique à la copie initiale ; le lot ajoute du code Rust de test uniquement.

## Constats traités

1. **P2 — faux positif dans la preuve d'absence de chemin, corrigé.** L'ancienne assertion acceptait tout exit non nul de `ls /run/secrets`. Permission refusée ou erreur I/O passait donc comme absence. Exemple de risque du harnais : un répertoire non énumérable mais traversable peut contenir un fichier lisible par son nom. Les régressions ont reproduit la logique erronée sans moteur (`/tmp/ai-guard-runtime-review-red.log`, 2 FAIL au total). Le test final utilise `ls -ld` et `require_missing_control_path` (`tests/runtime_confinement.rs:110`) exige exit 2, diagnostic ENOENT exact, non tronqué et complet. Permission/I/O et troncature sont refusées ; le cas positif est vérifié. Le libellé final porte sur **`/run/secrets` absent**, sans garantie universelle sur tous les secrets.
2. **P3 — nettoyage sauté en cas de panique dans le callback, corrigé.** Les anciennes assertions de configuration s'exécutaient dans `on_started` ; leur unwind contournait le nettoyage situé après l'await dans `Runtime::execute` (`src/runtime.rs:465`). `validate_configuration` (`tests/runtime_confinement.rs:50`) retourne maintenant `Result` avec `ensure` et `Context`, propagé directement par le callback (`:247`). Une régression vérifie l'absence d'unwind. Un contrôle négatif réel rejette volontairement une mauvaise attente de volume (`:274`) puis vérifie l'absence du seul conteneur de cette opération dans `ps --all` (`:300`). Aucun nettoyage global ou de volume n'est ajouté.

Un essai intermédiaire a échoué sur le préfixe du diagnostic GNU `ls:` au lieu de `/bin/ls:`. Le stderr observé a été conservé ; l'attendu a été corrigé précisément. Aucune autre catégorie d'erreur n'a été acceptée pour produire un vert.

## Réseau : changement d'assertion recevable

Le premier échec est conservé dans `/tmp/ai-guard-runtime-confinement.log` : le namespace contient des interfaces tunnel supplémentaires, contrairement à l'hypothèse « liste exactement lo ». Le diagnostic `/tmp/ai-guard-network-diagnosis.log` montre ces devices, aucune entrée de route IPv4 après l'en-tête et des routes IPv6 uniquement sur `lo`. Ce constat n'établit pas à lui seul qui a créé les devices.

Les contrôles finaux ne se contentent pas de retirer l'assertion : ils combinent `NetworkMode=none` inspecté, lecture des routes IPv4/IPv6 (`tests/runtime_confinement.rs:362`), et tentative HTTP littérale sans proxy ni DNS (`:386`), bornée par curl et par le runtime. Le code 7 correspond à l'échec de connexion dans la [documentation officielle curl](https://curl.se/libcurl/c/libcurl-errors.html). La documentation [Docker du driver none](https://docs.docker.com/engine/network/drivers/none/) décrit l'isolation visée ; le diagnostic de cet environnement demeure la preuve locale des effets. Aucun faux succès réseau identifié dans ce changement. Une seule tentative HTTP ne qualifie pas tous les canaux ou un filtrage bridge ; le rapport conserve cette limite.

## Preuves examinées et portée acceptée

| Statut | Preuve | Résultat constaté |
| --- | --- | --- |
| PASS | `/tmp/ai-guard-worker-resume-build.log` | Contexte 2 B ; image finale `sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6`, tag neuf décrit dans le rapport. |
| PASS | `/tmp/ai-guard-runtime-review-green-final.log` | 3 tests logiciels réussis, 0 échec ; test réel ignoré dans cette commande seulement. |
| PASS | `/tmp/ai-guard-runtime-review-real-final.log` | 1 test réel réussi, 0 échec/ignoré, 3.74 s ; daemon 29.9.2 et rootless=false consignés. |
| PASS | Configuration + effets dans le fichier relu | UID/GID 1000, CapEff/Prm/Bnd zéro, NNP 1, seccomp 2 ; root readonly inspectée et erreur d'écriture correspondante ; mounts sans bind ; chemin `/run/secrets` ENOENT. |
| PASS allocation | Configuration et lectures cgroup v2 (`:319`) | CPU, RAM et PID bornés aux valeurs attendues. Ce résultat n'est pas un test de saturation. |
| PASS | Timeout (`:417`) et copie/export (`:448`) | Sleep 60 interrompu par délai 1 s, retour <30 s ; export neuf, source intacte, mutation isolée et `.env` de fixture exclu. |
| PASS documenté | Clippy ciblé et rustfmt check | Résultats consignés dans le complément final du rapport auteur ; non relancés par le réviseur. |
| NOT_RUN | Audit/autorisation E2E, panne daemon, saturation, A1/A2/B1, quota/registre/relais/usage | Aucun succès déduit du test d'adaptateur direct. |
| BLOCKED dans le rapport | Podman et Kubernetes | Aucun résultat de cette exécution transposé à ces plateformes. |

Les commandes métier restent des programmes et arguments séparés ; les champs de format inspect sont des constantes du test. L'image passe par la validation existante. Les noms de session/opération sont générés par UUID. Les seules données écrites dans la fixture sont des marqueurs non secrets ; les preuves sont retenues par `TempDir::keep`. Les suppressions restent celles du runtime normal, ciblées sur les conteneurs d'opération ; aucun volume ni donnée préexistante supprimé par le nouveau test.

La reconnaissance exacte du diagnostic GNU en locale C.UTF-8 et l'exigence cgroup v2 limitent les prérequis du test : une autre image ou plateforme peut échouer et doit être diagnostiquée, sans transformer ce résultat en succès. Le fichier de test n'est pas une preuve que les parcours complets du produit sont livrés.

Rapport auteur et inventaire des ressources conservées : `/tmp/ai-guard-runtime-resume-report.md`, complément final inclus. Aucun blocage restant propre au lot revu.
