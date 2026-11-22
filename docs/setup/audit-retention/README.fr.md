# Rétention de l’audit

[English](README.md) · [Français](README.fr.md) · [Index de configuration](../README.fr.md)

Ce guide décrit la commande de maintenance hors ligne `agentsandbox audit-retention`. Elle archive un préfixe clos de la chaîne d’audit d’un tenant dans un **nouveau** répertoire privé, vérifie cette archive, puis libère de la capacité dans la base vivante **sans rompre la chaîne** : la base vivante et ses archives se vérifient ensemble, de bout en bout. Les fichiers exacts sont décrits dans la [référence des formats](FORMATS.fr.md). Ce guide est un contrat ; l’état d’implémentation et ses preuves sont suivis dans l’[ingénierie](../../engineering/CURRENT_STATE.md).

## Déploiements couverts

| Déploiement | Ce que fournit ce guide | Preuve exécutée |
|---|---|---|
| Base SQLite d’un tenant au format 13, sur des chemins hôte du compte de contrôle | Toutes les commandes ci-dessous | Tests logiciels sur macOS ARM64 uniquement (voir l’ingénierie) |
| Base SQLite au format 14 ou 15 (sources humaines possédées) | **Pas encore servie** : ces bases s’ouvrent par l’union des sources possédées, que cette commande n’ouvre pas | `NOT_RUN` |
| Base MySQL d’un tenant (mot de passe, ou mot de passe et mTLS) | **Pas encore servie** : la parité est un lot distinct encore ouvert | `NOT_RUN` |
| Installation Compose dont la base est dans le volume `audit_data` | Un CLI hôte ne peut pas ouvrir ce volume ; aucune procédure de conteneur ponctuel n’est encore fournie | `NOT_RUN` |

Rien ici ne tourne comme service, ne contacte un réseau ni ne supprime une archive.

## Pourquoi la rétention est nécessaire

Une base de tenant accepte au plus `--max-events` événements (100 000 par défaut ; au plus 1 000 000) et `--audit-max-bytes` de charges utiles (256 Mio par défaut ; une base SQLite peut aller jusqu’à 2 Gio, au prix d’une validation complète plus longue à chaque démarrage et après une modification externe ; MySQL reste à 256 Mio). Une fois ce budget épuisé, tout ajout est refusé : les lancements et les événements de contrôle s’arrêtent jusqu’à l’intervention d’un opérateur. La rétention est cette intervention. Elle retire de la base vivante d’anciens événements **feuilles** après les avoir archivés ; rien n’est jamais perdu silencieusement.

## Ce qui est récupéré, et ce qui reste toujours

| Classe d’événement | Exemples | Récupéré ? |
|---|---|---|
| `ordinary` | faits du collecteur et du runner : `command.*`, `runtime.*`, `workspace.*`, `observer.coverage`, `agent.*`, `file.*`, sorties des fournisseurs | Oui, si la politique liste `ordinary` |
| `human_read` | l’audit de chaque lecture humaine : `control.identity.access`, `control.identity.brokers.access`, `control.identity.workers.access` | Oui, si la politique liste `human_read` |
| Structurel | tout événement `control.*` autre que les trois ci-dessus, `security.decision`, `worker.*`, `usage.allocation`, faits de retrait des brokers, faits du canal de relais | **Jamais** : des projections les référencent et la base les rejoue à chaque démarrage |

Dans une plage récupérée, la base vivante conserve aussi :

- la **borne** : le dernier événement de chaque segment, pour que la chaîne vivante reprenne depuis un hash connu ;
- la **première mention** de chaque session, agent, sandbox et projet : l’événement dont une attribution d’identité a besoin pour prouver que la ressource existe ;
- chaque **position d’un point de contrôle signé** des répertoires passés avec `--checkpoint-dir`, pour que les [points de contrôle signés](../audit-signatures/README.fr.md) continuent de comparer la base à chaque position signée.

Les événements du cycle de vie des workers sont structurels. Dans les bases mesurées pendant le développement, ils représentaient la majorité des octets. La récupération des événements des workers clos est un lot distinct encore ouvert : d’ici là, la rétention repousse l’épuisement de la capacité sans supprimer la croissance due aux cycles de vie des workers.

## Ce que la base vivante enregistre

- **Le marqueur de capacité** `control.audit_retention.genesis`, ajouté une fois par `install`.
- **Un événement `control.audit_retention.applied` par segment**, dans la chaîne elle-même. Il désigne l’archive (`archive_id`, SHA-256 de son manifeste et de ses événements), la plage, le hash de borne, les nombres et octets récupérés par classe, et le segment précédent. Les points de contrôle signés pris ensuite signent donc aussi l’enregistrement de rétention.
- **La table de registre** `audit_retention_segments`, comparée à ces événements à chaque démarrage.
- **Un instantané** des vues historiques du dashboard (agents, sandboxes, compteurs de synthèse) à chaque borne, pour que ces vues continuent de montrer l’historique complet.

Le numéro de format de la base ne change pas. Un binaire antérieur à la rétention refuse une base qui la porte.

## Politique

Il n’existe aucune politique implicite. Créer un fichier privé (`0600`, au plus 16 Kio) :

```json
{"version":1,"min_age_seconds":2592000,"keep_recent_events":10000,"max_segment_events":250000,"classes":["ordinary","human_read"]}
```

| Champ | Sens | Bornes |
|---|---|---|
| `min_age_seconds` | Seuls les événements dont l’heure de réception maximale précédente a au moins cet âge | 86 400 (24 heures) ou plus |
| `keep_recent_events` | Événements les plus proches de la tête, jamais inclus dans un segment | 1 à 1 000 000 |
| `max_segment_events` | Plus grand segment, en événements archivés | 1 à 1 000 000 |
| `classes` | `ordinary`, `human_read` ou les deux, dans cet ordre | au moins une |

Le plancher de 24 heures existe parce qu’un identifiant d’événement récupéré n’est plus dans la base vivante : un producteur qui le réessaierait plus tard ne serait pas reconnu comme doublon. Après une rétention, la base refuse tout événement ordinaire déclaré à l’horizon archivé ou avant (`event_predates_archive`), et `verify` signale un identifiant réutilisé (`duplicate_event_id`).

## Procédure

Exécuter chaque commande avec le compte de contrôle et les mêmes `--max-events` et `--audit-max-bytes` que le gateway. `install` et `apply` exigent `--confirm-control-stopped` : arrêter d’abord le gateway, le dashboard et les brokers du tenant. C’est une attestation de l’opérateur, pas un fencing à l’exécution ; les commandes tiennent aussi le verrou d’écriture SQLite pendant toute leur transaction.

1. **Facultatif, recommandé avec les points de contrôle signés.** Signer la tête courante (`ai-guard-audit-checkpoint create`) et laisser le détenteur de l’ancre exécuter `anchor advance --data`.
2. **Installer une fois**, avec un nouveau chemin de sauvegarde :

   ```sh
   agentsandbox audit-retention install --data "$STORE" --tenant-id "$TENANT" \
     --backup "$BACKUPS/before-retention.sqlite" --confirm-control-stopped
   ```

3. **Planifier.** Rien n’est écrit :

   ```sh
   agentsandbox audit-retention plan --data "$STORE" --tenant-id "$TENANT" --policy "$POLICY" \
     --checkpoint-dir "$CHECKPOINTS"
   ```

   `plan` et `apply` exigent un choix explicite : passer chaque répertoire de points de contrôle du tenant (`--checkpoint-dir`, répété pour les répertoires de continuation), ou attester `--without-signed-checkpoints`. Sans l’un des deux, la commande refuse. Un événement récupéré à une position signée ferait refuser cette position par `ai-guard-audit-checkpoint verify --data` avec `database_truncated`.

   Relire `plan.first_seq`, `plan.last_seq`, `plan.reclaimed_events`, `plan.reclaimed_by_class`, `usage_before` et `usage_after_reclaim`. Conserver `plan_sha256`.
4. **Appliquer** le plan relu dans un nouveau répertoire, sous un parent privé existant :

   ```sh
   agentsandbox audit-retention apply --data "$STORE" --tenant-id "$TENANT" --policy "$POLICY" \
     --through-seq "$LAST_SEQ" --expect-plan "$PLAN_SHA256" --checkpoint-dir "$CHECKPOINTS" \
     --archive-dir "$ARCHIVES/segment-1" --confirm-control-stopped
   ```

   `apply` recalcule le plan dans sa transaction et refuse avec `plan_mismatch` si quoi que ce soit a changé. Il écrit et relit l’archive avant de retirer quoi que ce soit.
   Si la réponse se perd ou si `retention_commit_unconfirmed` apparaît, conserver l’archive et relancer la même commande. Une fois le plan enregistré et l’archive vérifiés, la commande restitue le reçu d’origine sans effectuer une nouvelle récupération. Si le reçu reste indéterminé, exécuter `verify` avec l’archive et examiner son résultat avant de planifier un nouvel `apply`.
5. **Préserver l’archive.** Copier le répertoire d’archive vers deux emplacements durables et noter son `manifest_sha256` par un autre canal. Supprimer une archive rend son préfixe invérifiable.
6. **Vérifier**, puis redémarrer le plan de contrôle :

   ```sh
   agentsandbox audit-retention verify --data "$STORE" --tenant-id "$TENANT" \
     --archive-dir "$ARCHIVES/segment-1"
   ```

7. **Facultatif.** Signer la nouvelle tête, qui inclut désormais l’enregistrement de rétention.

Relancer `plan` et `apply` pour chaque segment suivant ; chaque nouvelle archive se chaîne à la précédente.

## Vérifier de bout en bout

`verify` ne fait que lire. Il contrôle :

- la chaîne vivante, y compris le préfixe clairsemé : chaque événement conservé se hache sur lui-même, chaque segment conserve exactement ses événements comptés et sa borne ;
- que chaque segment a son archive, dont le registre vivant enregistre l’empreinte du manifeste ;
- que chaque archive est une chaîne valide depuis la fin de la précédente, en partant du hash nul ;
- que chaque événement conservé est identique octet pour octet à sa copie archivée et que chaque événement absent est un événement que le segment avait le droit de récupérer (`unaccounted_loss` sinon) ;
- le décompte de la récupération et les instantanés des vues persistés, recalculés depuis les archives ;
- qu’aucun événement vivant ne réutilise l’identifiant d’un événement récupéré.

Une archive transmise à `verify` mais absente du registre est signalée par l’avertissement `orphan_archive` : c’est ce que laisse un `apply` interrompu, et rien de ce qu’elle contient n’a été récupéré.

Pendant que le gateway tourne, `agentsandbox verify-audit --archive-dir A1 --archive-dir A2 …` demande d’abord au gateway son contrôle d’intégrité, qui couvre la chaîne vivante, son préfixe clairsemé et l’engagement de chaque segment sur les lignes conservées (`retained_sha256`). Il vérifie ensuite chaque archive contre le registre de rétention que le gateway rapporte : chaîne depuis le hash nul, empreintes des manifestes, décompte de la récupération et instantanés des vues. Sur une base qui a des segments, `verify-audit` échoue si une archive manque.

## Export vérifiable

`export` écrit le même format d’archive pour une plage vivante contiguë, sans rien récupérer, dans un nouveau répertoire. `verify --archive-dir` la contrôle ensuite contre la base vivante. L’utiliser pour remettre à un auditeur une copie vérifiable de l’historique récent.

```sh
agentsandbox audit-retention export --data "$STORE" --tenant-id "$TENANT" --archive-dir "$EXPORTS/2026-11"
```

## Résultats et erreurs

Chaque commande affiche un document JSON. Le code de sortie `0` est un succès, `2` un refus avec un code fermé (rien n’a changé dans la base vivante), `1` une erreur opérationnelle.

| Code | Sens | Que faire |
|---|---|---|
| `retention_capability_missing` | `install` n’a pas été exécuté | Exécuter `install` |
| `retention_capability_present` | `install` a déjà été exécuté | Rien |
| `retention_requires_format` | La base n’est pas au format 13 à 15 | Migrer d’abord |
| `policy_invalid` | La politique est mal formée, conserve moins de 24 heures ou liste les classes dans le désordre | Corriger la politique |
| `nothing_to_reclaim` | Aucun événement de la plage admise n’est encore récupérable | Attendre, ou revoir la politique |
| `range_invalid` | `--through-seq` est hors de la plage admise | Planifier à nouveau |
| `plan_mismatch` | La base a changé depuis la relecture du plan | Planifier à nouveau |
| `archive_destination_not_new` | Le chemin d’archive existe | Choisir un nouveau chemin |
| `archive_destination_unavailable` | Le parent manque ou n’est pas un répertoire privé | Créer un parent privé (`0700`) |
| `archive_destination_reserved` | L’archive partage le parent physique de SQLite et son nom est réservé par SQLite ou contient des caractères hors ASCII | Choisir un nom ASCII en dehors de l’espace de noms SQLite ou un autre parent privé |
| `archive_write_failed`, `archive_unverified` | L’archive n’a pas pu être écrite ou vérifiée | Conserver le répertoire pour le diagnostic et exécuter `verify` avant un nouvel `apply` |
| `retention_commit_unconfirmed` | L’archive est complète et le commit de la base reste indéterminé | Conserver l’archive et relancer la même commande pour vérifier le reçu |
| `reclaim_referenced_event` | Une projection référence un événement choisi pour la récupération | Rien n’a été récupéré ; le signaler comme défaut |
| `checkpoint_log_unavailable` | Un `--checkpoint-dir` est illisible ou appartient à un autre tenant | Rien n’a changé ; corriger le chemin |
| `snapshot_too_large` | L’instantané des vues historiques dépasse 8 Mio | Rien n’a été récupéré |
| `event_predates_archive` | Un ajout ordinaire déclaré à l’horizon archivé ou avant | Le producteur doit envoyer un nouvel événement |
| `archive_missing`, `archive_manifest_mismatch`, `archive_chain_invalid`, `archive_digest_mismatch`, `retained_event_diverges`, `unaccounted_loss`, `duplicate_event_id`, `snapshot_mismatch`, `export_diverges` | Refus de `verify` | Restaurer l’archive depuis une copie ; enquêter avant toute nouvelle rétention |

## Limites

- La chaîne vivante n’est pas signée. Face à qui peut réécrire à la fois la base vivante et ses archives, seuls les [points de contrôle signés](../audit-signatures/README.fr.md) avec une ancre indépendante détectent une réécriture cohérente.
- Les audits de lecture humaine récupérés sont vérifiés comme octets chaînés ; ils ne sont pas revalidés contre la projection d’identité à leur position.
- La capacité en événements compte les événements stockés ; la longueur de la chaîne continue de croître. Les budgets d’audit des lectures humaines et des workloads repartent à chaque borne de segment.
- Cette version ne récupère jamais d’événement structurel ; voir plus haut.
- Les formats 14 et 15, MySQL et le volume Compose ne sont pas encore servis.
