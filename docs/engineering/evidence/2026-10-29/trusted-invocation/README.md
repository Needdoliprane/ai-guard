# Invocation du moteur — gel261

29 octobre 2026. Ce dossier qualifie un lot ciblé, pas une release production. Le manifest `SOURCE_SHA256SUMS.txt` identifie97 fichiers applicatifs, tests et configurations ; `RELEASE_SHA256SUMS.txt` identifie le binaire hôte. Vérification des manifests PASS. Les captures d'écran ignorées par Git ne sont pas incluses dans ce manifest source.

## Résultat et périmètre

La recherche du programme dans le PATH ambiant est supprimée. Politique2 : approbation d'un programme absolu hors projet, ouverture du chemin canonique par composants sans liens, répertoire final retenu par FD, fichier lu via openat, contrôles propriétaire/modes/ACL, hash borné512MiB et lecture taille initiale+1. L'enfant exécute `./basename` après fchdir sur ce même FD, dans son répertoire de CLI et avec environnement reconstruit. Le PATH des helpers contient seulement les quatre répertoires système vérifiés.

Les nouvelles sessions enregistrent cette invocation séparément de l'identité moteur. Les anciennes liaisons restent lisibles et exigent une adoption explicite pour les effets ; policy1, client remplacé et absence d'invocation ont leurs régressions. L'adoption prouve en lecture seule le même moteur, conserve l'ancien binding privé, exige l'événement stable durable puis le checkpoint, sans modifier les claims. Une adoption en attente ne peut pas retomber sur l'ancien client après panne d'audit. Docker doctor/build/up/down figent endpoint Unix et EngineID ; un changement de contexte ne déplace pas les effets.

Les [rapports core](core-report.md), [caller](caller-report.md) et la [revue indépendante](final-review.md) détaillent changements, RED, GREEN et limites. La revue a fait corriger FIFO bloquant, reprise exigeant encore le projet d'origine, substitution de parent après validation et lien d'ancêtre à l'ouverture initiale. Elle ne relève aucun nouveau bloqueur concret dans ce périmètre gelé. Le rapport formel indépendant d'origine conserve ses19 verdicts `needs_validation`.

## Vérifications exécutées

| Commande / parcours | Résultat constaté |
|---|---|
| `cargo fmt --all -- --check` | PASS/exit0, `fmt.log` vide |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS/exit0, `clippy.log` |
| `CARGO_INCREMENTAL=0 cargo test --locked` | PASS/exit0, **261 exécutés/0FAIL/3ignorés**, `tests.log` |
| `cargo build --release --locked` | PASS/exit0, `release.log` |
| Image contrôle Linux ARM64 | PASS, `image-build.log` ; build frontend en cache, aucune nouvelle suite Node revendiquée |
| Doctor réel, profil release | PASS/exit0 ;1,48s observées, `doctor-release-{real,time}.log` |
| Confinement réel, profil debug |1PASS/124,98s, `confinement-debug-real.log` |
| Audit terminal HTTP503, profil debug |1PASS/86,95s, `terminal-debug-real.log` |
| Confinement et audit terminal, profil release |2PASS,11,04s et8,61s, `runtime-release-real.log` |
| Smoke déployé, profil debug |1PASS/449,00s, `smoke-debug-real.log` |
| Smoke déployé, profil release |1PASS/45,19s, `smoke-release-real.log` |
| Navigateur déployé partagé |10 contrôles PASS, `browser-real.log` ; Node24.22.2/Chrome156.0.8078.76 |

Les tests réels emploient `AGENTSANDBOX_TEST_RUNTIME=docker`, `AGENTSANDBOX_TEST_RUNTIME_PROGRAM=/opt/homebrew/bin/docker`, l'image worker `agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984` et l'exception `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1`. Pour le smoke : configuration privée `/tmp/ai-guard-control-qa.mF6i7C/config`, gateway `http://127.0.0.1:48787`. Les profils debug et release sont deux exécutions distinctes ; des essais se sont chevauchés sur la même pile QA. Ces durées ne constituent pas un benchmark isolé, ni une mesure de capacité100. Le surcoût debug reste visible ; aucun contrôle de sécurité n'a été retiré pour réduire la durée.

Le confinement vérifie les restrictions observées, réseau none, timeout avec broker vivant et copie/export filtrés. Le test terminal refuse l'ACK HTTP avant Store, conserve la même instance arrêtée et son quota inconnu ; ce n'est pas une panne disque. Le smoke exécute des sessions successives, vérifie l'allocation unique après reprise, le refus sans nouveau conteneur/volume et le masquage d'une longue valeur synthétique stdout/stderr. Aucun appel ni compte fournisseur réel. Ce parcours n'est pas la recette A1/A2/B1.

## Déploiement et compatibilité réels

Image active : `agentsandbox-control:qa-20261029-invocation-bd32c681`, ID `sha256:78d835562124c9d46c569c9efb715b7c35b2e44730780832ce14891a23efbb98`, Linux ARM64. Docker Desktop4.97.0/Engine29.9.2, client29.2.1, hôte macOS27.2 ARM64, Rust1.99.1. L'exception rootful est explicite, sans équivalence rootless revendiquée.

Projet QA `ai-guard-qa-7dce8f12`, Compose courant `/tmp/ai-guard-registry-qa.40IImn/compose-invocation.yaml`. Gateway/dashboard/ingress healthy sur cette image, volumes conservés, sans migration de schéma. Le premier appel Compose a été refusé avant mutation faute de variable de configuration ; son log `deploy-missing-config.log` est conservé. Le rejeu avec la variable explicite a passé. Ce déploiement utilise la CLI Docker de contrôle directement ; il ne constitue pas un essai réel complet des commandes applicatives build/up/down.

`audit-before.json` et `audit-after-deploy.json` sont identiques :347 événements et head `5024735d727aa4a4217328d8b9a9070cef3ffe754e0ac819155d35281a258af5`. Après les nouvelles écritures des essais et du navigateur : **512 événements**, head `5a13a49af8594eb38ef6c64c7e21106b174d352d66620eef869388f02078d13c`, audit valide (`audit-after-tests.json`).

Ancienne session235 `b3759203-3a1f-4c58-ac25-ca62f9b56f36` : status sans moteur PASS ; export sans adoption refusé ; adoption explicite puis deux exports neufs PASS. Worker `4fc4ffea-0c81-43ef-bf43-cede3a826574`, instance, opérations et claims identiques avant/après ; exactement un événement d'adoption et policy2 durable. Le fichier exporté est identique à l'original conservé. Preuves publiques minimales dans `legacy-*`, preuves complètes privées `/tmp/ai-guard-invocation-legacy.Cm1JLH` ; aucun checkpoint ou secret privé copié ici.

Le navigateur utilise le nouveau worker release `884223dc-2617-46b7-9717-c1e075015dab` de la session `38a82bb2-295e-4fd0-89bd-42ccf912d015`, avec allocation observée non facturable. Les événements de navigateur restent QA déclarés. Aucune nouvelle apparence n'est livrée : la référence Nothin’ relève de la gouvernance et du prochain chantier visuel.

## Écarts ouverts

NOT_RUN : Podman réel, broker Linux, autres plateformes, recette multi-tenant3workers, crash actif/reprise autonome,100workers et Kubernetes. Le build Linux de l'image ne qualifie pas son adaptateur moteur. La liaison opérateur Podman, les dépendances/plugins/helpers du compte de contrôle, les déplacements de vrais arbres de contrôle par un administrateur du namespace, sameUID/root et fencing atomique du moteur ne sont pas fermés par ce lot. Le plafond de lecture borne les octets, pas une panne de lecture sur filesystem distant.

P0 suivant : [conception filesystem](filesystem-next-design.md), snapshots de credentials/configuration et noms remis aux CLI, puis SQLite/WAL/backup. Cette conception reste non implémentée dans ce gel. La supervision autonome de durée, le relais inter-workers, la restauration sûre, les quotas disque/logs, la capacité et la publication restent obligatoires. Aucun gate production global n'est clôturé, aucune facturation activée.
