# Adaptateurs JSONL livrés

## Périmètre réellement implémenté

`src/adapters.rs` contient un normaliseur Rust sans accès réseau, sans accès au système de fichiers et sans exécution de commande. Il prend une ligne JSONL complète et produit zéro ou plusieurs événements du modèle du gateway.

```rust
pub struct AdapterContext {
    pub session_id: String,
    pub project_id: String,
    pub sandbox_id: String,
    pub agent_id: String,
    pub correlation_id: String,
    pub trace_id: String,
}

pub fn parse_line(
    provider: &str,
    line: &str,
    context: &AdapterContext,
) -> Vec<serde_json::Value>;
```

Les valeurs reconnues de `provider` sont `codex`, `claude`, `claude-code` et `generic`. L'alias `claude-code` produit la source canonique `claude-adapter`. Une valeur inconnue ou une ligne non reconnue produit `[]`.

Ces parseurs sont validés sur des **fixtures synthétiques de schémas documentés**, pas sur une session authentifiée exécutée contre chaque service. Le niveau affiché reste `coverage: "observed"` et la provenance reste **toujours** `declared`. Le mot Observed décrit ici le niveau d'intégration de la session ; il ne transforme pas une déclaration du CLI en preuve indépendante de l'effet système.

## Identité et temps

Le normaliseur crée un UUID et un timestamp UTC pour chaque événement. Ce timestamp est l'heure de normalisation. Lorsque le runner traite le stdout à la fin d'une exécution, ce n'est donc pas l'heure réelle de chaque appel d'outil. Une éventuelle date déclarée par un agent générique reste dans `data.declared_timestamp`.

Les six identifiants du contexte proviennent du runner de confiance. Les champs homonymes d'une ligne ne peuvent pas les remplacer. Les identifiants fournisseurs sont conservés dans `data.provider_session_id`, `provider_item_id`, `provider_message_id` ou `provider_tool_use_id` selon leur disponibilité.

`parent_tool_use_id` de Claude désigne un appel d'outil ; il est conservé comme `data.provider_parent_tool_use_id`, sans le convertir en `parent_agent_id`. Ce normaliseur sans état ne fabrique pas de hiérarchie de sous-agents. Le contrat générique peut déclarer des IDs d'agents. Le composant Rust `src/declared_agents.rs` valide et corrèle ces déclarations dans une projection distincte, décrite ci-dessous.

## Mappings

| Entrée | Événement normalisé | Données retenues |
|---|---|---|
| Codex `thread.started` | `session.declared` | ID de thread fourni. |
| Codex `turn.started` / `turn.completed` / `turn.failed` / `error` | `session.declared` | État rapporté, erreur si exposée. |
| Codex `turn.completed.usage` | `usage.reported` | Compteurs entiers positifs ou nuls explicitement présents. |
| Codex `item.started` de type `command_execution` | `tool.requested` | ID d'item, texte de commande, statut rapporté. |
| Codex `item.completed` de type `command_execution` | `tool.result` | Même contexte ; code de sortie et sortie agrégée si fournis. |
| Codex `item.completed` de type `file_change` | `file.declared` | Chemins, `add`/`update`/`delete`, statut rapporté y compris échec. |
| Claude `system` / `init` | `session.declared` | Session, modèle et répertoire déclarés lorsqu'ils existent. |
| Claude `assistant.message.content[]` / `tool_use` | `tool.requested` | Nom, ID et entrée de l'outil. |
| Claude `user.message.content[]` / `tool_result` | `tool.result` | ID de l'appel, contenu et état d'erreur éventuel. |
| Claude `result` | `session.declared` puis éventuellement `usage.reported` | Résultat de session et compteurs effectivement présents. |

Le code de sortie rapporté par Codex reste une déclaration du fournisseur. Le résultat du processus `codex` ou `claude` lui-même doit être enregistré séparément par le runner.

Les fragments `stream_event` Claude sont ignorés. Les blocs complets évitent de reconstruire des arguments à partir de fragments incomplets. L'usage des messages `assistant` n'est pas additionné : ces messages peuvent être partiels et conduire à des doubles comptes. Aucun coût n'est calculé par ces adaptateurs.

Les mappings Codex sont fondés sur les types du dépôt officiel : [events.ts](https://github.com/openai/codex/blob/main/sdk/typescript/src/events.ts) et [items.ts](https://github.com/openai/codex/blob/main/sdk/typescript/src/items.ts). Les blocs Claude sont documentés dans [Streaming output](https://code.claude.com/docs/en/agent-sdk/streaming-output) et [Handle tool calls](https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls). Consultation : 29 octobre 2026.

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

Cette voie permet à un agent personnalisé de rendre son arbre visible. Les parseurs Codex et Claude livrés ne produisent pas d'événements `agent.declared` à partir de leurs sorties actuelles ; ils n'affirment donc pas découvrir leurs sous-agents natifs. L'existence de hooks documentés chez un fournisseur ne vaut pas intégration de ces hooks dans ce projet.

## Limites et erreurs

| Contrôle | Limite / comportement |
|---|---|
| Taille d'une ligne avant parsing | 64 Kio maximum, mesurés en octets UTF-8. |
| Amplification en événements | 32 événements maximum par ligne. |
| Texte projeté | 4 096 octets maximum, coupe sur une frontière UTF-8, suffixe `[truncated]`. |
| Objet de données projeté | Budget de 12 Kio et vérification de la taille JSON après échappement. |
| Structure projetée | Profondeur de 8 ; 32 éléments par collection ; clés bornées. |
| Arguments génériques | Au plus 64 ; 8 192 octets cumulés ; 4 096 octets par argument. |
| JSON invalide, type inconnu, schéma incompatible | `[]`, sans exception et sans événement de succès fictif. |

Les projections tronquées d'objets portent `adapter_payload_truncated: true`. Si l'échappement JSON dépasse le budget, l'objet est remplacé par cet indicateur. Les troncatures empêchent une restitution exhaustive ; elles doivent rester visibles dans l'IHM.

Le runner doit émettre un **bilan de couverture par session** : taille du stdout capturé, collecte complète ou tronquée, parseur sélectionné et éventuelles limites. Le normaliseur ne produit pas un événement d'erreur pour chaque ligne de bruit. Une ligne ignorée ne signifie pas qu'aucune action n'a eu lieu.

Le masquage des secrets reste appliqué par le serveur avant stockage, sur tous les champs. Les parseurs ne prétendent pas détecter tous les secrets. Les données normalisées restent non fiables et doivent être rendues comme texte dans le frontend.

## Tests et critères d'acceptation

Commande depuis la racine du dépôt :

```sh
cargo test --lib adapters
cargo test --test declared_agents
```

Les fixtures sont dans `tests/adapters_fixtures/`. Elles contiennent uniquement des données de test ; `fixture-model` n'est pas un nom de modèle supporté promis par le projet.

Les tests vérifient les deux formats fournisseurs, le contrat générique, la conservation des caractères Unicode, les schémas malformés, l'usurpation de source/provenance/identité, la taille des lignes, l'amplification, les caractères de contrôle échappés, l'absence de faux parent-agent et la compatibilité du résultat avec le modèle `Event` du gateway.

Les tests de projection vérifient les enfants reçus avant leur parent, les aliases génériques, la stabilité des IDs, l'isolation entre namespaces, le refus des identifiants invalides, les tentatives de promotion de provenance, les orphelins, les cycles, les mises à jour partielles et les transitions d'état explicites.

Critères d'acceptation du module : tous ces tests passent ; aucune ligne ne déclenche d'exécution ou d'appel réseau ; les tests avec fournisseurs réels restent déclarés séparément. Ces critères ne certifient pas l'interception de l'ensemble des actions internes d'un CLI.
