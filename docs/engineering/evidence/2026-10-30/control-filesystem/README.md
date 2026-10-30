# Fichiers de contrôle — gel logiciel302

30 octobre2026. Configuration, sources de credentials, snapshots et checkpoints du broker utilisent maintenant une primitive Rust commune de chemins de contrôle. L'ouverture par FD vérifie chaque parent, ownership, modes et ACL ; les montages sans sémantique d'ownership supportée sont refusés. Les snapshots ne dépendent plus de TMPDIR et leurs bytes canoniques servent au digest, au masquage et à l'injection. Les handles sont retenus jusqu'à create et le nettoyage vise uniquement les fichiers créés, relativement au répertoire détenu.

Les [rapports core](core-report.md), [caller](caller-report.md) et [revue indépendante](final-review.md) donnent les contrats et les défauts reproduits. Les quatre constats du second regard sont traités : Darwin noowners, alias de casse, ACL des sources avant publication et copie des fichiers de contrôle lorsque la configuration est dans le projet. L'exclusion d'import repose sur le FD de la configuration ; le nom réservé `credential-snapshots` protège aussi les résidus connus. Projet identique ou situé dans la racine de contrôle refusé. Ce filtre ne constitue pas une DLP sur tous les fichiers arbitraires d'un projet.

Les nouvelles archives portent une preuve privée d'exclusion version1, liée au checkpoint et à ses digests immuables. Une ancienne archive sans preuve ne peut plus préparer un premier start ; elle n'est ni assainie ni réécrite sous le même claim. L'observation, l'arrêt et le terminal après claim déjà consommé restent possibles sans second start. Une reprise avec credentials externes conserve leurs bytes autorisés même si le projet original a été déplacé ; le scan de l'archive est borné et n'écrit plus de fichiers sur l'hôte. L'export public conserve ses protections originales.

## Vérifications

| Commande / scénario | Résultat constaté |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS |
| `CARGO_INCREMENTAL=0 cargo test --locked` | **302PASS/0FAIL/3ignorés**,40suites |
| `cargo build --release --locked` | PASS,26,25s |
| Primitive filesystem macOS/APFS |15 intégrations et4 unités spécifiques exécutées PASS ; incluses dans la suite globale, pas ajoutées une seconde fois au compte |
| Archives scan/export |8 unités et15 régressions security/races PASS ; parité liens/traversées/limites/conflits file-dir et gardes d'export conservées |
| Linux ARM64 natif, dans conteneur de test UID1000 |3 unités et13 intégrations PASS, dont ACL POSIX réelle ; `linux-fs-tests.log` |
| Docker réel, release | Confinement1PASS/11,84s et refus ACK terminal1PASS/10,02s, `runtime-release-real.log` |
| Smoke sur nouveau contrôle déployé |1PASS/42,04s, [preuve commune UI/FS](../editorial-ui/smoke-release-real.log) |

Rust1.99.1, hôte macOS27.2 ARM64 ; Docker Desktop4.97.0/Engine29.9.2, client29.2.1. Rootful explicitement autorisé pour les essais moteur. Les trois tests ignorés de la suite normale ont leurs exécutions séparées ; aucun ignoré compté PASS par la commande globale.

Le test Linux copie les seules sources/tests nécessaires dans une image de développement, sous UID1000, sans monter le projet original ni un socket. Les tests tournent offline avec réseau none après téléchargement de la toolchain/dépendances. Image de test `agentsandbox-fs-tests:qa-20261030-8c19d640`, ID `sha256:beb9719c84aca478d800123cdd98ece83ef8b59dc10afbc936e988d6e7003941`. La copie de src est identique au gel hôte. `LinuxTests.Dockerfile` enregistre le harness ; le contexte complet temporaire est référencé par `/tmp/ai-guard-control-fs-linux-context.txt`. Cela qualifie ces tests filesystem sur le noyau Linux de la VM Docker, pas un broker Linux pilotant un moteur réel.

Les échecs de mise en place du harness Linux sont conservés : tests ignorés par le .dockerignore de production, proxies rustup absents du nouveau CARGO_HOME, fixtures include_str manquantes. Ils ne sont pas des résultats des tests filesystem. Le contexte corrigé et complet passe. Le lint global `items_after_test_module` a aussi été corrigé par déplacement du module de tests, sans allow ; log initial conservé.

`BACKEND_SHA256SUMS.txt` et `RELEASE_SHA256SUMS.txt` identifient le gel backend et son binaire hôte. Le manifest commun sources/front de l'[artefact déployé](../editorial-ui/README.md) est distinct. Le rendu et les nouveaux fichiers frontend ne sont pas qualifiés par les seuls tests Rust.

## Compatibilité et limites

- Les données, valeurs de clés et claims existants sont conservés ; aucun schéma SQLite ou DTO HTTP n'est modifié. Les checkpoints anciens restent lisibles grâce aux champs optionnels, avec le refus de premier start décrit ci-dessus.
- Les fichiers privés exigent UID courant, mode0600, lien unique ; racines0700 et parents non modifiables par d'autres UID, avec exception root/sticky contrôlée. Les sources0444 historiques sont migrées par init comme auparavant ; les projections0444 et leur lecteur conteneurisé distinct sont préservés. ACL inconnue ou filesystem non supporté : refus, aucune réparation/déplacement automatique.
- Les alias système Darwin `/tmp`, `/var`, `/etc` sont normalisés explicitement ; les autres liens et `..` sont refusés. Les types de filesystem admis par la politique ne sont pas tous qualifiés par ces tests.
- NOT_RUN : volume HFS/APFS réellement monté noowners, autres types Linux, lecture par plusieurs UID réels, Podman et matrice complète. Le test du masque POSIX utilise une entrée nommée réelle mais n'est pas un scénario complet multi-UID.
- Root, le même UID de contrôle et les administrateurs stockage/moteur restent hors garantie DAC. Le handoff d'un pathname à Docker/Compose n'est pas une transmission atomique de FD contre ces acteurs.
- SQLite/WAL/SHM/backup n'utilisent pas encore cette primitive : leurs courses et permissions restent le prochain P0. Supervision autonome, relais inter-workers, restauration sûre, quotas disque/logs,100workers et cloud restent obligatoires. Aucune qualification production globale ni facturation activée.
