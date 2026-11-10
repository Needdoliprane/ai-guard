# SetupV2 — préparation hors ligne et reprise liées au bootstrap

Le manifeste fermé version2 exige le plan de capacité AuthV4 et son ensemble
exact de tenants. Le financement réel genesis+plan est validé avant tout
stage, secret ou base. Une préparation neuve produit l’audit V4 et la
configuration2 ; elle conserve le binding de création inode/propriétaire,
le namespace prévu, le témoin SQLite et le vrai reçu du plan à la séquence2.
Une reprise sans origine ne peut adopter que son bootstrap possédé exact de
deux entrées. Substitution, activité supplémentaire sans origine ou faux reçu
refusent sans reset. Manifest1, genesis1/origine1, credentials et reprise V1
conservent leur sens. Aucune donnée utilisateur n’a été migrée par ce lot.

## Résultats et sources distinctes

| Exécution réelle isolée du 10 novembre 2026 | Résultat | Manifeste complet |
|---|---|---|
| R2 : 11 fonctions SetupV2 et 4 canaris V1, bibliothèque | 15 sélections exactes, chacune 1 PASS/RC0 | 2370 fichiers, `21fe6c0af46ccc4ae109b4bd48fe8507796014fc7e145eaa07f80631b7903097` |
| R3 : nouvelle préparation V2 et 4 canaris CLI | 5 sélections exactes, chacune 1 PASS/RC0 | 2370 fichiers, `c7d00086479ae1002c41675110bdb38ddf4d8006df46641816d1843b011165bf` |
| R3 : `cargo check --frozen --offline --all-targets` | RC0, 14:01:59–14:03:19 UTC | Même source R3 |

Ce sont **20 fonctions uniques sur deux sources**, pas vingt exécutions sur
un seul gel. R2→R3 ne modifie que l’attente de `tests/setup_prepare.rs` :
normalisation par le type public OIDC avant comparaison de la valeur JSON
complète. Les six fichiers produit et les trois fichiers définissant les
quinze fonctions de bibliothèque sont inchangés. Les résultats R2 restent R2.

R1 conserve sa compilation RC101 : deux assertions `assert_eq!` exigeaient
`Debug` sur `NativeIdentity`. R2 remplace ces macros par la même égalité exacte
sans changer le produit. Puis la première CLI R2 échoue après préparation
réussie : l’attente brute omettait le null et les limites OIDC par défaut.
Les quatre autres CLI R2 n’ont pas été exécutées. R3 corrige seulement cette
attente et passe les cinq CLI. Ces erreurs de préparation ne sont pas des
RED produit. Les builds R2 (13:46:41–13:48:11) et R3 (14:00:06–14:00:15) sont RC0,
avec réutilisation explicite du cache de dépendances.

Les sélections réelles emploient `--exact --nocapture --test-threads=1`,
un environnement explicite et un timeout de120s/kill10s par parent, dans la
QA admise. Les quinze fonctions sont définies dans
`src/setup/{tests,v2_tests,v2_contract_tests}.rs` ; les cinq CLI sont dans
`tests/setup_prepare.rs`. Les listes, commandes admises et captures exactes
restent dans les index privés `ROOT_LIBRARY15_INDEX.json` et
`ROOT_CLI5_INDEX.json` sous `qa-setup-v2-20261110-r1`.

## Liaison des preuves et composition

- Revue source indépendante : `4c33db10a37a9991aff9067152fe6687211b099cf68e8dcc5633ecd31df0e2c6`.
- Revue des trois deltas de tests R4/R5 : `b08f089dfbac316a6d682124ebfacffb8d6d4dc2b203822718667060903cca76`.
- Index des 15 PASS R2 : `93a2ce509d9831b855071a08934f97db78c75baf07a0f3b4531144dc3eec890d`.
- Index des 5 PASS R3 : `506510dc0485aad4232a855250659f1a3e4eba643111c78141d5f612dce4c8d8`.
- Dix postimages publiables, manifeste : `6815f11803ad8ce4a2847ace229fb5aac6e888508046c6dfd851836a9c7893b1`.
- Archive réelle R3 : `d00f986407fa8a66958244623ef1eb0954aee8966aac7fb2bec6abc20352f96c`, 24 391 680 octets ; 2370 fichiers réguliers, contenus et chemins exacts.
- Archives réelles R1/R2 : `8e74593dab29143a3d75a5cb946e44ca8d47258b1fe7d2152660ff320ecd7a0d` / `8019138dca7e82d42a2371e560a9635ba9aa53e2780e5a8f03e069560743fc7c`, chacune 2370 fichiers exacts.
- Post-R3 Root : `f83988cd8ed19bc19a43d44c9a3257c2fec47ab3a806dfdf5cb82d3329d891a6`.
- Fermeture QA : `2506550b35d3d69f9da4a0e40f7740de66de1dc2d1c525bedcb8c7a1e116391d`.

Les huit préimages existantes du patch source sont identiques sur public
`fe99b82e254c6a2ac5290873347ededc1c50adc1` ; deux enfants de tests sont nouveaux.
Les dix postimages sont celles de R3. Aucun seed Auth19 ni ancien fichier
partagé n’est réimporté. Le candidat source sur fe99 compte2375 fichiers :
les cinq chemins supplémentaires et les écarts existants hors scope sont
Resume déjà publié et documentation/preuves. Le README de preuve ajoute
ensuite un fichier. La comparaison de hashes est conservée en privé ; elle
ne transfère aucun PASS au checkout WIP complet ou à cette composition.

Après check, le post-R3 vérifie2370 hashes/pathset exacts, non-réguliers0,
cache4 et OOM0. La QA ne conserve que son sommeil de contrôle puis est arrêtée
explicitement à14:07:29 UTC et retirée normalement ; exit137 est l’arrêt idle,
pas un OOM. Le pic mémoire est6 291 202 048 octets. L’essai de lecture `df -i`
avec options incompatibles reste RC1 ; sa lecture corrigée est RC0.
Les preuves et les trois sources réelles sont conservées hors worktree.

## Portée

Voir les guides [EN](../../../../setup/sso/README.md) et
[FR](../../../../setup/sso/README.fr.md) et l’[exemple public manifeste2](../../../../setup/sso/examples/local-demo.json).
Le plan et les capacités restent finis. Aucun nouveau droit, service, storage,
changement automatique d’une préparation V1 ou adoption d’une base existante.
Les coupures testées sont des retours d’erreur injectés autour de vraies
opérations fichiers/SQLite, pas des pertes OS/alimentation. La provenance
s’appuie sur le répertoire privé du contrôle ; elle ne protège pas du même
UID de contrôle, de root ou d’un administrateur du stockage. IdP/SSO réel,
Docker/Podman/Kubernetes, concurrence physique, runtime, capacité100 et
qualification production ne sont pas établis par ce lot. Aucun constat
d’audit n’est formellement clos par le total de tests. G09 FAIL,0/12 gates
production clos.
