# ai-guard — Agent Sandbox

**Exécuter un agent ou une commande dans une copie isolée d'un projet, puis consulter ses actions, ses décisions de sécurité et ses événements dans une IHM locale conteneurisée.**

Le backend, le collecteur, le moteur de politiques, le client Jev et la CLI sont écrits en **Rust**. Le frontend est en **TypeScript**, sans framework d'exécution. Le dashboard et le collecteur tournent dans des conteneurs Docker ou Podman distincts, derrière un relais de publication Rust sans credential monté ; chaque session de travail possède son propre worker et son volume. Le dashboard est séparé du domaine de confiance de l'agent. Les essais réels de cette topologie sur Docker Desktop macOS ARM64 et leurs limites sont consignés dans les preuves de validation.

ai-guard est en cours d'industrialisation vers un produit open source local-first, avec un cœur commun pour le cloud. La qualification production n'est pas encore acquise. L'[état réel](docs/engineering/CURRENT_STATE.md), les [résultats courants](docs/engineering/VALIDATION.md) et les [gates de qualification](docs/engineering/PRODUCTION_READINESS.md) distinguent code livré, preuves logicielles et essais moteur/fournisseur restant à effectuer. Aucun résultat sur fixture ne constitue une validation d'un agent cloud réel.

Le projet est distribué sous [licence Apache-2.0](LICENSE), retenue avec l'autorisation du mainteneur. Le canal privé de signalement des vulnérabilités reste à définir avant publication.

**Commencer par le [Quick start](docs/quickstart.md)** : outils requis, configuration privée, Docker ou Podman, connexion au dashboard, première commande hors ligne, refus, modification isolée, export, arrêt et reprise. Il inclut les commandes utiles, les réglages réels et les limites des agents fournisseurs. Aucun compte cloud n'est nécessaire au premier parcours.

Le mandat porte sur un produit complet, utile et soigné : parcours de bout en bout, erreurs et reprise compréhensibles, données réelles, accessibilité, documentation et maintenance. Les lots du [plan d'implémentation](docs/engineering/IMPLEMENTATION_PLAN.md) organisent le travail ; aucune étiquette MVP/V2 ne permet de reporter une exigence obligatoire. Voir le [mandat commun](PROMPT_PRODUCTION.md) et les [critères d'acceptation](docs/acceptance.md).

## Ce que le produit fait

- Prépare une copie filtrée du projet dans un volume de session, sans monter ni modifier le projet original.
- Demande une décision tracée avant le lancement ; une indisponibilité de l'audit ou un refus bloque le parcours.
- Configure un worker non privilégié, une racine en lecture seule, des limites de ressources, un timeout et un réseau coupé par défaut.
- Distingue demande, décision, soumission au runtime, démarrage observé et résultat.
- Conserve un audit SQLite indépendant du worker, avec masquage, pagination, SSE, exports JSON/JSONL et chaîne de hashes.
- Sépare les API d'audit par identité authentifiée et base SQLite liée au client ; la [configuration multi-client](docs/engineering/CLOUD_TENANCY.md) précise le périmètre livré et les fonctions encore manquantes.
- Normalise les sorties structurées Codex, Claude et d'agents personnalisés, en les étiquetant comme déclarations.
- Affiche une hiérarchie d'agents personnalisés à partir d'identifiants parent/enfant explicitement déclarés.
- Exporte les fichiers d'une session vers une nouvelle destination contrôlée.

Le broker Rust s'exécute sur le poste de confiance et utilise la CLI Docker/Podman. Aucun service conteneurisé ne reçoit le socket du moteur. Il n'existe aucun repli consistant à exécuter la commande métier sur l'hôte si le moteur est absent.

## Organisation du dépôt

| Chemin | Rôle |
|---|---|
| `front/` | Interface TypeScript, styles, tests et configuration de build Vite. |
| `src/` | Application Rust : CLI/broker, runtime, politique, Jev, audit, API et adaptateurs. |
| `deploy/docker/` | Stack Compose Docker et exemple d'activation explicite de Jev. |
| `deploy/podman/` | Stack Compose Podman. |
| `images/` | Dockerfiles communs du service de contrôle et du worker. |
| `policies/` | Politique déterministe initiale, fondée sur des invocations exactes. |
| `schemas/` | Schéma des événements normalisés. |
| `tests/` | Contrats, sécurité, persistance, import/export et test moteur explicite. |
| `docs/` | Architecture, exploitation, API, compatibilité, sécurité et validation. |

La CLI construit `agentsandbox-control:local` et `agentsandbox-worker:local`. Le premier contient le binaire Rust et les fichiers statiques de l'interface ; le même binaire démarre le gateway, le dashboard ou l'ingress, dans trois conteneurs distincts.

## Démarrer avec Docker ou Podman

Le [guide pas à pas](docs/quickstart.md) accompagne chaque résultat attendu. Les exemples ci-dessous recommandent une configuration privée extérieure au checkout et au projet importé ; conserver la même configuration pour toute la vie d'une installation. Le défaut CLI `.agentsandbox` reste pris en charge : sa racine physique est exclue de la copie lorsqu'elle se trouve dans le projet.

### Prérequis

Utiliser Rust **1.99.1**, fixé par `rust-toolchain.toml`, pour compiler la CLI hôte. Il faut un moteur fonctionnel et Compose : Docker avec son plugin Compose, ou Podman avec un fournisseur `podman compose`. Node 24 est utilisé dans le build des images ; il n'est nécessaire sur l'hôte que pour travailler directement sur le frontend. Les ports locaux `4000` et `8787` doivent être libres.

Le fonctionnement local n'exige aucun compte cloud. La construction initiale télécharge toutefois des images et dépendances. Le mode prioritaire est Linux rootless ; macOS/Docker Desktop, WSL, cgroups et SELinux nécessitent la qualification décrite dans les critères d'acceptation.

Depuis la racine du dépôt :

```sh
cargo build --release --locked
AIGUARD_BIN="$PWD/target/release/agentsandbox"
AIGUARD_CONFIG="$HOME/.ai-guard"
AIGUARD_PROJECT="/chemin/absolu/vers/votre-projet"
"$AIGUARD_BIN" init --config-dir "$AIGUARD_CONFIG"
```

Choisir **une** des deux stacks, qui utilisent les mêmes ports locaux.

**Docker rootless :**

```sh
"$AIGUARD_BIN" doctor --runtime docker
"$AIGUARD_BIN" build --runtime docker
"$AIGUARD_BIN" up --runtime docker --config-dir "$AIGUARD_CONFIG"
```

**Podman rootless :**

```sh
"$AIGUARD_BIN" doctor --runtime podman
"$AIGUARD_BIN" build --runtime podman
"$AIGUARD_BIN" up --runtime podman --config-dir "$AIGUARD_CONFIG"
```

**Docker Desktop ou moteur qui ne signale pas le mode rootless :** le fonctionnement normal refuse ce moteur. Si l'opérateur choisit cette exception, ajouter explicitement `--allow-rootful-runtime` à `doctor`, `build`, `up` et à chaque `run` concerné :

```sh
"$AIGUARD_BIN" doctor --runtime docker --allow-rootful-runtime
"$AIGUARD_BIN" build --runtime docker --allow-rootful-runtime
"$AIGUARD_BIN" up --runtime docker --allow-rootful-runtime --config-dir "$AIGUARD_CONFIG"
```

La VM de Docker Desktop n'est pas présentée comme une preuve de mode rootless. Les sessions utilisant cette exception enregistrent `runtime.security_downgrade`.

`doctor` et `build` n'acceptent pas `--config-dir`. Pour un moteur installé à un chemin personnalisé, ajouter `--runtime-program "/chemin/absolu/vers/docker"` ou Podman à **chaque** `doctor`, `build`, `up`, `run` et `down` concerné. `worker-stop`, `worker-resume` et `export-workspace` acceptent aussi cette option lorsque l'installation l'exige ; `init`, `worker-status`, `verify-audit` et `healthcheck` ne la prennent pas. Il ne s'agit pas d'un réglage global ; voir le [guide](docs/quickstart.md#2-initialiser-et-démarrer) et la procédure d'adoption d'un programme mis à jour dans [Exploitation](docs/operations.md#programme-et-cible-du-moteur).

Ouvrir **[http://127.0.0.1:4000](http://127.0.0.1:4000)**. Ouvrir localement `secrets/viewer` sous `$AIGUARD_CONFIG` dans un éditeur privé et saisir son contenu dans la connexion du dashboard. La CLI ne l'imprime pas. Le fichier `runner` est réservé au broker ; il ne doit pas être utilisé dans l'IHM.

`init` crée deux secrets indépendants et conserve les valeurs existantes. Les originaux restent privés (`0600`) ; des copies destinées au montage sont lisibles par les services non privilégiés, dans un répertoire hôte privé (`0700`). Les détails de permissions et les emplacements personnalisables avec `--config-dir` sont documentés dans [Exploitation](docs/operations.md). Garder le dossier de configuration privé et utiliser le même dossier pour les commandes d'une installation.

`up` recrée les services de contrôle pour remonter les fichiers de secrets actualisés. Les volumes sont conservés, mais les connexions au collecteur sont interrompues pendant ce redémarrage : terminer les jobs actifs avant de relancer `up`.

## Lancer une commande

Remplacer la valeur de `AIGUARD_PROJECT` par le projet à copier. Choisir `--runtime podman` pour la stack Podman. Avec Docker Desktop non rootless, ajouter l'exception décrite ci-dessus.

```sh
"$AIGUARD_BIN" run \
  --runtime docker --config-dir "$AIGUARD_CONFIG" \
  --project "$AIGUARD_PROJECT" \
  --profile strict \
  --capture-output \
  -- /usr/bin/id
```

`--` sépare les options du broker de la commande exécutée dans le worker. Le résultat contient l'UUID de session, son volume, le code de sortie et le nombre de fichiers modifiés lorsque le scan aboutit. `--capture-output` demande la conservation bornée et masquée de stdout/stderr ; cette capture est désactivée par défaut.

Les restrictions configurées par défaut sont : utilisateur `1000:1000`, réseau `none`, racine en lecture seule, capacités supprimées, `no-new-privileges`, 1 Gio de mémoire, 2 CPU, 256 PID et timeout de 300 secondes. Leur application effective dépend du moteur et doit être vérifiée sur la machine cible. `--timeout` règle la durée maximale, entre 1 et 86 400 secondes.

### Profils de sécurité

| Profil | Autorisation de premier niveau | Réseau |
|---|---|---|
| `strict` | Invocation exacte présente dans `policies/default.json`. | `none`. |
| `review` | Allowlist ou confirmation humaine liée au hash complet de la demande ; les refus explicites restent bloquants. | `none`. |
| `observe` | Invocation journalisée avec maintien des refus directs de politique ; les commandes transitives ne sont pas contrôlées individuellement. | `none`, ou `bridge` sur choix explicite. |

L'allowlist initiale est courte : `id`, `pwd`, `echo sandbox-ok` et une invocation précise de `printf`. Un script de build ou de tests peut exécuter du code du projet ; il nécessite une revue ou le profil observe.

```sh
"$AIGUARD_BIN" run \
  --runtime docker --config-dir "$AIGUARD_CONFIG" \
  --project "$AIGUARD_PROJECT" \
  --profile review \
  -- cargo test --offline
```

Le terminal demande de recopier le SHA-256 complet affiché. Sans terminal, la demande reste en attente et rien n'est lancé. Reprendre avec les mêmes options et `--approve-hash HASH_COMPLET` consomme cette autorisation pour cette demande exacte. Il n'existe pas de `--yes` global. Les dépendances de votre projet ne sont pas automatiquement disponibles hors ligne dans l'image.

Pour observer une modification simple dans la copie :

```sh
"$AIGUARD_BIN" run \
  --runtime docker --config-dir "$AIGUARD_CONFIG" \
  --project "$AIGUARD_PROJECT" \
  --profile observe \
  -- /usr/bin/touch /workspace/agent-sandbox-example.txt
```

Cette commande utilise le réseau `none`. Le fichier apparaît dans le volume de cette nouvelle session, pas dans le projet source.

## Agents cloud et identifiants explicites

L'image worker générique ne contient aucun CLI fournisseur par défaut. Leur installation optionnelle exige des versions exactes choisies et vérifiées par l'opérateur :

```sh
"$AIGUARD_BIN" build --runtime docker --codex-version X.Y.Z --claude-code-version A.B.C
```

Les lettres sont des espaces réservés, pas des versions validées. Les tags `latest`, jokers et plages sont refusés. Les paquets officiels sont [@openai/codex](https://github.com/openai/codex) et [@anthropic-ai/claude-code](https://code.claude.com/docs/en/setup#install-with-npm).

Préparer un fichier de credentials extérieur au projet, privé au propriétaire (`0600`), contenant des lignes littérales `NAME=value`. Il n'est pas interprété par un shell. Utiliser `CODEX_API_KEY` pour [Codex non interactif](https://learn.chatgpt.com/docs/non-interactive-mode) et `ANTHROPIC_API_KEY` pour [Claude en mode `-p`](https://code.claude.com/docs/en/env-vars). Ne placer aucune clé dans le prompt ou les arguments.

```sh
"$AIGUARD_BIN" run \
  --runtime docker --config-dir "$AIGUARD_CONFIG" \
  --project "$AIGUARD_PROJECT" \
  --profile observe --network bridge --provider codex \
  --secret-env-file "$AIGUARD_CONFIG/codex.env" --timeout 300 \
  -- codex exec --json --model gpt-6-luna \
  --sandbox danger-full-access --skip-git-repo-check --ephemeral \
  -c 'approval_policy="never"' \
  "Create only a new file named result.txt at the workspace root. Its full content must be exactly: Codex local smoke test completed. Do not read or modify other files, run tests, or start subagents."
```

Cette recette demande une image contenant Codex **0.165.0**, un fichier API privé et un projet où `result.txt` est absent. Le projet copié exclut `.git` ; l'option `--skip-git-repo-check` répond au contrôle de dépôt du mode `codex exec`. L'essai ciblé a nécessité le choix explicite du sandbox interne ci-dessus : `workspace-write` échouait sur le namespace utilisateur de `bwrap`. Les contraintes du conteneur ai-guard restent appliquées. Suivre ensuite l'[export neuf et la vérification de `result.txt`](docs/quickstart.md#ajouter-un-agent-fournisseur), avec l'UUID de cette session ; le guide décrit aussi la préparation de l'image et les limites de la preuve. Pour Claude, l'exemple suivant est une exploration sans création/export attendu ; son appel modèle n'est pas qualifié :

```sh
"$AIGUARD_BIN" run \
  --runtime docker --config-dir "$AIGUARD_CONFIG" \
  --project "$AIGUARD_PROJECT" \
  --profile observe --network bridge --provider claude \
  --secret-env-file "$AIGUARD_CONFIG/claude.env" \
  -- claude -p "Décris la structure du projet" --output-format stream-json --verbose
```

Le réseau bridge est sortant **sans filtrage applicatif**. Les secrets injectés sont accessibles à l'agent et au code exécuté dans le worker ; leur masquage dans les traces ne change pas cette propriété. Le daemon et son administrateur appartiennent aussi au périmètre de confiance. La protection des credentials et les restrictions egress du périmètre annoncé restent des écarts ouverts tant qu'elles ne sont pas implémentées et qualifiées.

Le home du worker est temporaire et stdin est fermé pour la commande métier : aucun TTY ni login interactif fournisseur n'est pris en charge. Codex 0.165.0 a produit un fichier puis un export vérifié lors d'un essai réel ciblé Docker Desktop ARM64. Claude Code 2.1.140 a passé uniquement `--version` et `--help` en worker ; authentification et appel modèle restent `NOT_RUN`. Ces résultats ne qualifient ni les actions transitives ni les autres plateformes. Voir la [validation datée](docs/engineering/VALIDATION.md).

## Lire les actions et les événements

Les vues et leurs états vides/erreurs sont décrits dans [docs/ui.md](docs/ui.md). L'acceptation du produit exige aussi les états partiels, périmés, déconnectés et interdits, avec une reprise utile et une accessibilité vérifiée selon [docs/acceptance.md](docs/acceptance.md).

| Vue | Contenu |
|---|---|
| Vue d'ensemble | Comptes, décisions, activité récente, état du collecteur et intégrité. |
| Agents | Tableau, arbre explicite, détails, provenance par champ et historique. |
| Actions | Demandes, décisions, lancements et résultats ; recherche, filtres et détails JSON. |
| Sécurité | Refus, approbations et décisions du moteur de politique ou de Jev. |
| Fichiers | Import/export et différences de contenu observées à l'échelle de la session. |
| Sandboxes | Runtime, image, profil, réseau et informations de conteneur disponibles. |
| Workers | Registre durable, dernières observations moteur et détails par tentative. |
| Quotas | Réservations et limites du client authentifié. |
| Allocations | Temps d'allocation CPU/RAM attesté, sans facturation. |

Le journal est paginé et actualisé par SSE. Le tableau live conserve au plus 1 000 événements ; les précédents restent consultables. Le sélecteur de période propose toutes les périodes, la dernière heure, 24 heures et 7 jours. L'export reprend tous les filtres appliqués, y compris la période, et inclut toutes les pages correspondantes. Sans filtre, il contient le journal complet retourné par l'API.

Deux notions sont affichées séparément :

| Notion | Signification |
|---|---|
| Provenance **Observé** | Relevé par le broker/runtime ou enregistré par le moteur de décision. |
| Provenance **Déclaré** | Fourni par le CLI ou l'agent ; ce n'est pas une preuve indépendante d'effet système. |
| Provenance **Inféré** | Déduction signalée comme telle. |
| Couverture **Native / Observée / Générique** | Niveau d'intégration ; le nom du fournisseur ne suffit pas à l'attribuer. |

Les compteurs observés et déclarés restent séparés. Les parseurs Codex/Claude livrés traitent le stdout borné à la fin de la session et ne reconstruisent pas leurs sous-agents natifs. Le contrat générique permet de déclarer explicitement un arbre ; les parents absents, contradictions et cycles sont signalés. Voir [Adaptateurs](docs/adapters.md) et [Compatibilité](docs/compatibility.md).

Les métriques CPU/RAM, coûts et autres mesures indisponibles restent `null` et s'affichent « Inconnu ». Une limite de ressources configurée n'est pas une mesure de consommation. Le résultat d'une commande peut fournir sa durée ; l'agrégation de durée globale des sessions reste à compléter. Le [contrat d'usage](docs/engineering/USAGE_METERING.md) distingue allocation, consommation et facturation ; aucune valeur inconnue ne devient zéro.

## Exporter, vérifier et arrêter

À partir de l'UUID renvoyé par `run`, exporter dans une destination **absente**, dont le parent existe, extérieure au projet original :

```sh
AIGUARD_SESSION="UUID_DE_LA_SESSION"
AIGUARD_EXPORT="/autre/repertoire/resultat-neuf"
"$AIGUARD_BIN" export-workspace --config-dir "$AIGUARD_CONFIG" \
  --session "$AIGUARD_SESSION" --output "$AIGUARD_EXPORT"
```

L'export réutilise le runtime et les options enregistrés pour la session. Il vérifie les chemins et rejette liens, fichiers spéciaux, doublons et dépassements de taille. Aucun fichier de la destination n'est remplacé. Les scans et exports sont limités à 20 000 entrées, 128 Mio de contenu au total et 32 Mio par fichier ; certaines dépendances produisant des liens peuvent rendre l'export impossible.

```sh
"$AIGUARD_BIN" verify-audit --config-dir "$AIGUARD_CONFIG"
"$AIGUARD_BIN" down --runtime docker --config-dir "$AIGUARD_CONFIG"
```

Utiliser `--runtime podman` pour arrêter la stack Podman. `down` conserve les volumes d'audit et de session. Le dashboard est consultatif : il ne peut ni lancer de commande, ni approuver une demande, ni supprimer un volume. La CLI fonctionne sans dashboard, mais a besoin du collecteur pour autoriser et tracer une exécution.

## Jev optionnel

Le client Rust implémente l'API officielle `POST /v1/systemone`, avec un modèle choisi explicitement et une clé séparée. Jev est désactivé par défaut. Un refus déterministe ne peut pas être transformé en autorisation par Jev ; erreur, timeout ou réponse non concluante entraînent un refus conservatoire.

L'[override Compose](deploy/docker/compose.jev.example.yaml) montre l'activation, avec sortie réseau du gateway et secret dédié. `agentsandbox up` ne charge pas cet override automatiquement. L'activation utilise un endpoint réel ; elle n'installe **aucun modèle Jev auto-hébergé**. Un proxy local ne démontre pas l'hébergement local du modèle. Voir [Architecture](docs/architecture.md), [Exploitation](docs/operations.md) et [Sources officielles](docs/sources.md).

## Vérifier le logiciel

La compilation et les tests logiciels ne nécessitent pas Docker/Podman. Certains tests HTTP utilisent un port loopback éphémère.

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Pour le frontend, avec Node 24 :

```sh
cd front
npm ci
npm test
npm run build
```

Les tests navigateur et leurs prérequis Chromium/Playwright figurent dans [Interface](docs/ui.md). `test:browser` utilise des fixtures isolées ; `test:browser:live` cible les vrais services configurés avec des événements QA explicitement déclarés. Aucun de ces parcours ne lance un agent cloud ; les preuves et prérequis du parcours live sont dans [la validation courante](docs/engineering/VALIDATION.md).

Le test moteur est ignoré par défaut. Après construction des images et démarrage du gateway, l'exécuter explicitement depuis la racine :

```sh
AGENTSANDBOX_TEST_RUNTIME=docker \
AGENTSANDBOX_TEST_CONFIG="$AIGUARD_CONFIG" \
cargo test --test runtime_smoke -- --ignored --nocapture
```

Choisir `podman` pour l'autre moteur. Pour une exception Docker Desktop non rootless déjà choisie, ajouter `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1`. Ce test produit de vraies sessions et conserve leurs volumes. Les [18 scénarios d'acceptation](docs/acceptance.md) complètent cette vérification ; leur exécution externe reste à faire.

## Limites à connaître

- Le confinement repose sur le moteur, son noyau et les images de confiance. Une sandbox inviolable, ou une protection contre l'administrateur du poste, n'est pas promise.
- Les politiques contrôlent l'invocation confiée au broker. Les parseurs de logs ne bloquent pas les commandes internes déjà exécutées par un agent.
- Les modifications de contenu sont constatées au niveau session. Les diffs Git détaillés et la visibilité sur les commits demandés restent à compléter ; `.git` n'est pas importé. Une attribution à un sous-agent exige une preuve propre et ne doit pas être déduite arbitrairement.
- Les tentatives de syscalls ou de connexions réseau refusées ne sont pas toutes collectées. L'absence d'événement n'atteste pas l'absence de tentative.
- Les secrets connus et formats reconnus sont masqués ; un secret arbitraire inconnu peut échapper à cette détection. La copie exclut des noms sensibles connus, sans analyser tout contenu comme un scanner de secrets.
- L'audit a des quotas et refuse de nouvelles écritures lorsqu'il est plein. Rotation/rétention configurables, sauvegarde/restauration et reprise restent à compléter et qualifier. Les signatures et l'ancrage externe sont des protections à évaluer selon la menace annoncée ; la chaîne locale ne résiste pas à la réécriture complète par un administrateur.
- La stack est prévue pour localhost. Une exposition distante exige une conception TLS, authentification et droits adaptée ; elle n'est pas fournie par le déploiement local.

## Documentation

| Document | À consulter pour |
|---|---|
| [Quick start](docs/quickstart.md) | Installer depuis le checkout et suivre le premier parcours complet, avec commandes et dépannage. |
| [Architecture](docs/architecture.md) | Frontières de confiance, choix de stack et cible produit complète. |
| [Exploitation](docs/operations.md) | Toutes les commandes, secrets, limites de copie, reprise et moteurs. |
| [Interface](docs/ui.md) | Vues, états incomplets, accessibilité, sécurité DOM et tests navigateur. |
| [API](docs/api.md) | Authentification, événements, filtres, SSE et export. |
| [Modèle de données](docs/data-model.md) | Identités, provenance par champ, événements, dates UTC et projections. |
| [Adaptateurs](docs/adapters.md) | JSONL, hiérarchie déclarative, mappings et limites. |
| [Compatibilité](docs/compatibility.md) | Capacités officielles étudiées et niveau effectivement livré. |
| [Sécurité](docs/security.md) | Menaces, mécanismes, garanties et limitations. |
| [Couverture des exigences](docs/requirements-coverage.md) | Correspondance des 13 sections et des 18 scénarios du besoin. |
| [Acceptation](docs/acceptance.md) / [Validation courante](docs/engineering/VALIDATION.md) | Résultats attendus / commandes et résultats du checkout ; [validation historique](docs/validation.md) conservée séparément. |
| [Sources](docs/sources.md) | Références primaires et distinctions entre faits et conception. |
