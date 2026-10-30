# Contrat API locale — v0.1.0

Les routes historiques du tableau ci-dessous sont également exposées sous le préfixe versionné `/api/v1` ; `/api` reste leur alias compatible. Les nouvelles [routes de supervision](#supervision-centrale--protocole-v1) existent uniquement sous `/api/v1`. `GET /api/v1/identity` (ou `/api/identity`) renvoie `{tenant_id,role}` depuis le jeton authentifié. Le gateway peut charger plusieurs identités et bases avec `--tenants-file` ; le [contrat multi-client](engineering/CLOUD_TENANCY.md) documente la configuration, la liaison durable et les limites. Les identifiants envoyés dans les requêtes ne choisissent jamais le tenant.

Gateway par défaut : `http://127.0.0.1:8787`. Le dashboard utilise les mêmes routes de lecture sous `http://127.0.0.1:4000`. La gateway et le dashboard ne fournissent aucune route d'exécution de commande HTTP : seul le CLI de confiance pilote Docker/Podman.

## Identités

Toutes les routes `/api/` exigent `Authorization: Bearer <secret>`. Le secret viewer donne accès aux lectures. Le secret runner permet les écritures et les lectures côté gateway. Le dashboard vérifie le rôle `viewer` auprès du gateway avant chaque requête relayée, même lorsqu'un fichier de clé a été mal configuré. Par défaut, il accepte seulement le viewer configuré ; `dashboard --shared-tenants` accepte les viewers des tenants configurés par le gateway, chacun dans son propre périmètre. Cette option exclut `--viewer-token-file`. Aucun paramètre d'URL ne transporte un jeton.

Sur une route protégée reconnue, l'identité est extraite avant `Path`, `Query` et `Json` : Bearer absent/invalide →401, viewer sur mutation →403, même si son corps ou ses paramètres sont malformés. Ces refus n'imposent pas de lecture/désérialisation applicative du corps. Les erreurs d'une entrée authentifiée invalide restent celles de son contrat. Routage, transport et protections Origin/URI restent en amont : cette règle ne prétend pas empêcher toute réception d'octets réseau. [Preuves](engineering/evidence/2026-10-30/api-auth/README.md) et [ordre des extracteurs Axum](https://docs.rs/axum/latest/axum/extract/index.html#the-order-of-extractors).

Gateway et proxy dashboard refusent les champs Authorization multiples, y compris identiques, ainsi que les Bearer vides, avec espace blanc ou dépassant4096octets. La syntaxe exacte historique `Bearer ` est conservée. Les constructeurs de bibliothèque refusent désormais les credentials configurés de plus de4096octets ou contenant un espace blanc : erreur pour `for_tenants`, état fail-closed503 pour `new`. Les lecteurs de fichiers gardent leurs contraintes existantes plus strictes côté gateway. Les empreintes restent dans un index privé de configuration, sans balayage des tenants par requête ni copie complète du dictionnaire de secrets à chaque extraction. Cela n'est pas une preuve de résistance générale au DoS ou de lookup constant-time.

| Méthode et route | Identité | Résultat |
|---|---|---|
| `GET /healthz` | Aucune | État du processus, service et version lorsqu'exposée |
| `POST /api/events` | Runner | Événement normalisé, expurgé puis stocké ; séquence et hashes |
| `POST /api/decide` | Runner | Décision tracée, hash de demande et `audit_seq` |
| `GET /api/events` | Viewer ou runner | Historique paginé et `next_cursor` |
| `GET /api/stream` | Viewer ou runner | SSE `event: audit`, `id: <seq>`, JSON d'événement |
| `GET /api/overview` | Viewer ou runner | Compteurs observés/déclarés séparés et état des composants |
| `GET /api/agents` | Viewer ou runner | `{items:[...]}` ; relations connues et provenance |
| `GET /api/sandboxes` | Viewer ou runner | `{items:[...]}` ; informations réellement recueillies |
| `GET /api/export` | Viewer ou runner | JSONL ou tableau JSON, borné par la tête de l'audit au début |
| `GET /api/integrity` | Viewer ou runner | Résultat de contrôle, nombre vérifié et hash de tête |
| `POST /api/workers/reservations` | Runner | Admission fraîche, réservation et reçu d'audit durables |
| `POST /api/workers/{id}/transitions` | Runner | Mutation CAS du worker et reçu d'audit ; aucun appel moteur dans le gateway |
| `GET /api/workers` | Viewer ou runner | Page `{items,next_cursor}` des workers du tenant |
| `GET /api/workers/{id}` | Viewer ou runner | Worker du tenant ; `404` s'il n'en fait pas partie |
| `GET /api/workers/operations/{id}` | Viewer ou runner, gateway uniquement | Reçu durable d'une opération du tenant |
| `GET /api/quota` | Viewer ou runner | Politique et réservations CPU/RAM/workers tenues |
| `GET /api/usage` | Viewer ou runner | Page `{items,next_cursor}` d'allocations terminales, `billable=false` |

Le dashboard ne relaie que les lectures de sa liste explicite, dont les workers, quotas et allocations ; il n'expose pas la consultation des reçus d'opérations du broker ni les mutations. Les lectures paginées workers/usage acceptent uniquement `cursor` (UUID canonique) et `limit` (1 à 100, 50 par défaut). Le curseur est le dernier worker ou la dernière tentative de la page. Un paramètre inconnu est refusé `400` ; le client ne choisit pas le tenant.

L'ingestion `POST /api/events` (et son alias `/api/v1/events`) refuse avec `403` les racines `security.decision`, `control`, `worker` et `usage.allocation`, ainsi que leurs descendants séparés par un point. Les décisions centrales sont produites par `/decide` ; un événement fourni par le runner ne peut créer une autorisation en cache. Les déclarations fournisseurs telles que `usage.reported` restent admises. Ce contrôle s'applique aux nouvelles ingestions : il ne permet pas d'authentifier rétroactivement un événement usurpé dans une ancienne base. L'historique n'est ni supprimé ni réécrit automatiquement ; une future admission d'exécution ne doit pas convertir ces anciens événements en droits de lancement.

## Demande de décision

```json
{
  "request_id": "0169b1de-e9d3-4eed-8f27-af2f5fd97493",
  "profile": "strict",
  "command": {"argv": ["/usr/bin/id"], "cwd": "/workspace"},
  "project_id": "project-example",
  "sandbox_id": "sandbox-example",
  "agent_id": "agent-example",
  "session_id": "session-example",
  "correlation_id": "correlation-example",
  "trace_id": "trace-example",
  "human_approved": false,
  "approval_hash": null
}
```

Une approbation conserve cette demande, ses IDs, son profil et sa commande. Seuls `human_approved` et `approval_hash` changent. Le hash est calculé sur la représentation sérialisée de la demande avec ces deux champs remis à leurs valeurs neutres. Le CLI utilise la même fonction Rust que la gateway. Ne pas recalculer le hash en supposant qu'un JSON réordonné est équivalent au format canonique du type Rust.

Le contrat du checkout ajoute `execution_mode` facultatif : absent signifie batch et conserve exactement les JSON/hashes historiques ; la seule valeur présente reconnue est `"interactive_tty_v1"`. Elle fait partie du hash de demande et de l'événement durable `security.decision`. Une approbation batch ne peut donc pas autoriser un canal interactif. Modifier ce mode sous la même identité constitue un conflit, pas une nouvelle décision implicite. Les valeurs inconnues sont refusées. Ce nouveau transport reste [en qualification](engineering/evidence/2026-10-30/interactive-cli/README.md), distinct du candidat batchv2 déployé en QA.

`provider_resolution` est un second champ facultatif, réservé à la sélection fournisseur gérée. **Son omission** conserve les octets/hashes historiques ; **`null` explicite est refusé**, comme une structure ou une variante inconnue. Exemple de résolution publique, sans credential :

```json
{
  "version": 1,
  "client": "claude_code",
  "auth_mode": "api",
  "transport": "batch",
  "profile_id": "claude-api",
  "profile_revision": 1,
  "model": "claude-haiku-5-5",
  "credential_binding_id": "ccaf8b98-84f5-4346-a030-c30246145821",
  "credential_names": ["AI_GUARD_PROVIDER_BOOTSTRAP"],
  "invocation_preset_version": 1,
  "provenance": {"client": "cli", "auth_mode": "cli", "profile": "profile", "model": "cli"}
}
```

Les clients sérialisés sont `codex` et `claude_code`, les modes `api` et `subscription`, les transports `batch` et `interactive_tty`. Les provenances sont `cli`, `profile`, `default_profile` ou `auto`. Le modèle est facultatif et omis lorsqu'il n'est pas fixé. Le binding est un UUID opaque ; ni source privée ni digest de credential ne figurent dans cette structure. L'argv autorisé est celui du bootstrap Rust avec ses seules options publiques ; le prompt en fait partie et ne doit contenir aucun secret.

La résolution entière participe au hash, à l'événement durable et à la vérification de l'admission ; un changement client/mode/transport/profil/révision/modèle/binding exige une nouvelle demande. Le rejeu d'audit relit ce type strict sans rétrograder une valeur invalide en demande historique. L'identité tenant runner/viewer reste indépendante de l'authentification fournisseur. Les [commandes et profils livrés](engineering/PROVIDER_AUTH.md#état-livré-et-limites--30-octobre-2026-après-revue) ne constituent pas une nouvelle API HTTP de secrets ou de lancement.

La réponse est de la forme :

```json
{
  "decision": {"outcome": "allow", "engine": "deterministic", "rule": "exact_allowlist", "reason": "Invocation exacte présente dans la politique locale."},
  "request_hash": "<sha256>",
  "jev": null,
  "audit_seq": 12,
  "audit_hash": "<sha256>",
  "receipt": {
    "version": 1,
    "request_id": "0169b1de-e9d3-4eed-8f27-af2f5fd97493",
    "request_hash": "<sha256>",
    "phase": "initial",
    "audit_seq": 12,
    "audit_hash": "<sha256>"
  }
}
```

Le broker refuse une réponse sans acquittement d'audit durable ou ne correspondant pas au hash demandé. Un refus n'est pas transformé en autorisation par une confirmation humaine. La gateway n'exécute jamais le contenu du champ `command`.

Une demande possède une identité durable `request_id` dans la base de son tenant. Répéter le même contenu et la même phase (`human_approved=false` ou `true`) restitue la décision, le résultat Jev et `audit_seq` du premier événement engagé, y compris après redémarrage ou concurrence. Modifier le contenu en conservant cette identité renvoie `409`. Pour une nouvelle évaluation, créer une nouvelle demande avec un nouvel UUID ; une décision déjà engagée n'est pas réévaluée implicitement après changement de politique ou indisponibilité Jev.

La phase approuvée exige une décision initiale `review` déjà durable et antérieure à l'approbation, ainsi que son hash exact. L'absence de revue ou un faux hash renvoie `403`, même si une autorisation approuvée existe déjà en cache. La phase initiale répétée continue à restituer la revue initiale ; seule la phase approuvée peut restituer la décision approuvée. Un historique ancien contenant plusieurs décisions pour une même phase ou des métadonnées incomplètes renvoie `409`, sans fusion ni modification des preuves.

Une décision déjà durable peut être relue lorsque la capacité d'audit est pleine ; une nouvelle décision reste refusée `503`. Un audit corrompu, un schéma inattendu ou une substitution du fichier bloque aussi les reprises. Une identité dont les métadonnées seraient altérées par le masquage des secrets est refusée `400` avant persistance. La réponse idempotente n'est pas un ticket de lancement : l'admission ci-dessous exige une nouvelle vérification.

## Admission et cycle de vie des workers

Les contrats sérialisés stricts font autorité dans [src/control.rs](../src/control.rs). Une réservation `AdmissionV1` contient `version:1`, un `operation_id` UUID, la demande de décision originale, son `receipt` exact et une spécification `launch` complète. Cette dernière lie session, commande/profil, autorité et type de runtime, référence et SHA-256 de l'image, réseau, ressources, durée, profil de sûreté, absence de restart, empreinte du manifeste de copie, version d'import, fournisseur et noms de credentials. Elle ne transporte aucune valeur de secret. Le profil livré fixe actuellement 2 CPU, 1 Gio et 256 PID ; il ne prétend pas offrir des limites arbitraires.

Pour un fournisseur géré, `launch.provider_resolution` est exactement égal à celui de `decision_request`, avec omission historique ou refus de `null` explicite selon la même règle. `launch.credential_binding_id` et `launch.credential_names` doivent correspondre à la résolution, les noms étant exactement `["AI_GUARD_PROVIDER_BOOTSTRAP"]`. `launch.provider` conserve le normaliseur (`codex` ou `claude`). Client, mode, transport, modèle et forme canonique de l'argv sont vérifiés côté serveur ; observe + bridge sont exigés. La validation refuse une nouvelle admission gérée en presetv1 et exige NoLogs avec batchv2 ou TTYv3 cohérent avec le transport. Une invocation dont `argv[0]` est le chemin canonique `/usr/local/bin/ai-guard-provider` sans résolution gérée est également refusée ; ce test textuel n'atteste pas l'identité de tous les alias ou programmes transitifs.

Le champ `launch.security_preset` accepte `fixed_sandbox_v1`, `fixed_sandbox_v2` et, dans le nouveau contrat interactif, `fixed_sandbox_interactive_v3` ; toute autre valeur est refusée. Les nouveaux lancements batch préparés par le broker utilisent `fixed_sandbox_v2`. Ce preset conserve le sandbox batch sans terminal ni entrée standard (`Tty=false`, `OpenStdin=false`, `StdinOnce=false`) et exige que le moteur ait désactivé son journal de sorties (`LogConfig.Type=none`). Le broker inspecte cette configuration avant le démarrage et lors des observations ; l'inspection publique porte le même `security_preset` que la spécification autorisée. L'admission seule n'est pas une preuve de cette configuration moteur. La capture de sorties demandée au broker reste un mécanisme distinct du journal moteur.

Le preset interactif exige la correspondance exacte avec `execution_mode=interactive_tty_v1`, `capture_output=false`, TERM fixé par le contrôle et aucun credential nommé TERM. TTY/stdin sont ouverts, journal moteur désactivé ; Docker attend `StdinOnce=true`, différence établie sur CLI29.2.1, et la forme Podman attend false sans qualification moteur acquise. Le fingerprint privév3 lie ces faits et TERM sans diffuser les autres valeurs Env ; les algorithmes v1/v2 restent inchangés. Aucune route HTTP de terminal n'est créée : le transport appartient au CLI de confiance. Les frappes et écrans ne sont pas des événements d'audit ; les compteurs stdout/stderr séparés sont inconnus (`null`) et les événements fournisseur TUI indisponibles. Lifecycle et diff conservent leur provenance générique.

`AdmissionV1`, `LaunchSpecV1`, les observations et les routes restent en version 1. La sérialisation et les hashes historiques de `fixed_sandbox_v1` sont conservés, sans migration de base ni réécriture de worker. L'API de contrôle accepte encore une admission explicite v1 d'un runner de confiance **sans résolution fournisseur gérée** ; le nouveau broker refuse son premier démarrage. Les workers v1 déjà démarrés restent observables, arrêtables et finalisables. Une tentative de changer le preset d'une opération déjà admise est un conflit `409`, pas une mise à niveau implicite.

La garantie de désactivation du journal moteur requiert un preset qui l'exige (batchv2 ou interactifv3) et un broker/moteur ayant effectué l'inspection correspondante. Elle ne couvre pas un ancien broker ou client de contrôle utilisant le preset v1. Les composants de contrôle doivent donc être mis à jour ensemble avant une publication revendiquant cette garantie ; une lecture d'historique v1 n'atteste pas rétroactivement l'absence de sorties dans le journal moteur. La preuve Docker batchv2 reste distincte de la qualification TTYv3 encore ouverte.

L'enveloppe version1 est conservée avec variantes explicitement versionnées. Un ancien serveur qui ne connaît pas le champ/mode/preset interactif le refuse ; aucun repli batch n'est autorisé. Coordonner la mise à jour du gateway et du broker. Le réattachement TTY après claim consommé n'est pas implémenté : refus explicite, sans start/restart/exec ; observation et arrêt restent les procédures disponibles. Cette limite et celle du timeout après mort du broker restent des écarts obligatoires de production.

Une nouvelle admission réévalue la politique courante et, si configuré, Jev ; un ancien `allow` n'autorise pas à lui seul le lancement. L'audit, le worker et la réservation sont engagés dans une transaction. La réponse `AdmissionResultV1` contient `accepted`, `refusal`, `worker`, `receipt` et `replayed`. Première acceptation : `201` ; replay identique : `200` ; refus politique durable : `403` ; refus quota durable : `429`. Même `operation_id` avec contenu divergent : `409`. Rejouer un refus restitue son résultat ; une nouvelle tentative exige une nouvelle identité d'opération.

Le contrôle attribue les identités du worker/de tentative et les noms du conteneur/volume ; un nom de session fourni par le client ne sélectionne pas un volume existant. Une mutation `WorkerMutationV1` porte `version:1`, `operation_id`, `worker_id`, `attempt_id`, `expected_revision` et `transition` avec discriminant `action`. Les actions sont `prepare`, `create_issued`, `created`, `start_claim`, `observe`, `unknown`, `stop`, `cancel_reserved`, `cleanup_result`. Le worker et son audit évoluent ensemble. Une révision périmée ou un état incompatible est refusé `409`.

Le claim de démarrage renvoie `disposition:emit_once` seulement lors de sa consommation initiale durable. Son replay renvoie `already_consumed` : il ne permet jamais de réémettre un start. Les états `reserved`, `preparing`, `created`, `start_issued`, `running`, `stopping`, `terminal`, `in_doubt`, `cancelled` distinguent les preuves connues. `in_doubt` conserve sa réservation ; un timeout de lease ne constitue pas une preuve d'arrêt. Une observation terminale complète et cohérente produit atomiquement l'allocation non facturable et la libération du quota. Les données manquantes, redémarrages ou écarts de configuration restent à réconcilier.

Ces API sont des contrats du contrôle de confiance ; une déclaration par le runner ne prouve pas à elle seule un confinement. Le broker inspecte le vrai moteur avant start et lors des observations. La qualification du parcours, les limites de reprise et les obligations encore ouvertes sont dans [CURRENT_STATE.md](engineering/CURRENT_STATE.md). Les anciennes bases v1 restent lisibles mais les routes du registre renvoient `503` avec `migration_required` jusqu'à une migration explicite sauvegardée ; voir [les opérations](operations.md).

Le Store Rust du schéma3 de supervision est vérifié séparément et les routes ci-dessous exposent ses transactions réelles. Le schéma par défaut reste2 ; aucune nouvelle migration CLI, route d'activation ou installation de broker n'est ajoutée. Une base v3 rouverte/restaurée est en maintenance : les effets frais sont refusés avec `503 supervision_inactive`. La capacité d'origine neuve est non sérialisée ; le numéro de schéma seul n'autorise aucun start. Réactivation après restart et migration des anciens Created restent des exigences ouvertes, sans repli legacy. [Contrats, preuves et limites du Store](engineering/evidence/2026-10-30/supervision-store/README.md).

### Supervision centrale — protocole v1

Ces routes du gateway exigent les mêmes guards d'identité que les routes historiques, avant l'extraction du chemin et du corps. Elles ne sont pas ajoutées à l'alias `/api` ni à la liste de relais du dashboard. Elles n'appellent aucun moteur : un reçu ou une permission centrale ne prouve pas un effet Docker/Podman.

| Méthode et route sous `/api/v1` | Identité | Corps de requête | Réponse |
|---|---|---|---|
| `GET /control/protocol` | Viewer ou runner | Aucun | `200 ControlProtocolV1` |
| `POST /workers/supervised-reservations` | Runner | `SupervisedAdmissionV1` | `AdmissionResultV1` ; première acceptation201, replay200, refus politique403 ou quota429 |
| `POST /workers/{id}/arms` | Runner | `ArmV1` | `200 ArmResultV1` ; `replayed` explicite |
| `GET /workers/{id}/arms/{arm_id}` | Viewer ou runner | Aucun | `200 ArmResultV1`, `replayed:true` ; absence ou autre parent404 |
| `POST /workers/{id}/armed-start-claims` | Runner | `ArmedStartClaimV1` | `200 MutationResultV1` |
| `POST /workers/{id}/helper-start-claims` | Runner | `HelperStartClaimV1` | `200 HelperClaimResultV1` |
| `POST /workers/{id}/helper-observations` | Runner | `HelperObservationV1` | `200 MutationReceiptV1` |
| `POST /workers/{id}/helper-cleanup` | Runner | `HelperCleanupV1` | `200 MutationReceiptV1` |
| `POST /workers/{id}/export-requests` | Runner | `ExportRequestV1` | `200 MutationReceiptV1` |

Les types et leur validation sont définis dans [src/control/supervision.rs](../src/control/supervision.rs). Les objets JSON refusent les champs inconnus. Les sept POST normalisent les rejets JSON en `400 invalid_worker_request`, sans reproduire le corps ni l'erreur détaillée du parseur. Ils appliquent le limiteur d'écriture du tenant. Aucun champ tenant, credential, chemin de configuration privé ou programme de moteur n'est accepté par ces contrats.

`ControlProtocolV1` ajoute `tenant_id` et `role` de l'identité authentifiée aux seuls faits du Store : `version`, `schema_version`, `control_namespace_uuid`, `protocols`, `active`. Son [schéma JSON](../schemas/supervision-protocol.schema.json) décrit cette réponse de lecture ; les autres contrats restent les types Rust stricts. Une base2 renvoie `active:false` et `protocols:[]`. Le schéma3 peut annoncer `broker_supervision_v1` tout en restant inactif. `active:true` n'atteste ni ordonnanceur, ni horloge native, ni confinement runtime : c'est la capacité transactionnelle de cette origine vivante.

`SupervisedAdmissionV1` contient `version:1`, `protocol:"broker_supervision_v1"`, un `intent_hash` SHA-256 et l'objet `admission:AdmissionV1` complet. Son autorité est un UUID ; les nouveaux presets exigent NoLogs. Le serveur valide forme, taille et absence de masquage nécessaire avant l'évaluation externe. Il recherche d'abord l'opération durable ; un replay n'appelle pas Jev. Une demande fraîche vérifie d'abord schéma3 et activation ; leur refus connu ne déclenche pas d'appel Jev. Les quotas dynamiques et la décision fraîche sont revérifiés dans la transaction du Store : cette prélecture ne remplace ni la réservation atomique ni son refus durable. Les champs et hashes historiques de l'admission, de la résolution fournisseur et de la spécification restent inchangés.

Un `ArmV1` lie worker/tentative/intention, inspection déclarée, empreintes d'autorité/configuration, horloge d'hôte et échéance absolue. Un replay conserve exactement ces octets, l'échéance et le reçu. Les nanosecondes sont des chaînes décimales canoniques vérifiées sans débordement ; le serveur ne prouve pas qu'un caller a réellement lu cette horloge. Pour un helper, `inspection.spec_hash` doit être le hash de la politique helper dérivée côté Rust, pas celui du worker. Le Store exige une importation puis une vérification exacte avant création/claim du worker ; un helper de vérification encore pending, inconnu ou divergent bloque la continuation.

Le chemin `{id}` est comparé au worker réel de l'arm chargé dans le Store de l'identité, notamment pour les helpers ; le `worker_id` d'un reçu JSON n'est pas une preuve de parenté. Une arm d'un autre tenant ou d'un autre parent n'est pas diffusée. Les reçus, permissions et observations sont ensuite vérifiés transactionnellement. La première consommation d'un claim renvoie `emit_once` ; son replay exact renvoie uniquement `already_consumed`. Cela ne signifie pas « effet moteur exactement une fois ».

Après réouverture ou restauration, la maintenance permet de relire les arms/reçus et de rejouer un claim déjà consommé, sans nouvelle permission. Les observations/cleanup conservateurs restent soumis à leurs preuves. Nouvelle admission, arm, claim ou destination d'export renvoie `503 supervision_inactive` avant append lorsque l'origine est inactive ; schéma antérieur non migré renvoie `503 migration_required`. Un JSON ou reçu invalide peut être refusé plus tôt en400 ; une identité réutilisée avec un autre contenu renvoie409. Il n'existe aucune route de fresh Bind d'un ancien Created. Les parcours de fencing, reprise après panne, broker/IPC/gardien et qualification moteur restent ouverts.

Le plan privév3 fige la sélection avant décision. Approbation et reprise réutilisent ses octets privés sans nouvelle résolution ni rotation implicite ; un binding modifié bloque un premier démarrage. Les plans1/2 ne se convertissent pas en plans3 et un claim consommé ne permet jamais un second start. Ce plan et son dictionnaire de masquage ne sont pas des ressources HTTP publiques. Le build et les sondes sans credential de la nouvelle image bootstrap ont passé sur Docker Desktop/macOS ARM64, puis un [Claude/API batch géré réel](engineering/evidence/2026-10-30/provider-auth/managed-claude/real-run.md) a produit/exporté le fichier exact avec audit valide après restart. Les autres authentifications, TUI et réglages adverses restent non qualifiés. Les anciennes preuves QA ne leur sont pas transférées.

## Adaptateur Jev réel optionnel

Le gateway peut consulter TypeSafe avec `POST https://api.typesafe.ai/v1/systemone`, un modèle explicite tel que `jev-latest` et une question `choice`. L'enveloppe contient `model`, `state` et `questions`; chaque question décrit `type`, `instructions` et `criteria`. Ces formes sont documentées dans [System One](https://docs.typesafe.ai/api/system-one) et [Choice](https://docs.typesafe.ai/primitives/choice).

L'adaptateur Rust envoie la question `execution_policy` avec les choix `allow`, `deny`, `review`. Son état minimal expurgé contient profil, commande, identifiant de projet et portée du workspace copié ; il n'atteste pas les effets transitifs. Il exige `answers.execution_policy.type="choice"`, un choix reconnu, une confidence valide atteignant le seuil configuré et un `model` de réponse présent. Il conserve le modèle retourné et l'usage déclaré. Une erreur, une réponse inconnue ou incertaine bloque le lancement ; une réponse favorable ne lève aucun refus local ni contrôle d'audit.

`jev-latest` est un alias qui peut évoluer ; le modèle retourné reste la provenance de la réponse. Le client livré n'ajoute pas le header `TypeSafe-Danger-Zone-Long-Context`, donc n'active pas Long Context implicitement. Voir [modèles et Long Context](https://docs.typesafe.ai/models). Le seuil configuré ne démontre pas une calibration indépendante d'ai-guard.

La clé Jev est un secret du contrôle, distinct des credentials runner/viewer et absent des workers. Le produit lit un fichier brut désigné par `JEV_API_KEY_FILE` (`--jev-key-file`), pas un fichier dotenv complet. Les appellations `JEV_API_KEY` et `TYPESAFE_API_KEY` peuvent désigner la même clé de service dans la documentation des comptes ; elles ne créent aucun alias de configuration automatique dans ce gateway. Voir [l'activation explicite](operations.md#jev-optionnel).

## Événements

Le schéma d'ingestion est dans `schemas/event.schema.json`. Les champs optionnels sont absents ou `null`. Le stockage ajoute :

| Champ | Sens |
|---|---|
| `seq` | Séquence monotone locale utilisée pour pagination et reprise SSE |
| `received_at` | Horloge UTC du collecteur à l'enregistrement |
| `prev_hash` | Hash du précédent événement de la chaîne |
| `hash` | SHA-256 de la séquence, réception, hash précédent et payload masqué |

`command.requested`, `security.decision`, `runtime.submitted`, `command.started`, `command.completed` et `command.failed` désignent des étapes différentes. Un `tool.result` de provider n'est pas une observation indépendante du noyau ; sa provenance reste `declared`.

`result` peut contenir `exit_code`, `duration_ms`, `timed_out`, `oom_killed`, tailles et indicateurs de troncature. `stdout` et `stderr` n'apparaissent que si la capture a été demandée. Les fichiers portent des empreintes avant/après, jamais une attribution automatique à un sous-agent.

## Recherche, filtres, reprise

Paramètres de lecture : `q`, `project_id`, `sandbox_id`, `agent_id`, `kind`, `decision`, `before`, `after`, `from`, `to`, `limit`, `order`. `before` et `after` sont exclusifs ; `from` et `to` s'appliquent au temps de réception et doivent être RFC3339. Ordre par défaut décroissant, `order=asc` pour l'ordre croissant. `limit` est borné entre1 et1000. La recherche texte est une recherche littérale, pas une expression SQL ou regex.

Le flux SSE accepte `Last-Event-ID` ou `after`. Sans curseur, il commence après la tête actuelle. Un client charge d'abord l'historique, conserve son dernier `seq`, puis se connecte avec ce curseur. Au maximum32 flux par gateway ; le navigateur doit reconnecter avec temporisation et dédoublonner par séquence.

L'export accepte les mêmes filtres et `format=jsonl` ou `format=json`. Il fixe une tête de lecture au début afin que les nouvelles écritures n'allongent pas indéfiniment l'export. Une erreur interrompt le flux ; le client doit signaler l'échec et ne pas présenter un export interrompu comme complet.

## Erreurs et limites

`401` : jeton absent/invalide. `403` : rôle insuffisant, origine refusée ou approbation sans revue/hash valable. `400/422` : paramètres ou JSON invalides. `409` : contenu divergent pour une identité de demande, ou historique ambigu. `413` : corps de plus de256Kio. `429` : débit d'écriture ou nombre de flux dépassé. `503` : audit indisponible, plein pour une nouvelle écriture ou déclaré incohérent. Le dashboard renvoie `502` si son collecteur est inaccessible. Le contenu renvoyé ne doit pas être rendu comme HTML.

Les tokens actuels n'ont pas d'expiration automatique ; c'est une limite à traiter selon le profil d'identité annoncé, particulièrement pour une exposition distante. La rotation est une opération explicite sur les fichiers de secrets, suivie du redémarrage des services concernés. Ne pas mélanger rotation et suppression de l'audit. Cette limite ne dispense pas des exigences d'identité, de révocation et de transport des [gates de qualification](engineering/PRODUCTION_READINESS.md).
