# Points de contrôle signés de l'audit

[English](README.md) · [Français](README.fr.md) · [Index d'installation](../README.fr.md)

Ce guide décrit la commande d'administration hors ligne `ai-guard-audit-checkpoint` : elle signe la tête de la chaîne d'audit d'un tenant et permet à un tiers de la vérifier plus tard avec une clé qu'il a lui-même approuvée. Les documents exacts qu'elle lit et écrit sont spécifiés dans la [référence des formats](FORMATS.fr.md). L'implémentation et ses preuves sont suivies dans [le handoff d'ingénierie](../../engineering/evidence/2026-11-10/audit-signatures/README.md) ; ce guide est un contrat, pas une qualification.

## Déploiements couverts par ce guide

| Déploiement | Ce que ce guide livre | Preuve exécutée |
|---|---|---|
| Compte de contrôle travaillant sur des chemins de l'hôte : base d'audit, clé de signature et enregistrements dans des répertoires privés de ce compte | Toutes les commandes ci-dessous | QA Linux ARM64 isolée uniquement ; voir le handoff |
| Poste d'auditeur ne détenant qu'une ancre de confiance et un bundle de preuve | `verify`, `anchor advance`, `anchor approve-rotation`, `diagnose` | QA Linux ARM64 isolée uniquement |
| Installation Compose dont la base d'audit vit dans le volume `audit_data` | **Pas encore desservie.** Un CLI de l'hôte ne peut pas ouvrir ce volume. La définition de l'image de contrôle emballe désormais ce binaire et la [section 12](#12-installation-compose--conteneur-ponctuel) donne la procédure par conteneur ponctuel, mais ni la construction de l'image ni cette procédure n'ont jamais été exécutées. Voir l'écart ouvert du handoff. | `NOT_RUN` |
| macOS natif | Mêmes commandes ; la politique ACL et de système de fichiers Darwin des répertoires privés n'a pas été exécutée | `NOT_RUN` |
| Kubernetes | Non desservi | `NOT_RUN` |

Rien n'est installé comme service, aucun réseau n'est contacté et aucun composant de signature externe n'est requis.

## Ce qu'un point de contrôle signé prouve

La chaîne d'audit d'un tenant est une chaîne SHA-256 sans clé, vérifiée depuis la base elle-même. `agentsandbox verify-audit` sans options de checkpoint et `GET /api/v1/integrity` détectent donc une altération accidentelle ou partielle, mais pas une réécriture cohérente par quelqu'un qui peut écrire le fichier SQLite : une telle réécriture recalcule les empreintes publiques et est annoncée intègre.

Un point de contrôle signé est une signature Ed25519 d'une déclaration qui lie :

- le tenant, le namespace de contrôle réel de la base (`namespace_uuid`, créé avec l'événement genesis de contrôle) et cet événement genesis de contrôle lui-même, par sa séquence et son empreinte de chaîne ;
- la version du contrat de déclaration et la version du schéma d'audit ;
- une position : le nombre d'événements et l'empreinte du dernier ;
- l'empreinte de la clé de signature ;
- l'enregistrement précédent du journal des points de contrôle et la position signée précédente.

La vérification avec une ancre de confiance tenue de façon indépendante établit alors que **la clé approuvée a signé cette tête de chaîne à cette position du journal** et, lorsqu'une base est comparée, qu'elle contient toujours chaque position signée à laquelle l'ancre fait confiance.

Elle n'établit **pas** :

- qu'un événement soumis par un runner est vrai, ni qu'un effet runtime a eu lieu ;
- quoi que ce soit sur les événements ajoutés après le dernier point de contrôle, jusqu'au suivant ;
- quoi que ce soit sur l'historique antérieur au premier point de contrôle : le premier point de contrôle d'un audit existant est une **ligne de base**, sans protection rétroactive ;
- une heure de confiance : `created_at` est déclarée par le signataire ;
- quoi que ce soit lorsque le même administrateur contrôle la base, la clé de signature et l'ancre de confiance.

Ce mécanisme est sans rapport avec les checkpoints métier des brokers et avec le registre d'authentification OIDC.

## Rôles et conditions de confiance

| Rôle | Détient | Ne doit pas détenir |
|---|---|---|
| Opérateur du contrôle | Base d'audit, clé privée de signature, enregistrements | L'ancre de confiance, si l'indépendance est recherchée |
| Détenteur de l'ancre (auditeur, second administrateur, support hors ligne) | Ancre de confiance : les clés publiques approuvées et la dernière référence acceptée | La clé privée, l'écriture sur la base |

- **Protection de la clé.** La clé privée est un fichier PKCS#8 en mode `0600` dans un répertoire en mode `0700`, tous deux possédés par le compte de contrôle. Elle n'est protégée que par les permissions du système de fichiers : ni phrase secrète, ni module matériel, ni effacement mémoire garanti. La politique des répertoires privés impose le même compte pour la base et pour la clé ; la clé ne peut donc pas être cachée au compte de contrôle lui-même : face à ce compte ou à `root`, la signature n'apporte rien et seule l'ancre de confiance compte.
- **Indépendance de l'ancre.** L'ancre est un document public. Sa valeur vient de son emplacement : là où le compte de contrôle et l'administrateur du stockage ne peuvent pas la réécrire — autre machine, autre administrateur, ou fichier possédé par `root` et seulement lisible par le compte de contrôle. La commande accepte une ancre possédée par `root` ou par le compte courant.
- **Binaire de vérification.** Vérifier avec un binaire de confiance, sur une machine de confiance.
- **Stockage pris en charge.** Les répertoires privés et les fichiers d'entrée doivent se trouver sur un système de fichiers local pris en charge (ext4, XFS, Btrfs, tmpfs, overlayfs, F2FS sous Linux ; APFS/HFS avec propriétaires activés sous macOS). FAT/exFAT, montages réseau et FUSE sont refusés : copier l'ancre sur un disque local avant usage.

## Prérequis

- Le binaire, construit avec la toolchain du projet : `cargo build --release --locked` produit `target/release/ai-guard-audit-checkpoint`.
- Une base d'audit au schéma 2 ou supérieur, dans un répertoire privé (`0700`) du compte de contrôle, avec un nom de fichier ASCII et le mode `0600`. Une base au schéma 1 est refusée avec `audit_namespace_unavailable` ; exécuter d'abord la maintenance explicite `agentsandbox migrate-control --target-version 2`.
- Un répertoire de points de contrôle et un répertoire de clé qui ne sont **pas** le répertoire de la base. Le répertoire de clé n'est jamais le répertoire des points de contrôle ni un répertoire situé dessous : le répertoire des points de contrôle est une preuve que l'on copie, exporte et sauvegarde, et la clé privée ne doit pas voyager avec lui.

Les exemples emploient trois emplacements ; `key init` et `create --new-log` créent les répertoires manquants en privé (`0700`) :

```sh
AUDIT_DB=/path/to/control/audit.sqlite   # le fichier --data du gateway
SIGNING=/path/to/audit-signing           # zone privée du compte de contrôle, hors du répertoire de la base
TRANSFER=/path/to/handover               # fichiers publics préparés pour le détenteur de l'ancre
```

La base est ouverte en lecture seule sur un instantané cohérent et n'est jamais réécrite. Un gateway peut rester en service : la commande ne conserve pas le verrou d'initialisation de la base pendant son parcours et ne bloque pas les écrivains. La lecture peut créer des fichiers `-wal`/`-shm` vides et le fichier réservé `.ai-guard-sqlite-open.lock` à côté de la base.

Chaque commande écrit exactement un document JSON sur la sortie standard ; seul `--help` affiche un texte d'usage. Le code de sortie `0` est un succès, `2` un refus d'intégrité, de confiance ou de périmètre, `1` une erreur opérationnelle (y compris une ligne de commande invalide). Tout code non nul signifie que rien n'a été vérifié.

## 1. Créer la clé de signature

```sh
ai-guard-audit-checkpoint key init --key-dir "$SIGNING/key-1"
```

Le répertoire ne doit pas exister ; il est créé en mode `0700`, et un chemin existant n'est jamais réutilisé, même après une initialisation interrompue (choisir un nouveau chemin). La sortie porte l'empreinte publique `key_id` (SHA-256 de la clé publique brute) et jamais la clé privée.

Exporter le document de clé publique pour le détenteur de l'ancre, et transmettre l'empreinte par un **canal distinct** (lecture à voix haute, impression, autre support) :

```sh
ai-guard-audit-checkpoint key show --key-dir "$SIGNING/key-1" --out "$TRANSFER/key-1.public-key.json"
```

## 2. Créer le premier point de contrôle

```sh
ai-guard-audit-checkpoint create --new-log \
  --data "$AUDIT_DB" --tenant-id tenant-a \
  --key-dir "$SIGNING/key-1" --checkpoint-dir "$SIGNING/checkpoints"
```

`--new-log` ouvre un journal de points de contrôle et se donne **une seule fois**. Il est requis, et seulement accepté, tant que le répertoire ne contient aucun enregistrement complet : le premier enregistrement est une ligne de base, et en ouvrir une est une décision. Toute exécution ultérieure, en particulier la ligne confiée à un ordonnanceur, l'omet :

```sh
ai-guard-audit-checkpoint create \
  --data "$AUDIT_DB" --tenant-id tenant-a \
  --key-dir "$SIGNING/key-1" --checkpoint-dir "$SIGNING/checkpoints"
```

Sans `--new-log`, un répertoire inexistant est refusé avec `checkpoint_log_unavailable` et un répertoire sans enregistrement complet avec `checkpoint_log_empty` ; rien n'est créé, pas même le fichier de verrou d'écriture `.writer.lock`, et il en va de même pour `rotate-key`. Un `--checkpoint-dir` mal orthographié échoue donc au lieu d'ouvrir un second journal pendant que le vrai cesse de croître. Une ouverture refusée pour une autre raison (mauvais tenant, schéma trop ancien) ne laisse aucun répertoire, et une ouverture refusée à cause de ce qu'un répertoire existant contient n'y écrit rien, pas même `.writer.lock` : un enregistrement complet (`invalid_argument`, le journal est déjà ouvert), un fichier d'enregistrement qui renferme un autre enregistrement que celui annoncé par son nom (`record_name_mismatch`), deux enregistrements complets à la tête (`log_forked`) ou le nombre maximal de fichiers d'enregistrement (`checkpoint_log_full`).

La sortie nomme `tenant_id`, `namespace_uuid`, `log_index`, `event_count`, `head_hash`, le fichier d'enregistrement, et `baseline: true` pour l'index 1 du journal. Le namespace peut aussi être lu au préalable avec `diagnose --data "$AUDIT_DB" --tenant-id tenant-a` (section 11). Les enregistrements sont des fichiers immuables nommés `cp-<index du journal>-<sha256 de la déclaration>.json` ; cet outil ne les réécrit ni ne les supprime jamais.

La fréquence d'exécution de `create` est une décision d'exploitation, confiée à l'ordonnanceur de votre choix ; cet outil n'en installe aucun. L'intervalle borne la queue non signée de l'audit.

## 3. Initialiser l'ancre de confiance

Le détenteur de l'ancre, sur son propre stockage, approuve la clé **par l'empreinte reçue via le canal distinct**, pour un tenant et un namespace de contrôle :

```sh
ai-guard-audit-checkpoint anchor init \
  --public-key key-1.public-key.json \
  --expect-key-id <64 caractères hexadécimaux reçus séparément> \
  --tenant-id tenant-a --namespace-uuid <namespace_uuid> \
  --out tenant-a.anchor-1.json
```

Un fichier de clé publique dont l'empreinte diffère de `--expect-key-id` est refusé avec `key_id_mismatch` et rien n'est écrit. Une clé qui voyage dans le matériel vérifié n'est jamais approuvée : les enregistrements ne portent aucune clé, et un enregistrement qui tente d'en porter une est refusé comme mal formé.

## 4. Vérifier

```sh
ai-guard-audit-checkpoint verify \
  --anchor tenant-a.anchor-1.json \
  --checkpoint-dir "$SIGNING/checkpoints" --data "$AUDIT_DB"
```

Le rapport (`ai-guard.audit.checkpoint-verification.v1`) porte un `verdict` (`verified` ou `refused`), la position de confiance du journal (`verified_through_log_index`), la comparaison avec la base, les avertissements, les limites applicables et la liste des erreurs. Sans `--data`, `database.compared` vaut `false` et le verdict ne couvre que les enregistrements : il dit ce que la clé a signé, et rien sur l'historique d'audit lui-même.

Avec `--data`, la base doit contenir **chaque position signée de confiance**, de la plus ancienne à la plus récente : les positions que l'ancre retient et référence (section 5) et la position de chaque checkpoint vérifié après la référence. `database.positions_compared` les compte. La première position que la base ne contient pas refuse la vérification avec `database_truncated` ou `database_diverged` ; `detail.signed_at_log_index` et `detail.position_source` (`anchor_retained`, `anchor_reference` ou `checkpoint_record`) la désignent.

La base doit aussi avoir le **genesis de contrôle** que les enregistrements de confiance ont signé. Cet événement est écrit une seule fois, avec le namespace de contrôle, et aucune migration ne le déplace : une base qui en désigne un autre est refusée avec `scope_mismatch` (`detail.field: control_genesis`), même si elle contient chaque position signée et qu'aucun enregistrement n'est nouveau. Le genesis signé est lu dans le checkpoint de confiance le plus récent : un checkpoint vérifié après la référence, ou sinon le checkpoint le plus récent situé à la référence ou en dessous lorsque le répertoire ou le bundle le contient. Lorsqu'aucun enregistrement disponible ne le désigne (un bundle qui ne porte qu'un enregistrement de rotation, par exemple), la comparaison est plus faible : le genesis de la base ne doit pas se trouver après une position signée de confiance. Entre enregistrements, la règle est la même : un checkpoint qui désigne un autre genesis de contrôle que le checkpoint de confiance qui le précède est refusé avec ce code, avec ou sans base.

`database.events_after_checkpoint` est la queue non signée : les événements ajoutés depuis la dernière position signée.

Un refus déjà établi n'est jamais remplacé par un échec ultérieur : si les enregistrements sont refusés et que la base ne peut ensuite pas être lue, le rapport porte toujours le refus, avec l'échec de lecture à côté et `database.compared: false`.

Pour vérifier une **copie** de la base ailleurs, ne pas copier le fichier principal d'une base en service : arrêter le gateway et tout autre consommateur, vérifier qu'aucun fichier `-wal` ne subsiste ou qu'il est vide, puis copier le fichier principal dans un répertoire privé (`0700`), en mode `0600` et sous un nom ASCII. La vérification en place ne demande aucune copie et ne bloque pas le gateway.

## 5. Enregistrer la référence : détection de rollback

Une ancre tout juste initialisée dit seulement à quelle clé faire confiance. Une base plus ancienne restaurée avec un journal plus ancien est correctement signée et se vérifie : le rapport annonce `rollback_detection: unavailable_no_reference`. Un rollback n'est détecté que face à une **référence** conservée par le détenteur de l'ancre :

```sh
ai-guard-audit-checkpoint anchor advance \
  --anchor tenant-a.anchor-1.json \
  --checkpoint-dir "$SIGNING/checkpoints" --data "$AUDIT_DB" \
  --out tenant-a.anchor-2.json
```

La commande vérifie d'abord. En cas de succès, elle écrit une **nouvelle révision** dont la `reference` est la tête vérifiée (index du journal, empreinte de la déclaration et position d'audit signée) et dont `previous_anchor_sha256` est le SHA-256 du fichier de la révision précédente. Aucun fichier n'est jamais écrasé : conserver toutes les révisions. Si aucun enregistrement n'est nouveau, le statut est `unchanged` et aucun fichier n'est écrit, à une exception près : si l'ancre retient des positions antérieures (voir ci-dessous) et que `--data` est fourni, la comparaison les confirme et la commande écrit une révision de même référence dont `retained_checkpoints` est vide, avec le statut `confirmed`. Une vérification refusée sort avec le code `2` et n'écrit rien. La référence ne recule jamais.

Avec une référence :

- un journal qui s'arrête avant elle est refusé avec `log_rolled_back` ;
- un autre enregistrement à la position de référence est refusé avec `log_diverged_from_reference` ;
- un enregistrement situé à la référence ou en dessous n'est accepté que comme maillon de la chaîne que la référence engage. Ces enregistrements ne sont pas vérifiés de nouveau par signature : chacun de ceux qui sont présents est rapproché, par l'empreinte de sa déclaration, de celui que désigne l'enregistrement du dessus. Un second enregistrement complet au même index est refusé avec `log_forked`, un autre enregistrement avec `log_diverged_from_reference`, et un enregistrement situé sous un index qui n'en contient aucun avec `log_discontinuous` (section 6) ;
- une base comparée qui est plus courte que la position référencée, ou différente à cette position, est refusée avec `database_truncated` ou `database_diverged`, **quoi qui ait été signé après la référence** : un enregistrement ultérieur signé par la clé épinglée ne fait pas oublier à l'outil la position que l'ancre a épinglée.

`verify --require-reference` refuse une ancre qui n'a pas encore de référence (`no_independent_reference`) ; l'employer partout où la détection de rollback est attendue.

### Avancer avec la base chaque fois que possible

Les enregistrements seuls prouvent que la clé épinglée a signé une nouvelle tête. Ils ne prouvent pas que cette tête prolonge l'historique épinglé jusque-là : seule une comparaison avec la base le fait.

- `anchor advance --data` compare la base à chaque position de confiance, puis n'épingle que la dernière.
- `anchor advance` sans `--data` (un auditeur qui ne détient qu'un bundle) déplace tout de même la référence, de sorte qu'un journal plus court soit détecté. Les positions épinglées jusque-là ne sont **pas abandonnées** : elles sont conservées dans la nouvelle révision sous `reference.retained_checkpoints`, le résultat porte l'avertissement `reference_advanced_without_database`, et chaque `verify --data` ultérieur compare chacune d'elles. Le prochain `anchor advance --data` confirme la continuité et vide la liste, qu'un enregistrement ait été signé entre-temps ou non : si aucun ne l'a été, le statut est `confirmed` au lieu de `advanced`. Une base qui ne contient pas l'une d'elles refuse cette avance comme toute autre vérification.
- Une révision retient au plus 64 de ces positions. Au-delà, une avance sans `--data` échoue avec `database_comparison_required`.

La détection de rollback ne vaut **qu'à la référence et en dessous**, et pour l'historique d'audit seulement lorsqu'une base est comparée. Entre la référence et la tête, quiconque peut écrire le journal et la base peut les ramener à un état signé antérieur sans détection, même sans la clé, et quiconque détient la clé peut réécrire ce qui suit la référence. La fréquence à laquelle le détenteur de l'ancre avance borne cette fenêtre.

## 6. Écritures interrompues et écrivains concurrents

- Un enregistrement est créé exclusivement, synchronisé avec son répertoire, puis relu. Un fichier portant un nom d'enregistrement qui ne s'analyse pas comme un enregistrement est un **enregistrement incomplet** : il est listé dans `incomplete_records`, ignoré pour la chaîne et jamais supprimé. `verify`, `anchor advance` et `anchor approve-rotation`, qui lisent chaque enregistrement, ajoutent l'avertissement `incomplete_records_present` ; le résultat de `anchor approve-rotation` ne liste pas les fichiers : lancer `verify` ou `diagnose` pour obtenir leurs noms. `create` reprend au même index du journal.
- Un enregistrement incomplet situé au-dessus du dernier enregistrement complet est ce que laisse une écriture interrompue, et aussi ce que laisse la troncature d'un enregistrement signé. `verify` ne peut pas les distinguer : il ne fait confiance au journal que jusqu'à `verified_through_log_index` et ajoute l'avertissement `incomplete_record_above_head`, comme le font `anchor advance` et `anchor approve-rotation`. Comme un enregistrement supprimé, ce cas n'est détecté que par une ancre dont la référence est à cet enregistrement ou après.
- Un fichier portant un nom d'enregistrement qui contient un enregistrement complet **autre que celui annoncé par son nom** (par exemple un enregistrement modifié sur place) ne peut pas venir d'une écriture interrompue. `verify`, `anchor advance`, `anchor approve-rotation`, `export`, `create` lorsqu’il prolonge un journal existant et `rotate-key` lisent chaque enregistrement et refusent le répertoire avec `record_name_mismatch` : aucune révision d'ancre, aucun bundle ni aucun enregistrement n'est alors écrit. Le fichier n'est jamais supprimé : le conserver, enquêter, puis le sortir du répertoire par une décision explicite.
- Si un enregistrement a été rendu durable mais que la commande a été interrompue avant son compte rendu, relancer `create` renvoie `status: unchanged` avec le même enregistrement.
- Un `key init` ou un `--out` interrompu laisse un chemin qui n'est jamais réutilisé : en choisir un nouveau.
- Un second écrivain sur le même répertoire est refusé avec `checkpoint_log_busy`.
- Une commande qui lit le répertoire pendant qu'une autre y écrit un enregistrement ne prend aucun verrou. Elle signale le fichier en cours d'écriture comme un enregistrement incomplet, ou échoue avec `checkpoint_log_unavailable` si ce fichier change pendant sa lecture (code de sortie `1`, aucun verdict) : la relancer.
- Deux enregistrements complets au même index sont refusés avec `log_forked` : par `create` lorsqu’il prolonge un journal existant et par `rotate-key` partout dans le répertoire, par `verify` et `anchor advance` partout dans le répertoire ou le bundle, par `anchor approve-rotation` jusqu'à la rotation qu'il approuve, et par `export` dans la plage qu'il exporte.
- Un enregistrement complet ne compte que si une chaîne le rattache à un enregistrement de confiance : la référence de l'ancre pour `verify`, `anchor advance` et `anchor approve-rotation`, la tête du journal pour `create` et `rotate-key`. Sous cet enregistrement, chaque index doit contenir l'unique enregistrement que celui du dessus désigne par son empreinte. Un autre enregistrement à cette place est refusé (`log_diverged_from_reference` sous une référence, `log_discontinuous` sous la tête), de même qu'un enregistrement situé sous un index qui ne contient aucun enregistrement complet (`log_discontinuous`, avec `detail.missing_log_index` et `detail.unattached_log_index`). Un enregistrement supprimé ou endommagé au milieu du journal est donc un refus, pas un avertissement : le restaurer depuis une sauvegarde du répertoire, ou sortir les enregistrements plus anciens par une décision explicite. Un matériel qui commence simplement plus tard (un répertoire de continuation, un export partiel, une copie à partir d'un index donné) n'est pas concerné : rien n'est exigé sous son premier enregistrement. `diagnose` montre toute la structure (`log_forked`, `log_gap`, `log_link_broken`) sans rien décider.
- Pour ce contrôle, `create` lorsqu’il prolonge un journal existant et `rotate-key` lisent chaque enregistrement du répertoire à chaque exécution, sous le verrou d'écriture : un fichier d'au plus 2 048 octets par enregistrement, au plus 8 208 fichiers.
- `create` ne signe jamais une base plus courte que la position signée par la tête du journal **telle que trouvée dans le répertoire**, ou différente à cette position (`database_truncated`, `database_diverged`). C'est une protection contre les accidents, pas contre quiconque peut écrire à la fois la base et le répertoire des points de contrôle. Cet acteur peut retirer les derniers enregistrements, réécrire les événements qu'ils couvraient et laisser le prochain `create` planifié signer le résultat, sans jamais lire la clé. `create` ne consulte aucune ancre : seul `verify` avec une référence située aux enregistrements retirés ou après le détecte, ce qui est une raison de plus d'avancer souvent l'ancre.
- `create` ne signe pas non plus une base dont le genesis de contrôle n'est pas celui que désigne le dernier checkpoint du journal, ou se trouve après la dernière position signée (`scope_mismatch`, `detail.field: control_genesis`) : le journal porterait sinon le nouveau genesis à partir de là.

## 7. Répertoire de points de contrôle plein

Un répertoire de points de contrôle contient au plus 8 192 fichiers d'enregistrement. Au-delà, `create` échoue avec `checkpoint_log_full` et nomme le dernier enregistrement dans `detail.last_record`. Poursuivre dans un nouveau répertoire, lié à cet enregistrement :

```sh
ai-guard-audit-checkpoint create \
  --data "$AUDIT_DB" --tenant-id tenant-a --key-dir "$SIGNING/key-1" \
  --checkpoint-dir "$SIGNING/checkpoints-2" \
  --previous-record "$SIGNING/checkpoints/cp-<index du journal>-<sha256>.json"
```

`--previous-record` ouvre le nouveau répertoire (sans `--new-log`) et n'est accepté que tant que celui-ci ne contient aucun enregistrement complet. Fournir le fichier sous le nom qu'il porte dans l'ancien répertoire : un fichier qui contient un autre enregistrement que celui annoncé par son nom, ou une copie de l'enregistrement sous un autre nom, est refusé avec `record_name_mismatch` (`detail.expected_name` donne le nom de l'enregistrement que le fichier contient) avant toute création, y compris celle du nouveau répertoire. Le nouveau répertoire commence à l'index suivant ; il ne se vérifie donc qu'avec une ancre dont la référence est au dernier enregistrement de l'ancien répertoire ou après : exécuter `anchor advance` sur l'ancien répertoire avant de basculer. Conserver l’ancien répertoire.

Lorsque le prédécesseur signe déjà la tête de la base, la continuation ne rend `unchanged` qu’après avoir examiné une destination existante sans verrou d’écriture. Un enregistrement complet y est refusé (`invalid_argument`, sortie `1`) ; un fork à la tête parcourue ou un enregistrement renommé est refusé (`log_forked` ou `record_name_mismatch`, sortie `2`). Ce contrôle de premier enregistrement peut s’arrêter au premier index complet : il n’authentifie pas une destination déjà commencée. Employer `create` sans `--previous-record` après le premier enregistrement. Une destination absente reste absente ; une destination vide reste vide ; les fichiers incomplets sont conservés et listés. Aucun enregistrement ni fichier de verrou n’est créé par ce no-op ou ces refus. La lecture ne prend aucun verrou : un autre processus peut changer le répertoire après le contrôle.

Les enregistrements de rotation peuvent employer 16 fichiers au-delà de cette borne, afin qu'un répertoire plein n'empêche jamais de retirer une clé. Un bundle de preuve porte au plus 8 208 enregistrements, soit tout ce qu'un répertoire peut contenir.

## 8. Sauvegarde et restauration

Restaurer une base plus ancienne est, pour ce mécanisme, un rollback : avec une référence, il est détecté et refusé, ce qui est le but. Il n'existe volontairement aucun enregistrement signé « accepter cette régression », car il permettrait au seul détenteur de la clé de masquer un rollback. Une restauration légitime est une rupture explicite décidée par le détenteur de l'ancre :

1. Conserver l'ancien répertoire de points de contrôle, chaque ancienne révision de l'ancre et la base restaurée pour mémoire.
2. Créer une **nouvelle clé de signature** (`key init`) pour le nouveau journal. L'ancien journal reste une chaîne valide sous l'ancienne clé : une ancre qui épingle l'ancienne clé et n'a pas encore de référence vérifierait le journal abandonné, avec la base qui l'accompagne, exactement aussi bien que le nouveau.
3. Ouvrir un nouveau répertoire de points de contrôle avec `create --new-log` et la nouvelle clé.
4. Créer une nouvelle ancre avec `anchor init`, qui épingle **uniquement** la nouvelle clé.
5. Enregistrer aussitôt la référence avec `anchor advance --data` : d'ici là, la nouvelle ancre ne détecte aucun rollback.

Sauvegarder le répertoire des points de contrôle et chaque révision de l'ancre comme toute autre preuve. Perdre les enregistrements n'endommage pas l'audit, mais il ne reste alors rien de signé pour le vérifier.

## 9. Faire tourner la clé de signature

Une rotation comprend trois étapes explicites, réalisées par deux personnes.

1. L'opérateur crée la clé successeur et exporte son document public. L'empreinte passe par le canal distinct, comme à la section 1.

   ```sh
   ai-guard-audit-checkpoint key init --key-dir "$SIGNING/key-2"
   ai-guard-audit-checkpoint key show --key-dir "$SIGNING/key-2" --out "$TRANSFER/key-2.public-key.json"
   ```

2. L'opérateur consigne le passage de relais dans le journal. La clé sortante endosse son successeur, et le successeur prouve la possession de sa clé.

   ```sh
   ai-guard-audit-checkpoint rotate-key \
     --checkpoint-dir "$SIGNING/checkpoints" \
     --key-dir "$SIGNING/key-1" --new-key-dir "$SIGNING/key-2"
   ```

   Dès lors `create` doit employer `--key-dir "$SIGNING/key-2"` ; la clé retirée est refusée avec `key_id_mismatch`. Relancer le même `rotate-key` après une interruption renvoie `status: already_rotated` et n'écrit rien.

3. Le détenteur de l'ancre approuve explicitement le successeur, avec l'empreinte reçue séparément.

   ```sh
   ai-guard-audit-checkpoint anchor approve-rotation \
     --anchor tenant-a.anchor-2.json --checkpoint-dir "$SIGNING/checkpoints" \
     --public-key key-2.public-key.json \
     --expect-key-id <64 caractères hexadécimaux reçus séparément> \
     --out tenant-a.anchor-3.json
   ```

Jusqu'à cette approbation, `verify` et `anchor advance` s'arrêtent avant l'enregistrement de rotation avec `untrusted_key` (`detail.record_kind: key_rotation`). Un endossement n'est pas une approbation : une clé sortante compromise pourrait introduire n'importe quel successeur.

Ajouter `--data "$AUDIT_DB"` à `anchor approve-rotation` chaque fois que la base est à portée. L'approbation déplace la référence jusqu'à l'enregistrement de rotation, au-delà des checkpoints que la clé sortante a signés depuis la référence précédente. Comme pour `anchor advance` (section 5), la base est alors comparée à chaque position de confiance jusqu'à la rotation ; sans elle, les positions épinglées jusque-là sont retenues dans la nouvelle révision et le résultat porte `reference_advanced_without_database`.

L'approbation exige que le répertoire ne contienne aucun fichier d'enregistrement renfermant un autre enregistrement que celui annoncé par son nom (`record_name_mismatch`, section 6), que chaque enregistrement situé à la référence ou en dessous soit un maillon de la chaîne que la référence engage (section 6), que chaque enregistrement entre la référence et la rotation se vérifie, que la rotation retire la clé courante de l'ancre, que trois empreintes concordent (celle attendue, le fichier de clé publique et l'enregistrement de rotation), que la clé sortante épinglée dans l'ancre ait endossé la rotation, et que le successeur l'ait signée. La nouvelle révision **épingle alors l'enregistrement de rotation lui-même comme référence** et ferme la clé retirée une position avant lui. Après approbation, la clé retirée ne peut plus rien signer à la rotation ni après (`key_not_valid_at_index`), et aucun autre enregistrement ne peut tenir la place de la rotation (`log_diverged_from_reference`, `log_rolled_back`). Un enregistrement incomplet ne fait pas refuser l'approbation : le résultat porte l'avertissement `incomplete_records_present` et, seulement lorsqu'un enregistrement incomplet se trouve au-dessus du dernier enregistrement complet, `incomplete_record_above_head` (section 6). L'approbation n'examine les enregistrements que jusqu'à la rotation : si des enregistrements se trouvent après elle, le résultat porte l'avertissement `records_after_rotation_not_examined`, et `verify` puis `anchor advance` avec la nouvelle révision les examinent.

### Clé perdue ou compromise

Lorsque la clé sortante ne peut pas ou ne doit pas signer, l'opérateur consigne une rotation non endossée et le détenteur de l'ancre l'approuve comme une reprise explicite :

```sh
ai-guard-audit-checkpoint rotate-key \
  --checkpoint-dir "$SIGNING/checkpoints" --new-key-dir "$SIGNING/key-2" \
  --unendorsed-recovery --retiring-key-id <empreinte de la clé sortante>

ai-guard-audit-checkpoint anchor approve-rotation \
  --anchor tenant-a.anchor-2.json --checkpoint-dir "$SIGNING/checkpoints" \
  --public-key key-2.public-key.json --expect-key-id <64 caractères hexadécimaux> \
  --unendorsed-recovery --out tenant-a.anchor-3.json
```

Sans `--unendorsed-recovery`, l'approbation est refusée avec `rotation_not_endorsed`. Avec ce drapeau, la clé est épinglée avec `approval: operator_recovery` et chaque rapport ultérieur porte l'avertissement `unendorsed_key_approval`. `--unendorsed-recovery` est refusé pour une rotation endossée.

Après une compromission, les enregistrements compris entre la référence précédente de l'ancre et la rotation ont été signés par la clé compromise : **ils ne sont pas dignes de confiance**, seul ce que la référence couvrait déjà l'est. Avancer souvent l'ancre borne cette exposition.

## 10. Exporter une preuve pour un auditeur

```sh
ai-guard-audit-checkpoint export \
  --checkpoint-dir "$SIGNING/checkpoints" --out "$TRANSFER/tenant-a.proof.json"
```

Le bundle (`ai-guard.audit.checkpoint-proof.v1`) porte des enregistrements contigus et **aucune clé, publique ou privée**. `--from-log-index` et `--through-log-index` sélectionnent une plage ; par défaut, tout ce qui est disponible. Un bundle porte au plus 8 208 enregistrements et 20 Mio : au-delà, `export` échoue avec `invalid_argument` et demande une plage, plutôt que d'écrire un bundle que `verify` refuserait. Ses champs d'en-tête sont dérivés des entrées et ne sont pas authentifiés : le vérificateur lit les entrées.

L'auditeur n'a besoin que du bundle et de sa propre ancre, sans base, sans clé et sans répertoire de points de contrôle :

```sh
ai-guard-audit-checkpoint verify \
  --anchor tenant-a.anchor-2.json --bundle tenant-a.proof.json \
  --require-reference --report-out tenant-a.report.json
```

- `--bundle` remplace `--checkpoint-dir` dans `verify`, `anchor advance` et `anchor approve-rotation`.
- Un bundle seul dit ce que la clé a signé. Avancer l'ancre depuis un bundle sans `--data` conserve les positions épinglées jusque-là sous `reference.retained_checkpoints` (section 5) ; elles sont comparées dès qu'une base est fournie.
- `--report-out` écrit une copie du rapport, vérifié ou refusé, dans un nouveau fichier.
- Sans `--data`, le verdict ne couvre que les enregistrements (`database.compared: false`) ; avec une copie cohérente de la base (section 4), il couvre aussi son contenu.
- Un bundle peut commencer après la référence de l'ancre si son premier enregistrement s'y lie. Un bundle qui s'arrête avant la référence est refusé avec `log_rolled_back`. Un bundle qui porte des enregistrements situés à la référence ou en dessous est refusé s'ils ne sont pas la chaîne que la référence engage (`log_diverged_from_reference`) : aucun enregistrement d'un bundle vérifié n'est laissé sans examen.
- Toute entrée invalide refuse le bundle entier (`bundle_malformed`, `record_malformed`, `record_not_canonical`).

Le bundle prouve ce que prouvent les enregistrements. Il ne rend pas les exports d'événements existants du gateway vérifiables par eux-mêmes.

## 11. Diagnostiquer

```sh
ai-guard-audit-checkpoint diagnose \
  --data "$AUDIT_DB" --tenant-id tenant-a \
  --checkpoint-dir "$SIGNING/checkpoints" --key-dir "$SIGNING/key-1" \
  --anchor tenant-a.anchor-2.json
```

Nommer n'importe quel sous-ensemble des quatre éléments. `diagnose` ne vérifie aucune signature et ne prend **aucune décision de confiance** : il explique un état. Chaque section (`database`, `checkpoint_log`, `signing_key`, `trust_anchor`) dit si elle est `readable` et porte sinon sa propre `error` ; un élément illisible n'est jamais présenté comme vide. `findings` liste ce qui mérite attention :

| Constat | Sens |
|---|---|
| `unsigned_tail` | Événements ajoutés après la dernière position signée (`detail.events_after_last_checkpoint`) |
| `incomplete_records_present`, `unexpected_entries_present`, `rejected_entries_present` | Restes d'écritures interrompues, entrées égarées, ou entrées qui enfreignent la politique de stockage privé et font refuser le répertoire par `create`/`verify` |
| `record_name_mismatch` | Fichiers d'enregistrement qui contiennent un autre enregistrement complet que celui annoncé par leur nom ; `verify`, `anchor advance`, `anchor approve-rotation`, `export`, `create` et `rotate-key` refusent le répertoire |
| `log_forked`, `log_gap`, `log_link_broken` | Défauts de structure du journal. `log_gap` donne le nombre de positions manquantes et les premières plages manquantes (`detail.missing_ranges`). `create`, `rotate-key`, `verify` et `anchor advance` refusent un répertoire qui en présente un (section 6) |
| `scope_differs` | Base, journal et ancre ne partagent pas tenant et namespace |
| `database_shorter_than_last_checkpoint`, `database_differs_at_last_checkpoint` | La base ne prolonge pas le dernier enregistrement du journal |
| `key_is_not_next_signer` | Le répertoire de clé n'est pas la clé que le journal attend ensuite |
| `anchor_has_no_reference`, `records_after_anchor_reference`, `anchor_reference_ahead_of_log` | Position de la référence de l'ancre par rapport au journal |
| `anchor_retains_uncompared_positions` | La référence a dépassé des positions antérieures sans comparaison avec une base ; `anchor advance --data` confirme la continuité et écrit une révision qui ne retient plus rien, même si aucun enregistrement n'est nouveau |
| `next_signer_not_current_anchor_key` | Une rotation attend peut-être `anchor approve-rotation` |

L'employer pour lire le `namespace_uuid` d'une base avant le premier point de contrôle, et pour contrôler à la main la chaîne des révisions de l'ancre : le `trust_anchor.sha256` d'une révision est le `previous_anchor_sha256` de la suivante.

## 12. Installation Compose : conteneur ponctuel

**`NOT_RUN` : cette procédure a été rédigée à partir de la définition de l'image et n'a jamais été exécutée. Ni la construction de l'image ni aucune commande ci-dessous ne constituent une preuve.** Elle suit le schéma de maintenance hors ligne du [guide d'accès local nominatif](../identity/README.fr.md) : un conteneur ponctuel de l'image de contrôle, exécuté sous l'UID de contrôle sur le volume d'audit existant, sans réseau, socket ni credential.

`images/control.Dockerfile` construit et installe `ai-guard-audit-checkpoint` à côté de `agentsandbox`. Inspecter le nom du volume `/data` existant sur le conteneur du gateway et l'identifiant de l'image de contrôle construite ; ne deviner ni l'un ni l'autre. Le gateway peut rester en service pour `create` : la base est ouverte en lecture seule.

```sh
AIGUARD_CONTROL_IMAGE_ID="sha256:inspected-current-control-image-id"
AIGUARD_AUDIT_VOLUME="existing-inspected-audit-volume"
(
set -eu
docker volume inspect --format '{{.Name}}' "$AIGUARD_AUDIT_VOLUME"
docker run --name ai-guard-checkpoint-key-1 --pull never \
  --user 1000:1000 --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges:true --cpus 1 --memory 384m --memory-swap 384m --pids-limit 128 \
  --tmpfs /tmp:rw,noexec,nosuid,nodev,size=16m,mode=1777 \
  --mount "type=volume,source=$AIGUARD_AUDIT_VOLUME,target=/data,volume-nocopy" \
  --entrypoint ai-guard-audit-checkpoint "$AIGUARD_CONTROL_IMAGE_ID" \
  key init --key-dir /data/audit-signing/key-1
docker run --name ai-guard-checkpoint-first --pull never \
  --user 1000:1000 --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges:true --cpus 1 --memory 384m --memory-swap 384m --pids-limit 128 \
  --tmpfs /tmp:rw,noexec,nosuid,nodev,size=16m,mode=1777 \
  --mount "type=volume,source=$AIGUARD_AUDIT_VOLUME,target=/data,volume-nocopy" \
  --entrypoint ai-guard-audit-checkpoint "$AIGUARD_CONTROL_IMAGE_ID" \
  create --new-log --data /data/audit.sqlite --tenant-id tenant-a \
  --key-dir /data/audit-signing/key-1 --checkpoint-dir /data/audit-signing/checkpoints
)
```

Les autres commandes emploient les mêmes options `docker run`, un nouveau nom de conteneur, et ces arguments après l'image :

```text
key show --key-dir /data/audit-signing/key-1 --out /data/audit-signing/key-1.public-key.json
create --data /data/audit.sqlite --tenant-id tenant-a --key-dir /data/audit-signing/key-1 --checkpoint-dir /data/audit-signing/checkpoints
export --checkpoint-dir /data/audit-signing/checkpoints --out /data/audit-signing/tenant-a.proof-1.json
diagnose --data /data/audit.sqlite --tenant-id tenant-a --checkpoint-dir /data/audit-signing/checkpoints --key-dir /data/audit-signing/key-1
```

Chaque nom de conteneur et chaque chemin de sortie doivent être nouveaux ; les fichiers `--out` vont dans le répertoire privé existant `/data/audit-signing`, créé par `key init`. Vérifier chaque code de sortie et chaque document JSON avant de poursuivre. Copier le document de clé publique et le bundle hors d'un conteneur conservé avec `docker cp` (sans `--archive`) et les remettre au détenteur de l'ancre ; ne jamais copier le répertoire de clé.

Ce que cette disposition apporte et n'apporte pas :

- La clé vit dans le même volume que la base, sous le même UID. Face à l'administrateur de ce volume ou au compte de contrôle, la signature n'apporte rien : seule l'ancre tenue ailleurs compte.
- L'ancre, `verify` et `anchor advance` restent **hors** du volume, sur la machine du détenteur de l'ancre, avec un binaire construit pour cette machine. Sous Docker Desktop, une ancre ou un bundle ne peuvent pas être fournis au conteneur par bind mount : la politique des fichiers privés refuse FUSE et les systèmes de fichiers réseau.
- Vérifier le contenu de la base demande une copie cohérente de celle-ci sur cette machine (section 4), ou une exécution de `verify --data` dans un conteneur ponctuel avec une ancre copiée dans le volume, qui n'est alors plus indépendante du volume.

## Erreurs

Chaque erreur porte un `code`, un `message`, un objet `detail` et une `action`.

| Code | Sortie | Sens | Action attendue |
|---|---|---|---|
| `signature_invalid` | 2 | Une signature est absente, en trop, ou ne se vérifie pas sur les octets exacts de la déclaration | Ne pas faire confiance à l'enregistrement ; comparer le journal à une copie indépendante |
| `untrusted_key` | 2 | L'enregistrement est signé par une clé que l'ancre n'épingle pas | Seules les clés approuvées dans l'ancre sont acceptées |
| `key_id_mismatch` | 2 | Une empreinte diffère de celle attendue | Comparer la clé attendue, le fichier de clé et l'enregistrement |
| `key_not_valid_at_index` | 2 | Une clé épinglée a signé hors des positions du journal qu'elle peut signer, typiquement une clé retirée après sa rotation | Considérer l'enregistrement comme non fiable et enquêter sur l'usage de la clé retirée |
| `rotation_not_endorsed` | 2 | La clé sortante n'a pas endossé la rotation, ou son endossement ne se vérifie pas | N'approuver que comme reprise explicite, après une décision hors bande |
| `scope_mismatch` | 2 | Le tenant, le namespace de contrôle ou le genesis de contrôle diffère entre ancre, enregistrements et base (`detail.field`, `detail.between`) | Employer la base, le journal et l'ancre du même tenant et du même namespace ; une base recréée est un nouveau namespace. Un genesis de contrôle ne change jamais : considérer comme altéré la base ou l'enregistrement qui en désigne un autre |
| `log_rolled_back` | 2 | Le journal s'arrête avant la référence de l'ancre | Restaurer les enregistrements manquants, ou faire décider une rupture documentée par le détenteur de l'ancre |
| `log_diverged_from_reference` | 2 | Un autre enregistrement que celui que la référence engage occupe la position de référence, ou une position située en dessous (`detail.log_index`) | Considérer le journal comme remplacé |
| `log_discontinuous` | 2 | Un enregistrement manque à un index attendu, ne se lie pas à son prédécesseur, ou se trouve sous un index qui n'en contient aucun (`detail.missing_log_index`, `detail.unattached_log_index`) | Restaurer le journal complet |
| `log_forked` | 2 | Deux enregistrements complets revendiquent le même index | Conserver les deux et enquêter |
| `checkpoint_regression` | 2 | Un enregistrement ne rappelle pas la dernière position signée | Considérer le journal comme incohérent |
| `database_truncated` | 2 | La base contient moins d'événements qu'une position signée de confiance (`detail.position_source`, `detail.signed_at_log_index`) | Traiter comme un rollback ou une troncature |
| `database_diverged` | 2 | La base ne porte pas l'empreinte signée à une position signée de confiance | Considérer l'historique comme réécrit ; conserver base et enregistrements |
| `audit_chain_invalid` | 2 | Le lecteur d'intégrité existant refuse déjà la base, ou le fichier n'est pas une base SQLite | La conserver et enquêter |
| `audit_schema_unversioned` | 2 | La base ne porte aucune version de schéma : elle est vide ou tronquée, ou c'est une base héritée que le gateway n'a pas encore mise à niveau (`detail.schema_objects`) | Si un point de contrôle a déjà été signé, traiter comme une troncature ; une base héritée est ouverte une fois par le gateway, puis migrée avec `migrate-control --target-version 2` |
| `audit_namespace_unavailable` | 2 | Base au schéma 1, sans namespace de contrôle | Exécuter `migrate-control --target-version 2` |
| `record_malformed`, `record_not_canonical`, `bundle_malformed` | 2 | Un enregistrement ou un bundle n'est pas la forme stricte et canonique de son contenu | Obtenir de nouveau les enregistrements |
| `record_name_mismatch` | 2 | Un fichier d'enregistrement, dans un répertoire ou fourni à `--previous-record`, contient un enregistrement complet autre que celui annoncé par son nom (`detail.records`) | Considérer le répertoire comme altéré : le conserver et le comparer à une copie ou à un export indépendants |
| `no_independent_reference` | 2 | `--require-reference` et l'ancre n'a pas de référence | Exécuter `anchor advance` après une vérification acceptée |
| `checkpoint_log_busy`, `checkpoint_log_full`, `checkpoint_log_empty`, `checkpoint_log_unavailable`, `checkpoint_record_exists` | 1 | Le répertoire des points de contrôle ne peut pas être écrit ou lu maintenant, n'existe pas, ou ne contient aucun enregistrement complet | Réessayer, poursuivre dans un nouveau répertoire, corriger le chemin ou la politique du répertoire, ou ouvrir le journal avec `--new-log` ; les entrées inattendues ne sont jamais supprimées |
| `key_directory_exists`, `key_unavailable`, `key_invalid` | 1 | Le répertoire de clé ne peut pas être créé ou utilisé | Choisir un nouveau répertoire, ou restaurer la clé prévue |
| `no_pending_rotation`, `anchor_key_limit` | 1 | Rien à approuver après la référence, ou l'ancre épingle déjà 512 clés | Consigner d'abord une rotation, ou faire décider un nouveau journal et une nouvelle ancre par le détenteur de l'ancre |
| `database_comparison_required` | 1 | L'ancre retient déjà 64 positions avancées sans base | Avancer avec `--data` |
| `anchor_unavailable`, `anchor_invalid`, `input_unavailable` | 1 | Un fichier d'entrée ne peut pas être lu sous la politique de fichier local de confiance, ou n'est pas ce que cet outil a écrit | Le copier sur un disque local avec la propriété requise ; ne jamais éditer une ancre |
| `audit_unavailable`, `audit_schema_unsupported` | 1 | La base ne peut pas être ouverte sous la politique de stockage privé ou est occupée, ou sa version de schéma est plus récente que celles connues de ce binaire | Corriger le chemin et les permissions, réessayer, ou employer un binaire correspondant |
| `output_exists`, `output_unavailable`, `io_failure`, `invalid_argument` | 1 | Chemin de sortie ou arguments | Choisir un nouveau chemin de sortie ; rien n'est écrasé |

Une erreur `checkpoint_log_unavailable` est aussi levée lorsque le répertoire contient une entrée qui n'est pas un fichier privé (par exemple un fichier en mode `0644`) : `create` et `verify` échouent fermés plutôt que de l'ignorer. Les entrées au nom ordinaire et au mode privé qui ne sont pas des enregistrements sont listées dans `unexpected_entries` avec l'avertissement `unexpected_entries_present`.

## Limites

- Aucune résistance à un administrateur qui contrôle simultanément les données, la clé et l'ancre.
- Les événements postérieurs au dernier point de contrôle ne sont couverts que par le suivant.
- Sans référence dans l'ancre, un rollback vers un état signé antérieur n'est pas détecté ; le rapport le dit. Avec une référence, la détection ne vaut qu'à la référence et en dessous.
- Avec la clé et la base, un acteur peut réécrire ce qui suit la référence de l'ancre. Ce qui se trouve à la référence et en dessous est protégé tant que l'ancre est intacte **et qu'une base lui est comparée** : une vérification des seuls enregistrements, depuis un bundle ou un répertoire, ne dit rien de l'historique d'audit.
- Au-dessus de la référence de l'ancre, `create` n'offre aucune protection contre un acteur qui écrit à la fois la base et le répertoire des points de contrôle : la prochaine exécution signe ce qu'elle trouve.
- Un fichier d'enregistrement tronqué au-dessus de la tête du journal est indiscernable d'une écriture interrompue et vaut un enregistrement supprimé : seule la référence de l'ancre le détecte.
- L'ancre de confiance ne porte aucun genesis de contrôle : celui qui a été signé n'est connu que par un enregistrement de checkpoint. Lorsque le matériel ne contient aucun checkpoint à la référence ou en dessous et qu'aucune base n'est comparée, un checkpoint qui désigne une autre empreinte de genesis à une séquence admissible n'est pas détecté.
- La clé n'est protégée que par les permissions du système de fichiers.
- `created_at` n'est pas un horodatage de confiance ; il n'existe ni journal de transparence ni ancrage externe.
- `GET /api/v1/integrity` et `Store::integrity` sont inchangés : ils annoncent toujours intègre une base réécrite de façon cohérente, et aucun des deux ne rend le verdict des checkpoints signés. `agentsandbox verify-audit --checkpoint-dir … --anchor … [--data …]` imprime en plus le rapport signé hors ligne après la réponse du gateway et échoue sur refus ou erreur ; sans ces options sa sortie reste inchangée. L’opérateur fournit les périmètres correspondants : les deux documents ne sont pas un verdict d’autorité fusionné. Sans base, seul le matériel signé est vérifié ; sans référence dans l’ancre, la détection de rollback reste indisponible. La commande composée n’est pas encore exercée dans sa QA d’intégration. Le gateway ne détient aucune ancre de confiance et ne doit pas en détenir : une ancre gardée par le contrôle qu'elle vérifie n'est pas indépendante. La détection passe par cette commande, exécutée par qui détient l'ancre, et par une ancre tenue de façon indépendante.
- Cet outil signe la base Store d'un tenant. Le journal d'authentification V4 reste distinct. Pour la vérification via le gateway avec SetupV2, [fournir explicitement le fichier privé `viewer_token_file`](../local/README.fr.md#vérifier-laudit-dun-tenant-setupv2). Le gateway détermine le tenant à partir du credential. Si l'option est omise, le CLI utilise `<config-dir>/secrets/viewer`. Pour demander le rapport signé, l'opérateur fournit la base, les checkpoints et l'ancre du même tenant.
- La tête signée lie la chaîne d'événements et les projections de contrôle que le lecteur existant reconstruit depuis elle ; elle ne rend pas les exports d'événements existants vérifiables par eux-mêmes.
