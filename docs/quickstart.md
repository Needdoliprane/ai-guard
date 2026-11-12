# Quick start — ai-guard

Ce parcours part d'un checkout existant et mène à une première commande autorisée, une modification isolée, puis un export vérifié. Le fonctionnement local n'exige aucun compte cloud. La construction initiale télécharge des images et des dépendances ; les premières sessions ci-dessous gardent le réseau coupé.

ai-guard vise un produit open source destiné à la production. Sa qualification globale reste ouverte : lire l'[état courant](engineering/CURRENT_STATE.md) et les [gates](engineering/PRODUCTION_READINESS.md) avant de retenir un environnement d'exploitation. Les essais Docker Desktop/macOS ARM64 ne qualifient pas Podman, Linux ou Kubernetes.

Ce parcours utilise le contrôle schéma2 par défaut. Le [bootstrap explicite d'un contrôle neuf en schéma3](operations.md#initialiser-un-contrôle-de-supervision-neuf) prépare le broker permanent en cours d'intégration ; il emploie des bases distinctes et ne remplace pas le démarrage ci-dessous.

Le guide ci-dessous utilise les clés techniques `runner`/`viewer` ; elles ne prouvent pas une identité humaine nominative. Les guides d'[accès local nominatif](setup/identity/README.fr.md) et de [SSO Web/CLI](setup/sso/README.fr.md) décrivent les procédures distinctes de consultation et d'export, dont la préparation explicitement activée d'une installation Keycloak de démonstration neuve. `init` ne crée pas ces comptes et n'active pas le SSO. Le [parcours complet de demande humaine au broker](engineering/HUMAN_EXECUTION.md) reste proposé, sans commande de lancement humain livrée. Environ cinq minutes reste une cible d'expérience à mesurer, sans promesse ; les limites de qualification de chaque parcours restent explicites dans ses guides.

Le client construit et distribue ses images depuis les Dockerfiles fournis, y compris leur préparation/import dans un environnement sans Internet. Aucune image publique préconstruite n'est obligatoire. Le [profil d'inférence interne](sso.md#inférence-interne-et-déploiement-sans-internet) reste à implémenter et qualifier ; les commandes actuelles de clé API ne prouvent pas ce parcours hors ligne.

## 1. Préparer les outils

Les [parcours d'identité et leur état](sso.md#état-livré-et-parcours-disponibles) distinguent les clés techniques, les comptes nominatifs locaux et OIDC. Pour configurer OIDC, suivre le [guide opérateur SSO](setup/sso/README.fr.md) avec les versions correspondantes du gateway, du dashboard compilé et du CLI ; le contrat technique seul n'active aucun service. Les quatre comptes publics du [profil Keycloak de démonstration](setup/keycloak/README.fr.md) ne remplacent jamais **Jeton de lecture**. `providers init` et `providers status` concernent l'authentification Codex/Claude, distincte du SSO ai-guard.

| Outil | Utilité | Nécessaire sur l'hôte |
|---|---|---|
| Rust **1.99.1**, Cargo et rustup | Compiler la CLI Rust du contrôle de confiance. La version est fixée dans [`rust-toolchain.toml`](../rust-toolchain.toml). | Oui. |
| Docker + plugin Compose **ou** Podman rootless + fournisseur `podman compose` | Construire les images et exécuter les services et workers. Choisir un seul moteur pour ce parcours. | Oui, avec un moteur démarré. |
| Navigateur | Consulter le dashboard local. | Pour l'IHM ; la CLI fonctionne aussi sans elle. |
| Node **24** et npm | Développer ou tester directement `front/`. Le build Docker/Podman les fournit déjà pour compiler l'interface. | Seulement pour le développement frontend. |
| Codex / Claude Code | Exécuter un fournisseur dans un worker avec une version explicitement installée. | Facultatif ; aucune installation hôte ni connexion fournisseur n'est reprise automatiquement. |

Le contrôle nécessite Unix. Les plateformes et versions effectivement essayées figurent dans la [validation](engineering/VALIDATION.md). Les ports **127.0.0.1:4000** et **127.0.0.1:8787** doivent être libres. La pile locale n'est pas un déploiement distant avec TLS.

Pour préparer des essais de certificats, le [générateur local de PKI](demo-pki.md) fournit racine, intermédiaire et chaîne serveur avec une commande Rust. Il crée un dossier privé neuf, sans modifier la confiance système ni le DNS. Le démarrage HTTP ci-dessous n'en dépend pas ; générer les fichiers ne configure pas automatiquement HTTPS.

Depuis la racine du checkout, dans le même terminal pour la suite :

```sh
rustup toolchain install 1.99.1 --profile minimal --component rustfmt --component clippy
cargo build --release --locked
AIGUARD_BIN="$PWD/target/release/agentsandbox"
AIGUARD_CONFIG="$HOME/.ai-guard"
AIGUARD_PROJECT="/chemin/absolu/vers/votre-projet"
"$AIGUARD_BIN" --version
```

Remplacer le chemin du projet par un répertoire réel à examiner. Pour simplifier sa protection, ce guide recommande une configuration **extérieure au checkout et au projet importé**, sous un parent de confiance. `$HOME/.ai-guard` est l'exemple d'une installation neuve. Le défaut CLI reste `.agentsandbox` ; une configuration placée dans le projet n'est pas interdite par principe et sa racine physique est exclue de la copie worker. Si une installation existe déjà, conserver ses fichiers et suivre la [procédure de mise à jour](operations.md). Ces variables de terminal contiennent uniquement des chemins, aucun secret.

## 2. Initialiser et démarrer

```sh
"$AIGUARD_BIN" init --config-dir "$AIGUARD_CONFIG"
"$AIGUARD_BIN" doctor --runtime docker
"$AIGUARD_BIN" build --runtime docker
"$AIGUARD_BIN" up --runtime docker --config-dir "$AIGUARD_CONFIG"
```

Pour **Podman rootless**, remplacer `--runtime docker` par `--runtime podman` dans `doctor`, `build`, `up`, chaque `run` et `down`. Les stacks partagent les mêmes ports ; ne pas les démarrer simultanément.

Pour **Docker Desktop ou un daemon qui ne déclare pas rootless**, le produit refuse le démarrage normal. Si vous choisissez cette exception, ajouter explicitement `--allow-rootful-runtime` à `doctor`, `build`, `up` et à chaque `run`. Ce parcours ne produit pas actuellement l'événement `runtime.security_downgrade` ; sa preuve d'audit dédiée reste à implémenter et qualifier (voir [exploitation](operations.md#ce-que-lance-le-produit)). La VM Desktop n'est pas une preuve de fonctionnement rootless.

`doctor` vérifie réellement le moteur et affiche ses versions ; il n'exécute aucune commande métier sur l'hôte en cas d'échec. Une installation particulière peut demander `--runtime-program "/chemin/absolu/vers/docker"` ou Podman. **Ce choix est propre à chaque invocation** : répéter l'option sur `doctor`, `build`, `up`, `run` et `down`, ainsi que sur `worker-stop`, `worker-resume` et `export-workspace` lorsqu'un chemin explicite est nécessaire à cette installation. `init`, `worker-status`, `verify-audit` et `healthcheck` ne prennent pas cette option. Les sessions enregistrent l'invocation approuvée ; un changement du programme déjà enregistré peut exiger l'adoption décrite dans l'exploitation. Le broker ne choisit pas un exécutable depuis le `PATH` ambiant et ne reprend pas `DOCKER_HOST` ou `DOCKER_CONTEXT` ; voir [programme et cible du moteur](operations.md#programme-et-cible-du-moteur).

`build` produit `agentsandbox-control:local` et `agentsandbox-worker:local`. Le dashboard **Rust conteneurisé** sert les fichiers TypeScript compilés ; aucun serveur Node de production n'est requis. Gateway, dashboard et relais d'entrée sont séparés. Aucun ne reçoit le socket moteur ni le projet original.

Avant leurs commandes de build ou Compose, `build`, `up` et `down` remettent au moteur une copie privée et bornée de leurs entrées, créée sous le répertoire temporaire (`TMPDIR`), et affichent une ligne `Prepared public distribution (…)` portant son SHA-256. Le [guide local](setup/local/README.fr.md#prepared-source) décrit les exigences de ce répertoire temporaire, les refus, l'[interruption](setup/local/README.fr.md#interrupt-build-or-compose) et la [reprise d'une copie conservée](setup/local/README.fr.md#prepared-source-retained).

`init` crée les clés `runner` et `viewer` si elles manquent et **préserve les valeurs existantes**. Les répertoires privés sont `0700`, les clés originales `0600`. Des projections de montage séparées sont `0444` dans un parent `0700` : ne pas les déplacer dans un dossier public. Un emplacement existant non conforme est refusé sans réparation générale. Détails dans [Exploitation](operations.md).

`up` attend la disponibilité des deux accès et recrée les services de contrôle, même s'ils tournent déjà. Cette opération coupe les connexions d'audit en cours : attendre la fin des sessions avant de la répéter.

## 3. Ouvrir le dashboard

Ouvrir **[http://127.0.0.1:4000](http://127.0.0.1:4000)**. Dans un éditeur local privé, ouvrir le fichier `secrets/viewer` sous `$AIGUARD_CONFIG`, puis saisir sa valeur dans **Jeton de lecture**. Ne pas afficher cette valeur dans un terminal partagé, une capture, un journal ou un message.

Le jeton **viewer** consulte uniquement le client authentifié ; le serveur applique cette restriction. Le jeton **runner** reste au broker de confiance et est refusé par le dashboard. L'IHM permet la consultation et l'export du journal ; lancement, approbation et arrêt d'un worker passent par la CLI.

Une installation neuve contient son événement de création du registre ; aucune activité worker n'est attendue avant le premier lancement. Une erreur de connexion, une identité refusée ou des données périmées demandent une reprise ; elles ne signifient pas « aucune activité ».

## 4. Lancer une première commande hors ligne

```sh
"$AIGUARD_BIN" run \
  --config-dir "$AIGUARD_CONFIG" --runtime docker \
  --project "$AIGUARD_PROJECT" --profile strict --capture-output \
  -- /usr/bin/id
```

Avec Desktop non rootless, ajouter l'exception choisie à l'étape 2. `--` sépare les options ai-guard de l'exécutable et de ses arguments dans le worker. Cette invocation exacte figure dans la [politique initiale](../policies/default.json).

Le broker obtient une décision auditée, prépare une copie filtrée et lance un worker dans son volume. L'original n'est ni monté ni réécrit. Le résultat JSON fournit notamment `session_id`, le volume, `exit_code`, `timed_out`, `cleanup` et `changed_files` quand son scan aboutit. Un code de sortie seul ne prouve pas que le travail demandé a été réalisé. `--capture-output` conserve des sorties bornées et masquées ; cette capture est désactivée par défaut.

Dans **Actions**, retrouver la demande, sa décision, le démarrage et le résultat. **Workers** présente le registre durable et ses dernières observations ; **Sécurité** montre les décisions ; **Fichiers** montre les changements observés. Les événements fournisseur marqués **Déclaré** restent distincts des effets **Observés**. **Exporter JSONL** exporte le journal correspondant aux filtres, toutes pages incluses ; il n'exporte pas les fichiers du workspace.

## 5. Comprendre un refus et autoriser le bon périmètre

Cet exemple doit être refusé en `strict`, car ses arguments ne figurent pas dans l'allowlist exacte :

```sh
"$AIGUARD_BIN" run \
  --config-dir "$AIGUARD_CONFIG" --runtime docker \
  --project "$AIGUARD_PROJECT" --profile strict \
  -- /bin/echo exemple-non-autorise
```

Lire le motif et la règle dans le refus CLI et dans **Sécurité**. Aucun worker métier n'est lancé pour cette demande. Un refus d'audit ou un gateway inaccessible bloque aussi le lancement ; rétablir ce prérequis avant de réessayer.

| Profil | Ce qu'il autorise | Réseau |
|---|---|---|
| `strict` | Seulement les invocations exactes de la politique. | `none`. |
| `review` | Une invocation hors allowlist après approbation humaine de sa demande exacte ; les interdictions explicites restent bloquantes. | `none`. |
| `observe` | Un lancement audité, avec maintien des interdictions directes. | `none` ; `bridge` sur choix explicite. |

Pour une commande à examiner, choisir `--profile review`. Le terminal demande de recopier le **SHA-256 complet** affiché. Sans terminal, la demande privée reste en attente et aucun worker n'est lancé ; répéter les mêmes options et le même argv en ajoutant `--approve-hash HASH_COMPLET`. Il n'existe pas de `--yes` global. Une modification du projet, de l'image, du réseau ou de la commande invalide cette reprise.

Un script, un build ou un agent peut lancer des commandes imbriquées. L'approbation de l'invocation initiale et l'observation des logs ne constituent pas un contrôle individuel de ces actions.

## 6. Modifier la copie, examiner et exporter

Lancer une nouvelle session autorisant explicitement une création de fichier, toujours sans réseau :

```sh
"$AIGUARD_BIN" run \
  --config-dir "$AIGUARD_CONFIG" --runtime docker \
  --project "$AIGUARD_PROJECT" --profile observe \
  -- /usr/bin/touch /workspace/ai-guard-example.txt
```

Choisir un nom absent de votre projet pour voir une création. Chaque `run` repart d'une nouvelle copie de l'original ; cette commande ne reprend pas le volume du premier `id`. Examiner `changed_files` et les événements **Fichiers**, puis affecter l'UUID de **cette** session à la variable suivante :

```sh
AIGUARD_SESSION="UUID_DE_LA_SESSION"
AIGUARD_EXPORT="/chemin/absolu/vers/un-parent-existant/resultat-neuf"
"$AIGUARD_BIN" export-workspace \
  --config-dir "$AIGUARD_CONFIG" --session "$AIGUARD_SESSION" \
  --output "$AIGUARD_EXPORT"
```

Remplacer ces deux espaces réservés. Le parent de sortie doit exister ; `resultat-neuf` doit être **absent et extérieur au projet original**. Le parent et ses ancêtres doivent appartenir à root ou au compte de contrôle, sur un filesystem local pris en charge, sans écriture par d'autres utilisateurs ; les ACL sont vérifiées et l'exception des répertoires sticky appartenant à root reste contrôlée. En cas de refus, choisir un autre parent sûr : ai-guard ne modifie pas ses permissions. L'export exige une session terminale durable et réutilise son runtime enregistré ; il ne prend pas `--runtime`. Il ne remplace aucun fichier existant.

Vérifier que le nouveau fichier existe dans l'export et reste absent de l'original :

```sh
test -f "$AIGUARD_EXPORT/ai-guard-example.txt"
test ! -e "$AIGUARD_PROJECT/ai-guard-example.txt"
```

Chaque vérification doit rendre le code zéro. Ouvrir ensuite le résultat dans votre éditeur et examiner les changements avant toute intégration manuelle. ai-guard constate les différences de contenu par empreintes ; il ne fournit pas encore un diff Git détaillé. `.git`, les dépendances/caches courants et les noms sensibles connus sont exclus de l'import. L'export n'est donc pas une copie d'archive exhaustive de l'original.

Liens symboliques/physiques, fichiers spéciaux et chemins ambigus sont refusés. Les bornes sont **20 000 entrées**, **128 Mio** de contenu cumulé et **32 Mio** par fichier. Le scan d'archive et l'export refusent aussi plus de **20 000 répertoires**, parents implicites compris, ou un chemin de plus de **64 composants**. Un ancien volume plus profond peut donc être refusé à cette étape. Une dépendance créée dans le worker avec des liens peut empêcher le scan ou l'export complet : conserver le volume et suivre le diagnostic, sans interpréter l'échec comme zéro changement. La liste de noms exclus n'est pas un détecteur universel de secrets.

L'export attend les synchronisations des fichiers, de l'arborescence et du parent avant son succès. Une erreur après renommage peut laisser une destination déjà publiée : conserver cette destination et le volume, examiner le diagnostic et réconcilier le résultat ; ne pas supprimer ou remplacer les fichiers pour forcer une nouvelle tentative. Cette vérification logicielle ne constitue pas une qualification de panne électrique du stockage. [Preuves et limites](engineering/evidence/2026-10-31/export-publication/README.md).

## 7. Suivre, arrêter et reprendre

Avec l'UUID concerné, depuis le contrôle de confiance :

```sh
"$AIGUARD_BIN" worker-status --config-dir "$AIGUARD_CONFIG" --session "$AIGUARD_SESSION"
"$AIGUARD_BIN" worker-stop --config-dir "$AIGUARD_CONFIG" --session "$AIGUARD_SESSION"
"$AIGUARD_BIN" worker-resume --config-dir "$AIGUARD_CONFIG" --session "$AIGUARD_SESSION" --wait-seconds 30
"$AIGUARD_BIN" verify-audit --config-dir "$AIGUARD_CONFIG"
```

`worker-status` lit l'état durable, sans prétendre faire une nouvelle observation moteur. `worker-stop` persiste un intent ciblé ; sa réception ne prouve pas encore la fin. `worker-resume` réconcilie la même tentative après interruption : avant le premier démarrage il peut démarrer la commande déjà admise ; après consommation du droit de démarrage il ne la rejoue pas. Un état `in_doubt` conserve le quota et demande un diagnostic. Consulter l'état terminal durable et le message, même si une commande de consultation a rendu le code zéro.

Ctrl-C et le timeout demandent l'arrêt de la session concernée. **La surveillance de durée ne survit pas encore à la mort brutale du broker** ; la reprise manuelle n'est pas une supervision autonome. Si le moteur est indisponible, le nettoyage peut rester non confirmé. Conserver configuration, preuves et volume, rétablir le même moteur/gateway, puis suivre [arrêt et reprise](operations.md#consulter-arrêter-et-reprendre-un-worker). Une mise à jour du programme moteur peut exiger son adoption explicite ; ne pas modifier les checkpoints.

Pour arrêter les services de contrôle, après la fin des sessions :

```sh
"$AIGUARD_BIN" down --runtime docker --config-dir "$AIGUARD_CONFIG"
```

`down` conserve les volumes d'audit et de session et n'arrête pas les workers à votre place. Il ne prend pas `--allow-rootful-runtime`. Un prochain `up` avec la même configuration remonte l'audit conservé. Aucun `prune` ni effacement de volume n'est nécessaire à ce parcours.

Pour la sous-commande explicite `broker cancel`, encore en qualification distincte du parcours ci-dessus, conserver l'UUID de commande dès le premier appel : fournir `--command-id` avec un UUID neuf choisi, ou noter la ligne `Command: <UUID>` affichée avant l'envoi quand la CLI le génère. Passer ensuite cet UUID à `--command-id` pour un retry. Réutiliser le même identifiant retente cette annulation précise ; en choisir un nouveau exprime une nouvelle demande et peut inclure des exports plus récents. Un reçu durable ou une livraison au gardien ne prouve pas la fin du worker. Après `NeedsReconciliation`, consulter les preuves et l'état runtime avant toute conclusion sur le quota. [Portée et limites du rejeu](engineering/evidence/2026-10-31/cancel-retry/README.md).

Sur le service permanent configuré explicitement, `broker recovery-status` fournit un diagnostic même si le chargement métier est indisponible. Fournir les mêmes root, profil, fichier de token runner et UUID d'opération que pour les autres commandes broker :

```sh
"$AIGUARD_BIN" broker recovery-status \
  --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" \
  --token-file "$AIGUARD_RUNNER_TOKEN_FILE" --operation-id "$AIGUARD_OPERATION_ID"
```

Ces variables désignent votre configuration de broker existante ; cette commande ne l'initialise pas. La réponse distingue `journal_mode` (`full` ou `stop_only`), `source=confirmed_journal`, `runtime_evidence=not_queried` et `incomplete`. `full` décrit la capacité de l'acteur, pas la santé actuelle des credentials. `reconciliation_required` ne résume que ces métadonnées et le mode de l'acteur ; sa valeur `false` ne prouve ni la santé du gardien, ni la possibilité de lancer une action. Les armements et échéances sont ceux d'origine. Ni la présence d'un Cancel, ni un code zéro de consultation ne prouve un worker terminé ; conserver les preuves en cas de réconciliation requise. Un namespace absent est refusé sans création. Un ancien service refuse cette commande inconnue avec `InvalidInput`, sans diagnostic de version plus précis : mettre client et service à la version qui la fournit. [Validation et limites](engineering/evidence/2026-10-31/stop-only-actor/README.md).

Pour une exécution batch confiée au service broker permanent, les vues **Journal** et **Actions** présentent `broker.batch.failure` sous « Échec signalé ». Le résumé indique la phase et l'étape reconnues ; une valeur absente reste inconnue et l'état final de l'exécution reste à confirmer. Ce signalement n'est pas une décision de sécurité, une preuve d'arrêt ou une libération de quota. Si sa publication échoue, conserver aussi le journal local et utiliser le diagnostic de reprise ci-dessus ; l'absence d'événement public ne prouve pas une absence d'erreur.

## Configuration et données à conserver

| Réglage ou donnée | Valeur / emplacement livré |
|---|---|
| Configuration CLI | `.agentsandbox` par défaut ; ce guide passe toujours `--config-dir "$AIGUARD_CONFIG"`. `doctor` et `build` n'utilisent pas cette option. |
| Identités, fournisseurs et reprise | `secrets/`, `providers.json`, `sessions/`, `pending/`, `credential-bindings/` et snapshots privés sous la configuration ; ne pas les publier ni les mettre dans Git. |
| Audit et registre | SQLite dans le volume Compose `audit_data`, monté en `/data` du gateway ; changer `--config-dir` ne crée pas une autre stack ni un autre volume d'audit. |
| Fichiers de travail | Un volume propre à chaque session, dont le nom est enregistré dans le résultat/registre ; l'original reste sur l'hôte. |
| Gateway CLI | `http://127.0.0.1:8787` ; les commandes de session acceptent `--gateway` pour une autre origine autorisée. Conserver l'origine de la session. |
| Image worker | `agentsandbox-worker:local` ; `run --image` sélectionne une image de confiance préparée. Les dépendances de votre projet ne sont pas préinstallées. |
| Ressources worker | UID/GID `1000:1000`, 2 CPU, 1 Gio RAM, 256 PID, racine readonly, capacités supprimées et `no-new-privileges`. Les limites ne sont pas des mesures de consommation. |
| Durée | `run --timeout 300` par défaut, borne CLI 1–86 400 s ; l'admission locale limite par défaut à 3 600 s. |
| Quota local total | 3 workers, 6 000 milliCPU, 3 Gio. Les options du gateway et le registre multi-client sont décrits dans [CLOUD_TENANCY.md](engineering/CLOUD_TENANCY.md). |
| Politique | [`policies/default.json`](../policies/default.json), copiée dans l'image de contrôle ; un changement exige reconstruction et redémarrage planifié du contrôle. |

Le réseau est `none` par défaut. `--network bridge` exige `--profile observe` et ouvre un accès sortant **sans filtrage applicatif**. L'usage relevé est une allocation CPU/RAM avec `billable=false`, pas une facture ni du CPU réellement consommé. Le mode cloud, le relais métier worker et Kubernetes ne sont pas fournis par ces Compose mono-client.

Avant une mise à jour, une migration ou une sauvegarde, consulter les [procédures d'exploitation](operations.md) et les [contraintes SQLite](engineering/CLOUD_TENANCY.md). Ne pas copier une base active en ignorant son WAL/SHM. La restauration v2 de claims déjà consommés, la rétention et la reprise après panne restent des qualifications ouvertes ; une ancienne sauvegarde n'autorise pas à rejouer un démarrage.

## Ajouter un agent fournisseur

Le checkout propose une sélection gérée de **Codex CLI ou Claude Code**, avec **API ou abonnement**, sans argv fournisseur à construire. Le choix est annoncé avant la décision et lié à l'autorisation. L'image contenant le bootstrap Rust a passé build, versions/aides et refus sans credential sur Docker Desktop/macOS ARM64 ; ces sondes ne qualifient aucune authentification. Un essai ultérieur **Claude/API batch géré** a une preuve ciblée, décrite ci-dessous ; **Codex géré, abonnements et TUI restent à qualifier**. L'ancienne image QA ne remplace pas cette image ; voir [les preuves et limites](engineering/PROVIDER_AUTH.md#état-livré-et-limites--30-octobre-2026-après-revue).

Préparer l'image depuis ce checkout, avec les versions exactes utilisées comme références datées le 30 octobre 2026 :

```sh
"$AIGUARD_BIN" build --runtime docker --codex-version 0.165.0 --claude-code-version 2.1.140
```

Ajouter l'exception Desktop si elle a été choisie. Le worker générique ne contient aucun CLI fournisseur sans ces options. Ni `latest`, ni joker, ni plage ne sont acceptés. Ces versions ne sont pas une recommandation de dernière version ; leur présence ne prouve pas l'authentification.

Préparer dans un éditeur privé un fichier littéral `NAME=value`, régulier, possédé par le compte de contrôle et en mode `0600`. Pour les profils créés ci-dessous, les noms sont `OPENAI_API_KEY` et `ANTHROPIC_API_KEY` ; renseigner seulement les credentials disponibles. Ne mettre aucune valeur dans une commande, un prompt, Git ou une capture. Le fichier n'est pas un script : ni `export`, ni interpolation, ni `source`. Le `.env` racine explicitement choisi est admis et exclu de la copie worker ; aucun fichier n'est recherché automatiquement.

Créer les références privées, puis consulter les choix configurés :

```sh
"$AIGUARD_BIN" providers init --config-dir "$AIGUARD_CONFIG" \
  --env-file "$AIGUARD_PROJECT/.env"
"$AIGUARD_BIN" providers status --config-dir "$AIGUARD_CONFIG" --json
```

`init` ne lit pas le contenu du fichier de credentials et n'écrase pas un `providers.json` existant. Il crée les profils `codex-api` et `claude-api`, avec une priorité automatique explicite. `status` affiche la configuration sans chemin privé ni secret ; `credentials_verified=false` et `worker_capabilities_verified=false` signifient que ces prérequis n'ont pas été vérifiés. Voir le [contrat fournisseur](engineering/PROVIDER_AUTH.md#état-livré-et-limites--30-octobre-2026-après-revue) pour les profils d'abonnement, les références relatives et la reprise.

Choisir un projet où `result.txt` est absent. Voici une recette gérée Codex/API à qualifier sur votre installation, avec une durée bornée et un effet vérifiable :

```sh
"$AIGUARD_BIN" run \
  --config-dir "$AIGUARD_CONFIG" --runtime docker \
  --project "$AIGUARD_PROJECT" --profile observe --network bridge \
  --client codex --auth-mode api --client-profile codex-api \
  --model gpt-6-luna --timeout 120 \
  --prompt "Create only a new file named result.txt at the workspace root. Its full content must be exactly: Codex local smoke test completed. Do not read or modify other files, run tests, or start subagents."
```

Pour choisir Claude/API explicitement, employer `--client claude-code --auth-mode api --client-profile claude-api --model claude-haiku-5-5`, avec un prompt utile et une durée bornée. Les prochains essais Claude doivent conserver cet identifiant exact ; aucun alias ni basculement de modèle n'est prévu. Le mode API est annoncé comme facturé par le fournisseur. `--client auto --auth-mode auto` utilise uniquement les profils configurés ; un abonnement sélectionné indisponible ne bascule jamais vers une API.

`--client-profile` sélectionne le profil fournisseur ; `--profile observe` reste le profil de politique. Les credentials exigent toujours `--profile observe --network bridge`, sans activation implicite. `--providers-file` permet de choisir un autre fichier privé ; son défaut est `$AIGUARD_CONFIG/providers.json`. Les options gérées ne se combinent pas avec l'ancien argv manuel ou `--secret-env-file`.

Après l'état terminal durable, exporter **cette session Codex** vers une destination neuve et vérifier son résultat, dont le texte attendu n'a pas de saut de ligne final :

```sh
AIGUARD_SESSION="UUID_DE_CETTE_SESSION_CODEX"
AIGUARD_EXPORT="/chemin/absolu/vers/un-parent-existant/resultat-codex-neuf"
"$AIGUARD_BIN" export-workspace \
  --config-dir "$AIGUARD_CONFIG" --session "$AIGUARD_SESSION" \
  --output "$AIGUARD_EXPORT"
printf '%s' 'Codex local smoke test completed.' | cmp - "$AIGUARD_EXPORT/result.txt"
test ! -e "$AIGUARD_PROJECT/result.txt"
```

Les deux dernières commandes doivent rendre le code zéro. Elles vérifient le contenu exporté et l'absence de ce nouveau fichier dans l'original ; les autres conditions et limites d'export restent celles de l'étape 6. Un résultat différent reste à examiner, même si Codex a rendu le code zéro.

**La clé injectée est lisible par l'agent et tout code du worker**, ainsi que par les autorités du moteur. Le masquage des traces n'empêche pas cet accès. `bridge` permet aussi l'exfiltration des données confiées au worker. Les événements Codex/Claude sont normalisés après la fin, avec provenance `declared` ; ils ne sont pas une médiation avant action. Le champ `coverage=observed` décrit ici la capture de cette déclaration, pas une observation indépendante de l'action (voir [audit et couverture](operations.md#audit-sorties-et-couverture), y compris les limites Web/MCP).

Le mode `--interactive` existe pour une session autorisée avec un vrai terminal ; il est incompatible avec `--capture-output`. Pour un abonnement, préparer volontairement le credential officiel dans un contexte isolé puis référencer le profil correspondant ; le produit n'extrait aucun cache OAuth ni keychain hôte et ne livre pas encore d'assistant `auth login/import/status`. Le [guide d'exploitation](operations.md#agents-cloud-et-clés-dapi) décrit arrêt et reprise, dont le refus de réattachement après un claim consommé.

Les preuves historiques restent distinctes : Codex **0.165.0** a un PASS manuel batch avec fichier/export vérifiés ; Claude Code **2.1.140** a un PASS manuel API batch sur l'ancien binaire et l'ancienne image QA. Cet essai Claude demandait l'alias `haiku`, sans preuve du modèle complet résolu : il ne qualifie pas Haiku5.5. La tentative Claude TUI est **FAIL de qualification par instrumentation**, sans réponse du premier tour établie. Aucun de ces résultats ne qualifie les nouveaux parcours gérés ; voir la [validation datée](engineering/VALIDATION.md).

Un essai ultérieur du **parcours géré Claude/API batch** a créé/exporté un fichier exact, avec choix explicite `claude-haiku-5-5`, audit valide après restart et quota libéré. [Image, commande et limites](engineering/evidence/2026-10-30/provider-auth/managed-claude/real-run.md). Le modèle effectivement servi reste inconnu ; le délai worker ne constitue pas un budget monétaire. Codex géré, abonnements et TUI demandent leurs propres preuves.

## Commandes à retrouver et dépannage

| Besoin | Commande / action |
|---|---|
| Voir la syntaxe exacte installée | `"$AIGUARD_BIN" --help`, puis `"$AIGUARD_BIN" run --help` ou le nom de la sous-commande. |
| Vérifier le moteur sans lancer de worker | `"$AIGUARD_BIN" doctor --runtime docker` ; choisir l'exception rootful seulement si nécessaire. |
| Voir les profils fournisseur configurés | `providers status --config-dir "$AIGUARD_CONFIG" --json` ; ce statut ne sonde ni credential ni image. |
| Vérifier les accès locaux sans secret | `"$AIGUARD_BIN" healthcheck --url http://127.0.0.1:8787/healthz --url http://127.0.0.1:4000/healthz` ; une santé HTTP ne prouve pas l'autorisation. |
| Lire un résultat ou une demande d'arrêt | `worker-status` avec le même `--config-dir`, `--gateway` et `--session` ; lire aussi les inconnues et `cleanup`. |
| Vérifier l'audit | `verify-audit --config-dir "$AIGUARD_CONFIG"` ; attendre `valid=true`, sinon conserver les données et diagnostiquer. |
| Exporter les fichiers / le journal | `export-workspace` vers une cible neuve / **Exporter JSONL** dans l'IHM. |

| Symptôme | Reprise utile |
|---|---|
| Moteur absent, programme refusé ou rootless non détecté | Lire `doctor`, démarrer le moteur choisi, vérifier son chemin approuvé et ses permissions. Pour un emplacement personnalisé, répéter `--runtime-program` sur chaque commande qui le prend ; aucun choix global ni repli hôte n'est prévu. |
| `up` échoue ou ports occupés | Vérifier les ports 4000/8787 et les logs Compose de cette stack. `up` la crée sous le projet `agentsandbox` à partir d'une copie temporaire déjà supprimée : s'y adresser par ce nom, sur le même moteur et avec `AGENTSANDBOX_CONFIG_DIR`, comme dans [suivre une pile démarrée par `up` du CLI](setup/local/README.fr.md#cli-up-stack). Conserver volumes et données. |
| Jeton refusé / écran déconnecté | Utiliser `secrets/viewer` de la même installation. Vérifier le collecteur et reconnecter après une rotation ; ne pas substituer `runner`. |
| `migration_required`, permissions ou ACL refusées | Suivre la maintenance hors ligne documentée ; ne pas supprimer la base, les verrous ou les checkpoints pour contourner le refus. |
| Audit plein ou indisponible | Les nouvelles écritures bloquent les lancements. Conserver l'audit, diagnostiquer la capacité et appliquer une maintenance autorisée. |
| `in_doubt`, timeout ou nettoyage non confirmé | Rétablir le moteur et l'audit de la session, puis consulter et réconcilier la même tentative ; ne pas relancer aveuglément une nouvelle commande. |
| Export refusé | Vérifier état terminal, cible absente, parent existant, liens et bornes ; le volume conservé reste la source à diagnostiquer. |

Pour aller plus loin : [Exploitation](operations.md), [API](api.md), [Interface](ui.md), [Sécurité](security.md), [contribution et tests](engineering/TEST_STRATEGY.md), [qualification](engineering/PRODUCTION_READINESS.md). La [licence Apache-2.0](../LICENSE) est livrée ; le canal privé de signalement de vulnérabilités reste à décider avant publication.
