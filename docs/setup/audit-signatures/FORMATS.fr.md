# Points de contrôle signés de l'audit : référence des formats

[English](FORMATS.md) · [Français](FORMATS.fr.md) · [Guide](README.fr.md)

Cette page est le contrat publié de la version 1 des documents lus et écrits par `ai-guard-audit-checkpoint`. Elle s'adresse à un auditeur qui veut relire ou réimplémenter la vérification, et sert de référence avec laquelle une version ultérieure devra rester compatible. Le [guide](README.fr.md) explique l'exploitation de la commande ; la source de ces règles est `src/audit/signed_checkpoint/`. Un contrat n'est pas une qualification : voir le [handoff d'ingénierie](../../engineering/evidence/2026-11-10/audit-signatures/README.md).

## Règles communes

- Chaque document est du JSON en UTF-8. Les enregistrements signés sont en ASCII uniquement.
- Un SHA-256 s'écrit en 64 caractères hexadécimaux minuscules. « Tout à zéro » désigne 64 caractères `0`.
- Un UUID s'écrit sous sa forme canonique minuscule avec tirets.
- Une heure s'écrit `AAAA-MM-JJTHH:MM:SS.mmmZ` (UTC, millisecondes) et rien d'autre. Toute heure est déclarée par son rédacteur et n'est pas un horodatage de confiance.
- Les valeurs binaires sont en base64url sans remplissage (RFC 4648, section 5), sous forme canonique.
- Les entiers sont non signés et valent au plus 9223372036854775807.
- Un identifiant de tenant comprend 1 à 128 lettres ASCII, chiffres, tirets ou tirets bas.
- Les membres inconnus, absents ou dupliqués sont refusés partout.

## Clé de signature

| Élément | Règle |
|---|---|
| Algorithme | Ed25519 (RFC 8032, variante pure) |
| Fichier de clé privée | `signing-key.pk8` dans le répertoire de clé : PKCS#8 v2 (RFC 5958) avec la clé publique, 83 octets, mode `0600`, répertoire en mode `0700`. Il n'apparaît dans aucun autre document. |
| Empreinte `key_id` | SHA-256 de la clé publique brute de 32 octets |

Document de clé publique, écrit par `key show --out` sur une ligne suivie d'un saut de ligne, au plus 1 024 octets :

```json
{"format":"ai-guard.audit.signing-public-key.v1","algorithm":"ed25519","key_id":"<sha256>","public_key":"<base64url de 32 octets>"}
```

Un lecteur refuse un document dont le `key_id` n'est pas le SHA-256 de sa `public_key`. Ce document n'approuve rien par lui-même : seule une ancre épingle une clé.

## Déclaration signée

Une déclaration est un objet JSON sous **forme compacte** (sans espace), comportant exactement les membres ci-dessous, **dans cet ordre**. Ce sont les octets de cet objet qui sont signés et hachés. Un lecteur analyse la déclaration, la sérialise de nouveau depuis son contenu typé et la refuse si le résultat diffère des octets reçus.

### Genre `checkpoint`

| N° | Membre | Type | Règle |
|---|---|---|---|
| 1 | `type` | chaîne | `ai-guard.audit.checkpoint` |
| 2 | `version` | entier | `1` |
| 3 | `kind` | chaîne | `checkpoint` |
| 4 | `tenant_id` | chaîne | Tenant lié à la base d'audit |
| 5 | `namespace_uuid` | chaîne | Namespace de contrôle de la base, créé avec son événement genesis de contrôle |
| 6 | `log_index` | entier | Position dans le journal des points de contrôle, à partir de 1 |
| 7 | `previous_statement_sha256` | chaîne | SHA-256 de la déclaration à `log_index - 1` ; tout à zéro si et seulement si `log_index` vaut 1 |
| 8 | `created_at` | chaîne | Heure déclarée |
| 9 | `signing_key_id` | chaîne | Empreinte de la clé qui signe cet enregistrement et doit signer le suivant |
| 10 | `audit_schema_version` | entier | Version de schéma de la base à la signature, au moins 2 |
| 11 | `chain_algorithm` | chaîne | `agentsandbox:audit:v1` |
| 12 | `event_count` | entier | Nombre d'événements d'audit signés, au moins 1 |
| 13 | `head_hash` | chaîne | Empreinte de chaîne de l'événement numéro `event_count` ; jamais tout à zéro |
| 14 | `control_genesis_seq` | entier | Séquence de l'événement genesis de contrôle, entre 1 et `event_count` |
| 15 | `control_genesis_hash` | chaîne | Empreinte de chaîne de cet événement |
| 16 | `previous_event_count` | entier | `event_count` de la dernière position signée avant cet enregistrement ; 0 s'il n'y en a pas |
| 17 | `previous_head_hash` | chaîne | `head_hash` de cette position ; tout à zéro si et seulement si `previous_event_count` vaut 0 |

`previous_event_count` vaut au plus `event_count` ; lorsqu'ils sont égaux, `previous_head_hash` est égal à `head_hash`.

Le genesis de contrôle d'une base est un événement unique, écrit une seule fois avec son namespace de contrôle ; aucune migration ne le déplace. Chaque checkpoint signé sur une même base désigne donc le même `control_genesis_seq` et le même `control_genesis_hash`.

### Genre `key_rotation`

| N° | Membre | Type | Règle |
|---|---|---|---|
| 1–8 | `type`, `version`, `kind`, `tenant_id`, `namespace_uuid`, `log_index`, `previous_statement_sha256`, `created_at` | | Comme ci-dessus, avec `kind` égal à `key_rotation` et `log_index` supérieur à 1 |
| 9 | `retiring_key_id` | chaîne | Empreinte de la clé qui passe le relais |
| 10 | `successor_key_id` | chaîne | Empreinte de la clé qui doit signer l'enregistrement suivant ; différente de `retiring_key_id` |
| 11 | `endorsement` | chaîne | `retiring_key` ou `none` |
| 12 | `previous_event_count` | entier | Dernière position signée avant la rotation, au moins 1 |
| 13 | `previous_head_hash` | chaîne | Son `head_hash` |

Une rotation ne signe aucune nouvelle position d'audit : elle reporte la précédente.

## Signature et enregistrement

Le **message signé** est formé des 35 octets ASCII `ai-guard:audit-signed-checkpoint:v1`, d'un octet nul, puis des octets exacts de la déclaration. `statement_sha256` est le SHA-256 des seuls octets de la déclaration.

Un enregistrement est un objet JSON sous forme compacte, d'au plus 2 048 octets, sans saut de ligne final :

```json
{"format":"ai-guard.audit.signed-checkpoint.v1","statement":{…},"signatures":[{"key_id":"<sha256>","signature":"<base64url de 64 octets>"}]}
```

- `signatures` contient une ou deux entrées. Les signataires requis, dans cet ordre et sans autre entrée, sont : `signing_key_id` pour un checkpoint ; `retiring_key_id` puis `successor_key_id` pour une rotation dont `endorsement` vaut `retiring_key` ; `successor_key_id` seul lorsqu'il vaut `none`.
- `key_id` ne fait que nommer une clé. La clé publique est toujours prise dans l'ancre de confiance. Un enregistrement n'a aucun membre qui pourrait porter une clé.
- L'enregistrement entier doit être, octet pour octet, la sérialisation compacte de son contenu.

### Répertoire des points de contrôle

Pour `create --previous-record` sans rien de nouveau à signer, la politique de premier enregistrement de la destination est quand même contrôlée sans verrou avant `unchanged` : une tête complète existante, un fork à cette tête, un enregistrement renommé ou un répertoire indisponible est refusé. Le parcours peut s’arrêter au premier index complet ; il n’authentifie pas un journal déjà commencé. Aucun répertoire ni verrou n’est créé ; les fichiers incomplets sont signalés. Après le premier enregistrement, lancer `create` sans `--previous-record`. Ce no-op sans verrou est une lecture à cet instant, pas une garantie contre un écrivain concurrent.

- Un enregistrement est le fichier `cp-<log_index sur 20 chiffres décimaux>-<statement_sha256>.json`, en mode `0600`, dans un répertoire privé (mode `0700`). Les fichiers sont créés exclusivement et l'outil ne les renomme, ne les réécrit ni ne les supprime jamais. `.writer.lock` est le verrou d'écriture : un fichier vide, jamais supprimé, créé par la première commande qui prend le verrou. `create --new-log` ou `--previous-record` le prend pour écrire le premier enregistrement, après avoir lu le répertoire sans lui : une ouverture refusée parce que le répertoire contient un enregistrement complet, un fichier d'enregistrement renfermant un autre enregistrement que celui annoncé par son nom, un fork ou le nombre maximal de fichiers d'enregistrement ne crée aucun fichier de verrou. `create` sans `--new-log` et `rotate-key` le prennent sur un répertoire qui contient un enregistrement complet. Avant de créer le fichier de verrou, ils refusent un répertoire qui ne contient aucun enregistrement complet, deux enregistrements complets à la tête, ou un fichier d'enregistrement situé à la tête ou au-dessus qui renferme un autre enregistrement que celui annoncé par son nom. Sous le verrou, ils lisent chaque enregistrement du répertoire, puis comparent leur clé, et `create` la base, à la tête du journal : un refus décidé là (`log_forked`, `log_discontinuous` ou `record_name_mismatch` sous la tête, `key_id_mismatch`, `database_truncated`, `database_diverged` par exemple) laisse le fichier de verrou dans un répertoire qui n'en avait pas, par exemple un répertoire restauré à partir d'une copie de ses enregistrements.
- Un fichier portant un nom d'enregistrement qui ne s'analyse pas comme un enregistrement est un **enregistrement incomplet** : signalé, ignoré pour la chaîne, conservé. C'est ce que laisse une écriture interrompue.
- Un fichier portant un nom d'enregistrement qui contient un enregistrement complet **autre que celui annoncé par son nom** ne peut pas venir d'une écriture interrompue : le refus est `record_name_mismatch`. `verify`, `anchor advance`, `anchor approve-rotation`, `export`, `create` lorsqu’il prolonge un journal existant et `rotate-key` refusent un répertoire qui en contient un, et `create --previous-record` refuse un fichier qui ne porte pas le nom de l'enregistrement qu'il contient, avant toute création.
- La tête du journal est l'enregistrement complet d'index le plus élevé. Deux enregistrements complets au même index forment un fork (`log_forked`).
- Un enregistrement complet vaut par ce qui le rattache à un enregistrement de confiance, non par sa présence dans le répertoire. `create` et `rotate-key` lisent chaque enregistrement sous le verrou d'écriture et n'écrivent qu'après une tête dont le répertoire contient, en dessous d'elle, la chaîne que cette tête engage : à chaque index inférieur, l'unique enregistrement complet dont l'empreinte de déclaration est le `previous_statement_sha256` de l'enregistrement du dessus. Ils refusent un second enregistrement complet au même index (`log_forked`), un autre enregistrement que celui que la chaîne désigne (`log_discontinuous`, avec `detail.log_index`) et un enregistrement complet situé sous un index qui n'en contient aucun (`log_discontinuous`, avec `detail.missing_log_index` et `detail.unattached_log_index`). Un répertoire peut commencer à n'importe quel index : rien n'est exigé sous son premier enregistrement.
- `create` ne signe qu'une base qui a le genesis de contrôle désigné par le checkpoint le plus récent que le répertoire contient à la tête du journal ou en dessous, et dont le genesis de contrôle ne se trouve pas après la dernière position signée ; `create --previous-record`, qui ne lit que cet enregistrement, n'applique la première règle que si l'enregistrement est un checkpoint. Le refus est `scope_mismatch` avec `detail.field` égal à `control_genesis`.
- Un répertoire contient au plus 8 192 fichiers d'enregistrement, plus 16 réservés aux enregistrements de rotation. `create --previous-record` poursuit dans un nouveau répertoire dont le premier enregistrement se lie au dernier enregistrement du répertoire plein.

### Exemple à réponse connue

Clé : RFC 8032, section 7.1, TEST 1 (clé publique `d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a`).

| Valeur | Résultat |
|---|---|
| `key_id` | `21fe31dfa154a261626bf854046fd2271b7bed4b6abe45aa58877ef47f9721b9` |
| `public_key` | `11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo` |
| `statement_sha256` | `e6ae63c790cdb627184eae9456d121da689c6b5b23eee1e20854817c7b2ccabf` |
| `signature` | `ZQQAfRJ4nYrjgO-L60YjnJIQwE-OHBaA0BV5gkJirThk3D0QmLzD6q7tHExkM_MpEWQJAoC5PDhazeF2T24cAA` |
| Nom de fichier | `cp-00000000000000000002-e6ae63c790cdb627184eae9456d121da689c6b5b23eee1e20854817c7b2ccabf.json` |

Déclaration (772 octets, une ligne) :

```json
{"type":"ai-guard.audit.checkpoint","version":1,"kind":"checkpoint","tenant_id":"tenant-a","namespace_uuid":"6f1c2a9e-3b7d-4c5e-8f90-1a2b3c4d5e6f","log_index":2,"previous_statement_sha256":"1111111111111111111111111111111111111111111111111111111111111111","created_at":"2026-11-10T09:00:00.000Z","signing_key_id":"21fe31dfa154a261626bf854046fd2271b7bed4b6abe45aa58877ef47f9721b9","audit_schema_version":11,"chain_algorithm":"agentsandbox:audit:v1","event_count":42,"head_hash":"2222222222222222222222222222222222222222222222222222222222222222","control_genesis_seq":1,"control_genesis_hash":"3333333333333333333333333333333333333333333333333333333333333333","previous_event_count":17,"previous_head_hash":"4444444444444444444444444444444444444444444444444444444444444444"}
```

Le test unitaire `statement_bytes_are_deterministic_and_match_the_frozen_vector` asserte ces valeurs.

## Ancre de confiance

Écrite sur une ligne suivie d'un saut de ligne, au plus 262 144 octets. Membres, dans l'ordre d'écriture :

```json
{"format":"ai-guard.audit.trust-anchor.v1","revision":3,"previous_anchor_sha256":"<sha256>","tenant_id":"tenant-a","namespace_uuid":"<uuid>","keys":[{"key_id":"<sha256>","algorithm":"ed25519","public_key":"<base64url>","valid_from_log_index":1,"valid_through_log_index":null,"approval":"initial"}],"reference":{"log_index":5,"statement_sha256":"<sha256>","checkpoint":{"log_index":5,"event_count":42,"head_hash":"<sha256>"},"retained_checkpoints":[]},"updated_at":"<heure>"}
```

| Membre | Règle |
|---|---|
| `revision` | À partir de 1. Chaque changement est un nouveau fichier portant la révision suivante. |
| `previous_anchor_sha256` | SHA-256 des octets exacts du fichier de la révision précédente ; `null` si et seulement si `revision` vaut 1 |
| `tenant_id`, `namespace_uuid` | Le seul périmètre que cette ancre vérifie |
| `keys` | 1 à 512 clés épinglées, chacune avec un `key_id` égal au SHA-256 de `public_key`. Une seule clé, la dernière, a `valid_through_log_index` égal à `null` : la clé courante. |
| `keys[].valid_from_log_index`, `valid_through_log_index` | Positions du journal auxquelles la clé peut signer un checkpoint. Une clé retirée s'arrête une position avant son enregistrement de rotation. |
| `keys[].approval` | `initial`, `endorsed_rotation` ou `operator_recovery` |
| `reference` | `null` tant qu'aucun enregistrement n'est accepté. Sinon, le dernier enregistrement accepté. |
| `reference.log_index`, `statement_sha256` | L'enregistrement accepté. Un journal doit contenir exactement cet enregistrement à cet index. |
| `reference.checkpoint` | La dernière position d'audit signée à l'enregistrement de référence ou avant lui, avec l'index du checkpoint qui l'a signée |
| `reference.retained_checkpoints` | Au plus 64 positions antérieures, de la plus ancienne à la plus récente, que la référence a dépassées **sans comparaison avec une base**. Une vérification avec une base compare chacune d'elles. Vidée par un `anchor advance` ou un `anchor approve-rotation` dont la base contenait chacune d'elles ; `anchor advance --data` le fait même si aucun enregistrement n'est nouveau, dans une révision qui garde la même référence. |

L'ancre est un document public. Sa valeur vient de l'endroit où elle est conservée.

## Bundle de preuve

Écrit par `export` sur une ligne suivie d'un saut de ligne, au plus 20 Mio et 8 208 entrées :

```json
{"format":"ai-guard.audit.checkpoint-proof.v1","tenant_id":"tenant-a","namespace_uuid":"<uuid>","first_log_index":1,"last_log_index":5,"entries":[<enregistrement>,…],"exported_at":"<heure>","notice":"…"}
```

`entries` contient des enregistrements complets aux index contigus de `first_log_index` à `last_log_index`, chacun octet pour octet tel que dans son fichier. Les membres d'en-tête répètent ce que disent les entrées ; ils ne sont pas authentifiés, et un lecteur refuse un bundle dont l'en-tête contredit les entrées (`bundle_malformed`). Un bundle ne porte aucune clé.

## Vérification

À partir d'une ancre, d'un ensemble d'enregistrements et éventuellement d'une base, un vérificateur :

1. Lit l'ancre. Son tenant et son namespace sont le seul périmètre : chaque enregistrement vérifié aux étapes 3 à 5 doit les porter (`scope_mismatch`).
2. Avec une référence : refuse un journal dont la tête précède la référence (`log_rolled_back`), puis authentifie les enregistrements situés à la référence et en dessous par la chaîne que l'enregistrement accepté engage, sans vérifier de nouveau leurs signatures. En descendant depuis l'index de référence, chaque enregistrement présent doit être l'unique enregistrement complet de son index (`log_forked`) et avoir l'empreinte de déclaration attendue : `reference.statement_sha256` à l'index de référence, puis le `previous_statement_sha256` de l'enregistrement du dessus (`log_diverged_from_reference`, avec `detail.log_index`). La descente s'arrête au premier index qui ne contient aucun enregistrement complet. Un ensemble peut commencer là, ou après la référence : un répertoire de continuation et un export partiel ne contiennent rien sous leur premier enregistrement. Un enregistrement complet situé sous cet index n'est rattaché à la référence par aucune chaîne et est refusé (`log_discontinuous`, avec `detail.missing_log_index` et `detail.unattached_log_index`). La marche de l'étape 3 commence ensuite à l'index qui suit la référence. Sans référence : commence à l'index 1.
3. Pour chaque index suivant jusqu'à la tête du journal, exige exactement un enregistrement complet (`log_discontinuous`, `log_forked`), lié au précédent par `previous_statement_sha256` (`log_discontinuous`) et rappelant la dernière position signée de confiance dans `previous_event_count` et `previous_head_hash` (`checkpoint_regression`).
4. S'arrête à un enregistrement `key_rotation` : son successeur n'est pas de confiance tant que le détenteur de l'ancre ne l'a pas approuvé (`untrusted_key` avec `record_kind: key_rotation`). Une approbation épingle l'enregistrement de rotation comme référence de la révision suivante.
5. Pour un enregistrement `checkpoint`, exige exactement les signataires requis (`signature_invalid`), chacun épinglé dans l'ancre (`untrusted_key`), valide à cet index (`key_not_valid_at_index`), et chaque signature valide sur le message signé avec la clé publique épinglée (`signature_invalid`). Le checkpoint doit ensuite désigner le genesis de contrôle du checkpoint de confiance qui le précède : le dernier vérifié à cette étape, ou sinon le checkpoint le plus récent situé à la référence ou en dessous lorsque l'ensemble le contient sur la chaîne de l'étape 2 (`scope_mismatch` avec `detail.field` égal à `control_genesis`). Lorsque l'ensemble ne contient aucun checkpoint antérieur, le genesis antérieur est inconnu, puisqu'une ancre n'en porte aucun : la seule exigence est alors que `control_genesis_seq` ne dépasse pas l'`event_count` de la dernière position signée de confiance.
6. Avec une base : la lit sur un instantané en lecture seule avec les validateurs d'intégrité existants, exige le tenant et le namespace de l'ancre (`scope_mismatch`), puis le genesis de contrôle que les enregistrements de confiance ont signé (`scope_mismatch` avec `detail.field` égal à `control_genesis`) : la base doit désigner le genesis du checkpoint de confiance le plus récent au sens de l'étape 5, connu même si aucun enregistrement n'est vérifié à cette exécution dès lors que l'ensemble contient un checkpoint à la référence ou en dessous ; et, qu'un enregistrement disponible désigne ou non le genesis signé, le genesis de la base ne doit pas se trouver après la plus ancienne position signée de confiance. Il exige ensuite que la base contienne **chaque position signée de confiance**, de la plus ancienne à la plus récente : les positions retenues par l'ancre, sa position de référence, et la position de chaque checkpoint vérifié à l'étape 5 (`database_truncated`, `database_diverged`). Une position est contenue lorsque la base compte au moins `event_count` événements et que l'empreinte de chaîne de l'événement numéro `event_count` est `head_hash`.

Le verdict est `verified` lorsqu'aucune étape n'a refusé. Sans base, le verdict ne couvre que les enregistrements. Une ancre ne porte aucun genesis de contrôle : lorsque l'ensemble ne contient aucun checkpoint à la référence ou en dessous et qu'aucune base n'est comparée, un checkpoint qui désigne une autre empreinte à une séquence admise par l'étape 5 ne se distingue pas de celui qui a été signé.

## Résultats

| Document | `format` | Écrit par |
|---|---|---|
| Résultat de commande | `ai-guard.audit.checkpoint-command.v1`, avec `command` et les membres du résultat | Toute commande réussie autre que `verify` |
| Rapport de vérification | `ai-guard.audit.checkpoint-verification.v1` | `verify`, et `anchor advance` lorsque la vérification refuse |
| Diagnostic | `ai-guard.audit.checkpoint-diagnostic.v1` | `diagnose` |
| Erreur | `ai-guard.audit.checkpoint-error.v1`, avec `errors` | Toute commande qui n'a pas pu aboutir |

Une entrée d'erreur est `{"code":"…","message":"…","detail":{…},"action":"…"}`. Les codes et leur code de sortie figurent dans le [guide](README.fr.md#erreurs).

Le résultat de `anchor advance` porte `status` : `advanced` (une nouvelle révision dont la référence est la tête vérifiée), `confirmed` (aucun enregistrement n'est nouveau ; une nouvelle révision de même référence, écrite parce que la base comparée contenait chaque position retenue, qu'elle ne retient donc plus) ou `unchanged` (aucun fichier écrit). `anchor` contient la révision écrite, ou `null` lorsque le statut est `unchanged`. Le résultat de `anchor approve-rotation` est la révision écrite, avec le statut `rotation_approved`. Une approbation contrôle la chaîne des enregistrements, les signatures des checkpoints et le genesis de contrôle jusqu'au premier enregistrement de rotation après la référence, vérifie l'endossement de la clé sortante (ou l'approbation explicite de récupération) et la preuve de possession du successeur, puis, si `--data` est fourni, compare chaque position signée de confiance avec la base. Elle n'examine aucun enregistrement après cette rotation : ses `warnings` portent `records_after_rotation_not_examined` lorsque des enregistrements complets se trouvent après la rotation approuvée. Ses `warnings`, comme ceux d'un rapport de vérification, portent aussi `incomplete_records_present` lorsque le répertoire contient des fichiers d'enregistrement qui ne sont pas des enregistrements complets, et `incomplete_record_above_head` seulement lorsque l'un d'eux se trouve au-dessus du dernier enregistrement complet ; le résultat ne liste pas ces fichiers : les rapports de `verify` et de `diagnose` le font, dans `incomplete_records`.

Membres d'un rapport de vérification : `format`, `verdict` (`verified` ou `refused`), `summary`, `scope`, `anchor` (`revision`, `sha256`), `reference`, `rollback_detection` (`anchor_reference` ou `unavailable_no_reference`), `entries_checked`, `signatures_verified`, `verified_through_log_index`, `head`, `database` (`compared`, `event_count`, `head_hash`, `audit_schema_version`, `positions_compared`, `extends_checkpoint`, `events_after_checkpoint`), `incomplete_records` (`name`, `reason`, `holds_another_record`), `unexpected_entries`, `warnings`, `limits`, `errors`. Un membre dont la valeur est inconnue vaut `null`, jamais zéro.
