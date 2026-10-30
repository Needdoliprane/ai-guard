# Critères d'acceptation du produit

Cette matrice définit les attentes du produit complet ; elle n'annonce aucun test réussi. Les résultats du checkout sont dans [engineering/VALIDATION.md](engineering/VALIDATION.md), les écarts dans [engineering/CURRENT_STATE.md](engineering/CURRENT_STATE.md), et les conditions de release dans les [gates G01–G12](engineering/PRODUCTION_READINESS.md). [validation.md](validation.md) conserve les résultats historiques dans leur environnement d'origine.

Le mandat [AGENTS.md](../AGENTS.md) et le [contrat d'architecture](engineering/ARCHITECTURE_CONTRACT.md) priment sur l'ancien découpage MVP/V2. Les lots organisent l'implémentation sans reporter sûreté, qualité d'usage, exploitation ou maintenance. Une fonction absente reste un écart ouvert ; une documentation ou un mock permanent ne la rend pas livrée.

## Preuve et statuts

Chaque preuve conserve date, révision ou empreinte du checkout, commande exacte, versions, OS/architecture, runtime et mode rootless/rootful, images/digests, prérequis, résultat attendu et constaté, limites et artefacts masqués. Aucun secret ne figure dans les rapports. Un `PASS` ne porte que sur ce périmètre. `FAIL` décrit un écart observé ; `BLOCKED` nomme le prérequis manquant ; `NOT_RUN` signifie non exécuté ; `NOT_APPLICABLE` doit être justifié avant essais. Un test ignoré reste `NOT_RUN`.

## Vérifications logicielles reproductibles

À la racine, avec la toolchain de `rust-toolchain.toml` :

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
CARGO_INCREMENTAL=0 cargo test --locked
cargo build --release --locked
```

Dans `front/`, avec Node 24 :

```sh
npm ci
npm test
npm run build
npm run test:browser
```

Certains tests HTTP ouvrent un port loopback éphémère. Le navigateur exige Chromium/Playwright selon [ui.md](ui.md). Les tests logiciels et fixtures ne qualifient ni les effets d'un moteur ni une session fournisseur authentifiée. Consigner les versions réellement utilisées et tout test non exécuté, sans les compter comme des succès.

## Les 18 scénarios d'origine

| # | Scénario | Prérequis et procédure | Acceptation et limites de preuve |
|---|---|---|---|
| 1 | Sandbox Codex | Image avec version exacte, compte et credentials privés ; parcours `run --provider codex` du [README](../README.md#agents-cloud-et-identifiants-explicites) | Session et lancement observés ; déclarations normalisées. Une fixture ne remplace pas le compte/API réel. |
| 2 | Sandbox Claude | Version exacte et parcours `run --provider claude`, sortie structurée configurée | Même séparation des provenances ; format et permissions vérifiés sur la version utilisée. |
| 3 | Tâche simple avec chacun | Modification limitée d'un fichier dans une copie jetable, puis examen et export | Original inchangé ; changement visible dans la session et destination d'export neuve ; preuve distincte par fournisseur. |
| 4 | Création de sous-agents | Fonction réellement exposée par le fournisseur ou contrat générique explicite | IDs et parents explicites ; provenance par champ et couverture réelles. Aucune parenté inventée à partir d'un texte ou ID d'outil. |
| 5 | Arborescence | Racine/enfants, enfant avant parent, parent absent, cycle, plusieurs sessions ; parcours navigateur | Bon contexte d'identité, orphelins/cycles signalés ; processus observés et statuts déclarés distincts. |
| 6 | Commande autorisée | Image et gateway actifs ; `run --profile strict -- /usr/bin/id` | Demande, autorisation, soumission, démarrage inspecté et résultat distincts ; UID non privilégié observé. |
| 7 | Commande interdite | `run --profile strict -- rm -rf /workspace` dans l'environnement de recette | Refus durable ; aucun worker d'exécution créé. Le message de refus seul ne prouve pas le non-effet. |
| 8 | Décision Jev | Endpoint local contrôlé pour le contrat, puis compte/modèle réels explicitement autorisés | Décision, contexte, règle et motif visibles ; échec/timeout/réponse invalide refusent. Le serveur de test ne qualifie pas le modèle distant. |
| 9 | Aucun effet après refus | Commande à marqueur refusée par politique ou Jev, puis inspection du moteur et volume | Marqueur absent, aucun lancement effectif ; panne d'audit également couverte avant effet. |
| 10 | Audit par étape | Session complète, lecture corrélée et `verify-audit` | Traces durables et distinctes, séquences/hashes valides, réception séparée de l'horloge source. |
| 11 | Fichiers et Git | Créer/modifier/supprimer dans la copie, comparaison avant/après et `export-workspace` | Liste et empreintes exactes, export neuf, original intact ; compléter les diffs Git demandés sans monter l'original ni inventer d'attribution individuelle. |
| 12 | Arrêt brutal / timeout | Job long, timeout court, Ctrl+C, puis panne broker/moteur dans des essais séparés | Arrêt ciblé ou état inconnu explicitement réconciliable ; audit/volume conservés ; procédure de reprise vérifiée. |
| 13 | Redémarrage dashboard | Arrêt/redémarrage du seul dashboard pendant une session | Politique et collecte maintenues ; reconnexion par curseur, sans doublon ni perte silencieuse de portée. |
| 14 | Persistance | Redémarrer le gateway en conservant son volume, puis sauvegarde/restauration selon procédure | Événements, identités, séquences et hashes conservés ; version de schéma et échecs de restauration traités. |
| 15 | Secrets | Tokens de test, PEM, Bearer, arguments et JSON imbriqué ; DB, flux, export et navigateur | Valeurs reconnues/connues masquées avant persistance/diffusion ; aucun token de session stocké dans l'URL ou le stockage navigateur. Limite des secrets arbitraires inconnus explicite. |
| 16 | Charge multi-agents | Audit synthétique puis vrais workers et lecteurs lents ; charge 100 distincte | Débit, mémoire, latence, pagination, saturation et reprise mesurés ; producteurs de fixtures non assimilés à des agents actifs. |
| 17 | Docker / Podman | Smoke réel explicite, puis scénarios négatifs, pour chaque moteur et plateforme | UID, réseau, mounts, capacités, limites, copie/export et non-effet vérifiés ; quatre rapports Docker/Podman × Linux x86_64/macOS ARM64. |
| 18 | Front/API | Faux credentials, viewer en écriture, identité A/B, XSS, requête cross-site, export et clavier | Refus côté serveur, rendu texte, isolation des données, états d'erreur utilisables et accessibilité vérifiée ; exposition distante qualifiée séparément. |

Le smoke existant `runtime_smoke` est ignoré par défaut. Le lancer explicitement après préparation du moteur, des images et du gateway selon [operations.md](operations.md) et [engineering/TEST_STRATEGY.md](engineering/TEST_STRATEGY.md). Il ne couvre pas à lui seul l'ensemble de la matrice.

## Acceptation transversale du produit

| Critère | Parcours et preuve attendus |
|---|---|
| P01 — Installation et premier usage | Depuis une installation neuve sur chaque cible : prérequis diagnostiqués, erreurs actionnables, stack démarrée, connexion, session autorisée, refus expliqué et export neuf. Fonctionnement local sans compte cloud obligatoire ; préparation réseau initiale explicite. |
| P02 — Interface aboutie | Chargement, vide, partiel, périmé, déconnexion, 401/403, erreur de lecture et reprise sont distincts. Afficher portée/fraîcheur des données et motifs de décision. Les filtres, pagination et export correspondent au même périmètre. Aucune donnée de démonstration ni contrôle sans effet. |
| P03 — Accessibilité et lisibilité | Vérifier clavier seul, focus, labels, annonces d'état, contrastes, clair/sombre, taille réduite, zoom et tableaux longs. Consigner l'étendue réelle de la revue et les défauts ; un test DOM ou une capture ne vaut pas audit complet. |
| P04 — Isolation multi-client réelle | Recette A1/A2 chez A et B1 chez B : identité authentifiée, données/volumes séparés, quotas, démarrage/arrêt, grant A1→A2, refus A1→B1, révocation, refus d'accès worker aux secrets/registre/métriques du contrôle. |
| P05 — Durabilité et usage | Audit indisponible avant effet, quota concurrent, replay identique, conflit divergent, arrêt brutal et redémarrage. Transitions, libération et mesure terminale atomiques ; observation absente inconnue et réconciliable ; allocation CPU/RAM `billable=false`, sans prix ni facture. |
| P06 — Exploitation et évolution | Installation/mise à jour documentées, compatibilité API/schéma, migrations admises, sauvegarde et restauration testées, rétention/rotation configurables, saturation, diagnostic et récupération sans supprimer preuves/volumes existants. |
| P07 — Fournisseurs et contrôle | Version et capacités réelles par connecteur ; comptes autorisés ; médiation avant action démontrée pour toute promesse de blocage transitif. Lacunes Native/Observed/Generic visibles, sous-agents seulement si réellement exposés. |
| P08 — Cloud et capacité | Adaptateur Kubernetes effectif sur cluster autorisé, workload identity, chiffrement, politiques réseau appliquées, stockage/reprise et autorité du compteur ; charge 100 workers réels par client distincte de la recette locale. Aucun attribut de configuration ne remplace ces preuves. |
| P09 — Publication et maintenance OSS | Licence Apache-2.0 présente, canal privé de vulnérabilités décidé, contribution et support expliqués, dépendances/artefacts vérifiés, compatibilité et limites publiées, gates applicables fermés. Aucun compte, contact ou engagement de service inventé. |

Les extensions réellement facultatives, par exemple OTLP/Prometheus ou une administration UI, demandent une justification liée au besoin et aux gates. Une exigence du produit ou du périmètre de release ne devient pas facultative en étant déplacée dans une liste « V2 ». Un changement structurel, notamment microVM ou stockage, reste soumis à décision avant réalisation.

## Échecs à provoquer et revue

Provoquer notamment audit plein/arrêté, décision non durable, Jev invalide/timeout/redirect, hash d'approbation altéré/rejoué, archive avec traversal/liens/fichier spécial/doublon/dépassement, substitution concurrente de chemin, credentials ambigus, accès inter-client, révocation concurrente et chaîne altérée. Vérifier le refus et le non-effet approprié ; conserver une régression pertinente. Les changements de sécurité, lancement, audit et import/export exigent une revue indépendante dont les défauts bloquants sont traités.

Une release n'est acceptée que lorsque tous ses gates applicables ont une preuve acceptable et leurs écarts bloquants sont fermés. Une qualification locale restreinte ne retire ni le cloud, ni Kubernetes, ni la capacité 100 du mandat produit. Les dépendances externes manquantes restent `BLOCKED` ou `NOT_RUN`, avec le manque précis, pendant que les travaux indépendants continuent.
