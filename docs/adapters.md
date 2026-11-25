# Adaptateurs fournisseurs livrés : stdout JSONL et hooks

## Périmètre réellement implémenté

`src/adapters.rs` et ses modules `claude`, `codex` et `hooks` forment un normaliseur Rust sans accès réseau, sans accès au système de fichiers et sans exécution de commande. Il prend un enregistrement complet (ligne JSONL du stdout ou charge d'un hook officiel) et produit zéro ou plusieurs événements du modèle du gateway, avec la catégorie de l'enregistrement.

```rust
pub struct AdapterContext {
    pub session_id: String,
    pub project_id: String,
    pub sandbox_id: String,
    pub agent_id: String,
    pub correlation_id: String,
    pub trace_id: String,
}

pub enum RecordDisposition { Mapped, Ignored, Unrecognized, Oversized, Invalid }
pub struct NormalizedRecord { pub events: Vec<serde_json::Value>, pub disposition: RecordDisposition }

pub fn normalize_stdout_record(provider: &str, line: &str, context: &AdapterContext, redactor: &Redactor) -> NormalizedRecord;
pub fn normalize_hook_record(client: &str, payload: &str, context: &AdapterContext, redactor: &Redactor) -> NormalizedRecord;
pub fn parse_line(provider: &str, line: &str, context: &AdapterContext) -> Vec<serde_json::Value>;
```

Les valeurs reconnues de `provider` sont `codex`, `claude`, `claude-code` et `generic` ; celles de `client` sont `claude-code` (alias `claude`) et `codex`. Les sources canoniques sont `claude-adapter`, `codex-adapter` et `generic-adapter`. Une valeur inconnue produit `Unrecognized` et aucun événement. `Ignored` désigne un type connu volontairement non projeté (par exemple `stream_event` ou `PermissionRequest`) ; `Oversized` et `Invalid` restent des pertes comptées par le collecteur.

Ces parseurs sont validés sur des **fixtures synthétiques écrites d'après les schémas officiels** (`tests/adapters_fixtures/SOURCES.json` liste les sources et les champs non confirmés), pas sur une session authentifiée. Les événements Claude Code et Codex portent `coverage: "native"` (canal officiel du client) et **toujours** `provenance: "declared"`. Le contrat générique conserve `coverage: "observed"`. Ni l'un ni l'autre ne transforme une déclaration du CLI en preuve indépendante de l'effet système.

## Identité et temps

Le normaliseur crée un UUID et un timestamp UTC pour chaque événement. En collecte live, le collecteur remplace ensuite l'identifiant par une valeur déterministe dérivée de la position dans le spool et le timestamp par l'heure de réception observée (`data.activity.received_at`). Dans le traitement historique de fin d'exécution, le timestamp reste l'heure de normalisation, et non l'heure réelle de chaque appel d'outil. Une éventuelle date déclarée par un agent générique reste dans `data.declared_timestamp`.

Les six identifiants du contexte proviennent du runner de confiance. Les champs homonymes d'un enregistrement ne peuvent pas les remplacer. Les identifiants fournisseurs sont conservés dans `data.provider_session_id`, `provider_agent_id`, `provider_turn_id`, `provider_item_id`, `provider_message_id`, `provider_uuid` ou `provider_tool_use_id` selon leur disponibilité. Les chemins de transcription internes au worker (`transcript_path`, `agent_transcript_path`) ne sont jamais conservés.

`parent_tool_use_id` de Claude désigne l'appel d'outil `Agent` (anciennement `Task`) qui a lancé un sous-agent. Le normaliseur le conserve comme `data.provider_parent_tool_use_id`. La projection décrite ci-dessous relie uniquement les parents identifiés explicitement comme agents. Un sous-agent Claude dont le parent est connu par le seul ID d'appel reste sans lien de parenté.

## Mappings du stdout (mode batch)

| Entrée | Événement normalisé | Données retenues |
|---|---|---|
| Codex `thread.started` | `session.declared` puis `agent.declared` | ID de thread ; agent racine `agent_role: "root"` sans parent. |
| Codex `turn.started` / `turn.completed` / `turn.failed` / `error` | `session.declared` | État rapporté, erreur si exposée. |
| Codex `turn.completed.usage` | `usage.reported` | Compteurs entiers présents, dont `reasoning_output_tokens`. |
| Codex `agent_message` / `reasoning` (`item.completed`) | `message.declared` / `reasoning.declared` | Texte visible ou résumé de raisonnement, borné, `text_truncated`. |
| Codex `command_execution` (`item.started` / `item.completed`) | `tool.requested` / `tool.result` | Texte de commande (chaîne d'affichage, jamais argv), statut dont `declined`, code de sortie et sortie agrégée bornée. |
| Codex `file_change` (`item.completed`) | `file.declared` | Chemins, `add`/`update`/`delete`, statut rapporté y compris échec. |
| Codex `mcp_tool_call` | `tool.requested` / `tool.result` | `mcp_server`, `mcp_tool`, arguments et résultat bornés, erreur. |
| Codex `web_search` | `tool.requested` / `tool.result` | Requête déclarée. |
| Codex `todo_list` (`item.started` / `item.updated` / `item.completed`) | `plan.declared` | Étapes et état coché. |
| Codex `collab_tool_call` | `delegation.declared`, puis à la complétion `agent.declared` par destinataire | Outil (`spawn_agent`, `send_input`, `wait`, `close_agent`), expéditeur, destinataires, consigne bornée ; un `spawn_agent` déclare le parent de chaque destinataire. |
| Codex élément `error` « event stream lagged » | `session.declared` | `provider_reported_loss: true` : perte déclarée par Codex. |
| Claude `system` / `init` | `session.declared` puis `agent.declared` | Session, modèle, répertoire, `declared_client_version`, mode de permission ; agent racine. |
| Claude `system` / `compact_boundary`, `api_retry` | `session.declared` | État `compacted` ou `api_retry`, compteurs entiers. |
| Claude blocs `text` | `message.declared` | Rôle `assistant` ou `user`, texte borné. |
| Claude blocs `thinking` / `redacted_thinking` | `reasoning.declared` | Texte borné, sans la signature ; ou `redacted: true`. |
| Claude `tool_use` | `tool.requested`, plus `delegation.declared` pour `Agent`/`Task` | Nom, ID, entrée ; `agent_type`, `description` et consigne bornée de la délégation. |
| Claude `tool_result` | `tool.result` | ID de l'appel, contenu et état d'erreur éventuel. |
| Claude `result` | `session.declared` puis éventuellement `usage.reported` | Résultat de session et compteurs effectivement présents. |

Codex n'émet sur `exec --json` que le fil principal : les commandes d'un sous-agent n'y apparaissent pas. Elles arrivent par les hooks lorsque l'image les configure. Le code de sortie rapporté par Codex reste une déclaration du fournisseur ; le résultat du processus `codex` ou `claude` est enregistré séparément par le runner.

Les fragments `stream_event` Claude sont ignorés : seuls les messages complets sont projetés, ce qui évite de reconstruire ou de masquer partiellement un secret réparti sur plusieurs fragments. L'usage des messages `assistant` n'est pas additionné. Aucun coût n'est calculé, y compris `total_cost_usd`.

## Mappings des hooks officiels (batch et TTY)

| Hook (Claude Code et Codex) | Événement normalisé | Données retenues |
|---|---|---|
| `SessionStart` | `session.declared` ; pour Claude, aussi l'agent racine | Source et modèle déclarés. |
| `UserPromptSubmit` | `message.declared` (`role: "user"`) | Consigne bornée. |
| `PreToolUse` | `tool.requested`, plus `delegation.declared` pour `Agent`/`Task` (Claude) ou les outils collab (Codex) | Nom, ID, entrée bornée, `command_text`, `mcp_server`. |
| `PostToolUse` / `PostToolUseFailure` / `PermissionDenied` | `tool.result` | Réponse bornée, `duration_ms`, erreur, interruption ou refus. |
| `SubagentStart` / `SubagentStop` | `agent.declared` (`started` / `completed`), plus le dernier message à l'arrêt | ID et type du sous-agent ; **aucun champ parent**, le hook ne le nomme pas. |
| `Stop` | `message.declared` (`turn_final: true`) | Dernier message de l'assistant pour le tour. |
| `SessionEnd`, `StopFailure`, `Notification`, `PreCompact` | `session.declared` | État, raison ou type de notification. |

Les événements définis par un client mais non projetés (`PermissionRequest`, `PreModelRequest`, `PostModelRequest`, `PostCompact`, `Interrupt`, `MessageDisplay` et les autres hooks de configuration) sont `Ignored`. Les charges des hooks sont acceptées jusqu'à 1 Mio : un champ volumineux est borné par la projection au lieu de faire perdre l'enregistrement.

Sources consultées le 21 novembre 2026 : types de `codex-rs/exec/src/exec_events.rs` et schémas `codex-rs/hooks/schema/generated` aux tags [rust-v0.165.0](https://github.com/openai/codex/tree/rust-v0.165.0) et [rust-v0.170.0](https://github.com/openai/codex/tree/rust-v0.170.0) ; [hooks Claude Code](https://code.claude.com/docs/en/hooks), [mode headless](https://code.claude.com/docs/en/headless) et [types du SDK TypeScript](https://code.claude.com/docs/en/agent-sdk/typescript). Aucune de ces sources ne promet la stabilité de ces formats ; la [matrice de couverture](#couverture-par-version-du-client) indique ce qui est qualifié.

## Contrat générique explicite v1

Un agent personnalisé peut produire une ligne de ce format :

```json
{
  "type": "agentsandbox.telemetry",
  "version": 1,
  "kind": "tool.requested",
  "command": {
    "argv": ["printf", "bonjour"],
    "cwd": "/workspace"
  },
  "data": { "tool_name": "example" }
}
```

`kind` est limité à `tool.requested`, `tool.result`, `file.declared`, `usage.reported`, `agent.declared` et `session.declared`. `data`, lorsqu'il existe, doit être un objet. Pour `usage.reported`, les compteurs se placent dans `data.usage`.

`command` est optionnel. Lorsqu'il est fourni, il exige un tableau non vide de chaînes `argv` et un `cwd` explicite non vide. L'adaptateur conserve ces arguments comme **données déclarées** ; il ne les exécute jamais. Pour une commande fournie sous forme de texte, conserver ce texte dans `data.command_text`. Le parseur ne découpe pas le shell et n'invente ni arguments ni répertoire de travail.

Un champ top-level `decision` déclaré par l'agent est seulement retenu dans `data.declared_decision`. Il ne remplit pas le champ de décision du broker. Les champs `source`, `provenance`, `coverage`, `id` et identifiants du contexte ne sont jamais acceptés comme autorités depuis le JSONL.

## Arbre des agents déclarés

Un agent personnalisé peut émettre ces deux lignes JSONL pour déclarer une racine et un enfant :

```jsonl
{"type":"agentsandbox.telemetry","version":1,"kind":"agent.declared","data":{"provider_agent_id":"main","provider_parent_agent_id":null,"name":"Coordinateur","state":"started"}}
{"type":"agentsandbox.telemetry","version":1,"kind":"agent.declared","data":{"provider_agent_id":"tests","provider_parent_agent_id":"main","name":"Tests","state":"started","task":"Vérifier le correctif"}}
```

Le contrat conserve aussi les alias historiques `data.declared_agent_id` et `data.declared_parent_agent_id`. L'adaptateur générique place dans ces alias les champs `agent_id` et `parent_agent_id` présents au premier niveau de la ligne. Il ne remplace jamais les identifiants de contexte du runner. Si un identifiant canonique et son alias se contredisent, la fiche est refusée ; pour un parent contradictoire, la fiche reste visible sans lien parent et avec un avertissement.

La projection possède cette interface :

```rust
#[derive(Default)]
pub struct DeclaredAgentProjection { /* état interne */ }

impl DeclaredAgentProjection {
    pub fn observe(&mut self, event: &Event);
    pub fn finish(self) -> Vec<serde_json::Value>;
}
```

Le collecteur lui passe les événements dans l'ordre du journal. Seuls les événements `kind: "agent.declared"`, `provenance: "declared"`, contenant un ID fournisseur valide et les trois identifiants de contexte `session_id`, `sandbox_id`, `agent_id`, créent une fiche. Les identifiants doivent être non vides, ne contenir aucun caractère de contrôle et mesurer au plus 256 octets UTF-8. Ils ne sont ni tronqués, ni simplifiés, ni normalisés ; deux identifiants Unicode distincts restent distincts.

L'identifiant de fiche vaut `declared-` suivi du SHA-256 hexadécimal du tuple JSON `[session_id, sandbox_id, context_agent_id, source, provider_agent_id]`. Cette identité reste stable lors d'un changement de nom ou d'état. Le contexte du runner et le namespace du fournisseur restent séparés. Un champ `data.id` ne peut pas choisir l'identifiant de fiche. Les mêmes IDs déclarés dans deux sessions, sandboxes, sources ou contextes d'agents produisent des fiches différentes.

Les parents sont résolus à la fin de l'agrégation, même si l'enfant a été reçu en premier. Un lien existe uniquement lorsqu'une autre fiche porte l'ID parent dans le même namespace. Aucun parent fictif ni rattachement implicite au processus supervisé n'est créé.

| Situation | `parent_id` | `parent_unresolved` | `hierarchy_warning` |
|---|---|---|---|
| Parent déclaré et reçu dans le même contexte | ID de fiche du parent | `null` | `null` |
| Parent déclaré absent de ce contexte | `null` | ID fournisseur du parent absent | Message explicatif |
| Cycle, y compris agent parent de lui-même | `null` pour chaque lien du cycle | `null` | Message explicatif |
| ID parent invalide ou alias contradictoires | `null` | `null` | Message explicatif |
| Première déclaration sans parent, ou parent explicitement `null` | `null` | `null` | `null` |

Une mise à jour sans champ parent conserve le lien précédent. Un parent explicitement `null` le retire. La suppression des liens d'un cycle conserve les descendants qui ne participent pas au cycle.

Les fiches portent toujours `provenance: "declared"`, `compatibility: "observed"` et un objet `field_provenance` dont les valeurs sont des provenances textuelles. Les données déclarées ne deviennent pas des mesures système et ne modifient pas la fiche observée du runner. Les compteurs d'agents observés et déclarés doivent être présentés séparément.

L'état est calculé uniquement depuis `data.state` : `started` donne `running`, `completed` donne `completed`, `failed` donne `error`, `stopped` donne `stopped`. Un état inconnu donne `unknown`, et un champ absent conserve l'état précédent. Le premier `started` explicite renseigne `started_at` avec le timestamp de normalisation de l'événement ; `last_activity` utilise le dernier événement reçu. Ces dates ne prouvent pas le démarrage ou l'activité d'un processus enfant. `provider`, `name`, `model` et `task`, lorsqu'ils existent, restent des métadonnées déclarées et bornées. Aucun coût ou usage n'est inventé.

Cette voie permet à un agent personnalisé de rendre son arbre visible.

Les parseurs Claude Code et Codex déclarent désormais l'agent racine de la session (`agent_role: "root"`, identifiant de session Claude ou fil racine Codex) et les sous-agents que le fournisseur nomme : `SubagentStart`/`SubagentStop` des deux clients et `collab_tool_call` de Codex. Seul un `spawn_agent` Codex nomme un parent ; un hook `SubagentStart` ne déclare aucun parent, et une mise à jour sans parent conserve le lien connu. Un sous-agent n'est jamais `observed` : ai-guard n'observe pas un sous-agent indépendamment du client qui le déclare.

### Reprise de la hiérarchie après rétention SQLite

Le lecteur legacy repart du dernier snapshot de rétention V1 et replie les événements dont la séquence dépasse sa borne. Il conserve les fiches déclarées, leurs identifiants et leurs références explicites de parent. Un lien explicite peut donc toujours être résolu si les deux fiches sont présentes dans le snapshot ou dans le journal relu. Le snapshot V1 ne conserve ni les références nécessaires à la jointure Claude, ni la distinction entre un champ parent omis et un parent explicitement `null` lorsqu'il ne reste aucun ID parent. Le lecteur ne rouvre pas les archives et ne relit pas les lignes dont la séquence est inférieure ou égale à la borne, même si certaines sont encore présentes en base. [Lecture du Store SQLite](../src/audit.rs#L2240)

**Compatibilité V1.** L'empreinte des octets du snapshot V1 est engagée dans le registre et le manifeste de rétention. Le lecteur legacy SQLite active un état de jointure éphémère pour les agents. Le vérificateur recalcule l'empreinte avec le pli V1 historique. Conserver l'état de jointure lors d'une nouvelle compaction exige un contrat de rétention versionné. [Projection partagée](../src/audit/entities.rs#L16) · [Vérification des archives](../src/audit/retention/store.rs#L1286)

La jointure exige un événement d'appel `Agent`/`Task` et un événement qui porte à la fois l'ID de l'enfant et son `provider_parent_tool_use_id`. Les deux événements doivent être déclarés et se trouver dans le suffixe relu du tenant autorisé. Avant la première compaction, ce suffixe couvre tout le journal. Les IDs d'appel doivent correspondre exactement et sans ambiguïté dans la même session, le même sandbox, le même contexte d'agent et la même source Claude. L'enfant et l'émetteur doivent chacun avoir une fiche déclarée visible. Un lien explicite, y compris celui fourni par Codex, reste prioritaire. Un `null` explicite relu fait foi. Une fiche enfant issue du snapshot sans ID parent garde un parent inconnu tant qu'une mise à jour ne déclare pas explicitement le parent ou son absence. Si une inférence ferme un cycle, le lien inféré est retiré en préservant les liens explicites valides.

Quand la jointure aboutit, la fiche reste `declared` et seuls `parent_link` et `field_provenance.parent_id` indiquent `inferred`. Si une source de jointure est absente, compactée, ambiguë ou hors droits, le parent reste inconnu en l'absence de lien explicite. La vue renvoie alors `parent_id: null`, `parent_link: null` et omet `field_provenance.parent_id` lorsqu'aucune déclaration parentale ne subsiste. Un `null` explicitement déclaré dans le suffixe conserve sa provenance `declared`. Une référence d'appel seule ne remplit pas `parent_unresolved` et ne révèle aucun ID hors périmètre. Les permissions et la portée du lecteur continuent de venir du gateway. Une erreur de lecture ou d'intégrité reste une erreur. Le compteur `declared_subagents` de la synthèse SQLite compte les fiches actives dont le parent est résolu dans cette vue.

## Limites et erreurs

| Contrôle | Limite / comportement |
|---|---|
| Taille d'un enregistrement avant parsing | 1 Mio maximum, mesurés en octets UTF-8 ; au-delà, `Oversized`. |
| Amplification en événements | 32 événements maximum par enregistrement. |
| Message, raisonnement, consigne | 8 Kio maximum mesurés après échappement JSON, coupe sur une frontière UTF-8, `text_truncated: true`. |
| Autre texte projeté | 4 096 octets maximum, coupe sur une frontière UTF-8, suffixe `[truncated]`. |
| Objet de données projeté | Budget de 12 Kio, message inclus, et vérification de la taille JSON après échappement. |
| Structure projetée | Profondeur de 8 ; 32 éléments par collection ; clés bornées. |
| Arguments génériques | Au plus 64 ; 8 192 octets cumulés ; 4 096 octets par argument. |
| JSON invalide, type inconnu, schéma incompatible | Aucun événement, sans exception ni succès fictif ; catégorie `Invalid` ou `Unrecognized`. |

Les projections tronquées d'objets portent `adapter_payload_truncated: true`. Si l'échappement JSON dépasse le budget, l'objet est remplacé par cet indicateur. Les troncatures empêchent une restitution exhaustive ; elles doivent rester visibles dans l'IHM.

Le runner émet un **bilan de couverture par session** ; en collecte live, c'est l'événement observé `activity.coverage` décrit ci-dessous. Le normaliseur ne produit pas un événement d'erreur pour chaque ligne de bruit ; il retourne sa catégorie au collecteur, qui la compte. Une ligne ignorée ne signifie pas qu'aucune action n'a eu lieu.

Le masquage des secrets est appliqué aux valeurs JSON décodées avant toute projection, avec le dictionnaire des credentials de la session et les motifs connus, puis de nouveau par le serveur avant stockage. Les parseurs ne prétendent pas détecter tous les secrets : un secret inconnu affiché par un outil peut être conservé dans un contenu. Les données normalisées restent non fiables et doivent être rendues comme texte dans le frontend.

## Collecte live

`src/provider_activity/` collecte l'activité pendant la session, au lieu de la normaliser après la fin :

1. **Canaux.** En batch supervisé par le broker, le stdout du client est découpé en enregistrements complets au fil de l'eau, en plus de la capture terminale bornée existante. Le lancement géré hors broker (`run`) ne lit pas encore son stdout en direct : ce stdout garde la normalisation de fin d'exécution, tandis que ses hooks sont collectés en direct, avec les budgets par défaut. En batch comme en TTY, les hooks officiels configurés sur la racine en lecture seule de l'image appellent `ai-guard-activity`, qui transmet la charge au broker par un canal `exec` dédié, sans credential ni réseau.
2. **Durabilité.** Chaque enregistrement est masqué, normalisé puis ajouté à un spool local au broker, append-only et chaîné par SHA-256, avec `sync_data` avant l'accusé de réception. Pour acquitter un hook bloquant `PreToolUse` ou `UserPromptSubmit`, le broker exige une capture complète, un type concordant avec l'enveloppe validée et une écriture confirmée. Le hook renvoie un refus lorsque ces conditions échouent.
3. **Livraison.** Un transmetteur authentifié comme runner envoie le spool au gateway legacy par lots d'au plus 64 événements sur `POST /api/v1/events/batch`. Le gateway utilise le Store SQLite ou MySQL configuré pour ce tenant. L'identifiant de chaque événement dérive de sa position dans le spool. Un rejeu à l'identique retrouve les lignes stockées et un contenu divergent pour le même identifiant reçoit `409`. Un gateway owned renvoie `503 identity_unavailable` avant lecture du corps et le lot reste en attente dans le spool local. [Contrat de livraison des lots](api.md#provider-activity-event-batch).
4. **Budgets.** Les limites par défaut sont de 1 000 événements et 2 Mio par session, avec un arriéré non livré de 200 événements ou 512 Kio. Le champ `provider_activity` (`max_events`, `max_bytes`) du registre du broker les relève jusqu'à 5 000 événements et 8 Mio. L'arriéré suit au cinquième des événements et au quart des octets. Prévoir la capacité d'audit du tenant pour au moins 50 sessions en événements comme en octets. `ai-guard-diagnose` signale un dimensionnement inférieur. La rotation automatique reste indisponible et la rétention hors ligne couvre les Stores SQLite au format 13 accessibles depuis l'hôte. Le budget ordinaire compte les lignes d’événements encodées, y compris les changements de canal et les pertes. Le fichier contient aussi les curseurs de livraison, dont le nombre est borné par l’avancement des positions. Deux écritures de 16 Kio maximum chacune sont réservées au marqueur `session_budget_exhausted` et au bilan terminal. Elles exigent de l’espace disque disponible. Une réouverture conserve ces marqueurs sans les dupliquer. Les enregistrements suivants sont comptés comme perdus et les hooks bloquants sont refusés. Lorsque l'arriéré dépasse sa borne ou que le spool est indisponible, la lecture du stdout est suspendue jusqu'à l'échéance de la session et les hooks bloquants sont refusés. [Rétention de l'audit et déploiements pris en charge](setup/audit-retention/README.fr.md).
5. **Faits observés par ai-guard.** `activity.channel` (ouverture, attachement, suspension, fermeture d'un canal), `activity.loss` (perte observée par le contrôle, ou `provenance: "declared"` lorsqu'elle est rapportée par l'assistant du worker) et `activity.coverage` (bilan final : compteurs par canal et par catégorie, budget, arriéré, suspension, complétude). Ces événements portent `source: "runner"`.

Chaque événement fournisseur porte `data.activity` : canal (`stdout` ou `hook`), rang de l'enregistrement, client, mode interactif, version demandée pour l'image, worker, tentative et heure de réception. Ces champs viennent du contrôle, jamais du worker.

## Couverture par version du client

| Capacité | Claude Code 2.1.140 | Claude Code 2.1.329 | Codex CLI 0.165.0 / 0.170.0 |
|---|---|---|---|
| Messages, outils et commandes en batch | Implémenté, non qualifié (stream-json) | Implémenté, non qualifié (stream-json) | Fil principal seulement (exec --json) ; sous-agents par les hooks |
| Activité en TTY | Hooks : consignes, outils, sous-agents, dernier message du tour | Idem ; `MessageDisplay` non projeté | Hooks : idem ; recherche web hébergée non couverte |
| Hiérarchie des sous-agents | Sous-agents déclarés par les hooks. Le lecteur legacy SQLite peut inférer le parent par jointure exacte des événements relus. | Idem | `spawn_agent` nomme le parent ; parents imbriqués possiblement non résolus |
| Blocage si l'audit est indisponible | Quand le client de hook s'exécute ; un échec du hook lui-même laisse passer | Aussi en cas d'échec du hook (`onFailure: "block"`, 2.1.295+) | Quand le client de hook s'exécute ; un échec ou délai du hook laisse passer |
| Non couvert | OpenTelemetry, transcriptions internes, actions transitives hors hooks | Idem | app-server, OpenTelemetry, fichiers rollout, actions transitives hors hooks |

« Implémenté, non qualifié » signifie que le parseur et les tests sur fixtures existent, sans session réelle authentifiée pour cette version. Une version absente de ce tableau est inconnue : la collecte s'applique aux formats reconnus, sans affirmation de couverture. Aucune ligne de ce tableau n'est une médiation : le client et ses hooks s'exécutent dans le worker non fiable, sous le même utilisateur que l'agent.

## Tests et critères d'acceptation

Commande depuis la racine du dépôt :

```sh
cargo test --lib adapters
cargo test --test declared_agents
cargo test --test provider_activity_adapters
cargo test --test provider_activity_live
```

Les fixtures sont dans `tests/adapters_fixtures/` ; `SOURCES.json` indique pour chacune les sources officielles et les champs non confirmés. Elles contiennent uniquement des données de test ; `fixture-model` n'est pas un nom de modèle supporté promis par le projet.

Les tests vérifient les formats stdout et hooks des deux fournisseurs, le contrat générique, la conservation des caractères Unicode, les schémas malformés et leur catégorie, l'usurpation de source/provenance/identité, la taille des enregistrements, l'amplification, les caractères de contrôle échappés, le masquage avant projection, l'absence de faux parent-agent et la compatibilité du résultat avec le modèle `Event` du gateway. Les tests de collecte live vérifient le spool (chaîne, coupe d'une écriture incomplète, refus d'une altération, reprise), les budgets et pertes explicites, le refus d'un hook bloquant pendant une panne, le découpage du stdout, la livraison avec reprise sans perte et la route de lot du gateway (atomicité, rejeu idempotent, conflit, refus des namespaces réservés, rôle runner, débit compté par événement).

Les tests de projection vérifient les enfants reçus avant leur parent, les aliases génériques, la stabilité des IDs, l'isolation entre namespaces, le refus des identifiants invalides, les tentatives de promotion de provenance, les orphelins, les cycles, les mises à jour partielles et les transitions d'état explicites.

Critères d'acceptation du module : tous ces tests passent ; aucune ligne ne déclenche d'exécution ou d'appel réseau ; les tests avec fournisseurs réels restent déclarés séparément. Ces critères ne certifient pas l'interception de l'ensemble des actions internes d'un CLI.
