# Formats de la rétention de l’audit

[English](FORMATS.md) · [Français](FORMATS.fr.md) · [Guide](README.fr.md)

Chaque document est du JSON UTF-8. Les champs inconnus sont refusés. Les empreintes sont des SHA-256 en hexadécimal minuscule.

## Répertoire d’archive

Une archive est un répertoire créé exclusivement, en mode `0700`, sous un parent privé existant. Il contient deux fichiers, chacun en `0600` :

| Fichier | Contenu |
|---|---|
| `events.jsonl` | Un événement archivé par ligne, terminée par `\n`, par séquence croissante, sans trou |
| `manifest.json` | Le manifeste ci-dessous |

L’archive couvre une plage contiguë `first_seq..=last_seq`, événements structurels compris. ai-guard ne réécrit ni ne supprime jamais un de ces fichiers.

### Événement archivé (`ai-guard.audit.archive-events.v1`)

| Champ | Sens |
|---|---|
| `seq` | Position dans la chaîne |
| `id` | Identifiant de l’événement |
| `received_at` | Heure de réception enregistrée par le contrôle |
| `prev_hash` | Hash de la position précédente |
| `hash` | `SHA-256("agentsandbox:audit:v1\0" ‖ seq sur 8 octets gros-boutiste ‖ received_at ‖ 0 ‖ prev_hash ‖ 0 ‖ payload ‖ 0)` |
| `payload` | Le JSON exact de l’événement stocké, en une chaîne : les octets hachés |

### Manifeste (`ai-guard.audit.archive.v1`)

| Champ | Sens |
|---|---|
| `format` | `ai-guard.audit.archive.v1` |
| `archive_id` | UUID de cette archive |
| `purpose` | `retention` (un segment du registre) ou `export` (aucune récupération) |
| `tenant_id`, `control_namespace_uuid`, `control_genesis_seq`, `control_genesis_hash` | Identité de la chaîne du tenant |
| `backend` | `sqlite` (ou `mysql` une fois servi) |
| `segment_index` | Segment, à partir de 1, d’une archive de rétention ; `0` pour un export |
| `first_seq`, `start_prev_hash`, `last_seq`, `last_hash` | La plage et les hashes qui la lient à la chaîne |
| `floor_at_last` | Heure de réception maximale jusqu’à `last_seq`, telle que le plancher temporel vivant l’enregistre |
| `event_count`, `payload_bytes` | Événements archivés et somme des tailles de leurs charges utiles |
| `events_file`, `events_sha256`, `events_bytes` | `events.jsonl`, son empreinte et sa taille |
| `previous_manifest_sha256` | Empreinte du manifeste du segment précédent ; `null` pour le premier segment et pour les exports |
| `reclaim` | Rétention seulement : `classes`, `reclaimed_events`, `reclaimed_bytes`, `reclaim_sha256`, `by_class`, `retained_structural`, `retained_pins`, `kept_positions` (événements récupérables conservés pour les points de contrôle signés ou le rejeu egress) |
| `snapshot_sha256` | Rétention seulement : empreinte de l’instantané persisté des vues historiques à `last_seq` |
| `policy_sha256` | Rétention seulement : empreinte de la politique |
| `created_at` | Déclaré par le producteur ; pas un horodatage de confiance |
| `producer` | `component` et `version` du binaire |

`reclaim_sha256` vaut `SHA-256("ai-guard:audit-retention-reclaim:v1\0" ‖ chaque seq récupérée sur 8 octets gros-boutiste, par ordre croissant)`. Le registre vivant enregistre le SHA-256 des octets exacts de `manifest.json`.

## Enregistrements dans la chaîne

### `control.audit_retention.genesis`

`data` : `version` (1), `protocol` (`audit_retention_v1`), `control_namespace_uuid`, `previous_seq`, `previous_hash` (la tête de chaîne immédiatement avant le marqueur).

### `control.audit_retention.applied`

`data` (`RetentionAppliedV1`) : `version`, `protocol`, `control_namespace_uuid`, `segment_index`, `first_seq`, `last_seq`, `last_hash`, `floor_at_last`, `archive_id`, `manifest_sha256`, `events_sha256`, `archived_events`, `archived_bytes`, `classes`, `reclaimed_events`, `reclaimed_bytes`, `reclaim_sha256`, `reclaimed_by_class`, `retained_structural`, `retained_pins`, `kept_positions`, `snapshot_sha256`, `policy_sha256`, `previous_manifest_sha256`, `previous_retention_seq` et `previous_retention_hash` (le marqueur de genèse pour le segment 1, sinon l’enregistrement appliqué précédent), `previous_seq` et `previous_hash` (la tête de chaîne immédiatement avant cet enregistrement).

Les nouveaux enregistrements incluent `apply.plan_sha256` et `apply.usage_before` (`stored_events`, `stored_bytes`). Le chaînage lie le plan et permet à `apply` de retrouver le reçu d’origine à partir d’une archive vérifiée. Le format d’archive reste identique. Un enregistrement sans `apply` reste vérifiable et ne permet pas de restituer le reçu d’application.

## Documents des commandes

| `format` | Affiché par |
|---|---|
| `ai-guard.audit.retention-command.v1` | `install`, `plan`, `apply`, `export`, avec `command` qui nomme la sous-commande |
| `ai-guard.audit.retention-verification.v1` | `verify` : `verdict` (`verified` ou `refused`), `segments`, `segments_verified`, `archived_through_seq`, `reclaimed_events`, `exports_verified`, `stored_events`, `live_head_seq`, `live_head_hash`, `warnings`, `failure` (`code`, `message`, `detail`) et `limits` |
| `ai-guard.audit.retention-error.v1` | Tout refus ou erreur : `code`, `message`, `detail` |

L’empreinte du plan `plan_sha256` vaut `SHA-256("ai-guard:audit-retention-plan:v1\0" ‖ JSON du plan)` ; `apply --expect-plan` l’exige.
