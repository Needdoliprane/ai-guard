# Clients et authentification dynamiques — proposition du 30 octobre 2026

**Historique :** la proposition ci-dessous décrit le checkout avant l'implémentation. Elle est conservée avec ses constats et statuts d'alors. L'[état livré après revue](#état-livré-et-limites--30-octobre-2026-après-revue), en fin de document, décrit les commandes et contrats actuels ; il ne requalifie pas ce premier gel.

## Besoin et périmètre

Remplacer l'obligation de connaître l'argv fournisseur par une résolution Rust explicite, traçable et stable : Codex CLI ou Claude Code, abonnement ou clé API, conversation TTY ou commande batch. Les quatre combinaisons d'authentification restent obligatoires. Ce document propose le contrat ; aucun parcours nouveau n'est déclaré livré ou qualifié. Aucune valeur de `.env`, aucun cache OAuth et aucun keychain n'a été ouvert. Aucun modèle n'a été appelé.

## État réel et fichiers concernés

- `src/runner.rs:20` : `RunOptions` exige `argv`, conserve `provider` comme chaîne, n'a ni mode d'authentification ni profil fournisseur. `choose_plan` valide TTY/confinement, exige `observe + bridge` pour les credentials et refuse toute source physiquement dans le projet (`:420–481`). Le `.env` racine autorisé par le mainteneur reste donc inutilisable par ce chemin, même si sa copie est exclue.
- `src/runner.rs:654` : le parseur littéral `NAME=value` refuse doublons et variables dangereuses, puis photographie et injecte **toutes** les valeurs acceptées. Il ne sélectionne pas les credentials du client retenu. Les protections `PrivateFile`, snapshots privés, séparation runner/viewer/Jev et masquage sont à conserver.
- `src/adapters.rs:43–114` : `provider` choisit seulement la normalisation JSONL Codex/Claude/générique. Ces événements restent déclarés et collectés après exécution ; ce module ne résout ni installation ni authentification.
- `src/operations.rs` et `images/worker.Dockerfile` : installation facultative des paquets officiels, avec versions exactes. Aucun manifeste de capacités, profil d'exécution ou diagnostic fournisseur. La CLI hôte n'atteste pas les capacités de l'image worker.
- `src/model.rs:40`, `src/policy.rs:36`, `src/control.rs:79` : le hash de décision ne porte pas le mode d'authentification. L'admission possède `provider`, `credential_binding_id` et `credential_names`, mais leur relation au choix autorisé n'est pas vérifiée. `src/runner/managed.rs:1231` construit cette admission après décision ; le digest privé des credentials protège ensuite le premier démarrage repris (`:2144`).
- `src/runtime.rs:180` : environnement fixe, seul worker reçoit `--env-file`, home en tmpfs, pas de montage OAuth. `README.md`, `docs/quickstart.md:187–220` et `docs/operations.md:150–197` imposent encore des argv manuels et contiennent des formulations devenues périmées sur TTY/login. Les preuves historiques restent datées et inchangées.

## Mécanismes fournisseurs établis

| Parcours | Mécanisme retenu et preuve |
|---|---|
| Codex/API | En batch, `CODEX_API_KEY` est officiellement reconnu par `codex exec`. Pour TTY, `codex login --with-api-key` reçoit la clé sur stdin, puis le client utilise son stockage isolé. Les aides locales Codex **0.165.0** confirment cette option. [Automatisation officielle](https://learn.chatgpt.com/docs/non-interactive-mode). |
| Codex/abonnement | Connexion officielle ChatGPT, notamment `codex login --device-auth`, dans un `CODEX_HOME` dédié avec stockage `file`. Un `auth.json` explicitement préparé peut être transféré ; aucun balayage du home ni extraction du keychain. Le rafraîchissement est géré par Codex. L'aide locale confirme device-auth. `--with-access-token` existe mais vise l'automatisation Enterprise : ce n'est pas un substitut universel à l'abonnement. [Authentification officielle](https://learn.chatgpt.com/docs/auth). |
| Claude/API | Uniquement `ANTHROPIC_API_KEY`. La documentation indique qu'en `-p`, cette clé est utilisée dès qu'elle est présente ; elle doit donc être absente du parcours abonnement. [Authentification officielle](https://code.claude.com/docs/en/authentication#authentication-precedence). |
| Claude/abonnement | Token produit volontairement par `claude setup-token`, transmis en `CLAUDE_CODE_OAUTH_TOKEN`, ou login officiel dans un contexte dédié. Le token documenté fonctionne pour les requêtes modèle de Claude Code ; les fonctions Remote Control/connecteurs ont des restrictions distinctes. Aucune copie implicite de `.credentials.json`. [Token officiel](https://code.claude.com/docs/en/authentication#generate-a-long-lived-token), [variable officielle](https://code.claude.com/docs/en/env-vars). |

Les pages actuelles n'attestent pas rétroactivement Claude **2.1.140**. Son aide en worker et les vrais essais restent nécessaires. Codex hôte : `--version`, `login --help`, `exec --help` constatés avec exit0 ; ce sont des capacités locales, sans authentification ni modèle.

## Contrat recommandé

Ajouter un module Rust `src/provider.rs`, sans dépendance nouvelle, avec `ClientKind`, `AuthMode`, `ProviderProfileV1`, `ProviderCapabilitiesV1`, `ProviderResolutionV1` et un constructeur typé d'invocation. Les presets Rust dépendent du client, du transport et des capacités/version qualifiées ; aucune chaîne shell ni template d'argv arbitraire dans une configuration de projet.

La CLI gérée devient par exemple `agentsandbox run --client codex --auth-mode subscription --prompt "…"`, avec `--interactive` pour TTY, `--client-profile ID` et `--model` facultatifs. `--client auto --auth-mode auto` autorise la résolution. Conserver le parcours avancé `--provider … -- argv` pour compatibilité, avec conflit explicite entre argv manuel et sélection gérée. Ne pas donner au vieux normaliseur une signification d'authentification rétroactive.

Configuration privée `providers.json`, validée strictement, version1 : profils nommés contenant client, mode, image, modèle facultatif et référence de credential (`api_env_file`, `codex_auth_file`, `claude_oauth_env_file`), profil par défaut et ordre automatique explicite. JSON réutilise serde_json déjà présent. Les valeurs secrètes n'entrent pas dans ce fichier ; `.env` est une source explicitement configurée, jamais une recherche automatique ascendante.

Le nom canonique de la source Claude par abonnement est `claude_oauth_env_file`. L'ancienne graphie dérivée `claude_o_auth_env_file` reste un alias de lecture ; le chargement ne réécrit pas les fichiers existants. Une sérialisation nouvelle utilise le nom canonique, qu'un ancien binaire peut refuser : aucune compatibilité de rollback n'est promise pour un fichier ainsi réécrit. Cet ajustement ne modifie ni résolution, bootstrap, binding accepté ni plans persistés. La régression `claude_oauth_source_accepts_documented_and_legacy_names_without_rewriting` conserve les deux lectures, l'écriture canonique, les octets du fichier ancien et le refus de champ inconnu ; le manuel [anglais](../setup/providers/README.md) et [français](../setup/providers/README.fr.md) porte le même contrat.

Priorité : choix CLI explicite, profil choisi, défaut privé, résolution automatique. Un client explicite ne change jamais parce qu'une autre clé est disponible. Un abonnement explicite ne devient jamais une API. En auto, préférer un profil abonnement valide explicitement configuré ; en son absence, une API explicitement configurée peut être retenue et annoncée. Un profil abonnement expiré/refusé exige une reprise indiquée, sans nouvelle tentative en API. Deux candidats sans priorité déclarée produisent `ambiguous_configuration`. La présence seule d'une variable ambiante ne choisit rien.

Le résultat structuré non secret contient version, client/mode demandés et résolus, provenance de chaque choix, profil/révision, image et version CLI observées, preset, transport, modèle éventuel, type et identifiant opaque du binding, prérequis et fraîcheur. Afficher avant demande de décision « API, utilisation facturée par le fournisseur » lorsque pertinent. États distincts : `client_missing`, `version_incompatible`, `login_required`, `credential_missing`, `credential_unsafe`, `ambiguous_configuration`, `probe_failed`, `ready`. `ready` signifie prérequis présents, sans promettre qu'un serveur acceptera la clé.

`doctor providers --image … --json` effectue des sondes bornées sans réseau, projet ni credential, avec autorité runtime et journal durable. Les versions et fonctionnalités réellement présentes dans l'image alimentent le diagnostic ; un tag/label seul reste déclaré. Une aide compatible ne qualifie pas les effets du preset. Ne pas créer d'API navigateur de secrets ou de lancement : conserver le runner de confiance. Les types sérialisés enrichissent les admissions et les API existantes de consultation ; le viewer voit le choix et sa provenance, jamais les sources privées.

## Handoff privé, autorisation et reprise

Le parseur accepte le `.env` explicitement désigné dans le projet uniquement si l'exclusion physique est garantie par la même politique d'import, avec tests de substitution. Conserver les exigences propriétaire/0600/lien unique/ACL/limites. Refuser les autres sources internes non exclues. Sélectionner le seul secret utile : Codex accepte une référence explicite `OPENAI_API_KEY` ou `CODEX_API_KEY` et normalise pour son mode ; Claude/API prend `ANTHROPIC_API_KEY` ; Claude/abonnement prend seulement `CLAUDE_CODE_OAUTH_TOKEN`. Deux alias Codex concurrents exigent une sélection explicite. Le fichier complet et les clés de l'autre fournisseur ne sont jamais transmis.

Pour Codex/abonnement et Codex/API TTY, ajouter un bootstrap Rust à l'image : recevoir un JSON compact via une variable interne du snapshot privé, préparer le home tmpfs, effectuer le login API sur stdin ou installer le seul `auth.json` explicitement fourni, retirer les variables de bootstrap, puis `exec` le client. L'argv autorisé contient réellement le bootstrap et sa cible non secrète ; le dictionnaire de masquage contient séparément les tokens du JSON. Pas de shell, nouveau montage hôte ou cache global. Le matériel OAuth est lisible par son client, comme les clés actuelles ; ce mécanisme ne garantit pas sa confidentialité face au code worker. Un assistant `auth login/import/status` doit préparer volontairement les credentials dans un contexte fournisseur isolé, sans projet, et ne jamais imprimer/persister les tokens dans ses traces. Les refresh/concurrences exigent exclusivité par binding et qualification du masquage des nouveaux tokens ; ne pas réécrire automatiquement le cache hôte fourni.

Créer le binding opaque avant décision, associer ses octets immuables et son digest uniquement dans le stockage privé, puis inclure `provider_resolution: Option<ProviderResolutionV1>` dans `DecisionRequest` et `LaunchSpecV1`. Vérifier côté serveur identité exacte client/mode/preset/binding/noms entre décision et admission ; vérifier côté runner checkpoint, snapshot et invocation avant create/start. Toute modification exige une nouvelle demande. Ne jamais envoyer un digest de secret ou un chemin privé dans l'événement public. Auto ne se réexécute pas lors d'une approbation ou reprise.

Les champs nouveaux portent `default + skip_serializing_if=None` : snapshots de référence doivent prouver des hashes historiques identiques octet pour octet lorsque le champ est absent. Les anciennes sessions gardent leur ancien sens, leurs claims consommés et leurs capacités d'observation/arrêt/export. Elles ne se convertissent pas en sessions gérées. Lecture des plans1/2 conservée ; nouveau plan3 pour la résolution gelée. Premier démarrage et reprise utilisent le binding admis, sans rotation silencieuse ni répétition d'un claim consommé. Authentification tenant runner/viewer et authentification fournisseur restent deux contrats indépendants.

## Découpage, propriétaires et acceptation

| Lot / propriétaire proposé | Fichiers et interface | Preuve attendue |
|---|---|---|
| Résolution / agent provider | Nouveau `provider.rs`, export `lib.rs`, config/capacités/presets purs | Sélection explicite, auto autorisé, ambiguïté, zéro fallback après refus, matrice batch/TTY. |
| Contrats / racine | `model.rs`, `policy.rs`, `control.rs`, API/validation durable | Hashes historiques stables ; changement client/auth/binding refusé ; identité tenant indépendante. |
| Exécution / agent runner | `runner.rs`, `runner/managed.rs`, `operations.rs`, bootstrap/image/runtime | `.env` exclu, secrets minimaux, absence argv/log/audit, snapshot substitué refusé, reprise idempotente, TTY conservé. Revue indépendante obligatoire. |
| Qualification / Luna | Tests ciblés existants et nouvelles régressions utiles | Quatre vrais parcours : choix annoncé, autorisation durable, appel réel, effet/export vérifié, arrêt/reprise ; versions moteur/image/client consignées. |
| Documentation / racine | Quickstart, opérations, API, compatibilité, VALIDATION, gaps G05/G07 | Parcours gérés sans argv fournisseur ; statuts et limites cohérents ; gouvernance AGENTS/DECISIONS synchronisée. |

La première verticale utilisable relie configuration → diagnostic → résolution gelée → décision → injection minimale → worker réel → résultat/export. Elle exerce d'abord les deux API déjà fournies pour découvrir les défauts du chemin commun, tout en réalisant les deux transports d'abonnement décrits ; elle ne clôt aucun parcours abonnement sans ses credentials et preuves réelles. Statut de cette proposition : diagnostic documentaire **PASS** ; implémentation et quatre parcours dynamiques **NOT_RUN**. Les authentifications abonnement manquantes constituent des prérequis précis, jamais un succès simulé.

## État livré et limites — 30 octobre 2026 après revue

Cette section remplace les hypothèses de livraison de la proposition, sans modifier ses preuves historiques. Le code Rust de sélection, bootstrap, liaison d'autorisation et reprise est implémenté. La validation logicielle post-revue a passé formatage, Clippy, build release et les cibles fournisseur retenues : 25 tests PASS, une fixture interne ignorée **NOT_RUN**. Ce résultat logiciel ne qualifie aucun client natif. Un essai ultérieur **Claude/API batch géré est PASS ciblé** : demande explicite `claude-haiku-5-5`, fichier réel créé/exporté exact, original intact, quota libéré, allocation observée non facturable et audit36 valide après redémarrage du gateway. Le modèle effectivement servi reste inconnu ; aucun TUI, abonnement, Codex/API géré ou réglage adverse qualifié par ce cas. [Rapport](evidence/2026-10-30/provider-auth/managed-claude/real-run.md). Voir [VALIDATION.md](VALIDATION.md) pour les gels et écarts.

L'image `agentsandbox-worker:qa-managed-providers-20261030-ccgpe3`, ID `sha256:069ba7b703077238f22edbefc9295a11cc07e71e8dd490b8de7ab81da51ffa38`, a passé le build sur Docker Desktop/macOS ARM64 (`linux/arm64`). Les versions réelles Codex0.165.0/Claude2.1.140, les aides requises, le bootstrap exécutable et son refus sans handoff ont été constatés sous confinement. Aucune clé ni authentification ni requête modèle n'a été utilisée par ces sondes. Le postinstall npm Claude n'a pas été autorisé lors de l'installation ; son éventuel effet sur le vrai lancement reste non vérifié dans cette preuve. Ce build ne qualifie ni Podman, ni Linux x86_64, ni une conversation.

### Commandes et sélection

```sh
agentsandbox providers init --config-dir /chemin/prive/controle \
  --env-file /chemin/projet/.env
agentsandbox providers status --config-dir /chemin/prive/controle --json
agentsandbox run --config-dir /chemin/prive/controle --project /chemin/projet \
  --profile observe --network bridge --timeout 120 \
  --client claude-code --auth-mode api --client-profile claude-api \
  --model claude-haiku-5-5 --prompt "Inspecte le projet sans modifier les fichiers"
```

Ces commandes décrivent la procédure livrée, à qualifier sur une image construite depuis ce checkout. Ajouter les options runtime et image propres à l'installation. Le modèle Claude est explicitement `claude-haiku-5-5` pour les prochains essais ; les anciens résultats avec l'alias `haiku` ne prouvent pas Haiku5.5.

`providers init` crée les références privées `<config-dir>/providers.json` sans lire les credentials ni écraser une configuration existante. Les profils générés sont `codex-api` (`OPENAI_API_KEY`) et `claude-api` (`ANTHROPIC_API_KEY`), avec `auto_priority=["codex-api","claude-api"]` et sans profil par défaut. `providers status` valide la configuration et montre profils/types de source sans chemin privé ni valeur. Il ne sonde ni source secrète ni image : `credentials_verified=false` et `worker_capabilities_verified=false` sont explicites. Aucun `doctor providers` autonome n'est livré ; `doctor` concerne le moteur.

| Option de `run` | Contrat livré |
|---|---|
| `--client codex\|claude-code\|auto` | Client explicite ou résolution parmi les profils configurés ; pas de détection par environnement ambiant. |
| `--auth-mode api\|subscription\|auto` | Filtre explicite du mode ; Auto considère les profils abonnement configurés avant les profils API. |
| `--client-profile ID` | Profil fournisseur exact, qui doit correspondre aux filtres explicites. Distinct de `--profile`, le profil de politique. |
| `--providers-file PATH` | Fichier privé explicite ; défaut `<config-dir>/providers.json`. |
| `--model ID` | Remplace le modèle du profil ; aucun modèle de remplacement silencieux. |
| `--prompt TEXTE` | Obligatoire en batch, facultatif avec `--interactive` ; fait partie de l'argv autorisé, donc aucun secret. |
| `--interactive` | Transport TTY autorisé, vrai terminal requis ; incompatible avec `--capture-output`. |

La sélection explicite doit rester cohérente avec le profil choisi. Sans profil explicitement sélectionné, Auto applique le filtre abonnement/API, puis le défaut compatible et l'ordre `auto_priority` ; plusieurs candidats sans priorité rendent `ambiguous_configuration`. L'absence du credential d'un abonnement retenu rend `login_required`, sans tentative API. Le choix non secret est affiché avant décision, avec avertissement de facturation pour une API. Les credentials exigent toujours observe + bridge explicites. Les options gérées ne se combinent pas avec un argv manuel, `--secret-env-file` ou un `--provider` non générique.

### Configuration privée et matériel d'authentification

Le schéma version1 est strict : `version`, `default_profile` facultatif, `auto_priority`, `profiles`. Chaque profil contient `id`, `revision` positive, `client`, `auth_mode`, `model` facultatif et une référence `credential`. Dans le JSON, Claude s'écrit `"client":"claude_code"` ; la CLI emploie `--client claude-code`. L'image reste choisie par `run --image`, pas par le profil.

Exemple sans valeur secrète, pour deux profils API dont les sources sont adjacentes à la configuration privée :

```json
{
  "version": 1,
  "auto_priority": ["codex-api", "claude-api"],
  "profiles": [
    {
      "id": "codex-api", "revision": 1,
      "client": "codex", "auth_mode": "api",
      "credential": {"kind": "api_env_file", "path": "credentials/api.env", "key": "OPENAI_API_KEY"}
    },
    {
      "id": "claude-api", "revision": 1,
      "client": "claude_code", "auth_mode": "api", "model": "claude-haiku-5-5",
      "credential": {"kind": "api_env_file", "path": "credentials/api.env", "key": "ANTHROPIC_API_KEY"}
    }
  ]
}
```

Les références relatives sont ancrées au parent du `providers.json` privé retenu, jamais au répertoire courant. Traversées et liens sont refusés. Le runner et `status` partagent le même parseur, sans seconde ouverture indépendante du fichier dans le runner. La configuration est bornée à64Kio ; les fichiers sont privés, possédés, réguliers, sans lien physique ni ACL étendue admise.

| Client / mode | Référence du profil | Matériel sélectionné |
|---|---|---|
| Codex / API | `api_env_file` ; `key` explicite `OPENAI_API_KEY` ou `CODEX_API_KEY` | Une seule clé ; deux alias présents sans choix explicite sont ambigus. |
| Claude / API | `api_env_file` ; `key` `ANTHROPIC_API_KEY` | Une seule clé Anthropic. |
| Codex / abonnement | `codex_auth_file` avec `path` | Un `auth.json` ChatGPT volontairement préparé, validé, sans clé API non nulle. |
| Claude / abonnement | `claude_oauth_env_file` avec `path` | Seulement `CLAUDE_CODE_OAUTH_TOKEN`, obtenu volontairement par le mécanisme officiel. |

Pour ajouter un abonnement, créer un profil `auth_mode=subscription` avec la source correspondante et un identifiant propre, puis le sélectionner explicitement. Ne pas configurer un profil dont le credential n'est pas prêt en comptant sur un fallback API. Les aides officielles de connexion restent celles citées dans la proposition ; aucun assistant opérateur `auth login/import/status` n'est livré. Aucun cache OAuth ou keychain hôte n'est exploré ni réécrit automatiquement.

Le runner accepte dans le projet uniquement le `.env` racine explicitement référencé, physiquement exclu de la copie ; les autres sources doivent être extérieures. Seul le matériel du client/mode retenu entre dans le handoff, même si le `.env` contient d'autres clés, dont une clé Jev. Le chemin manuel historique `--secret-env-file`, extérieur au projet, injecte toutes ses variables acceptées et exige donc un fichier dédié ; `JEV_API_KEY` et `JEV_API_KEY_FILE` y sont refusés avant snapshot. Les collisions par valeur avec des credentials du contrôle restent refusées.

### Autorisation, bootstrap et reprise

La résolution publique version1 lie client, mode, transport, profil/révision, modèle éventuel, provenance de chaque choix, preset d'invocation et UUID opaque de binding. `credential_names` vaut exactement `["AI_GUARD_PROVIDER_BOOTSTRAP"]`. Aucun chemin privé, secret ou digest de credential n'est public. `provider_resolution` est identique dans demande et admission, participe aux hashes et à l'audit ; champ absent signifie historique, `null` explicite et valeurs inconnues sont refusés. Les nouveaux parcours gérés exigent NoLogs : batchv2 ou TTYv3 selon le transport, jamais le preset historiquev1. Voir [le contrat API](../api.md#demande-de-décision).

Le plan3 et `credential-bindings/<UUID>.json` sont privés. Le binding conserve les octets canoniques du bootstrap sélectionné et son dictionnaire de masquage, synchronisés avant décision. Approbation et reprise réutilisent ce choix sans relire la source ou résoudre Auto. Le premier démarrage repris compare les octets au digest admis ; une modification exige une nouvelle demande. Les plans1/2 gardent leur sens historique, leurs claims consommés et leurs opérations existantes, sans conversion. Un claim consommé ne permet ni second démarrage ni réattachement TTY.

Le bootstrap utilise les binaires absolus de l'image, un environnement reconstruit et un home temporaire privé. Codex/API TTY effectue le login officiel avec la clé sur stdin privé ; Codex/abonnement installe le seul fichier explicitement préparé. Claude reçoit uniquement la variable du mode choisi, avec configuration isolée et sources de settings contrôlées. Les sondes version/aides sont bornées, après admission et avant appel modèle ; leurs variables d'environnement excluent les credentials. Pour Codex abonnement, `auth.json` est toutefois déjà écrit dans le `CODEX_HOME` privé conservé par la sonde, qui peut donc accéder à ce fichier. Les sondes d'image sans handoff sont distinctes et n'ont aucun credential. Les diagnostics distinguent notamment `client_missing`, `version_incompatible`, `login_required`, `credential_missing`, `credential_unsafe` et `ambiguous_configuration` ; ils ne prouvent pas qu'un serveur acceptera le credential.

Les credentials restent lisibles par le client, le code worker et les autorités du moteur. Le masquage initial couvre le matériel connu, sans garantie sur des tokens OAuth renouvelés inconnus. Rétention des bindings, rotation/refresh, diagnostic avant admission et qualification adverse des réglages des clients épinglés restent ouverts. Une rotation du fichier source ne modifie pas un plan déjà autorisé ; préparer une nouvelle demande, sans éditer ou supprimer les preuves pour forcer une reprise.

### Preuves réelles historiques et limites actuelles

- Codex0.165.0 : PASS manuel batch avec fichier et export vérifiés, après un FAIL de sandbox imbriqué conservé.
- Claude2.1.140 : PASS manuel API batch sur l'ancien binaire et l'ancienne image QA, avec deux fichiers exportés vérifiés. L'alias demandé `haiku` ne prouve pas le modèle résolu et ne qualifie pas Haiku5.5.
- Claude TUI : FAIL de qualification causé par l'instrumentation après démarrage ; ni réponse du premier tour ni conversation à deux tours établie. Le préflight ultérieur du wrapper est une preuve hors ligne, sans retry fournisseur.
- Nouvelle image bootstrap : build et sondes sans credential PASS sur Docker Desktop/macOS ARM64, puis invocation Claude/API batch gérée PASS ciblé. Les deux préflights (gateway éteint puis exception rootful manquante) sont conservés ; aucun appel modèle avant la tentative réussie. Les autres authentifications, abonnements, TUI et réglages adverses demeurent non qualifiés. Le warning postinstall n'a pas empêché cette commande batch, sans prouver le reste de l'installation.
