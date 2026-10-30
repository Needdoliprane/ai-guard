# Revue indépendante ciblée — runtime géré

2026-10-29. Inspection READONLY de `src/runtime.rs`, `src/runtime/managed.rs`, `tests/runtime_managed.rs` et de la fixture Rust `tests/runtime_fixtures/managed.rs`, avec le rapport auteur et les logs fournis. Aucun fichier du dépôt modifié, aucune dépendance installée, aucun code cible, moteur, navigateur, service ou endpoint exécuté par ce reviewer. Les outils utilisés ont seulement lu les fichiers, calculé leurs empreintes et consulté une source primaire publique. Pas de full audit ni de qualification production.

## Conclusion du périmètre relu

Aucun P0/P1 nouveau établi dans le snapshot final. La piste de reprise `lookup_created` sans recoupement des labels de tentative a été reproduite par l'auteur puis corrigée pendant la revue : **PASS source + preuves fournies pour ce défaut précis**. Ce verdict ne constitue pas une reproduction indépendante ni une approbation globale du runtime, du runner en développement ou du produit.

Empreintes calculées en lecture seule et confirmées gelées par root après l'exception de correction des labels :

```text
ae5e0cba0533b166059c10232351294f6d8efb6ad6df0a46184a547cdb8c1a3d  src/runtime.rs
e5ea48c5d231a904e234adc8847a422e216dab8cb59c7e1a58ecc84cf1bcef48  src/runtime/managed.rs
6ec0625cfb34f753c1a05ee1b7e2b988550f9a64a698ee7c379cf39ff2b90420  tests/runtime_managed.rs
cfdd5a6cbdab772386e699694e5af98fa6110ecf22da003de25f7d1be298318a  tests/runtime_fixtures/managed.rs
```

Les empreintes intermédiaires ont changé pendant la revue ; elles ne sont pas utilisées comme état final. La baseline `/tmp/ai-guard-runtime-retention-baseline-20261029.rs` est plus ancienne que la fondation retention et ne constitue pas à elle seule le diff de ce lot. Le rapport auteur reçu inclut déjà le fix OOM mais précède le correctif final des labels ; les logs et empreintes ci-dessus complètent sa portée.

## Défaut de reprise traité pendant la revue

Avant correction, `lookup_created` (`managed.rs:570–629`) sélectionnait un ID complet dans un inventaire par nom puis vérifiait nom, image, commande, montages et limites. Il construisait ensuite le handle avec la session et l'opération du spec sans lire les labels correspondants de l'instance. La fixture pouvait omettre Labels et la reprise réussissait. Le risque concret était l'association à une tentative d'un conteneur de même nom/configuration créé autrement. La précondition est un artefact en conflit ou un acteur ayant accès au moteur de contrôle ; aucun accès moteur depuis un worker ni contournement tenant distant n'a été démontré. Aucune sévérité de vulnérabilité worker/remote n'est attribuée.

Le code final exige maintenant la map Config.Labels et recoupe exactement `agentsandbox.session`, `agentsandbox.operation` et `agentsandbox.purpose` contre le spec dans `validate_configuration` (`managed.rs:944–950,1002–1022`). Cette validation est utilisée avant on_created et par lookup_created ; ces labels participent aussi à l'empreinte de configuration. La régression ajoute la variante de labels étrangers à l'inventaire de reprise (`tests/runtime_managed.rs:247–263` ; fixture `:170–177`) et conserve l'absence de start/kill/rm dans le chemin de consultation.

Preuves lues :

- **FAIL avant correctif**, `/tmp/ai-guard-runtime-label-red.log` : le cas observe_only échoue sur « a matching name and spec with foreign attempt labels must not be adopted » ; c'est un échec comportemental, pas un échec de compilation.
- **FAIL avant correctif**, `/tmp/ai-guard-runtime-label-red-missing.log` : la même assertion échoue. La variante d'entrée « labels manquants » est identifiée par l'auteur/root ; le texte du log seul ne distingue pas cette entrée de celle aux labels étrangers. Le code final refuse néanmoins l'absence de la map ou de chacun des trois labels requis.
- **PASS fourni**, `/tmp/ai-guard-runtime-label-green.log` : 6 managed, 3 races, 1 retention, 1 terminal, soit 11 réussites, zéro échec et zéro ignoré. Les warnings présents concernent la fondation runner en cours ; ce log ne prouve pas un clippy global vert.

## Contrôles constatés dans les sources

- **Autorité et invocation** : binaire résolu vers un chemin canonique, programme/arguments séparés, endpoint explicite ; les sélections Docker/Podman ambiantes listées sont retirées. Docker recoupe un engine ID ; Podman exige un bootstrap explicite séparé, puis graphRoot/runRoot, compte et marqueur privé recoupés (`managed.rs:141–243,357–398,411–519`). `connect_podman` ne crée rien. Debug masque le marqueur et les erreurs CLI managed remplacent les diagnostics bruts (`:25–33,400–409`). Les labels du marqueur ne sont pas des credentials de worker.
- **Endpoints** : Unix Docker, Podman natif local et Podman remote par socket Unix sont implémentés ; SSH/TCP sont refusés par validate_endpoint (`:898–917`). Root a explicitement confirmé que le socket Unix Podman est autorisé pour une VM locale ; ce n'est pas un défaut à refuser pour réduire l'ancien périmètre de preuve. La destination réelle et les autorisations d'exploitation d'une passerelle opérateur restent externes au code. Aucune qualification du transport ou d'un moteur Podman réel n'est donnée.
- **Faits incomplets** : snapshots avec Option pour running, dates, exit code, OOM, restart, identité et limites. Valeurs manquantes/sentinelles/incohérentes gardent une inconnue ou un diagnostic ; ratios CPU réduits en entiers. Env et State.Error ne font pas partie du snapshot (`runtime.rs:817–859,923–1156`). Le callback managed relit une inspection complète, ce qui préserve OOM absent au lieu d'utiliser le booléen du shim legacy (`managed.rs:745–751`).
- **Avant démarrage** : create doit rendre un ID hex complet ; la première inspection recoupe ID, image immuable autorisée, nom, allocation bornée, état created/configured/initialized non running et sans début observé. La configuration privée recoupe maintenant les labels, l'UID, argv/entrypoint/workdir, réseau, read-only, non privilégié, capabilities, NNP, restart, auto-remove, tmpfs et volume de travail (`managed.rs:701–732,835–886,1002–1104`). Le callback FnOnce doit produire EmitOnce ; erreur ou ObserveOnly n'émettent pas start. Le démarrage cible l'ID acquis, jamais le nom (`runtime.rs:693–709`).
- **Volumes** : création avec labels de contrôle, réponse exacte, puis Name/Driver=local/Scope=local/options absentes ou vides/labels attendus ; conflit de propriétaire ou bind hôte refusé sans rm (`managed.rs:304–355`). Les noms sont validés sans chemins ou séparateurs de mount (`runtime.rs:85–89,195–207,1159–1176`).
- **Après démarrage** : identité/image/limites/restart=0 et empreinte Created recoupés avant les callbacks et le terminal normal ; terminal reconnu par moteur, non running, exit code connu et dates ordonnées dans le domaine runtime (`runtime.rs:861–919`, `managed.rs:631–638,748–755`). La durée attach/contrôle du résultat n'est pas une preuve d'allocation.
- **Arrêt et nettoyage** : create échoué/réponse ambiguë ne mène à aucune adoption, inspect, kill ou rm. Après acquisition, arrêt ciblé au seul ID et conservation ; observation indisponible reste stop unknown. Cleanup recoupe le terminal acquitté puis `rm ID` sans force ; aucun Drop ne détruit les preuves (`managed.rs:650–668,762–807`). Le runtime conserve le résultat en cas de cleanup échoué.

La structure native publique Podman v5.7.1 a été consultée pour les champs Config, State et Mounts ; cela étaye la lecture des formats, sans valoir preuve d'un moteur installé. [Source Podman v5.7.1](https://raw.githubusercontent.com/containers/podman/v5.7.1/libpod/define/container_inspect.go)

## Contrats d'intégration encore à prouver

**NEEDS_VALIDATION dans le caller root, pas défaut runtime confirmé dans cette passe** : `resume_terminal` (`managed.rs:779–782`) valide les faits intrinsèques contre eux-mêmes. `validate_identity_allocation` ignore l'empreinte si celle de la baseline vaut None (`runtime.rs:863–867`). La reprise doit donc exiger un Created durable avec empreinte présente puis comparer le terminal à ce Created avant finalisation. Une auto-comparaison du terminal ne prouve pas le confinement initial. Root a reçu et confirmé ce contrat comme prioritaire ; le runner est hors de cette revue ciblée et reste en développement.

Le caller doit aussi lier on_created à Created+StartClaim durables, ne renvoyer EmitOnce que sur réponse fraîche, conserver le verrou OS de tentative, acquitter terminal+audit avant cleanup et préserver l'incertitude/quota si l'autorité ou l'observation manque. Le shim ExecutionResult.state ne remplace pas le snapshot Option pour OOM ou la mesure. Les labels recoupent l'identité d'un artefact ; ils ne permettent jamais de choisir le tenant.

## Résultats et limites

- **PASS source + preuves fournies** : correctif précis des labels et les 11 tests visibles dans le log final. Aucun test n'a été exécuté par le reviewer.
- **PASS rapporté seulement** : rustfmt et clippy ciblé historiques du rapport auteur ; aucun log brut de ces commandes n'a été fourni ici pour le snapshot final. Aucun clippy global PASS revendiqué.
- **FAIL historique fourni** : `/tmp/ai-guard-worker-runtime-suite.log` s'arrête sur api_security count=2 au lieu de 1 après genesis. Ce résultat est hors du correctif runtime et ne devient ni défaut runtime ni PASS global.
- **NOT_RUN** : moteur managed réel Docker/Podman, effets cgroups/userns/copy-up/réseau, Linux/macOS et architectures, reprise réelle, transport VM Podman, release, cloud/Kubernetes/NAS. Les 18 cas CLI sont des fixtures Rust, pas 18 exécutions moteur.
- **NOT_RUN par cette revue** : toute exécution cible, faute de profil OS complet établi pour la guidance security-audit ; ni permission utilisateur ni bypass n'ont été demandés.
- **BLOCKED pour une qualification production** : intégration runner/Store/API, preuves de reprise/fencing et essais moteurs applicables restent nécessaires. Ce rapport ne ferme aucun gate global.

Instructions de contexte déjà lues : AGENTS.md, RTK.md et contrats engineering imposés. Skill security-audit en guidance ciblée ; skill projet ai-guard-security-review absent, chargement automatique non confirmé.
