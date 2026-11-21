# Native17 et recette locale des workers — 19 novembre 2026

## Recette de concurrence Docker

Le mainteneur a fixé la recette locale à trois à cinq vrais workers indépendants en parallèle. Le test standard de la candidate `5b3c2ed044d5` lance A1 et A2 pour A, et B1 pour B. Il prévoit de rapprocher les intervalles d’exécution Docker et les quotas actifs, puis de constater la fin normale d’A1 pendant que les deux autres travaillent. Chacun a son identifiant, son volume et son export. L’arrêt explicite, le relais métier et le parcours dashboard gardent leurs preuves propres.

| Contrôle | Statut | Observation |
|---|---|---|
| `cargo test --locked --offline -j1 --test runtime_user_journey --no-run` | PASS | Binaire compilé en 3,26 s sur macOS ARM64 avec Rust 1.99.1. |
| `real_cli_runs_three_independent_workers_across_two_tenants` | BLOCKED | Test ignoré sélectionné explicitement avec le socket Docker Desktop local et l’exception rootful. Le vrai `doctor` retourne `1 account_home_unavailable` avant inspection d’image, création de fixture ou lancement. Le test rend RC101 et `workers_started=0`. Le dossier temporaire n’a reçu aucun sous-dossier. |
| Capacité 100 workers actifs | NOT_RUN | Mesure physique à exécuter sur une infrastructure dimensionnée. La preuve de 100 réservations reste dans sa recette de registre. |

Le harnais se trouve dans `tests/runtime_user_journey.rs` de la candidate. La commande de rejeu est dans `docs/engineering/LOCAL_CLOUD_TESTS.md` de ce checkout. SHA-256 du test source `ff12693dc26b2f9f94e636729385e668cc3c4d22494409d2f8f7cfa87acb5ccc`, du libtest d’intégration `f008bbb61e2537f36bc4b9ec3de91d39af7b04363cac70dc9e4195681b56eed9` et du CLI `daed223aa89d64f725571e2dff07e2f9d026a305878ac8d96933c613c6a8479f`.

Une vérification locale indépendante confirme `geteuid() == 501` et une entrée nulle retournée par `getpwuid_r`, avant toute invocation Docker. Les trois empreintes de la recette concordent. Le rejeu des binaires existants demande une session où le système résout le compte de contrôle approuvé. Aucun worker supplémentaire n’a été lancé pour diagnostiquer ce prérequis.

### Prévols des runtimes à 08 h 55 (Paris)

| Runtime | Statut | Observation |
|---|---|---|
| Docker Desktop | BLOCKED | `getpwuid_r` renvoie 0 avec une entrée nulle pour l’UID effectif 501. Le prévol du contrôle ne peut résoudre le compte et son répertoire personnel. |
| Podman | BLOCKED | La machine `podman-machine-default` est démarrée et le socket rootful existe. `podman info` sur la connexion locale échoue avec le code 125 et une erreur E/S sur `overlay-images/images.lock`. |
| Kubernetes Desktop | BLOCKED | Le contexte `docker-desktop` répond. Aucun objet `CSIDriver` n’est déclaré. Les deux classes de stockage disponibles utilisent `rancher.io/local-path`. |

Ces sondes sont en lecture seule. Elles n’ont créé aucun worker, pod ou volume.

### Nouveau prévol des runtimes vers 13 h 48 (Paris)

| Runtime ou prérequis | Observation |
|---|---|
| Stockage Docker Desktop | Le daemon 29.9.3 Linux ARM64 répond et confirme cgroup v2. La création du réseau interne Qwen dédié est refusée sur `local-kv.db: read-only file system`. La vérification finale ne trouve ni réseau ni conteneur du projet. |
| Compte de contrôle local | La session de test ne résout toujours pas son UID effectif via `getpwuid`. |
| Podman | La VM déclare `running`. `info` sur le socket Unix local renvoie 125 et une erreur E/S du stockage. |
| Kubernetes Desktop | La sélection de contexte réussit. Le nouveau relevé CSI expire avant de rendre une réponse. Le relevé précédent à zéro pilote reste historique. |

La recette Qwen s’arrête avant le modèle et la lecture de mémoire. Les autres sondes sont en lecture seule. Les trois workers A1/A2/B1 restent à lancer dans un environnement qui passe ces prévols.

### Trois workers A1, A2 et B1 après redémarrage

Docker Desktop 29.9.3 répond sur son socket Unix local, serveur Linux ARM64 avec 8 CPU et 12 480 524 288 octets de mémoire. Le système résout de nouveau le compte de contrôle. Une lecture locale du flux Docker HTTP/1.1 `/events` filtré reçoit HTTP 200 avec encodage chunked en 6,53 ms et s’arrête à la borne courte de la sonde. La perte du broker et la continuité de l’historique du daemon restent à qualifier.

Le premier essai sur le gel `434c1400` passe le `doctor` et lance trois conteneurs d’import, qui sortent chacun avec le code 2. Sur un autre volume de recette, l’image de contrôle reproduit le refus d’écriture de `tar` dans `/workspace` en UID 1000. Le second essai emploie une image QA issue de Debian local, avec `/workspace` et le répertoire agent attribués à `1000:1000` comme dans le Dockerfile worker. L’import de vérification passe sur un volume neuf. Image SHA-256 `50e2db50151941e8e6f0b2c59cc38393c23f34473a1259a276bd2c27de96cac6`. Exception rootful explicite pour Docker Desktop et réseau coupé dans les workers.

`real_cli_runs_three_independent_workers_across_two_tenants` retourne `PASS` en 348,52 s. Les intervalles observés par Docker se chevauchent pendant 20,08 s. A1 termine normalement pendant qu’A2 et B1 restent actifs. Le harnais vérifie les trois volumes et exports distincts, la correspondance entre chaque export et sa copie, les quotas avant et après A1 et à la fin, les allocations non facturables et les deux chaînes d’audit. L’accès viewer inter-tenant au détail reçoit HTTP 404. Les trois workers sont retirés et aucun conteneur de l’image QA ne reste actif à la vérification finale.

SHA-256 du manifeste de 1 090 entrées `434c14004d91bd247dbceb8f7feb39adfa16e536146eb71553203e06b963e112`, du test Rust `8587f7635efb7e95e108f44f58c6801f1865523d9859540c12a1198dfadf009d`, du journal du rejeu `87ba09020acd0d680d5a95b3c96aa148af81cf555aae7f74d67e6ac1c029d0ee` et du résumé d’observation `4b75f4d2e3d25b65544c430572c2b4824d5c1024eb55e874d9a93532d048c373`. Les projets et bases sont synthétiques. La fin normale des trois workers, l’arrêt explicite, le relais et le dashboard sous cette charge constituent des recettes distinctes.

Le compte de la console et le propriétaire du répertoire personnel ont le même UID que le processus. La bibliothèque C résout l’UID 0, mais ne renvoie aucune entrée pour l’UID 501. Le même diagnostic échoue dans le processus Node à `uv_os_get_passwd`. La comparaison avec un Terminal de connexion du compte de contrôle reste à faire. L’origine exacte du refus dans cette session reste indéterminée.

Le prévol ci-dessous peut être rejoué depuis ce Terminal avec le même binaire déjà compilé. Le socket et l’exception rootful correspondent au profil Docker Desktop de la recette.

```sh
.worktrees/v1-assembly-20261118/target/debug/agentsandbox doctor \
  --runtime docker \
  --runtime-program /Applications/Docker.app/Contents/Resources/bin/docker \
  --runtime-endpoint "unix://${HOME}/.docker/run/docker.sock" \
  --allow-rootful-runtime
```

## Session humaine Native17

Le checkout d’intégration distinct est basé sur `e8886bd1d97664eda5971a2d7f64c8e933807816`, avec ses deltas en cours. Rust 1.99.1 compile sur macOS ARM64. Le premier check offline manque une dépendance épinglée et le fetch direct échoue au DNS. Le cache de recette est ensuite préparé dans une image Rust déjà présente sur Docker Desktop. Seuls le manifeste et son lockfile sont fournis au conteneur. La compilation est ensuite reprise offline sur le poste.

| Contrôle | Statut | Observation |
|---|---|---|
| `cargo check --locked --offline --lib -j1` | PASS | 0 erreur en 1 min 39 s. |
| `cargo check --locked --offline --all-targets -j1` | PASS | 0 erreur et 2 352 avertissements sur l’ensemble des cibles en 1 min 17 s. |
| `cargo test --locked --offline --lib -j1 --no-run` | PASS | Binaire de test produit en 3 min 29 s. 1 324 empreintes Rust et manifestes concordent avant exécution. |
| Frontend TypeScript, Vite et Node | PASS | 196 tests ciblés et build réussis sur 86 entrées vérifiées, avec Node 24.22.2 hors réseau. |
| Panneau « Nouvelle session » dans Chromium | PASS sur fixture | Le script `front/tests/browser_session_launch.mjs` vérifie permissions, catalogue vide et cible visible, refus d’une tâche hors limite, relecture de la même préparation après un 503 et unique soumission après confirmation de facturation. Chromium 148.0.7778.96 est lancé en processus unique, avec un serveur synthétique loopback. |
| Profil Native17 après migration C3 | PASS | La régression ciblée vérifie le maintien du profil installé et la fermeture de transactions après une modification DDL sur la même connexion. Elle passe de nouveau en 23,66 s sur le binaire corrigeant la garde E2. |
| Admission OIDC Native17 | FAIL | Sept tests s’arrêtent à `fresh preactivation contains publication or business history`. La garde refuse le seed de comptes effectué avant le passage Fresh. Le producteur de comptes OIDC correspondant au format 17 reste à raccorder. |
| Écritures nominatives et restauration R16 | FAIL initial | Trois tests d’administration nominative et trois tests R16 s’arrêtent sur `owned source redaction configuration differs`. Le fixture B16 et le contrôle utilisent les mêmes masques dans un ordre différent. Les correctifs de fixture normalisent les six masques et utilisent le constructeur sans egress pour les tests d’identité. |
| Rejeu des écritures nominatives et restauration R16 | FAIL | Les six tests franchissent la garde de masquage et s’arrêtent ensuite sur `human_authority_epoch_expired`. Les délais d’autorité sont conservés. |
| Configuration egress E2 | FAIL initial | Les deux cas s’arrêtent sur `native E2 requires exact17` avant leur preuve de publication ou de refus. |
| Configuration egress E2 après correction | FAIL | La garde utilise désormais le format SQLite physique. Le rejeu hérité reste au format 16 et les validations natives du profil et des événements restent exigées. Les deux tests franchissent la garde de format et s’arrêtent ensuite sur `human_authority_epoch_expired`. Toutes les cibles compilent après correction et les 1 324 empreintes de ce rejeu concordent. |
| Session réelle dans le navigateur | NOT_RUN | Le catalogue Rust est câblé. Le producteur de politique natif n’a pas encore de point d’entrée opérateur pour exposer une cible réelle. Aucun nouveau fournisseur n’a été sollicité pour ces régressions. |

Manifestes SHA-256 du gel Rust initial `0cf57cc29029c8cfb29f05eef939567d5ebc37cc368389d390f75565c5b19572`, du libtest initial `2579cd24293b73ed966902a1fcea9a1bc94b76dc6b356a00f78db61832fddd6e`, du gel frontend `1a217720cfb3eb7dc8f6f9a9feb4305766774cc2eb3a1cc8e1596490cd45f298` et du bundle JS `95f62df16180bfe08707ce4c279d06441d66c89462408b3bffde546a465ed9dd`.

Le harnais Chromium est documenté dans les guides EN/FR de `docs/setup/session-control/` du checkout Native17. Son SHA-256 est `6a89bc2d21e3e7a55bed9b71877e3783a51200f9de18819cab7d7c48052b69d9`. Le rapport Playwright de recette indique deux lectures de catalogue, une préparation, une relecture avec le même identifiant et une soumission. L’état d’exécution demeure « Non observée ». Une assertion provoquée uniquement sur une copie privée du harnais confirme le rapport identifiant l’étape d’échec, ainsi que la production de HTML assaini et PNG valides en mode `0600`. Le HTML ne contient ni bearer ni texte de la tâche de recette.

Les SHA-256 du gel Rust contenant le correctif de format et de son libtest sont respectivement `bb87d61de1adaed0f9d2c078c99362521b8866240a9aa470e51dbb5e6f7db141` et `232a10d750740657f37e13a8996255976e0941b5019d0a05b1096cc79816e4a5`. Les sélecteurs de rejeu sont `native_oidc_human17_tests::`, `native_identity_admin_provision_tests::`, `broker_maintenance_backup::tests::` et `native_e2_fixture_tests17::` dans `cargo test --locked --offline --lib -j1`, avec `--test-threads=1` passé au libtest.

Le provisionnement OIDC Native17 reste un écart produit. `human apply` accepte les formats historiques jusqu’à 13. Le chargeur Native17 sait rouvrir une publication complète, et le rejeu d’administration sait vérifier les mutations nominatives. Le point d’entrée qui crée dans la même transaction un compte lié à l’issuer et son appartenance reste à raccorder. Sur une source portant déjà des événements causaux OIDC, la sauvegarde devra aussi vérifier son historique Auth5 retenu.

La revue source confirme que le fixture OIDC écrit le compte et l’appartenance au format 13 avant Fresh. Déplacer ces écritures au format 16 rencontrerait la garde de Repin. La recette à compléter publie d’abord A17 et conserve B16 et Auth5. Elle doit obtenir un refus durable pour un sujet non provisionné, puis utiliser l’administration de la source A17 pour inscrire le compte `(iss, sub)` et l’appartenance. Les deux mutations doivent partager la même transaction et conserver leurs reçus au rejeu. Un nouveau JWT de recette signé doit ensuite obtenir `201` et un seul contexte A borné à `ReadMetadata`. La preuve attendue inclut l’absence de mutation de B16. Elle concerne le rapprochement compte et jeton. Les parcours PKCE des interfaces gardent leurs recettes propres.

## Écriture d’incarnation sous échéance

Les six échecs d’identité et de sauvegarde mesuraient entre 2 042 et 2 213 ms à `incarnation_append17` sur l’arbre précédent. La tranche suivante retire le rejeu médian dans la transaction SQLite d’écriture et la répétition du même validateur dans la relation de configuration. Les contrôles de publication et de connexion, le CAS, l’échéance d’autorité, les validations autour du commit et la rétention séparée restent exigés. Le libtest compile en 2 min 32 s avec Rust 1.99.1, et les 1 324 empreintes concordent. SHA-256 du manifeste `961600b9b290ce4088389eea27c0c680518e1c4d6493161765c76f18fd528bfe` et du binaire `0300e2513efa9328b2b64b086a4ad93c201ba72e45bee7e4e559a2873dfa3b9c`.

Les cinq tests `writer_tests17::` passent sur fichiers SQLite réels, avec un seul test à la fois et réseau interdit. Ils vérifient le mauvais CAS sans mutation, la réconciliation exacte et la rétention séparée, le rollback sur expiration et dérive de projection avant commit, puis la conservation du reçu et des ressources après un refus post-commit.

La régression C3 passe également. Les trois cas d’identité, les trois cas de sauvegarde R16 et les deux cas egress E2 échouent encore sur `human_authority_epoch_expired` dans ce premier gel. Pour les trois cas d’identité, `incarnation_append17` dure de 2 016 à 2 048 ms.

Le second gel regroupe la validation de publication sur le même instantané d’écriture avant l’append et avant le commit. Ses trois contrôles de rollback et de rétention post-commit passent. Le nominal d’identité atteint la préparation de fichiers et révèle une utilisation du domaine d’amorçage des archives pour `credential` et `plan.json`. Le nominal de sauvegarde atteint son inspection dans le vrai service, puis son autorité expire. Les deux cas E2 expirent encore. Le manifeste Rust de ce rejeu a pour SHA-256 `44b2f9eb0dfd35b1a83179d9cfcce08d3c14c2c31030aeb02a85fef7584b3a24`, le libtest `dd6376ed04b331d78b2bea91995c457c1f9a6c27cf84c344f926bf3f501bd9fe`.

## Fichiers d’identité locale

Le domaine `LocalIdentity` utilise les deux noms fixés par le producteur. Les fichiers `credential` et `plan.json` sont bornés à 70 octets et 32 Kio. La création reprend le mécanisme exclusif sur descripteur retenu. Le test des refus croisés et des tailles passe. Le test injectant une interruption après l’écriture du credential passe en 36,99 s et observe le même inode, les mêmes identifiants, le même condensat et le plan revalidé. Le chemin nominal passe la préparation et rencontre encore `human_authority_epoch_expired`. Le build de ce gel prend 2 min 30 s. Les 1 324 empreintes concordent. SHA-256 du manifeste `9cc4adcc107e41381486093a80f0dc609051f0aae854a210cd349ee38dd786f0` et du libtest `e8f2e12f7df667acbec24b4e54e3f24d38333fcd2d5b95beb2ed37a21a6429ea`.

Les deux sondes instrumentées localisent `human_authority_epoch_expired` à l’authentification de l’administrateur de sauvegarde R16 et à la lecture de la genèse native dans le montage E2. Le rejeu rend 2 PASS et 3 FAIL, dont le nominal d’identité. Le code d’installation de `control.public.json` utilise encore le domaine Bootstrap. Ce point est issu d’une revue source et son exécution reste à vérifier séparément.

## Consigne dans Chromium

Le script `front/tests/browser_session_message.mjs` passe avec Chromium 148.0.7778.96 et le même bundle que la recette de lancement. Il utilise une fixture HTTP synthétique sur loopback. L’utilisateur voit la taille maximale, la destination, une seule soumission après double activation, puis trois lectures du même identifiant d’opération après un 503. L’interface distingue file d’attente, transmission sans acquittement et livraison inconnue. Le test vérifie l’acquittement protocolaire dans l’historique, la confirmation exigée avant une nouvelle consigne et le retrait du contenu visible après perte des droits. L’effet agent reste non observé dans cette recette. Le guide Native17 EN/FR indique la commande de rejeu. SHA-256 du script `4be9be0bd6965b9112460b0f95595a85b65d2cc97a101c3f2c500e2dde1688fe` et du rapport `796d11217a01f9c605017e2e7c50721719c86588d3c0a1b9e11bbeb68e6c6184`.

## Fichier de contrôle public Native17

Le producteur Rust écrit `control.public.json` dans un domaine à nom fixe, avec une limite de 1 Mio et le mécanisme exclusif à descripteur retenu. Les deux tests `setup::native_profile17::public_control::tests::` passent sur le vrai système de fichiers. Ils vérifient le refus d’écrasement, la reprise du même inode après une interruption post-écriture, le mode `0600`, les octets et leur SHA-256, puis le refus d’un fichier d’origine dont le contenu a changé. La revue source confirme l’absence de point d’entrée CLI vers la publication Fresh/Repin Native17. Les tests qualifient le producteur de fichier.

Le libtest du lot compile en 2 min 30 s et les 1 324 empreintes concordent. SHA-256 du manifeste Rust `869922d127077c0ae4358cc1507bf7130dd7cb134c16a3eef0137b0568a2bec3` et du libtest `98e2c3dbc54c0d26fcdd604a03b404300619a98a27ec4fff5915ef879fe0be49`.

## Configuration egress et diagnostics de credential

Le montage de recette E2 emploie une époque courte par opération, comme le gateway pour les deux installations. Le profil est réévalué avant la lecture de sa genèse et le commit. Le test à deux emplacements disponibles passe en 82,29 s. Le vrai prévol refuse la préparation, le journal et le quota d’audit conservent leurs valeurs, le répertoire N6 reste vide et aucune observation ou opération n’est publiée. Le scénario positif atteint la limite de 150 s du harnais. Son résultat reste indéterminé lors de ce premier essai. Cette limite concerne le processus de test. L’échéance d’autorité reste de deux secondes par opération.

L’authentification R16 expire encore. Les contextes internes du Store sont absents de la chaîne de cet échec. Le test d’identité nominale indique maintenant l’expiration dans `provision local identity`, après réussite de la sauvegarde et du lien de préparation. Les deux tests du fichier public et le cas de refus E2 passent sur ce même gel.

Le gel suivant réutilise le manifeste lu et vérifié par le contrôle Complete, aux deux endroits où les mêmes vérifications étaient contiguës. Le libtest compile en 2 min 27 s et les 1 324 empreintes concordent. SHA-256 du manifeste `051640d4e31d938ca4ab3c501c8fcbc7fcb16a11a1f43cf4259b771b1985255a` et du libtest `8664bfef550723079a93649078a64d1690f12f59122817395f2d311ee2720ee2`.

| Rejeu sur ce gel | Statut | Observation |
|---|---|---|
| Sauvegarde R16 | FAIL en 48,26 s | Le Store termine la vérification du credential. Le contrôle final dans le registre renvoie `human_authority_epoch_expired`. |
| Identité locale nominale | FAIL en 52,38 s | L’autorité expire dans le provisionnement, avant le passage à l’état de tentative de commit. |
| Configuration E2 positive | FAIL en 188,49 s | Avec une limite de test de 300 s, le test renvoie `human_authority_epoch_expired`. Deux échantillons locaux du processus observent `finalize_histories17`, après le commit de la paire et ses assertions, puis `install_rebound_native_egress_profile17` dans le nouveau profil. Cette dernière partie de la recette enchaîne encore le recensement, les deux installations et la lecture finale sous une seule époque. La sous-phase exacte du refus n’est pas instrumentée. |

## Rejeu positif de configuration E2

Le scénario positif passe en 198,11 s avec des epochs distinctes pour les quatre opérations de lecture et d’installation. Le test vérifie le commit contigu des deux événements, la finalisation des historiques, la publication de la nouvelle configuration et le démarrage de la source sur son nouveau lien. Les empreintes du profil egress et du fichier canonique concordent entre les deux installations. Le test vérifie aussi la nouvelle incarnation, le Store voisin au format 16 et l’installation finale dans le gateway Rust. Ce gel exerce SQLite et des fichiers privés de recette.

Le dernier contrôle de l’authentification R16 reste en échec à `check native credential epoch after transaction scope`, après restauration du timeout SQLite. La trace d’identité nominale situe le refus à `capture local identity Auth5 history`, avant l’ouverture de la transaction tenant. Build en 2 min 24 s et 1 324 empreintes concordantes. SHA-256 du manifeste `54a64c558a1ae876c5ccba895b7e0b870a0b5992e59a53dd6a72be9b3b770920` et du libtest `791b57f7a9e0c9caa07d8b93ee1441f0c03f94150561dabc6df9099dac70af58`.

## Recensement, Auth5 indisponible et admission de sauvegarde

Le Store termine l’authentification par la vérification des fichiers du candidat puis par le contrôle de l’époque d’origine, après restauration du timeout SQLite. Les vérifications du grant et les contrôles avant et après commit restent en place. Le recensement continue de comparer les générations de tous les Stores. La lecture Auth5 s’applique au format exact 17, qui porte les faits causaux natifs. Le Store vérifie le format physique, l’identité, l’incarnation et la liaison du voisin au format 16.

| Rejeu | Statut | Observation |
|---|---|---|
| Refus E2 de capacité | PASS en 80,36 s | Deux emplacements disponibles. Journal et quota inchangés, répertoire d’opération vide. |
| Provisionnement avec Auth5 indisponible | PASS en 51,94 s | Le déplacement en recette du fichier Auth5 réel entraîne `OwnerInvalid` avant toute tentative de commit. Même source, même inode du fichier de credential préparé, mêmes identifiants et même état du Store tenant. |
| Identité locale nominale | FAIL en 52,25 s | La capture Auth5 aboutit et la transaction tenant s’ouvre. Le contrôle d’autorité native expire avant les mutations. |
| Sauvegarde R16 | FAIL en 61,26 s | L’authentification, la copie neuve et son contrôle aboutissent. L’admission de la sauvegarde existante retourne sa valeur, puis le contrôle de la même époque refuse avant synchronisation. Le fixture masque encore la cause typée de ce dernier refus. |

Build en 2 min 26 s. Les 1 324 empreintes concordent. SHA-256 du manifeste `e59a279340c8f522cdf3a97c93fc4eef599d1b9ca037eaef05be0ad071ceb718` et du libtest `c29245e5bfde2d9fd0812bad1aa0340c8802b2453dcb8d539883f97dc0328e0e`.

## Contrôle d’autorité d’identité et admission R16

Le gel suivant utilise une seule validation du contexte natif et de la classe de l’époque à l’entrée du chemin d’administration d’identité. Les autres contrôles conservent leur portée et la borne de deux secondes reste en vigueur.

| Rejeu | Statut | Observation |
|---|---|---|
| Époque métier utilisée pour l’administration | PASS en 22,27 s | Une époque `Business` ne peut pas construire une source d’administration d’identité. |
| Identité locale nominale | FAIL en 54,70 s | Le contrôle d’autorité dans la transaction tenant renvoie `human_authority_epoch_expired` avant mutation. |
| Sauvegarde R16 | FAIL en 55,21 s | Le contrôle qui suit l’admission de la sauvegarde neuve refuse. Le fixture de ce gel masque la cause typée à ce point. |

Build en 2 min 29 s. Les 1 324 empreintes concordent. SHA-256 du manifeste `e317b42c3d6f57a01866186535793fd3fde19d891ae20b124f62a2cdb71d95c6` et du libtest `4ec1a06d2fae36e250b96b42626c2418582e15ff1f63a075119ecadabd080fb5`. Le fixture a ensuite été modifié pour conserver la cause typée à la sortie du contrôle de la sauvegarde neuve.

Le rejeu de ce dernier gel échoue en 49,13 s au contrôle final de l’authentification nominative de l’administrateur R16, après sortie de la transaction SQLite. La cause typée est `human_authority_epoch_expired`. Ce rejeu s’arrête avant la phase d’admission de la sauvegarde et laisse le refus précédemment observé à cette phase à confirmer. Build en 2 min 26 s, 1 324 empreintes concordantes. SHA-256 du manifeste `f30d581ac5256e1f3c919aba981bda28c58cc760e83ed7da6c8efd7929697dee` et du libtest `2c7cfd6521615582b4aa8418d9936519c797e39b2ac97cac5063c775bd415531`.

## Relectures sous la même époque

La relation MetadataRepin garde l’admission de chaque jeton, la vérification de la publication, de ses pièces et de chaque descripteur, puis la vérification finale du propriétaire. Elle réutilise le contrôle complet que `source_token` effectue déjà avant de rendre le jeton. Dans le recensement d’identité, Auth5 en cours d’exécution et le Store valident les accès avant de prendre leurs mutex SQLite. Le constructeur validant reste utilisé pour les sources d’historique de publication, dont la lecture commence par la revalidation du journal. La revue indépendante a vérifié cet ordre. La borne de deux secondes reste en vigueur.

| Test exact | Statut | Dernier point observé |
|---|---|---|
| `complete_scans_and_copy_run_off_gate_then_exact_admission_refuses_foreign_source_and_backup_drift` | FAIL en 56,33 s | L’authentification aboutit. Le contrôle après admission de la sauvegarde neuve renvoie `human_authority_epoch_expired` avant la copie. |
| `native_local_admin_membership_mutation_audits_launch_send_and_revokes_its_old_scope` | FAIL en 55,39 s | `open local identity checked transaction` renvoie `human_authority_epoch_expired` avant toute mutation d’identité. |
| `real_mixed_native_publication_reaches_gateway_and_rejects_competing_histories` | FAIL en 26,76 s | L’activation du gateway demande une configuration egress retenue par Setup. Le test remonte `native_egress_profile_requires_retained_setup`. |

Rejeu séquentiel sur macOS ARM64 avec des fichiers et bases SQLite de recette, dans des processus sans réseau. Build en 2 min 30 s et 1 324 empreintes concordantes. SHA-256 du manifeste `a556bd5988b04e08735647309c4eb6a7277c0fc4e2d053077a68832435ece691` et du libtest `65cc68e74f0c9734ec8ea79cf50f3898c78d126c98f4a72650ba53d827c2f8f0`. Une compilation précédente a été écartée après divergence d’une empreinte, avant exécution de ses tests.

## Démarrage mixte sans profil egress

Le binaire précédent `2c7cfd6521615582b4aa8418d9936519c797e39b2ac97cac5063c775bd415531` reproduit `native_egress_profile_requires_retained_setup` en 25,30 s sur le même test. Quatre scénarios de publication mixte transmettaient le profil synthétique au constructeur Native17 sans fournir de Setup retenu. Ces scénarios vérifient les historiques, les sources et l’installation du contrôle. Leur fixture utilise désormais la configuration de démarrage sans egress enregistré. La garde de production continue d’exiger un Setup retenu lorsqu’un profil egress est demandé. Les essais egress E2 font l’objet de leurs propres preuves sur un profil réellement installé.

| Test exact | Statut | Observation |
|---|---|---|
| `real_mixed_native_publication_reaches_gateway_and_rejects_competing_histories` | PASS en 27,86 s | L’historique concurrent donne un conflit. La publication mixte installe les deux sources avec leurs propriétaires d’origine. |
| `actual_same_parent_group_reaches_native17_16_complete_and_gateway` | PASS en 27,45 s | Les deux bases partagent un parent réel, avec inodes distincts. La reprise C3 et l’installation dans le gateway aboutissent. |
| `actual_recorded_cold_parts_reach_current_owner_gateway_without_fresh_origin` | FAIL en 32,74 s | `cold_materialization` du Store 17 dure 2 126 ms et renvoie `human_authority_epoch_expired`, avec les ressources retenues. |
| `actual_recorded_files_reach_the_same_native_c3_gateway_union` | FAIL en 30,33 s | Le chargeur atteint la reprise C3 et retourne `source17_native_c3_resume_required`. Le fixture de ce gel n’affiche que le code de phase. |

Build en 2 min 34 s. Les 1 324 empreintes concordent. SHA-256 du manifeste `f6ace41b83ae2c5b1eefa35a0e452484f9cc0eebda81290f4e73c94d96a9ffd0` et du libtest `bfd2ea6bb1486a0557920eeef38fcbcabd78b42b228e6510ebdc0b958122b566`. Rejouer depuis le checkout Native17 avec `cargo test --lib --locked --offline -j1 NOM_DU_TEST -- --test-threads=1`, en remplaçant `NOM_DU_TEST` par un nom du tableau. Les tests travaillent avec les vrais lecteurs et bases SQLite de recette, hors réseau.

Le rejeu du chargeur avec la chaîne d’erreur conservée confirme la même cause que la reprise à froid. Après 2 092 ms dans `cold_materialization` du format 17, l’époque expire et le rejet conserve les ressources natives. Le test échoue en 30,25 s. La sortie de diagnostic concerne le fixture et la CLI publique garde son code de phase. Build en 2 min 31 s et 1 324 empreintes concordantes. SHA-256 du manifeste `af321530bc7c302bfc0e883cd5c88e2c70f35833355616102a644b0e891f9459` et du libtest `60cd9b86dc7cdf5df4fe6f714b5c7372261a6a5f593bcc79ba637886b7c512c5`.

## Reprise à froid et rétention au refus

Le validateur du format physique 17 inclut le replay Control commun. La matérialisation à froid appelle ce validateur une fois par observation. Les formats antérieurs passent par le validateur commun. Les deux observations encadrant le même snapshot SQLite restent exigées. La revue indépendante a confirmé les contrôles de fichier, de chaîne, de transaction et de rétention. Pour une base 17 invalide, le premier motif de refus peut venir du validateur natif.

| Test exact | Statut | Observation |
|---|---|---|
| `actual_recorded_cold_parts_reach_current_owner_gateway_without_fresh_origin` | FAIL en 33,98 s | `cold_materialization` du format 17 expire après 2 296 ms. |
| `actual_recorded_files_reach_the_same_native_c3_gateway_union` | FAIL en 30,04 s | Le chargeur rend le même défaut d’époque pendant `cold_materialization` du format 16, après 2 178 ms. |
| `cold_materialization_split_keeps_native_owner_and_original_typed_cause` | PASS en 32,47 s | Sur un refus injecté dans la recette, la cause typée et les ressources natives d’origine sont conservées jusqu’à libération du rejet. |

Les résultats localisent l’expiration sur les deux formats et ne démontrent aucun gain de temps. Build en 2 min 30 s, 1 324 empreintes concordantes. SHA-256 du manifeste `5fcb209aa1c21a5766ca4f437bd9a96e6b82ad13a4e75fa590224f515438196e` et du libtest `5fcc064de83b00babc1583e621dd1fb0dde724848a388d26eaa5519420c9077a`. Les trois tests s’exécutent avec la même commande Cargo que le tableau précédent.

## Observations SQLite du démarrage à froid

Un échantillonnage exploratoire du chargeur sur le libtest `5fcc064d…` a obtenu 72 piles pendant la matérialisation. 32 passent par le transfert de la connexion et 39 par la préparation du Store. Le scénario expire en format 16 après 2 038 ms dans cette phase. Le profileur s’attache au processus de recette et ces chiffres servent à localiser le travail.

Le gel suivant consolide la validation complète sous le même snapshot SQLite. Les contrôles de la connexion, du backing, de l’ancre, du flock et de l’époque encadrent toujours l’observation de la publication. La même transaction conserve ses contrôles de `query_only`, `data_version`, `total_changes`, de la tête d’audit et de sa clôture native. Les formats antérieurs gardent le replay Control. Le format 17 l’exécute via son validateur complet.

| Rejeu | Statut | Mesure en format 16 |
|---|---|---|
| Reprise directe à froid | FAIL en 30,86 s | Connexion 490 ms, préparation 1 232 ms, validation de publication interrompue après 377 ms par expiration de l’époque. |
| Chargeur depuis les fichiers | FAIL en 31,45 s | Connexion 557 ms, préparation 1 395 ms, validation de publication interrompue après 236 ms par expiration de l’époque. |
| Refus par expiration et rétention d’origine | PASS en 31,76 s | Le rejet conserve la cause typée et les ressources d’origine. |

Build en 2 min 31 s et 1 324 empreintes concordantes. SHA-256 du manifeste `70ba78c282a530997351809082a226e8d47dbb736884c51d313c17ae75370c35` et du libtest `fd29ca731188794aa3aaea9eeade8fde8a03c0e77f4d19c65a8926db0219f32b`.

## Préparation fermée et validation de publication

La préparation restitue la connexion d’origine en autocommit et `query_only=1`, avec le même backing, le même flock d’initialisation et la même publication retenue. La validation de publication ouvre sa propre époque de deux secondes. Elle compare la tête d’audit, les versions de données et de schéma, `total_changes` et les métadonnées physiques de `main`, du WAL et du journal. Elle réexécute la validation de la publication avant de rouvrir les écritures pour l’entrée privée de C3. Un refus referme les écritures et conserve la cause, la connexion, le flock et le propriétaire. Le Store entre dans le gateway après le reste du démarrage C3.

| Rejeu | Statut | Observation sur le premier gel de séparation |
|---|---|---|
| Reprise directe à froid | FAIL en 31,88 s | Le format 16 expire pendant la vérification après la transaction de lecture de préparation. |
| Chargeur depuis les fichiers | FAIL en 43,91 s | C3 termine et le chargeur fournit ses Stores. Le gateway refuse ensuite `owned source redaction configuration differs`. |
| Dérive du Store entre préparation et publication | PASS en 37,51 s | Un second descripteur SQLite modifie l’`application_id` du fichier de recette. Le refus conserve la connexion fermée aux écritures et le flock, libéré à l’abandon du rejet. |
| Refus par expiration et rétention d’origine | PASS en 30,24 s | La cause et le propriétaire d’origine restent retenus. |

Build en 2 min 30 s. Les 1 324 empreintes concordent. SHA-256 du manifeste `3f3cfceda032529b096a0769a9889e731bd6f329a62f48e9bb5a121b66fcbdba` et du libtest `f84c1fed312f9e2abcce6421a820fd0cc370b62242b78a1c1bb348af1277c8cb`.

Le diagnostic de masquage situe le défaut au chargeur de production. Il conservait l’ordre de collecte des masques privés, alors que la couverture exige les masques longs en premier. Le Store 16 utilisait cet ordre tel quel. Le correctif réutilise l’union canonique de `Redactor` lors du chargement, avant l’ouverture des Stores. Le gateway de recette garde sa comparaison indépendante. Les vérifications qui encadrent les seuls réglages locaux de connexion sont également recentrées sur l’epoch, le verrou et la connexion. Le replay complet a lieu avant et après la construction des projections sous le même snapshot, puis après la clôture.

### Rejeu après correction du chargeur

| Test exact | Statut | Observation |
|---|---|---|
| `actual_recorded_cold_parts_reach_current_owner_gateway_without_fresh_origin` | PASS en 53,60 s | Reprise à froid, incarnation retenue, Store voisin au format 16 et installation réelle du gateway. |
| `actual_recorded_files_reach_the_same_native_c3_gateway_union` | PASS en 49,62 s | Le chargeur part des fichiers privés enregistrés. Le gateway accepte la configuration de masquage et retrouve la même union A17/B16. |
| `cold_prepared_store_refuses_drift_and_keeps_initialization_guard` | PASS en 36,68 s | La dérive SQLite refuse la promotion, conserve le flock et maintient `query_only=1`. |
| `actual_commit_before_complete_with_unserved_authentication5_recovers_c4_gateway` | PASS en 98,36 s | Après commit et perte de tous les propriétaires d’origine, la reprise retrouve Auth5, permet d’inscrire et de relire la session de recette et installe le gateway. |
| `cold_materialization_split_keeps_native_owner_and_original_typed_cause` | PASS en 32,53 s | Une époque expirée refuse la préparation. La cause exacte, la base et le propriétaire d’origine sont conservés jusqu’à abandon du rejet. |

Build en 2 min 35 s, 1 324 empreintes concordantes. SHA-256 du manifeste `eca030ca4200293641253031e64aa5d1f58cfec1b98825abcb41ddab030cbbc1` et du libtest `3b3507851465a8d5cead0d160b99def68c55556fe8eeb63216bc3cabe52d95a8`. Les temps indiquent la recette complète, incluant la création des données, la migration et la reprise. Chaque phase d’autorité vérifie séparément sa limite de deux secondes.

Rejouer les cinq tests depuis le checkout Native17 en utilisant chaque nom du tableau avec `cargo test --lib --locked --offline -j1 NOM_DU_TEST -- --test-threads=1`. L’exécution de preuve met chaque libtest dans un dossier privé avec le réseau bloqué. Les journaux SQLite, fichiers privés, connexions et propriétaires sont réels. La session Auth5 utilise des données synthétiques et une API de recette.

## Rejeu de l’admission R16 et de l’administration locale

Le même libtest `3b3507851465a8d5cead0d160b99def68c55556fe8eeb63216bc3cabe52d95a8` échoue en 59,39 s sur R16 au contrôle qui suit l’admission de la sauvegarde existante. La sauvegarde neuve a franchi son propre contrôle. Le scénario de création nominative échoue en 55,69 s au contrôle d’autorité avant mutation. Les deux refus sont `human_authority_epoch_expired`.

La restauration R16 réutilise la vérification du backing déjà effectuée par le contrôle de la connexion SQLite. Le plan et la racine sont revalidés avant le contrôle du grant et après le retour en mode SQLite d’origine. Le contrôle de l’époque entre le retour d’admission et la copie garde sa propre frontière. Le contrôle d’accès d’un Store réutilise l’observation des fichiers de propriétaire qui a déjà eu lieu à l’entrée. Le contrôle du backing, le replay de l’autorité, le latch sur divergence et les contrôles de sortie restent exercés.

| Rejeu exact | Statut | Observation |
|---|---|---|
| `complete_scans_and_copy_run_off_gate_then_exact_admission_refuses_foreign_source_and_backup_drift` | PASS en 60,40 s | Sauvegarde neuve, reprise de la sauvegarde existante, refus d’une source différente et d’une dérive physique. |
| `actual_control_drift_retains_opening_complete_nonadmitted_and_postadmission_snapshots` | PASS en 149,85 s | Le test couvre les refus lors de l’ouverture, après préparation complète et après admission, avant copie. Les ressources d’origine restent retenues. |
| `native_local_admin_membership_mutation_audits_launch_send_and_revokes_its_old_scope` | FAIL en 53,50 s | La transaction franchit l’entrée d’autorité. La validation du grant d’administrateur expire avant mutation. |
| `actual_a17_b16_auth5_oidc_admission_keeps_native_personal_ceiling_and_original_context17` | FAIL en 58,55 s | Le fixture publie d’abord A17/B16/Auth5. Le routeur refuse un sujet inconnu et le writer refuse un issuer différent. L’application du plan nominal expire avant commit. |
| `real_native_business_epoch_cannot_construct_an_identity_admin_source` | PASS en 23,42 s | Le contrôle d’une transaction métier refuse l’autorité d’administration. |
| `actual_retained_backing_replacement_keeps_the_permanent_failure_path` | PASS en 0,38 s | Le remplacement réel du fichier de recette conserve le refus permanent. |
| `identical_copy_transaction_refuses_without_mutating_or_poisoning_the_source` | PASS en 0,38 s | La connexion d’une autre copie est refusée. Le Store d’origine reste utilisable. |

Build en 2 min 29 s. Les 1 325 empreintes concordent. SHA-256 du manifeste `a4c57ed231ede1438ea6eec33bd3e5df4553435ba2d9c9cd73f8b72e2c12eba2` et du libtest `13313f544d1317d5371c1fe2270f57ef343d24c470e2230210606eeb701fcc0e`.

Le port Rust OIDC inscrit le compte et l’appartenance à partir du grant InstallationAdmin et de l’issuer épinglé, dans une seule transaction. Il demande une vraie sauvegarde et un historique Auth5. Le rejeu reste lié aux deux identifiants du plan. Le point d’entrée opérateur Native17 et le passage effectif du scénario nominal restent ouverts.

Les prévols locaux à 10 h 10 (Paris) retrouvent les mêmes blocages matériels. Le lookup POSIX ne résout pas le compte courant pour Docker, Podman retourne une erreur E/S sur `overlay-images/images.lock` et `docker-desktop` ne déclare aucun pilote CSI. Les sondes sont en lecture seule et la recette à trois workers reste bloquée.

### Diagnostic des transactions d’identité Native17

Le gel suivant conserve la validation après acquisition du verrou de supervision, avant de consulter la suspension ou de publier des caches. Il retire sa répétition à la première instruction de la reprise. Les vérifications en transaction et les refus legacy restent exercés. Le test Business utilise aussi une vraie WriteEpoch avec le bon jeton et vérifie que le contrôle de classe interdit la construction d’une autorité d’administration.

| Rejeu | Statut | Observation |
|---|---|---|
| Identité locale | FAIL en 52,30 s | Le recensement termine en 1 297 ms. La validation du grant refuse après 705 ms dans le writer, sous la même époque et avant mutation. |
| Provisionnement OIDC | FAIL en 58,36 s | Les deux appends ont lieu dans la transaction. La revérification des fichiers OIDC épinglés expire avant l’appel au commit. |
| Refus d’administration en classe Business | PASS en 23,96 s | Refus pour une ReadEpoch et pour une WriteEpoch de classe Business. La tête d’audit reste identique. |
| Format legacy au début de la reprise et avant commit | PASS, 2 tests | Le refus du format précède la récupération suspendue et la publication des caches. Le refus au commit ne change pas le plancher d’audit. |

Build en 2 min 34 s, 1 325 empreintes concordantes. SHA-256 du manifeste `7488ef5620421e038eca02580d62bb07134596812725aef92de31ab4f3bdacad` et du libtest `fee46974f93fd07eececf213b40c7b3ca3fa49635c94df2e9170a45cae0bf36a`. Les sélecteurs exacts d’identité et de classe Business figurent au tableau précédent. Les deux contrôles legacy sont `legacy_header_refusal_precedes_suspended_recovery_and_cache_publication` et `legacy_commit_rechecks_format_and_rolls_back_without_advancing_floor`. Ils s’exécutent avec la même commande Cargo. Les deux parcours nominaux d’identité restent ouverts.

### Recensement d’identité et contrôle avant mutation

Le plan conserve une observation de chaque Store prise dans le recensement natif. L’observation comprend la connexion retenue, la tête d’audit, les versions SQLite, le nombre de changements et les métadonnées des fichiers. Une nouvelle époque compare chaque Store à son observation dans une lecture SQLite, puis contrôle à nouveau la connexion à la clôture. L’index et les fichiers du plan restent liés au même propriétaire. Le writer capture de nouveau Auth5 et utilise sa propre autorisation. Les observations décrivent le recensement, sans transférer de droit d’écriture.

Le port OIDC revérifie le grant exact à partir du replay de la transaction courante. Il exige encore l’unicité du grant actif, les fichiers privés retenus, la sauvegarde et la configuration OIDC épinglée. La recette ajoute un compte neuf avec un identifiant d’appartenance déjà pris pour exercer le rollback au second append.

| Rejeu | Statut | Observation |
|---|---|---|
| Création locale et révocation d’ancienne portée | FAIL en 54,43 s | Recensement terminé en 1 321 ms. Le writer arrive à la réservation après 2 005 ms et refuse sur expiration, avant le premier append. |
| Dérive du voisin B16 entre recensement et admission | PASS en 50,71 s | L’entête de la base change avec la même génération SQLite et la même tête d’audit. Le writer refuse ce recensement, rend le fichier original et conserve l’état d’A17. |
| Provisionnement OIDC, réconciliation, rollback et profil | FAIL en 73,52 s | Les deux reçus sont écrits puis retrouvés. Le conflit du second append annule le compte du plan de conflit. L’admission OIDC répond `201`, puis la lecture du profil reçoit `503` à la place de `200`. |
| Pair Auth5 indisponible | PASS en 51,85 s | Le refus rend le plan préparé, le même inode de credential et l’état d’origine du Store. |

Build en 2 min 31 s, 1 325 empreintes concordantes. SHA-256 du manifeste `afb03012b33659ca6fe776c02d0193c13bda82d9d520967e1073c347bd7087bd` et du libtest `db743faada91b584c9b5a556dfd7e32040760c4a0627ba320da21e8ce25076d3`. Le rejet du profil reste un échec du parcours complet.

### Rejeu du provisionnement avant la correction de sauvegarde

| Rejeu | Statut | Observation |
|---|---|---|
| Provisionnement local et révocation d’ancienne portée | FAIL en 28,80 s | La préparation du Store 17 dépasse l’époque de deux secondes à `opening_floor`, avant le recensement. |
| Refus de dérive de la sauvegarde à tête d’audit inchangée | FAIL en 23,06 s | La vérification initiale de la sauvegarde refuse avant la modification prévue par le test. Le refus sur dérive n’a pas été exercé. |
| Provisionnement OIDC, réconciliation, rollback et profil | FAIL en 55,56 s | La vérification initiale de la sauvegarde refuse avant la mutation OIDC et avant la lecture du profil. |
| Profil d’administration OIDC sans portée personnelle | FAIL en 50,03 s | La vérification initiale de la sauvegarde refuse avant l’inscription du compte. |

Les trois refus de sauvegarde produisent `execution17_integrity` avec le motif `execution authority backup contents changed during verification`. Les résultats n’établissent pas l’effet du dernier changement de lecture OIDC. Le rejeu R16 précédent utilise un autre chemin de sauvegarde. Build en 2 min 28 s, 1 325 empreintes concordantes. SHA-256 du manifeste `f5e061c0b78bd13b82dc41ec5b18582ae8ae2362979e17d20b5213ee536dd659` et du libtest `bc3eb2ef7b2eb2c7d64728f90592f863546a8b7d7cf712a74fca86d94d0986bc`.

### Journal SQLite de la sauvegarde d’identité

Une sonde synthétique sur SQLite 3.51.2 reproduit le changement de fichiers lors d’une lecture seule. La copie d’une base en WAL garde ce mode. Après fermeture, seul `main` existe. Le premier SELECT sur une connexion ouverte en lecture seule crée `-wal` et `-shm`, encore présents après fermeture. La copie préparée en mode `DELETE` garde uniquement `main` avec les mêmes métadonnées physiques avant et après cette lecture.

Le correctif passe la seule copie neuve en mode `DELETE` et vérifie ce mode et l’absence des trois fichiers auxiliaires avant synchronisation et vérification de la copie. Le test contrôle la tête et l’intégrité de la sauvegarde intacte. Il modifie ensuite `application_id` en gardant la même tête et exige le refus précis du changement de contenu. Le test vérifie aussi la tête du Store source.

Deux constructions Cargo du premier gel déclarent le succès, avec 1 325 empreintes de sources concordantes. Le chemin annoncé contient dans les deux cas un fichier de 1 632 octets en mode `0600`, sans format exécutable. Les tests Rust de ce gel sont **NOT_RUN**. Le lancement a été bloqué avant la sélection d’un test. Le gel utilise le manifeste SHA-256 `a35971a54240f0637691ed72cd56d123d6abb2ed78f57814d9802a796d13bd0f`.

### Rejeu Rust après correction de sauvegarde et de démarrage

La lecture de démarrage fixe désormais explicitement le snapshot SQLite avant la validation de publication. Les deux variantes de publication exécutent déjà le replay commun sur ce snapshot. Le contrôle intermédiaire est omis seulement dans une transaction explicite sur un snapshot `Read` ou `Write`. La validation après commit, la revalidation Auth5 et la limite de deux secondes gardent leurs vérifications.

La première compilation vers une sortie explicite échoue au manque d’espace disque. Sept binaires de preuve antérieurs sont archivés en Zstandard sans perte et leurs empreintes sont vérifiées après décompression. La compilation suivante termine en 2 min 38 s avec un vrai exécutable Mach-O ARM64. Les 1 325 empreintes de sources concordent. Manifeste SHA-256 `2da0b3b6d4c995a18a83a5d5247a3a17874451592da22e18ca9ad9a2d0435fbc` et libtest `1078c8cf37f9ef0e6765f47276dbd6a9d6bdc044a6e15f8b7a85c840c76c824b`.

| Test exact | Statut | Observation |
|---|---|---|
| `identity_backup_refuses_physical_drift_with_unchanged_audit_head` | PASS en 22,43 s | La copie intacte est admise et relue sans fichier auxiliaire. Après modification réelle d’`application_id`, le refus identifie la dérive et la tête d’audit reste identique dans la copie et dans la source. |
| `actual_a17_b16_auth5_oidc_admission_keeps_native_personal_ceiling_and_original_context17` | FAIL en 75,24 s | Le provisionnement, le rejeu du conflit et l’admission HTTP 201 aboutissent. Le premier GET du profil renvoie `503 audit_unavailable`. |
| `actual_native_oidc_admin_only_mapping_produces_administration_without_personal17` | FAIL en 67,18 s | Cette recette ne provoque aucun rollback de provisionnement. L’admission aboutit et le premier GET du profil renvoie également 503. |
| `native_local_admin_membership_mutation_audits_launch_send_and_revokes_its_old_scope` | FAIL en 54,26 s | L’ouverture du Store et le recensement aboutissent. Le writer expire à l’écriture de l’appartenance après 2 003 ms, avant le commit. |
| `actual_a17_b16_authentication5_hot_startup_reaches_native_gateway` | PASS en 47,95 s | Le vrai démarrage mixte A17/B16 et Auth5 atteint le gateway. |
| `native_startup_memory_postcheck_detects_control_mutation_and_retains_original_store` | PASS en 49,26 s | Une dérive des fichiers de contrôle pendant la mise à jour mémoire est refusée après la transaction. Le Store d’origine est conservé. |
| `native_startup_writer_expiration_rolls_back_and_keeps_original_cause_store_owner` | PASS en 27,40 s | L’expiration du writer annule le changement et restitue le Store et la cause d’origine. |

Pour rejouer un cas depuis le checkout Native17, sélectionner son nom dans le tableau et lancer `cargo test --lib --locked --offline -j1 NOM_DU_TEST -- --test-threads=1`. La recette de preuve a exécuté chaque sélecteur exact dans un processus dédié, réseau bloqué et répertoire temporaire privé.

### Rejeu d’identité après réduction des relectures

La lecture de la session Auth5 courante valide déjà le même journal, le propriétaire et le plancher avant de contrôler le binding actif. Elle remplace les deux relectures immédiatement successives de ce chemin. La projection d’identité soumise au CAS réutilise les valeurs déjà contrôlées dans sa transaction SQLite. Les écritures d’audit munies d’une preuve d’horloge conservent leur vérification de format et leur contrôle horloge, qui valide la même projection Native17 avant la recherche de reçu. Les réservations et limites d’audit restent contrôlées dans le chemin commun.

Le diagnostic de test du premier GET distingue la réponse de l’audit d’authentification de celle de la projection. Il indique si la lecture entre dans la transaction du Store A, sans afficher le corps du profil ni le jeton.

| Test exact | Statut | Observation |
|---|---|---|
| `actual_a17_b16_auth5_oidc_admission_keeps_native_personal_ceiling_and_original_context17` | FAIL en 29,59 s | L’ouverture du Store 17 expire à `opening_floor`, lors de la validation de publication après le commit de lecture. Le GET de profil n’est pas atteint. |
| `actual_native_oidc_admin_only_mapping_produces_administration_without_personal17` | FAIL en 54,12 s | La vérification OIDC épinglée en précommit du provisionnement expire. Le GET de profil n’est pas atteint. |
| `native_local_admin_membership_mutation_audits_launch_send_and_revokes_its_old_scope` | FAIL en 32,87 s | L’incarnation au démarrage expire après 2 247 ms. Le writer nominatif n’est pas atteint sur ce gel. |
| `actual_auth5_logout_retires_the_original_admitted_arc17` | PASS en 60,43 s | La déconnexion enregistre son audit et retire la session admise d’origine. |
| `sqlite_cas_and_rollback_do_not_publish_partial_epochs` | PASS en 0,02 s | Le CAS et l’annulation SQLite gardent les générations cohérentes. |
| `real_native_read_epoch_refuses_identity_epoch_mutation` | PASS en 23,15 s | Une vraie époque de lecture ne permet pas une mutation d’identité. |
| `same_millisecond_authorizations_work_but_a_fresh_raw_receipt_before_the_proof_is_refused` | PASS en 0,21 s | L’horloge refuse un instant reçu antérieur à la preuve. |
| `utc16_append_advances_floor_and_regression_and_floor_tampering_are_rejected` | PASS en 4,57 s | L’append legacy fait avancer le plancher et refuse retour en arrière ou plancher modifié. |
| `ordinary_control_and_identity_writes_preserve_terminal_audit_reservation` | PASS en 75,58 s | La saturation par les écritures ordinaires de contrôle et d’identité conserve les réserves nécessaires aux opérations terminales. |

Build en 2 min 31 s avec 1 325 empreintes concordantes. SHA-256 du manifeste `6f79079444cf3e9eeb61cd33400a69bc5d0d3bf8d49e3014b6779169d33601ef` et du libtest `622e5a03d8ab94ce1e0672f693fdb406c6dd2e1b3d34cdab43286251f82e8642`.

### Reprise du profil OIDC dans Chromium

L’ancien bundle échoue après le premier HTTP 503 avec une seule remise de jeton. Il n’affiche aucune action pour relire le profil. Le HTML assaini et le PNG de cet échec sont conservés en privé.

Le bundle corrigé passe la recette Playwright en 2,56 s avec Chrome 148, Playwright 1.66 alpha et Node 24.22.2. Le navigateur relit le profil avec la même admission après un 503 typé et présente les contextes partiels à la réponse 200. Une lecture en vol bloque les doubles demandes. Les 401 et 403, le 503 hors contrat et les corps 503 HTML ou JSON mal formés réaffichent la connexion. Le renouvellement pendant l’attente laisse la nouvelle génération attendre une action de l’utilisateur. L’expiration annule une lecture en cours. La compilation TypeScript/Vite et les 38 tests Node ciblés passent.

Le conducteur `front/tests/browser_oidc_identity_recovery.mjs` sert des réponses HTTP synthétiques sur loopback. Il vérifie le comportement du bundle. Le backend OIDC Native17 et SQLite gardent leurs recettes Rust. La rotation pendant une relecture manuelle déjà en vol a été revue statiquement. SHA-256 du bundle `c8f954663083861b70acb11358915b5094b3113d5daa7b8656f208f32eb9f3c7` et du conducteur `90e5f8de72660cbf82367d60f339066b919d544180fac28ab49e0948ced3f853`. [Commande de rejeu](../../../../../tests/e2e/README.md#reprendre-la-lecture-du-profil-oidc-native17)

Le conducteur étendu passe en 1,55 s sur le même bundle. Le premier HTTP 200 présente une liste partielle vide et l’action « Actualiser les contextes ». Le navigateur n’annonce aucune absence de droits. L’action au clavier déclenche un deuxième GET sur la même admission et affiche le contexte personnel, sans nouvelle remise de jeton ni requête métier avant le choix. La réponse et la remise de jeton restent synthétiques. SHA-256 du conducteur étendu `eac161b04e984ca0fdb8d652b68831530ebce11b391e2f09c0817c9d0107f21c`.

### Lecture de publication après commit

Après le premier commit SQLite de lecture, le contrôle de publication prend un second instantané `Read` sur la même connexion sous le même verrou de supervision. Les deux contrôles de publication et le pair Auth5 sont conservés dans cet instantané. Après fermeture, le garde compare à nouveau le format et les faits SQLite de la connexion, puis revalide l’époque et la source. Un changement de contenu ou des métadonnées du WAL à cette frontière peut entraîner un refus. Le contrôle des fichiers propres à la publication se fait dans le second instantané, après le premier commit.

Le writer d’incarnation réutilise les vérifications de source et de replay que l’append vient de faire dans la même transaction, y compris lors d’un rejeu. Ses contrôles avant et après commit restent en place. En écriture OIDC, les contrôles d’époque d’entrée et de sortie et les deux fichiers épinglés restent vérifiés. La validation intermédiaire vérifie l’appartenance du token Auth5 retenu à son propriétaire et l’état d’admission, sans réémettre le même token.

| Test exact | Statut | Observation |
|---|---|---|
| `native_startup_publication_read_refuses_sqlite_drift_at_same_head` | PASS en 24,01 s | Après le second commit, une seconde connexion SQLite modifie `application_id` sur le fichier de recette. Le garde refuse précisément la dérive de l’instantané, avec la même tête, le même fichier et la connexion retenue en autocommit. |
| `actual_native_oidc_admin_only_mapping_produces_administration_without_personal17` | FAIL en 70,75 s | Le commit, l’admission et la lecture du compte aboutissent. Le GET rend 200 avec zéro contexte au lieu d’un contexte d’administration. |
| `actual_a17_b16_auth5_oidc_admission_keeps_native_personal_ceiling_and_original_context17` | FAIL en 77,37 s | Le provisionnement et l’admission aboutissent. Le GET rend 200 avec zéro contexte au lieu d’un contexte personnel. |
| `native_local_admin_membership_mutation_audits_launch_send_and_revokes_its_old_scope` | FAIL en 54,17 s | Le démarrage et le recensement aboutissent. Le writer expire à l’écriture d’appartenance après 2 007 ms, avant commit. |
| `actual_a17_b16_authentication5_hot_startup_reaches_native_gateway` | PASS en 47,88 s | Le démarrage mixte conserve le vrai pair Auth5 et atteint le gateway. |
| `native_startup_writer_postcommit_refusal_keeps_canonical_debt_without_publishing_authority` | PASS en 26,07 s | Une dérive après commit laisse la dette canonique observable. Aucune autorité n’est publiée. |
| `actual_retained_trust_mutation_refuses_route_and_context_without_reinstall17` | PASS en 58,72 s | La dérive du fichier de confiance retenu fait refuser la route et le contexte sans réinstallation. |

Build en 2 min 29 s, 1 325 empreintes concordantes. SHA-256 du manifeste `8d18a331052204e4a131845077e65f6afe658e458ab8091a37dd895662de6a8c` et du libtest `fad8d24ba239bf8e16d1db14f33f3c351c144060d820d4652f18bd4d2069683b`.

Le nouveau `doctor` de recette Docker retourne à nouveau le code 1 et `control OS account home is unavailable`. Le refus précède l’accès au moteur. Aucun worker ni volume n’est créé. Le même prévol dans le Terminal de connexion reste à comparer.

### Diagnostic du contexte OIDC absent

L’observateur de test du Store A relève l’étape qui écarte le contexte. Il rapporte des valeurs fermées et conserve la même requête HTTP et le même résultat attendu.

| Recette | Statut | Observation |
|---|---|---|
| Profil personnel OIDC A17/B16/Auth5 | FAIL en 77,95 s | HTTP 200 et zéro contexte. Le premier refus observé est `ObtainContext(EpochExpired)`, lors de l’enregistrement du contexte après l’autorisation. |
| Profil administrateur OIDC | FAIL en 51,91 s | L’époque expire pendant `commit authorized OIDC enrollment`. L’API de lecture et son observateur ne sont pas atteints sur ce gel. |

Build en 2 min 28 s, 1 325 empreintes concordantes. SHA-256 du manifeste `cfa03acc20a81350eb2c964cd1a641522eeedf9d3539b28ecc5094bf5cb955c6` et du libtest `d8a8633186955ab039156f25e50c85e55ddfed9b3cf60b5c381b2da8825da2d3`.

### Tête Auth5 et session courante dans la même lecture

Le précontrôle OIDC prélève le binding courant et la tête de l’audit Auth5 dans la même projection SQLite vérifiée. Le code vérifie toujours le binding exact et actif, le fencing par empreinte de credential, les fichiers retenus, l’époque et le périmètre demandé. La relecture historique vérifie cette tête précise et le binding actif à cette position. Elle reste une opération distincte. Le changement supprime une relecture d’Auth5 dans le précontrôle en lecture et deux dans le précontrôle en écriture.

| Recette | Statut | Observation |
|---|---|---|
| Profil personnel OIDC A17/B16/Auth5 | FAIL en 74,30 s | HTTP 503 `audit_unavailable` lors de la projection du profil, après l’entrée dans le Store. Le diagnostic d’enregistrement est hors de portée de ce rejeu. |
| Profil administrateur OIDC | FAIL en 69,90 s | Le compte est lu, mais la liste est vide avec `ObtainContext(EpochExpired)`. La réduction des lectures Auth5 ne suffit pas à faire passer l’enregistrement. |
| Déconnexion depuis la route OIDC Native17 | FAIL en 25,59 s | Le démarrage expire pendant la validation de publication de `from_published17`, après 2 295 ms. La déconnexion n’est pas atteinte. |
| `v5_requires_exact_source_epoch_and_current_incarnation` | PASS en 1,45 s | Auth5 exige la source, l’époque et l’incarnation courantes. |
| `v5_native_logout_and_refresh_keep_existing_capacity_contract` | PASS en 1,11 s | Le vrai journal de recette retrouve la session courante après rotation et préserve ses contraintes de capacité à la fermeture. |

Build en 2 min 28 s, 1 325 empreintes concordantes. Les cinq essais utilisent le même binaire. SHA-256 du manifeste `008ad5249e5dcb73b9819a2397818ebac80df3ae6d7b8b70b35d65ff07978d20` et du libtest `72ef5abded9ec7fbb62ce5f21b60b781b2cb8db62361b4379b35b130d39f20df`.

### Prérequis de l’activation opérateur après reprise

La revue du code distingue la reprise de l’union C3 de l’activation du profil utilisateur. `load_native_broker_source17` reconstitue les Stores et les fichiers retenus sans produire la preuve `RegisteredNativeEgressProfile17` dont le serveur natif a besoin. Le producteur E2 actuellement disponible exige le résultat frais de publication et les Stores d’origine. En l’absence de cette preuve de profil, le chemin de bootstrap refuse `ProfileAbsent`.

Le writer natif de politique de lancement existe et exige un administrateur d’installation courant sur le même Store, sa sauvegarde vérifiée et un broker enrôlé. Il n’a aucun appelant CLI. Le guide mentionne encore la forme historique `review_binding_id`, alors que le contrat natif porte `grant_policy`. L’activation opérateur et le catalogue après reprise restent ouverts. Ces constats proviennent d’une revue du code, sans lancement de worker.

### Reçu du profil et provisionnement local Native17

La lecture du profil conserve le replay complet d’entrée, le reçu obligatoire `control.identity.access` et le même commit vérifié. Après ce seul append d’identité, le contrôle Native17 vérifie l’accès `Business`, la source et le genesis canonique lié dans la même transaction. L’oracle compte les reçus du principal OIDC de recette avant et après le premier GET. Un succès HTTP doit avoir exactement un nouveau reçu durable.

| Recette | Statut | Observation |
|---|---|---|
| Premier GET du profil personnel OIDC | FAIL en 43,37 s | Refus de construction du registre au démarrage avant de pouvoir lire le profil. Le registre rend les ressources d’origine. |
| Premier GET du profil administrateur OIDC | FAIL en 69,74 s | HTTP 200 et exactement un reçu de profil durable. L’autorisation aboutit, mais l’enregistrement du contexte expire. La liste est vide. |
| Provisionnement local nominatif | FAIL en 54,69 s | L’époque expire à l’écriture du credential avant commit. Le lot inclut encore un contrôle complet supplémentaire après admission. |
| Réconciliation après perte de la réponse au commit | FAIL en 54,21 s | Expiration à l’écriture du credential. Le point de perte de réponse n’est pas atteint. |
| Réconciliation après refus avant commit | FAIL en 54,09 s | Expiration à l’écriture du credential. Le point d’injection du refus n’est pas atteint. |

Build en 2 min 28 s avec 1 325 empreintes concordantes. SHA-256 du manifeste `e3d9ae4541b9b006c5eebd17555994b3c64902c9f7dcbaf64656a05165fbb836` et du libtest `69b7623a82903d52c589de70876c05bf79ab15dab67e2547793d08cb5a1c13c2`.

### Publication du contexte OIDC et validation E2 reprise

Le registre OIDC termine la vérification durable de génération sur le même Store et dans la même époque que le précontrôle Auth5. Cette vérification précède l’insertion. Sous le verrou mémoire, le rappel vérifie le fence, l’époque, la couverture du propriétaire et l’expiration du contexte. Il ne fait aucun SQL. Un refus transitoire ne crée pas d’entrée et préserve le contexte existant et son échéance. Le contrôle du membership reste celui de la phase d’autorisation et des usages du contexte.

L’admission locale prend en charge la lecture des trois identifiants dans le writer, le contrôle du plan et la réservation du candidat dans une seule scelle. Chaque opération conserve sa projection, son quota, son horloge, son CAS et ses contrôles d’accès. Sur refus, le test mesure séparément l’admission et le writer, puis vérifie le nombre d’opérations du plan dans SQLite après la fermeture de la transaction.

Le port cold E2 reprend la publication `Complete` et les fichiers privés du bundle C3. Pour chaque Store déclaré, il vérifie sous époque de lecture le profil durable, sa provenance et le rejeu E2 avec l’historique Auth5 disponible. Le bundle reste chez l’appelant lors d’un refus. Ce port prépare une preuve de profil à destination de l’installateur existant. Son raccordement au démarrage du service et à la CLI reste ouvert.

| Test exact | Statut | Observation |
|---|---|---|
| `refused_context_publication_keeps_new_entries_out_and_checks_reused_entries` | PASS en 0,05 s | Un refus du contrôle terminal laisse le registre sans nouvelle entrée. Sur réutilisation, l’Arc d’origine et son échéance sont conservés. |
| `cold_c3_recovers_durable_e2_on_the_same_source_without_writes` | PASS en 132,56 s | Après fermeture des propriétaires d’origine, C3 réouvre A17, B16 et le vrai Auth5 de recette. Les deux reçus sont retrouvés sur le même inode. Un digest public divergent est refusé. L’installateur accepte A17 et refuse B16. Les Stores et le journal Auth5 restent inchangés entre les observations après C3. |
| `cold_c3_refuses_declared_egress_without_its_durable_profile` | PASS en 112,86 s | Le profil est déclaré et le genesis de session existe. L’absence du profil E2 durable déclenche `ProfileRequired`. Le Store reste inchangé. |
| Profil administrateur OIDC | FAIL en 69,57 s | Le premier GET répond 200 et enregistre un reçu durable. Après la comparaison durable, la publication du contexte est refusée par le registre avec `StateRevoked`. L’étape exacte à l’intérieur du garde terminal reste à localiser. |
| Provisionnement local nominatif | FAIL en 56,03 s | Recensement 1 248 ms, admission sous époque d’écriture 339 ms, appel writer 1 665 ms. L’époque expire à l’écriture de l’appartenance. Après fermeture de la transaction, aucun des trois identifiants du plan ne figure dans `identity_operations`. |
| Profil personnel OIDC A17/B16/Auth5 | FAIL en 77,79 s | Le GET renvoie 503 `audit_unavailable` au contrôle `InstalledExit`, après l’étape d’écriture du reçu et avant commit. SQLite ne contient aucun nouveau reçu de profil. La cause interne de ce refus reste ouverte. |

Build en 2 min 27 s avec 1 325 empreintes concordantes. SHA-256 du manifeste `6b1e27939cfb09de0080cd805c70068faebc085ab78ff03461cb9e56a8a4e7e4` et du libtest `13c7a43159cd2c39dc95b1e272ba520d29e0d27e48e4dcee19371e983de20195`.

Pour rejouer les deux cas E2 depuis le checkout Native17, exécuter `cargo test --lib --locked --offline -j1 cold_c3_ -- --test-threads=1`. Les essais utilisent les fonctions de récupération et d’installation Rust avec SQLite et Auth5 de recette. Le redémarrage du service CLI, le lancement d’un worker et la reprise après un lancement OIDC causal ne font pas partie de ces deux tests.

### Démarrage du gateway avec le profil E2 repris

Le démarrage récupéré transmet le vrai bundle C3, le fichier public retenu et la preuve E2 au gateway natif. Les sources candidates viennent du registre privé retenu. La validation du registre utilise les Stores et la publication `Complete` d’origine. Le fichier Control public est revalidé avant la mise à disposition et la réservation des listeners.

| Test exact | Statut | Observation |
|---|---|---|
| `cold_c3_hosts_the_recovered_egress_on_the_reserved_gateway` | PASS en 143,47 s | Le vrai serveur installe la preuve pour A17 uniquement, écoute sur `127.0.0.1` et renvoie HTTP 200 sur `/healthz`. L’arrêt et le drainage aboutissent. |
| `refused_context_publication_keeps_new_entries_out_and_checks_reused_entries` | PASS en 0,05 s | Le contrôle terminal précède l’insertion et la restitution du contexte existant. Un refus ne crée aucune entrée et ne rallonge aucune échéance. |
| Profil personnel OIDC | FAIL en 77,68 s | HTTP 200, un reçu durable et aucun contexte. Le contrôle terminal du registre rapporte exactement `PublicationRevalidation(EpochExpired)`. |
| Profil administrateur OIDC | FAIL en 65,17 s | HTTP 503, un reçu durable et aucun refus observé à l’intérieur de la transaction de profil. L’API refuse après la lecture auditée. |
| Provisionnement local nominatif | FAIL en 53,64 s | Recensement 1 215 ms, acquisition de l’époque 7 ms, admission 298 ms, writer 1 702 ms. Le contrôle des fichiers après les trois écritures expire. L’inspection vérifie zéro opération et zéro événement commis pour les trois identifiants du plan. |
| `staged_census_rejects_neighbor_file_drift_with_unchanged_generation` | PASS en 50,49 s | La dérive physique de B16 est refusée malgré une génération inchangée. Le plan et le backing d’origine restent retenus. |
| `cold_c3_refuses_declared_egress_without_its_durable_profile` | PASS en 114,04 s | Le démarrage récupéré refuse `ProfileRequired` avant la réservation. Les ressources d’origine restent retenues et le Store n’enregistre rien. |

Build en 2 min 28 s avec 1 325 empreintes concordantes. SHA-256 du manifeste `d3de7cc9ebb2a26aea030aad7ff4a49865ccfa5851f884fa5050c28a001db234` et du libtest `ce53b2237374acbc7329ab65b522c107f6f8077c67ad094ee586eb68c79b6770`. Seul le test serveur a reçu l’autorisation réseau localhost du harnais. L’adresse d’écoute effective est vérifiée dans ce test.

La sonde HTTP prouve la disponibilité du gateway démarré sur la fixture froide. La configuration CLI de récupération, l’egress d’un worker et le drainage avec une requête encore en vol gardent leurs propres critères.

Depuis le checkout Native17, rejouer le démarrage et son refus sur profil durable absent avec une seule compilation et un seul test à la fois. Le premier cas nécessite l’écoute et les requêtes HTTP sur `127.0.0.1`.

```sh
cargo test --locked --offline --lib -j1 cold_c3_hosts_the_recovered_egress_on_the_reserved_gateway \
  -- --test-threads=1
cargo test --locked --offline --lib -j1 cold_c3_refuses_declared_egress_without_its_durable_profile \
  -- --test-threads=1
```

### Prévol du vrai binaire gateway Native17

Le binaire natif Arm64 compile en 1 min 39 s et les 1 326 empreintes concordent. SHA-256 du manifeste `6d0fb328ed44eef8e74a8962dbf2a940bf47ffe70a48995d16089b5ad8d33557` et du binaire `a5b2d838dd7da8379d6046904536f68ff0833c88d79ea94685f16e6a8c98086f`. Les cinq invocations sont exécutées sans réseau, avec des chemins synthétiques et sans credential.

| Invocations du binaire | Statut | Observation |
|---|---|---|
| `gateway --help` | PASS, code 0 | L’aide expose `--native-source17-locator`. |
| `gateway --native-source17-locator <chemin-absolu-absent>` | PASS, code 1 | Refus `source17_private_configuration_required`. Aucun chemin dans le diagnostic, aucune annonce de listener. |
| `gateway --native-source17-locator <chemin-relatif>` | PASS, code 1 | Même refus et aucun chemin révélé. |
| `gateway --native-source17-locator <chemin> --human-sources-activation <chemin>` | PASS, code 2 | Clap refuse la sélection simultanée des sources. |
| `gateway --native-source17-locator <chemin> --tenants-file <chemin>` | PASS, code 2 | Clap refuse le registre tenant indépendant. |

Rejouer avec `cargo build --locked --offline --bin agentsandbox -j1` dans le checkout Native17 puis avec `target/debug/agentsandbox`. Pour les refus, utiliser des chemins synthétiques et une session sans variable `JEV_BACKEND`. `--listen 127.0.0.1:0` rend la demande de port explicite.

La revue du même lot détecte que la lecture d’un profil `egress_profile17` dans un registre du gateway historique peut accepter le champ sans appliquer E2. Le refus explicite dans ce mode doit être vérifié séparément du démarrage Native17. Le démarrage positif de la CLI depuis un locator publié reste à exécuter.

### Étapes d’autorisation après déduplication des validations

Les lectures et écritures conservent les vérifications de l’époque d’origine, du vrai propriétaire, du backing, du rejeu SQLite et des fichiers retenus. La validation sous époque contrôle le propriétaire via le garde de l’accès après le rejeu. Lorsqu’une expiration intervient à cette étape, le contrôle de stockage garde sa possibilité de signaler une dérive réelle. Les deux chemins se ferment sur refus.

| Cas du même binaire de test | Statut | Observation |
|---|---|---|
| Registre, refus avant publication mémoire | PASS en 0,05 s | Aucun nouveau contexte après refus. Une réutilisation garde le contexte et son échéance d’origine. |
| Locator CLI relatif | PASS en 0,02 s | L’adaptateur refuse avant d’appeler le résolveur des credentials. |
| Dérive du Store B16 à génération identique | PASS en 49,25 s | Le recensement refuse la dérive physique. |
| Provisionnement local nominatif | FAIL en 52,77 s | Recensement 1 042 ms, acquisition 7 ms, admission 294 ms, writer 1 881 ms. Le contrôle des fichiers et celui du credential administrateur réussissent. Le chemin expire à la revalidation de la sauvegarde, avant le commit. Zéro opération et zéro événement du plan dans SQLite. |
| Profil administrateur OIDC | FAIL en 66,68 s | HTTP 200 et un reçu durable. L’insertion du contexte expire au dernier contrôle, `PublicationRevalidation(EpochExpired)`. |
| Profil personnel OIDC | FAIL en 78,18 s | Le GET répond 200, écrit exactement un reçu et produit le contexte personnel attendu. La lecture native de ressources appelée par le témoin interne refuse ensuite. Le GET HTTP de ressources reste à vérifier sur ce chemin. |

Build en 2 min 28 s, 1 326 empreintes concordantes. SHA-256 du manifeste `5eb0baa20807ba6bda138d8a462107c13a1d7ed947290045f875847355cba6ed` et du libtest `4e0851d548eb353ad13568c46ffd60c2cb618c7a9d42e9025d1edda368e42763`.

### Refus d’un profil E2 en démarrage historique

Le vrai binaire refuse un registre à deux tenants lorsque le second déclare un profil `egress_profile17`, en mode historique direct et en mode `owned_sources` direct. Les deux appels rendent `1 gateway_egress_profile_requires_native17` sans annoncer de listener ni créer de fichier. Les références de credentials sont absentes dans le répertoire synthétique. Le test du parseur couvre aussi l’acceptation du registre sans profil et la conservation du profil par les outils hors ligne. La voie d’activation historique scellée est couverte par la revue source du même parseur.

Build en 1 min 39 s, 1 326 empreintes concordantes. SHA-256 du manifeste `bc564a9b45b7b985d37281aa86d05c7b8c40207eb42a784ecc653fbfd374f90b` et du binaire `91d3120a1a133f7156cfab5ee9af05080b2eeeccb3ae6be6b45e610e9fcf6c0f`. Les deux essais tournent sans réseau et sans secret.

### Recette de redémarrage par le vrai CLI et régressions du parseur

Le test Rust produit la configuration canonique publique, un registre privé distinct et quatre références privées, puis prépare les Stores A17/B16 et Auth5 d’origine. Le binaire `agentsandbox gateway` utilise ce montage pour tester `/healthz`, le refus de la route historique, l’absence d’accès nominatif anonyme ou via credential technique, puis l’arrêt et les incarnations SQLite. [Commande de rejeu](../../../../../tests/e2e/README.md#rejouer-le-démarrage-à-froid-de-la-cli-native17)

| Test exact | Statut | Observation |
|---|---|---|
| `native_gateway_cli_recovers_both_tenants_and_preserves_identity_boundaries` | FAIL en 43,64 s | La préparation expire à `incarnation_retain17` avant de créer le locator final ou le processus CLI. Phase 2 006 ms, acquisition de l’époque précédente 7 ms. Aucune preuve nouvelle de démarrage CLI sur un locator valide. |
| `gateway_requires_native17_for_declared_egress_before_resolving_credentials` | PASS en 0,89 s | Le registre sans profil reste lisible. Le profil sur le deuxième tenant déclenche le refus avant le résolveur. La lecture de préparation préserve la déclaration. |
| `native_gateway_requires_independent_private_tenant_references` | PASS en 0,03 s | Le document public est refusé. Le contrôle privé retient les références runner et viewer distinctes et accepte les trois champs natifs historiques. |
| `native_gateway_locator_conflicts_with_other_source_selection` | PASS en 0,02 s | La sélection de source Native17 reste exclusive des sélections historiques. |

Le libtest compile en 2 min 29 s. Le manifeste contient 1 326 empreintes et diffère du gel du binaire uniquement par le module de recette `egress_native17_profile_tests.rs`. SHA-256 du manifeste `74cf7587d642336b266e2cae32196969380288a327bd1d5192efbca3fc25cd85`, du libtest `d09cd05513e5eaf3994c132625ae537121c1389b0733b711faeddc4703ef5c3f`, et du binaire de tests CLI `21050c284378850e615d6417b9493879e56e38802c604e612453c8b8449b532c`.

### Nouveau passage HTTP et contrôle physique de la sauvegarde

Le même libtest est rejoué avec le vrai GET de ressources placé avant le témoin interne. La vérification de sauvegarde conserve la comparaison de préfixe, le backing et le contrôle de contenu avec un contrôle d’accès final.

| Cas | Statut | Observation |
|---|---|---|
| Profil personnel puis GET de ressources | FAIL en 69,62 s | Le premier GET de profil répond 200 et rend le reçu attendu. Le registre n’expose aucun contexte après `ObtainContext(EpochExpired)`. Le GET de ressources reste `NOT_RUN`. |
| Provisionnement local | FAIL en 51,43 s | Recensement 1 132 ms, acquisition 7 ms, admission 320 ms et writer 1 778 ms. L’expiration intervient cette fois au contrôle des fichiers, après les trois écritures de la transaction. Zéro opération et zéro événement du lot restent commis. |
| `identity_backup_refuses_physical_drift_with_unchanged_audit_head` | PASS en 23,31 s | La copie intacte est relue. La modification physique sans changement de tête est refusée et les têtes d’audit restent identiques. |

### Rétention sur deux lectures SQLite et lancement du processus CLI

La rétention s’appuie sur une lecture SQLite ancrée pour produire l’autorité de review. Une seconde lecture vérifie à nouveau la publication et compare les faits de la même connexion, le format, le backing et la tête à ceux de la première. Après le second commit, la garde vérifie encore SQLite, le propriétaire et Auth5. Les deux transactions gardent leurs lectures complètes de la publication, leur lease et l’époque d’origine.

| Recette | Statut | Observation |
|---|---|---|
| `native_startup_writer_keeps_exact_idempotence_and_real_separate_retention` | PASS en 25,58 s | L’append garde le même effet au rejeu. La rétention réussit dans sa propre lecture. |
| `native_startup_publication_read_refuses_drift_between_sqlite_reads` | PASS en 26,58 s | La modification de `application_id` entre les deux lectures garde la tête d’audit. La comparaison refuse avec `Mismatch`. |
| `native_startup_publication_read_refuses_sqlite_drift_at_same_head` | PASS en 22,84 s | La même dérive après le dernier commit reste refusée avec `Mismatch`. |
| Démarrage à froid par le vrai CLI | FAIL en 133,53 s | La préparation franchit `incarnation_retain17` et lance le vrai processus. Le harnais expire sur le canal du signal prêt après 90 s. La sonde HTTP et l’arrêt gracieux restent non exécutés dans ce rejeu. |
| Profil OIDC et ressources | FAIL en 72,98 s | Le profil renvoie 200 et produit le reçu durable. Le dernier contrôle du registre expire avec `PublicationRevalidation(EpochExpired)` et aucun contexte. La route des ressources n’est pas atteinte. |

Build du vrai binaire en 1 min 38 s et du libtest en 2 min 31 s. Les 1 326 empreintes correspondent aux deux builds. SHA-256 du manifeste `65673616a728cee54d61117561aba68b2b0fe2d5d7e5057d8792e8becd37f9b6`, du binaire CLI `0fd6a10e2ab08ffe258d4946cbd74c244a0a01feeb1e09494b8b52f19b9415a9` et du libtest `0bfec41ff719ff9ac1e94bd6839fe80d9abe778ea4ab18971d6fcd2fc649ca69`.

### Rejeu CLI de la même reprise avec une attente adaptée

**PASS en 139,88 s** sur `native_gateway_cli_recovers_both_tenants_and_preserves_identity_boundaries`. Le même binaire démarre depuis le locator privé préparé par le test, reprend le Control et les stores A17/B16 et sert `/healthz` sur `127.0.0.1`. La route historique d’identité répond `503 identity_unavailable`. Le point d’entrée nominatif renvoie 401 sans credential et refuse chacun des quatre identifiants techniques. Le CLI sort avec le code 0 après `SIGTERM` dans la fenêtre de drainage de dix secondes. En lecture seule, SQLite retrouve les formats, le préfixe d’audit et exactement une nouvelle incarnation dans chacun des deux Stores, avec vérification d’intégrité.

Le seul changement depuis le build CLI concerne la recette. Son attente du signal prêt passe de 90 à 210 secondes et rend le statut du processus en cas d’échec. Build du libtest en 2 min 33 s, 1 326 empreintes concordantes. SHA-256 du manifeste `04ad65e4d4d044dbadc6fe192ed6180ef3c82534876660444a54b1792ef24794`, du libtest `24aed8ab1a26aa7359356aa8145ca20c089f77b2b9bedee850dc727f25a99513` et du binaire CLI repris `0fd6a10e2ab08ffe258d4946cbd74c244a0a01feeb1e09494b8b52f19b9415a9`. Les bases et credentials sont synthétiques. Ce test couvre un vrai service HTTP, avec décision Rust et réseau loopback. Connexion OIDC nominale, lancement de worker, trafic sortant et démarrage sur les autres moteurs gardent leurs preuves séparées. [Commande de rejeu](../../../../../tests/e2e/README.md#rejouer-le-démarrage-à-froid-de-la-cli-native17)

### Publication du contexte administrateur sous la même époque

La formation de la preuve de publication conserve le fence contrôlé après Auth5 et revalide la source dans la même époque de lecture. La génération durable et le contrôle complet sous le verrou du registre précèdent encore toute restitution du contexte. Les autres consommateurs de la preuve gardent le prévol générique.

| Recette | Statut | Observation |
|---|---|---|
| Refus avant insertion dans le registre | PASS en 0,05 s | Une erreur de validation ne publie aucun contexte et ne prolonge aucune échéance. |
| Profil administrateur et lecture des ressources | FAIL en 70,37 s | Le premier GET renvoie 200, ajoute un reçu et rend exactement un contexte administration. Temps cumulés après acquisition de l’époque de publication : 1 073 ms au prévol, 1 489 ms après lecture de la génération, 1 497 ms à l’entrée du registre et 1 779 ms à sa sortie. Le GET de ressources qui suit renvoie 503. |
| Compte non provisionné puis profil personnel | FAIL en 48,87 s | L’admission du compte non provisionné retourne 503 au lieu du refus 403 attendu. La partie nominale de cette recette reste `NOT_RUN` dans ce gel. |
| `actual_retained_trust_mutation_refuses_route_and_context_without_reinstall17` | PASS en 53,26 s | La route et le contexte refusent le fichier de confiance modifié avant toute réinstallation. |

Build en 2 min 27 s, 1 326 empreintes concordantes. SHA-256 du manifeste `bf84038b6a6f56a3360e096836b11a333c6a3b745226f6e4aeac2ba42be07c80` et du libtest `6ff80232cc87cf15235c302a9ee5259c2dbe59f083ad41b825847c9cbc04e1b0`.

### Diagnostic de la lecture OIDC des ressources

L’acquisition du fence avant une lecture vérifiée utilise désormais l’époque déjà empruntée par le lecteur. Les chemins autonomes d’installation et de rotation et les vérifications après lecture restent en place. Le refus du compte non provisionné a sa propre recette avec l’assertion 403, et les deux parcours nominatifs conservent l’assertion 200 des ressources. Les sondes de durée et les catégories de refus sont compilées uniquement pour les tests.

| Recette | Statut | Observation |
|---|---|---|
| `actual_native_oidc_admission_refuses_unprovisioned_account17` | PASS en 50,27 s | Le POST d’admission refuse en 403. |
| Profil administrateur puis lecture des ressources | FAIL en 69,94 s | Profil 200, un reçu et le contexte administration attendu. La publication finit en 1 799 ms. Ressources 503 `identity_unavailable` après 6 300 ms côté requête, avec expiration de l’époque à `ReaderEntry`. Le prévol OIDC du lecteur dure 1 306 ms, la lecture d’historique Auth5 suivante 148 ms et le contrôle d’entrée 370 ms jusqu’au refus. |
| Profil personnel puis lecture des ressources | FAIL en 72,67 s | Profil 200, un reçu et le contexte personnel avec `read_metadata`. La publication finit en 1 724 ms. Ressources 503 `identity_unavailable` après 5 996 ms côté requête. Le prévol OIDC dure 1 239 ms, l’historique Auth5 suivant 140 ms et le contrôle d’entrée 396 ms jusqu’à l’expiration. |
| `actual_retained_trust_mutation_refuses_route_and_context_without_reinstall17` | PASS en 51,04 s | Le changement du fichier de confiance bloque toujours les deux voies attendues. |

Dans les deux parcours de ressources, la transaction contrôlée a été ouverte. Le refus survient avant le callback de lecture auditée et avant son commit. Les temps des phases de publication sont cumulés depuis l’acquisition de l’époque. Les temps des phases du lecteur sont mesurés par étape de cette autre époque. Les deux cas utilisent une preuve signée de recette sur le vrai routeur Rust et les bases SQLite A17/B16/Auth5. La connexion au fournisseur d’identité et le navigateur ont leurs propres recettes.

Build en 2 min 25 s, 1 326 empreintes concordantes. SHA-256 du manifeste `b4add4bf6dde334bf851acdc642bb781c44c1b12c826a6d0705cd15944e57cbe` et du libtest `a86b704239ed621bb209f1325c1a83968009374173c1af541304e07525a4c72d`.

### Lecture conjointe de la session et de l’historique Auth5

Le prévol de la route des ressources OIDC prélève la session active et l’historique causal à la même tête d’audit, dans une seule transaction SQLite `DEFERRED` en lecture seule. Le garde conserve le pair Auth5 et le binding exacts, le verrou de lecture sur l’empreinte du credential, l’époque et les contrôles de source. Le lecteur vérifie ces autorisations avant la lecture métier.

| Recette | Statut | Observation |
|---|---|---|
| `actual_auth5_history_observes_native_deferred_read_and_query_only` | PASS en 32,99 s | Même tête d’audit pour la session active et l’historique. Une session absente reste absente, aucun événement n’est ajouté et une écriture légitime réussit ensuite. |
| Profil administrateur puis lecture des ressources | FAIL en 66,21 s | Profil 200, reçu et contexte attendus. Le GET des ressources renvoie 503 `identity_unavailable` après 6 000 ms. Le prévol conjoint dure 1 217 ms et `ReaderEntry` expire avant la lecture métier. |
| Profil personnel puis lecture des ressources | FAIL en 68,56 s | Profil 200, reçu et contexte attendus. Le GET des ressources renvoie le même 503 après 6 021 ms. Le prévol conjoint dure 1 245 ms et `ReaderEntry` expire avant la lecture métier. |
| `actual_retained_backing_replacement_keeps_the_permanent_failure_path` | PASS en 0,43 s | Le remplacement du fichier SQLite retenu conserve le refus permanent attendu. |

Les quatre cas utilisent le même libtest et des bases de recette, en série et sans accès réseau. Aucun appel fournisseur ni test de moteur de conteneurs n’a été lancé pour ce gel. Build en 2 min 29 s, 1 326 empreintes concordantes. SHA-256 du manifeste `3286f20ef33bdf91b3e3df474e38171e76a5d8d33a20b0fdfc897443ac8c4873` et du libtest `93e555389e0f54f6a82212ea172d244d33793c4a67157e29dc4b4915df199c08`. Depuis le checkout Native17, rejouer un cas avec `cargo test --lib --locked --offline -j1 NOM_DU_TEST -- --test-threads=1`.

### Lecture métier après regroupement des contrôles OIDC

Le contrôle tenant et Auth5 partage les deux validations du même propriétaire. Il conserve les vérifications du rattachement exact, de l’échéance d’origine, des fichiers OIDC et de la révocation par empreinte. Si un fichier OIDC épinglé refuse, la vérification du propriétaire peut encore inscrire l’invalidation d’un défaut permanent. Le garde d’empreinte reste tenu du prévol Auth5 jusqu’à la fin de la lecture, avec contrôle complet avant le callback métier.

| Recette | Statut | Observation |
|---|---|---|
| `actual_native_oidc_admin_only_mapping_produces_administration_without_personal17` | FAIL en 65,96 s | Profil 200, reçu durable et contexte administration attendus. Le prévol des ressources prend 961 ms, `ReaderEntry` passe en 741 ms, puis l’époque expire à `Read` après 75 ms dans cette étape. HTTP 503 `identity_unavailable` après 5 790 ms. |
| `actual_a17_b16_auth5_oidc_admission_keeps_native_personal_ceiling_and_original_context17` | FAIL en 68,33 s | Profil 200, reçu durable et contexte `read_metadata` attendus. Le prévol des ressources prend 968 ms, `ReaderEntry` passe en 720 ms, puis l’époque expire à `Read` après 93 ms dans cette étape. Même 503 après 5 816 ms. |
| `actual_retained_trust_mutation_refuses_route_and_context_without_reinstall17` | PASS en 51,02 s | La dérive du fichier de confiance reste refusée sur la route et le contexte, sans réinstaller le propriétaire. |
| `actual_native_oidc_admission_refuses_unprovisioned_account17` | PASS en 46,49 s | Le POST d’admission refuse en 403. |

Les deux lecteurs atteignent le callback métier. Ils expirent avant `ReaderExit` et avant commit. La revue du code relève que le callback de ressources revalide immédiatement le périmètre déjà établi à l’entrée dans la même transaction SQLite. Le reçu d’audit obligatoire et les gardes de sortie restent indépendants de ce doublon. Les tests utilisent des jetons signés et de vraies bases SQLite de recette, en série et sans réseau. Build en 2 min 28 s, 1 326 empreintes concordantes. SHA-256 du manifeste `5c635aef0f525688db394c236e7908f0f26570a1fd34946ac923a5684b00fc2d` et du libtest `9ab66ccd47feabbf1a402d34d18dc8fd0ac10ef8b94641b4d17fc1df4cb42227`. Rejouer depuis le checkout Native17 avec `cargo test --lib --locked --offline -j1 NOM_DU_TEST -- --test-threads=1`.

### Rejeu du périmètre des ressources et du provisionnement local

Le callback natif des ressources emprunte le périmètre validé dès l’entrée sur sa propre transaction. Les filtres ACL, le masquage, les limites de page, le reçu `control.identity.access` et les gardes de sortie restent appliqués. Sur HTTP 200, la recette exige un seul nouveau reçu durable avec le même identifiant d’événement, le même identifiant de requête et le contexte demandé dans la réponse. Le provisionnement local fusionne deux vérifications contiguës du même propriétaire en gardant l’époque d’écriture et les validations des fichiers. Les gardes du grant, de la sauvegarde et du commit restent appliquées.

| Recette | Statut | Observation |
|---|---|---|
| `actual_native_oidc_admin_only_mapping_produces_administration_without_personal17` | FAIL en 66,50 s | La lecture métier termine en 36 ms. L’époque expire au contrôle `NativeReaderAccessEntry` de `ReaderExit`. HTTP 503 après 5 816 ms et zéro reçu de ressources durable. |
| `actual_a17_b16_auth5_oidc_admission_keeps_native_personal_ceiling_and_original_context17` | FAIL en 68,52 s | La lecture métier termine en 36 ms. Même refus avant commit, HTTP 503 après 5 821 ms et zéro reçu de ressources durable. |
| `native_local_admin_membership_mutation_audits_launch_send_and_revokes_its_old_scope` | FAIL en 47,52 s | Les contrôles des fichiers privés et administrateur passent. L’époque expire à la seconde vérification de la sauvegarde, après les écritures et avant commit. SQLite confirme zéro opération et zéro événement du plan. |
| `unavailable_actual_auth5_refuses_local_provision_and_returns_same_prepared_files` | PASS en 45,47 s | L’indisponibilité du vrai pair Auth5 de recette bloque le provisionnement et restitue les mêmes fichiers préparés. |

Les traces du lecteur montrent deux validations OIDC de 258 à 261 ms chacune dans `ReaderEntry`, contre 33 à 34 ms pour un contrôle Business pris isolément. La requête SQLite de collision prend moins d’une milliseconde sur ces données. Les temporisations et les codes fermés sont réservés aux tests. Le test nominal local mesure 1 031 ms pour le recensement, 7 ms pour l’acquisition, 297 ms pour l’admission et 1 838 ms dans le writer.

Le premier build a été repris après correction du type de retour du helper de test. Le suivant échouait au retrait des symboles faute d’espace et ne produisait aucun exécutable valide. Le build exécuté conserve les symboles et termine en 2 min 22 s, 1 326 empreintes concordantes. SHA-256 du manifeste `c268660bb82d63564d5f440ccf34bbc9e8593c272a0565a22efde472e2f779c8` et du libtest `a606977c51c684d2cac69ce2a1d44e4444bfd1589f8c1bdac527738179b07e37`. Rejouer les sélecteurs ci-dessus depuis le checkout Native17 avec `cargo test --lib --locked --offline -j1 NOM_DU_TEST -- --test-threads=1`. Les deux GET exercent le routeur Rust. Le provisionnement exerce directement le port du Store. Les quatre cas utilisent SQLite hors réseau. Les parcours avec un fournisseur, un navigateur ou un moteur gardent leurs propres preuves.

### Acquisition et contrôle du verrou OIDC de lecture

Le prévol termine les contrôles de source avant d’acquérir le verrou de lecture par empreinte. La fabrique vérifie encore les collisions en attente, publiées et de bootstrap avant Auth5. Le même verrou reste détenu pendant le rejeu Auth5 et ses contrôles de sortie. La validation d’une époque de lecture OIDC complète vérifie une seule fois le bootstrap, après les vérifications des fichiers et de la session. Les écritures gardent les vérifications complètes. En présence de plusieurs défauts simultanés, le premier motif de refus peut changer. Dans ces cas le prévol ou le contrôle de sortie refuse la lecture.

| Recette | Statut | Observation |
|---|---|---|
| Ressources du profil administrateur | FAIL en 66,13 s | Prévol en 745 ms contre 978 ms au gel précédent. Entrée en 693 ms et lecture métier en 36 ms. `ReaderExit` expire à son premier contrôle OIDC. HTTP 503 et zéro reçu durable. |
| Ressources du profil personnel | FAIL en 68,38 s | Prévol en 761 ms contre 968 ms au gel précédent. Entrée en 685 ms et lecture métier en 36 ms. Même expiration, HTTP 503 et zéro reçu durable. |
| `actual_retained_trust_mutation_refuses_route_and_context_without_reinstall17` | PASS en 51,35 s | La dérive du fichier de confiance est refusée sur la route et le contexte. |
| `actual_auth5_logout_retires_the_original_admitted_arc17` | PASS en 53,09 s | La déconnexion enregistre son reçu Auth5, révoque la session originale et le profil refuse l’ancien jeton. |

Les validations OIDC de l’entrée coûtent 241 à 247 ms chacune sur cette fixture. Les quatre tests exercent le routeur Rust et de vraies bases SQLite synthétiques. Build en 2 min 22 s, 1 326 empreintes concordantes. SHA-256 du manifeste `14c31bb94a41163cc9bc59b4ed17f8adf28051a25a5a6b2d4fdafef8625f53a7` et du libtest `4e16226040a7b23ddebc3de8b39744696b443f9e8ba0ffa6ed6290e6270c2767`. Les deux tests nominaux gardent les sélecteurs du tableau précédent.

### Prototype de reprise à froid avec historique causal Auth5

Une copie privée du gel Native17 précédent a servi à composer un port de reprise. Celui-ci obtient le pair Auth5 réel avant le rejeu des références causales du tenant. Les deux nouvelles recettes préparent un broker et une policy native, puis doivent passer par la route de préparation de session OIDC avant d’exercer la reprise. Le premier build échoue sur des imports dupliqués et des signatures SQLite incompatibles. La correction maintient `query_only=1`, la liaison à la connexion réellement utilisée et la validation d’un voisin de format 14–16. Le build suivant produit le libtest en 2 min 25 s.

| Recette | Statut | Observation |
|---|---|---|
| `native_read_postfinish_refusal_retains_outcome_until_explicit_rejection_drop` | PASS en 0,14 s | Le refus tardif rend le résultat possédé. La connexion reste en autocommit avec `query_only=1`. |
| `cold_c3_recovers_durable_e2_on_the_same_source_without_writes` | FAIL en 96,29 s | Le test avait passé sur le gel antérieur. Sur cette copie, `cold_materialization` mesure 2 106 ms et expire avant la fin de la reprise. |
| `actual_post_oidc_causal_preparation_reopens_metadata_repin_c3_with_same_auth5` | FAIL en 61,32 s | Le montage bloque au chargement initial du broker avec `source17_native_c3_resume_required`, avant la demande HTTP de préparation. |
| `actual_post_oidc_causal_preparation_rejects_different_auth5_backing_before_c3` | FAIL en 61,59 s | Le même prérequis bloque avant la préparation et avant la substitution du pair. Le refus du pair divergent reste non exercé. |

La copie de recette reste isolée du checkout Native17. Le jeu de sources enregistre 1 358 empreintes concordantes. SHA-256 du manifeste `18a5e9a9148a88054a719b1b5fcc0e838d12e0bc34e4f62778303385213545f9` et du libtest `ff0a1cac274f49fb86bad3ed54d490e491fdceb5b9a06698bc6835b2db690e5c`. Dans la copie de recette, `cargo test --lib --locked --offline -j1 actual_post_oidc_causal_preparation_ -- --test-threads=1` sélectionne les deux tests causaux. Ils utilisent des jetons et des bases de recette, en série et hors réseau.

### Sauvegarde en lecture retenue et contrôle du gel Business

Le provisionnement réutilise le lecteur SQLite en lecture seule pour ses deux contrôles de sauvegarde. Le dernier contrôle relit la tête, vérifie les empreintes des fichiers, puis contrôle l’époque. Le corpus OIDC partage les deux scans du propriétaire qui se suivaient immédiatement avant les fichiers d’historique. Les gardes Business comparent une fois le journal d’autorité à l’intérieur d’un même instantané SQLite Native17. Elles revérifient le fichier et l’époque à la fin. Les écritures d’identité gardent leur validation dédiée.

| Recette | Statut | Observation |
|---|---|---|
| `identity_backup_refuses_physical_drift_with_unchanged_audit_head` | PASS en 21,27 s | Le changement du seul `application_id` de la copie entre les deux contrôles est refusé. La tête de sauvegarde reste identique. Le Store garde sa tête et ses comptes d’opérations et d’événements après rollback. |
| `native_local_admin_membership_mutation_audits_launch_send_and_revokes_its_old_scope` | FAIL en 47,37 s | Le sous-contrôle `check selected backup source` expire dès l’entrée dans le contrôle final, avant de relire le fichier. SQLite conserve zéro opération et zéro événement du plan. |
| Ressources du profil administrateur | FAIL en 64,95 s | Prévol en 713 ms, entrée en 651 ms et lecture métier en 35 ms. `ReaderExit` franchit la première vérification OIDC et expire à `OidcScope`. HTTP 503, zéro reçu durable. |
| Ressources du profil personnel | FAIL en 67,39 s | Prévol en 715 ms, entrée en 657 ms et lecture métier en 36 ms. Même expiration à `OidcScope`, HTTP 503 et zéro reçu durable. |
| `real_native_business_epoch_cannot_construct_an_identity_admin_source` | PASS en 20,67 s | Un accès Business est refusé pour l’administration d’identité. |
| `actual_retained_backing_replacement_keeps_the_permanent_failure_path` | PASS en 0,37 s | Le fichier remplacé déclenche toujours l’invalidation attendue. |
| `structured_descriptor_shortage_refuses_without_unlock_and_same_owner_revalidates` | PASS en 0,14 s | La pénurie de descripteurs refuse l’opération et le propriétaire reste utilisable après retour des ressources. |

Les sept sélecteurs utilisent le même libtest hors réseau. Build en 2 min 24 s, 1 326 empreintes concordantes. SHA-256 du manifeste `e88b0e79b1d3d62933add8e6d1822f2790312a4aa306d2bb1ecdde007987e054` et du libtest `356035b946e8758ea5044e7ea51449c6411599c2aa80db4c93aeb356ab064f5f`. Les deux sélecteurs de ressources figurent dans le tableau du rejeu de périmètre.

### Reprise E2 et détection de la dépendance Auth5

En reprise à froid, un instantané SQLite `DEFERRED` avec `query_only=1` détermine si les événements et projections nécessitent un pair Auth5. La transaction se ferme avant la capture du pair. Les replays suivants revalident les références sous leur propre instantané, y compris si la première observation ne nécessitait aucune capture. La durée de l’époque reste de deux secondes. Les fixtures exposent aussi la cause interne du refus de chargement du broker, avec le code public conservé.

| Recette | Statut | Observation |
|---|---|---|
| `native_read_postfinish_refusal_retains_outcome_until_explicit_rejection_drop` | PASS en 0,09 s | Refus tardif et résultat retenu, transaction terminée, `query_only=1`. |
| `cold_c3_recovers_durable_e2_on_the_same_source_without_writes` | PASS en 117,34 s | La reprise retrouve le profil E2 durable, sans nouvelle écriture. Ce scénario expirait sur la copie précédente. |
| `actual_post_oidc_causal_preparation_reopens_metadata_repin_c3_with_same_auth5` | FAIL en 62,08 s | Au chargement initial du broker, `cold_materialization` du Store 16 dure 2 015 ms et expire. La préparation OIDC reste non exécutée. |
| `actual_post_oidc_causal_preparation_rejects_different_auth5_backing_before_c3` | FAIL en 61,76 s | Le même chargement expire sur le Store 17 après 2 238 ms dans cette phase. La substitution du pair reste non exécutée. |

Build en 2 min 24 s sur la copie isolée, 1 358 empreintes concordantes. SHA-256 du manifeste `57e4e8cb6a2770a9e312bae044b1b6328f27bcfd7faf042ff8f889049ecc350c` et du libtest `e88c0b26f922f5faa1413c5e93e447916e75bbe5d66954b0dfdaaef2918deb2d`. Les quatre tests ont été lancés sans réseau et en série. Les sélecteurs du tableau précédent restent applicables.

### Comparaison du crate Native17 avec optimisation

Les 1 326 empreintes de source du [gel Business et sauvegarde](#sauvegarde-en-lecture-retenue-et-contrôle-du-gel-business) restent identiques. Les quatre sélecteurs sont rejoués avec `-C opt-level=1` sur le seul crate testé, les mêmes dépendances du profil de test et la même limite d’époque. La compilation du libtest dure 11 min 23 s. SHA-256 du manifeste `e88b0e79b1d3d62933add8e6d1822f2790312a4aa306d2bb1ecdde007987e054` et du libtest optimisé `1f1139516629c6ce86cff270cc1bfeb98c82487324439789b33930b01ff06a18`.

| Recette | Statut | Observation |
|---|---|---|
| Sauvegarde modifiée à tête inchangée | PASS en 20,10 s | Refus confirmé et comptes du Store identiques, contre 21,27 s au gel de référence. |
| Provisionnement local nominal | FAIL en 45,00 s | Le contrôle `check selected backup source` expire après 38 ms, avant lecture du fichier. Zéro opération et zéro événement du plan. |
| Ressources OIDC administrateur | FAIL en 62,40 s | Prévol 686 ms, entrée de lecture 626 ms. À la sortie, le périmètre passe puis `OidcAfterScope` expire. HTTP 503 et zéro reçu durable. |
| Ressources OIDC personnelles | FAIL en 64,45 s | Prévol 719 ms, entrée 627 ms. Même expiration, HTTP 503 et zéro reçu durable. |

Les quatre cas réutilisent les fixtures hors réseau de référence. Avec cette compilation, la reprise des parcours nominaux exige encore du travail sur les contrôles d’époque. Ce rejeu apporte des mesures pour le niveau d’optimisation 1 du crate testé.

### Prévol C3 avant l’époque d’écriture

Sur la copie isolée, le Store 17 détermine son besoin d’historique Auth5 sous une époque de lecture. Le premier instantané d’écriture vérifie le même propriétaire, la même source, la même connexion, la tête d’audit, `data_version` et `total_changes` avec `query_only=1`. Les vérifications strictes des références causales restent appliquées dans les instantanés suivants. Le format historique observé est revérifié lors de son ouverture.

| Recette | Statut | Observation |
|---|---|---|
| `native_read_postfinish_refusal_retains_outcome_until_explicit_rejection_drop` | PASS en 0,09 s | Le résultat et le mode lecture seule restent préservés sur un refus tardif. |
| `cold_causal_preflight_refuses_changed_snapshot_before_materialization` | PASS en 33,37 s | Un changement du seul `application_id` entre les deux époques est refusé avant matérialisation, avec tête d’audit identique et garde d’initialisation retenue jusqu’au relâchement du refus. |
| `cold_c3_recovers_durable_e2_on_the_same_source_without_writes` | PASS en 117,49 s | Le profil E2 est récupéré sans nouvelle écriture. |
| `actual_post_oidc_causal_preparation_reopens_metadata_repin_c3_with_same_auth5` | FAIL en 61,82 s | Le broker expire pendant la préparation du Store format 16, avant le POST. Ouverture 883 ms, préparation 1 019 ms, segment avec acquisition 2 022 ms. |
| `actual_post_oidc_causal_preparation_rejects_different_auth5_backing_before_c3` | FAIL en 62,32 s | Même blocage format 16, avant la substitution du pair. Ouverture 856 ms, préparation 1 288 ms, segment avec acquisition 2 258 ms. |

Build en 2 min 19 s, 1 358 empreintes concordantes. SHA-256 du manifeste `65f845e6d32a67debcbc0aeebc5c043b0aa5c2bbdf9b23eeeff5819334d6c175` et du libtest `b20cce6eb1d513d757453c921442b2808e562aa87261595df106e220e4ea262a`. Les cinq sélecteurs sont exécutés sur des bases de recette, en série et sans réseau. Depuis la copie de recette, utiliser `cargo test --lib --locked --offline -j1 NOM_DU_TEST -- --test-threads=1`.

### Attestation du Store historique pendant la reprise C3

Dans la copie isolée, le contrôle C3 réutilise les vérifications de la publication sur le même instantané SQLite pour les formats 14 à 16. Il vérifie explicitement le tenant, l’ancre et son reçu. Les deux contrôles entourant le chargement des projections et l’échéance d’autorité de deux secondes sont conservés. Si le dernier contrôle expire, la vérification physique du propriétaire reste prioritaire pour ces formats. La coïncidence précise entre une expiration et une dérive du propriétaire à cet endroit n’est pas couverte par les tests rejoués.

| Recette | Statut | Observation |
|---|---|---|
| `legacy_postcommit_expiry_returns_the_same_startup_store` | PASS en 2,82 s | L’expiration après commit rend le même Store avec son état de reprise. |
| `native_read_postfinish_refusal_retains_outcome_until_explicit_rejection_drop` | PASS en 0,09 s | Rejet tardif et lecture seule préservés. |
| `cold_causal_preflight_refuses_changed_snapshot_before_materialization` | PASS en 33,17 s | Dérive SQLite à tête d’audit inchangée refusée entre les deux époques. |
| `cold_c3_recovers_durable_e2_on_the_same_source_without_writes` | PASS en 119,69 s | Profil E2 récupéré sans écriture supplémentaire. |
| `actual_recorded_cold_parts_reach_current_owner_gateway_without_fresh_origin` | FAIL en 35,68 s | Échéance dans `union_redactor17` sur le format 17, avant le contrôle historique visé. Le segment dure 2 132 ms. |
| `actual_post_oidc_causal_preparation_reopens_metadata_repin_c3_with_same_auth5` | FAIL en 63,75 s | Échéance sur le format 16 avant le POST. Ouverture 948 ms, préparation 1 075 ms, segment avec acquisition 2 144 ms. |
| `actual_post_oidc_causal_preparation_rejects_different_auth5_backing_before_c3` | FAIL en 64,43 s | Même point d’arrêt avant la substitution du pair. Ouverture 863 ms, préparation 1 077 ms, segment avec acquisition 2 056 ms. |

Build en 2 min 23 s. Les 1 358 empreintes concordent avant et après compilation. Seul `src/audit/control17/startup/cold.rs` diffère du prototype précédent. Le manifeste a pour SHA-256 `2fb06788bd191db641d8e9d3aef4053cc533bf8dd224ebd25baeff3995c3ea8a` et le libtest `4872122a44839ce8e9ef88b1ad84a9f14e30b8e8f5493a4a25c2e642209c0920`. Le dossier privé de rejeu contient `results.json`, les sorties par sélecteur et les manifestes. Les cas tournent en série, avec le réseau interdit, en suivant la commande de rejeu de la section précédente. Ces mesures ne démontrent pas de gain stable sur les deux scénarios causaux.

### Instantanés de lecture fixés à la reprise

Le prototype fixe `main=Read` avant la vérification de la publication lors de la fusion des masques. Les deux transactions d’ouverture du format 16 revérifient chacune l’ancre une fois sur leur propre instantané. Les réglages locaux vérifient toujours le propriétaire après l’observation, y compris sur erreur locale. Les relectures physiques, la validation après commit et l’échéance restent présentes. Une erreur SQLite peut faire remonter un diagnostic différent plus tôt pendant la fusion des masques.

| Recette | Statut | Observation |
|---|---|---|
| `native_startup_memory_updates_preserve_masks_policies_and_canonical_head` | PASS en 22,56 s | Masques, politiques et tête d’audit vérifiés. |
| `native_startup_memory_postcheck_detects_control_mutation_and_retains_original_store` | PASS en 44,33 s | Le contrôle après commit constate la modification et conserve le Store d’origine. |
| `native_startup_memory_expired_postcheck_restores_masks_policies_and_original_cause` | PASS en 46,73 s | L’expiration conserve la cause initiale et restaure les masques et politiques. |
| `cold_causal_preflight_refuses_changed_snapshot_before_materialization` | PASS en 33,08 s | Dérive refusée entre la lecture et la matérialisation. |
| `cold_prepared_store_refuses_drift_and_keeps_initialization_guard` | PASS en 34,37 s | Dérive refusée avant publication. Écritures fermées et verrou d’initialisation conservé. |
| `actual_recorded_cold_parts_reach_current_owner_gateway_without_fresh_origin` | PASS en 43,42 s | L’état du gateway accepte la reprise A17/B16. Les Stores ne reprennent pas comme des exécutions neuves. L’incarnation de B16 est durable. |
| `actual_post_oidc_causal_preparation_reopens_metadata_repin_c3_with_same_auth5` | FAIL en 64,84 s | Échéance au format 16 avant le POST. Ouverture 931 ms, préparation 1 100 ms, segment avec acquisition 2 157 ms. |
| `actual_post_oidc_causal_preparation_rejects_different_auth5_backing_before_c3` | FAIL en 66,08 s | Échéance au format 16 avant la substitution du pair. Ouverture 969 ms, préparation 1 145 ms, segment avec acquisition 2 245 ms. |

Build en 2 min 23 s et exécution des huit tests en série, réseau interdit pour les deux étapes. Les 1 358 empreintes concordent avant et après compilation. SHA-256 du manifeste `dbe0567aa827c26bef71baf99d9f2ed33202efdb71c9a3b0625356a2d0050162` et du libtest `65ef305cd5b541d1c59be495fde913c2be4eefbba881cda07dc388f86b743bd5`. La reprise mixte passe sur cette exécution. Les deux mesures causales ne démontrent pas de gain. Le conducteur filtre encore les sous-phases internes de préparation dans les erreurs de bootstrap.

### Prévalidation en lecture du Store format 16

Sur le prototype, la première validation complète du format physique 16 utilise une époque de lecture, avec la connexion d’origine et `query_only=1`. La comparaison sous époque d’écriture porte sur le propriétaire, le format et les noms physiques, le verrou d’initialisation, la connexion, la tête, `data_version`, `schema_version`, `total_changes` et les métadonnées de `main`, WAL et journal. La publication et ses fichiers sont revérifiés. Le rejeu complet après bascule et le refus de toute divergence sont conservés. Le conducteur de recette extrait les sous-phases chronométrées des erreurs de bootstrap et de réouverture sans imprimer leurs messages bruts.

| Recette | Statut | Observation |
|---|---|---|
| `cold_format16_preflight_refuses_changed_snapshot_before_materialization` | PASS en 33,64 s | Modification de `application_id` à tête constante refusée, verrou conservé jusqu’à la libération du rejet. |
| `cold_causal_preflight_refuses_changed_snapshot_before_materialization` | PASS en 33,12 s | Le refus correspondant reste valide au format 17. |
| `actual_recorded_cold_parts_reach_current_owner_gateway_without_fresh_origin` | PASS en 43,41 s | Reprise mixte A17/B16 et incarnation durable vérifiées. |
| `actual_post_oidc_causal_preparation_reopens_metadata_repin_c3_with_same_auth5` | FAIL en 65,07 s | Le format 16 passe. Au format 17, l’observation après la transaction de lecture expire après 342 ms. Ouverture 899 ms, préparation 1 076 ms. Aucun POST. |
| `actual_post_oidc_causal_preparation_rejects_different_auth5_backing_before_c3` | FAIL en 65,54 s | La même observation expire après 349 ms au format 16. Ouverture 831 ms, préparation 1 112 ms. Le pair divergent est encore hors d’atteinte. |

Build en 2 min 19 s et cinq tests en série, réseau interdit. Les 1 358 empreintes concordent avant et après compilation. SHA-256 du manifeste `731624812671d6f671de2f8abd4de96d4bfd3237463e894f69647a3d97b11d37` et du libtest `a0ff4cb00a271ca0cd5ee4d50a1efd68c9c15dd6d3e897915b787e589ffb478f`. L’échéance reste de deux secondes pour chaque époque. Le temps de segment inclut le prévol dans une autre époque.

### Contrôle du Store préparé avec les écritures fermées

La préparation capte la tête, les versions SQL, le compteur de changements et les métadonnées physiques dans la transaction des projections. Après commit elle ferme les écritures et compare ces faits. Le contrôle complet suivant utilise une époque de lecture séparée, avec la même connexion, le même verrou et `query_only=1`. Auth5, lorsqu’il est requis, est relu sous cette époque. Le contrôle de publication sous époque d’écriture reste obligatoire avant la réouverture des écritures.

| Recette | Statut | Observation |
|---|---|---|
| `cold_prepared_store_refuses_drift_and_keeps_initialization_guard` | PASS en 34,39 s | Une modification d’`application_id` à tête inchangée est refusée. Le refus garde la vraie connexion en lecture seule et retient le verrou jusqu’à sa libération. |
| `cold_format16_preflight_refuses_changed_snapshot_before_materialization` | PASS en 33,65 s | Refus avant matérialisation et maintien du verrou sur le format 16. |
| `cold_causal_preflight_refuses_changed_snapshot_before_materialization` | PASS en 32,98 s | Refus correspondant sur le format 17. |
| `actual_recorded_cold_parts_reach_current_owner_gateway_without_fresh_origin` | PASS en 44,31 s | Reprise mixte A17/B16 et incarnation durable vérifiées. |
| `actual_post_oidc_causal_preparation_reopens_metadata_repin_c3_with_same_auth5` | FAIL en 81,83 s | Le bootstrap franchit la matérialisation et la validation des Stores. `union_redactor17` expire après 2 107 ms, avant le POST. |
| `actual_post_oidc_causal_preparation_rejects_different_auth5_backing_before_c3` | FAIL en 67,89 s | Au format 17, la vérification après chargement des projections expire après 382 ms. Ouverture 1 040 ms, préparation 863 ms. Le pair divergent reste hors d’atteinte. |

Le premier build a refusé un alias Rust `Result` ambigu. Après correction, build en 2 min 20 s et six tests en série sans réseau. Les 1 358 empreintes concordent avant et après compilation. SHA-256 du manifeste `5ad6e080a685f0a512b751c88fcca71d2a45b73165d47a77d29e1de824a8b3e5` et du libtest `7f34d694f57ce4268e3d85aec8a16fdc43d82beb6705c703ac84d841c9abe271`. Chaque époque garde la borne de deux secondes. Les deux scénarios causaux restent en échec sur ce prototype.

### Masques à froid et stabilité de la lecture des projections

Les Stores 17 repris à froid reçoivent les masques dans l’ordre canonique avant leur construction. Le coordinateur omet leur seconde fusion uniquement pour ces sources après validation de leur publication. Le cas chaud garde sa fusion et le binding des politiques reste obligatoire. Dans la transaction des projections, le premier rejeu reste complet. Avant le commit, le contrôle revérifie l’époque, les fichiers de la publication, le pair Auth5 si requis, le verrou, le mode de lecture et l’égalité de l’instantané. Le rejeu complet sous l’époque suivante reste en place.

| Recette | Statut | Observation |
|---|---|---|
| `cold_prepared_store_refuses_drift_and_keeps_initialization_guard` | PASS en 34,36 s | Refus d’une modification à tête constante et maintien de la connexion en lecture seule avec son verrou. |
| `native_startup_memory_updates_preserve_masks_policies_and_canonical_head` | PASS en 22,27 s | La fusion chaude et les politiques conservent la preuve de la tête SQLite. |
| `actual_recorded_cold_parts_reach_current_owner_gateway_without_fresh_origin` | PASS en 46,24 s | L’ordre de deux masques chevauchants est vérifié sur le Store repris. La reprise mixte et l’incarnation durable restent observées. |
| `actual_post_oidc_causal_preparation_reopens_metadata_repin_c3_with_same_auth5` | FAIL en 79,86 s | Le raccordement des politiques expire en format 17. Phase 1 947 ms, segment d’autorité 2 001 ms. Aucun POST. |
| `actual_post_oidc_causal_preparation_rejects_different_auth5_backing_before_c3` | FAIL en 78,84 s | Même point d’arrêt. Phase 1 953 ms, segment d’autorité 2 009 ms. Le refus du pair divergent reste hors d’atteinte. |

Build en 2 min 25 s. Cinq tests en série, réseau interdit. SHA-256 du manifeste des 1 358 fichiers `700aa6d86b5f796e48445fd074f20f166925331cd59c447d56ed401136632a61` et du libtest `ca83d01cc00f688279443a109a53692293fbe40061d77bf26cbe06647ecebe9d`. La limite d’époque est conservée. Le refus de dérive rejoué porte sur l’intervalle après préparation et ne vise pas une modification pendant le calcul des projections.

### Instantané de lecture pour le raccordement des politiques

La mise à jour des politiques établit `main=Read` avant le premier rejeu de publication. Le contrôle avant commit exige le même mode `Read` et un compteur `total_changes` inchangé. Le contrôle hors transaction après commit, la restauration sur refus et le binding des politiques restent obligatoires. La limite de deux secondes reste en place.

| Recette | Statut | Observation |
|---|---|---|
| `native_startup_memory_updates_preserve_masks_policies_and_canonical_head` | PASS en 22,22 s | Les deux mises à jour gardent la même tête et conservent masques et politiques. |
| `native_startup_memory_postcheck_detects_control_mutation_and_retains_original_store` | PASS en 42,80 s | La dérive du contrôle reste détectée après commit et les valeurs précédentes sont restaurées. |
| `native_startup_memory_expired_postcheck_restores_masks_policies_and_original_cause` | PASS en 45,89 s | L’expiration garde la cause typée et restaure les deux états de configuration. |
| `actual_post_oidc_causal_preparation_reopens_metadata_repin_c3_with_same_auth5` | FAIL en 81,11 s | Le raccordement des politiques passe. `incarnation_append17` expire après 2 031 ms dans le bootstrap, avant le POST. |
| `actual_post_oidc_causal_preparation_rejects_different_auth5_backing_before_c3` | FAIL en 81,25 s | Même point d’arrêt en 2 041 ms, avant le test du pair divergent. |

Le premier build échoue à l’édition de liens avec `No space left on device`. Après compression vérifiée de deux anciens exécutables et de 17 objets de compilation privés, le même gel compile en 2 min 25 s. Les 1 358 empreintes concordent. Les cinq tests tournent en série, réseau interdit. SHA-256 du manifeste `5260ad52e174e3fe43ced45357c877b65c3143c5176def7c0a2928db10a89864` et du libtest `b4dfca4f90abcd39ed444e307a7990726458439717927c308ef152695c052d5c`. La trace localise la phase d’ajout de l’incarnation, sans établir si un commit a eu lieu avant le refus.

### Incarnation avant et après le commit

Les diagnostics distinguent l’arrêt avant commit, le résultat du commit non confirmé et le refus après commit. La cause typée reste accessible. Le postcheck `MetadataRepin` utilise un `Deferred Read` sur la même connexion et avec les mêmes verrous, puis compare le format et les faits SQL et physiques après la fin du Read. Auth5, le propriétaire et les fichiers privés de publication sont revérifiés. Le writer garde son CAS et ses vérifications avant commit.

| Recette | Statut | Observation |
|---|---|---|
| `native_startup_writer_keeps_exact_idempotence_and_real_separate_retention` | PASS en 24,70 s | L’opération identique retrouve sa valeur, avec une seule écriture durable et une rétention d’autorité séparée. |
| `native_startup_writer_expiration_rolls_back_and_keeps_original_cause_store_owner` | PASS en 23,99 s | L’expiration est identifiée avant commit. Aucune ligne d’incarnation supplémentaire. |
| `native_startup_writer_postcommit_refusal_keeps_canonical_debt_without_publishing_authority` | PASS en 22,57 s | Une mutation du contrôle après commit refuse le reçu. Le journal et le plancher restent durables. |
| `native_startup_writer_postcommit_expiry_retains_one_durable_incarnation` | PASS en 24,06 s | L’expiration est identifiée après un commit connu. Une seule incarnation reste durable et l’owner ne devient pas invalide. Aucun reçu ni autorité d’exécution ne sort. |
| `actual_post_oidc_causal_preparation_reopens_metadata_repin_c3_with_same_auth5` | FAIL en 80,69 s | `incarnation append before commit` expire après 2 039 ms. Aucun POST. |
| `actual_post_oidc_causal_preparation_rejects_different_auth5_backing_before_c3` | FAIL en 84,67 s | Même branche avant commit après 2 039 ms. Le pair divergent n’est pas encore testé. |

Build en 2 min 27 s et six tests en série, réseau interdit. 1 358 empreintes concordantes. SHA-256 du manifeste `f7ca6392aa6a6772e1b255787557142fb50c82da212c9578cbc3328aa18a1af0` et du libtest `9b4e1fc7a84cdcbc19437e6bc1213ee2d202d529438f16af7e6bcf8f424ed0d4`. Le writer ne tente pas de commit à ce point d’arrêt. Les deux scénarios identifient la phase, sans compter les lignes durables des autres sources du bootstrap.

### Rejeu des bords du writer d’incarnation

Les validations consécutives de l’époque et de la source sont regroupées aux mêmes bords. La vérification de la publication commence toujours par cette validation. Les contrôles SQLite, CAS, politiques et Auth5, ainsi que les séparations de commit et de rétention, sont conservés.

| Recette | Statut | Observation |
|---|---|---|
| `native_startup_writer_cas_refusal_keeps_the_original_canonical_head` | PASS en 22,24 s | Une révision inattendue laisse la tête intacte. |
| `native_startup_writer_revalidates_pending_projection_before_commit_and_rolls_back` | PASS en 22,17 s | Une projection divergente refuse le commit et ne laisse pas de ligne durable. |
| `native_startup_writer_keeps_exact_idempotence_and_real_separate_retention` | PASS en 24,25 s | L’opération identique conserve l’unique écriture et la rétention séparée. |
| `actual_post_oidc_causal_preparation_reopens_metadata_repin_c3_with_same_auth5` | FAIL en 83,80 s | Commit du writer format 17 connu. La validation après commit expire après 85 ms, avant retour du reçu et avant POST. |
| `actual_post_oidc_causal_preparation_rejects_different_auth5_backing_before_c3` | FAIL en 84,25 s | Commit du writer format 17 connu. Même refus après 86 ms, avant test du pair divergent. |

Build en 2 min 24 s et cinq tests en série, réseau interdit. 1 358 empreintes concordantes. SHA-256 du manifeste `582370509297f7e0c566554d5e17bf367b248179a98b077136c5caa8e7ea8450` et du libtest `e665b84bab2fbaa5da9af2ae934b2599d70220ca2efa57b78eac03b53308b045`. Le registre de diagnostics situe l’expiration après un vrai commit pour cette source. Il ne qualifie ni le retour d’un reçu ni la réussite de la reprise causale.

### Expiration réelle du contexte dans Chromium

Le conducteur `ttl` passe en 304,21 s sur le gateway Rust de recette avec 24 entrées Workers et 60 événements synthétiques. Le contexte expire après 300 s. Le navigateur retire les données et propose la reprise après une seule lecture d’identité, sans actualisation manuelle. Il attend la confirmation au clavier avant de lire les ressources protégées. Le gateway fournit un nouvel identifiant de contexte. Le filtre est réappliqué aux mêmes lignes et le bail de broker apparaît comme périmé. Le bail et les workers proviennent de la fixture SQLite.

Node 24.22.2, Playwright 1.66.0-alpha-2026-11-18 et Chromium 148.0.7778.96. Réseau limité à IPv4/IPv6 loopback. SHA-256 du conducteur `de9147dc57ff2f858e6b165bbc68bd28d07279eeee0ab7b7b758b9b73b18e4d2`, de `supervision_stack` `5c433a4069246d6b48b792e5f7be17b14025d737f290d8232bcb39991a568898` et du manifeste du bundle et des aides `75abaa1b30ad38519fca61d1b651803b05f2f9c221b1cd61306a5b06b511f4e1`. Le conducteur conserve le HTML et le PNG assainis lorsqu’un échec survient avec le dashboard authentifié encore visible. Le rejeu est décrit dans `tests/e2e/README.md` de la candidate.

### Relecture de l’incarnation à froid sous une époque de lecture

Le writer confirme le commit, met à jour le plancher et le cache, puis vérifie encore l’époque d’écriture d’origine avant de rendre le témoin privé. Le démarrage à froid peut alors relire le même reçu sur le même Store et la même connexion sous une époque de lecture. Cette lecture lie l’opération, la révision et les faits SQLite du commit. La rétention de l’autorité intervient dans une écriture distincte. Un refus du writer empêche de remettre le témoin.

Le gel du prototype compile en 2 min 27 s. SHA-256 du manifeste des 1 358 fichiers `a5bc7bc83eff2b526fc1c66a2702b4d0b32b23ea9ea9f316309a0c3a06291bd6` et du libtest `b4195a7bb4dac1e4e08a55b98750ebf9146836e28342587d7c4cf4514675d5a8`. La première exécution rencontre des erreurs d’E/S lors de la préparation des Stores. Le témoin d’idempotence et l’écriture du registre de résultats renvoient `ENOSPC`. Cette exécution est `BLOCKED` par le disque. Deux anciens exécutables de recette ont été compressés et leurs empreintes vérifiées après décompression. Le nouveau rejeu utilise le même libtest et le même manifeste.

| Recette | Statut | Observation |
|---|---|---|
| `native_startup_writer_cold_read_confirms_same_operation_and_rejects_snapshot_drift` | PASS en 50,40 s | Même reçu, UUID et tête dans le cas positif. Une modification de la SQLite après création du témoin refuse la relecture, même avec une tête inchangée. La relecture n’installe aucune autorité. |
| `native_startup_writer_postcommit_expiry_retains_one_durable_incarnation` | PASS en 26,38 s | Quand l’époque d’écriture expire après commit, l’incarnation reste durable et le writer refuse le témoin. |
| `native_startup_writer_keeps_exact_idempotence_and_real_separate_retention` | PASS en 26,84 s | Le chemin chaud conserve son reçu idempotent et la rétention séparée. |
| `actual_post_oidc_causal_preparation_reopens_metadata_repin_c3_with_same_auth5` | FAIL en 83,47 s | La préparation dépasse son époque de deux secondes dans `configure_policies17`, avant la nouvelle relecture et le POST. Phase de 1 944 ms et segment de 2 000 ms. |
| `actual_post_oidc_causal_preparation_rejects_different_auth5_backing_before_c3` | FAIL en 83,90 s | Même phase. Segment de 2 006 ms, dont 1 948 ms dans `configure_policies17`. Le contrôle du pair divergent reste hors d’atteinte. |

La revue indépendante confirme que la relecture refuse la dérive après capture. Elle relève un intervalle à couvrir entre le commit et la première capture, qui doit encore être raccordée à la tête du writer. Aucun des deux chemins OIDC complets n’atteint la relecture dans ce rejeu.

Depuis ce gel du prototype, la vérification ciblée du reçu et du refus de dérive se rejoue en série.

```sh
cargo test --locked --offline -j 1 --lib native_startup_writer_cold_read_confirms_same_operation_and_rejects_snapshot_drift -- --test-threads=1
```

### Réglages de démarrage dans deux lectures SQLite

La vérification après mise à jour mémoire passe par un deuxième instantané `Read` de la même connexion. Les deux lectures conservent le verrou et comparent le format, la tête, les compteurs de connexion, les identités et les horodatages des fichiers SQLite. Après le second commit, le contrôle revérifie le pair d’authentification, les faits SQLite et le propriétaire. L’époque d’écriture d’origine et la restauration des valeurs en cas de refus restent requises. Le témoin de l’incarnation compare aussi la tête initialement capturée après commit au plancher enregistré par sa transaction.

Le prototype compile en 2 min 32 s. Les 1 358 empreintes concordent. SHA-256 du manifeste `184105926d8f78caa59d7c666fa2e759759166f5ce93386ae5a000f8afefd373` et du libtest `4bccebb3fafd4ba6cea5f03a610898d7b28c800b08c0b2163ef6f8c36cfa25a4`.

| Recette | Statut | Observation |
|---|---|---|
| `actual_post_oidc_causal_preparation_reopens_metadata_repin_c3_with_same_auth5` | FAIL en 108,31 s | Le chargement C3 et la création du service passent. Le premier backup du grant refuse l’administration courante avant le POST. |
| `actual_post_oidc_causal_preparation_rejects_different_auth5_backing_before_c3` | FAIL en 109,57 s | Même refus pendant la préparation commune. La vérification du pair divergent reste à exercer. |
| `native_startup_writer_cold_witness_refuses_a_later_committed_head` | PASS en 21,97 s | Un événement de recette avance la tête après commit. Le writer refuse le témoin et conserve le plancher initial. |
| `native_startup_writer_postcommit_expiry_retains_one_durable_incarnation` | PASS en 23,47 s | L’expiration de l’écriture d’origine conserve une incarnation durable et empêche le retour du témoin. |
| `native_startup_memory_refuses_same_head_drift_and_restores_bound_policies` | PASS en 41,97 s | Une dérive SQLite à tête inchangée est refusée après chacun des deux commits. La liaison canonique précédente est restaurée. |
| `native_startup_memory_postcheck_detects_control_mutation_and_retains_original_store` | PASS en 41,89 s | La mutation du fichier de contrôle conserve la cause de refus et restaure les réglages précédents sur le même Store. |

Les six tests tournent en série, réseau interdit. Le code `source17_current_installation_admin_required` suit le rechargement du registre et précède la préparation du backup. Il couvre plusieurs causes possibles. La cause interne du refus n’est pas établie sur ce gel.

Commandes depuis ce gel du prototype, sur un seul job Cargo et avec les tests exécutés en série.

```sh
cargo test --locked --offline -j 1 --lib \
  native_startup_memory_refuses_same_head_drift_and_restores_bound_policies -- --test-threads=1
cargo test --locked --offline -j 1 --lib \
  actual_post_oidc_causal_preparation_ -- --test-threads=1
```

#### Cause du refus administrateur lors du backup

Le test conserve une catégorie et une phase issues du refus de la tentative réelle. Le harnais affiche uniquement des libellés fixes. Le rejeu nominal échoue en 109,49 s avec `admin_phase=store_credential_postcheck admin_cause=human_authority_epoch_expired`. La transaction de vérification est terminée. Le contrôle final de l’époque de lecture refuse la preuve d’administrateur avant la préparation du backup et le POST. Le cas du pair divergent garde son résultat sur le gel précédent.

Le gel du diagnostic compile en 2 min 28 s. Un seul test est exécuté, réseau interdit. 1 358 empreintes concordent. SHA-256 du manifeste `358d2c600b8fb1282e7a1de592975f6a792660f2724f08f57355b8d812f4bddf` et du libtest `7d28a4c15049c50ae106e3dd1a6ef5a32f0180bed758765eb8b3fc04e64d2d33`.

Le gel suivant évite une troisième reconstruction de l’instantané d’autorité dans la vérification de l’administrateur. Le contrôle Business compare encore l’instantané au journal dans la même transaction et garde la validation du grant, de l’horloge et de la preuve finale. Le rejeu nominal donne `FAIL` en 109,21 s avec le même refus `store_credential_postcheck` et `human_authority_epoch_expired`. Build en 2 min 30 s, 1 358 empreintes concordantes, réseau interdit. SHA-256 du manifeste `8c34bcbc409d98cb36d999e5edac9762c3a7fb63a3031fb7fae197e3dc65453d` et du libtest `40a9928ff66cff92dcf2076950b55943b47cc230e032a35125acd6a00a53776c`.

#### Relectures pendant l’ouverture de la transaction administrateur

L’authentification InstallationAdmin sous époque de lecture et avec Auth5 conserve la première vérification durable après l’ouverture de la transaction. Pour les contrôles d’ouverture suivants, une connexion et un instantané SQLite identiques permettent de réutiliser le résultat du rejeu. Chaque garde vérifie toujours l’époque, la source et le fichier courant. Une divergence de tête, de compteur, de format ou de schéma impose le rejeu complet. Le grant, le commit et le contrôle après transaction gardent leurs propres vérifications.

Le gel compile en 2 min 30 s. SHA-256 du manifeste des 1 358 fichiers `080b1ebdfb35a2fad9aa684691c20244babb045fec2bbcd006111034bc67959a` et du libtest `e4378191df098f1a4570a35944c329e053df259380097522be2ebffd7c002445`.

| Recette | Statut | Observation |
|---|---|---|
| `actual_post_oidc_causal_preparation_reopens_metadata_repin_c3_with_same_auth5` | FAIL en 108,94 s | Le nominal expire toujours pendant `store_credential_postcheck` avant le backup et le POST. |
| `actual_retained_backing_replacement_keeps_the_permanent_failure_path` | PASS en 0,41 s | Le remplacement du backing retient le refus permanent. |
| `temporary_objects_refuse_until_dropped_without_poisoning_the_source` | PASS en 0,35 s | La forme SQLite temporaire est refusée et la source redevient lisible après retrait de l’objet. |

Les trois tests tournent en série, réseau interdit. La durée globale de la recette comprend le bootstrap et ne mesure pas le coût de l’authentification seule.

#### Temps mesurés de l’authentification administrateur

Le test nominal arme une trace privée de la seule tentative d’authentification du premier backup. Les offsets monotones sont mesurés depuis l’entrée dans le Store, après acquisition et liaison à l’époque d’origine.

| Jalon | Temps cumulé |
|---|---:|
| Capture Auth5 terminée | 517 ms |
| Contexte causal prêt | 616 ms |
| Garde de connexion acquis | 641 ms |
| Ouverture SQLite terminée | 1 001 ms |
| Grant vérifié | 1 541 ms |
| Commit et sortie du garde | 1 897 ms |
| Contrôle final et refus d’expiration | 2 310 ms |

Le contrôle final dure 413 ms. Le résultat reste `FAIL` avant le backup et le POST, après 108,38 s au total. Un seul test, réseau interdit. Build en 2 min 36 s et 1 358 empreintes concordantes. SHA-256 du manifeste `1f090771629a176bcc87ac67dcebc8cef3fa75e77e0b3d017b1ffb91d15d04d5` et du libtest `992c90b50091b92326b4f2c6a682c4d2d6835d492281becc05c1280688aec02e`.

#### Relectures groupées avant le contrôle final OIDC

Trois correctifs revus indépendamment sont compilés dans la copie Native17 isolée. Ils regroupent le contrôle des deux jetons d’une même époque pendant la capture Auth5, réutilisent le contrôle du propriétaire effectué à la création du jeton d’historique et retiennent l’enregistrement de fermeture vérifié sous la même garde. Le test nominal mesure 385 ms pour la capture Auth5 et 2 070 ms entre l’entrée du Store et la fin du contrôle final. La mesure précédente donnait respectivement 517 ms et 2 310 ms. Le contrôle refuse toujours la preuve administrateur avec `store_credential_postcheck` et `human_authority_epoch_expired`. Le premier backup et le POST restent en aval.

| Recette | Statut | Observation |
|---|---|---|
| `exact_closure_domains_and_foreign_source_tokens_are_rejected` | PASS en 0,14 s | Un jeton d’un autre propriétaire reste refusé même avec un identifiant de source égal. |
| `an_expired_epoch_does_not_latch_unrelated_future_operations` | PASS en 0,09 s | L’expiration prime sur le refus du jeton étranger et les opérations suivantes conservent leur propre époque. |
| `actual_post_oidc_causal_preparation_reopens_metadata_repin_c3_with_same_auth5` | FAIL en 100,97 s | Le contrôle final refuse encore l’époque expirée avant le premier backup. |
| `actual_post_oidc_causal_preparation_rejects_different_auth5_backing_before_c3` | NOT_RUN | Le nominal n’atteint pas la préparation du POST. |

Les tests tournent en série et réseau interdit. Build en 2 min 38 s. SHA-256 du manifeste des 1 358 fichiers `e9d5f5bbab51d7f0bd889263a75e5cc87776484dd8479be750032ebbf19af9a4`, du libtest `aeb9039940b466bc8904c529abc0cea122fe94571b1b76df51d1e0281622e7b6` et du rapport `176b6bea02d015725b9b18741d0590f78c8d0df1ae54bd06b9be92cf72e71564`. Ces résultats concernent le prototype isolé.

#### Relecture groupée de la couverture et coût du corpus OIDC

La copie Native17 regroupe le contrôle d’admission et le scan physique des sources dont la couverture est comparée à la publication. Elle garde les contrôles physiques qui encadrent le registre et la vérification terminale du propriétaire. Le test d’appartenance confirme l’ordre des descripteurs et le refus d’une source absente. Les deux régressions Auth5 passent, en 0,13 s et 0,08 s.

| Jalon après la transaction administrateur | Temps depuis l’entrée du Store |
|---|---:|
| Sortie de transaction | 1 710 ms |
| Premier contrôle du propriétaire | 1 735 ms |
| Fin de revalidation des fichiers, du corpus et de la génération | 2 035 ms |
| Refus `human_authority_epoch_expired` | 2 036 ms |

La revalidation des fichiers, du corpus et de la génération prend 300 ms. L’époque est déjà expirée quand le contrôle terminal s’exécute. Le test nominal reste `FAIL` en 102,02 s avant la sauvegarde et le POST. Le scénario du pair Auth5 différent reste `NOT_RUN` sur ce gel. La mesure part de l’entrée dans le Store, après l’acquisition de l’époque de deux secondes.

Compilation en 2 min 32 s, 1 358 fichiers vérifiés et réseau interdit. SHA-256 du manifeste `e92dd01974c6e7220cb741d2744bd45a71d72260c4aedc76e11c3dde432ae070`, du libtest `f468192ed3c41bb1b6b821fb5683af4baca53797aa95a564c0ee3577a19539eb` et du rapport `2d231c8b6244021d411f364807781d11b790c526a0022c5a0d8fd828e1924aa9`.

#### Vérification de types des deux contrôles OIDC suivants

Le gel suivant réutilise le même instantané SQLite pour les deux comparaisons durables précommit de l’administrateur sous Auth5. Les deux gardes Auth5, l’autorité dynamique, le backing et les contrôles postcommit restent en place. Le contrôle de liaison d’un fichier de la publication conserve une seule relecture au point où l’observateur la faisait déjà.

`cargo check --locked --offline --lib --profile test` passe en 49,98 s, réseau interdit et 1 358 fichiers vérifiés. Le manifeste SHA-256 est `a2a7983ed880a5e8fb2f9e414c77c62ef8c87f1338ba9664a4d89de8c4f6150a`.

La production d’un exécutable de test échoue deux fois à l’édition de liens LLVM faute d’espace disque. Le premier processus Cargo retourne zéro malgré l’erreur et ne laisse qu’un fichier de 896 octets non exécutable. Le conducteur vérifie désormais aussi le diagnostic LLVM. Le nominal OIDC et le cas du pair divergent restent `NOT_RUN` sur ce gel.

#### Rejeu du même gel OIDC après le lien sans stripping

La recopie par `rust-objcopy` échoue faute d’espace disque après le premier lien. L’option finale `-C strip=none` permet de produire le Mach-O de test de 413 575 888 octets. Les 1 358 sources et le profil de test non optimisé sont ceux du gel précédent. Le gel compile en 2 min 30 s, réseau interdit. SHA-256 du manifeste `a2a7983ed880a5e8fb2f9e414c77c62ef8c87f1338ba9664a4d89de8c4f6150a`, du libtest `3474c48420d88bf7d6fa363472c05cdec1ae799f8a4f9f2ac5fe607abe232238` et du rapport `e821659d7434de0407cc4c5604b66c3d8fa6df9bd35b56378187c0b5258ef135`.

| Recette sur ce binaire | Statut |
|---|---:|
| `exact_closure_domains_and_foreign_source_tokens_are_rejected` | PASS en 0,15 s |
| `an_expired_epoch_does_not_latch_unrelated_future_operations` | PASS en 0,09 s |
| `actual_retained_backing_replacement_keeps_the_permanent_failure_path` | PASS en 0,38 s |
| `temporary_objects_refuse_until_dropped_without_poisoning_the_source` | PASS en 0,38 s |
| `actual_post_oidc_causal_preparation_reopens_metadata_repin_c3_with_same_auth5` | FAIL en 101,65 s |
| `actual_post_oidc_causal_preparation_rejects_different_auth5_backing_before_c3` | NOT_RUN |

En partant de l’entrée dans le Store, la capture Auth5 finit à 387 ms, le grant à 1 329 ms et le commit à 1 692 ms. Le contrôle des fichiers, du corpus et de la génération se termine à 2 014 ms. Le contrôle final constate l’époque expirée, avant le backup et le POST. Le cas du pair divergent reste en aval de ce prérequis.

#### Essai d’un rejeu groupé lors de la preuve du grant OIDC

Le gel `8af003e0` réutilise l’instantané SQLite au second contrôle durable du grant dans la transaction InstallationAdmin. Le premier lien reçoit `ENOSPC` dans `ld`. L’archivage vérifié de dix-sept objets du premier lien et d’une ancienne bibliothèque de test libère assez d’espace pour une compilation de 2 min 28 s sans stripping et sans réseau.

Les cinq régressions ciblées passent. Elles couvrent le jeton d’un autre propriétaire, l’époque expirée, le remplacement du backing, les objets temporaires et une connexion sur une copie identique du Store. Le nominal échoue encore avant le backup et le POST avec `human_authority_epoch_expired`. Le grant finit à 1 355 ms, le commit à 1 722 ms et le contrôle terminal à 2 048 ms depuis l’entrée dans le Store. Le cas du pair divergent reste `NOT_RUN` et la recette nominale totale dure 101,88 s.

Ce rejeu ne démontre aucun gain global sur le nominal. La copie de travail du prototype revient au manifeste `a2a7983e`. Le patch, les 1 358 empreintes, le binaire et le rapport de l’essai restent disponibles. SHA-256 du manifeste `8af003e0289e289c107a611981f96ec3b624e4bfa4d4c46c1879acd05d21b009`, du libtest `cde728f94df55cacdd85e54a893ebf43f2f543312971e499a7a18cc89e73ccc0` et du rapport `28928f247c2b8ef72e8e10dbbe95930365c582332a8defd7bfb851399a5121f6`.

### Séparation navigateur après déconnexion

Le conducteur `isolation` vérifie que le passage de l’administration au contexte personnel retire les lignes et le filtre du premier contexte. Il obtient ensuite du vrai gateway un HTTP 200 pour la flotte A via un unique GET loopback sans redirection. Il retient cette réponse et vérifie que la déconnexion annule exactement cette requête, avec `net::ERR_ABORTED`. B se connecte, reçoit un HTTP 200 lié à son propre contexte et affiche son seul worker. Les identifiants de A sont absents du rendu de B. Les workers et événements sont des données synthétiques de la SQLite de recette.

Chromium 148.0.7778.96, Playwright 1.66.0-alpha-2026-11-18 et Node 24.22.2 donnent `PASS` en 5,46 s. Réseau limité à IPv4/IPv6 loopback. SHA-256 du conducteur `66cab4f77c84b1bc61c00213da591b35dcfbdb292ef1692597c663b81dcd1837`, du binaire `supervision_stack` `5c433a4069246d6b48b792e5f7be17b14025d737f290d8232bcb39991a568898` et du manifeste bundle et aides `75abaa1b30ad38519fca61d1b651803b05f2f9c221b1cd61306a5b06b511f4e1`. Le conducteur réutilise la capture assainie HTML/PNG sur dashboard authentifié en cas d’échec.

### Ouverture du dashboard avec des droits partiels

Le scénario `partial` traverse les trois contextes Workers seuls, Brokers seuls et Ressources personnelles sur le vrai gateway et dashboard Rust. Il vérifie la seule lecture de supervision accordée dans chaque contexte, les sections et tables accessibles, l’état « Non accordé » et les associations inconnues quand le registre correspondant est indisponible. La mise en page est vérifiée à 1 440, 390 et 320 px, en clair et en sombre. Les 24 entrées Workers et les 60 événements sont synthétiques dans la base SQLite de recette.

Un Chromium 148.0.7778.96 sur macOS ARM64 donne `PASS` en 6,24 s avec Playwright 1.66.0-alpha-2026-11-18 et Node 24.22.2. Le processus est limité au loopback et la garde navigateur ne relève aucune requête hors origine. Le rejeu conserve 18 captures PNG privées. Le conducteur appelle la capture HTML/PNG assainie si une assertion échoue sur le dashboard authentifié. Le rejeu nominal n’emprunte pas cette branche.

SHA-256 du conducteur `c06c84b8e69d16d21088207c20216e7e3cb70e33bd49322096be637a15ce5f66`, de `supervision_stack` `5c433a4069246d6b48b792e5f7be17b14025d737f290d8232bcb39991a568898`, du manifeste des quatre fichiers du bundle E1 `35c32a4db260bb58dcdbb8d922446ac1078832b321cd18063b1beafec2c948db` et du journal de rejeu `5387e6c56dd27446a2b60f6795328eb523d8fc1c2cbb7e8a68e97c4ef5daefcd`.

### Reprise Chromium après un refus tardif des événements

Sur le même bundle E1, `resource-stream-late-denial` passe en 7,45 s avec Chromium Headless Shell 148.0.7778.96 sur macOS ARM64. Le conducteur injecte une panne réseau et retarde le HTTP 403 d’un seul GET Événements dans Chromium. Le retour à Ressources garde les lignes périmées et le bouton « Actualiser » conserve le focus. Le vrai gateway Rust répond HTTP 200 à la vérification de l’identité avant les nouvelles lectures de ressources et la reprise d’un seul flux SSE. La déconnexion ferme ce flux.

Les 24 entrées Workers et les 60 événements sont synthétiques. Les processus utilisent le loopback et la garde navigateur ne relève aucune requête hors origine. Les empreintes du conducteur, de la fixture et du bundle sont celles du scénario `partial`. SHA-256 du journal de rejeu `341629eec6a27a43f24f6f2a1cb0791a55076936eb93b070f13c50fdf6b64c23`. Le scénario se rejoue avec `AI_GUARD_SUPERVISION_SCENARIO=resource-stream-late-denial` dans la recette Chromium Ressources.

### Export nominatif en deux pages dans Chromium

Sur le bundle E1, `human-export` passe en 5,34 s. Le compte A1 télécharge un seul fichier JSON contenant 39 événements en deux pages et l’indicateur `selection_complete`. Chromium compare les événements, le curseur et le contexte aux deux lectures HTTP autorisées. La recette retrouve deux reçus à séquences croissantes dans la chaîne SQLite et en vérifie l’intégrité. A2 garde ses métadonnées. Le bouton d’export est désactivé et un GET adressé au vrai gateway répond HTTP 403 `permission_denied` sans page ni reçu.

Le profil nominatif local utilise des données synthétiques. Chromium Headless Shell 148.0.7778.96 et le gateway restent sur loopback. Les empreintes du conducteur, de la fixture et du bundle sont celles du scénario `partial`. SHA-256 du JSON téléchargé `8b6705d14eef2a878ae60e054310b24ea5578dac716510bf7efaaf150d426044` et du journal de rejeu `4d56f8767f01476fe6ea7e6c2f191740da5ec84d4b267e5e969760a44552814a`. Le scénario se rejoue avec `AI_GUARD_SUPERVISION_SCENARIO=human-export`. Les captures sont disponibles pour une erreur sur le dashboard authentifié.

### Reprise d’une sauvegarde E1 depuis un dossier distinct

La création et la reprise partagent le contrôle du parent physique de la sauvegarde. Le test part d’un vrai Store14 issu de l’adoption de recette. Il reprend la sauvegarde SQLite depuis le dossier de sauvegarde distinct, puis refuse une copie placée dans le dossier du Store. Après le refus, le format, la tête d’audit de la source et les octets des deux fichiers de sauvegarde correspondent aux observations précédentes.

| Sélecteur dans `audit::egress::tests` | Résultat |
|---|---:|
| `maintenance_resume_requires_a_distinct_backup_parent` | PASS en 3,19 s |
| `maintenance_migration_is_explicit_backed_up_and_format14_remains_closed` | PASS en 1,27 s |
| `maintenance_active_owner_refusal_is_typed_at_acquisition_and_effect_free` | PASS en 1,28 s |

Les trois tests utilisent le même binaire macOS ARM64 de la candidate `5b3c2ed0` avec ses deltas. Compilation hors ligne en 1 min 42 s, réseau interdit et 707 fichiers vérifiés. SHA-256 du manifeste `2f460d6f515f5d32505377f43098c67f053cf0956f9fb0516f33ceeefaa3e2e1`, du libtest `5de45b7d57d9c8fa0b62af41c744c5474020cf771e7773bb383645c3c7ff4c61` et du rapport `21900d7b7c713f202f15200446e1932f662a32abe8c5e9b1525134f8dc3c2538`. Le test exerce le writer interne. L’orchestration opérateur de la migration reste à livrer.

```sh
cargo test --locked --offline --lib \
  audit::egress::tests::maintenance_resume_requires_a_distinct_backup_parent -- --exact
```

#### Reçu du manifeste admis lors de la reprise E1

La recette reprend le manifeste format14 réellement publié pendant l’adoption. La garde de maintenance retient le reçu d’ancre de la même source physique. L’ouverture en lecture compare sa séquence et son empreinte au rejeu SQLite avant l’ouverture en écriture du Store. Le test confirme le reçu attendu, la reprise dans le dossier distinct et le refus du mauvais dossier. La branche de refus d’un reçu divergent reste à exercer.

Sur ce gel, `maintenance_resume_requires_a_distinct_backup_parent` passe en 3,51 s, `maintenance_migration_is_explicit_backed_up_and_format14_remains_closed` en 1,33 s et `maintenance_active_owner_refusal_is_typed_at_acquisition_and_effect_free` en 1,28 s. Compilation hors ligne en 1 min 43 s. Les 707 fichiers sont vérifiés et le réseau est interdit. SHA-256 du manifeste `74ccdf60f5429079cc238c1ab421c8cd39a6fe47f126c432809d6c3a6505671b`, du libtest `30e694af88a2706d801c3e351ea01aee63a234a523fa0c815fe60ce3a8c29256` et du rapport `6d6a8c3e0b1295f23fdf350e4ea05699d4190cda25c7578dd8c15f2812d9f659`. L’entrée opérateur reste à relier au writer interne.

#### Confirmation du reçu committé et observation du Store voisin

Le test de confirmation refuse un Store14 avec une sauvegarde valide. Après un commit réel vers le format15, il relâche puis reprend la garde de maintenance. Il retrouve la même séquence et la même empreinte de genèse et refuse d’autres limites. Le format logique, le schéma, la tête d’audit et le quota sont comparés avant et après. La confirmation utilise une ouverture SQLite de maintenance en lecture-écriture avec WAL et une transaction `Immediate`. Cette ouverture peut affecter le journal et les fichiers auxiliaires.

Dans une seconde recette, la même garde encadre deux Stores. Le port d’observation lit le voisin B en SQLite `query_only` et compare sa liaison physique et son ancre au manifeste. La conversion effective de A au format15 laisse B lisible en format14 avec la même tête. La recette refuse le token destiné à A et une garde sans manifeste. Chaque lecture porte sur un instantané de la base concernée.

| Sélecteur dans `audit::egress::tests` | Résultat |
|---|---:|
| `maintenance_confirmation_recovers_exact_committed_genesis_without_installing` | PASS en 1,70 s |
| `maintenance_reads_anchored_store14_neighbor_during_store15_upgrade` | PASS en 2,26 s |
| `maintenance_resume_requires_a_distinct_backup_parent` | PASS en 3,34 s |
| `maintenance_migration_is_explicit_backed_up_and_format14_remains_closed` | PASS en 1,27 s |
| `maintenance_active_owner_refusal_is_typed_at_acquisition_and_effect_free` | PASS en 1,26 s |

Les douze tests `build_identity::tests::` passent également sur ce binaire. Compilation hors ligne en 1 min 40 s, réseau interdit et 707 sources vérifiées. SHA-256 du manifeste `564cd9c73b659284bb142d0fd4b18550d37065846582c193d62c750d38ac01c8`, du libtest `a518bd7f046d014a16b08a935deaeda3571c63b0f6145543998dfd7549fba2b3` et du rapport E1 `f200c69d2b8e45de793f8e99c4783eeca4713988c07cd31ab5e950ae7dc790c3`. L’orchestration opérateur, la publication de la fermeture et les bindings runner restent à relier.

```sh
cargo test --locked --offline --lib -- \
  audit::egress::tests::maintenance_confirmation_recovers_exact_committed_genesis_without_installing \
  audit::egress::tests::maintenance_reads_anchored_store14_neighbor_during_store15_upgrade \
  --exact --test-threads=1
```

#### Intention durable et prévol fournisseur sur le gel reconstruit

Le module E1 retient l’intention avec le plan, l’ancre, les limites et le préfixe SQLite sous le writer. Le cas A/B vérifie le refus d’une reprise avant création de l’intention, puis le refus d’une deuxième préparation. Après le commit, la garde est réacquise. La reprise compare le même préfixe et les mêmes limites et restitue le reçu de genèse. Un Store15 avec de nouvelles destinations ne peut ouvrir une intention fraîche. Les cinq tests E1 passent en 19,81 s sur ce binaire. La preuve durable liant la sauvegarde à l’intention et le parcours opérateur restent ouverts.

Le même build compile le vrai CLI et le compagnon `ai-guard-diagnose`. Les trois sélecteurs de la CI passent en 4,48 s. Ils couvrent les références de credential, la classification d’une sélection refusée et la détermination du modèle. La classe `InstalledDiagnosticTests` passe ses seize sous-cas en 2,07 s avec le compagnon issu du même build. Les quatre combinaisons Codex/Claude et API/abonnement sont exercées en batch et TTY avec les références de recette. Réseau interdit pour ces trois groupes de tests.

Build hors ligne en 3 min 10 s sur macOS ARM64, 1 090 entrées du gel vérifiées. SHA-256 du manifeste `434c14004d91bd247dbceb8f7feb39adfa16e536146eb71553203e06b963e112`, de l’inventaire des exécutables `5827b01eb32ed35e3d17f843955f1fa9271c4c93e53a2a90da7c514929e3dedc`, du CLI `5fa8393d82ff3beb2cad5cd3a1e2ba4ab04739232a5e3933890c2b436ff468aa` et du compagnon `69605d0a165d9602e916ef504cb35e886547f50a6360045da6a7cbd10bad393e`. Le prévol s’arrête à la référence et au choix du client et du modèle. Les essais de conversation et d’authentification réelle au fournisseur gardent leur propre qualification.

### Navigation au clavier sur le bundle E1

Le scénario `keyboard` passe en 6,35 s. Chromium vérifie le focus au titre, dans la recherche et à l’ouverture du détail, puis son retour avec Échap. Tab et Maj+Tab conservent un ordre et un focus visibles à 768, 390 et 320 px. À 320 px, l’entrée dans la vue Fédération conserve le mouvement réduit et une mise en page sans débordement.

Le vrai gateway utilise les 24 entrées Workers et 60 événements synthétiques de la fixture. Chromium 148.0.7778.96 et la fixture restent sur loopback. Aucune violation du garde navigateur. Les empreintes de la fixture et du bundle sont celles du scénario `partial`. SHA-256 du nouveau conducteur `1a667da707964aba576c2feb03d8e797772d81d45d8d35b5fdd889a1a3920140` et du journal privé `40bb13ddf0ed390915ffc3e16b0a4b1956c6d4cf3135b49c725bdb14296c9f39`. Le conducteur conserve le HTML et le PNG assainis en cas d’échec sur le dashboard authentifié.

### Activation du flux après deux retours de visibilité

Après redémarrage du poste, les 34 entrées du front et son lockfile permettent de reconstruire hors ligne les quatre fichiers du bundle E1 à l’identique. La compilation TypeScript et Vite passe en 1,96 s. Le scénario `resource-stream-visibility-race` passe dans Chromium en 4,98 s sur le gateway Rust et la base de recette. Les deux vérifications d’identité reçoivent HTTP 200 du vrai gateway. Le conducteur contrôle le moment où Chromium remet chaque réponse à l’application. Seule la seconde réactive les détails et l’export, relit les événements et ouvre un unique flux SSE. La déconnexion le ferme.

Le navigateur, la fixture et le garde restent sur loopback. SHA-256 du manifeste des sources front `19f1b3f5cb700b1e3fb95159de71d73b7976af5912b13f8a471069b5380d71a3`, du manifeste du bundle E1 reconstruit `35c32a4db260bb58dcdbb8d922446ac1078832b321cd18063b1beafec2c948db` et du verdict privé `54bb8b722c917fd45318164b79468c0ae91debd9ef5c6ecbb93a647a1f766fa3`. Le conducteur et le binaire de fixture sont ceux du scénario clavier. Le guide de rejeu sélectionne `AI_GUARD_SUPERVISION_SCENARIO=resource-stream-visibility-race`.

### Trois tests hors moteur du harnais de reprise Docker

Les trois sélecteurs `docker_worker_` passent. Le test de l’historique refuse l’absence ou la répétition du démarrage et la présence d’un autre CID. Les deux tests du support couvrent la précision nanoseconde et la projection d’une trame du flux sur la session. Le test s’exécute avec le réseau interdit.

Un manifeste Cargo privé compile la cible `tests/broker_runtime.rs` comme test et la vraie bibliothèque de la candidate comme dépendance de chemin. Il conserve les mêmes versions et sommes du lockfile. Cette disposition omet l’édition de liens des autres binaires applicatifs et garde les tests inchangés. Compilation en 1 min 05 s. SHA-256 des entrées `868238c0852ade2dc284a62b444734b823e41ddeefbcf35a20cd41be8b41c4bf`, du binaire `5fd5fa9c99443a1f82213b65b369bf7e05f177751241e5e2c5bbbc8caf6f6fd0` et du rapport `985a3222dbed01994c0c4941f535262e0d135c22a4c770880516380d54499877`. La recette sur le daemon sélectionné reste `NOT_RUN`.

```sh
cargo test --locked --offline --test broker_runtime docker_worker_ -- --test-threads=1
```

### Perte du broker Docker sur la candidate

Le 20 novembre, le vrai test `broker_sigkill_preserves_original_deadline_without_second_start` s’exécute sur Docker Desktop 29.9.3 ARM64, avec un seul worker de recette, un délai de 30 s et un `sleep` sans fournisseur. Le build de test est non optimisé. Le CLI a la même empreinte que celui de la recette à trois workers. L’image QA est aussi la même.

Le premier essai échoue avant soumission. Le client HTTP du harnais transmet directement les bornes RFC3339 à `/events`. Une sonde ciblée sur le socket local obtient HTTP 400 avec ces bornes et HTTP 200, un flux chunked et une fermeture normale avec des secondes Unix fractionnaires. La correction applique ce dernier encodage au client HTTP et garde les neuf chiffres de précision. Les trois tests de collecte passent sur la cible Cargo du dépôt.

Le second essai ouvre le flux HTTP 200 et obtient l’acceptation de la soumission. Le broker Rust s’arrête ensuite sur un débordement de pile. La recette échoue en 61,77 s, au stade de soumission. Le quota est retenu et aucun démarrage du worker par Docker ni arrêt du broker par le harnais ne sont observés. Le test n’atteint donc ni l’échéance, ni la réconciliation. Docker ne déclare aucun conteneur actif de cette image QA après le test. Les ressources privées du test sont retenues et la continuité des événements Docker reste inconnue.

SHA-256 du manifeste de 1 090 entrées `b341bd8597ad878e87170ea430cf1dd94cb608f7ab3a5c42a5486a3bc4a55472`, du binaire d’intégration `efa83a3c359416e3fd97cc8348e898b64ea7febe7dcc5c1c7cba74c93426c0b2`, du CLI `5fa8393d82ff3beb2cad5cd3a1e2ba4ab04739232a5e3933890c2b436ff468aa` et du rapport d’échec `1785075a2e8f10f04e088731d5fe2663d1474a35941cc04b9e43e54a95798b13`. La recette de perte du broker reste FAIL sur ce gel de test. Le scénario distinct de perte de la CLI conserve sa preuve historique.

### Preuves journalisées de sauvegarde et de confirmation Store15

Le 20 novembre, cinq régressions E1 passent en 26,14 s avec le vrai writer de maintenance SQLite et une garde réacquise. Le scénario A/B abandonne une première transaction avant commit. La confirmation est alors refusée et le catalogue conserve l’intention et sa preuve de sauvegarde. Après reprise, commit puis réacquisition, deux réouvertures rendent le même reçu de genèse et la même preuve de confirmation. Le test relit ses empreintes d’intention et de sauvegarde, son numéro de séquence et son hash. Il vérifie aussi le refus d’une autre sauvegarde SQLite valide du même préfixe et la conservation du Store14 voisin.

Le binaire utilise l’ouverture de maintenance en lecture-écriture avec WAL. Une panne pendant l’écriture d’une preuve peut encore laisser un fichier partiel qui suspend la reprise. Restent ouverts la publication atomique de la preuve, le traitement d’une copie SQLite créée sans preuve, les bindings runner, le point d’entrée opérateur et la publication de la nouvelle fermeture.

Build hors réseau en 1 min 59 s. Le manifeste de 1 090 entrées change sur les cinq fichiers de ce lot. SHA-256 du manifeste `1adfd760d3cd17c8ae4f11363f5a50b6a0c52ad4ab777e881b89c966e9569ab8`, du libtest `0c0cf814fa00c48945bd3b7b91cc8c478abcfd597e088edde196b748ef27dd5e` et du journal `a365698d18ffbeeb3ed6791d58213365d92dc8773fd1dbf5ef9c5be69670ecd3`.

```sh
cargo test --locked --offline -j1 --lib -- \
  audit::egress::tests::maintenance_reads_anchored_store14_neighbor_during_store15_upgrade \
  audit::egress::tests::maintenance_confirmation_recovers_exact_committed_genesis_without_installing \
  audit::egress::tests::maintenance_resume_requires_a_distinct_backup_parent \
  audit::egress::tests::maintenance_migration_is_explicit_backed_up_and_format14_remains_closed \
  audit::egress::tests::maintenance_active_owner_refusal_is_typed_at_acquisition_and_effect_free \
  --exact --test-threads=1
```

### Podman et Kubernetes Desktop le 20 novembre

Le socket Unix de Podman répond à `info` avec Podman 6.1.4 ARM64 en mode rootful. La précédente erreur E/S du stockage n’est plus reproduite lors de ce prévol. L’image QA de la recette Docker est transférée localement, sans registre, et Podman retrouve le même identifiant complet, l’architecture et l’utilisateur `1000:1000`. Le point `/readyz` du contexte `docker-desktop` répond par EOF. Les pilotes CSI n’ont pas pu être relus lors de cette sonde. SHA-256 du relevé `3285b808f11a021aeabbef7c64e8a35059c4b611ebd22e795b829cfb80c06ba3`.

La recette Podman `rejected_terminal_audit_keeps_the_real_terminal_instance_inspectable` échoue en 58,62 s sur le socket explicite. SQLite contient une réservation et une préparation. Le volume d’import est confirmé dans le checkpoint et l’archive conservée concorde avec son empreinte. Le registre ne contient ni identifiant de conteneur worker, ni demande de création ou de démarrage du worker. Le test n’atteint pas l’injection du refus d’audit terminal. Il émet une erreur d’état générique qui laisse la première cause inconnue. La lecture des événements Podman filtrée sur la session revient vide. Aucun conteneur de l’image QA n’est actif après le test.

SHA-256 du manifeste `1adfd760d3cd17c8ae4f11363f5a50b6a0c52ad4ab777e881b89c966e9569ab8`, du test `29247e2c0dd9e12783d88fda6ec8a814c30ca1f8dc17418fe3997e349b640131` et du journal `d5cf34ebe33464702a23d784facc86fa2fcce9e128ec9a632c63e0002563f5e7`. La recette terminale reste FAIL sur ce profil Podman rootful. Une recette d’import et de lancement avec le CLI en profil strict est nécessaire pour séparer les prérequis du terminal des effets du moteur.

Le vrai CLI confirme le même échec d’import en profil strict, réseau coupé et commande bénigne `/usr/bin/id`. Podman retourne le code 125 et refuse l’option de montage `uid=1000`. Le worker reste en préparation, le quota est retenu et aucune allocation n’est enregistrée. Le fichier original est intact. SHA-256 du verdict `b29dc99d5a9f807c63a1ba091f0da46ee7cd6fca966caf9ef9a1a1f8ff0b9fa1`.

Une sonde Podman distincte monte `/home/agent` en tmpfs via `--mount` et `U=true`. Le conteneur vérifie l’utilisateur et le propriétaire `1000:1000`, le mode `0700`, l’écriture, la capacité de 64 Mio et les restrictions `nosuid` et `nodev` lues dans les informations de montage du noyau. Un relevé de création séparé retrouve la même politique dans `HostConfig.Tmpfs`. L’option `U` y représente la propriété fondée sur l’utilisateur du conteneur. Le verdict de la sonde a pour SHA-256 `2981c1cd5accf476cc8b88c1d14287995d93e87908eb74ff89a75fb0ff696c5c`. Le passage par le builder, la validation de l’inspection et l’exécution du produit doivent encore être éprouvés ensemble. [Contrat Podman des montages temporaires](https://docs.podman.io/en/latest/markdown/podman-run.1.html#mount-type-type-type-specific-option)

Le premier rejeu avec le builder corrigé crée le conteneur d’import, puis s’arrête sur l’inspection du driver de journalisation. Le conteneur reste à l’état `created`. `HostConfig.LogConfig` porte `Type=none`, une configuration vide, un chemin et un tag vides et `Size=-1B`. Le correctif accepte cette valeur uniquement pour Podman dans cette configuration. Les deux tests de validation sélectionnés passent hors réseau. SHA-256 du premier journal de rejeu `3872e043017bdc150438cf0a2c8479e23424907c7620758039952ff8d9270776` et du test hors réseau `d2f3ff35f781ebedb1f1a654b7cce0a6ee5f2d9d0f7541c72a679f9d6e32660c`.

Le même conteneur montre les retraits détaillés dans `CapDrop`, aucune addition, ainsi que les deux champs `EffectiveCaps` et `BoundingCaps` présents et nuls. Le code de Podman 6.1.4 calcule les retraits par différence avec la liste par défaut. Le contrôle avant démarrage exige les deux ensembles vides et conserve la validation de l’absence d’ajouts et de `no-new-privileges`. Une sonde distincte dans un conteneur de recette vérifie les cinq masques Linux à zéro et `NoNewPrivs=1`. [Inspection Podman 6.1.4](https://github.com/containers/podman/blob/v6.1.4/libpod/container_inspect_linux.go#L128-L169)

Les sept contrôles Rust ciblés passent sur le gel `2699f967`. Le rejeu physique franchit la création et le démarrage du conteneur d’import. Celui-ci termine avec le code 0. Le runner refuse d’attester la fin de l’import en raison des deux empreintes divergentes. La réservation reste inscrite dans le gateway. Le test FAIL en 84,84 s avec SHA-256 du journal `ff5579d3c710a9d480317c426381b4bd9e0d466dfc45a91851873cf2fba52765`.

Une sonde sur le même conteneur dédié compare les états `created`, `running` et `exited`. Dans la projection contrôlée, seul le montage du home change. L’option `U` devient `uid=1000,gid=1000` au démarrage, avec les mêmes taille, mode et restrictions. SHA-256 du relevé `adab1aa8f8404d8726db0fc21ea7cbbf3852448f792d62196cfe856f77f8068a`. Le nouveau calcul rapproche ces représentations pour le preset Podman validé et conserve les autres options dans le digest. Les empreintes historiques v1, Docker et Podman à propriétaire numérique conservent leur calcul. Les tentatives de recette qui ont déjà enregistré un digest brut avec `U` restent retenues séparément et demanderaient une reprise dédiée.

Sur le gel `916b8b3b`, neuf tests Rust ciblés passent. Le rejeu Podman complet ouvre un seul worker. Le journal durable confirme l’import, la commande `/usr/bin/id` exécutée en UID 1000, son code 0, le quota libéré et une seule allocation. Le scan et le diff terminent. Le pilote retourne toutefois 101 après 226,11 s avec `original execution deadline expired before start acknowledgement`. Le reçu `command.started` est observé 31,65 s après `runtime.submitted`. La recette conserve sa borne de 30 s et son verdict FAIL. SHA-256 du rapport `0d96128581313a612c7af85c0b955ec890ce55e0e0c267151187704592c8053a`.

Après réouverture du gateway sur la même origine, le CLI `worker-status` répond avec le terminal. `worker-resume` rend le diagnostic d’échéance conservé et sort avec le code 1. `export-workspace` rend 0 en 69,43 s, copie le fichier attendu vers une destination neuve et exclut le `.env` synthétique. Le fichier et le `.env` d’origine sont intacts. Le même worker, une unique exécution et une unique allocation restent dans SQLite. Aucun conteneur de cette image QA n’est actif après l’export. SHA-256 du CLI `fb916f9216e026553f1a49755c3e809067117d0d34b1e0c56134a81c80dc0d37` et du rapport `0925c909da08c97cb0c377ef1851edab8cc9473231f906d1476916d6ce5f2b72`.

### Prérequis des témoins logiciels du broker

Le témoin logiciel Legacy s’arrête en 3,38 s avec code 101 avant la soumission. Le répertoire du gardien contient la capacité privée de fixture mais le port existant exige le verrou de rôle créé au démarrage frais. Un essai isolé du harnais utilise la création fraîche et atteint la soumission. Le broker refuse alors la génération du gardien amorcée par un autre processus, au contrôle d’admission. Ce delta de diagnostic est conservé séparément. La candidate revient à son manifeste initial.

Le témoin logiciel Full3 existant compile avec le même CLI et ne franchit pas le prévol IPC du broker avant sa limite. Les répertoires du gardien et du broker sont créés et les processus du test sont collectés. Le test renvoie 101 en 13,81 s. Il n’atteint pas l’import et ne permet donc pas d’attribuer le débordement de pile constaté sur Docker. SHA-256 du manifeste `1adfd760d3cd17c8ae4f11363f5a50b6a0c52ad4ab777e881b89c966e9569ab8`, du test Full3 `56d336bfcab0c2da7d9d7f7889cd267b8a71a6065eedce8c4212f2131cde3eca` et du journal `7f49b98e8143d6ae8e6ff177457a30ae076b8f63c3700167cc737b67a4b7bf8b`. Le passage utilise uniquement le témoin moteur Rust et le gateway loopback.

### Dashboard terminal des trois workers Docker

La candidate ajoute `front/tests/browser_runtime_journey.mjs` à la recette opt-in des trois workers. Le nouvel essai moteur atteint A1 terminal pendant qu’A2 et B1 travaillent, puis les trois commandes et les trois diffs terminent. La CLI d’A2 sort avec le code 1. Les checkpoints d’A2 et B1 gardent le diagnostic d’échéance de supervision. Le test renvoie 101 en 224,85 s avant les exports de cette recette et avant Chromium. Aucun conteneur de l’image QA ne reste actif. SHA-256 du journal `d45935f2061c7dc8c426d97f4753c733d51e260f37b2f7a7f9fd90fc0f7bfda0`.

Le conducteur est alors rejoué sur des copies SQLite de la première recette Docker réussie. Celle-ci a produit trois vrais workers, trois exports distincts et leurs reçus. Le vrai CLI démarre un gateway et un dashboard Rust sur loopback, avec le bundle vérifié. Aucun nouveau worker n’est lancé. Le profil macOS supplémentaire de processus empêche Chromium d’établir son IPC ou d’initialiser son propre sandbox. Le rejeu utilise le sandbox natif de Chromium, avec la garde Playwright, le résolveur limité et le proxy désactivé.

Le premier rejeu qui atteint l’IHM échoue sur un sélecteur du détail worker A. Le rapport ferme les processus et conserve un HTML sans les deux jetons viewer et un PNG, tous deux en mode `0600`. SHA-256 du HTML `6a0702374a8d2f102f37480bf2e4b84aba8cf6c9b2d0c6f330d627811880142a` et du PNG `4b292c23d4e7e359761d4e638c9a87e98617fc5cf38a8509c540dec810d11eaf`.

Après correction du sélecteur, Chromium confirme les trois états terminaux et le détail de leurs inspections, les quotas libérés, les trois allocations non facturables, les trois 404 inter-tenant et six événements rapprochés de leurs reçus. Le rejeu PASS en 2,76 s. SHA-256 du conducteur `bcc3a424176caaa24bf2279cd8a4849d3f1d8262f6b8bada0221c1e998bbad7c`, du CLI `fb916f9216e026553f1a49755c3e809067117d0d34b1e0c56134a81c80dc0d37` et du rapport `d31e748476f32436618873ff232bb4c4e527ca1fad31c9131fc542aa2e4fa930`. Node 24.22.3 et Chromium 148.0.7778.96. Le parcours combiné depuis le lancement et le rendu navigateur pendant le chevauchement gardent leurs preuves distinctes.

### Publication atomique des trois preuves Store15

La candidate publie chaque phase dans un répertoire complet par renommage exclusif. L’oracle A/B injecte une perte d’accusé après le renommage de l’intention, de la preuve de sauvegarde puis de la confirmation. Après commit SQLite et réacquisition de la garde, la réouverture retrouve le même reçu et les mêmes octets. La recette conserve un répertoire temporaire incomplet hors du catalogue, refuse une copie SQLite sans preuve publiée et refuse la substitution par une autre copie valide du même préfixe. Le Store14 voisin reste intact.

Les cinq sélections E1 ci-dessus passent en 13,40 s hors réseau sur le gel de 1 091 entrées. SHA-256 du manifeste `e5989a1600fd5a4b9ad88cb71d7f717de4c96d0f1038863797786d1d5eb8643c`, du libtest `59cb166ac42f52027cf567cd1038d3cb1e8abef7ebce8f5fe42f25528c90e71e` et du rapport `4aaedd5decc11317dd5e155e5d0b224dd62b5a388cb60e76e92ec8b0b2e6f524`. Le point d’entrée opérateur, les bindings runner, le sceau d’activation et la panne physique restent à qualifier.

### Prévol Qwen après redémarrage de Docker

Le 20 novembre, Docker Desktop 29.9.3 ARM64 répond sur son socket Unix local. Le profil Compose Qwen crée son réseau interne dédié et démarre ses deux services avec les images locales des essais historiques, sans construction ni téléchargement. La vérification de santé du relais passe et son port est publié sur `127.0.0.1` uniquement. Le relevé cgroup v2 enregistre un pic de 823 574 528 octets pendant le démarrage, sous la limite de 1 000 000 000 octets, avec zéro OOM et zéro redémarrage du modèle. Les deux services de recette sont ensuite arrêtés et aucun ne reste actif. L’essai demande zéro inférence. Le rejeu intégré exige encore le CLI, le bundle et le relais d’un même gel de sources.

L’empreinte SHA-256 du rapport de démarrage est `a31da657b57d0949c8c5d2ee8a722b3495517c7b420175556c995fb8a83cacbd`. Les relevés privés sont dans `target/qa-evidence/qwen-post-restart-preflight.SQtE7T3b` sous la candidate. Le démarrage se rejoue avec le bloc Compose de `tests/e2e/README.md` de la candidate. Le contexte Kubernetes Docker Desktop vérifié localement renvoie encore `EOF` sur `/readyz`.

### Reprise Native17 avec le registre Auth5 retenu

Le 20 novembre, une copie isolée du checkout Native17 `e8886bd1` compile hors réseau le raccord entre la publication Native17 et le registre Auth5 retenu. Le point d’entrée historique des migrations de profil passe le validateur sans preuve causale, car il détient la source neuve à l’arrêt. Une source déjà utilisée par un parcours causal y reste refusée. La reprise normale capture le registre Auth5 avec la même autorité avant le rejeu historique du Store tenant.

Cinq régressions SQLite passent en 247,02 s. Les trois premières contrôlent le commit repris, les refus de dérive et les ressources conservées. Les deux autres recréent le gateway C4 après perte de la publication, puis vérifient qu’un démarrage à froid ajoute une vraie incarnation Auth5 et rend encore la session Native17. Le parcours autorisé OIDC `POST` 202 suivi d’un `GET` à froid avec le même reçu causal reste à exécuter dans cette copie.

Le manifeste des 1 552 fichiers de la copie porte le SHA-256 `02d5909ccd08c9e900b21274a15e23e81bdaea9a4a008d59904880fd5e373f3a`. Le libtest porte `15cd8349e2cb54480d4ad4cdb983bf9c7d735b2c992bd8367e2085ee55879030` et le relevé `a9038b7d407454952c78f4cb05190346436468c88f0bdbf11d70970b2320c309`. Les sélecteurs et le relevé sont dans `target/qa-evidence/native17-auth5-compile.NXqlE3Q3` sous la candidate. Ces résultats s’appliquent à cette copie isolée.

### Régressions du correctif de pile du broker

Le rapport macOS et le journal durable placent l’ancien débordement pendant `Draft::from_job`. Le journal confirme l’Arm d’import en révision 10. La demande `HelperClaim` n’y est pas publiée. Le correctif alloue le `RunPlan` sur le tas. La comparaison des binaires debug ARM64 mesure quatre cadres du chemin d’import. Leur total diminue de 274 960 octets. Le cadre de `Draft::from_job` passe de 29 632 à 368 octets. Cette mesure caractérise le binaire et le passage physique fait l’objet d’un essai séparé.

Le calcul SHA-256 du client moteur utilise la dépendance `ring` déjà épinglée. Le banc local retrouve la même empreinte et réduit le temps d’un hachage de 0,26–0,29 s à 0,02–0,023 s sur les binaires Docker et Podman présents. Les bornes de lecture, la vérification du fichier et les délais de supervision sont conservés.

Sept régressions Rust passent en 9,30 s sur le nouveau libtest, dont la fermeture du journal après une observation saine avant l’échéance. Dans le cas tardif, la même recette conserve le motif post-accusé, un seul STOP et le journal ouvert. Les trois tests du harnais de collecte Docker passent sur le binaire d’intégration du même build. Le scénario avec les vrais délais Podman et le parcours combiné à trois workers gardent leurs verdicts antérieurs.

SHA-256 du manifeste de 1 124 entrées `82974b50805489687e8f8e8e94ece2bcdea0002fe95c20939d5c90b543cabee6`, du CLI `179f7abd78121c5ff9d195a59452291961c38868e3f86341fb9c5da46d2b9590`, du libtest `bad4f4fea8fe9cb863823d82d025a46bc27bdc64246e5acd690fa54aee03f91a` et du binaire d’intégration `ac82f5a1b77cdf9959068daacb66e2c9bd111c78192b0de455f33772f2c4f28a`. Les deux rapports de régression portent respectivement `c95447a523d0a79d063b0d665f565dcafd9079c8def69aa34f56a242c36b600e` et `985a3222dbed01994c0c4941f535262e0d135c22a4c770880516380d54499877`. [Commandes de recette](#perte-du-broker-docker-sur-la-candidate)

Le rejeu sur Docker Desktop 29.9.3 ARM64 utilise cette même paire, un seul worker et la borne de 30 s. Il rend FAIL en 136,57 s. Le journal local concorde jusqu’à la révision 29. Les révisions 11 et 12 publient puis acquittent le `HelperClaim` d’import, avec disposition `emit_once` et les empreintes liées au même Arm. Le worker reste en préparation avec le quota réservé. Le stderr privé atteste un nouveau débordement de pile du broker. La recette n’a encore observé ni démarrage du worker ni SIGKILL du broker. Aucun conteneur de l’image QA n’est actif à la fin de l’essai.

Le rapport macOS du processus candidat situe le second débordement dans `Execution::patch`, avant le RPC de la transition `Created`. La vérification d’import est acquittée en révision 24 et `CreateIssued` en révision 28. En révision 29, le journal du broker conserve une observation du conteneur `created` avec `running=false`. Le Store reste `Preparing` sans inspection `Created`, Arm ni Claim du worker. Les relevés d’événements du moteur n’ont pas été produits.

SHA-256 du journal `7697d5e4f81c938732b86a67ec8d6d25be2d46c86729741571cc5e192d1ebdf7`, de la projection expurgée `f6002189d31c930744e079e89fa4804544c20a8059f4b908f799438c128e7477` et du résultat `3cfcb2fa7505972a5060c41ba8f8b47513fecab38e0f46c3514e7dd5c42b9c79`. La projection est ancrée au journal local et ne constitue pas un reçu IPC indépendant. Les relevés sont dans `target/qa-evidence/docker-broker-loss-stack-fix.AiKlTnyS` sous la candidate.

### Supervision du CLI avant le second accusé de démarrage

Deux recettes D1 passent en 5,43 s avec le vrai CLI, le gateway Rust sur boucle locale, SQLite et un moteur de recette Rust. La première contrôle l’arrêt durable demandé à la CLI sans défaut préalable. La seconde bloque l’accusé `command.started` après un démarrage connu, injecte un refus de lecture, puis vérifie que le terminal, l’audit et l’allocation restent cohérents après la sortie de la CLI. Elle exige une seule émission de démarrage et l’absence de fenêtre de tolérance avant le second accusé.

Les sélecteurs sont `canary_cli_owner_enforces_durable_stop_without_prestart_fault` et `control_failure_after_start_before_started_ack_preserves_terminal_known` de `cargo test --locked --offline -j1 --test audit_run2_d1_prestart`. Le test utilise le CLI `179f7abd78121c5ff9d195a59452291961c38868e3f86341fb9c5da46d2b9590`, le manifeste de sources `82974b50805489687e8f8e8e94ece2bcdea0002fe95c20939d5c90b543cabee6`, le binaire de test `ecb739b3ac14537572434c69214e3d60d9634c00b8a4aa9e443ed8daa97a1cf0` et le rapport `0752376da4882276465655a3612caa9bbbdc736ea383521874b2d1cf9817f6d8`. Le rejeu physique Podman après correction reste à éprouver.

### Rejeu G09 après correction du cadre `Created`

La candidate sépare la construction des futurs du lancement et des callbacks du cadre `execute_worker`. La correction conserve l’ordre du journal, les identifiants de demande et les délais. Deux canaris Arm, le refus d’un rôle sans verrou et trois tests de collecte passent. Le canari de réconciliation annulée s’arrête avant son oracle parce que son dossier de recette ne contient pas le verrou V1 requis par le processus fils. Le correctif de cette fixture reste distinct.

L’essai Docker à un seul worker atteint `StartIssued`, révision 5 du registre, avec le claim et l’émetteur conservés. Le journal local est cohérent jusqu’à la révision 37. La recette dépasse sa borne globale et rend FAIL en 600,06 s. Le relevé ne contient aucun débordement de pile détecté, aucun `Running` confirmé, aucun SIGKILL du broker planifié par la recette et aucun accord entre flux et historique des événements moteur. Aucun conteneur de l’image QA n’est actif au relevé final. La preuve d’arrêt à l’échéance et de reprise sans second démarrage reste ouverte.

SHA-256 du CLI `642f82fcef331a0d9e76e0f4d2cf5669623ab950d5f1ea4081a46effc049e139`, du manifeste `7350096b0c493c5dd96363db0f3fd42d25b1889bf179a56d4874bdc3636e7e47`, du journal `9120a67b799d1046188b0818108dcbb2d2ad5567ed06cdafd38347bb084f3ee6` et de la projection locale `7228c3a56bfee877c94eda04f0ed738946ecaebf5220561c906802a50bb61eda`. Les relevés sont dans `target/qa-evidence/docker-broker-created-stack.Ahd6V9AF` sous la candidate.

La sonde IPC `inspect_retained_guard_after_runtime_failure` relit le slot exact avec le même binaire de test. Le gardien rapporte `quarantined`, `prepared_once=false`, la dernière observation `created` et zéro échec d’arrêt. Le CID correspond au registre, et Docker inspecte ce même conteneur à l’état `created` avec `StartedAt` et `FinishedAt` à zéro et aucun redémarrage. La lecture historique Docker sur la fenêtre et le CID exacts rend zéro événement. Sa couverture reste inconnue. Le prochain écart se situe avant la préparation du permis de démarrage. SHA-256 du relevé `4f3eb6532039534d678cab2c22f8eb01de6e655eb6b9927a6685a6f4a3650b43`.

### Build Chromium en activité et garde V1 de la fixture de reprise

La candidate compile le conducteur navigateur en activité, le test de concurrence Docker, le test local Podman et le libtest. Les inputs et le bundle servi par la recette Chromium sont vérifiés par SHA-256. Le mode en activité confirme une lecture visible par tenant, puis inspecte les trois CIDs encore actifs avant de fermer Chromium. La preuve physique combinée reste `NOT_RUN` à ce stade. Les échecs sur la page dynamique conservent le HTML assaini lorsqu’il est disponible. Le mode terminal conserve son mécanisme HTML et PNG.

La fixture du canari de reprise utilise maintenant le bootstrap du verrou V1 existant avant de lancer son processus fils. Dans le nouveau libtest, `existing_root_missing_lock_is_refused_without_creation` passe. Le canari positif atteint `peer_ready` et `decision_ready`, puis s’arrête sur `private enforcer stop actor is not ready; no start permitted`. Il n’atteint pas son oracle de réconciliation. Ce refus est conservé pendant la correction de la fixture.

SHA-256 du manifeste `62e40db39a44b1b26f2c86b23bf2df44c29bbe440f6a855a6c91a939144ee47a`, du test de concurrence Docker `847f927d1d156c7e3bd3b51957f30932aabaf688f315692c8d9beffb20bd872d`, du libtest `a86e98c1bd5941feeee851360db8687cec57b40310067a839f0311fc5c282f22` et du rapport canari `1ba7a499e1b047db395e5d20f0bee0a36f9ed2db6a99f395be108e7dfea6db7b`.

Le prévol du CLI valide le socket Podman explicite, le mode rootful autorisé et l’image locale attendue en `1000:1000`. Aucun worker QA n’est actif au prévol. Le conducteur `run-one-podman-cli.py` est dans `target/qa-evidence/live-dashboard-and-broker-canary.kXOb9pNY`. Il soumet une seule commande `id` en réseau `none`, rapproche le départ, l’allocation et l’export de cette session et garde le code de sortie initial. SHA-256 du rapport `doctor` `dbdae4700c3e0f3bfefddf6607bad044531a2d32df6f987edbed13d20336db98`. Le prévol ne démarre aucun conteneur.

La partie SQLite du conducteur passe sur une copie du Store synthétique du précédent essai Podman. Elle retrouve le terminal, un `command.started`, un `command.completed` avec une identité UID 1000 et une allocation observée rattachée exactement au worker et à sa tentative. SHA-256 du conducteur `c23a65f5160605e58dc51ad7b9b3aaaa0820e6ca9c58b554edc537e76fe1fd72`. Ce rejeu confirme l’oracle de lecture. La nouvelle soumission CLI reste à exécuter.

Le prévol dédié du même CLI initialise une configuration privée et vérifie le vrai gateway prévu pour Podman, avec vérification de `/healthz` HTTP 200 sur boucle locale. Il arrête ensuite le gateway sans avoir démarré de worker. Empreinte du rapport `0f574f9f52844b36ee999f46a7448b208206c7f5b07c287354fd49824f30d7f5`.

### Prévol Chrome du conducteur en activité

Chrome 157.0.8103.80 ouvre les deux contextes viewer du harnais sur les registres vides avec le vrai gateway et le vrai dashboard Rust. Le navigateur rend `READY` en 4,61 s. Une trame de pic synthétique sans worker déclenche ensuite le refus attendu de la recette. Les deux captures HTML sont privées en mode `0600` et les jetons viewer de recette sont absents. La fermeture Chromium est confirmée et le garde ne signale aucune violation. Le test ne démarre aucun conteneur. Le chevauchement de trois workers réels et les assertions du pic restent à vérifier avec Docker.

Le cache headless historique était absent. Cette mesure utilise Chrome installé avec un profil neuf et le contrôle loopback du conducteur. Rapport `a813250784bade716de80990946128ad8d823c337ae44216257190448ebb4479`, script `44c88f8e7ff068120b2b5349b1d20b949a24663b3b7ce5025afe4f86bf558e11`. Rejeu privé `probe-live-browser-readiness.py --chromium <exécutable-absolu>` dans `target/qa-evidence/live-dashboard-and-broker-canary.kXOb9pNY`.

### Podman — soumission et export par le vrai CLI

Une seule soumission `agentsandbox run` sur Podman 6.1.4 ARM64 rootful passe avec le socket local explicite, l’image QA inchangée, `/usr/bin/id`, le profil strict et le réseau `none`. Le CLI rend 0 en 18,93 s avec la borne de 30 s. `worker-status` confirme le terminal, `verify-audit` valide la chaîne et `export-workspace` rend 0 en 6,20 s vers une destination neuve. L’export contient l’original attendu et exclut le `.env` synthétique. Les deux fichiers sources sont intacts. Aucun `worker-resume` n’a été nécessaire dans ce passage.

SQLite conserve un seul `command.started`, un `command.completed` qui atteste l’UID 1000 et un reçu d’export. L’allocation observée, non facturable, correspond au worker, à sa tentative, au CID et à l’empreinte de spécification. Elle est inchangée après l’export. Le moteur ne trouve plus le conteneur, en accord avec le registre. Pour le CID exact et la fenêtre de la recette, l’historique Podman contient un `create`, un `start` et un `remove`. Ce relevé porte sur les événements observés, avec couverture d’export inconnue.

SHA-256 du rapport `3f0ae0c27bd007ba08d73e42a914271d518ac5b38714c5a0023bb48e2d8b19b3`, du rapport moteur `7babf9dfde00cc0be14ec23784ca3b832b5f321faf273c3c3a43b16171958bba` et du CLI `642f82fcef331a0d9e76e0f4d2cf5669623ab950d5f1ea4081a46effc049e139`. Rejeu avec `run-one-podman-cli.py` et le même binaire, le socket sélectionné et une destination privée neuve. Le conducteur et le dossier `podman-cli-one-run.e72e92ae-8969-4087-8955-0562792160ae` sont sous `target/qa-evidence` de la candidate. Le résultat couvre ce parcours à un seul worker. La qualification multi-tenant, la panne du contrôle et Podman rootless restent ouvertes.

### Docker et Chrome en activité — refus HTTP 429 de l’observateur

Le harnais combiné `real_cli_runs_three_independent_workers_across_two_tenants` rend FAIL en 66,83 s avec un HTTP 429 générique. Chrome avait déjà rendu `READY` pour les deux viewers, avant que le harnais soumette les workers. Les captures sont restées en vue d’ensemble et précèdent le premier événement de transition d’un worker. La relecture du code situe le refus sur la boucle Rust qui lit le registre avant le pic. La réponse n’a pas conservé le code fermé du refus ni le tenant de la requête. Le sémaphore de lecture et son interaction avec les lectures de synthèse/SSE restent à distinguer par mesure.

Les trois commandes ont terminé, avec trois diffs durables, trois quotas libérés et zéro conteneur de l’image QA encore actif. Les intervalles natifs conservés dans les registres indiquent 20,067 s de chevauchement. Le test s’est arrêté avant les exports et avant l’assertion navigateur du pic. Il a conservé deux HTML privés mode `0600` sans les deux jetons viewer de recette. Empreintes `04714f90bab23af9ee4ca2fa1ec51b11b3c420fc9764a0f2463dfe52e62e9de5` et `be2c51a2f4eb842a18defa44298ddc3e5f742d1f7c42c697209c1b7a561535e6`.

Rapport `8b4712a2fdef46232d123d0246f521a2cc1b0c088323a352b3bcbe9b5f156284`, journal `67fdbc112cab79073251f8a4e01b160c9ffdf5709eec30b6d5132e65929a0fce`, sous `target/qa-evidence/docker-chrome-live-combined.k5tq6y7h` de la candidate. Navigateurs et workers utilisent toujours les limites du gateway. Le prochain rejeu doit conserver le code de refus et effectuer son observation dans la fenêtre de chevauchement réelle.

### G09 Docker avec instrumentation du claim

Le rejeu sur un seul worker expire à 600,08 s, avec le code 101. Le journal vérifie toute sa chaîne jusqu’à la révision 34. Il acquitte `Created` en 31, publie l’Arm du worker en 32, l’acquitte en 33 puis publie `FailureObserved(worker)` en 34. Le registre reste à `Created`, révision 4, avec quota réservé. Le journal ne publie aucun `WorkerClaim` pour ce worker. La recette n’atteint ni `Running`, ni le SIGKILL prévu, ni la reprise. Le moteur dénombre zéro conteneur actif de l’image QA après la fin.

Les 5 172 octets de stderr du broker ne fournissent aucune marque du claim. L’écart entre les dates de fichiers de l’Arm en attente et de son ACK est de 21,926 s, puis de 14,661 s jusqu’au constat d’échec. Ces dates ne mesurent pas un délai du Writer ou de la préparation du permis. Une prochaine sonde doit également relever le retour de l’Arm et les contrôles natifs qui précèdent le claim.

Après la terminaison, la lecture IPC de l’Arm exact retourne un conflit d’enregistrement avec l’état retenu et aucun rapport de slot. L’inspection Docker en lecture seule retrouve le même CID rattaché à la tentative et à l’Arm, à l’état `created`, avec les deux dates moteur à zéro et zéro redémarrage. L’état du slot et la cause de l’échec avant le claim restent inconnus. Projection `7d0c1b36111c85f45509e073c72f99a8251076b8ee825bf1f5bbae7773f86a21`.

Preuves privées sous `target/qa-evidence/docker-g09-writer-phase.HEoSeHqO` de la candidate. Manifeste du build instrumenté `a4b7242de78b045ae8e1f1fa8c66fc96c0b1dd8581fe77a0c60798d5ec8da9aa`, binaire de test `5bd13d457c7469c642c3fa1c228f3482baf90cde8ff192d2131d30642324d436`, CLI `de8df8c5ea585d138b74c79f6cc7aa5db1846c79e3dbe9a315820c6063fe74c7`, log `4383b2a4bf068a5344c31fb9d960d5db5d310bed3100634cd3d729aee911295a`, projection du test `c94215cdd16638057c73dfedcd346aa990c37b6ff7936e427fff26031322173f` et projection du probe `2e77091ae83eb5156b91a9be83dc8ab38ec3723307d3327eb5f6e37589a501b3`. La sonde a été compilée dans une copie privée et les trois canaris de collecte passent avec le même binaire de test.

Le rejeu instrumenté suivant rend **FAIL à 600,15 s**, code 101, avec une chaîne valide jusqu’à la révision 16. Il acquitte le claim du helper d’import en révision 12, publie son terminal avec code 0 en 13 puis l’acquitte en 14. En 15, le journal porte `HelperCleanup` avec `removed=true`. En 16, cette transition reste sans ACK local et `FailureObserved(import, unavailable)` est enregistré. Le worker reste `Preparing`, révision 2. Aucune preuve d’`ImportVerification`, de claim worker, de démarrage ou du SIGKILL prévu. L’absence du sidecar worker est cohérente avec une interruption avant son périmètre. Le moteur rapporte zéro conteneur actif de l’image QA après le test. La cause de l’ACK manquant reste à établir. Dossier `target/qa-evidence/docker-g09-preclaim-runtime.bwdc13M4`, journal `0574b79d4871bc7045218f1b0eb3f49157818567ffe5cdd658b3ce8610bb2868`, projection du journal `6d5de251e61fd81158812a8531021aa77ebb46022bc9650ace99c436b81f851c` et projection du sidecar `c8167feb6966bd793e9e4a134b33594583598113e27df231e46ce49f354028da`.

Une copie cohérente de la base et de son WAL confirme l’absence de reçu `HelperCleanup` pour l’opération en attente. La projection vérifie le reçu positif `HelperObservation` du même helper comme témoin et compare les fichiers d’origine avant et après la copie. Le résultat borne l’absence d’effet durable. La tentative HTTP reste inconnue. Rapport `fc0422532bbce1ad04207da751a910c36a4dbee66858158eac073e749c093060` sous `target/qa-evidence/g09-helper-cleanup-cold-check.QjSWd685`.

Une revue du chemin d’appel identifie deux frontières compatibles avec cette projection. Le retour Writer de la révision 15 peut manquer après la publication durable. Si le retour Writer a abouti, le client ou le gateway peuvent interrompre la requête avant le commit. La RPC `HelperCleanup` passe sans garde Claim. Le défaut `Unavailable` à la révision 16 et l’absence de reçu Store ne permettent pas de compter les POST ni d’attribuer l’interruption à une frontière unique. Le prochain libtest instrumenté doit observer la prise en charge, la publication, le retour et l’entrée HTTP avant toute correction produit.

Le canari ciblé `import_cleanup_reconciles_same_pending_after_writer_ack_loss` compile et rend **FAIL** en 85,38 s. Sa fenêtre de 5 s expire avant que le test observe l’entrée dans la barrière Writer. Les vérifications du pending, du nombre de POST et du reçu Store ne sont pas atteintes. Le résultat ne localise pas le blocage entre la préparation de la RPC, la prise en charge Writer et le retour du Journal. Rapport `07eae85d08c55a42ee448c0bcb689481a84bd08d9736849ed5af1051fb938123` et journal `7616c755b284a6ea3c1ff0df7584965afb5deab9d36898edd11acf2f1be8fbe3` sous `target/qa-evidence/g09-import-cleanup-canary-build.LG6aaGy9`.

Le libtest du manifeste instrumenté `b232ec189e1d0ee3f177555797c82f40f47621fb58ca1a890ad5ec3164173dc2` compile puis exécute le même sélecteur seul. Le Writer prend la demande de nettoyage à 4,793 s. L’échéance est observée à 5,005 s sans confirmation de publication durable, de retour du Journal ou de réponse du Writer. Le compteur d’entrées HTTP de nettoyage vaut zéro à cet instant. Le canari termine **FAIL**, code 101, en 51,84 s. La réconciliation attendue reste sans preuve. Reçu `c62f33b129bb8c88af69c86912244438c9e39b77ccc80fd1e076a5bd8f35d812` et journal `fb46d2714bf5745d5d7548af73daee2ab2f018d878018c9bbe5eced614e6bb22` sous `target/qa-evidence/g09-instrumented-canary.w1mKGxCc`.

Le delta de recette suivant sépare l’envoi accepté dans la file, son retrait et le début de publication du Journal. Les jalons sont liés à la même opération et gardent la fenêtre de 5 s. Les quatre préimages et postimages, le formatage syntaxique et l’application à blanc passent. Le gel privé de 1 127 fichiers correspond au manifeste `c5f276264af91b29e9dff13c515490eabf45ada787f460dbcaa0719ba48e0639`. Delta `ff2c235e82671945eb6a516a5f05fea9e685d536f2de932e7fadbdf87fc3b02c` et gel sous `target/qa-evidence/g09-queue-stages.9e02b0A3` et `target/qa-evidence/g09-exact-queue-replay.uIrSQlmB`. La compilation et le résultat de ce canari restent distincts des vérifications statiques.

La première compilation de ce gel échoue avant le canari avec quatre diagnostics `include_bytes!`. Trois entrées de compilation avaient été omises dans le gel, `.dockerignore` et les Dockerfiles contrôle et worker. Reçu `80203627551314473de0cb46fd4eb570a10d5fe16dddcd01d1f1df1457ad87d6`. Le nouveau gel privé inclut et atteste ces trois fichiers de la candidate. Les 1 130 entrées concordent avec le manifeste `e3050d15dbb65c17a1b53ab6fc8cd113ca111b4a7bfa7a5f3de514472e89d9e3` avant compilation. Le sélecteur, la fenêtre et les sources du canari restent identiques. Nouveau dossier `target/qa-evidence/g09-queue-replay-complete-inputs.QJ6lC7mg`.

Le libtest du nouveau gel compile en 25 min 18 s puis exécute le seul sélecteur attesté. L’envoi accepté, le retrait de la file et la prise en charge par le Writer sont tous relevés à 4 339 ms, à la résolution du reçu. À 5 002 ms, le début de publication, le retour du Journal, la durabilité et l’ACK ne sont pas attestés. Le compteur HTTP est à zéro à cet instant. Le canari finit `FAIL` en 51,72 s. Les jalons orientent l’analyse vers la préparation avant l’envoi et les validations avant la publication. Reçu `717c5e9d3df1fb1e204c3ae087eec276009d53a53821e863a544051ed792151d`, binaire `e50c0486edd5d9dafceba80dd7836d803ce0ecc56d40d29d3be0aa649e2153db`, journal `e822c6462a9d7c0009274ec331888b40c715e0d3da01d461289e73cb43862e8d` dans le même dossier.

La correction réutilise pour le premier CAS la lecture stricte effectuée dans le même RPC de nettoyage. Un conflit déclenche une nouvelle lecture. Les 1 130 entrées concordent avec le manifeste `84cc2eff1d83d823466fbf9f7ea67c808fc3ea7e4f3a2154c94b05200e8bd338`. Le test de perte d’ACK passe en 38,67 s avec la fenêtre RPC de 5 s. Le test de conflit passe en 1,82 s et confirme la conservation des écritures concurrentes. Reçus `03b7bd68e6e868dc255b2dfcdf7fab513ea9f173d3bffb9ec9df6d949542d64d` et `5a488c1cdd15f18fc2426217e12b6e66e7ebe0032c35faacb9dd9df3b173b3cc` sous `target/qa-evidence/g09-cleanup-reuse-replay.knr49qa9`. Le résultat du rejeu Docker complet est consigné ci-dessous.

Le build du CLI et de la recette Docker à un worker passe sur le même manifeste. Le prévol retrouve le sélecteur exact et l’image locale épinglée sur Docker Desktop ARM64 29.9.3. Reçus de build `57900eee899f64ae8fa8e85a2ed9ac1ca873b57b1c3c1bcb32d5a2dc18b32f37` et de prévol `2a65e8c4f4c97de776ef3b59ea01852a57dcd21b00ffd19f46d5934c686f68ad`, conducteurs sous `target/qa-evidence/g09-runtime-build.rIwRpSxl`.

Le rejeu Docker du même build se termine `FAIL` à 600,15 s, avec le code de libtest 101 et sans preuve de l’événement worker final. Reçu `c9bbd08875b17baa9cefc016d6ace9a01a400c1af1b67a91010c053643eba198`, log `37c3fae8f8cc9a0377319dd00e02880a013f00a7979caec37fb137def9e27fc8`. La fixture conserve les ressources et reçus de cet essai.

Le reçu ciblé vérifie l’empreinte du checkpoint 23. L’import a reçu un acquittement de résultat et de nettoyage. Pour la vérification de l’import, l’Arm et le claim sont acquittés. Aucun résultat de cette vérification n’est enregistré. La dernière commande locale enregistre `failure_observed`, phase `import_verification`, code `unavailable`. Le worker reste `preparing` avec quota réservé et aucun démarrage consigné. La base d’audit de la recette contient zéro événement `broker.batch.failure` à la lecture du reçu. L’inspection Docker après la recette identifie le même helper grâce à son identité, son image et ses labels de session et d’opération. Le conteneur est encore `created`, sans horodatage de démarrage ni redémarrage. Reçu `5df808f8b351f1d1652ef4e277bbcde44186ec2e795df321121478849b40e756` et conducteur `inspect-verification-helper.py`. La sonde en lecture seule du gardien retenu retrouve le même conteneur à l’état `created`. Elle retourne `stopping`, `prepared_once=false` et zéro échec d’arrêt pour ce slot au moment de la lecture. Reçu `8ff981dd3272769cdecdd8c9ff0a81ce16d1e1d86af2c250dad998c9f2d31a56` et conducteur `inspect-verification-guard.py`. La cause de l’indisponibilité et la réponse à la préparation native restent indéterminées. Projection `ab4b11836f4c65fdc240bf8d37a8701e278c17869a45f37f04b175efaa565124` et conducteur `summarize-prestart.py` sous `target/qa-evidence/g09-runtime-build.rIwRpSxl`.

### Régression ciblée de la lecture HTTP 429

Trois tests du harnais passent. Ils vérifient la reconnaissance du seul HTTP 429 `capacity_exhausted` dont l’action est `retry`, la relecture des deux tenants et de leurs quotas avant d’annoncer un pic, et l’abandon sans signal après quatre lectures de paire refusées et trois pauses. Les tests d’observation injectent les réponses classifiées. Les limites du gateway et la fenêtre globale de la recette restent les mêmes. Le CLI a la même empreinte que pour l’essai précédent.

Le libtest `6545012ff8c3861b867a675884ba1ceac74de6c513f0d55f708abcc85489d607` est issu du manifeste `2cb8ca977bf4eb58ee54d7dd02fbc845d13ae5c4fb150d3135885c1ec4511ce8`. Journal des canaris `2326f2cb16b0c40ef67f3fa53694cd91646fd81e2144aa4b3cae371b80ce8e14`, sous `target/qa-evidence/parallel-viewer-recovery-build.sCO5zt1F`. Les sélecteurs sont `parallel_viewer_only_retries_explicit_read_capacity`, `parallel_viewer_reloads_both_tenants_before_signalling_peak` et `parallel_viewer_capacity_retries_stop_without_signalling_peak` dans `runtime_user_journey`.

### Docker et Chrome après reprise du HTTP 429

Le rejeu combiné avec ce libtest rend FAIL en 103,93 s. Le registre Workers du tenant A a réellement répondu `capacity_exhausted`. Après reprise et nouvelle lecture des deux tenants et des quotas, le harnais signale le pic. Les deux HTML de Chromium montrent les pages Workers en lecture réussie pendant l’exécution, avec deux et une lignes « En cours » selon le tenant, toutes avec « Quota retenu ». Le sous-oracle navigateur exact qui échoue n’est pas enregistré. Le signal `DISPLAYED` et l’inspection moteur qui doit ensuite confirmer les trois CIDs simultanément actifs manquent à la recette.

Trois fichiers exportés correspondent aux sources synthétiques et aucun conteneur de l’image QA ne reste actif. Le conducteur signale sa fermeture sans violation réseau. Les deux captures HTML privées en mode `0600` ont les empreintes `34b8bb329b4ee07300370d6a881fa597b66ed6f9469282401df7f94659399932` et `9337658e5899fc0222c02e80066acb938563445dac8382366ada55aa04618880`. Rapport `dee8d780ba56517459e1b9321ff5724dd46b96b2643a1563f0ca5e6aa7a75724`, log `9b79dda39c963d5013db3fe21869f8a920c8719fc7a17cbef394f0af9b4015bb`, sous `target/qa-evidence/docker-chrome-viewer-recovery.kY7a5NZ1` de la candidate.

Le harnais compile ensuite avec des diagnostics bornés par tenant sur la lecture HTTP, les identifiants, la visibilité et le compte des lectures. Il conserve les conditions de réussite et le même CLI. Dossier `target/qa-evidence/parallel-peak-diagnostic-build.4q5uTUq5`, manifeste `88dc1917e7bf8b668369e5852a8c6efb7471f5104032a0a32223003115d9fbee`, binaire de test `d3ba1660b372049e6cb47dfaa6e9c47b44faba31dd883106913643419affa646`. Rejeu physique avec ces diagnostics `NOT_RUN`.

### Reprise Rust et clôture Kubernetes avant effet

Sur le libtest `d5eb741a203b66761227d563175e5106f990bdb0a420d7e22aad138aaad3c824` du manifeste `2cb8ca977bf4eb58ee54d7dd02fbc845d13ae5c4fb150d3135885c1ec4511ce8`, les tests `existing_root_missing_lock_is_refused_without_creation` et `existing_empty_role_is_cold_fenced_after_real_shutdown` passent. Le canari `cancelled_running_worker_reconciles_a_created_owned_snapshot` franchit maintenant l’admission et la préparation des helpers. Le conducteur l’interrompt à 90,05 s, code 124, avant le verdict de réconciliation. Ce délai est celui du conducteur local. Journal `390a7b824b51248f1738935f5c15cd45153dd4ecba4a800be3f86ce773c7dd80`.

Le test `broker_close_invalidates_competing_runner_close_and_restart_only_replays_receipt` passe en 2,01 s sur ce même binaire. Il confirme avec Axum et SQLite que la clôture par le broker invalide la requête concurrente du runner, libère le quota une fois et restitue le reçu après réouverture. L’essai reste avant effet et n’appelle aucun cluster. Journal `24381df7da3eceb3387fed9ec0e2aeda2f5a7128bc51c033222433ff9fa0a052`. Les rapports par test sont sous `target/qa-evidence/parallel-viewer-recovery-build.sCO5zt1F`.

### Diagnostic du rendu Workers pendant une revalidation

Le rejeu instrumenté Docker et Chrome rend FAIL en 108,99 s. Les deux tenants déclenchent chacun un refus `capacity_exhausted` pendant l’observation Rust, que le harnais franchit avant de signaler le pic. Le test navigateur du tenant B concorde avec un GET et une réponse. Pour A, le premier GET sélectionné répond 200 et le navigateur observe trois GET et trois réponses. Le DOM affiche une évaluation différente de celle que le harnais attendait du premier GET, puis l’oracle expire avec `evaluation_changed`. Le journal n’établit pas le statut de chaque réponse supplémentaire. Aucun `DISPLAYED` ni aucune inspection Docker après rendu ne sont obtenus. Les trois exports correspondent aux sources synthétiques et aucun conteneur de l’image QA ne reste actif.

Rapport `2b7bcd70ba365ad8d8e35b177962284b40df8de8080be7e466814a280aa9fc9c`, journal `484a0c9f411f25a6f17d5a202aed98ae798e39a2acce9d7bba5b9924101363c9`, conducteur privé `77c2ff8d5fc2b8254697b5cd84689d9c19e5c75cb9f0752ab14c259250e7d9e3`, dossier `target/qa-evidence/docker-chrome-peak-typed.M42IDam8`. Les deux HTML privés en mode `0600` ont pour empreintes `bd40e8a40c52d79d9a3fb5bf1cb9a32c46af6f1b4d4eb5b68ae8729cbdb82032` et `fa9dec39c2d7c22f35d2bbc387c568e478452882b944beca2b457e5cb7983f54`.

### Rejeu ciblé des pannes du Store d’audit

Le libtest macOS ARM64 `e1f769d6d3e799cb5c0c71303bfdb58f77fbb115222d7ca27a953b9f8ebf9260` exécute trois tests sur des bases de recette avec le réseau refusé et un dossier temporaire canonique. Les 205 entrées d’audit, d’API, de contrôle de stockage et de verrou de dépendances vérifiées correspondent au manifeste du build archivé.

- Une injection `EIO` après un commit réel du tenant A rend l’API indisponible pour A, y compris après retrait de la faute. Le reçu demeure unique et B commite indépendamment dans son Store.
- Une pression sur les descripteurs impose la revalidation du quota, du schéma, des métadonnées et de la chaîne. Les quatre mutations synthétiques sont refusées.
- Après un commit suivi d’une indisponibilité des noms, le retour à un préfixe SQLite antérieur cohérent est refusé par le témoin durable retenu.

Les trois sélecteurs passent en 1,984 s, 0,188 s et 0,214 s. Reçu `11330b89495286f6d84c9ff7aebf2547357660429ff56276871dd08d4aa0bb8e` et procédure sous `target/qa-evidence/g06-store-resource-replay.8mjrgb9i`. La restauration complète des claims et les pannes physiques de stockage restent ouvertes.

### Réconciliation des claims Kubernetes sous SQLite

Trois tests passent sur le même libtest macOS ARM64. Après réouverture du journal et perte simulée de l’ACK, le claim conservé est observé sans réémission. Par HTTP loopback, un claim exact commité conserve son quota et sa chaîne d’audit. Un journal préparé sans claim enregistré reste sans reçu ni mutation de la tête d’audit. Les deux parcours HTTP utilisent un vrai Store SQLite de recette. Le reçu identifie les trois sélecteurs, les cinq écarts entre les sources actuelles et le build archivé et l’absence d’effet runtime. La restauration depuis une sauvegarde et la validation sur un cluster réel restent ouvertes. Reçu `f90ee61c9d4b0fb5794b721afc16662bed92ddc6bdb90ddee06ad48105008108` sous `target/qa-evidence/g06-held-claim-reconciliation.g9zrdlyg`.

### Écriture dans la base SQLite configurée

Le cas SQL-06 passe avec le vrai gateway Rust du CLI archivé `642f82fcef331a0d9e76e0f4d2cf5669623ab950d5f1ea4081a46effc049e139`. Deux bases privées de recette portent des historiques distincts pour le même tenant. Après réouverture de la cible configurée et un `POST /api/v1/events`, son audit valide passe de deux à trois événements et le hash précédent relie le nouvel événement à la tête existante. Les lignes historiques de la cible et les empreintes des fichiers de l’ancienne base concordent avant et après. Une configuration d’un autre tenant et un schéma non reconnu bloquent avant écoute, avec les diagnostics attendus et sans ajout à l’audit. Le conducteur utilise uniquement HTTP loopback et SQLite. MySQL et le navigateur restent hors de ce résultat. Reçu `4fd9b5534c555a68697203984291293c8fd942e63bd72c2024c272338026713f` et conducteur `b701a29a97303f090fbc88fa4d2caf75436226ddb8c1608d4ecabbe9f500c6ae` sous `target/qa-evidence/sql06-sqlite-target.x8IN8jdb`.

Le rejeu étendu du même CLI renvoie un reçu identique après un nouveau redémarrage du gateway. L’audit reste valide à trois événements, les lignes de la cible restent identiques et les fichiers de la base témoin gardent leurs empreintes. Reçu `8dd7ea916ab37aa6dc78b20e0391b6c0a7a06b4eb00ae637b3b04b3c4af812a2` et conducteur `5bca26be8c712c4a614159e5edf2e1cb417dcec214743042ad8954df8040a99c` sous `target/qa-evidence/sql06-sqlite-replay.Z435YHHO`.

Un rejeu ciblé utilise le même vrai gateway, avec tous les fichiers de l’ancienne base synthétique illisibles pendant l’ajout et le rejeu après redémarrage. Les deux opérations passent. La cible conserve son historique, reçoit exactement un nouvel événement et rend le même reçu au rejeu. Les modes sont restaurés sur la fixture et ses empreintes restent identiques. Le mauvais tenant et le schéma inconnu sont toujours refusés. Reçu `651edce29fb40dadc2d74822dc04037f63243122087e075c31fd6cd4b4fbdd08` et conducteur `27243c657b901b83315159d39f992a771028c82ac32955a1cec1d29b763ca526` sous `target/qa-evidence/sql06-sqlite-source-unavailable.fv_zasrq`. Ce résultat couvre SQLite avec le CLI archivé. Le raccordement MySQL du gateway reste à qualifier.

### Prévol des deux profils MySQL réels

Le client et le serveur MySQL 8.4.13 proviennent de l’image officielle ARM64 au digest de manifeste `sha256:f4539b86c82f2c168fc95b439448bde3b9f48e8460344ce41e39081867d2b58e`. Ils échangent sur le réseau interne du projet de recette Docker Desktop, avec CA autosignée et un seul serveur actif. Le premier passage s’arrête sur un oracle qui exigeait `3159` pour TCP sans TLS. Après correction, le compte avec mot de passe passe six contrôles, dont succès TLS 1.3, refus de mot de passe, de CA et de nom, refus sans TLS `1045` et nouveau succès. Le témoin positif lit aussi `@@GLOBAL.require_secure_transport`.

Le rejeu mTLS seul passe huit contrôles, avec succès TLS 1.3, refus sans certificat, mot de passe faux, CA serveur étrangère, nom faux et connexion sans TLS. Avec le certificat client signé par l’autre CA, le client MySQL reçoit `2013`. Une sonde OpenSSL TLS 1.2 de la même image établit la session avec le certificat valide et reçoit du serveur l’alerte fatale `unknown_ca` avec le certificat étranger. Ce témoin attribue le refus au certificat. Les deux exécutions se terminent avec zéro serveur, client, réseau ou volume de recette restant. Reçus `60b74177a247ea3a53b68319e424d19221c8e8c2e685c3edcae931f594186946` et `e400f1ce4428a9a37dcd0de646925462f9d89da5b8f10c4bd5046b545230f40d` sous `target/qa-evidence/mysql-two-profiles-transport-oracle.cl26jye0` et `target/qa-evidence/mysql-mtls-causal-alert.aNT3A4mM`. Reçu combiné `3bc20cb550e6dfb120b29d5bd24d50303b86d08d0097df13fcb85182d233812b`. L’adaptateur ai-guard et la recette applicative MySQL restent ouverts.

### Transport MySQL du diagnostic Rust

Dans le worktree local `codex/mysql-storage-20261120`, l’exemple optionnel choisit `mysql_async` 0.37.1, lit des références privées, limite la confiance à la CA configurée et vérifie le nom du serveur. Il compare la base et le compte authentifié avant de rendre compte de la version TLS et du mode demandé. Le premier essai montrait que la variante `password` ignorait un champ supplémentaire. Après fermeture de cette variante, les trois configurations invalides sont refusées avant connexion. Face à un serveur TCP muet, le diagnostic se termine en 8,023 s. Le serveur observe la fermeture et ne reçoit aucun octet.

Le binaire macOS ARM64 `6a60793d6f39d007323527ebac1a0e449def1c0789b92318f86e86ded37fe8cf` est compilé hors ligne à partir des sources exactes du transport, du diagnostic et de `control_fs`. Il passe cinq cas mot de passe et sept cas mTLS contre la même image MySQL 8.4.13 ARM64, via le port loopback de la recette. Les deux témoins positifs de chaque profil négocient TLS 1.3. Les refus couvrent le mot de passe, la CA et le nom du serveur, et en mTLS le certificat absent et celui signé par une autre CA. Ce dernier est corroboré par l’alerte entrante `unknown_ca`. Les deux nettoyages passent. Reçus `006e4bd0b3d8d04d753103abc5db65c82eda6accd548ae1a330b08977e8cbee7` et `a616ed42e046b39fd2c36c45f205fef6d5119f3e27d901d52bc6f0e31ec7ae87` dans `target/qa-evidence` du worktree SQL. Le test porte sur le diagnostic et un vrai MySQL. Le gateway, les écritures métier, les schémas et le navigateur restent hors de cette preuve.

### Initialisation et append de l’audit ordinaire MySQL

Le worktree SQL partage le codec de validation, de masquage et d’empreinte avec SQLite. Le premier `cargo check` du diagnostic dans le manifeste complet avait terminé avec succès. Un scénario Rust ciblé passe ensuite sur MySQL 8.4.13 ARM64 via TLS, à partir du schéma posé dans une base jetable. Il vérifie la genèse unique sous deux initialisations concurrentes, le refus d’un tenant ou d’un namespace divergent, l’expurgation et l’empreinte de l’événement, le rejeu à l’identifiant exact, le refus d’un contenu divergent, deux ajouts concurrents sans trou et la préservation du reçu après atteinte du plafond. Le nettoyage du projet de recette passe.

Le libtest `fff40fac0f4f04db5416b564bcdc41482da4e43b19fc8f9feee9800360eee65e` utilise 15 fichiers identiques à ceux du worktree et 154 dépendances verrouillées identiques. Le harnais de compilation sélectionne les sources du stockage et laisse le type de résolution fournisseur hors du parcours. Reçu `f01f098c58f1f7eaae02cc5209672292ad52490acc9c70501f6f387f3d72ff1d`, preuve de build et conducteur dans `target/qa-evidence/rust-mysql-audit-build.rn1v59n4` et `target/qa-evidence/mysql-audit-store-physical.hDCZwzZ5` du worktree SQL. Le passage couvre l’audit ordinaire avec mot de passe et TLS. La politique de privilèges de production, la détection d’un ancien préfixe cohérent, les transactions de contrôle, le gateway et le navigateur restent hors de ce résultat.

### Droits d’écriture et préfixe confirmé sur les deux profils MySQL

Le 20 novembre, le même test Rust ciblé passe sur deux bases jetables MySQL 8.4.13 ARM64, avec mot de passe et TLS puis avec mot de passe et mTLS. Le compte d’écriture peut ajouter des événements et mettre à jour les trois colonnes de tête. Il reçoit un refus SQL lorsqu’il tente de fixer la liaison. La maintenance initialise la cible et le compte d’écriture prolonge l’historique. Après le rejeu d’un ancien reçu, la même instance refuse un préfixe raccourci et une chaîne de même longueur dont l’empreinte diverge. Une instance neuve vérifie la cohérence de chaque état restauré et confirme que les refus ont préservé la tête. Le nettoyage de chaque recette passe.

Les deux essais initiaux restent consignés avec zéro exécution du libtest. Le diagnostic du profil mot de passe relève une sortie 126 pendant l’initialisation du serveur. Le passage vert emploie un montage SQL de recette en lecture seule. Son reçu est `a707439aae7026ab1f7409e9836b8e1c17477fde7a433bf9bdc06785c6505848` dans `target/qa-evidence/mysql-audit-role-sql-init.bD2T9nn1` du worktree SQL. Le binaire `9eb4822482c89eebe2c75d4c704019effadf7b9df275b0d7cb5f2708643b755e` est construit à partir de 15 fichiers Rust identiques et 154 versions verrouillées identiques. Le harnais sélectionne le code de stockage et exclut le gateway. Attestation de build dans `target/qa-evidence/rust-mysql-audit-floor-build.lH6vksSW`.

La garde de préfixe dépend de la continuité de l’instance Rust. La reprise après redémarrage, l’issue d’un commit non acquitté, les transactions de contrôle et les parcours applicatifs restent à qualifier. Le transfert entre bases revient à l’opérateur.

### Transaction de décision et reçu sur MySQL

Le 20 novembre, le scénario Rust passe sur MySQL 8.4.13 ARM64 avec chacun des deux profils TLS. La décision, son reçu indexé et la tête sont écrits dans une transaction. Deux propositions concurrentes pour la même demande rendent le même reçu. Une modification de la demande déclenche un conflit. La phase approuvée exige une revue durable et l’empreinte exacte, y compris en présence d’un reçu en cache. L’ajout ordinaire du namespace des décisions est refusé.

Sur chaque base jetable, un contrôle SQL force le code 3819 à l’insertion du reçu. Le test vérifie ensuite une tête inchangée et une demande absente. La suppression contrôlée d’un reçu dans la fixture fait échouer la vérification et le prochain ajout, avec tête et nombre d’événements préservés. Les vérifications d’audit ordinaire, de privilèges et de recul de préfixe passent aussi dans la même recette.

Le premier binaire avait échoué sur une erreur de syntaxe SQL 1064 dans la jointure. Le journal initial `25182673904a65ca5ab37c4121e4f316ceddc639ec4bc1fe0bd956094ee5182a` reste conservé. Le rejeu corrigé utilise le binaire `5d28521c602af8933696c326fef2227f8c50bc811515e692f2b5dcc7698c8f32`, construit à partir de 23 fichiers Rust identiques au worktree et 176 dépendances verrouillées identiques. L’attestation est sous `target/qa-evidence/rust-mysql-decision-recheck.uzxVGI9d`. Reçu du rejeu `66537d765e6cd0f88c08fdb7a4eb2ba680e5f035e8027f903c8c0c575e60c5c6` et conducteur sous `target/qa-evidence/mysql-decision-recheck.ISIbYf7D`. Le nettoyage des deux profils passe.

Ce résultat couvre l’adaptateur de stockage. Le raccordement du gateway, les tests de configuration en manifeste complet, la réserve de capacité, les mutations workers et relais, et la reprise après perte d’acquittement ou redémarrage restent à qualifier.

### Réserve des décisions sur MySQL

Le delta de capacité préparé à côté du gel SQL extrait le calcul commun à SQLite et MySQL. Le scénario Rust passe sur les deux profils MySQL 8.4.13 ARM64. Avec un plafond de 12 événements, l’ajout ordinaire s’arrête à la séquence 8. Les décisions atteignent 11 et laissent une place réservée à l’identité. Les refus sont typés et les deux types de rejeu conservent le reçu et la tête. Une politique explicite avec 100 octets ordinaires encore disponibles refuse le prochain événement ordinaire alors que des emplacements d’événements restent libres. Une décision passe dans sa réserve et son rejeu laisse la tête stable.

Reçu `eaca43f087378974734ea44cde55cf4aa6638065823b07b29a7088c08cedf619` et conducteur dans `target/qa-evidence/mysql-capacity-physical.nPFJtuz0`. Le libtest `fff1e99b15411f649b4b508e92ec964bc02bb246bf6fc6da656f710a0d795ab8` est attesté sur 25 fichiers Rust et 176 dépendances verrouillées dans `target/qa-evidence/rust-mysql-capacity.7FJgjQbM`. Les deux nettoyages passent. Le delta `3a80b7cf9649f0002c0a578e251690edf0ed05b7febd57309fb36d93a85061aa` est sous `target/qa-evidence/mysql-capacity-stage.h3PZgiDS`. Ce passage couvre le calcul partagé et l’adaptateur MySQL. Le Store SQLite complet, les écrits d’identité et les réservations egress restent hors de cette preuve.

Le delta a été intégré au worktree SQL avec le rétablissement de l’import `policy` utilisé par la supervision et le contrôle. La vérification complète précédente se termine avec sept erreurs E0433 liées à cet import. Le manifeste de la composition vérifie 914 fichiers, avec le test du vrai CLI sur le refus d’une sélection ambiguë. Le manifeste et la validation de la composition sont sous `target/qa-evidence/mysql-gateway-composition.6vv5LWDP`.

Le `cargo check --locked --offline -j1 --features mysql-storage --lib --bin agentsandbox --test decision_idempotence --test mysql_audit_store --test mysql_gateway_startup` de la composition passe en 5 min 14 s. Les 914 empreintes concordent dans la copie de validation avant et après le check. Manifeste `b54aee238d5ea0dda5a400fc154d3d7b98e2a39399bb22d675cfb3bac7dc6fac`, journal `baaf9d5b7189bc87fc78b7de981eadc1ceae00a1dd50c69fb8f7183c2b8fc00f`. Ce résultat porte sur la compilation. L’exécution du vrai CLI et des régressions SQLite suit séparément.

Sur le même manifeste, le vrai CLI refuse une sélection MySQL ambiguë avant lecture des identifiants ou effet sur le stockage. Les neuf régressions HTTP de décision et d’idempotence passent sur SQLite. Les 914 empreintes concordent après l’exécution. Journal `60ccd90ebe2bf0ecc9d05284834fd092b586ef7707e50f6dbc9bdf8ef078d412` dans le même dossier. Le manifeste précède le lot de réservation MySQL.

### Écriture sur cible MySQL préparée et réservation de worker

Le Store Rust passe sur deux bases MySQL 8.4.13 distinctes, chacune lancée en boucle locale sur Docker Desktop ARM64. Le premier serveur exige mot de passe et TLS, le second mot de passe et certificat client. Sur chaque cible, le compte de maintenance prépare deux événements, puis le compte d’exécution prolonge la chaîne. Le test compare directement les octets des deux lignes initiales, vérifie le lien avec l’ancienne tête, le rejeu et les refus de tenant et de namespace. Il accorde le dernier créneau de réservation à un seul des deux Stores concurrents. L’échec SQL d’écriture de la tentative annule la transaction entière. Une projection de worker modifiée fait refuser la vérification et le prochain ajout. Les deux nettoyages passent.

Reçu `9adfabd9a38d4da693927a8163326ea11383093053089f5e0af8ce0f162fc492` et conducteur sous `target/qa-evidence/mysql-worker-physical.eJGulHb4` du worktree SQL. Le libtest `7321a80abcaaf7219d4827763ad15627d12fdc838122e11c1c861112f8c6f407` est attesté sur 29 fichiers exacts, le module des DTO et 176 dépendances verrouillées sous `target/qa-evidence/rust-mysql-worker.6GbTi56T`. Cette recette couvre le Store MySQL. Le parcours MySQL via le gateway, la base témoin de ce parcours et le navigateur restent `NOT_RUN`.

Le `cargo check` de la composition avec réservation passe en 4 min 26 s avec les 128 avertissements déjà relevés sur la composition précédente. Il compile la bibliothèque, le CLI et les cinq cibles de test sélectionnées, dont `worker_registry` et `supervision_store`. Les 918 empreintes concordent avant et après. Manifeste `4688b27ae4084e85c01dbb87f7f748a01a48a3cb285965dfc2d5807f7b450587` et journal `df7f576040e4efb15a8a2b31a8a5698169147c261cec8b9b16d304a09a71404f` sous `target/qa-evidence/mysql-worker-composition.ZjhwqN3H`. L’exécution des régressions SQLite sur ce gel est distincte.

Sur ce manifeste de 918 fichiers, les trois sélecteurs exacts de régression SQLite terminent avec code 0. Les deux tests de `worker_registry` vérifient le quota et le rejeu après redémarrage ainsi que la reconstruction d’une admission approuvée. Le test de `supervision_store` vérifie l’admission supervisée atomique et idempotente avec compatibilité SQLite v2. Les 918 empreintes concordent après exécution. Reçu `ea977313d64c42a6daa3d40713b882b71459e51e6094f10c5842b0fde2a3b7a2` et journal `3a555d5cfe87bc77f1767845848f979de889029975c4695144dbc304cb74a6d4` dans le même dossier.

### Cycle de vie V2 sur MySQL

Le premier passage sur chacun des deux profils MySQL atteint l’assertion de restauration du dernier test de projection après le cycle V2, le CAS, l’observation incomplète et la reprise après rollback d’allocation. Le verdict complet est `FAIL` sur les deux profils. La requête de restauration cible toutes les lignes en révision 2, dont un worker déjà annulé par le nouveau scénario. Les deux projets jetables sont nettoyés. Reçu `8f5b599798658150d6700e382612da0d6a4c33c555c8d89530bd306309bc351c` sous `target/qa-evidence/mysql-worker-lifecycle.e9532ea06e`. La restauration du test Rust corrigé cible la seule identité modifiée par l’injection.

Le test Rust corrigé passe intégralement sur MySQL 8.4.13 avec mot de passe et TLS puis avec mot de passe et mTLS, avec un serveur à la fois sous Docker Desktop ARM64. Deux écritures concurrentes sur la même révision rendent un seul reçu frais et un conflit. Le rejeu du claim restitue `AlreadyConsumed`. Une observation terminale incomplète garde le quota. Une contrainte SQL sur l’allocation provoque l’annulation de la mutation. Une nouvelle instance retrouve la tête d’origine et l’allocation est absente. Après retrait de la contrainte, la même opération produit une allocation de 2 s non facturable et libère le créneau, qu’une nouvelle réservation consomme. Le rejeu de cette observation rend le même reçu sans écrire une nouvelle ligne. Les deux nettoyages passent.

Reçu `dbc21e9017a57da4f800fb7cfbfe597fc5d9d08db2ee9c28d334497747ff33c0` et conducteur sous `target/qa-evidence/mysql-worker-lifecycle-retry.MUNePUh3` du worktree SQL. Libtest `e3e04b37b23f58f53e4caf94af2ee21d8190aa718bcf22317786a5160c5a1e18` sur 31 fichiers exacts, les DTO et 176 dépendances verrouillées. Manifeste `d1901fc3f239919df3cb7a55685093bf5503d9a249931199ea13066adbcbd5c4` sous `target/qa-evidence/rust-mysql-lifecycle-retry.A3HKSgcd`. Il s’agit du Store Rust sur un vrai MySQL avec observations worker synthétiques. Le lancement physique, le raccordement du gateway, les claims supervisés et la perte d’accusé après commit durable restent ouverts.

Le `cargo check` du lot cycle de vie passe en 7 min 34 s pour la bibliothèque, le CLI et les cinq cibles de test sélectionnées. Les 921 empreintes concordent avant et après compilation. Manifeste `40002620b84f5618f94cdb7b18a22d3c7792499e3d8f2455bffdfcb790f691ad` et journal `31efe67a355806eb138a8d8f9afacccfe0aaf33111d9db9e68c3fe4b8d661cfb` sous `target/qa-evidence/mysql-lifecycle-composition.GJ4REKcW`.

### Lecture paginée de l’audit MySQL

Le Store Rust passe sur MySQL 8.4.13 avec mot de passe sous TLS puis mot de passe avec mTLS, sur deux projets Docker Desktop ARM64 jetables lancés successivement. La lecture parcourt les mêmes reçus qu’à l’écriture. Elle vérifie les curseurs exclusifs, l’ordre par séquence, les filtres agent, sandbox, projet, type et décision, la date de réception avec décalage RFC3339 et la recherche littérale insensible à la casse ASCII. La borne RFC3339 convertie en année 10000 donne une page vide comme SQLite. La lecture refuse un mauvais tenant ou namespace. Même la première page d’une ligne est refusée si un reçu indexé ou une projection de worker est modifié plus loin. Les deux nettoyages passent.

Reçu `f1a7ffe1151ec4d59524e20e41640de8b3679261835753bf3655de97dd5482f3` et conducteur sous `target/qa-evidence/mysql-events-physical.COJoyMyZ` du worktree SQL. Libtest `2e2aad66c82775245899b7cdd51ddc9237dc59db4100e7241e7756dbf63a7b4e`, 34 sources exactes, les DTO et 176 dépendances verrouillées. Manifeste `e2344d30f34691efa09d824d8118e0458fb83446fa7d3bb084c2af8638f8ba4d` sous `target/qa-evidence/rust-mysql-events-reviewed.aMbm8a6P`. Le raccord des routes MySQL, leurs autorisations et leur qualification de bout en bout restent ouverts.

Le `cargo check` de la composition suivante passe en 4 min 49 s avec la bibliothèque, le CLI, la lecture MySQL et les deux cibles sélectionnées du relais SQLite. Les 925 empreintes concordent avant et après. Manifeste `268deba1176072415baae4a7a5399c9a778902e7b71791588d969cea08c9b65e` et journal `878716df4fd7ba048bcc9d8d0a8aff7f8aab327b023e1f482cecee08d438978d` sous `target/qa-evidence/mysql-events-composition.ju0HBtgP`. Le lot sépare la validation et le format de journal des grants et de leur révocation. Le port du relais MySQL exige encore la lease de supervision et les bindings durables.

Sur ce gel de 925 fichiers, les quatre sélecteurs exacts de relais SQLite passent sous le profil loopback. Ils vérifient le cloisonnement tenant, le rejeu sans duplication de corps ni débit supplémentaire, l’arrêt de livraison après révocation et la réserve de révocation quand l’audit est saturé. Les routes HTTP sont exercées dans le processus de test avec le vrai Store SQLite. Les 925 empreintes concordent après le passage. Reçu `556dcce06b5f3311fa1fe37443b6c415eb06ea58624b56624640a722100556e6` sous `target/qa-evidence/mysql-events-composition.ju0HBtgP/relay-receipt.json`.

### Admission supervisée et format partagé

SQLite utilise maintenant le format commun d’admission supervisée. La transaction reprend la même validation de la demande et les mêmes champs pour l’événement durable et son reçu. Le raccordement de l’admission supervisée côté MySQL reste à implémenter. Le `cargo check` verrouillé hors réseau passe en 2 min 54 s pour la bibliothèque, le CLI et `mysql_audit_store`. Les 930 fichiers du gel concordent avant et après. Manifeste `25b04a6ee3dabe08b01cf442eac561551842e9e03daa4a512af5f62063127e14`, reçu `732f9bdc96f52a707af32b9d7e9ccf34602d73f707a78bfd389ff582b81a8d54` et conducteur `check.sh` sous `target/qa-evidence/sql-supervised-admission-codec.6qu30jv7` du worktree SQL. Deux régressions SQLite passent sur ce même gel. Elles vérifient l’admission supervisée atomique et idempotente ainsi que la validation de l’autorité du worker en conservant le comportement V2. Reçu `d02c35fc9b8d9d3f642cdfc067e307c5381d0f332c29ccba36290aa8cff6bb9c` et script `run-sqlite.py` dans le même dossier.

Le lecteur MySQL accepte le format d’admission supervisée après son marqueur durable et vérifie la projection d’intention préparée ainsi que l’index, le worker, la décision et le quota. Les dix tables sont attendues avant le démarrage. Le `cargo check` du lecteur et de la recette passe sur 932 fichiers. Reçu `437d715ef157c20aba8906aac760a3bc8bdfdfe9892e062b4ad88f704b0ec7de` sous `target/qa-evidence/mysql-prepared-admission-composition.ocv1ejpc`. Le gel suivant renforce la fixture avec le preset de supervision et les quotas CPU/RAM. Manifeste `453db590df166de05de0f96ca1e8f710ed86d2bacc372540dc3e6b5498ee00dd` sous `target/qa-evidence/mysql-prepared-admission-exact.wa4spd8x`. Le build du sélecteur exact passe sur ce second gel. Reçu `77224cd0e46915bfd04aaee37dab887b502b56c84c73ee07a65af1f663c6d19b`. Le test passe sur MySQL 8.4 sous TLS avec identifiant et mot de passe, puis avec certificat client et les mêmes exigences d’authentification. Chaque profil utilise une cible jetable distincte et son nettoyage passe. Le Store Rust vérifie l’admission et le refus préparés, le quota des workers et leurs allocations CPU/RAM, la conservation des octets historiques après ajout, puis le refus d’ajout face à une projection divergente. Reçu `8a635ab73c332c95b688f9ddb0516b5518bdd75bc5c9242d5cbc68ef05d6a51d` et conducteur sous `target/qa-evidence/mysql-prepared-admission-replay.qwvDvyfN`. La fixture prépare l’historique par SQL de maintenance. La preuve couvre sa lecture et l’ajout d’événements par le Store. La réservation supervisée est vérifiée séparément ci-dessous. Le raccordement au gateway reste ouvert.

### Réservation supervisée MySQL et reprise après rollback

Le sélecteur Rust `supervised_mysql_reservation_is_atomic_and_replayable` passe sur les deux profils MySQL réels. Le compte de maintenance établit le marqueur sur une cible vierge puis réserve sous le témoin de cette instance. Le compte d’exécution d’une autre instance relit le même reçu, refuse l’identifiant utilisé avec une autre intention et refuse une demande nouvelle. Le refus de quota est audité sans réservation et son rejeu restitue le même reçu. Le test compare les octets des deux genèses et de la décision d’origine.

Une contrainte SQL sur la liaison d’intention échoue après la préparation des projections worker et tentative. Le test confirme le rollback de l’audit, de la tête, du reçu et du quota. Après retrait de la contrainte, le même appel sur le handle d’origine réserve exactement un worker. Le passage par le compte d’exécution pour une nouvelle réservation et le raccordement au gateway restent ouverts.

Le build hors réseau vérifie le manifeste de 933 fichiers `5d3cc8b649a09837c0b7d4f0860516e7e587af32cd26284cb6a8224c9bf62c9b` et produit le binaire `67bc57b9bb1b37de64609bb4e7513700abea161b49abae6c99cd41646f2d2dc9`. Le premier conducteur transmet la variable de configuration des tests ordinaires au nouveau sélecteur et n’établit aucune preuve de réservation. Les deux serveurs de ce passage sont nettoyés. Reçu `6b6841360c83be19e73e7ade46ce71e483648e65ef470b6f52ad50a0051834ea` sous `target/qa-evidence/mysql-supervised-physical.a4kzmjaa`. Le conducteur corrigé rejoue le même binaire sur deux cibles successives et confirme les deux nettoyages. Reçu `28e46263dd3ed992e7b74a0a5f9fec1953c40d1f5ba66a8b2d592d677ed1691f` sous `target/qa-evidence/mysql-supervised-physical.zn9ucmh6`. Le build, `run-physical.py` et le guide de rejeu se trouvent sous `target/qa-evidence/mysql-supervised-reservation.q63r8lpr`.

### Bootstrap de l’origine de supervision MySQL

Le sélecteur exact `prepared_supervision_origin_requires_fresh_atomic_bootstrap` passe sur MySQL 8.4.13 sous TLS puis sous mTLS, sur deux projets Docker Desktop ARM64 jetables lancés successivement. Le compte d’exécution est refusé au bootstrap. Une contrainte SQL sur la projection annule les deux genèses et la liaison de la cible vierge. Le compte de maintenance rejoue après retrait de la contrainte et obtient le marqueur en séquence 2. Un nouveau handle relit le marqueur et refuse le bootstrap avec `Conflict`, sans récupérer le témoin d’origine. L’instance d’origine perd ce témoin après une divergence de projection observée. Rétablir les octets d’origine rétablit la lecture, tandis que le témoin reste absent. Le marqueur bloque l’admission V2 et son quota. Les deux nettoyages passent.

Reçu `ca8be33d56f5c5fdb835c2cbae0b36d66492c07c3b71fb2cf8d5abf301c7ee0e` et conducteur sous `target/qa-evidence/mysql-supervision-physical.2bf9c01946` du worktree SQL. Le champ `selector` identifie le test effectivement listé et exécuté. Le champ `test` du reçu conserve le nom historique du conducteur. Libtest `0692686c185af1d5c89bb589c2b1a279284c6ffc2ed51c358c8e53782a47cea7`, manifeste `732ed81d956ec2850f25c44c0454aa64a46262a038e5745fba0c7a1d853c92e4`, 37 sources exactes et 176 dépendances verrouillées. L’oracle rouvre un handle dans le même processus Rust. La continuité du processus MySQL et la panne avant ouverture de transaction restent hors de cette recette. Les claims supervisés et le raccord gateway restent ouverts.

Le passage de non-régression du parcours MySQL V2 avec ce même binaire sort sur `SIGABRT`. Le rapport système recoupe le gestionnaire de dépassement de pile Rust avec la relecture des projections worker dans la préparation de transaction. Le projet SQL de recette est nettoyé. Reçu `21fb74ba8c24a4af65dfbf37ac5ae3d320019688e5dead75ed01a8028278c418` et projection expurgée `b9ab8739a7e0ed6b6d5338f4276aef088268af29cbf57b83014384e7f1ef1aff` sous `target/qa-evidence/mysql-supervision-legacy.Zn9MHLFZ`. Le correctif place la préparation de transaction asynchrone sur le tas. Le rejeu conserve la pile par défaut.

Avec ce correctif, le parcours MySQL V2 passe sous TLS. Le sélecteur du bootstrap supervisé passe ensuite sous TLS et sous mTLS. Les trois libtests rendent 0 avec la pile par défaut et les trois nettoyages passent. Reçus `68aa83cb4d8724e69486ff4f45df14a76538acb752849ae9a7ca991a57798a67` et `6d8a914bc16af5759cfb229b96585438ae75dc585ac194d1d15883ff2a411f66` sous `target/qa-evidence/mysql-supervision-stack-legacy.e02esjli` et `target/qa-evidence/mysql-supervision-stack-bootstrap.flAwKmgM`. Binaire `4bceeb424c95e7238d9a527bb50aec236b7212016630b21c50a50a761f5f0a39`, manifeste `7c11364433ccde5428f219cbed1914c1d80023fb0c5b2dfc3b9b838a1b5032a4`. Le delta vérifié du libtest porte sur `src/mysql_storage/audit.rs`.

Le `cargo check` de cette composition passe en 4 min 59 s pour la bibliothèque, le CLI, `mysql_audit_store`, `supervision_store` et `worker_relay_api`. Les 929 empreintes concordent avant et après. Le check signale un import inutilisé de `conflict`, retiré dans le worktree après ce gel. Manifeste `935bdbb0d7c8027d6a89a987f41d97dbedb7d8b6c4b139cf27b416fcd5bbf583` et journal `b4ea05baab3aceea9b80aad20a71b54fb8782b1a885fbfdb1ccd43d40279f988` sous `target/qa-evidence/mysql-supervision-composition.jRdqyDy7`.

### Disponibilité de Kubernetes Docker Desktop

À 19 h 05 (Paris), `docker desktop kubernetes status --format json` rapporte encore `starting`, provisionneur kind 1.36.1 et un nœud en attente. Le passage ne lance aucun test physique. Reçu `b5bc2984bbbd6c14facb00df73a929fdf9f657ede88229d13151af168513e4e0` sous `target/qa-evidence/kubernetes-desktop-status-recheck.tlvMr29R`.

La dernière sonde du 20 novembre sur le candidat `5b3c2ed044d5` confirme l’état Desktop `starting` avec un nœud kind v1.36.1 encore en attente. Sur l’origine HTTPS loopback du contexte `docker-desktop`, avec vérification TLS configurée et sans proxy, `/readyz` rend EOF en 407 ms et sort avec le code 1. L’inventaire CSI et StorageClass reste `NOT_RUN`. Preuve `8e43fa741d6bbb2fb856d9476d8b1c33444fd755c446ec48c9d4355b3b4e282e` sous `target/qa-evidence/kubernetes-readiness-rerun-20261120-r1`.

Le 20 novembre à 10 h 46 (Paris), le seul appel à `/readyz` sur l’origine HTTPS en boucle locale du contexte `docker-desktop` rend EOF en 0,15 s. L’inventaire CSI et des classes de stockage ainsi que les tests physiques restent bloqués. La preuve SQLite avant effet reste indépendante. Rapport `0fa952ace0d8f22338c638ba7d21974f274e702a81114abf0ee44fc18f098112`, dossier `target/qa-evidence/kubernetes-local-readiness.MxgM2yje`.

À 11 h 17 (Paris), l’état local `docker desktop kubernetes status` renvoie `starting`. Un relevé indépendant retrouve l’écoute de `com.docker.backend` sur l’origine du contexte. Le TCP aboutit et la négociation TLS se ferme avant authentification. Aucun inventaire CSI ou StorageClass n’a été obtenu sur cet état du cluster. Dossier `target/qa-evidence/kubernetes-desktop-starting.wg9DdFuW`, rapport `031122a5c75d15ff4a175357625a571c0e7d29a342acdeb81f8f4b34a864f9b5`.

La vérification locale de 11 h 50 (Paris) retourne toujours `starting`. Elle s’arrête au statut Desktop. Rapport `b41da9472b92ef252ffc9bf6eabcb0913b9a88aa25be9309a168d1feebed8f25` sous `target/qa-evidence/kubernetes-desktop-recheck.mxjy2RDV`.

Le journal agrégé de Docker Desktop 4.99.0, build 245822, confirme que son backend contacte la même origine loopback que le kubeconfig. Un seul listener Docker est observé. Du 19 novembre à 21 h 49 min 39 s UTC au 20 novembre à 12 h 32 min 55 s UTC, le backend a journalisé 2 701 échecs sur `/api/v1/nodes`, dont 2 550 EOF seuls, 126 EOF avec expiration TLS et 25 expirations TLS seules. La frontière exacte entre le transfert vers la VM et le serveur API reste indéterminée. Le lecteur AWK ne conserve que des compteurs et ses bornes ont été vérifiées sur des données synthétiques. Reçu `5c9a0d5dd58b2f65e055793957daf2058e98d3d90f7336d158498aff5a3e5a9a` et lecteur `4ae3060c41d2e518751d9582b16fd878049103c5a596d9e331b19aee9dcd8401` sous `target/qa-evidence/g11-kubernetes-desktop-node-probe.KrXMaGnz`.

À 13 h 51 min 26 s (Paris), Desktop signale toujours `starting`. La sonde unique `/readyz` sur le même endpoint loopback retourne EOF en 135 ms. L’inventaire StorageClass et CSI reste bloqué. Reçu `3309532b7c22a78e8a9e7ed1c3697e4078df80c22cb23259c6ed2df349e489f7` sous `target/qa-evidence/g11-readyz-recheck.n96CUlNl`.

À 14 h 40 min 03 s (Paris), Desktop indique encore `starting`. La sonde locale unique sur la même origine, sans proxy, renvoie EOF en 78 ms. Le relevé garde le contexte et le cluster intacts. Aucun inventaire StorageClass ou CSI n’est obtenu.

Le libtest existant `e1f769d6d3e799cb5c0c71303bfdb58f77fbb115222d7ca27a953b9f8ebf9260` refuse d’abord le répertoire temporaire macOS de la fixture à cause d’un lien dans le chemin. Avec un dossier privé sous le chemin canonique `/private/tmp`, le même sélecteur `runtime::kubernetes::session_effect::tests::refused_guard_cancellation_or_changed_anchor_sends_zero_effect_requests` passe en 9,541 s. Le pair TLS en boucle locale vérifie zéro requête d’effet supplémentaire pour refus de garde, annulation ou divergence d’ancre. Aucun appel à un cluster. Reçus `270100307481809b71af634b16718e9039c28e791bb8ad86f218fe38f273a088` et `2978d3122ac0a43a7c16f7c48e01ba89610b084a93fe0433d98f1b66d8e8fb10` sous `target/qa-evidence/g11-before-effect-current-libtest.jmfcymai` et `target/qa-evidence/g11-canonical-temp-libtest.o_9t9ycu`.

### Chaîne GitHub de test et de release

La vérification locale de la candidate `5b3c2ed044d5fdf4bbfb7489762d1eb17684fcd6` passe pour le YAML avec refus des clés dupliquées et les quinze blocs Bash des deux workflows. Les fichiers sont présents localement et absents de l’index Git. La CI testée en lecture vise les pushes sur `dev` et `main` et les PR vers `main`. Le workflow de release est manuel. Il exige un succès de la CI de push sur le SHA exact de `main` avant de créer le tag annoté. Il prévoit ensuite les archives Linux x86_64, macOS ARM64 et frontend, puis leur dépôt avec `SHA256SUMS` dans un brouillon GitHub Release.

Les exécutions hébergées et le retéléchargement des quatre fichiers restent **NOT_RUN**. La SBOM et la provenance exigées figurent parmi les écarts ouverts. Reçu `79c765df224e56342a7f3b92e673689ba84ed90dda6a9f0a479102855c0f23f8` sous `target/qa-evidence/g12-workflow-static.reirwkju`.

L’exécution locale des gardes Bash exactes avec réponses GitHub synthétiques et réseau interdit donne douze PASS et un FAIL. Les branches, SHA, résultats de CI et versions divergents sont refusés. Une réponse CI déclarant le succès sans `html_url` laisse passer la garde et écrit `ci_url=null`. Le statut de `printf` perd le statut d’échec de `jq` dans sa substitution. Les workflows ont gardé les mêmes empreintes. Reçu `e0e36e0cb537dc1636ef4032affce867cd7d9d2fae8e3241195839c9989a02ac` et conducteur `23317137e9cd939af47ef3205d3819f0e789ac7c1378edfc8e9e960acc1ab554` sous `target/qa-evidence/g12-release-guard.qYcb4iJq`.

Dans le checkout principal, le workflow bloque désormais avant sortie si le lien CI manque ou ne vise pas un run du dépôt courant. Les 17 cas Bash passent avec réponses synthétiques et réseau interdit. Le contrôle d’identité CLI en CI exige un seul objet JSON et passe ses cinq cas, dont le refus d’un flux à deux objets. Reçus `c00f181790c3c04100c3353b5da6e54754b187a1b30ebed391f3388a98d41e4b` et `c3cdca3305defce417d32c8459209afdbc4b7eec9e2eb63970f5d0f18de0d470` sous `target/qa-evidence/g12-gate-ci-url-fix.wm14xe6k`. Ces corrections sont locales au principal. La chaîne GitHub hébergée reste `NOT_RUN`.

Le bloc Bash de dépôt du principal compare la taille et le SHA-256 de chaque reçu et de chaque fichier du brouillon à l’artefact local. Les douze cas hors réseau passent avec trois archives de recette et `gh` simulé. Ils couvrent le brouillon neuf, la reprise avec vérification des octets déjà présents et les refus de taille, d’empreinte absente ou divergente et de nom incorrect, y compris pour `SHA256SUMS`. Reçu `8815d0e2d7c0e83f148fae0d5f6154915b655bc338f9bb83f0152d484eb75738` sous `target/qa-evidence/g12-release-asset-gate.wWEa0068`.

Un gel privé reprend les deux workflows de la candidate et applique les trois gardes validées sur le principal. Les 17 cas de sélection, les 5 cas d’identité, les 12 cas des fichiers de release et les 9 tests du prévol fournisseur passent hors réseau. Les 15 blocs Bash passent aussi la vérification de syntaxe. Le premier gel de recette omettait une entrée navigateur nécessaire au prévol. Le rejeu l’inclut et l’inventaire recense les dépendances locales non suivies. Reçu `761c0a3eac5e7a8686d0df76bec07d85433956904d461ca8a73ca19a5a02e30b`, patch et inventaire sous `target/qa-evidence/g12-workflow-composition.8xfg4cn4`. `actionlint` est absent localement. L’exécution hébergée et la création de release restent `NOT_RUN`.

Le bloc Bash exact du tag passe trois cas hors réseau avec un faux `gh`. Il crée l’objet annoté et sa référence au SHA sélectionné. Après une réponse perdue, le même run relit le tag et n’écrit plus. Un tag rattaché à un autre commit est refusé. Reçu `458b311010be6cbd41931ee4bfe4ca9d7af8cf9a818f25b838ef6e80cc2a165a` et script `verify-tag.py` sous `target/qa-evidence/g12-workflow-composition.8xfg4cn4/release-tag-qa.aGlxEn`. Les droits et l’exécution GitHub restent `NOT_RUN`.

### Corrélation entre le registre lu et les lignes rendues

La vue Workers expose la date exacte de la page acceptée dans un élément HTML `time`. Elle conserve le même libellé et le même format visible. Le conducteur Chromium rattache les identifiants du worker et de sa session, leurs états visibles et le quota retenu à une seule lecture complète et validée. Si une autre lecture en cours ou reçue remplace le résultat pendant l’évaluation du DOM, il reprend la mesure dans la même fenêtre. Une date identique pour deux réponses ne permet pas d’attribuer la page à l’une d’elles. L’inspection Docker après le rendu garde son rôle de confirmation du chevauchement.

TypeScript, le bundle de production, les neuf tests de contrôle et huit groupes de canaris Node passent. Le témoin Node simule notamment des mises à jour avec des dates distinctes dans la même seconde, deux lectures indiscernables, une lecture en vol et une session étrangère. Le canari Rust `live_browser_view_requires_the_rendered_read_even_after_revalidation` passe. Dossier frontend `target/qa-evidence/frontend-revalidation-build.wwOJ0JgT`, manifeste des entrées `c0694b6ef3e75444b5e991fc7c8e7aa48100d78410277a368c4e33108402ca96`, manifeste du bundle `a4b745cac2c64b8c61afe2936641d7a1d7f0bd86b0d3b2b7ef011de595d50861`, journal du canari Node `cc637fd00fa9416022a97717968f34e117ba3942ab59d4fd186b73a95b49a057`.

Build Rust `target/qa-evidence/parallel-causal-render-build.XUL3b8e3`, manifeste `09afe36dbf480d15c8bdc52364767ee53cc36cab8d01f4c1d2714cc5bd34d68e`, binaire de test `2e6f20fed5bbfcefa34e32f3cf17f08e6fd967ae760f5741a029e0161d17f1c5` et journal canari `7973cee2c5c8fb82c4bf145db7a5b0e9f52cdeb23bb2cea379c49a3b50435a6f`.

### Visibilité de la seconde ligne du tenant A

Le premier rejeu physique avec la corrélation du registre rend FAIL en 119,43 s. Le tenant A franchit la vérification de l’horodatage après deux réponses et échoue avec `row_visibility`. B passe après une réponse. Aucun signal `DISPLAYED` n’est émis pour la paire. Aucun conteneur de l’image QA ne reste actif. Rapport `728cfaa03abe384319673857b77d7ad8641cb8f529fcf87765c66731dea1e019` et journal `0a158d848f46534da4a7ddb6193f354474aa39bac4b36f526e9091c3184d6cd5`, sous `target/qa-evidence/docker-chrome-causal-render.Ekaq0mOL`.

Sur la capture HTML assainie de A rendue hors réseau avec le CSS exact et une fenêtre de 1440 × 1050 px, la deuxième ligne commence à 1092 px. La centrer masque le statut sous la navigation fixe. Le conducteur calcule un seul défilement vertical qui garde le statut visible et reprend le même contrôle de la date, des lignes et des champs. Sur la capture rejouée dans Chromium, son oracle passe de `row_visibility` à `scrolled`, puis à `ready` après 122 px. Ce rejeu confirme la géométrie de la capture. Les faits moteur restent du ressort de la recette physique. Rapport `e31c87e1f7f648c47048cbb64b331eb8dd953efa4b20b8866d27f986beb710e4` et PNG assaini `636bdf7208d2d5e06f9ae712c293f741ce3b8d6814c1c0de934d5e24e4c14f17` sous `target/qa-evidence/chrome-workers-bounded-scroll.1DDqQFvU`.

Le rejeu physique du conducteur avec défilement rend FAIL en 217,24 s lors de la fermeture de Chromium. Dans le code Rust du binaire utilisé, le message `live dashboard closure report missing` n’est l’erreur principale qu’après validation de `DISPLAYED` pour A et B et retour réussi de l’inspection des trois conteneurs exacts. Le journal indique que la fenêtre de 15 s expire avant le rapport de fermeture. Les détails et dates de l’inspection ne sont pas conservés dans cette branche et la continuité finale avec les allocations n’est pas validée. Après la fin de la recette, le moteur ne compte aucun conteneur actif de l’image QA.

Rapport `94d1e5095a5f0a9c4dd4a7e8a85714f9a878158033329f33f5facf938dd80c88`, log `c00901b63c3bde96724dfd74095f23564b21ab3202aa1aace3c220aad566827f`, manifeste d’exécution `b4cd43059623c221c2420df91368e66fda1c965cdca4c9e606394c97b19361d6`, sous `target/qa-evidence/docker-chrome-scroll-confirmation.Rr09jVhZ`. Le harnais consigne désormais les comptes de rendu et d’inspection en cas de fermeture incomplète, tout en gardant le verdict d’échec. Le raccord compile et le canari Rust de la validation du rapport passe. Manifeste `41fe9052e3e1e17021f783f1f2b38386a9cd06677e444fc8b129d5f32ad5e3d6` et libtest `4ea217d6c8ed4568697080cad67ee7fb4c263965c0bf608e73b6e5e7141cb495`, sous `target/qa-evidence/parallel-closure-diagnostics-build.92FciLUn`. L’émission du nouveau diagnostic lors d’une fermeture physique reste à exercer.

### Pilotage Tokio du canari de réconciliation

Le même libtest qu’au premier essai atteint cette fois `running_ready`, `cancel_ready` et `setup_complete`. Il demeure à `run_start` jusqu’à la coupure externe de 600,02 s, code 124. Après la préparation, la fixture appelait `Handle::block_on` hors de tout `Runtime::block_on` sur un runtime Tokio `current_thread`. Le GET d’authentification et son minuteur dépendent du pilote conservé uniquement par `Runtime::block_on`. La fixture rétablit cet appel pour le chemin synchrone. Le seul chemin froid utilisant un handle continue de l’utiliser pendant que le runtime attend sa tâche bloquante.

Le changement porte sur la fixture. Ses délais et ses oracles sont conservés. Journal `bcc9781d58590273b8edde269baa93c9d97079419b70beb292fedf4a902ff2d0`, rapport `3c77a04d3c7b4c00d12163462f50dccb2ec088714ab48ffa19c1c8abd9292ca8`, sous `target/qa-evidence/broker-cancel-extended-observation.o9wdZFRz`. Le correctif formaté a pour empreinte source `ac1413f9494f52a2a97893534525a49c349507a4e9ded1492c006615698e92a7`.

Le libtest corrigé compile avec vérification des 1 126 entrées du manifeste avant et après le build. Le test à chaud se termine sur la borne externe à 600,02 s, code 124. Il atteint `setup_complete` après l’Arm, le claim et l’annulation, avant le marqueur `run_start` du pilote à vérifier. Le test reste sans verdict sur la réconciliation. Libtest `cc0a878920b7688b3111584e8f7f2c49677f42ecf2acba7ef534b2830f96bbfb`, journal `083248d2f7b454af969c4df061538b266056ffc39b2ed43af359206fe1056c68` et commande dans `target/qa-evidence/broker-cancel-runtime-driver-validation.Hozg2eY7`.

Le test à froid du même libtest expire à 600,01 s, code 124. Il atteint `context_ready`, puis la coupure de la réponse HTTP du handler de transition et le marqueur `executor_complete` du chemin Tokio `Handle`. Le journal s’arrête avant la preuve de publication StopOnly et la relecture des ACK. Le résultat du test est FAIL. Cela établit le retour de l’exécuteur sur ce chemin, sans verdict sur la reprise. Rapport `result-cold.json` et journal `8784de1502d61d96c860288f74a0c5703d44786b75f52dccccefffe2eacf166a` dans le même dossier.

La candidate ajoute des marqueurs de test fixes après les lectures Store et Journal, après la réouverture de la lease StopOnly, après la publication et après la vérification des ACK. Le chemin à chaud distingue les lectures de préparation du début d’exécution. Les délais et les assertions sont conservés. Format Rust et application vérifiés. Compilation et exécution de ces nouveaux marqueurs restent à faire.

### Choix du moteur Rust dans le CLI et Chromium

La candidate exécute `tests/e2e/run_browser_decision.py --judge-mode rust` avec le CLI `642f82fcef331a0d9e76e0f4d2cf5669623ab950d5f1ea4081a46effc049e139` et le bundle du manifeste `a4b745cac2c64b8c61afe2936641d7a1d7f0bd86b0d3b2b7ef011de595d50861`. Le gateway et le dashboard Rust servent deux tenants neufs en boucle locale. A obtient une autorisation sur liste explicite puis un refus impératif `strict_default_deny` avec `skipped_local_policy`. La vérification d’audit CLI compare la tête et le compte des trois événements du tenant A à ceux retournés par le gateway. Chromium confirme l’affichage des décisions attendues, leur export et l’absence de la décision témoin de B. Aucun modèle ou worker n’est lancé.

Rapport `fc0583712742b6ba64de5d7406f863ab87bdf78af4296040ab668d57876456fe` et relevé des décisions `d7d9b69a70fd1eeaa73ce8e0a8206a126297e21346afe19f3392e6b3786a52ca` sous `target/qa-evidence/rust-decision-functional.sCfDCDRq/rust`. La provenance Jev réelle et l’inférence Qwen physique restent des preuves séparées.

Avec le même CLI et bundle, `--judge-mode local-fixture` passe sur deux nouveaux tenants. Le vrai gateway conserve le refus après l’avis de revue synthétique, le refus impératif qui ne sollicite pas le backend et `decision_backend_unavailable_or_invalid` après HTTP 503. Le scénario vérifie la chaîne CLI et les décisions affichées et exportées dans Chromium. Il utilise un service HTTP synthétique local et aucun modèle ni worker. Rapport `657ad512f10bf1039fd67967736c095ca5f404185079af670177dc35a572825d`, preuves de décision `ec571171721a92b3507c50cac71b87bf613c2e9590ac8d6f6f2b1941b352a02f`, sous `target/qa-evidence/rust-decision-functional.sCfDCDRq/local-fixture`.

### Fermeture Chromium en boucle locale

Le canari de fermeture reprend la garde Playwright, un parent qui envoie `close` et deux contextes Chromium fermés dans le même ordre que la recette Docker. Il obtient le rapport en 2,385 s sans flux et en 3,732 s avec les deux flux SSE actifs, sous la borne conservée de 15 s. Les deux corps SSE se ferment côté serveur de recette avant la fermeture du serveur, et les deux processus Node sortent naturellement. Ce témoin ne reproduit pas la charge Docker, le proxy Rust ou une lecture Workers en cours.

Dossier `target/qa-evidence/docker-chrome-close-observation.4kKt2pRt`, rapport `033b0dfe0d34e22fcc042b1077587c3b3869faea5d5c54848b434278e8fcd50e`. Reproducteur `target/qa-evidence/docker-chrome-close-canary.aZdVQjrU/close_canary.mjs`, SHA-256 `5ae567dc3290d2295ad5b096b175990dbc7d560322400b43cb9973c67e3caeb4`. L’échec mesuré sur le parcours Docker intégré reste ouvert.

Un deuxième canari maintient le dernier octet d’une réponse JSON de revalidation du tenant A, avec les deux flux SSE en cours. La commande `close` arrive au processus enfant. Les deux fermetures de contexte aboutissent et les trois réponses HTTP sont fermées avant l’appel à `browser.close()`, qui reste en attente à la borne de 15,001 s. Le processus Node émet encore sa pulsation à 14,028 s. Le canari rend FAIL. Il reproduit la classe de blocage sans Docker ni proxy Rust. Le journal Docker précédent ne consigne pas quels GET étaient en vol au moment de `close`.

Rapport `029578034a2bf711aa6be8414f20dda7271a5a56e1a4d3f44efaea45d9e2f03e` sous `target/qa-evidence/docker-chrome-close-pending-observation.vc9UeUuB`. Reproducteur `target/qa-evidence/docker-chrome-close-pending-read.Dt3XcSza/close_canary.mjs`, SHA-256 `5dea95035a4726526fe459d06bb6305d4730828975975a7efe328455a4700009`.

La variante qui passe chaque contexte hors ligne et arrête le chargement de la page ferme la réponse en vol dès 19 ms. Les deux contextes sont fermés à 121 ms. `browser.close()` reste en attente à 15,000 s. Cette interruption du transport ne suffit pas pour la fermeture du navigateur dans ce témoin. Rapport `773fe2584ea6bea7805b4c62c4404620ac325734e483fe22e3aba5e539067e95`, script `620445e52173c586ff5e28c459c68c5e9a7b446f7928b379a9a9854c31c0dde4` sous `target/qa-evidence/docker-chrome-close-abort-observation.Zej362z1`.

Un autre canari traverse le vrai dashboard Rust `--shared-tenants`, son listener HTTP/1 et deux SSE amont synthétiques. Chaque tenant s’authentifie comme viewer, ouvre un flux et le ferme pendant que le proxy reste actif. Les deux contextes ferment à 150 ms. Le rapport de fermeture manque à 15,004 s après entrée dans `browser.close()`, alors qu’aucune lecture JSON n’est introduite par ce canari. Dossier `target/qa-evidence/docker-chrome-close-proxy-observation.EXZdeqg9`, rapport `d37c753462f7a86566e72d981c8bb71f8d2c782a0d4de734e81ce4d70092a040`. Ces canaris bornent la phase du blocage sans en attribuer la cause interne.

Deux autres contrôles gardent la borne de 15 s. Fermer le navigateur avant les contextes avec la réponse JSON en cours ne rend aucun rapport dans la fenêtre. Laisser la page consommer cette réponse entière avant `close` ne suffit pas non plus. Dans ce second contrôle, les deux SSE sont encore actifs à la demande de fermeture, les deux contextes répondent en 45 ms et `browser.close()` reste en attente. Les rapports sont `8c99d76626dd4c56a53777ed6a8c3f69ff430a02ab6dcce594ebf6dcdda72800` sous `target/qa-evidence/docker-chrome-close-browser-first-observation.TUaxU1eK` et `e4c1cfe09f3ec78db6db158f070bf4348432c251db5d3ccfc9d8fc44e1135af7` sous `target/qa-evidence/docker-chrome-close-json-finished-observation.s7k4sC8o`.

### Sortie du processus Chromium et fermeture des flux stdio

Le même canari à réponse JSON consommée échoue aussi avec le Chromium Headless Shell ARM64 en cache, version 148.0.7778.96, révision 1223. Les deux contextes et les connexions amont se ferment avant l’échéance de 15 s. Cette comparaison utilise une révision distincte de la révision 1234 déclarée par Playwright 1.62.1. Rapport `7391f7307fec852e0fbc734c39b4b3ef398ab802e59a3c4547a31d6b1d214775` sous `target/qa-evidence/chromium-shell-close-comparison.QMjWyJyD`.

La sonde du processus exact lancé par Playwright enregistre, pour Chrome installé, le début de fermeture gracieuse à 273 ms et l’événement Node `ChildProcess.exit` à 11,469 s avec code 2, sans signal. La recette expire à 15,003 s. Le repli force la fermeture à 23,006 s et l’événement `ChildProcess.close`, qui inclut la fermeture des flux stdio, arrive à 23,011 s. La preuve distingue l’événement de sortie OS du retour de la fermeture Playwright. Elle ne localise pas encore le flux restant ni l’origine du code 2.

Rapport `b484400bdd189b36c82c34ff466caeb0ffbc685dae20b20e36d88e2bc70395d8` et sonde `ef5e7bed2468322a16db9d32ce85c7c5db56ac31c47e219ed09b0377455639a3` sous `target/qa-evidence/chromium-owned-process-lifecycle.ptfjA7CU`. Les marqueurs ne conservent ni les arguments de lancement, ni le contenu de stderr.

Un relevé plus fin du même processus observe la fermeture des deux descripteurs DevTools 3 et 4 vers 0,942 s. Le processus principal sort avec le code 2 à 11,718 s. Les descripteurs 1 et 2 restent ouverts à 15 s et se ferment à 23,013 et 23,016 s, après l’arrivée du repli. Le code source Chromium nomme le code 2 `RESULT_CODE_HUNG`. Le watchdog de fermeture macOS prévoit ce code à son échéance. La durée et le code observés sont compatibles avec ce chemin. La tâche interne bloquée et le détenteur des deux descripteurs restent à identifier. [Codes de sortie Chromium](https://chromium.googlesource.com/chromium/src/+/refs/heads/main/content/public/common/result_codes.h) · [Watchdog macOS](https://chromium.googlesource.com/chromium/src/+/HEAD/chrome/browser/shutdown_watchdog_mac.cc)

Rapport `2cd8e8004328d58898048d24935aac4edc0a38f61ab4d0e4d1b58274023c9cff` sous `target/qa-evidence/chromium-stdio-observation.AKRo5q0s` et sonde `59d4a6e8db0c8d815f2aa71ac6d1680ff2a6b01e51d299b88e5e607263a5a2c8`. Le rapport contient des numéros de descripteurs et des jalons, sans données brutes du navigateur.

Le navigateur Headless Shell exact de Playwright 1.62.1, révision 1234 et version 151.0.7922.34, reproduit le dépassement de 15,001 s. Les descripteurs 1 et 2 restent ouverts. Les événements de sortie du processus et de fermeture de ses flux arrivent après le signal de repli `SIGKILL`, vers 23 s. Le test reste FAIL sur ce navigateur aussi. La dépendance navigateur a été téléchargée dans le dossier privé de recette. Rapport `74b62585ba89dee93c4f1ae8237539fb487a5934483ff5018d19762b2406dbbc` sous `target/qa-evidence/chromium-pinned-stdio-comparison.aLxskYnV`.

Un dernier contrôle rend chaque contexte hors ligne, désactive l’interception de réponses sur la session DevTools qui l’avait ouverte, puis détache cette même session avant de fermer les contextes. Les deux détachements réussissent. La recette atteint encore 15,002 s en attente de fermeture. Chrome sort avec le code 2 à 11,554 s et les descripteurs 1 et 2 restent ouverts jusqu’au repli. La variante reste dans la copie QA. Rapport `1cfa991fd7f330dd40e9599b975e7f201983dbbd94f4bfab7d1bf8fc35f73f8e` sous `target/qa-evidence/chromium-guard-session-release.pZZMDFOg`.

Un contrôle avec un seul contexte est réalisé sur la même recette avec JSON entièrement consommé et un flux SSE actif. Le contexte ferme à 29 ms et le flux SSE termine côté serveur. Chrome sort avec le code 2 à 11,399 s. Les descripteurs 1 et 2 restent ouverts au-delà de 15 s. Le blocage est donc observé même avec un seul contexte. Rapport `495ef82c9fcf6e35694ab482c7659bd17aacbed471a4b4cfdf0181c16f849ad3` sous `target/qa-evidence/chromium-one-context-observation.fqDWoJDt`.

La variante avec `BrowserServer` garde deux contextes et relie le driver par un seul WebSocket en boucle locale. Les deux contextes et le driver ferment, puis le processus Chrome sort avec le code 2 à 11,380 s. Ses flux se ferment à 11,383 s sans repli observé. Le parent reçoit le verdict à 11,422 s et le test rend FAIL pour la sortie du navigateur. Rapport `11d5bbcb39f9edcf9f5ec6e33c59f539c2c9c24381fbb0d483652b72a90c8aa3` sous `target/qa-evidence/chromium-launch-server-observation.ai1jLwVb`. Ce chemin reste un canari privé.

### Une décision Qwen physique après démarrage

Le profil Docker neuf démarre le modèle `qwen2.5-0.5b-q4km` et le relais local avec les mêmes images épinglées qu’au prévol. Le réseau d’inférence est interne et seul le relais publie un port sur `127.0.0.1`. L’identité, la route et le compteur de redémarrages des deux services restent inchangés entre mesures. Le gateway sélectionne `local_openai` par le CLI. Le relais de recette compte un POST. Le client du modèle atteint sa limite de 12 s avec `transport_unavailable` et le motif `timeout`. L’audit conserve le refus `decision_backend_unavailable_or_invalid`. Le conducteur rend FAIL à `real_qwen_decision`, avant Chromium. Aucun worker ou second appel modèle n’est lancé.

Le pic cgroup du même conteneur passe de 337 936 384 à 371 437 568 octets, sous la limite effective de 999 997 440 octets, avec zéro OOM et swap désactivé. Le conducteur arrête uniquement le projet créé et vérifie zéro conteneur actif de ce projet. Le relevé confirme le refus durable dans le chemin physique de temporisation. Une réponse Qwen exploitable, son affichage et la reprise à froid restent à démontrer.

Dossier `target/qa-evidence/qwen-local-physical-functional.hR0dE7O3`. SHA-256 du conducteur `ef91b0d9b0335b35208493f16f21a8072397a9d174b0873b12f8655fa7165f20`, du rapport `d448e8e65046180a7ce4bb81d8bb6d2e546e8997eedb934984a9a08a1e4c9cec`, du reçu du parcours `7bf7154553653f2e793e5ae3343afc991b5bbd77023b9ff70867865277096400` et des mesures `e6b2ff3af9a17c75b2fb10b85fc4f2301dbeb1b2020b414de473155d69ed99e8` et `c70b981943681842874c877111149eac07892c667a7aa8462162cfbf1c4beeee`. La procédure est décrite dans la section « Rejouer Qwen avec le modèle réel » de `tests/e2e/README.md` sur la candidate.

### Disponibilité Qwen par le port publié

Un nouveau projet de recette reprend les deux digests Docker. Le vrai CLI exécute `healthcheck` sur le port publié en boucle locale et constate `/health` disponible à travers le relais Rust. L’identité du modèle et du relais reste stable et le pic cgroup est de 292 667 392 octets aux deux relevés. Le conducteur ne fait aucune requête d’inférence, arrête son projet et constate zéro conteneur actif. Ce contrôle porte sur la route HTTP de santé.

Dossier `target/qa-evidence/qwen-loopback-zero-inference.BppdlSxg`, rapport `eb18fa48b6cd59e8f2de87dcc7d89d85e845d595b8c3be2a76791b20d305558e` et conducteur `7800b04e7dc779a779d31dbf3768c6fe247d04a7b6dce5ee04e4d0b8ab8011e5`.

Le relais de test compte un POST avant l’envoi amont et lit le corps entier avant de répondre au gateway. Le timeout précédent ne localise donc pas le retard côté modèle. Une copie QA ajoute des marqueurs bornés à l’envoi, aux en-têtes et à la lecture du corps. Elle conserve les limites et les délais existants. Deux tests en mémoire passent en couvrant l’échange complet et le timeout aux trois frontières, après avoir corrigé l’état du faux thread de test. Ils injectent les réponses et n’ouvrent aucun socket. Relais `a99c5c4317899ce6de0725c9467edb8f083f7130fbc4b74f9390fb0e009f16b4`, test `9f25cc08ed42028e9692f63b90f9fbbd2fa13e2ab63ed8a3e32f7f67c63cd50e`, journal `be00dda77487c03c3af30e10a109ed9838a32a8e60f139736f385340508b5aca`, sous `target/qa-evidence/qwen-local-relay-phases.kXKrgW3S`.

### Prévol du parcours à trois workers Podman

Une lecture du socket Unix déjà utilisé par le test mono-worker confirme le mode rootful et le même digest d’image local. Aucun worker ou bootstrap d’autorité n’est créé. Rapport `6ab9b72b5c8f38c9ac84516347e82f39a68e684a9b7b057fd2bb050e98d105b1` sous `target/qa-evidence/podman-three-readonly-preflight.aC280a0Z`.

La proposition de test passe l’application à blanc et le formatage dans la copie QA. Elle conserve les trois soumissions CLI, les oracles d’isolement et de quota, les inspections rattachées aux allocations et les exports distincts. Dossier `target/qa-evidence/podman-three-cli-delta.no2eEcVY`, patch `a7a3f563443280472262329184a71a0ea71403a25a89d9cfd23934e851780cb8`. Compilation et passage réel à trois workers restent à établir. Le navigateur sous Podman garde sa qualification distincte.

La révision de la proposition exige une autorisation explicite pour le mode rootful. Si le socket atteste le mode rootless, le conducteur poursuit en rootless et ne transmet pas l’option rootful au CLI. Type et format des lectures d’identité et d’inspection vérifiés statiquement. Dossier `target/qa-evidence/podman-three-cli-mode.M5PTigi7`, patch `ad61b7b5ac0ed86f13191bdc2cd385b2344a6fe100c47b9df6acd84459ecaa55`, postimage `42d66e0c4e696d24888d56951ae5dded74dbf465d321a07804bf4109fdf7509b`.

### Un POST Qwen mesuré aux frontières du relais

Un projet local neuf réutilise les deux images épinglées. Le vrai CLI reçoit le contrôle de santé via le port publié. Le relais de recette valide un seul POST et l’envoie au relais Rust. Il ne reçoit aucun en-tête de réponse à 14,003 s, tandis que le gateway atteint sa propre borne de 12 s. L’audit contient le refus durable. L’envoi et l’attente sont mesurés depuis la connexion au relais publié. La réception par llama.cpp et la phase interne du modèle restent inconnues. Le conducteur rend FAIL avant Chromium. Aucun second POST n’est lancé.

Le pic cgroup du même conteneur passe de 292 618 240 à 324 333 568 octets, sous la limite prévue. La route reste identique, puis le conducteur arrête son projet et observe zéro conteneur de recette actif. Dossier `target/qa-evidence/qwen-local-one-post-instrumented.CJyYHeTY`, conducteur `a51fd4dd5c3f2a9aa7060ed76cbf46e45edd9a91d2b323695c6408c9e0fb1eaa`, rapport `27079e146e7e79a54d350f086cb193fef248631fcfd771767d5a0674a26c4013` et reçu de décision `d33677fdbc0cba8f712e449f1e50a025ffc5361a74fdc39de0d0a94e8a2be2a1`.

Le conducteur installé dans la candidate poursuit désormais le parcours après un refus durable du backend. Un serveur de recette loopback rend une seule réponse HTTP 503 au mode `local-real`. Le vrai CLI et Chromium vérifient le reçu rejoué, la tête d’audit, l’export JSONL et l’isolement A/B. Le témoin passe avec `fail_closed_proof_verified: true`. Le parcours nominal garde `FAIL` et le code 1. Dossier `target/qa-evidence/qwen-failclosed-checkout-fixture.P02niQfl`, rapport `a785db3a1023447e0862844d87cc7b8f4c5ca00c327e25eb53caac0af86fea9e`.

La lecture de llama.cpp `b11718` permet de distinguer l’admission de la tâche, le progrès du prompt et la boucle de génération. Le journal peut exposer l’admission. Les traces périodiques de progrès et de génération ont une fenêtre de trois secondes. Le premier token met à jour des compteurs internes, dont une trace périodique ne donne pas l’instant exact. Avec `stream:false`, l’absence d’en-têtes avant le délai laisse ces étapes inconnues. Le profil coupe la journalisation du conteneur modèle. Un prochain relevé doit filtrer les lignes en mémoire, conserver les phases et des compteurs, et garder `UNKNOWN` pour une phase sans couverture attestée. [Code llama.cpp épinglé](https://github.com/ggml-org/llama.cpp/blob/b11718/tools/server/server-context.cpp#L1805) · [Boucle de génération](https://github.com/ggml-org/llama.cpp/blob/b11718/tools/server/server-context.cpp#L4059)

Un filtre QA privé borné projette les phases, compteurs et temps relatifs en JSONL. Douze essais synthétiques passent. Deux exécutions du CLI du filtre distinguent `OBSERVED` avec l’attestation indépendante du conducteur et `UNKNOWN` sur le même flux terminé sans attestation. Aucune attache Docker ni génération dans ces essais. Filtre `292aa05af4ddef3550ae52d694d80b3235ff26f1fb605f80a8b97a66cc516c4f` dans `target/qa-evidence/qwen-llama-progress.QF1esM0A`.

Un nouveau projet exécute un seul POST vers Qwen avec le conducteur de la candidate et le filtre branché. Le contrôle de santé passe, le gateway atteint 12 s et le relais passe 14,004 s sans en-tête. L’essai nominal est **FAIL**. Le refus durable est rejoué dans le vrai CLI, puis confirmé dans Chromium et dans l’export. Le contrôle `fail_closed_proof_verified` passe aussi avec le backend réel indisponible. Le pic mémoire du modèle est de 324 632 576 octets. Le projet est arrêté et aucun conteneur de recette ne reste actif. Le filtre conserve une projection fermée et rend `UNKNOWN` avec `owned_close_unconfirmed`. Neuf lignes ont été lues, sans repère de démarrage ni phase reconnue. La réception et la génération côté modèle restent indéterminées. Rapport `e2ee6addf827758639d9a75f5cbfbe35760006771b621f6e5194563216e3b660`, conducteur `127acf03a2886c9a232ac4d267346d6669fbc72c3ec7311ed693ce24959f2c3a`, rapport navigateur `11e40e1b9aa5399fd214e1d667eb56b968d1f125a8aa09e1e57cf049910e71b4` et projection fermée `6b6b78d92636350de6bf9093b0f78e40634ff03febb9c2c3e503032f71791fa8` sous `target/qa-evidence/qwen-one-post-model-observation.rjwYsEJS`.

Sans modèle, une fixture Docker `logging:none` sans TTY transmet les deux flux dans le vrai client `docker attach`. Le premier prévol échoue sur le rendu du template combiné. Deux lectures scalaires et un format JSON pour le conteneur possédé corrigent ce prévol. La paire exacte arrive pendant que le client est vivant en 854 ms. Le client reste actif après SIGTERM et le conducteur attend puis force l’arrêt du seul client. Le CID, `StartedAt` et l’état du conteneur concordent encore après cet arrêt. Le CID de recette est ensuite supprimé. Le transport est attesté pour cette fixture et le verdict de fermeture reste `UNKNOWN`. Aucun POST modèle et aucun flux brut conservé. Rapport `bc4990a8ade036a13fb2f6289e4a0a74c706969f5b63f827e93440ddeb9eb946` et conducteur `0d247b78025fa98fbc45a13e2986699092ba63990e22798bfb09e99a0c0fd9d2` sous `target/qa-evidence/docker-attach-transport-fix.5Y7EsF8j`.

Une seconde fixture, sans modèle, s’attache via le socket Unix Docker au seul conteneur QA. L’API Engine v1.47 répond HTTP 101. La paire déterministe est observée dans deux trames multiplexées en 579 ms. Le conducteur atteste la fermeture du descripteur, relit le même CID, `StartedAt` et l’état actif, puis supprime le conteneur. La preuve de transport et de fermeture locale est **PASS**. La collecte ne démontre ni la réception de la demande par llama.cpp ni un désabonnement côté moteur. Onze cas hors ligne couvrent notamment trames fragmentées ou tronquées, limites et fermeture vue par le serveur de test. Rapport `9b48c0887d4bd0d769525477f206cf4cad08c6d97447e535824ab3d26709e456`, conducteur `cd7520ae353481c4017adb3def1b8194ba6136d9a67e4df1b664ba77f4512a28` sous `target/qa-evidence/docker-unix-mux-attach.fLilOQOP`.

Le conducteur Qwen passe ensuite le GET de santé et l’attache Unix HTTP 101 dans le mode `--startup-only`. Le descripteur est fermé et le projet est arrêté avec zéro conteneur actif. Le filtre reçoit quatorze lignes complètes et aucun repère de démarrage reconnu. Le prévol rend **BLOCKED** et comptabilise zéro POST. Cette exécution ne renseigne pas l’admission d’une demande. Rapport `6668b8c22685c3e300465c9543c1fd928a16264c58278db70efbcf71c43c8d38` dans `target/qa-evidence/qwen-unix-mux-conductor.a9aPL6C8/attempt-63a06662d9c141f6b2a728f78b6b34c1`.

Une fixture Docker sans modèle établit que l’attache Unix peut précéder le démarrage. Le conteneur QA reste `created` après HTTP 101. Un unique `start` livre la première paire stdout et stderr, séquence 1, en 505 ms. La fermeture locale laisse le même conteneur actif et son retrait est vérifié. Rapport `0ccef34475afdb7d1a0fb99e6092df37e3b251019eebe5f1e5840de6121d696f` sous `target/qa-evidence/docker-before-start-mux-attach.WbNoqUD9`.

Le conducteur Qwen applique ensuite cette séquence. Il vérifie l’image et le profil du modèle précréé, s’attache avant le démarrage, puis vérifie que l’ingress conserve le CID et `StartedAt`. La santé passe et le descripteur est refermé proprement. Malgré cette attache précoce, quatorze lignes sont reçues sans repère reconnu. Le garde-fou rend **BLOCKED** avant Chromium et zéro POST est émis. Le projet est arrêté et ne laisse aucun conteneur actif. Rapport `b175c0906b36569aa9f67fe8b79749f516651f97e282bad704d600f6d9bd0e9e` dans `target/qa-evidence/qwen-created-before-start.WGCJXNtH/attempt-b11c1bc60fd24ef28942ac5e40f05cb2`.

Un prévol ultérieur avec classificateur étendu crée et attache le modèle, puis s’arrête à `compose_ingress` en 1,724 s avec code 1. La classification de stderr identifie l’épuisement des sous-réseaux Docker au moment de créer le réseau de publication. La santé n’est pas atteinte, le descripteur se ferme et zéro POST est émis. Les neuf lignes reçues avant l’arrêt ne donnent aucun repère de version ni de démarrage. Zéro conteneur actif reste après `compose stop`. La libération des réseaux et la cause de leur occupation ne sont pas établies par cet arrêt. Reçu `4949d373bbe27fe1340a7e54e172b877d2e4b3292ca17c159b6e436112de7054` et stderr `0b6a39b63cc4032898e7c59ecc64902e41ab97a617021f378521c6806427c226` dans `target/qa-evidence/qwen-startup-log-shapes.Uj5xkWvz/attempt-580cc06f1af84576809e9e628f3e3765`.

Le manifeste privé des deux prévols relie trois conteneurs arrêtés et trois réseaux à leurs reçus, images et labels Compose. La relecture par identifiants confirme zéro montage persistant, zéro dépendance Docker extérieure et l’absence de journal persistant du conteneur. Les trois conteneurs puis les trois réseaux sont retirés par leurs identifiants et leur absence est vérifiée. Le retour du prévol Qwen à la santé reste à éprouver. Manifeste `215622138766f07f030e848064ba82f8442e2ce8829a3001c067f193e63d04e8` et reçu `1f8ebe23027cf7024c9c3bd138b7dfa5e4cb5705692b71a2d99bd34f81fba5c8` sous `target/qa-evidence/qwen-exact-cleanup-scope.bj0993l5`.

Un nouveau prévol atteint la santé sur le même CID, avec attache avant démarrage et fermeture vérifiée du descripteur. Sur quatorze lignes, le classificateur compte un suffixe de démarrage sous une autre forme et aucun repère exact de démarrage ou de build. Le verdict reste **BLOCKED** avec zéro POST. Le retrait vérifié des deux conteneurs et des deux réseaux du nouveau projet passe. Reçu `1a0ffa902faf5c3801df37ad0eef901390becb4452f91061bb49f655cb8d670b` sous `target/qa-evidence/qwen-startup-owned-cleanup.7O7RdRlC/attempt-3defec1e62d44c1cb2302a40ad7b1318`.

Un classificateur qui suit le code source b11718 distingue le composant `llama_server` et les champs d’horodatage du logger. Les 28 tests hors ligne vérifient ensemble le transport, la classification et le nettoyage. Le prévol physique identifie exactement un repère de ce composant avec préfixe INFO et horodatage dans les quatorze lignes stderr. Il atteint la santé sur le même CID, puis retire proprement deux conteneurs et deux réseaux. Le verrou conserve son verdict **BLOCKED** et zéro POST. La qualification de l’image et le nouvel oracle doivent s’appuyer sur ces preuves sans accepter un suffixe générique. Reçu `461ef021c869d3ab67a3a0b708406128b27b998d064dda3490a631e0c5c1e867` sous `target/qa-evidence/qwen-startup-b11718-source-form.1tt50dc_/attempt-b5b623417dee43dd9bccfe6fbf6edf8c`. [Émetteur b11718](https://github.com/ggml-org/llama.cpp/blob/b11718/tools/server/server.cpp#L450-L459) · [Format du logger](https://github.com/ggml-org/llama.cpp/blob/b11718/common/log.cpp#L115-L143)

Le conducteur corrigé associe cette forme exacte à l’image locale épinglée et impose un nouveau repère stderr sur la tentative courante. Les 31 tests ciblés passent hors ligne. En Docker, le repère arrive 21,585 s après le début de l’observation, la santé passe et le prévol rend **PASS**, avec zéro POST. Le pic mesuré à la frontière avant requête est de 290 820 096 octets. Le descripteur est fermé et les deux conteneurs et réseaux sont retirés. La version du binaire reste `UNKNOWN` et le parcours d’inférence nominal reste ouvert. Reçu `eb6bfaaabf305086ec2844c965e45bbc1843860ee997348b3b816804b0500db6` sous `target/qa-evidence/qwen-b11718-exact-startup.8KFI8sEf/attempt-af8dc3aa677846e694e47dd159605b75`. Rejouer le script voisin `run-local-qwen.py --startup-only <bundle-absolu>` avec les empreintes du paquet.

Le conducteur nominal assemblé passe 40 tests avec des sockets Unix privés et le réseau IP refusé. La tentative Docker émet un POST. L’attache du même CID observe le repère exact avant dispatch, puis une admission à 24,261 s, 96 jetons traités à 82 % du prompt à 27,422 s et une libération de slot à 38,527 s. Les temps d’attache et du relais ont leurs propres origines. Le relais a envoyé la requête amont et expire en attente des en-têtes à 14,005 s. Le gateway applique sa borne de 12 s. La collecte ne donne ni fin de prompt ni preuve de génération complète et la recette nominale rend **FAIL**.

Le refus durable est relu et Chromium passe sur son audit, les exports et la séparation des tenants. Le pic cgroup du modèle est de 324 579 328 octets à la mesure après requête, sous une limite effective de 999 997 440 octets, avec zéro OOM relevé. Après synchronisation des preuves privées, le conducteur confirme le retrait des deux conteneurs et de leurs deux réseaux. Reçus fonctionnel `4229063321f249caef2f64fb5c55bd2d6aec75a3724ed5507d241206ab1c78eb`, phases `ed9629f2632c02fa64b26210720c1beab47ea697c77b96d2f619599a8466c049` et nettoyage avant retrait `208b85af3134388cf5f9a8c00c5ad3b5a87b1e39f0c2796fb1ae7921c4de9b65`, sous `target/qa-evidence/qwen-nominal-merged._6_ny2eg/attempt-8e8f6e5f16c24c6d80cae2a96e442f9f`. La procédure de rejeu est dans le `README.md` du paquet.

Dans la candidate testée, `src/jev/local.rs` d’empreinte `c36fffa769959256981574961785c1a83e9412f9a6733aadcdd82428d07b0da8` demande 256 jetons de sortie et une réponse non diffusée. Le défaut Compose de 64 jetons s’applique aux requêtes qui omettent ce réglage. Le principal contient un fichier non suivi `src/jev/local.rs` d’empreinte `57164733014d3d4e477c62896a835bb8d272b160fa66361f67e38bb3f9d45e07` qui demande 64 jetons et change le prompt. Ces sources n’ont pas servi au binaire de cette tentative. Le compte de CPU utilisé et les durées en throttling restent inconnus. La mesure sur le même CID permet de tester le quota et la disponibilité CPU avant de conclure sur le budget de génération.

Le code à 64 jetons diffère aussi par la CLI, le transport, le schéma de réponse et la validation de l’usage. Aucun binaire attesté à cette empreinte n’a été retrouvé dans les preuves consultées. Le témoin CPU privé prépare la lecture de `cpu.max` et `cpu.stat` après santé et après retour du navigateur, sur le même CID, PID et démarrage. Il rend `UNKNOWN` si les relevés ou la route divergent. La fenêtre couvre la recette, y compris l’orchestration navigateur. Les six tests de pairage et le format Rust passent. Paquet `target/qa-evidence/qwen-cpu-window.ymuK1IUG`, témoin source `7dacccf3c96223ae615181e629f1972993fc2ffb15915b36102ab0b43698fa8c`.

La compilation directe du témoin passe sur Rust 1.99.1, avec les 44 artefacts exacts du cache isolés et vérifiés. Trois tests Rust passent et le binaire expose `--cpu-counters`. Le conducteur de rejeu utilise une copie privée du même CLI que la tentative à 256 jetons. Le fichier `Cargo.lock` reste la référence de la résolution d’origine. Reçu de compilation `7a8353795f5dc195d8133e7cc8ed249b75a1551df6fd9ef663708bbf7ae2d06f` et commande reproductible sous `target/qa-evidence/qwen-cpu-direct-run.wwpmtn7r`.

Le conducteur à mesure CPU préalable passe huit tests hors réseau. Une mesure initiale inutilisable bloque le dispatch. En Docker, les deux mesures physiques attestent le même CID, PID, démarrage et quota `200000/100000` en µs. Les deltas sont 24,265723 s CPU, 199 périodes comptées, 54 périodes bridées et 4,762921 s de bridage cumulé. Cette fenêtre encadre les 72,952 s de recette navigateur et inclut les relevés. Le relevé hôte juste avant le lancement compte cinq processus de compilation actifs ou en attente et un suspendu. Ces valeurs établissent du bridage dans la fenêtre, sans attribuer le délai du modèle à une seule cause.

L’unique POST atteint le slot et le modèle libère ce même slot. Aucune progression du prompt ni de la génération n’est qualifiée dans cette tentative. Le relais expire en attente des en-têtes à 14,011 s. Le refus durable et sa vérification Chromium passent. Le pic modèle atteint 327 704 576 octets, avec zéro OOM observé. La route et le nettoyage exact passent. Verdict nominal **FAIL**, reçu `823006dbd671835d709b3068d518b13de2e82cb17fef7220c5b6a59ee91b957b` et phases `440e28388a608f3bdb3fa3f7eaea3e04bb51ecaa0ac684718db034ade93c1f57` dans `target/qa-evidence/qwen-cpu-guarded-run.9vi8l4aa/attempt-212efc319a354607b8dc29ae8b606bbe`. Le relevé hôte `2d3dc77f31c8f4296bd3830d36431a8181678352b103aa69c3e4ebfd97e567be` et les commandes sont dans le paquet parent.

L’aide du binaire de l’image épinglée s’exécute sans réseau ni POST et ne laisse aucun conteneur. Elle contient exactement une déclaration de `--threads N` et une de `--threads-batch N`. Le premier lecteur attendait un seul espace autour de l’alias. Le lecteur corrigé confirme les deux options sur le même fichier d’aide, sans relancer l’image. Source de l’aide `9c15905aca31fd975b183f00f4dbc9c79b46b2b61df7247682d2cc2738feea8d` sous `target/qa-evidence/qwen-pinned-thread-flags.oj9p43x_`.

Le Compose HTTP dérivé ne change que les arguments `--threads 2 --threads-batch 2`. Son empreinte distincte est `8322137e725b613f80e222734a3e28e83776eaa4b0c19d19fdcb3d906782ebab`. Les treize tests purs passent. Le prévol Docker atteint la santé, confirme les arguments appliqués sur le même CID et relève le repère de démarrage à 14,634 s. Il n’envoie aucun POST et confirme le retrait des deux conteneurs et des deux réseaux. Le verdict d’inférence de cette configuration reste **NOT_RUN**. Reçu `f01833930d3ddb09c4334924ac0203edf789c5c76015e39f9d4906552cc1b461` et procédure sous `target/qa-evidence/qwen-fixed-two-threads.nPKN2hM6`.

### Podman rootful à trois workers sur deux tenants

Le sélecteur `real_cli_runs_three_independent_workers_across_two_tenants` passe en 119,34 s avec le vrai CLI et Podman 6.1.4 rootful sur le socket Unix local approuvé. Les trois intervalles observés par le moteur ont un chevauchement de 16,298292834 s. Au pic, les quotas actifs valent deux chez A et un chez B, sous les plafonds cumulés de 6 vCPU et 3 Gio pour les workers. Après la fin normale d’A1, A2 et B1 restent en activité et les quotas passent à un chez chaque tenant. Les trois lectures de détail hors tenant rendent HTTP 404.

Les trois volumes et exports sont distincts. Le test rapproche les identifiants observés du conteneur, de la tentative et de l’autorité avec le registre et le ledger. Les deux chaînes d’audit sont valides et les allocations sont non facturables. Le moteur ne retrouve aucun conteneur actif de l’image QA à la fin. Ce résultat qualifie le parcours CLI Podman rootful du poste. Rootless, les autres plateformes et Chromium sous Podman gardent leurs propres preuves.

Manifeste source `077282ee5949e0b34d7c7cf6d94b60352b6c8f4060d0bd6f7818002f4fb297ca`, binaire de test `58222d7f66c750cfe9b5d0252d445cb93910f2afe37c23fb6a87dacba49c557a`, CLI `642f82fcef331a0d9e76e0f4d2cf5669623ab950d5f1ea4081a46effc049e139`. Dossier `target/qa-evidence/podman-three-physical-cli.1Kcl3Gzu`, rapport `fd94553e859fda65f71701fabe6c39c60dacf20e888382459d2175dc2338e4c4`, assertions bornées `0212eba45276bf9818b7c30b0a7b2a693537ef10bd74bf76149c7e739e0fb87f` et journal `0f64d10f7a812c9f9335cff4eae53bafa2fa96a4155762e718fee3395e63e439`. La candidate conserve la commande dans `docs/engineering/LOCAL_CLOUD_TESTS.md`.

### Dashboard terminal des trois workers Podman

Chromium, le vrai gateway et le dashboard Rust vérifient les trois workers Podman rootful au terminal en **PASS**. Le rejeu copie et vérifie les deux bases SQLite et leurs WAL, puis recoupe les exports et reçus du parcours physique avec ces copies. Il crée des identités techniques de recette neuves. Les deux vues affichent le runtime `podman` du lancement et de l’inspection, ainsi que des allocations observées. Trois détails hors tenant sont refusés et six événements d’audit correspondent aux reçus. Le conducteur et les services s’arrêtent avec code 0, sans processus Chromium Headless Shell restant. Aucun worker n’est lancé durant le rejeu.

Depuis la candidate, le rejeu privé utilise `python3 -B target/qa-evidence/podman-terminal-browser-replay.ApJ1foX7/run-browser-replay.py`. Rapport `e67344974d7901d2b22f955935eed226d68463900bafe9fb42b2c59bc6f28b86` dans `run-t8rfnxii`, conducteur `34b44e5a77d23c15950a095672152f8cf4bb454bee06ef92acbf71fe29bc4542`, oracle navigateur `779501e24234d8804a062f9059ed5a3dd0e005d07541c7b16c948d98ca300b2e`. Huit essais de format vérifient la sélection explicite, le format historique Docker et les refus pour runtime absent sur Podman, mélangé ou inconnu. Rapport `f9ef6f8a7190c5cea6300b71c5048fa7a6515dcc05a5c9f686bf813d3cd11362` sous `target/qa-evidence/podman-browser-schema.eyu7108f`.

Les variables Chromium sont documentées pour le harnais normal de la candidate. Sa compilation Rust et le parcours complet avec création des workers dans le même essai restent à vérifier. Le recouvrement en direct et Podman rootless gardent leurs propres preuves.

### Prévol Podman rootless en lecture seule

La connexion Podman locale explicite distincte du défaut répond `rootless=true`. Le même digest d’image QA est absent du stockage rootless. Le test vérifie la connexion SSH loopback configurée. Il ne qualifie ni un socket Unix utilisable par le produit ni le lancement. Aucune connexion par défaut n’a changé et aucun worker n’a démarré. Rapport `5414150469881c27746940c173a10c363129dad574a89e1baf2187e533c26f7a` sous `target/qa-evidence/podman-rootless-connection-preflight.ecbz1wl6`.

Une lecture de l’image sur le socket rootful exact confirme l’ID de la recette, `linux/arm64` et `1000:1000`. L’export par ID produit une archive Docker de 100 285 440 octets. Le manifeste contient deux couches et aucune référence de tag. Aucun chargement rootless n’a eu lieu à ce stade. Archive `20decb3f3ecf7fd5ee771a9bbc683a51eeeb6b3000b832f53a022ffb59e53d7b` sous `target/qa-evidence/podman-rootless-image-export.rql0zzs7`.

Le validateur confirme les deux DiffID, l’empreinte du `Config`, zéro tag et un objet `repositories` vide. Les deux liens d’alias correspondent aux couches attendues sans extraction. Neuf cas synthétiques passent. Rapport `3d13ad5995cf4a8d18e0b4add693d29d269df943f4ec758fb34031ea08e80ec5` sous `target/qa-evidence/podman-image-archive-check.Rit0xVTK`. Le premier chargement par la connexion SSH Podman échoue à l’étape `load`. Le conducteur n’isole pas son code ou la cause. La lecture qui suit confirme que l’image reste absente et que le défaut rootful est conservé. Dossier `target/qa-evidence/podman-rootless-immutable-load.UT9B3y0n`.

La console de la VM libkrun active confirme la clé hôte ED25519 lue en loopback. Avec cette clé verrouillée pour l’essai, un transfert SSH crée un socket Unix privé `0600`. Podman interroge le socket exact et confirme `rootless=true`. Le vrai `agentsandbox doctor` confirme le runtime et le mode sans dérogation rootful. Le conducteur ferme le seul SSH qu’il a lancé et le défaut rootful reste intact. Aucun worker ni chargement dans cet essai. Reproducteur et reçu sous `target/qa-evidence/podman-rootless-unix-bridge.fbvIU5ut`.

Après le même prévol, le chargement explicite par le socket Unix rootless passe en 8 205 ms. L’image exacte est absente avant l’opération. La vérification après import confirme l’ID, `linux/arm64`, `1000:1000` et zéro tag. Le conducteur utilise le snapshot des octets dont il vérifie le SHA-256 et ferme son propre pont. Aucun worker dans cet essai. Rapport `8c0456617696b01fd8b80385ed19c45cb7bcf4ef39b535ecb20523230d3bcf57` sous `target/qa-evidence/podman-rootless-bridge-load.KR6vFrbV`. L’échec précédent reste enregistré pour le chargement par la connexion native.

La recette CLI rootless à trois workers sort avec code 0 en 125,609 s et ne laisse aucun conteneur actif de l’image QA. Cargo remplace le fichier du harnais avant sa seconde vérification d’empreinte. Le conducteur extérieur rend FAIL `candidate_artifact_changed`. Ce run n’est pas retenu comme preuve qualifiée. Rapport et log `e0ff1996008ee555b939344d992d5d474433c56da5d6c6bec0d8b2168a5116c4` sous `target/qa-evidence/podman-rootless-physical-cli.MaKUKomg`.

### Parcours CLI et dashboard terminal sur Podman rootless

La recette normale de la candidate lance trois workers Podman 6.1.4 rootless sur deux tenants. Le vrai CLI utilise le socket Unix privé, sans option rootful. Les trois workers se chevauchent pendant 18,311703703 s. Les quotas passent de 2/1 à 1/1 après la fin normale d’A1. Les trois exports correspondent à leurs sources, les trois détails hors tenant rendent 404 et les allocations observées restent non facturables. Après la fin des workers, Chromium vérifie les vues de chaque tenant et rapproche trois refus et six événements d’audit avec les reçus. Le rapport garde `runtime_kind=podman`. La recette passe en 162,716 s, sans conteneur actif de l’image QA ni pont SSH restant.

Le conducteur utilise une copie du harnais dont les octets sont vérifiés avant et après le parcours. Les empreintes du CLI et des ressources du navigateur concordent aussi. Rapport `b68723b7e800912221d729890854860be42d1c1f3af3cc13dc122c8dc1db2a66`, conducteur `01cc852547e3c8d938788b0e7fa8116d09f15c65bae40c30bdd7026083ddf666` et journal `5d5b0f478587af4d742779fc886be8ff827fe0925caad49dbd7ff6340a30e207`. La preuve du dashboard en activité et l’arrêt explicite d’A1 restent à établir.

Depuis la candidate, après le prévol du pont et la préparation de l’image rootless, copier le conducteur privé dans un dossier neuf au même niveau que les preuves qu’il vérifie.

```sh
qa_rootless=$(mktemp -d "$PWD/target/qa-evidence/podman-rootless-replay.XXXXXXXX")
cp target/qa-evidence/podman-rootless-terminal-browser.5lhjEst5/run-private-bridge.py "$qa_rootless/"
python3 -IB "$qa_rootless/run-private-bridge.py" --run-rootless-cli
```

### Prévol de la projection Podman en activité

Sur le même pont rootless attesté, un seul conteneur QA réseau `none` sert de témoin. Le template natif de la future recette live rend un objet fermé pour son CID dans la fenêtre de 3 s. Les champs d’identité, le démarrage, l’état actif et l’absence de redémarrage concordent. Les limites configurées sont 2 000 milliCPU avec encodages natif et quota cohérents, 1 Gio et 256 PID. Le conducteur arrête son CID exact et confirme zéro conteneur restant et la fermeture de son pont. Le quorum des trois CIDs après rendu du navigateur reste à établir. Conducteur et reçu dans `target/qa-evidence/podman-live-one-native-shape.I27lrbtb`.

Trois conteneurs synthétiques rootless permettent ensuite de vérifier la forme multi-CID. Une seule commande renvoie trois lignes dans l’ordre des CIDs demandés, 1 581 octets en 1 386,2 ms, sous les bornes de 4 Kio et 3 s. Les trois états actifs et les limites configurées concordent. Le conducteur vérifie la disparition des trois conteneurs et ferme le pont. Ces conteneurs éprouvent la projection moteur. La corrélation avec le DOM, les deux tenants et les quotas appartient à la recette intégrée. Rapport `a4f08b7a2ab1a90a05594b9fceb7527355ab8bd90b584cae6d9b96c968657fc8` dans `target/qa-evidence/podman-live-three-native-shape.hIZiSmWU`.

### Intégration des recettes STOP, Podman live et des diagnostics Rust

La première file d’arrêt d’A1 expire au stade `queue` sans lancer Cargo. Le conducteur physique indique `BLOCKED` avant tout effet moteur. Reçus `31bd01c2eb10b04a72567c51ce42c68653833e57cf86b14771e28b2c86400e5e` et `ea372194226b6781634b9d387ee2f251dbdb356798828773c692d09becb796e2` sous `target/qa-evidence/explicit-stop-a1-build.YB0UUWUg` et `target/qa-evidence/docker-explicit-stop-a1.44t1TcJL`.

Après la fin de cette file, le candidat reçoit le conducteur Podman live avec ses dépendances Rust et Node, le canari d’ACK de décision G07 et la trace de nettoyage du helper G09. Les huit empreintes de sortie et la syntaxe du conducteur Node concordent. Le manifeste combiné vérifie 1 127 fichiers. Le script `queue-build.py` effectue un seul build hors ligne des deux libtests puis lance six sélecteurs ciblés. Au relevé, le build attend la libération de capacité de compilation. Les rejeux physiques STOP et Podman live restent **NOT_RUN**. Reçu d’intégration `251f5984f7870903399eea93d681cbf840b6bc69a20eb7242e71c030d615a549` sous `target/qa-evidence/podman-g07-g09-assembly.ikatpvj_`.

Le nouveau conducteur d’arrêt d’A1 attend le reçu de ce build, vérifie le libtest, le CLI, le manifeste et le canari de protocole avant toute action moteur. Il démarre une seule recette Docker sur l’image QA épinglée si aucun test de cette image ni Qwen n’est actif. Conducteur `dc89e7684d8295a8fd739d64ea84a483cf90e015bbb8bf63a1a602fd6b9aec50` sous `target/qa-evidence/docker-explicit-stop-assembled.rk7fb882`.

La revue de ce conducteur relève un intervalle entre la vérification du CLI partagé et son lancement, ainsi qu’un cas de persistance du PID hors de la garde de fermeture. Son processus d’attente est arrêté avant Cargo et sans effet moteur. La file du build reste active. Reçu de remplacement `682b1424b2196e027b0c295fe629a52e85465c66e220a97a052dfbc6d2f8e6c6` dans le même dossier.

Le conducteur de remplacement attend un reçu JSON lisible, exige les quatre canaris du harnais et rapproche le catalogue des cibles Cargo. Il revalide les empreintes après l’attente et à la sortie, confirme le mode Docker avant effet et récolte uniquement son groupe de test en cas d’échec de persistance du PID. Les 17 tests purs passent. Le CLI reste au chemin Cargo partagé et les vérifications attestent les instants de lecture. Script `965a160edf79fa3f61519018d7b94ea3f5e2007fcee9079e8593bf343c7185f2` sous `target/qa-evidence/docker-explicit-stop-verified.UkGFiqzV`.

Le vérificateur privé pour Podman live valide les sources, les quatre assets du bundle et les conducteurs Chromium avec leurs imports. Il signale `combined_build_receipt_absent` avant tout effet tant que le build manque. La procédure décrit soit la réattestation d’un pont prêté, soit l’ouverture d’un pont Unix temporaire avec le pin d’hôte existant et le retrait de son seul processus. Le rejeu requiert l’image rootless déjà disponible. Procédure `821fc7b24662bb7c1235043032b75bd059968c996ac421dd7286bc70c58e2ab0` dans `target/qa-evidence/podman-live-replay-plan.xjFPk8yb`.

Une revue compare les 1 126 sources de l’ancien build Docker live aux 1 127 de la candidate. Les cinq écarts applicatifs constatés concernent `cfg(test)`. La preuve CLI historique concorde avec le binaire présent. Une liaison directe du harnais utiliserait 253 artefacts représentant 817 Mo et la bibliothèque d’application historique dont le reçu ne conserve pas le SHA. Ce raccourci permettrait un diagnostic attaché aux anciens artefacts et au nouveau harnais. La compilation Cargo ciblée `cargo test --locked --offline -j1 --no-run --test runtime_user_journey` reste la preuve reproductible pour la candidate et garde G07 et G09 séparés.

La file combinée termine **BLOCKED** avant de lancer Cargo, avec `existing_compilers_still_active`. Le conducteur STOP associé termine **BLOCKED** sans effet moteur. Reçus `4f65cbd3db8fd24fd23b7f4dce0a9fe9768b4cc1e2a759f91a1cdf2636a0f0d9` et `ba2a7acd250d6d0fef7e909b9ee9fd93833d8073ba03939f7b5f3f424f927b5f` dans les deux dossiers précédents.

Le nouveau paquet `target/qa-evidence/runtime-integration-only.10ITMcU8` vérifie 1 129 entrées avec les deux imports Chromium. Il ne cible que le harnais CLI et web et son vrai CLI. Le lancement est différé avant Cargo avec cinq compilateurs actifs ou en attente au relevé. Le conducteur indépendant `b410ba708a722e6933dc6f1ad7d10080955d9b155ae6f00ee01eba5cabda421b`, sous `target/qa-evidence/docker-explicit-stop-integration-only.3meIj8xB`, passe 19 tests hors moteur sur ce nouveau reçu. Les preuves physiques de STOP et de Podman live restent **NOT_RUN**. Les canaris G07/G09 conservent leur qualification propre.

La procédure Podman mise à jour valide le manifeste d’intégration et ses deux exécutables, les quatre canaris et leurs journaux. Les tests purs de substitution, journal divergent et artefact en trop passent. Le prévol réel des fichiers rend `integration_build_receipt_absent` avant tout effet. La même fenêtre physique est réservée jusqu’au contrôle terminal pour STOP ou Podman live. Procédure `d73ef9e11cb86668b4cd6690242e6ca25f7006025eba2a6e508c19ef653ba2f1` sous `target/qa-evidence/podman-live-integration-replay.g4V3c2YT`.

À 15 h 08 (Paris), Cargo démarre le build d’intégration seul à `-j1`. Le relevé précédent compte quatre compilateurs actifs ou en attente et indique 33 % de mémoire disponible. Le manifeste de 1 129 fichiers est écrit et le journal Cargo reçoit ses premiers événements. Le conducteur STOP d’intégration est lancé en attente du reçu final. Les canaris et les verdicts physiques restent à obtenir.

À 15 h 10 min 37 s (Paris), le contrôle local en lecture seule retrouve la VM Podman libkrun en marche, avec 4 vCPU et 3 814 Mio de RAM. Le profil rootless, le listener loopback associé à la console et le pin ED25519 existant concordent. Les prérequis locaux du futur pont sont vérifiés. L’ouverture du pont, l’authentification et la disponibilité de l’image restent **NOT_RUN**.

Le champ `active_qa_containers_after` compte les conteneurs en cours d’exécution. Le lecteur complémentaire vérifie après le reçu terminal STOP les conteneurs de l’image QA dans tous les états, le processus et le journal du test, le socket Docker et deux lectures d’inventaire concordantes. Il ne retire aucun objet. Six contrôles purs passent sur le lecteur d’origine, script `a2ea8aa614f4f641d27a816da4beef5672ee2dcd3ec18fdb0d3370ae2a2241eb`.

Le relevé antérieur à l’apparition du processus STOP observe quatre conteneurs arrêtés de l’image QA et zéro actif. Le contrôle de version 2 lie le SHA du reçu terminal au SHA du relevé initial et exige zéro actif, zéro nouvel objet et l’absence du processus STOP. Il accepte le même ensemble exact ou un inventaire vide. Les quatre objets antérieurs sont conservés. Cinq cas purs passent. Le lecteur d’attente initial a été arrêté sans toucher Cargo, STOP ni le moteur. Le lecteur de version 2 attend le reçu réel. Relevé `72529f8fd1081480646185e70879d3b0b21c7222d8911a86eb059f6455624f3b` et script `abd1058ae2203498b0c5cd50a6ff363acfdeec6d68184482262c4e918007e41c` dans le dossier STOP d’intégration.

Le conducteur Podman automatisé et relu valide le reçu d’intégration, les binaires et le bundle, puis attend le reçu terminal Docker et l’inventaire v2. Un FAIL terminal recevable conserve son verdict. Le conducteur ne peut ouvrir qu’un pont rootless privé à partir du pin existant et doit récolter son seul processus. Les douze tests purs passent et le prévol en attente de STOP rend `stop_result_pending`. Le conducteur est lancé en attente des deux reçus. Aucun effet Podman n’est attesté à ce stade. Script `73dd0a06c7220d9894c6b10760719e608700403533a6ff5273cf3994ea298a45` et procédure sous `target/qa-evidence/podman-live-automated.YGEvLOr8`.

Le build du seul harnais d’intégration atteint sa borne de 2 400 s avec le code 124 sous `cargo_no_run`. L’inventaire Cargo ne contient aucun nouvel exécutable de l’application. Docker STOP termine `BLOCKED` avant effet et l’inventaire v2 retrouve les quatre objets préexistants, zéro actif et zéro introduit. Podman live termine `BLOCKED`, `stop_not_started`, avant ouverture du pont. Son prévol sur ces mêmes reçus reproduit le refus. Reçus `cce2f0cbda36a61b9914612df1a9c26a2d275395a232ec10b6de4faad30363a0`, `ba2a7acd250d6d0fef7e909b9ee9fd93833d8073ba03939f7b5f3f424f927b5f`, `7e428f3d9c553bae731ede044c7b0e4cc0db6ac602912422f2b6bc06335c6f9a` et `bdc8974eb96fd586e5f4f75724becce5adc2f576d479a550e1980cf7f71b25a7` dans les dossiers d’intégration, STOP et Podman correspondants.

Un prévol hors réseau atteste le CLI historique et vérifie sept cas synthétiques du contrôle des sorties STOP. Il couvre le reçu d’arrêt A1, le chevauchement, les quotas, les deux fins naturelles et les exports attendus. Le sélecteur STOP manque dans les libtests disponibles et la bibliothèque Rust nécessaire à la liaison directe du nouveau harnais n’est plus présente. Le drainage des seuls conteneurs de recette après interruption d’un conducteur basé sur le CLI historique reste à démontrer. Reçu `765d77525e8888b20388b8e43fff3994b859a7989225084ee2ab7e6958b3dd9a` et conducteur `5d393d8e77f27e9638d447a0943c2bb63d92865c0402e59d7a61fefa3d4ff5b4` sous `target/qa-evidence/stop-historical-preflight.360pPNhi`.

### Reconstitution PNG d’un échec navigateur live

Le prototype QA rend après la fermeture du harnais live un HTML expurgé avec le CSS identifié par empreinte. Il refuse les pages de connexion. Cinq tests Node purs passent. Le premier essai réel est refusé avant Chromium pour des chemins relatifs. Après correction des chemins, le lancement Chromium est observé, puis le watchdog de 20 s interrompt le rendu avec code 124. Le reçu ne situe pas l’étape du rendu et aucun PNG n’est obtenu. Reçu `746535a83bd8331d327777cb2ba1a7af6a526bead8d3ae6608b7646e71e420c1` sous `target/qa-evidence/png-private.bpmDlo`.

Le rejeu suivant ajoute des jalons et des délais par appel. Le contexte hors ligne, les gardes réseau, la page, le chargement de l’HTML, les validations et la préparation du DOM passent. Le délai de 2 s expire pendant l’injection CSS. La fermeture de Chromium est confirmée et aucun processus headless ne reste. La capture et son compteur réseau restent **NOT_RUN**. Verdict `ef040cf5ceea9809ab2de5fcecfed0017222d4a5f63ee8eca1c9e93d9d807eb3` et journal `f72d153245ed681003ace25d54f49e050ba4fa852ece3be0fb67a852c85d4cee` sous `target/qa-evidence/png-phases-private.Pq3HUj`.

Le rendu par insertion du CSS comme texte remplace l’attente de son événement de chargement. Chromium confirme via le CSSOM que la règle du bundle positionne réellement l’en-tête. Le canari synthétique passe en 6,1 s, sans requête et avec fermeture attestée. Le PNG privé valide le format 1440 × 1050 et montre la sentinelle de la fixture. Le dock fixe masque le bandeau d’erreur synthétique. Verdict `959d95f8def05bf15c000344bdb1590deb22d717fd42fe272785638ed6e708a9` sous `target/qa-evidence/png-css-text-private.ckSiHu`. PNG `8a7104c704bbf78c6c71c4936e56350529add7517584d37215453578425e5f78` sous `target/qa-evidence/png-private.bpmDlo/browser-failure-SynthCanary`.

Le même conducteur reconstruit le snapshot expurgé de l’échec Docker `evaluation_changed`. Le SHA du HTML concorde avec la preuve historique et le CSS est rattaché à son manifeste de bundle. Le rendu 1440 × 1519 passe sans requête et avec fermeture attestée. Il montre deux workers et la consigne de migration du registre des brokers. Le conducteur reconstitue l’état statique. La position du défilement live et la cause de l’échec ne sont pas déterminées par ce PNG. Verdict `7d3c0be0bc7895d61579608bf2c729c47cdb9f2274ce95f0b849c0519d3a58f2`, journal `a8555ba5dbbf8dfbd7aca2c962a10eadce96d57726474f77d61b61ee282c2fcc` et PNG `5e34d497cf5a72bbe6cb8de44cc5e022be85446cedab30bc74adb89712b5f926` sous `target/qa-evidence/png-real-docker-private.NeUevV`. Le paquet garde le HTML et le PNG en mode `0600`.

Le post-traitement automatisé conserve le code 101 du vrai échec Docker et rend `SKIPPED/live_cleanup_unconfirmed` avec son reçu historique, qui ne prouve pas la fermeture du navigateur live. Le renderer ne se lance pas dans ce cas. Verdict `138ae56d6f7e440340e958d3fcf836b03f12b36f3b906eabda9c619194d09c85` sous `target/qa-evidence/png-postprocess-private.OL0dBD`. La variante à watchdog portable garde une borne de 19 s et une seconde de grâce. Les cinq tests Node passent. Le binaire local accepte les deux syntaxes vérifiées et rend 124 à l’expiration. Reçu `429e59fb5aa1de6a5f13414c88d1de3371a7e5bc37bfaa290b98721eb825207e` dans `target/qa-evidence/live-error-png-timeout-portable.kzJ5Bks3`.

Avec une déclaration de fermeture propre sur une fixture synthétique distincte, le wrapper portable conserve le code live 47. La capture en mémoire et sa validation passent avec zéro requête. La fermeture du renderer expire ensuite. Le watchdog rend 124, le nettoyage reste indéterminé et aucun PNG n’est enregistré par ce conducteur. L’inspection après le watchdog ne trouve aucun processus headless restant. Verdict `0ee3268f8329770b97f533b4311f30008caae1783511f1dac88a36046705b0e7` et phases `8e71f9374b70f79a2119dcd1f906831c3c20dbb04fb58f431678cd0b0f112bf2` sous `target/qa-evidence/png-postprocess-positive-private.Rd0KbY`.

Le conducteur corrigé publie le PNG vérifié et son reçu lié avant la fermeture. Les douze tests Node ciblés passent. Sur une nouvelle fixture synthétique en Chromium, le fichier et le reçu sont écrits en `0600`, le PNG fait 1440 × 1050 et aucune requête n’est observée. Le watchdog expire pendant `browser_close`. L’adaptateur vérifie le nonce et les empreintes, conserve le code live 47, atteste le PNG avec le statut `PARTIAL` et garde la fermeture indéterminée. Aucun Chromium headless de recette ne reste au contrôle. Verdict `cd61ce5bcb5d7b0d500c6960488399033a5e19022b0b7b1d44cfe3a1cb24a38d` et reçu du PNG `886cc4decf320636d55d85e72a4ea0870414e93c579f7f074a7c0f8f815091b1` sous `target/qa-evidence/png-before-close-private.mrxvqs`. Le script et les consignes sont dans `target/qa-evidence/live-error-png-before-close.dqhWj4rj`.

### Déconnexion des deux lecteurs après inspection en direct

Le harnais en direct vérifie désormais les deux déconnexions dans le navigateur après l’inspection Docker. Il attend un formulaire vierge, l’absence des données de supervision et l’annulation de chaque flux SSE. Chromium émet un jalon avant sa fermeture, puis son verdict de clôture. Le harnais conserve la borne de 15 s pour ces étapes et garde FAIL si la clôture manque. Si l’inspection moteur échoue, seul le chemin de fermeture de secours est disponible. Application et syntaxe Rust/Node vérifiées. La compilation du harnais et le test de protocole `live_browser_logout_protocol_requires_separate_purge_and_cleanup_witnesses` passent. Manifeste `3e97eb333acda9084bde54b8c9c492d3c76e0d3f8df4c74489cd841d3d497649`, binaire `4167bec3e35ac30fb49016daca714f4538a65d1021967f39e6b4c7621217ad20`, sous `target/qa-evidence/runtime-browser-logout-build.yij7pvzL`.

Le parcours en direct passe en **130,71 s** avec Docker 29.9.3 et Chromium. Le registre rendu montre 2/1 workers sur les deux vues. Docker inspecte les trois CIDs exacts après le rendu et le harnais recoupe les observations finales. Le chevauchement moteur est de 20,090342495 s. La déconnexion des deux lecteurs passe et le rapport indique `logged_out: 2`. Le rapport de fermeture aboutit dans la fenêtre de 15 s. Les trois exports concordent et zéro conteneur actif de l’image QA reste après le test. Rapport `c2b46af8a934ff45f46e3d31c7e325a773a7861fb19db8ee8034f05d54ce3904`, conducteur `c6bdeb71f726b07d7d5352e6e51957af2885aae19ea123e59981b592529dd335` et journal `1d645476e4ba47bc65c62338635c55d06654a2c0861a329d8d5bcd3db5a6fac6` sous `target/qa-evidence/docker-live-viewer-logout.TvcGaArH`. Le sélecteur `real_cli_runs_three_independent_workers_across_two_tenants` avec `AGENTSANDBOX_TEST_BROWSER_LIVE=1` sélectionne ce parcours. Les observations antérieures de fermeture abrupte restent enregistrées séparément.

### Rejeu indépendant du retrait des données après déconnexion

Le scénario `isolation` avec le gateway Rust archivé passe le prévol sur `5b3c2ed044d5`. L’unique rejeu de cette recette se termine en `FAIL` avant `READY`, avec la base synthétique A créée et B/C absentes. L’assertion Chromium et les captures HTML/PNG restent `NOT_RUN`. Le profil utilise Node 24.22.3 et Chromium Headless 148, avec réseau en boucle locale. Le reçu situe la frontière de démarrage et laisse la cause exacte ouverte. Le nettoyage des données temporaires et l’absence de processus de la recette ont été vérifiés. Rapport `80aca1c15895c627e4439597efecba853ea35506e68ccae1228267e8f916febe`, commande de rejeu et delta de diagnostic dans `target/qa-evidence/g08i.G7t6fErm`.

Une régression isolée reproduit une exception du moniteur lorsqu’une ligne du relevé des processus est incomplète. Le moniteur corrigé passe trois tests ciblés. Avec les mêmes dépendances, artefacts et délais, un nouveau rejeu atteint l’ouverture des bases A, B et C puis expire à 45,341 s, code 124. Le relevé du moniteur est complet et zéro processus de la recette reste après nettoyage. `READY` et le lancement Chromium restent sans témoin. Les assertions de retrait et les captures restent `NOT_RUN`. Reçu `2ded56ce6283f742df66eded484bf949c938beed5a5e0dcd3431d7727c378c1a` sous `target/qa-evidence/g08r.3yEvS7ZU`, avec le harnais et le delta de diagnostic préparé pour les deux jalons.

Le conducteur privé avec jalons confirme `READY` à 31,365 s et le lancement Chromium à 32,851 s. Le navigateur vérifie la disparition des lignes Workers et Brokers au passage d’administration à personnel, puis le filtre réinitialisé et les 24 lignes au retour en administration. Le parcours complet échoue à `isolation_hold_a_reply` et le harnais termine le groupe de recette à 47,187 s. Le succès du GET de A, son annulation à la déconnexion et l’affichage exclusif de B restent sans preuve sur cet essai. Le dossier de capture est vide et le contrôle final retrouve zéro processus de la recette. Reçu `4e97178c9b198a78e32ffeeccffd605f4921b50dad68a7fcb394f927d33c90f0` sous `target/qa-evidence/g08m.vjw4t9pn`.

Le rejeu suivant distingue le démarrage, l’interaction et la finalisation. Les budgets respectifs sont 45 s, 25 s depuis le lancement de Chromium et 8 s à partir du verdict. Les délais internes et les assertions du scénario restent identiques. Chromium démarre à 32,075 s. Le GET exact de A est retenu à 43,683 s, son HTTP 200 est reçu à 46,423 s et son contenu vérifié à 46,433 s. La déconnexion annule cette même requête à 46,567 s. La session B se connecte ensuite et affiche uniquement ses données. Toutes les assertions interactives passent à 50,280 s.

Le passage reste globalement `FAIL` avec `finalization_unconfirmed`. L’ordre de fermeture Chromium et arrêt de la fixture Rust reste à mesurer séparément. Le repli termine le groupe de recette et zéro processus possédé reste à 59,011 s, contrôle final inclus. La capture authentifiée n’a pas été déclenchée, les assertions interactives ayant abouti après déconnexion. Reçu `55ba95c514c0ee884edb2010bb1b87e7bf116254e4eb1eb21c5be5853f1dd9ae`, conducteur et six tests ciblés du moniteur sous `target/qa-evidence/g08p.0xN8Yy5R`.

Le passage instrumenté suivant confirme toutes les assertions interactives Chromium à 62,894 s. Le GET exact de A reçoit un HTTP 200 et son contenu est vérifié avant l’annulation à la déconnexion. B affiche uniquement ses données. Le signal `STOP` est envoyé à 62,896 s et la fixture Rust sort normalement avec le code 0 à 62,917 s. La fin de fermeture de Chromium reste sans confirmation dans les 8 s de finalisation. Le verdict global reste `FAIL browser_close_unconfirmed`. Le repli termine les processus de recette et le contrôle final en retrouve zéro à 71,607 s. Le scénario ne déclenche aucune capture. Reçu `31219af28fd100661153fc55321f1ecce7fee1140e76ddcce3155f5015d6b995` et empreintes vérifiées sous `target/qa-evidence/g08f-replay.WYaPkcWu`.

Le rejeu avec jalons séparés passe les 21 tests Node et 14 tests Python puis exécute le vrai navigateur sur la même fixture. L’oracle d’isolation passe à 56,596 s et la fixture Rust confirme son arrêt normal. Les flux Chromium se ferment et Playwright confirme sa déconnexion à 57,116 s. Le processus Chromium attesté est sorti sur signal, sans intervention forcée du conducteur. Le reçu ne capture pas l’identité du signal. Le verdict reste `FAIL browser_close_unconfirmed`, sans capture ni processus de recette restant au contrôle final. Reçu `2118ea1be2ef64e0ca85fea4827c6e73d208c42e00b4be8458e11cde496d777a`, conducteur et tests sous `target/qa-evidence/g08-close-stages-replay.G9e23H3x`.

Le passage multiprocessus avec attestation du groupe détaché passe 22 tests Node, 23 tests Python et le canari natif sans navigateur. Dans le vrai passage, la fixture est prête à 30,504 s et Chromium démarre à 31,487 s. La finalisation commence à 31,758 s, avant les assertions d’isolation. Rust quitte normalement à 31,782 s. Le conducteur termine `FAIL` à 40,356 s après repli. L’exit Chromium, la fin des flux, la déconnexion Playwright et l’absence de membres du groupe restent inconnus. Le compteur final à zéro couvre seulement les processus déjà observés. Le reçu ne localise pas l’erreur de préparation entre contexte et page, et aucune capture n’est produite. Reçu `5c74f81c6d224e6a49d087673b7950ccfc2eb11e68a8980e9815abadf9ab0e58` et entrées vérifiées sous `target/qa-evidence/g08-chromium-group-replay.af3gKG53`.

Le rejeu diagnostique vérifie le gel des entrées et passe les 27 tests Node et 26 tests Python avant le navigateur réel. Chromium lance le contexte à 32,071 s. La première page échoue à l’issue de la fenêtre d’interaction. Chromium est déjà sorti au début de la fermeture et la déconnexion Playwright reste sans preuve. Les contrôles du groupe donnent zéro membre avant et après finalisation, et Rust s’arrête normalement, sans repli forcé. Le reçu ne produit aucune capture. Verdict `FAIL chromium_page_setup_failed` avec assertions d’isolation `NOT_RUN`. Reçu `addf0666b95d3cebfb0a5a606a6e35c5c0b8c2fb6d7701f8ab53406ae61b6205` sous `target/qa-evidence/g08-page-setup-replay.tinYELdP`.

Le canari sans gateway isole le défaut de première page avec le même headless shell. Le contexte est créé à 3,830 s, puis `newPage()` échoue à 3,906 s avec une erreur opaque classée `other`. La seule page demandée est `about:blank` et aucune origine réseau de page n’est admise. La fermeture naturelle et l’absence de tout membre du groupe restent sans preuve. Le conducteur applique son repli et termine `FAIL` à 12,847 s. Reçu `8ca18e7192d28491882615276ce9a0c7d995ac199cbda1e2495528924ee6f304` sous `target/qa-evidence/g08-first-page-replay.wy7q6KB0`.

Le même canari utilise ensuite le headless shell 151.0.7922.34 ARM64 épinglé, avec les gardes, le conducteur et les délais inchangés hors sélection du binaire. Le contexte est créé à 2,668 s. Le transport se ferme pendant la première page à 3,401 s. Le groupe est attesté vide, sans repli. Quatre rapports natifs macOS correspondent à l’UUID Mach-O du binaire dans la fenêtre du passage, trois `SIGTRAP` et un `SIGSEGV`. Leurs symboles partiels ne permettent pas d’attribuer la cause. Reçu `3d7a7cad5770bdc9617ebf6db81da7460c8d59a0a254b3b6e51faf2d8825a2db` et synthèse expurgée `31711e92885e0a314820165bf87b8bf675fb1a12f59e69a440980bb729ee1e96` sous `target/qa-evidence/g08-first-page-1234-replay.kPQBl12L`.

Le contrôle d’un autre binaire navigateur reste `BLOCKED` avant lancement. L’application Chrome installée satisfait l’exigence d’identité de signature Google. La validation stricte de ses attributs annexes échoue. La vérification du clone privé s’arrête sur un écart de métadonnée lors de la copie APFS. Le reçu confirme l’original intact et aucune exécution de ce binaire. Reçu `bbe7e15dbfd2bc8218720d8b18ce4d19400ed74df61dc8ebdb31e083d82b3fcb` sous `target/qa-evidence/g08-chrome-app.EI2AqrT6`.

L’archive officielle Chrome for Testing 158.0.8127.36 ARM64 correspond au condensat annoncé par Google. Son binaire ne porte pas la signature de l’équipe Google attendue dans cette recette et le sceau des ressources est absent. Le prévol s’arrête avant exécution. Reçu `0e57c535f9331beb7d5415251857cc0aa188d0160f7f0d289d2f86c77b5f6248` sous `target/qa-evidence/g08-chrome-for-testing-158.GBissuc3`. [Catalogue officiel Chrome for Testing](https://googlechromelabs.github.io/chrome-for-testing/)

Le Chrome installé 157.0.8103.80 passe la vérification de son identité Google et de son bundle avant et après le canari. Le profil interdit les écritures dans le bundle et garde les limites réseau et Playwright du rejeu précédent. Le processus apparaît puis Playwright renvoie un échec de lancement en 3,806 s. Aucune page ni assertion d’interface n’est atteinte. Le conducteur ne trouve plus de descendant observé et l’attestation du groupe navigateur reste inconnue. Reçu `b8c66ba02634495988f4920ba5dd6bb3d97cadf8a54eca6d66946e1a6dd8faf1` et script sous `target/qa-evidence/g08-installed-chrome-readonly.ji66y75f`.

### Prévol Codex avec le vrai compagnon de diagnostic

Le test `InstalledDiagnosticTests` exécute ses quatre cases Codex, API et abonnement en batch et TTY, avec deux états de référence synthétique par case. Le vrai compagnon vérifie le profil sélectionné et l’état de la référence dans huit sous-cas. Aucun appel fournisseur, navigateur ni worker. Le prévol passe hors réseau. Le batch Codex/API supervisé reste `NOT_RUN`. Rapport `36ba7434ee1dbc5e6181323cb0f970020c8b5a9a0f5a5d9f77cd627f02d81145` sous `target/qa-evidence/codex-matrix-passive-preflight.PN66o587`.

Sur l’image worker locale `sha256:925aace4da32c536e3846eb1cf32b94940a999d442bcbe970704009c787c3ffe` en ARM64, la version effectivement exécutée est Codex CLI 0.170.0. Le bootstrap fournisseur est exécutable. L’aide batch annonce les cinq options requises par la candidate. Deux sondes sans réseau ni fichier de credential passent et ne laissent aucun conteneur actif. Rapports `8521bb546d02eeb84348d2cc5e14f59760ec2132b6c3d4e9c4c6527e901ba453` et `1b9dbf98c7054d88c500c02a0d22fd1eab717098a8d552fd67c048a40566e11b` sous `target/qa-evidence/codex-worker-image-capability.ux86z60b` et `target/qa-evidence/codex-worker-bootstrap-capability.v9jvyyki`.

Le compagnon de diagnostic confirme aussi le profil explicite `codex-api`, le modèle demandé `gpt-6-luna` et la référence privée dédiée au seul Codex dans une configuration jetable. Le harnais utilise la validation habituelle de `run_provider_case.py` avec le réseau bloqué. Le rapport ne contient aucune valeur de credential et le diagnostic ne lit pas le contenu du fichier. Aucun appel fournisseur. Rapport `017559310a01af9822d870767ac5850c167bcc90bf247d4f3b0a7964c27df066` sous `target/qa-evidence/codex-api-private-profile.YWBTh8DR`.

Une pile fraîche de recette passe la sélection du contrôle schéma 3 actif, l’IPC `broker list` sur le profil Codex/API et le prévol de Chromium sur le dashboard lié au même gateway. Elle utilise les binaires attestés et le bundle de la recette moteur. Le broker s’arrête avec code 0. Aucun worker ni appel fournisseur dans ce prévol et le fichier de credential du fournisseur est vide. Rapport `3e63b38d3f7f9e78587ef8d135bf9ec8a17734b72554a49906fd9a42be1a7903` sous `target/qa-evidence/codex-broker-control-preflight.gcrZAAhC`.

La soumission unique Codex/API supervisée rend **FAIL** à la relecture du résultat. Le journal et la copie SQLite vérifiée concordent sur la décision `Allow` en séquence 3, avec résolution `codex-api`, `batch` et modèle demandé `gpt-6-luna`. En révision 3 du journal, `FailureObserved(decision, unavailable)` suit l’ACK de décision durable en révision 2. Son événement d’échec est durable au Store en séquence 4 et reste en attente d’ACK dans le journal. Aucune préparation workspace durable, admission, mutation worker ou allocation. Le retour Writer de l’ACK et le contrôle de génération qui précède la préparation restent deux frontières à distinguer. Le démarrage physique, l’appel au fournisseur et le modèle servi sont indéterminés. La copie temporaire du credential a été supprimée, zéro conteneur de cette image était actif à la fin et le processus broker de recette a nécessité un arrêt forcé. Rapport du conducteur `daeab92025aaabb33858523cd4bc76af50a68e38da9a7de4e6b589417bc3dd25`, résultat de la recette `3f95a71f8361a8260a0b5f8519a6b6f1ac4a2b3d157521b7c368ae54e3b8689a` et projection à froid `2eabfb2010b0ca882317b0ed58b37d15a8868f0f73afc45e0214b802e0fca215`. Dossiers `target/qa-evidence/codex-api-broker-one-case.uz59GNxh` et `target/qa-evidence/g07-codex-cold-projection.At7w2jih`.


### Prévol viewer et décisions Rust dans Chromium Linux ARM64

Le gel de 709 fichiers Rust est `ee9b4512c7b9d1c7a11bbc30259d3f07c240e4726caed650a199c8976c51e4e4`. Cargo a compilé le CLI ELF ARM64 hors réseau dans Docker Desktop. Reçu de build `05a4b325a8e0b2dafcaa538f3cd0eaa0f9c2b3f455ce6d2867ba1011e95d3630`. Les 34 empreintes d’entrée du frontend et les quatre sorties du bundle concordent.

Le canari Playwright 1.64.1 ouvre une page Chromium 156.0.8078.80 avec son sandbox actif. Reçu `9215d0948587f14848e073ae711aa7235ebe3c6417963153280dc6cebc3810dd`. Le navigateur tourne dans un conteneur dédié en boucle locale. Le pilote conserve le profil [Docker de Playwright](https://playwright.dev/docs/docker) et l’option sandbox dans la copie de la recette. Le pilote initial a échoué à charger les modules Node. Avec leur chemin corrigé, il rapporte la phase de lancement et l’échec de l’initialisation du sandbox. Le pilote révisé conserve UID non-root et zéro capacité effective au lancement Node, avec le syscall nécessaire au sandbox interne permis par le profil. Les essais viewer passent après correction de la résolution des modules Python et déplacement des fichiers de contrôle SQLite du partage macOS vers le tmpfs local du conteneur.

Sur ce gel, `viewer` est **PASS** pour les services HTTP, Chromium et l’identité viewer comparée entre le gateway, le proxy et la Vue d’ensemble. Reçu `ee0456353340ca14f0bf0de0c31d6ae1ff8d15544cb37488b9e81e56f30f6d7a`. `decision-rust` est **PASS** pour la décision Rust acceptée/refusée, la chaîne d’audit, l’export JSONL et l’absence de la décision du tenant voisin. Reçu `3982ac779a48226f54627ef57294490de03c7c51e540a5c1f20c985d140749bc`. Scripts, versions et procédures de rejeu sous `target/qa-evidence/g08-linux-arm64.mUoDoLZb`. Les deux cas utilisent de vraies routes Rust, un navigateur multiprocessus et des données de recette. Le scénario terminal à trois workers et le retrait de droits sous ce profil Linux restent à rejouer. Ce gel précède l’intégration P2/P4/P5/P6b de la seconde passe Claude.


### Décisions simulées dans Chromium et délai du relais Qwen

Les scénarios Linux `decision-typesafe-fixture` et `decision-local-fixture` sont **PASS** avec le même CLI Rust et le bundle déjà attesté. Chromium vérifie les décisions effectives, l’audit, l’export JSONL et l’absence de la décision du tenant voisin. Les fixtures TypeSafe et du modèle local produisent des réponses synthétiques. Reçus `1036d2bd53727c1496a515df32b762fdb4094cf3310343a8a105544799857c29` et `daccf5b71e07bdbfdf743ca2c41ee246ee6b3c144bfe7832f95669cd08dca25f` sous `target/qa-evidence/g08-linux-arm64.mUoDoLZb`. Les cas de fournisseur réel gardent leurs preuves propres.

Le même binaire exerce le vrai relais TLS dans un conteneur Linux avec un serveur synthétique en boucle locale. Une réponse rapide réussit via le relais en 192 ms. Une réponse retardée de six secondes réussit en direct en 6 012 ms. Le même retard via le relais rend HTTP 504 et le code public d’erreur de transport en 5 040 ms. Ce test établit la borne du relais pour un transfert valide et lent. Il ne localise pas à lui seul la cause de l’échec Qwen historique. Reçu `c9b0324ad20d49284d69a1432aa978754da0d9f55ea6406c54f32d0debf74232` et scripts sous `target/qa-evidence/qwen-relay-deadline.l3omKq1z`. Aucun modèle réel n’a été appelé.


### Rejeux CLI P5 et état de santé SQLite

Le gel post-intégration Claude `13835cef1698b7a4739a6dad3567b0e6cab91c9ea9e907422f752b6121478d84` compile le binaire CLI et les tests pour Linux ARM64. Les huit sélecteurs du binaire principal sont **PASS**. Ils couvrent les cinq oracles corrigés et les trois voisins du fixture OIDC. `health_reports_an_observed_unsupported_audit_schema` est **PASS**. Dans cette recette SQLite, une version inconnue est détectée et le gateway répond ensuite `503` à `/healthz`. Reçu `da6bf779fd305d1da72217b7439b3804aa97f0e86b0f484891ccaa44136e2582` et conducteurs sous `target/qa-evidence/claude-p5-current-linux.zzxs4b8x`. Les premiers échecs des métadonnées dans le même reçu se produisent avant les assertions, lorsque le pilote monte en lecture seule le répertoire temporaire produit par Cargo.

Le gel distinct du worktree SQL `3eb057505421747e6c3f632128498fd3e4afc3f5cf395ee6b8dbea50a5a07f2d` compile avec `mysql-storage`. La régression HTTP exacte `viewer_integrity_store_work_stays_off_runtime_worker` est **PASS** avec un vrai Store SQLite de recette. Elle vérifie la page, son curseur, les dates ou la recherche invalides en `400`, l’identité inconnue en `401` et la lecture d’audit indisponible en `503` au handler. Reçu `7659a3831eb471e120210813689c65f2b1d5c98c11fa4f25f215ac5b3f01531f` sous `target/qa-evidence/journal-filter-full-inputs.a8kzl5tb`. Ce test qualifie la distinction des erreurs dans ce worktree. Le gateway MySQL reste à raccorder.


Le rejeu des métadonnées garde le même libtest et le même CLI. Le pilote donne au fixture engine un répertoire Cargo inscriptible, conserve l’exécutable exact sur un tmpfs local exécutable et inscrit le compte de recette dans le seul conteneur de test. Les exécutions intermédiaires permettent d’isoler les erreurs de préparation, de compte Linux et de provenance du système de fichiers. Le rejeu final est **12 PASS**, reçu `3cd14e35e62850cf48d1dbab3e1b2d7f194d192d449f61847f2c07596f4b15c3`. Le conducteur réutilisable est corrigé dans le même dossier de preuve. La vérification complète des artefacts de publication et GitHub Actions restent distinctes.

### Délai configurable du relais TLS

Le relais a un délai total configurable de 1 à 12 secondes, avec cinq secondes par défaut et les bornes de deux secondes pour connexion et lecture de la requête. Le CLI publie la valeur effective au démarrage. Les deux profils Compose TLS transmettent cinq secondes par défaut et la valeur explicite en override. Les quatre vérifications de rendu passent. Le gateway conserve son propre délai dans son empreinte de configuration.

Le nouveau CLI ARM64 du gel `e9cafab92ec1256e123a40ddacf307286f4cafb33daca29278a6c9716d7fb72f` compile. En TLS vérifié, avec le délai du relais réglé à huit secondes, la réponse rapide passe en 128 ms, la réponse synthétique de six secondes passe en 6 019 ms, et la réponse de neuf secondes reçoit HTTP 504 typé en 8 008 ms. Reçu `e5e8da5368f601217097cd0b7111b814f660b05b46114b382173650e4c84364b` et scripts sous `target/qa-evidence/qwen-relay-configured.iddctzyc`.

### Décision Qwen réelle sur Docker, du CLI à Chromium

Le même CLI Linux ARM64 traite une demande consultative avec Qwen2.5-0.5B-Instruct Q4_K_M physique. Deux conteneurs de recette sur un réseau interne Docker Desktop ARM64 font tourner le modèle et les services Rust avec Chromium. Le CLI vérifie la santé du relais en HTTPS sur une AC privée. Le relais accepte un délai de onze secondes et le gateway de douze secondes. Le CLI et le bundle Chromium conservent les empreintes de leurs gels respectifs.

Le parcours est **PASS**. Qwen retourne un avis de refus. Le gateway le conserve avec son origine et produit un refus durable, rejoué sans nouvel événement. La décision et son rejeu prennent 3 690 ms. Les compteurs physiques concordent avec les 117 tokens de prompt et 47 de sortie déclarés et restent stables après les autres refus et la lecture du dashboard. Le CLI et Chromium vérifient la même tête d’audit. Le navigateur contrôle les deux refus du tenant A, son export JSONL et l’exclusion de la décision du tenant B. Le cgroup du modèle culmine à 322 916 352 octets pour une limite effective de 999 997 440 octets, sans swap ni OOM.

Le premier essai a produit un refus durable après une erreur de connexion TLS du client Rust. Il utilisait le même certificat auto-signé comme feuille et ancre de confiance. Le nouveau conducteur valide la santé par le CLI avec une AC et une feuille distinctes. L’oracle navigateur tient compte de la configuration HTTPS et le contrôle mémoire compare la limite cgroup arrondie à celle demandée par Docker.

Scripts et reçus sous `target/qa-evidence/qwen-real-tls.LB4VBlDU`. Reçu Docker `95070fe700f6e3d188f47bc1cd843ce2a6a3e19204bfc01b177befe604e59fbc`, reçu de décision et CLI `5f75f09beba546160045cef14c578c649fdf6f8e5a38fc461f045795dc0e84c3`, reçu Chromium `0a3c35b6fdde80113a9ae670b9e72f74743316d6550d3d99e1bc577e6ecf4f35`. Le déploiement Compose complet, Podman, Kubernetes, le comportement de Qwen sur un corpus et la reprise à froid après arrêt du modèle restent à qualifier.

### Rejeu OIDC du scénario de reprise corrigé

Le gel Native17 de 1 553 fichiers et le correctif de fixture compilent pour un seul sélecteur exact. Le test conserve l’époque produit de deux secondes. Sur le Mac sous charge, la commande rend **FAIL** avec le code 101. Au format 16, `initial_input_revalidation` prend 2 338 ms dans un segment de 2 405 ms et se termine par `human_authority_epoch_expired`. Les assertions de reprise et du reçu OIDC restent non atteintes. Ce résultat ne départage pas le coût de revalidation et le temps d’attente dû à la contention de l’hôte.

Le conducteur, les journaux et les empreintes sont sous `target/qa-evidence/codex-p1-oidc-oracle-replay.Ue0rpLsm`. SHA-256 du manifeste patché `74bbda0774d1692d6b015f76050cac5e62ccca07c261dfc9c68e99ba57e2a7f0`, du libtest `916569827976f496ced64988165d522ab543a2f4748f3a93b00de2ff05d57595` et de la sortie du test `3391628dfc9a7f59ddcc367af8eafaa6a50e593b2898dce17a85c3cb258d7c18`.

### Workflows CI et release intégrés à la candidate

Les deux workflows passent leurs vérifications après port des gardes du lien CI, du JSON unique de version et des octets des archives du brouillon. La CI sélectionne aussi le test de l’étape navigateur simulée. Le gel des fichiers courants donne 34 cas de garde, 10 tests du prévol fournisseur, trois scénarios du tag manuel et 15 blocs Bash valides. Sept tests frontend ciblés passent avec Node 25.6.1. L’index local contient les deux workflows et leurs sept entrées ciblées. La CI et le tag sur GitHub restent **NOT_RUN**.

Rejeux et preuves sous `target/qa-evidence/g12-candidate-workflows.sKA9lvSx`. SHA-256 du reçu des gardes `b822e3c2f8d343fbb490966540835e408d8b8c9473a1e3a1db13133e5c221168`, du tag simulé `3693bdd16364bca9df4f000dd20da6ae65ca583881f5feb47f5f5a0721616d38` et de l’index `e7f930de9f03fac3b9acfbe72d1b5fb560c211bf8b50df975050f4be1fa5c99e`.

### Diagnostic ObserveOnly du chemin moteur

La candidate reporte `observe_only` lorsqu’une tentative reçoit la directive d’observation sans émission. Le code sérialisé garde `container_id=null` et le dashboard affiche l’étape avec un effet moteur non confirmé. Le contexte d’une tentative antérieure doit encore être réconcilié. Deux tests Rust isolés de la structure et 26 tests du modèle frontend passent. Le journal moteur G09 antérieur ne prouve pas que cette directive ait été atteinte. Le nouveau diagnostic reste à vérifier sur un rejeu moteur complet.

Les journaux et les empreintes des modules ciblés sont dans `target/qa-evidence/g09-observe-only.mjFmK5b1`.

### Présence d’une liaison humaine sur deux bases MySQL

Le gel SQL de 947 fichiers compile sur Darwin ARM64. Son test Rust passe sur MySQL physique en TLS avec mot de passe puis en mTLS avec mot de passe. Chaque serveur utilise deux schémas neufs liés à des tenants et namespaces distincts. Le compte de lecture dispose du seul droit `SELECT`. Le test réouvre le Store, conserve la présence d’une liaison révoquée et refuse un marqueur mal formé ou une projection divergente. Il contrôle l’ordre d’apparition de la session référencée par un droit et compare le préfixe du tenant voisin octet par octet. Le nettoyage de chaque projet de recette est **PASS**.

Reproduction et reçus sous `target/qa-evidence/mysql-identity-read.VV9eKbj8` dans le worktree SQL. SHA-256 du manifeste `faf49ff6cfe4c03ff088b2af5fa8b0a0153a6a51aae2a6d37e691f1e98ca7a37`, du binaire `c0e8c896bf4c21b5826f470cac9659f51de0275281bccca7795b0a851806855d` et du résultat physique `4923ce47e0bfb23c4fcb5a81ce5b7733be89730c2f6e454b4c432ade9d9b5141`. La lecture de présence workload, les mutations typées et les appels du gateway nécessitent leurs propres preuves.

### Six régressions Store15 sur le gel post-intégration

Le libtest Linux ARM64 du gel de 1 131 fichiers exécute les six sélecteurs exacts en deux conteneurs séquentiels, sans réseau et avec les délais produit d’origine. Les six cas passent. Dans le scénario interprocessus, la recette provoque l’arrêt de son enfant après le commit SQLite, puis dans un second cas après l’enregistrement de la confirmation. Le parent retrouve le reçu de séquence 15 et la tête de séquence 15 sur le Store14 voisin. Les cinq autres cas couvrent la migration explicite, la confirmation exacte, le parent distinct de sauvegarde, le refus d’un propriétaire actif et la lecture du Store14 voisin. Les deux groupes utilisent le même binaire attesté avant et après le rejeu.

Reçu `52886f4496db0ef913a5123a111c18fe242a0cf3c88337905ca0c620a96783c0` sous `target/qa-evidence/codex-p2-store15-build-only.it8KLEtF/exact-six.isqh77jt`. SHA-256 du manifeste `13835cef1698b7a4739a6dad3567b0e6cab91c9ea9e907422f752b6121478d84` et du libtest `110ee61f469cf6c7167bf6e7cbd2e48c470767bf6129ab6a99de98a5bb1f28f4`.

### Rejeu P5 des refus historiques

Le même libtest Linux ARM64 exécute en séquence les trois sélecteurs du triage P5. La détection d’une modification non vérifiée sur la même connexion passe. Le test d’ouverture legacy du Store14 s’arrête sur le texte attendu et le test du diagnostic de récupération s’arrête sur le code attendu. Aucune échéance d’époque n’expire. Le reçu des trois cas reste **1 PASS, 2 FAIL**, SHA-256 `ae95583fcd91b37e736eb26385aa1637d5a0f643f0f75af5e25dd6cdd46d4644` sous `target/qa-evidence/codex-p2-store15-build-only.it8KLEtF/p5-exact.61ynuewb`.

Deux relectures du chemin gelé confirment le refus typé `ReviewAuthorityFailure::Required` pour l’ouverture legacy du format 14 et `NeedsReconciliation` lorsque le journal de récupération est absent. Les deux tests de la candidate attendent maintenant ces variantes. Ils contrôlent aussi la conservation de la base et de sa sauvegarde, et l’absence de création du répertoire manquant. Ces assertions viennent après celles qui échouaient. Le nouveau libtest Linux ARM64 passe les deux sélecteurs avec les assertions de préservation. Le même rejeu passe la distinction `400` et `503` des deux alias de lecture du journal et la sérialisation du diagnostic ObserveOnly. Quatre cas passent en 1,05 s, aux délais produit. Reçu `14d84c29b7b3a52ad3abe405381e95a145701c3a4761f7aff51c997a5ac119c8` sous `target/qa-evidence/candidate-four-libtest.tGYsz23U/four-exact.ntog79xr`. La suite P5 complète et le scénario Docker G09 restent ouverts.

### Commandes de rejeu fournisseur depuis le dépôt

Les guides fournisseurs EN et FR de la candidate donnent les mêmes commandes pour le prévol simulé, le pseudo-terminal, le prévol avec les binaires installés et un essai avec le vrai fournisseur et Chromium. Les deux guides ajoutent le dossier de preuve neuf et les versions de référence. Le rejeu des dix tests Python de fixture et le dialogue à deux reçus en pseudo-terminal passent. Pas de nouvel appel fournisseur lors de cette vérification. Les deux sections de documentation sont indexées localement avec leurs liens résolus. Empreintes EN `6dcc6f06c7b44708240073c37fa16abf8bbf4dd40aef13f55a3009ee350150b6` et FR `91e6c4dd603cb674fff045f7ef675eae55a7694904bfee07d71039dfde57fcde`.

### Nouvelle sonde Kubernetes locale

Le contexte `docker-desktop` pointe vers une origine HTTPS en boucle locale. L’unique requête `/readyz`, bornée à cinq secondes, renvoie EOF. Le statut reste **BLOCKED** avant le lancement de la recette à un worker. Le contexte et les ressources du cluster ont été préservés.

### Recettes navigateur indexées et suite frontend locale

Le lot indexe le harnais `runtime_user_journey`, son module Podman et les conducteurs Chromium de fin de session et de suivi en direct. La recette terminale conserve des captures HTML et PNG assainies quand une page authentifiée est disponible à l’échec. Le guide indexé documente la construction du bundle, Playwright et Chromium, et la sélection Docker ou Podman. Reçu `4f6b7f60dd2d3d47cfdaf6f19f0879eb93701839d4f964deaf288457ab37c9ad` sous `target/qa-evidence/claude-p3-pin.ulidikob/local-index.json`. Le build et le rejeu physique de ces quatre entrées exactes restent **NOT_RUN**. La preuve Podman terminale antérieure porte sur un autre parent Rust.

La suite frontend locale porte sur le gel de 89 fichiers de la candidate, Node 25.6.1 et 31 fichiers `*.test.mjs`, avec réseau interdit. Elle rend 247 PASS, 0 FAIL et 8 SKIP. Les huit cas attendent des captures de réponses Rust. Le premier parseur supposait le format TAP et n’a pas lu le résumé du rapporteur par défaut. Le reçu corrigé interprète le même journal et le même code retour 0. Manifeste `a492093269275e3bfb79c23a873130b9f87a3c360b3825abadf1e97b4de48f99` et reçu corrigé `8087d1c8940502e3641636f45b28872cd082a0802f38cf872f81dbb9c12bd51a` sous `target/qa-evidence/front-unit-current.q3e02a37`.

Le bundle du même gel compile avec TypeScript et Vite 7.3.7 sous Node 25.6.1. Le cache local ne contenait pas Vite. Le passage suivant obtient les archives fixées dans le lock sur le registre npm public avec une configuration de recette neuve. Le premier passage de cette configuration utilisait le même fichier vide pour les scopes npm et a été refusé avant téléchargement. Avec deux fichiers distincts, l’installation et le build sont **PASS**, sources préservées. Le build s’exécute sans réseau et produit quatre fichiers d’assets. Reçu `574c0c72df7a0b3af56d48798902aebedfc61863c035cc6497b8c1fba87169da` sous `target/qa-evidence/front-unit-current.q3e02a37/registry-bundle`. Le navigateur avec ce bundle et l’exécution sur la version Node de CI restent distincts.

L’archive officielle Node 24.22.2 Darwin ARM64 est vérifiée par SHA-256. Cette version est celle du workflow CI. La suite hors réseau confirme **247 PASS, 0 FAIL, 8 SKIP**. Le bundle se reconstruit avec les mêmes octets pour les quatre assets qu’avec Node 25. La plateforme du résultat est Darwin ARM64. Reçu `b0f0fd0fc3a171330846cdb1010bbc908c0121b10c9224d6744cda1a1eabb256` sous `target/qa-evidence/front-unit-current.q3e02a37/ci-node-darwin`.

### Prévol de mesure OIDC sans recompilation

Le libtest Darwin du rejeu P1 corrigé reste à l’empreinte `916569827976f496ced64988165d522ab543a2f4748f3a93b00de2ff05d57595`. Le nouveau conducteur Rust atteste ces octets, isole ses sorties et prépare les temps utilisateur, système et mural. L’inventaire du sélecteur exact est **READY**, reçu `b91879205edbb38c4302bb7f4b70a850201dbeb724e79d91f0cb1e7a2dc22768` sous `target/qa-evidence/codex-p1-oidc-oracle-replay.Ue0rpLsm/out/measured.pLoS3BSB`. La mesure métier du nouveau conducteur reste **NOT_RUN**. Le dernier rejeu métier garde son échec à deux secondes.


### Préparation du rejeu workload MySQL

Le worktree SQL contient la projection préparée, sa vérification domaine et la recherche du worker avant l’émission dans l’instantané vérifié. Le test physique compare deux digests en rotation après réouverture et le préfixe du second tenant. Il couvre la capacité manquante, la projection divergente et une ancienne liaison dont la réservation vient après. Le gel de 950 sources réutilise une copie APFS de la cible précédente. Le prévol atteste les 11 changements et se termine avant démarrage faute de reçu de build. La compilation et les profils TLS/mTLS restent **NOT_RUN**. Reçu de préparation `dd71e6c5a73ca7ec701f297d17ca9b7a8654e66648db9f2f3a7f34539a0b5273` sous `target/qa-evidence/mysql-workload-read.343xyty6` dans ce worktree.


### Revue des gardiens conservés

Sur le relevé local, neuf processus gardiens sont rattachés à PID 1. Sept concordent avec des fixtures QA terminées en échec à la soumission et dont le reçu conserve le gardien pour diagnostic. Les deux autres n’ont pas de reçu terminal associé dans ce relevé. Le groupe utilise environ un cœur au total sur huit échantillons. Cinq gardiens ont lancé des descendants Docker. Parmi dix identités d’auxiliaires vérifiables, deux sont présentes et à l’état `created` avec `Running=false`, et huit sont absentes du daemon local. Le coût du daemon et la portée des ressources non identifiées restent inconnus. Les gardiens et les ressources ont été préservés. Le relevé de capacité garde le lancement du build Darwin G09 en attente.

Le contexte Docker Desktop répond encore EOF sur la requête locale `/readyz`. La qualification d’un pod réel demeure **BLOCKED** avant la création d’une fixture.


### Viewer Chromium avec le bundle courant

Le vrai dashboard Rust et le gateway de recette démarrent en boucle locale dans un conteneur Linux ARM64 sans réseau. Chromium 156.0.8078.80 utilise son sandbox. Le conducteur vérifie les deux réponses d’identité et le rendu de la Vue d’ensemble. Les quatre assets du bundle courant sont montés en lecture seule. Le backend Rust est celui du gel archivé `ee9b4512…`. Aucun worker n’entre dans ce scénario. Le résultat de compatibilité est **PASS**. Reçu `7f75829966b53fba7b2605b8e30316fce23aa531b216e7f982587e667b36e830` sous `target/qa-evidence/g08-current-bundle-viewer.HKVWek0u/viewer.7a04t4kw`.

### Installation physique du DDL workload MySQL

Le DDL du gel workload est appliqué à une base liée de recette sous mot de passe et TLS, puis sur une nouvelle base sous mot de passe et mTLS. Chaque passage vérifie 6 tables InnoDB et 8 clés étrangères, le préfixe préexistant conservé et le nettoyage du seul projet de recette. Sans certificat, le même compte mTLS reçoit le refus SQL 1045. Les deux profils passent avec une limite effective de 1 CPU et 1 Gio, sans conteneur client supplémentaire. Le build Rust et le rejeu du Store workload restent **NOT_RUN**. Reçu `331f8f3aea7b19f89ea03bb0e277c626163a917c971080ee3d782840e3e7efe4` sous `target/qa-evidence/mysql-workload-ddl.8s6uh952` dans le worktree SQL.

### Orientation du mainteneur sur le Mac sous tension

Le mainteneur écarte le rejeu chronométré OIDC et le rejeu G09 sur ce poste. Les statuts historiques restent attachés à leurs reçus. Le travail de qualification continue sur les autres parcours.

### Recette fournisseur manuelle de fin

Le mainteneur réserve l’authentification d’abonnement Codex et Claude Code à ses essais manuels de fin. Les essais réels par clé API reprennent en autonomie, y compris en TTY. Les reçus existants gardent leurs statuts historiques. Le nouveau verdict réel attend les essais du mainteneur. Les cas d’interface, les prévols hors fournisseur et les tests logiciels du harnais poursuivent leur validation indépendante.


### Agrégateur et harnais fournisseur hors réseau

La CI locale intègre les neuf cas du harnais broker avec les dix cas de prévol et navigateur simulé. Les deux commandes locales passent. Le code indexé de l’agrégateur compile en réutilisant les bibliothèques Darwin archivées et donne 14 PASS. Le premier assemblage autonome omettait la dépendance `libc`, ajoutée au passage attesté. L’agrégateur lit les deux reçus historiques réels du batch API Codex Luna et Claude Haiku 5.5, avec CLI et image identiques. Il rend 2 PASS et 6 NOT_RUN sur ces deux seules entrées. Les TTY API reprennent sur une recette séparée. La garde `--require-pass` rend le code 1. Reçu `d2e7188bb9df67ec22ac681395d00e778106286eb7e4abead958a48b5a44a404` sous `target/qa-evidence/provider-matrix-current.pyt9_lmt/fixed-link.cjd46627`.


### TTY API réels avec le conducteur intégré

Les deux recettes utilisent l’image Docker ARM64 locale attestée, le CLI et le bundle du gel P6b et le conducteur interactif actuel 03a3. Un seul worker est lancé à la fois dans une pile jetable. Le prévol du compagnon P6b passe les 16 sous-cas de référence avec les données synthétiques du harnais courant. Les vrais essais respectent les 120 s du worker.

Codex `gpt-6-luna` est **PASS** en TTY réel. Le CLI sort à 0 après deux reçus distincts et restaure le terminal. Le worker termine, son quota est libéré et son retrait est confirmé. La recette vérifie le projet préservé, l’export, l’audit valide et le rapprochement Chromium. SHA du reçu du cas `945fe99a3294b956a4e53db08962a97671de14822e3a9077178e5f5d33be69f4`.

Claude Code `claude-haiku-5-5` est **FAIL** avant le premier reçu. Les écrans d’initialisation et `working` sont observés. Le CLI sort à 1 avec terminal restauré. Le worker atteint un état terminal et libère son quota. Son code d’exécution et d’attachement 137 apparaît après 119,211 s de vie observée, sans OOM ni motif d’arrêt transport attestés. L’origine de l’arrêt et la requête Anthropic restent inconnues. SHA du reçu du cas `80dac0e4c404e1a52b35a3464a148fadffdb3637a24c1e98be6d5f959d7229a3`.

Le conducteur reproductible et les deux preuves sont sous `target/qa-evidence/claude-api-tty-current.w1o2CvAw`. L’agrégation hors réseau sur le même CLI et la même image donne **3 PASS, 1 FAIL, 4 NOT_RUN** avec les deux batchs API déjà attestés. Les quatre cases abonnement attendent les essais manuels du mainteneur. Reçu `e1716a39bd46f4f5c5ec605b21983205612d145cf9603c4217164d9aebf8e3ae`.

### Diagnostic après sortie TTY et contrôle Chromium des motifs

Les 11 tests de prévol et navigateur simulé, les 9 tests du harnais broker et l’autotest sur PTY privé passent hors fournisseur. Le reçu TTY conserve le code de sortie, le dépassement d’échéance, le motif transport, l’état OOM et le retrait uniquement lorsqu’ils appartiennent à la session et au worker attendus. Les informations absentes restent inconnues. Le conducteur lit une queue bornée du PTY après l’annonce de sortie pour relever les écrans d’état connus. Les marqueurs reçus après cette annonce ne valident aucune réponse. Les trois sources du harnais indexées concordent avec le relevé sous `target/qa-evidence/tty-diagnostic-current.SqKNGnZU`. Aucun appel fournisseur supplémentaire. Les sources figées et le lanceur `run-api-tty-replay-03a3.py` restent disponibles avec les deux reçus réels.

Chromium Linux ARM64 avec sandbox vérifie l’affichage de quatre combinaisons de l’échéance et du motif, sur le bundle du worktree et une fixture HTTP loopback. Le premier montage Docker échoue avant le démarrage du navigateur. Le rejeu avec un seul montage de recette contenant les quatre assets passe. Les deux résultats, le script et le journal sont sous `target/qa-evidence/browser-timeout-current.fiwsz2sf`. Cette preuve couvre l’affichage sur fixture. Le TTY Claude reste **FAIL** sur le dernier essai réel.

Le conducteur générique Docker avec sa sonde Rust est sous `target/qa-evidence/tty-generic-offline.E9S4Phvf`. Il prévoit un contrôle positif de l’aller-retour TTY et un arrêt volontaire sur la borne, sans clé ni modèle. La syntaxe et la revue statique passent. La compilation de la sonde et le parcours Docker restent **NOT_RUN** sur ce poste sous charge.

La première prévisualisation de l’index frontend révèle six erreurs TypeScript déjà présentes dans la composition indexée. La vue du journal lisait `olderError` alors que le store indexé précédait cette propriété. L’intégration reprend les transitions du store pour la pagination ancienne et leurs trois régressions. Le rattrapage du direct garde sa sélection propre. L’index recomposé passe TypeScript, Vite et les tests Node avec **228 PASS, 8 SKIP**. Les huit cas ignorés attendent leurs données Rust. Le script Chromium indexé avec sandbox passe aussi ses quatre cas sur les assets de ce même index. Un premier essai Chromium sur cette composition s’arrêtait au chargement de `playwright-core`. Le rejeu monte les deux paquets épinglés sous `node_modules`. Reçu, journaux et commande de rejeu sous `target/qa-evidence/browser-timeout-current.fiwsz2sf/index-composed.LpLnR1uM`.

### Réponses HTTP Rust relues par le frontend

Le binaire Darwin ARM64 archivé `supervision_stack_api-827058ea3fc075f9` produit six captures exactes du gateway et du proxy dashboard sur boucle locale. Les trois cas Rust passent en 17,05 s, 23,09 s et 125,76 s. Le registre vide reste une lecture réussie. Les 120 entrées de pagination sont des données SQLite de recette. Les événements de session suivent les droits et se rattachent à la ressource exacte. Aucun worker ni fournisseur n’est lancé.

Les trois oracles Node 24.22.2 passent en consommant les six réponses. Les 29 fichiers de frontend vérifiés correspondent à l’index courant. Le premier setup, sans réseau, avait refusé l’ouverture des serveurs avant toute requête. Le reçu validé conserve cet échec et le rejeu limité à localhost. Le SHA du binaire est `bb09e86abfb3274edc83b16449a971327151177e7004e41ac5c4b2288bd32886`. Le gel de compilation n’est pas attesté. Cette preuve vérifie la compatibilité frontend et ne qualifie pas le backend du checkout actuel. Reçu et rejeu sous `target/qa-evidence/front-rust-wire-archived.RbQmSwPp`.

Les cinq oracles restants sont **NOT_RUN**. Leur prochaine exécution demande de compiler les cibles `federation_workers_api`, `human_workers_api` et `human_recent_events`. Le lanceur `front/tests/run_rust_wire.mjs` assemble les quatre suites Rust et les huit oracles Node dans une recette privée. Son mode `supervision` cible uniquement le groupe vérifié ci-dessus. Le build de cette recette sur le checkout courant reste **NOT_RUN**.

Le nouveau prévol du TTY Rust générique confirme le compte de contrôle, Docker Desktop et les deux images locales épinglées. La compilation de la sonde et le lancement restent **NOT_RUN**. Le prévol MySQL workload confirme quatre dépendances réutilisables et l’absence de reçu de build pour la régression Store. Le DDL sous TLS et mTLS garde ses reçus physiques distincts.

### Journal récent sur le gateway et le proxy archivés

Le vrai exemple Rust de supervision produit deux pages de journal et deux refus HTTP. Le profil de recette utilise 12 entrées workers et 400 événements synthétiques sur SQLite. Les quatre corps intacts passent l’oracle Node 24.22.2 du journal récent sur le gel frontend indexé. Le refus de détail concerne un UUID inexistant. Il fournit le même contrat que le refus d’une ressource hors périmètre et que le corps attendu pour un gateway antérieur. Les captures et le conducteur sont sous `target/qa-evidence/front-recent-http-archived.aCVbAGYK/run.Fji4nA`. Le premier démarrage avec zéro entrée ne remplissait pas les préconditions de l’exemple. Un chemin de détail non UUID avait ensuite renvoyé le refus du proxy. Les deux essais sont conservés. Ce résultat ajoute un quatrième oracle avec backend archivé. Les quatre oracles fédération et workers legacy restent **NOT_RUN**.

### Garde CI de version et régression de montée de version

La garde de la CI compare désormais la version Cargo et la révision exactes au JSON du CLI. Sur six cas locaux, l’ancien bloc acceptait une version absente ou différente. La nouvelle garde rejette les deux, ainsi qu’une mauvaise révision, un JSON dupliqué et un échec Cargo. Le module Rust d’identité compilé isolément sous la version de recette `1.0.0` passe 12 tests après intégration des attentes de version dynamique. L’état initial passait 8 tests et échouait sur 4. L’autotest PTY macOS passe et figure dans la CI. Les preuves sont dans `target/qa-evidence/ci-build-identity.OfUD6lQO`. Le run GitHub hébergé reste **NOT_RUN**.

Le job de brouillon valide le contenu minimal de chaque archive avant son premier appel GitHub. Licence, métadonnées et fichiers requis doivent être réguliers, non vides et uniques. Le natif contient le CLI exécutable et le frontend son HTML et un fichier JavaScript. Les 26 cas hors réseau passent avec des archives réelles et un GitHub de recette. Quatorze archives invalides ne déclenchent aucun appel. Le tar physique de ce rejeu est BSD. Le format GNU est aussi couvert par six fixtures textuelles. La preuve n’atteste pas le run Ubuntu hébergé. Reçu et lanceur dans `target/qa-evidence/g12-draft-archive-content.r55r0p3k`.

Le contexte Kubernetes Desktop désigne le serveur local avec contrôle TLS. La requête `/readyz` renvoie EOF avant l’inventaire des classes et des pilotes. La qualification Kubernetes reste **BLOCKED**. Dans le worktree SQL, le chargeur refuse encore les deux profils MySQL avant écoute. La validation du DDL et des tests Store archivés ne qualifie pas l’ouverture du gateway distant.

### Huit oracles frontend sur des réponses Rust attestées

Deux libtests Darwin sont liés au rlib G09 archivé `db964146…`. Leurs sources correspondent au gel et la bibliothèque historique garde son identité distincte. Les deux cas Rust de fédération A/B et legacy puis les trois cas des workers legacy passent. Ils fournissent six réponses brutes. Les seize captures des quatre groupes restent intactes et les sources front correspondent à l’index de la candidate. Node 24.22.2 obtient **8 PASS, 0 FAIL, 0 SKIP** en une exécution. Le premier lien de fédération omettait `chrono`. Le lien corrigé et le second binaire passent. Reçu agrégé et rejeu sous `target/qa-evidence/front-worker-wire-darwin.abRkueun`. Un seul gel backend recompilé sur la candidate reste **NOT_RUN**.

### Sonde MySQL liée au registre de démarrage

Le worktree SQL partage la lecture bornée et le décodeur du registre v1. La résolution typée conserve le profil MySQL configuré et la garde SQLite du gateway continue de refuser le démarrage MySQL avant les credentials. La sonde existante accepte un registre et un tenant explicites. Elle ouvre uniquement les références SQL de cette entrée et appelle `MysqlAuditStore::verify()` en lecture seule. Les sorties ne contiennent que le mode configuré, la liaison vérifiée et la tête d’audit. La première compilation isolée a révélé une incompatibilité `Path` / `PathBuf` entre les deux formes SQLite. Après correction, les quatre tests d’arguments, sélection et parité des champs passent avec les dépendances SQL archivées. Reçu sous `target/qa-evidence/mysql-registry-wire-isolated.GaqSI8dU` dans le worktree SQL. Le build complet et le rejeu physique de la sonde du registre sur TLS et mTLS restent **NOT_RUN**.

### Sonde du registre sur MySQL physique

La sonde du registre a été compilée depuis les sources courantes du worktree SQL et liée au Store du gel MySQL identité. Les deux profils physiques passent en série sur Docker Desktop. La cible B liée répond à la séquence attendue 4. Le schéma sans liaison, un tenant différent et un namespace différent sont refusés. En mTLS, l’absence de certificat et le mauvais mot de passe rendent séparément le code serveur 1045 avec le même compte de lecture. La tête est identique avant et après les refus. Les deux projets confirment leur nettoyage. Les 5 et 7 cas et leurs commandes sont conservés dans `target/qa-evidence/mysql-registry-cli.neHh9rBf`. Le démarrage et l’ajout par le gateway restent ouverts.

### Reprise d’une page ancienne du journal dans Chromium

Le scénario `journal-older-recovery` passe avec le vrai gateway Rust archivé et le bundle frontend indexé. Il conserve deux pages déjà lues, injecte un HTTP 503 pour la page suivante et vérifie que le direct continue. La reprise au clavier garde le focus, réutilise le même curseur et reçoit HTTP 200 du gateway. Elle conserve le flux existant, qui se ferme à la déconnexion. Le premier conducteur intégral et la sélection minimale ensuite indexée passent séparément. Le reçu du conducteur exact est sous `target/qa-evidence/browser-journal-older-index.wdkVP3iD`.

Le test Rust distribué `identity::prepared_registry_probe_preserves_bound_target` passe sur TLS et mTLS physiques avec le même Store archivé et les sources actuelles du test et du probe. Il rapproche les préfixes A/B autour des refus. Le premier conducteur conservait le nom du test historique dans son oracle de sortie. Le conducteur corrigé vérifie le nouveau sélecteur exact. Le reçu est sous `target/qa-evidence/mysql-registry-recipe-compile.6RVDP3NA/physical-corrected.json`.

### Précondition d’exécutable de la sonde TTY

Le compilateur Docker hors réseau produit l’ELF ARM64 attendu et retire son conteneur de recette. Le premier appel CLI refuse le chemin `/workspace/tty-probe` au contrôle de politique, avec `invalid_executable`, avant toute réservation. L’audit et le binaire restent conservés. Le résultat ne couvre aucun attachement TTY. Le premier reçu est sous `target/qa-evidence/tty-generic-offline.E9S4Phvf/both.sj4ym5ld`.

La variante suivante installe les mêmes octets sous `/usr/local/bin/qa-tty-probe` dans une image de recette dérivée. Les métadonnées du fichier et les couches de l’image sont vérifiées avant lancement. Le premier worker Docker passe les deux reçus et la sortie native à 0, avec projet original préservé, trace de fin, quota libéré et retrait confirmé. Le second obtient les deux reçus, puis sort à 137 après 119 784 ms de transport. Le résultat durable indique `timed_out=false` et aucun motif d’arrêt transport. Le test d’échéance échoue donc sur l’attribution de la cause. Le quota, le worker, le conteneur de préparation et l’image de recette sont nettoyés. Aucun fournisseur n’est appelé. Le symptôme déjà observé lors du TTY Claude se reproduit ainsi avec une sonde Rust en réseau coupé. Reçu sous `target/qa-evidence/tty-qa-image-identity.cq1yU49W/both._cdifm_m`.

Le rapprochement en lecture seule avec les octets conservés montre le même diagnostic exact dans les checkpoints du TTY générique et du TTY Claude. Le contrôleur conclut à une expiration de sa fenêtre ou de l’échéance originale. Le résultat TTY terminal ne porte ni `timed_out=true` ni le motif transport. Aucun reçu corrélé n’établit l’auteur du signal moteur. L’oracle initial cherchait le diagnostic sans préfixe interne. Le conducteur corrigé enregistre le refus de contrôle séparément de l’attribution et le reçu original reste inchangé. Le dossier du passage contient `deadline-evidence.json` et son vérificateur.

### Typage de la candidate TTY et verdict de la matrice fournisseur

Le typage Rust 1.99.1 Darwin ARM64 passe hors réseau sur 707 entrées gelées, toutes identiques aux fichiers courants au contrôle après compilation. Rustc réutilise les métadonnées des dépendances du même verrou et produit une nouvelle métadonnée de la bibliothèque. Le reçu `target/qa-evidence/tty-current-rust-metadata.df5PqoRY/check-result.json` atteste les entrées et un code zéro en 216,38 s. L'édition de liens du CLI et le rejeu moteur courant restent **NOT_RUN**.

Les deux binaires de l'agrégateur ingèrent le même reçu synthétique TTY. Il affirme une sortie à zéro et deux réponses avec terminal restauré, tout en attribuant une échéance au contrôleur. L'ancien binaire le classe **PASS**. L'agrégateur compilé depuis la source désormais indexée le classe **FAIL** et passe ses 14 tests. Les quatre reçus API historiques à empreintes vérifiées donnent les mêmes verdicts qu'avant, soit **3 PASS, 1 FAIL et 4 NOT_RUN** pour les huit cases. Rejeu et journaux sous `target/qa-evidence/provider-matrix-tty-verdict.zS7nkhVs`. Ces verdicts portent sur les reçus sélectionnés et la régression de l'agrégateur.

Le sélecteur Docker opt-in `docker_interactive_records_controller_deadline_and_owned_stop_outcome` réutilise la sonde TTY du dépôt. Il prévoit le rapprochement du même worker entre le CLI, l'inspection Docker et l'audit rouvert, puis la vérification de la restauration du terminal et de la libération du quota. Le typage de `runtime_interactive.rs` contre la nouvelle métadonnée Rust passe en 2,738 s. La première invocation du contrôle de type omettait la dépendance déjà déclarée `tempfile`. Le test et sa commande sont sous `tests/runtime_interactive.rs`. Reçu `target/qa-evidence/tty-current-rust-metadata.df5PqoRY/interactive-test-check.json`. L'exécution Docker du nouveau sélecteur reste **NOT_RUN**.

### Présence combinée des identités sur MySQL

Le gel de 951 fichiers du worktree SQL inclut la lecture combinée des projections humaine et workload et la nouvelle régression A/B. Cargo type la bibliothèque et la cible `mysql_audit_store` avec la feature `mysql-storage`, en un seul job hors réseau. Les deux artefacts racines sont nouvellement typés et Cargo rend le code zéro en 3 min 26 s. Le reçu est sous `target/qa-evidence/mysql-control-collision-check.rodvscjj/typecheck-result.json` dans le worktree SQL. Le gel précède le raccord de la livraison d'événements de contrôle. Les deux rejeux physiques TLS et mTLS et l'ajout SQL-06 via le gateway restent **NOT_RUN**.

### Typage de la livraison des événements de contrôle MySQL

La composition du Store avec la réserve de contrôle et la lecture des reçus de reprise passe le typage de la bibliothèque. La première compilation de `mysql_audit_store` échoue sur l'ordre des arguments du test. Le rejeu modifie uniquement cet appel dans une nouvelle copie du gel et type la cible avec la métadonnée de bibliothèque produite par Cargo. Rustc rend le code zéro. Reçus sous `target/qa-evidence/mysql-control-delivery-check.cr0yw7ze` et `target/qa-evidence/mysql-control-delivery-test-verified.jy4ek84T` du worktree SQL. L'essai sur les deux profils MySQL et le lot de lecture de l'intégrité arrivé ensuite restent à vérifier.

### Image de sonde du test Docker TTY courant

La construction hors réseau utilise les images ARM64 présentes localement par leurs IDs. Elle installe la sonde `wait` sous `/usr/local/bin/ai-guard-interactive-probe` dans une couche au-dessus de la base aux 14 couches vérifiées. Les octets de l'ELF statique, son mode `0755` et l'utilisateur `1000:1000` de l'image sont attestés. L'essai initial s'arrêtait sur le template d'inspection du compilateur, dont le champ utilisateur est absent. L'essai suivant compilait la sonde mais rejetait la forme de la sortie de `docker commit`. Les corrections inspectent les champs facultatifs, l'ID immuable et le tag. Le dernier build est **PASS**, avec les deux conteneurs temporaires retirés et l'image dérivée conservée pour le test. Reçus et conducteur dans `target/qa-evidence/tty-wait-probe-image.YLJ9HFr3`, passage `run.0kckmq9m`. Le rejeu Docker du CLI reste **NOT_RUN**.

### Rejeu Docker de l’échéance TTY sur le CLI lié de la candidate

La bibliothèque, le CLI et le libtest Darwin ARM64 sont liés à partir des 707 entrées attestées. Le premier test physique atteint l'échéance et enregistre le résultat dans le résumé et l'audit. Le test échoue car son filtre d'inspection avant accusé de réception ignore la route réelle sous `/control/api/v1/`. Le témoin du template Docker et les reçus de cet échec sont conservés dans le passage `physical.93zgnk5o`.

Le rejeu relie uniquement le test avec le filtre corrigé et réutilise la même CLI octet pour octet. Un worker Docker sans réseau dépasse la durée autorisée de 120 s. L'inspection indépendante, le registre et l'audit rouvert concordent sur la sortie 137, l'absence d'OOM et l'arrêt acquitté par le moteur. Le résumé et l'audit enregistrent `original_execution_deadline/engine_acknowledged`, avec `timed_out=false` côté transport. Le test vérifie le PTY restauré, le quota libéré et le conteneur retiré. **PASS** en 152,88 s. Les reçus sont sous `target/qa-evidence/tty-current-darwin-link.oNxa3MSr`, lien `run.x70gtgn9` et rejeu `physical.k2ltuni7`. La preuve porte sur la sonde générique et la borne d'exécution originale sur Docker Desktop ARM64.

Chromium avec confinement actif passe sur le dashboard Rust servi par le même CLI et un gateway rouvert sur une copie de la base SQLite de ce test. L'identité viewer est vérifiée avec les reçus de configuration de la recette. Le CLI `worker-status`, les API du gateway et du dashboard, l'événement d'audit haché à la séquence 18 et son rendu concordent sur la session, le worker, la sortie et l'échéance. Les libellés du contrôle portent la provenance observée. L'empreinte de la base SQLite originale reste identique. Reçu et commande sous `target/qa-evidence/tty-browser-continuation.utnIOqg2`, passage `run.YdVFBH`. Le TTY Claude Code API conserve le verdict de son dernier essai fournisseur.

### Quatre scénarios du Store MySQL sous TLS et mTLS

Le gel de 954 fichiers du worktree SQL est compilé avec la feature `mysql-storage`. Les quatre scénarios passent sur MySQL 8.4.13 ARM64. Le compte du premier profil exige TLS et le second exige un certificat client en plus du mot de passe. Chaque scénario ouvre une cible de recette déjà préparée avec des droits limités. Les huit nettoyages passent et le port local de recette est libre après le dernier arrêt.

| Scénario | Mot de passe sur TLS | Mot de passe et certificat client |
|---|---|---|
| Conserver une collision d’identité déjà vérifiée après panne de projection | PASS | PASS |
| Préserver la réserve de contrôle et rejouer les événements déjà reçus | PASS | PASS |
| Distinguer rupture de chaîne et panne de lecture | PASS | PASS |
| Produire les mêmes entités que SQLite et refuser une rupture tardive | PASS | PASS |

Le conducteur, le manifeste, le reçu de lien et le reçu `sql06-store-profiles.w3pmya45.json` sont sous `target/qa-evidence/mysql-sql06-store-entities-libtest.wpq8zizf` dans le worktree SQL. L’empreinte SHA-256 du reçu physique est `421945243a93e74d4d96e0562b5d708e4af7ef6b78e1d02c08615479bffe589b`. Le gel suivant ajoute la synthèse `overview` et rejoue les quatre scénarios. [Rejeu de la synthèse](#synthèse-overview-sur-mysql-tls-et-mtls) Le démarrage du gateway et l’ajout HTTP sur une cible MySQL restent **NOT_RUN**.

### Échéance TTY sur Podman rootless et Chromium

Le même CLI Darwin ARM64 que dans le rejeu Docker est identique octet pour octet. Seul le libtest a été relié pour partager les assertions avec Podman. Un pont Unix privé ouvre le store Podman rootless, avec attestation du moteur local. L’image ARM64 dérivée d’une base locale contient les mêmes octets de la sonde `wait`. Le conteneur temporaire d’inspection de l’image est retiré avant le lancement.

**PASS** en 158,04 s pour le worker Podman sans réseau. Le gateway enregistre un arrêt à la durée d’exécution originale acquitté par le moteur. L’oracle rapproche l’inspection avant ACK, l’état terminal dans `worker-status`, le PTY restauré et la chaîne d’audit rouverte. L’arrêt libère le quota, retire le worker et ferme le pont. Le délai du transport TTY reste `false`. Zéro appel fournisseur.

**PASS** dans Chromium pour la même session sur le dashboard Rust. Le navigateur et le CLI utilisent le même worker et l’événement observé `command.failed` à la séquence 18. Le reçu rapproche également l’image, l’identité d’exécution, l’autorité du runtime, le hash de la spécification et l’empreinte de la base originale, inchangée après la lecture sur une copie.

Le conducteur natif et le reçu sont sous `target/qa-evidence/podman-tty-wait-rootless.91Cm1Vtt`, passage `physical.fya4nga5`. Le conducteur Chromium et le reçu sont sous `target/qa-evidence/podman-tty-browser-continuation.UscGLvI0`, passage `run.7QU2gg`. La preuve emploie une session générique et s’applique à Podman rootless ARM64.

### Synthèse overview sur MySQL TLS et mTLS

Le gel de 956 fichiers est lié avec la feature `mysql-storage`. Les cinq sélecteurs sont exécutés sur le vrai MySQL ARM64 avec compte `REQUIRE SSL`, puis avec compte `REQUIRE X509`. **10 PASS** et dix nettoyages confirmés. Le port de recette est libre après le dernier arrêt.

Le scénario supplémentaire compare les compteurs et les agents à SQLite sur le même historique. Il contrôle la cohérence du résultat quand un autre client ajoute une ligne et exige un refus si une rupture tardive empêche de vérifier le préfixe. Les quatre scénarios précédents sont rejoués dans la même campagne.

Le conducteur et le reçu `sql06-store-profiles.oa40i3mm.json` sont sous `target/qa-evidence/mysql-sql06-store-overview-libtest.5lqvgt0g` dans le worktree SQL. Le SHA-256 du reçu est `48ff56fe9a277a7f0fbb601037f09e7358bb627c4821b40030c5e60df071cc7e`. Les lectures de workers arrivent après ce gel. SQL-06 via le gateway MySQL reste **NOT_RUN**.

### Retour Writer et contrôle de génération du batch Codex

Le libtest Darwin ARM64 du gel `g09-queue-replay-complete-inputs.QJ6lC7mg` exécute le sélecteur `runner::broker::executor::journal::tests::decision_ack_publication_and_generation_gate_are_distinct`. Le conducteur vérifie le manifeste, les octets du binaire et l’inventaire d’un seul test. **PASS** en 3,22 s avec le vrai gateway local, SQLite et le Writer de ce gel. La recette synthétique distingue un ACK durable dont le retour au caller échoue, un retour réussi suivi d’une génération périmée et le début de préparation quand elle reste courante. Elle vérifie un seul POST par variante et aucun démarrage de worker.

Le fichier de test et la couture d’exécution correspondent à la candidate. Le Writer, le journal sous-jacent et le manifeste Cargo ont changé. La preuve porte sur le gel archivé et la cause de l’échec Codex/API réel reste indéterminée. Le reçu et le conducteur sont sous `target/qa-evidence/g07-decision-native-replay.lSdD5je5`, passage `run.12c8j8lb`. SHA-256 du reçu `b6df1bda4d36bd832ffb9bbf7fc09c66d885c289328ab8e119898fc825fada23`.

### Rejeu Rust SQL-06 avec arrêt propre du gateway SQLite

Le test Rust `sql06::prepared_sqlite_gateway_replays_the_complete_receipt_after_restart` passe en 2,77 s. Il prépare deux cibles SQLite explicites, démarre et arrête proprement deux instances successives du vrai gateway sur loopback, puis compare le reçu HTTP complet avant et après redémarrage. Le viewer reçoit 403. La tête de A avance une seule fois, son préfixe reste identique et la lecture SQLite indépendante confirme que les lignes de B restent identiques.

Le premier lien du fichier de test global échoue car il utilise aussi des fonctions du registre ajoutées après le gel. Le nouveau conducteur compile le module SQL-06 intact, avec le CLI et la bibliothèque Darwin ARM64 archivés. SHA-256 du CLI `d7ee877148dc1ed94ce69d4c213a506364c86b648956b4b3338e65f81ff5934d`. Reçu, lien et lancement dans `target/qa-evidence/sql06-sqlite-oracle-only.07tp0ryc` dans le worktree SQL. SHA-256 du reçu `feb3b208723085853b3af94c23f1df65848f27b23211f220aceaec237be4b653`. Les profils MySQL du gateway et le rejeu sur le CLI courant restent **NOT_RUN**.

### Détail et pagination des workers sur MySQL

Le gel worker dérive du gel `overview` avec trois fichiers modifiés. Le sélecteur `prepared_target_bootstrap_append_replay_and_capacity` passe en mot de passe sur TLS puis en mot de passe et certificat client. Chaque profil prépare une base distincte, puis confirme le retrait de ses ressources. Le port de recette est libre.

Le test relit un worker terminé et un worker annulé. Il vérifie l’ordre des UUID, le curseur exclusif, l’absence d’un worker valide inconnu et le rejet d’un curseur ou d’une limite invalide. Une projection hors page divergente et une ligne d’audit corrompue provoquent le refus du détail et de la page. Après restauration des lignes de recette, la tête et l’historique correspondent au préfixe attendu.

Rejeu dans `target/qa-evidence/mysql-worker-read-libtest.8ndhvma4` du worktree SQL. Le reçu `worker-read-profiles.zmia47ql.json` a le SHA-256 `38b645c47eff8670a20aeacba83c39bc106ad84fae27e047806d392d2f921d67`. La preuve porte sur les lectures Store. Les lectures HTTP MySQL et SQL-06 via le gateway restent **NOT_RUN**.

### SQL-06 SQLite après retrait du chemin de l’ancienne source

Le sélecteur Rust enrichi passe en 1,34 s sur le CLI Darwin ARM64 archivé `d7ee8771…`. Il ouvre d’abord l’ancienne source configurée et vérifie son historique depuis le vrai gateway. Après un arrêt propre, il conserve cette base dans le dossier de recette et rend son chemin d’origine indisponible. Le registre désigne alors uniquement les deux nouvelles cibles préparées.

Deux autres démarrages confirment le refus du viewer, l’ajout unique dans A et le reçu identique après rejeu. Le test compare les lignes d’audit de A, du témoin B et de la source conservée. Le chemin précédent reste absent après le rejeu. Le conducteur et le reçu sont dans `target/qa-evidence/sql06-sqlite-reconfigure.vdbhz5k_` dans le worktree SQL. SHA-256 du reçu `cfad2903a16ef1da1172ef83173aa711d07f93e4ac0d3af4a1ad32aed07620d4`. Le rejeu sur le gateway courant et les deux profils MySQL restent **NOT_RUN**.

### Rejeu Docker du scénario TTY partagé

Le sélecteur Docker tourne sur le même libtest Darwin ARM64 que le rejeu Podman rootless. Le conducteur vérifie les 707 entrées produit, le fichier de test partagé et le même CLI. Il vérifie l’image QA et le socket local avant et après le passage.

**PASS** en 150,51 s. Un worker Docker sans réseau atteint son échéance d’exécution. L’arrêt acquitté par le moteur concorde avec l’inspection terminale, `worker-status` et l’audit rouvert. Le PTY est restauré et le quota libéré. Le conducteur confirme l’acquittement du retrait et un inventaire vide pour l’image QA au début et à la fin. Le délai du transport TTY reste `false`. Zéro appel fournisseur.

Rejeu et reçu dans `target/qa-evidence/docker-tty-shared-replay.2hrut8Cu`, passage `physical.up5c_laz`. SHA-256 du reçu `74a14e7037999e459c2e52ff0a70d7d2b427d41df0ed4fd6cf78b7314216d7f5`.

Le rejeu Chromium du même worker est **PASS**. Le conducteur scelle la base SQLite fermée et le checkpoint après la fin du test physique, relie leurs empreintes au journal de recette et ouvre uniquement une copie. Le CLI, le gateway Rust et le dashboard concordent sur la même session, le même worker et l’arrêt acquitté par Docker. L’événement observé `command.failed` est relu en séquence 18. Les empreintes de la base originale et du checkpoint restent identiques après lecture.

Conducteur et reçu dans `target/qa-evidence/docker-tty-shared-chromium.iZydnIdc`, passage `run.L5HUM0`. SHA-256 du reçu `d1f4b7656a96ed2a12e2840db7e2d65eaf3c8b03da5445459664a819ee97db0a`.

### Sonde stricte des projections d’identité MySQL

Le gel de 956 fichiers lie la bibliothèque, le libtest et `mysql_storage_probe` avec la même feature `mysql-storage`. Le sélecteur `identity::prepared_registry_probe_verifies_control_projections` passe avec compte `REQUIRE SSL` puis `REQUIRE X509`, sur deux cibles préparées distinctes.

Chaque profil vérifie le reçu historique du mode registre, le refus lorsque l’identité locale manque et le refus lorsque la projection workload manque. Après préparation des deux projections, la sonde stricte restitue la tête auditée et `control_projections_verified: true`. La comparaison indépendante des lignes et de l’état de la cible confirme l’absence d’altération lors des sondes. Les deux nettoyages passent et le port de recette est libre.

Rejeu et reçu dans `target/qa-evidence/mysql-strict-registry-libtest.gxfyfvhx` du worktree SQL, fichier `profiles-result.toazwvwa.json`. SHA-256 `5d8742225d7e92970b84aee304da8cedb2f8a25263fe0216347bb4a21e6728ac`. Le reçu porte sur la sonde de registre. SQL-06 via le gateway MySQL reste **NOT_RUN**.

### Registre SQL et redémarrage du gateway sur le gel strict

Le CLI Darwin ARM64 et le test complet `mysql_gateway_startup` sont liés avec la bibliothèque du gel de la sonde stricte. Le conducteur vérifie les 956 fichiers gelés, les dépendances et les trois sélecteurs. **3 PASS** en 4,92 s sur le vrai CLI `a963ed08…`. Le groupe de processus de test est vide à la sortie.

Le cas SQL-06 vérifie la lecture de l’ancienne cible SQLite, l’inaccessibilité de son ancien chemin et le changement de registre. Les nouvelles instances confirment l’ajout unique, le reçu complet identique après redémarrage, le refus du viewer et les lignes intactes de l’ancienne source conservée et du témoin B. Les deux autres cas confirment le refus de la sélection ambiguë et des profils MySQL avant lecture des credentials et avant toute modification SQLite.

Le reçu identifie le gel `89db3e3b…`. Les trois sources modifiées ensuite concernent les reçus et l’usage des workers MySQL. Conducteur et reçu dans `target/qa-evidence/sql06-gateway-current-link.YwcgclmQ`, passage `run.01acugm2`, dans le worktree SQL. SHA-256 du reçu `4f614209731677074fbbb51ff125e60a24de9b1d1f0512a95a1aedc6439d86cc`. Le démarrage et l’ajout sur un gateway MySQL restent **NOT_RUN**.

### Premier rejeu du broker submit Codex API synthétique

Le test candidat part du CLI Darwin ARM64 lié aux 707 sources produit attestées. Le conducteur relie l’intégration et vérifie le sélecteur unique `broker_submit_codex_api_commits_admission_before_runtime_mutation`. Le premier lien échoue sur un extern `rusqlite` absent du conducteur. Le second lien passe en réutilisant la dépendance déjà attestée dans le manifeste de build.

La recette soumet le profil Codex/API synthétique par le vrai `broker submit` et prévoit la lecture du statut avec le même CLI. L’environnement utilise un témoin runtime local. **FAIL** avec code 101 en 16,21 s. Le test remonte `broker CLI did not return success`. Ce message commun laisse indéterminée l’action CLI en cause. Le prochain rejeu doit la distinguer avant d’attribuer un défaut au produit. La fixture atteste le nettoyage et la fermeture de son IPC de gardien. Aucun appel fournisseur.

Les deux liens, le conducteur et le reçu physique sont dans `target/qa-evidence/g07-synthetic-submit-native.mLYwauxl`, passage `run.oktyg5go`. SHA-256 du reçu `6af7637c4762ca46896126e695fb9a8db63434cee67d9eb29e680c9c94112268`. Ce verdict concerne la recette synthétique du CLI et conserve séparément le FAIL du batch API réel.

La relecture du journal privé et de SQLite fermée confirme l’acceptation en révision 0, l’ACK `Allow` en révision 2, le reçu d’admission supervisée et le worker réservé en révision 4. En révision 5, l’intention `Prepare` est présente sans réponse persistée. SQLite atteste `control.supervision.transition` avec action `admission` en séquence 8 et aucun `command.started`. Les empreintes du reçu, du journal de test, du lien, de SQLite et des quatre révisions concordent avec `first-run-private-projection.json`, SHA-256 `b35ca2c42ed4c054c4c1084beaeaf2a91b76191e6cade662d8cefc219db6ba85`. Le résultat utilisateur du premier rejeu reste FAIL. Le conducteur suivant distingue les actions CLI et garde leurs réponses obligatoires pour un PASS.

### Compatibilité du contrat de contrôle V1

Quatre sélecteurs Rust de `control::supervision` passent sur le libtest Darwin ARM64 archivé. Le module et son interface `control.rs` correspondent octet pour octet à la candidate et au worktree SQL. Les assertions couvrent la plage fermée des schémas, le rôle et le namespace du contrat V1, le rejet des champs inconnus et les descriptions 14 et 15 sans activation d’autorité.

Le premier inventaire utilisait un nom de module de test générique et ne sélectionnait aucun cas. Le conducteur retient maintenant les quatre chemins Rust exacts. Rejeu hors réseau et reçu `run.hjo96qen/result.json` dans `target/qa-evidence/control-wire-v1-baseline.JfnYmV9P`. SHA-256 du reçu `328d48280a046af143cef0c6d84359732d8770a2a0749b95eed1712592260788`. Ce résultat porte sur les DTO et sert de référence pour une future négociation des capacités.

### Disponibilité Kubernetes Docker Desktop le 21 novembre

À 12 h 59 CET, Docker Desktop annonce Kubernetes kind 1.36.1 en `starting` avec `Waiting for node to be ready`. Le même processus local possède le socket de contrôle Desktop et l’écoute TCP du contexte `docker-desktop` sur `127.0.0.1:64604`. La connexion TCP aboutit puis se ferme à la négociation TLS, avant toute réponse HTTP. `kubectl --context=docker-desktop --request-timeout=7s get --raw=/readyz` renvoie EOF avec le contrôle TLS actif.

L’état Desktop est observable par `docker desktop kubernetes status --format json`. Le point de terminaison Kubernetes reste **BLOCKED**. L’inventaire CSI et l’exécution d’un worker n’ont pas été atteints dans ce relevé.

Les nouveaux relevés à 14 h 51 et 15 h 22 CET rendent encore `starting`. À 15 h 22, après vérification de l'origine HTTPS loopback du contexte, `kubectl --context=docker-desktop --request-timeout=5s get --raw=/readyz` s'arrête avec le code 1 et `EOF`. L'inventaire CSI et StorageClass reste `NOT_RUN`.

Le relevé de 16 h 04 donne le même état `starting` et la même fin TLS `EOF` sur l'origine loopback vérifiée.

### Usage des workers et reçus d’opération sur MySQL

Le gel `3a472f8d…` lie deux sélecteurs avec le même Store. `prepared_target_bootstrap_append_replay_and_capacity` et `supervised_mysql_reservation_is_atomic_and_replayable` passent sur les deux profils MySQL physiques, soit **4 PASS**. Chaque passage utilise une cible neuve et confirme le nettoyage.

Le cas V2 vérifie deux allocations distinctes non facturables, l’ordre et le curseur de pagination et le refus si une projection hors page diverge. Le reçu d’opération relu par UUID correspond au reçu d’origine après réouverture, y compris pour un claim suivi d’un état terminal. Le cas supervisé vérifie une admission acceptée et un refus de quota durable dont le reçu ne porte aucun worker ni révision. Les deux lectures utilisent un préfixe vérifié.

Le conducteur et le reçu sont sous `target/qa-evidence/mysql-worker-usage-receipts-libtest.yd67df9u` dans le worktree SQL. SHA-256 du reçu `worker-usage-receipts-profiles.omehbtm6.json` égal à `55b8f1209d8e010c0a7244712aff42a33f9a8fe8da9a2b964be5f35c793e1f56`. L’ouverture du gateway sur MySQL et ses routes d’usage et de reçus restent **NOT_RUN**.

### Historique des identités workload sur MySQL

Le 21 novembre, les sélecteurs `prepared_workload_binding_requires_verified_history` et `checked_credential_presence_retains_known_collisions_during_projection_failure` passent sur MySQL réel, avec mot de passe sur TLS puis avec mot de passe et certificat client mTLS. Les **4 passages** utilisent chacun deux schémas préparés séparés, avec lecteur `SELECT` seulement. Les quatre nettoyages passent.

Les lectures retrouvent le propriétaire des empreintes actuelles et historiques après réouverture, rotation et révocation inscrite dans la fixture. Le second cas vérifie qu'une projection devenue indisponible ne fait pas oublier les collisions déjà établies. La révocation émise par le Store Rust et le parcours passant par le gateway restent **NOT_RUN**.

Le conducteur reproductible `run-workload-owner-profiles.py` et `profiles-result.s2qcvh7g.json` sont sous `target/qa-evidence/mysql-workload-owner-libtest.65bc2bun` dans le worktree SQL. Gel `95394bdf8bd04deb08ebed3dab6699d4adac623cb17c812321ae7cea6dd2d59b`, SHA-256 du reçu `82828aba367b7e2b44cd5add6c4278ee6259bfdea921a5874cb3b82e103f265f`.

### Révocation d'une identité workload sur MySQL

Le Store Rust émet une révocation contre chacun des deux profils MySQL sur des schémas préparés. **2 PASS** et nettoyage confirmé dans les deux cas. Le scénario couvre le refus d'une révision obsolète et d'une projection divergente, le contrôle d'un champ masqué, l'utilisation du dernier créneau de la réserve d'identité et le rejeu du reçu exact après réouverture à capacité pleine. Il vérifie la conservation du préfixe avant l'écriture et l'absence d'effet sur la base voisine.

Le conducteur est `run-workload-revocation-profiles.py` dans `target/qa-evidence/mysql-workload-revocation-libtest.gyzokiqv` du worktree SQL. Gel `e06f31dca5c48145275ec52684c6bf4e391eb2532ff69e09d17122bde2b717ae`, reçu `profiles-result.fpqbhyrp.json` SHA-256 `b8cca675118ec65665af6a86b6dc445e16488174500882afea36bf4b0d87eaac`.

Le gel de rollback ajoute seulement une variante de la fixture. Le rejet SQL au moment d'insérer le reçu confirme le rollback après les premières écritures. Le journal, la tête, les liaisons, l'historique et les reçus restent identiques, puis une révocation distincte réussit et restitue son reçu au rejeu. **2 PASS**, nettoyage confirmé sur TLS et mTLS. Conducteur `run-workload-rollback-profiles.py` sous `target/qa-evidence/mysql-workload-rollback-libtest.c47nambt`, SHA-256 du reçu `profiles-result.1c8t9iqr.json` égal à `dd07905a60a3a2159dfa87afdd0c335ef3d750c1cbecfdc92a96342b946a578b`. Régression appliquée dans le worktree SQL après vérification de ses empreintes. L'exécution via la route du gateway reste **NOT_RUN**.

### Émission d'une identité worker sur MySQL

Le Store Rust émet et renouvelle une identité worker, et effectue sa rotation sur une cible MySQL préparée. La recette A/B vérifie le rejeu exact après réouverture, le maintien de l'historique et de la base voisine, les refus de tentative, de révision, de réemploi d'empreinte et de capacité. Sur le même gel, la régression de révocation et de rollback tardif repasse aussi. **4 PASS** sur les deux sélecteurs et les deux profils physiques, avec nettoyage confirmé à chaque passage.

Le conducteur `run-workload-issue-profiles.py` est sous `target/qa-evidence/mysql-workload-issue-libtest.we1pfdw_` dans le worktree SQL. Gel `9e95b9a565417de4b12c88be0b18d36132af85cf1e591182b2190c8863f88d04`, reçu `profiles-result.tvgs3uz0.json` SHA-256 `5aad38800493cc42d829092e6b6eb7c17c862f4b25885deb8b75449d69c4ccf3`.

Sur la même bibliothèque Rust, la recette du sujet Broker valide l'émission, le rejeu, les projections de liaison, les conflits d'incarnation et d'empreinte et le maintien de B. **2 PASS**, nettoyage confirmé sur TLS et mTLS. Le conducteur `run-workload-broker-issue-profiles.py` est sous `target/qa-evidence/mysql-workload-broker-issue-libtest.0rzlo04r`. Reçu `profiles-result.v_8syiz0.json` SHA-256 `f144ca1156d161843664ec68df51f337985bab9cb36b5b6baccc38b0147f6c06`. Régression appliquée dans le worktree SQL. L'appel est celui du Store par un contrôle de confiance. Le registre et les permissions du broker via le gateway MySQL restent à qualifier.

### Registre administratif des brokers sur cibles SQL préparées

Le 21 novembre, le Store lit le registre administratif dans la cible MySQL préparée. Le compte SQL de lecture vérifie le préfixe d'audit et les cinq projections du registre. La recette confirme l'ordre et la pagination, la réservation des autorités historiques, le refus d'une projection incomplète ou divergente et l'absence d'effet sur la base témoin. Sur une cible ancienne dont le registre est absent, la vérification d'audit reste valide et la lecture du registre demande sa préparation.

Le contrôle d'émission de l'identité Broker, avec un schéma de recette distinct, repasse sur la même bibliothèque. **Quatre passages MySQL PASS** avec deux schémas neufs par passage et nettoyage confirmé, en TLS mot de passe puis en mTLS avec mot de passe. **Deux tests SQLite PASS** pour le rendu partagé et le refus d'une divergence du journal ou de sa projection. L'ouverture et l'autorisation de la route gateway MySQL restent **NOT_RUN** pour SQL-06.

Gel `cc6b4c9d3b242be9842a91f8ffc26ec3c6076bfd0be0afd7b2a90f2faadf31ab` sous `target/qa-evidence/mysql-broker-registry-admin-libtest.gowlmf9p` du worktree SQL. Depuis ce dossier, lancer `python3 -IB link-libtests.py --link`, `python3 -IB link-libtests.py --run-sqlite` puis `python3 -IB run-broker-registry-profiles.py --run --port 13306`. Reçu SQLite `sqlite-result.wqanuhrr.json` SHA-256 `b46e989752900ce74844700533deea3337b028349ebb0696533ed4cd47e2db3e` et reçu MySQL `profiles-result.449korx_.json` SHA-256 `f3a4ec011df117e2b662cfc68f8e84a4020b9da2345db9006648332a5a4c64cb`.

### Santé SQLite après observation d'un schéma inconnu

La sonde de santé affiche l'état mémorisé de l'audit. Le contrat prévoit le refus de l'opération dès qu'une validation constate un schéma inconnu. Le delta local distingue ce cas d'un schéma 14 connu dont l'accès demande une autre autorité. Les trois sélecteurs Rust de validation incrémentale et de refus d'autorité passent dans le libtest natif dont les 729 entrées correspondent au gel et aux sources courantes. Dossier `target/qa-evidence/g03-g06-workspace-store-native.yntvpcfm`, reçu SHA-256 `22720260723dd5cd80797abf70252e1cbcb8a34074b6dac4548555ac0c6b9a62`. Le code HTTP `503` après la validation a son sélecteur d'intégration séparé.

Le premier lien du sélecteur HTTP omettait l'extern `hex` de la crate de test. La reprise avec l'archive attestée exécute `health_reports_an_observed_unsupported_audit_schema` et confirme `200` avant observation puis `503`. Reçu `p5-tenant-api-hex-retry.u0pp2udv/replay.wdpg2n8x/result.json` SHA-256 `5f81db477e2e1600637deec134e627d98fc9ca2370cf4daa2c6ac1e26d145b07`. Le patch sélectif `p5-audit-index-patch.rv0hhqxx/p5-audit-schema-latch.patch` est transmis pour intégration.

### Défaut de statut du broker sur un checkpoint V3

Le second rejeu synthétique atteint l'admission et rend **FAIL** en 17,91 s avec `broker status exit_code=1 category=broker_error code=NeedsReconciliation`. Le nettoyage est confirmé, l'IPC du gardien est absent et aucun fournisseur n'est appelé. Le reçu `run.q88ywg_j/result.json` a le SHA-256 `8cbd0c4da7e718a43b17cdcae1b69d81d88339ac0d8df39e194dfc8f92e96a80`.

Le dernier message d'audit de retraite en attente précède de 2,997 s le commit de l'intention `Prepare`. Le journal s'arrête sur cette intention sans réponse ni `FailureObserved`. La lecture de retraite exigeait un checkpoint V5 et renvoyait un conflit sur le V3 valide, ce qui fermait la projection de statut avant le 503 injecté. Projection et empreintes dans `target/qa-evidence/g07-synthetic-submit-native.mLYwauxl/second-run-private-projection.json`, SHA-256 `d97b8b3cafae05451ec4e1b73f92b12ce3da595e5cc2354940d502337f456ad6`.

Le correctif traite l'absence de file de retraite sur un checkpoint ancien valide. Les régressions vérifient la lecture du statut dans ce cas et le refus sur conflit réel. Le formatage et la vérification du patch passent. Le lien du CLI corrigé et ses tests restent **NOT_RUN**.

### Route de contrôle V1 avec SQLite

Le sélecteur `served_control_protocol_versions_match_schema_enum_and_active_constraint` passe en 2,75 s sur le gel strict. Il exerce le routeur Axum en mémoire sur des bases SQLite réelles de recette. Il vérifie les schémas actifs et inactifs puis le vrai CLI sur une copie de recette pour la migration de schéma 8→9. Le réseau est interdit pendant le lien et l'essai.

Le conducteur réutilise le CLI et la bibliothèque attestés et vérifie les 35 dépendances. Son dossier est `target/qa-evidence/sqlite-control-protocol-v1.7q97BS6Z` du worktree SQL. SHA-256 du reçu `run.k2ukjg3f/result.json` égal à `0f7abbd04babe2f6a5b9694246dfe4735a5904d2d8a267f9cb3b9134ee93e073`. Le résultat porte sur la route V1 et SQLite.

### Rejeu synthétique après correction du statut V3

Le gel du correctif V3 lie la bibliothèque, le CLI et la recette à partir de 707 entrées produit et des dépendances Darwin ARM64 vérifiées. Le test `broker_submit_codex_api_commits_admission_before_runtime_mutation` passe en 35,91 s. Le vrai `broker submit` accepte la demande, puis `broker status` rend l'état attendu après admission. Le test rapproche décision, reçu, worker réservé et archive du workspace avec SQLite. La fixture établit le refus `Prepare` et confirme l'absence de mutation runtime et la fermeture de l'IPC du gardien.

Le reçu `run.81bjk6ok/result.json` se trouve sous `target/qa-evidence/g07-retirement-v3-native.1DjBe0ac` dans la candidate. SHA-256 `0c6b893d49ed52da0584293539be77ea5b048cbca4829e7669d59c51c9ccf921`. **PASS** pour le parcours synthétique, nettoyage PASS, zéro appel fournisseur.

Le 21 novembre à 15 h 07 CET, les deux sélecteurs Rust du writer passent sur le gel de 707 sources sans écart avec les sources courantes. `retirement_audit_on_a_v3_job_keeps_its_last_confirmed_status` vérifie le maintien du dernier statut validé, et `retirement_audit_conflict_for_a_missing_observed_job_closes_the_status_projection` vérifie le refus lorsqu'un vrai conflit subsiste. Dossier `target/qa-evidence/g07-writer-libtest.6XnbKg99`, reçu `run.f50289rk/result.json` SHA-256 `81ef92f91896eb2a72a8f8930ee11a2918f98a44df997d5bd5d238d814da6ac9`.

### Prévols du batch Codex API et de Chromium

Les reçus sont sous `target/qa-evidence/g07-codex-api-one-shot-current.ozYyBFKD` dans la candidate. Le premier essai `run.woxipujf/result.json` s'arrête au prévol navigateur avec zéro invocation du conducteur fournisseur. Le HTML et la capture Chromium sont référencés dans le reçu. Le HTML montre un HTTP 429 pendant l'ouverture de la vue d'ensemble. Le code de la requête de la sonde Workers reste inconnu pour cet essai.

Le diagnostic reproductible `probe-worker-registry.py --browser` vérifie le vrai gateway et son proxy dashboard. Avec la sonde Workers exécutée avant les lectures de l'écran, Chromium passe, l'origine de l'identité est vérifiée et le nettoyage passe. Reçu `worker-registry-browser-loopback.zpp0k0jo.json`, SHA-256 `b26a4b7e632b31ec9307c09f4429343a7f1ba4a48f8a45f1cebd9b882a9abf29`. L'alerte de saturation du produit reste suivie.

Le deuxième essai atteint le conducteur mais s'arrête dans `provider_preflight` sur `diagnostic_reference_not_verified`, avant toute soumission de job ou appel modèle. Le broker se ferme, le gardien est absent et aucun worker du profil ne reste actif. Le reçu `run.1bpzr17r/result.json` a le SHA-256 `26acda7e728182e32db0ffe8b541a38a0a1a041192a7d54f2aca41d5790cb033`. Le rejeu de métadonnées `probe-provider-reference.py` confirme une référence présente et régulière avec `private_rule=not_evaluated` et `content=not_read`. L'essai batch fournisseur reste **BLOCKED** au prévol.

### Batch Codex API avec référence dédiée

Dans `target/qa-evidence/g07-codex-selected-reference.FsZuDpkl`, la fixture hors ligne valide la sélection d'une seule variable, le diagnostic de métadonnées, la suppression du fichier privé et la reprise d'un arrêt avant disponibilité. Le reçu `projection-synthetic.json` a le SHA-256 `e57913cdbb75b41d062abf05820df80488d340a71d4157a002af58b99da04ad4`.

Le conducteur réel passe le prévol et le broker accepte le job. Le journal confirme Allow, admission, Prepare, Arm et claim du helper d'import. Le helper sort à 0 sans OOM. Le CLI échoue sur la lecture du résultat avant l'ACK de fin d'import. Ensuite, le journal s'arrête sur `helper_cleanup` en attente d'ACK. SQLite conserve le worker métier `Preparing` et son quota. Le parcours s'arrête avant `Create` et `Start` du worker Codex. Le nombre de requêtes fournisseur n'est pas attesté.

Le batch est **FAIL**. Le reçu `run.w38mhkl6/result.json` a le SHA-256 `7a8ebf7d277e39996e8ebeb2d751c693f0625d7612919701229aece02c9a320a`. Le fichier temporaire de credential et le processus broker ont disparu. L'IPC du gardien reste joignable après le refus de fermeture. Le gel et les preuves sont conservés pour distinguer la réponse typée à `broker result` pendant l'import de l'ACK de nettoyage.

Le test séparé `broker_import_result_pending_then_acknowledged` est lié contre le même CLI et la même bibliothèque. Son premier rejeu local s'arrête sur `phase=fixture_start` avant l'envoi du job. La fixture a compilé son témoin et migré ses bases, puis a tenté d'ouvrir par le chemin de reprise un nouveau répertoire de gardien sans verrou de propriétaire. Le contrôle IPC après l'essai ne trouve que la socket du témoin déjà fermée. Le reçu `g07-import-result-gate.7mk0VhPO/run.sf5hf320/result.json` a le SHA-256 `50dd348e2923795209c8e1cb5bd59bf67940ba16a9a8abeccfac5984c53a0850`. La lecture pendant l'import et ses ACK restent à vérifier avec le chemin de création normale de la fixture.

### Fermeture d'un descendant de recette sous Darwin

Trois sondes privées vérifient la fermeture d'un enfant direct, d'un descendant lancé dans sa propre session, puis du même type de descendant sous Seatbelt. Le dernier essai associe le pair de la socket au seul enfant du superviseur détenu, envoie le signal sur l'identité attestée par le noyau, observe `NOTE_EXIT`, reçoit la confirmation de récolte et vérifie la disparition du listener. Le noyau refuse ensuite le jeton périmé. Les scripts arrêtent uniquement les enfants qu'ils ont créés.

Conducteur `probe-detached-kqueue.py` sous `target/qa-evidence/g07-darwin-owned-process-probe.jWNutGN3`, reçu `detached.703ls5sa.json` SHA-256 `ee24abc837c234452a2ec6a21c2c6ddb6207558e987d63483d55369548e2300f`. La preuve porte sur les primitives locales de la fixture. Les phases d'import et la fermeture du gardien Full3 sur le vrai broker restent à vérifier.

La variante Full3 Fresh distingue les ACK métier de la fermeture de sa fixture et lie uniquement le test d'intégration contre la bibliothèque attestée. Son premier lien est **BLOCKED** avant exécution. Le dossier privé omet `broker_intent-plan-v3.json` et l'extern `serde` dans les entrées de `rustc`. Dossier `target/qa-evidence/g07-import-full3-fresh-verdicts.SpaTkruE`, reçu `link.9kep1i2c/result.json` SHA-256 `b94e6512036883c70612ae4044bc5abc82d1b15422cbc26d94bda42af94d4457`.

Le deuxième gel lie le test avec le JSON exact de `HEAD` et l'extern `serde` vérifié. Le vrai `broker submit` accepte le job. Le journal confirme décision, admission et `Prepare`, puis `FailureObserved(Import, Unavailable)` avant `Arm` et `Claim`. Le reçu marque **FAIL** sur le délai d'attente de l'import. Le moteur de recette est joint, le gardien est récolté et leurs deux IPC sont absents. Le résultat agrégé de fermeture reste `UNCONFIRMED`. Ce scénario ne produit aucune preuve de lecture pendant l'import ni de fermeture métier du broker. Conducteur `replay.py` sous `target/qa-evidence/g07-import-full3-fresh-link-inputs.zd3AKpuE`, reçu `run.5d8ed54g/result.json` SHA-256 `0abce7f39575965ab20d1b795a55769a46da0adf9fa1ace688df61bc193b2ac9`.

La revue du témoin constate que sa grammaire rejette `--pull=never`, option émise par le constructeur `ContainerSpec::create_args`. Le reçu ne comporte pas le compteur d'arrivée de `create`. La correction de recette doit exercer le constructeur réel, distinguer le statut du témoin de la récolte du broker et de la fermeture du gateway, puis rejouer l'import.

Le nouveau contrôle de grammaire passe sur le constructeur réel et le témoin accepte l'import attendu. Pendant l'attente contrôlée, le vrai CLI renvoie `broker status=Preparing` et `broker result=Pending`, tous deux code 0. Le stockage confirme la réserve de quota et un seul helper d'import. Après libération, le témoin enregistre sa fin terminale et zéro erreur. Le scénario complet est **FAIL** au délai d'attente de vérification. La chaîne durable valide jusqu'à la révision 14 contient l'ACK `EmitOnce` du claim à la révision 13, puis `FailureObserved(Import, Unavailable)` sans demande d'observation. L'observation et le retrait ACK restent à prouver. La fermeture de la fixture est **PASS**, avec moteur joint, gardien et broker récoltés, IPC absents et gateway joint.

Conducteur `replay.py` sous `target/qa-evidence/g07-import-full3-fresh-pull-policy.HYhetlnA`, reçu `run.2r47rmmy/result.json` SHA-256 `9a4f39ead146deaff9ec726c33e5dfe6c83c035bf11865a8a4ccc7e316d0833c`. Le test emploie un moteur synthétique Full3 Fresh et zéro fournisseur. Le batch réel Legacy conserve son verdict et sa preuve distincts.

Le sélecteur Rust `pending_started_callback_is_not_reinvoked_when_attach_reaches_eof` passe en 1,209 s sur le même rlib produit, avec le runtime synthétique dédié au transport et réseau refusé. Il vérifie que l'EOF de l'attach ne provoque pas une seconde invocation du callback de démarrage en attente. `prearmed-eof-transport.a6b794f9cf/result.json` SHA-256 `13dbc468eae264c160f5bd652f5ee4866a1eefe859136733c20cedaf3d3dcb24`. La prochaine variante rapproche ce test de l'archive et du témoin de la recette Full3.

La variante Import isolée passe avec la même archive et le même témoin. Elle confirme EOF, sortie de l'attach, inspection après cette sortie et terminal validé par Rust, avec fermeture du groupe de recette. Rejeu `prearmed-import-transport.4hLmEn1b/run.be51l8ym/result.json` SHA-256 `a55420a6e505c5c6ff8f72bfdb3131cff6db8e06de45e865b24acb737991cf38`. Les ACK et le profil broker géré conservent le verdict du scénario complet.

### Workspace et reprise d'export sur le gel courant

Le 21 novembre, le libtest natif du gel G07 confirme l'exclusion du répertoire de contrôle dans le manifeste et l'archive, la préservation du mode de l'original, le mode importé et un export neuf sans bit d'exécution. Une sonde démontre que le profil Seatbelt précédent transformait `04755` en `0755` lors de la préparation du fichier, avant ai-guard. Le test de mode passe ensuite sur le filesystem natif avec le même binaire.

Deux autres sélecteurs passent sur le vrai journal et les fichiers de recette. Après une réponse d'audit perdue, le même export et son reçu sont retrouvés à la réouverture. Si le journal ne peut écrire au moment de la publication, le dossier et l'original persistent et la republication au même nom est refusée. La sortie libtest comporte un avertissement d'export intercalé avant `ok`. Les reçus vérifiés à partir des logs corrigent le classement initial du conducteur et conservent ses preuves brutes.

Reçus `workspace-mode-environment.ro3rr0q8/result.json` SHA-256 `93232d57bf31845a9fccd8a0f78114f221fb0e0a0d22e6cbac9e4897df6a71cd` et `g03-native-mode.0c4ncdri/verified-result.json` SHA-256 `e8f4bfe8ad3509f28ee29772cc795207b2d8005d6352b13bc964c829dc15fccf`. Le binaire et les sélecteurs complets y sont identifiés. Les parcours d'export ou de reprise sur un moteur conteneurisé ont leurs propres preuves.

### Moteur Rust déterministe sur le gel courant

Le 21 novembre, le conducteur `tests/e2e/run_browser_decision.py --judge-mode rust` passe avec le CLI du gel G07 corrigé, le bundle G08 attesté et les listeners locaux du gateway et du dashboard. L'oracle vérifie la décision `allow/exact_allowlist`, le refus `deny/strict_default_deny`, l'intégrité de trois événements SQLite relus par le CLI et le téléchargement JSONL relu par Chromium. La décision du tenant B reste absente du tenant A. Zéro worker et zéro modèle.

Reçu `decision/result.json` sous `target/qa-evidence/g07-rust-current.2jrxTHhL`, SHA-256 `a58f9c0b58f2faae3de6823cbbe3950c430bf5e926e6181378aab2b1935534f7`. Le manifeste `inputs.sha256` identifie le CLI, le bundle, le conducteur, le helper, l'oracle Chromium, la politique et le profil boucle locale. Son SHA-256 est `e55c309b9e615b666e769d8b9340fa96381cc2ca4aa1098100513e3647b55cde`. Les reçus Jev et Qwen restent liés à leurs propres profils et empreintes.

### Reprise du journal viewer après un HTTP 429

Le rejeu ciblé Chromium se connecte à la fixture locale comme viewer, provoque un seul refus HTTP 429 du journal, puis exerce « Actualiser ». Sur la préimage, l'événement 14 revient et le bandeau HTTP 429 reste affiché. Le scénario rouge est confirmé avec le HTML et une capture. Avec le correctif, la même action rétablit le journal et retire le bandeau. Compilation TypeScript, build Vite et scénario Chromium passent. Ce résultat porte sur le comportement de l'interface avec la fixture.

Le conducteur `replay-focused.py` et le reçu `focused-replay.a94mm8fj/result.json` se trouvent sous `target/qa-evidence/g08-viewer-journal-429.2CEfDOfU` dans la candidate. SHA-256 du reçu `9fc07bf01d5907c65290bc221ff19f0fd95aa2cadef1a9bb225c97e48a365b12`. Le correctif et sa régression ont été appliqués avec vérification des préimages et des résultats. Le premier échec du harnais général est conservé sous `replay.05o9tdqs/result.json`. Il concerne des requêtes de version publiques précédant la connexion. La suite navigateur générale reste à qualifier.

Le rejeu `replay-general-current.py` réutilise le front du gel vert et ajoute au test les deux GET publics de version sans autorisation dans l'assertion de préconnexion. Le bloc principal passe, y compris la régression du journal. Le test d'identité échoue ensuite dans l'attente des lectures simultanées retenues par la fixture, avant la bascule A→B. Le verdict global reste **FAIL**. Le patch limité à la règle de préconnexion est appliqué. Le reçu `general-current.850nxcr4/result.json` a le SHA-256 `6f1337c9da471a49d6421e5ba96b54a237f2342d6828c7d647b8cf912ae713c6`.

### Bascule viewer avec réponses tardives sous Chromium

Le 21 novembre, le test d'identité répartit les lectures retenues en groupes compatibles avec les trois permis du client. Il repasse par la connexion B avant chaque libération, avec le journal de B indisponible pour les réponses positives. Le test atteste la remise de douze réponses ou événements A, puis de deux refus 403 A après connexion de B. L'interface ne réaffiche pas les données A aux points vérifiés, l'export tardif ne télécharge aucun fichier et les deux refus ne ferment pas B. Le scénario couvre aussi l'état d'intégrité inconnue, les quotas inconnus et la conservation de la dernière lecture de B lors de sa propre panne.

Le test d'identité seul et le point d'entrée navigateur complet passent sur le même bundle que le rouge, sans changement du code produit. Les 34 entrées produit et build concordent avec la candidate. Le test mis à jour est appliqué dans celle-ci. Conducteur `replay-identity.py --general` sous `target/qa-evidence/g08-viewer-identity-waves.ZVpLcyHo`. Reçu final `replay.5byowyae/result.json` SHA-256 `e4517e4eda9a7ec585ecae7f2101f2d8264b1adf7d6b5ed623f83939bf749ba6`. Les vérifications de bascule utilisent une fixture HTTP locale.

À 15 h 50 CET, le correctif 429, sa régression et les deux patches de test navigateur sont appliqués à l'index après vérification des reçus et simulation de l'index. Le reçu `target/qa-evidence/g08-index-verified.lv5kogq5/result.json`, SHA-256 `c40574d219d667c9beb204634def9cc8ad4c306471e37226541138ed5cb792eb`, atteste les trois chemins concernés et les deltas du worktree qui restent à intégrer.

La suite Node de ce checkout réussit 247 cas avec huit ignorés dans le profil standard. Elle réussit ensuite **255 cas**, sans échec ni ignoré, lorsque les seize réponses capturées sur des backends Rust archivés sont fournies. Sortie `unit-with-archived-wire.GdNlUKuo/node-tests.stdout` SHA-256 `ec1b298cea92990ed6026be9d174e324ea3561c653f3e26743874e1c56eb5684`. Le rejeu des huit cas filaires porte sur les réponses archivées vérifiées par empreinte.

### Contrats HTTP des registres workers sur la candidate

Le 21 novembre à 15 h 12 CET, cinq sélecteurs Rust et quatre contrôles Node passent sur six réponses fraîchement capturées, sans cas ignoré. Le groupe fédéré vérifie les vues par tenant et la visibilité du registre historique, y compris la limite de migration des anciens workers. Le groupe nominatif vérifie la session nil du registre historique, sa date runtime inconnue et le passage d'un code de sortie entier sans perte de précision. Les tests utilisent les routes Rust et SQLite de recette sur la boucle locale.

Le conducteur `replay.py` est sous `target/qa-evidence/g08-workers-wire-current.bgdTyGDa` dans la candidate. Il vérifie les 707 entrées du gel G07 corrigé et dix entrées des tests et parseurs. Exécuter `python3 -IB replay.py --run federation_workers_api` puis `python3 -IB replay.py --run human_workers_api`. Reçus `federation_workers_api.eysmq9ai/result.json` SHA-256 `c68f102c9d42bc4d889b2d6f057826b3550a16e0ec80a874cfb6d7131c83200c` et `human_workers_api.4pcsx73i/result.json` SHA-256 `9d7de50dc9a1ea3736a4e4f016ad59551590366cf56ed9c0b40fe567fd273a24`. Le navigateur et le démarrage de workers physiques ont des preuves distinctes.

### Export nominatif sur Chromium et SQLite

Le 21 novembre à 15 h 18 CET, le profil `human-export` passe sous Chromium 148. Le backend Rust utilise SQLite et 25 fiches synthétiques dans deux tenants. Le lecteur autorisé exporte 39 événements en deux pages. Le test retrouve les deux reçus dans SQLite et compare le téléchargement au flux HTTP. Le lecteur de métadonnées reçoit HTTP 403 quand il demande l'export. La fixture démarrée et le groupe navigateur se ferment. Zéro worker physique et zéro appel fournisseur.

Le premier lien s'arrête sur `chrono` absent de la commande de recette. Le reçu est conservé sous `target/qa-evidence/g08-human-export-current.eh6MbbsL/link.yvkp0e0r/result.json`. Le second conducteur `replay.py` réutilise le même rlib et ajoute l'extern de la même provenance. Dossier `target/qa-evidence/g08-human-export-chrono.beKexPJQ`, reçu `run.uqllyg_3/result.json` SHA-256 `cf968a96f70935dc362bde2f64968c83decf9add179864f8962c387c942220f9`. Les commandes `--link` puis `--run` et l'empreinte du bundle figurent dans le guide du dossier.

### Révocation et droits partiels sur Chromium et SQLite

Le 21 novembre, `revoked` et `partial` passent avec le même binaire et le même bundle que l'export nominatif. `revoked` vérifie le 403 lors du cycle de lecture, le retrait des données et du détail, la fermeture du journal en direct et la nouvelle connexion après révocation du credential. `partial` vérifie les trois profils de lecture avec des requêtes limitées aux droits effectifs et collecte 18 captures aux largeurs 320, 390 et 1 440 px en clair et sombre. Le scénario `partial` inclut une garde de requêtes mono-origine. Le profil d'exécution borne les deux scénarios à la boucle locale. Fermeture confirmée, zéro worker physique et zéro fournisseur.

Conducteur `replay.py` sous `target/qa-evidence/g08-human-revoked-partial-current.03wcYYla`, commandes `python3 -IB replay.py --run revoked` et `python3 -IB replay.py --run partial`. Reçus `revoked.jj8ppewu/result.json` SHA-256 `61808887db0ada0613620afef8d758e7c87fd101d1bc7f1a831678e239cc007a` et `partial.m31rb_i6/result.json` SHA-256 `0b4062ad008de55842254d6cfaeab4189bc046ba7cba7b199741472100662602`.

### Contrôle des compagnons dans les archives de release

Les métadonnées Cargo de la candidate identifient six binaires natifs. Sur fixture locale, la garde précédente accepte deux archives privées d'un compagnon malgré une somme valide. Le job de vérification transmet désormais la liste Cargo au build et au job de brouillon. Celui-ci exige une entrée régulière, unique, non vide et exécutable pour chaque binaire attendu dans chacune des archives natives.

La fixture locale confirme le refus des sept formes d'archive invalide avant tout appel au simulateur GitHub. La création du brouillon et sa reprise figurent parmi les **36 scénarios** passés. Le patch du workflow et de la checklist est appliqué dans la candidate. Recette sous `target/qa-evidence/g12-native-companion-content.Zdz8vqlq`, rejeu indépendant `g12-native-companion-replay.XyEVmOwN/result.json` SHA-256 `7911629a509b55f091dfb91054cd822d8abc355c5a3e7cde42ff74c118eeea9d`. Le workflow hébergé et les binaires distribués restent **NOT_RUN**.

Le prévol local macOS ARM64 retrouve les six cibles dans les métadonnées Cargo. Le reçu `native-broker-and-podman-build.daaTk4k6/cargo.jsonl` identifie les binaires présents comme profil de développement. La recette de distribution attend un build des six binaires du gel intégré avec `cargo build --release --locked --offline --bins --target aarch64-apple-darwin`, la version et la révision de ce gel, puis la validation de l'archive extraite dans un dossier privé neuf. Aucun build release n'a été lancé pendant ce prévol.

La revue compare les deux diffs locaux aux préimages et postimages du patch testé. Le workflow et la checklist sont indexés aux blobs `3167588bc0b0f6f9ea685ab05823290509e035f1` et `6e16281718b7d8b85c9ef6dfb0c977e658ee9892`. Les autres entrées de l'index sont identiques avant et après cette mise à jour.

Le rejeu rouge confirme que la version précédente accepte le mauvais `LICENSE` avec une somme d'archive cohérente. Le workflow corrigé calcule le SHA du fichier du commit vérifié, accepte la création et la reprise avec les contenus exacts, puis refuse séparément la substitution dans l'archive Linux, macOS et frontend avant tout appel au service simulé. Rejeu `target/qa-evidence/g12-license-exact.nJmUUB0t/replay.ntr5anoz/result.json` SHA-256 `e0d3a5dca167525e692f565b0539b0dc02872c7640d9224dfd5375739f0ddae2`. Le workflow et la checklist de ce rejeu sont indexés, reçu `g12-license-index.wea9cu5r/result.json` SHA-256 `9be5a259596000f0d5a7042ac4395170a7f0a3d2d7a6feea1e2ca262f1ecadf7`.

Le workflow de recette empaquette le bundle attesté avec l'attribution du paquet Vite verrouillé, conservée octet pour octet. Le job de brouillon accepte cette archive et refuse l'ancienne archive sans le membre avant tout appel simulé. Reçu `g12-vite-attribution.dEe7DIuE/replay.2qoobwtd/result.json` SHA-256 `312685b150ad36664c92627a1d7bfac8bb2c2470f1ed67a2beaaee7bc0f9cfc8`. Le delta et la checklist sont indexés, reçu `g12-vite-index.3guwfoml/result.json` SHA-256 `995c305995ae6e91d310d7edf0a9dbe66269c9b0b2aae362bb061987913de2a7`.
