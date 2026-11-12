# Diagnostic opérateur `ai-guard-diagnose` — handoff du lot E (11 novembre 2026)

Destinataires : Codex, qui intègre, et le mainteneur. Rédacteur : agent Claude
chargé du handoff. Il n'a écrit ni le code ni les tests du lot et n'a lancé
aucune exécution de QA : chaque énoncé vient d'une commande Git refaisable, des
journaux privés de QA, du journal de travail privé du lot ou des notes des
contrôles indépendants, et le texte dit laquelle de ces sources il emploie.

**Livraison logicielle d'un outil, pas une qualification.** Les tests lancent
le vrai binaire `ai-guard-diagnose` sur des installations synthétiques créées
par les vrais `agentsandbox init` et `setup prepare`, avec le vrai binaire
gateway comme oracle, sous Linux ARM64, dans un conteneur de QA sans réseau.
Aucun moteur, aucun fournisseur d'identité réel, aucun fournisseur IA, aucune
image de contrôle, aucun build `--release` et aucune exécution macOS ne sont
qualifiés. Aucun gate n'est fermé par ce lot.

Les tests ont été lancés par des agents Claude dans la QA isolée du lot. Luna,
préférée par le mainteneur pour l'exécution des tests, n'est pas disponible
dans cet outillage.

## À décider avant ou pendant l'intégration

Quatre points demandent une décision de Codex ou du mainteneur. Le premier et
le troisième concernent des fichiers que le lot ne possède pas ; le deuxième,
une admission que le mandat réserve. Aucun n'a reçu de réponse du mainteneur :
Q1 et Q2 sont des admissions du coordinateur Claude.

### 1. Le raccord fournisseur est hors de la liste des fichiers partagés du mandat (Q1)

- **Le fait.** Le mandat nomme deux fichiers partagés, `src/lib.rs` et
  `src/gateway_config.rs`, et deux propositions (image de contrôle, index de
  documentation). Le commit `a95bf67` (`wire: expose the pure provider profile
  choice`) touche en plus `src/provider.rs` (+9) et
  `src/provider/selection.rs` (+9). Le coordinateur Claude l'a admis seul
  (décision Q1 du 10 novembre, 21:35 UTC). Son motif, consigné au journal : le
  mandat interdit de reproduire un résolveur à la main, exige « client et mode
  explicite/automatique » et le test « choix explicite abonnement conservé »,
  et `src/provider` n'est pas dans les zones interdites.
- **Ce que le raccord fait.** `provider::choose_profile` délègue à
  `selection::choose_profile`, qui exécute `config.validate()`,
  `validate_model`, puis le `choose` privé existant, et rend le profil choisi
  avec sa `ChoiceSource`. Dix-huit lignes ajoutées, aucune ligne existante
  modifiée ; `prepare` et ses appelants sont inchangés ; aucun credential,
  fichier ni variable d'environnement n'est touché. `choose_profile` répète
  les trois premières instructions de `prepare` au lieu de les partager
  (proposition P3, section 9).
- **Ensemble de retrait, du plus récent au plus ancien.** Les phrases des
  guides (`a881c60`, `b7ad456`), la fonction (`8b52ed9`), puis le raccord
  (`a95bf67`). Recalculé au commit validé par simulation (section 4) : ce
  n'est **pas** une suite de `git revert`. Seul `a95bf67` s'applique sans
  conflit, et son retrait seul casse la compilation : `provider::choose_profile`
  est appelé par `src/diagnostics/providers.rs:378` et
  `tests/diagnose_cli.rs:5885` (par lecture). Les trois autres s'arrêtent sur
  des conflits, et des tests de commits ultérieurs emploient la sélection. Le
  retrait est une édition manuelle.
- **Ce que le retrait retire.** Tout ce qui concerne la sélection revient à
  l'état d'avant le raccord, celui du commit `ea6cc4a`, parent de `a95bf67`
  (relevé par `git show -U0 8b52ed9`, `git log -S` et lecture des deux
  arbres) :
  - les options `--client`, `--auth-mode`, `--client-profile` et `--model`, et
    le bloc `providers.selection` du rapport ;
  - sept codes, tous créés par `8b52ed9` : les cinq `provider.selection_*`
    (`resolved`, `ambiguous`, `no_profile`, `invalid`, `not_resolved`) et les
    deux codes qui portent l'énoncé de facturation,
    `provider.subscription_never_falls_back_to_api` et
    `provider.api_billing_notice`. Au commit validé, ces deux derniers ne sont
    émis qu'après l'appel de `provider::choose_profile`
    (`src/diagnostics/providers.rs:378`, puis `:427` et `:477`) ;
  - en sens inverse, `provider.selection_resolution_not_available` revient :
    `8b52ed9` l'a supprimé (c'est l'un des deux codes supprimés du point 4).
    L'outil dit de nouveau qu'il ne calcule pas le profil qu'un lancement
    retient, et un credential absent ou non conforme n'est de nouveau une
    erreur que pour un fichier qui ne déclare qu'un seul profil ; pour un
    fichier à plusieurs profils c'est un avertissement
    (`provider.unselected_profile_reference_absent` ou `…_unsafe`) ;
  - les deux exemples documentés à abonnement explicite
    (`report.providers-explicit-subscription.json` et `.txt`) et les passages
    des guides sur la sélection ; les deux rapports documentés
    `report.providers-two-modes.json` et `.txt`, qui portent le bloc de
    sélection, sont à régénérer dans la QA après le retrait (la section 4
    liste leurs conflits) ;
  - neuf tests entiers de `tests/diagnose_cli.rs`. Six créés par `8b52ed9` :
    `explicit_subscription_is_preserved`,
    `refused_subscription_never_becomes_api`,
    `auto_resolution_and_billing_notice`, `selection_refusals_are_classified`,
    `selection_never_reads_credentials` et
    `selection_values_cannot_forge_or_leak`. Trois créés ensuite :
    `a_refused_subscription_states_the_automatic_rule_of_the_product`
    (`a2a6855`),
    `the_model_of_a_profile_is_resolved_as_the_product_resolves_it`
    (`44b5c4b`) et `the_selection_of_run_is_built_as_it_is_restated`
    (`4721538`) ;
  - une partie des cas de huit autres tests, qui passent une option de
    sélection ou lisent le bloc ou un code de la sélection et sont à éditer :
    `provider_references_from_metadata`, `hostile_environment_is_ignored`
    (quatre de ses six passes), `refusals_and_usage`,
    `json_contract_round_trips`, `documented_examples_are_the_real_output`,
    `the_entry_of_a_file_read_before_it_was_named_says_so`,
    `long_values_are_masked_before_they_are_clipped` et
    `a_credential_refused_for_its_path_is_reported`. Cette liste vient d'une
    recherche de texte au commit validé : un test qui attend le verdict du
    profil résolu sans nommer la sélection n'y figure pas.
- **Ce que le retrait rouvre.** Deux phrases du mandat et un test mandaté :
  - « client et mode explicite/automatique », avec le test mandaté « choix
    explicite abonnement conservé » ;
  - **« Un abonnement demandé ne bascule jamais vers une API payante. »** Ce
    critère perd toute sa preuve exécutée. Les trois tests que la section 5
    cite pour lui (`refused_subscription_never_becomes_api`,
    `a_refused_subscription_states_the_automatic_rule_of_the_product`,
    `explicit_subscription_is_preserved`) partent avec le raccord, et sans
    `--auth-mode` l'outil ne reçoit plus aucune demande d'abonnement. À
    `ea6cc4a`, les cas d'abonnement de `provider_references_from_metadata`
    sont un fichier qui ne déclare qu'un profil abonnement, où aucun profil
    API n'existe, et un fichier à quatre profils, où l'outil rapporte un
    avertissement pour le credential absent sans dire quel profil un
    lancement retient (par lecture du test et du catalogue à ce commit) :
    aucun ne prouve ce critère.

  La section 5 marque « Q1 » chaque ligne dont une preuve dépend du raccord.
- **État.** L'arbre sans le raccord n'a jamais été compilé ni testé :
  `NOT_RUN`.

### 2. La vérification active `--active oidc-discovery` n'a que l'admission du coordinateur (Q2)

- **Le fait.** Le mandat dit : « Les vérifications actives éventuelles
  demandent un flag explicite et emploient les adaptateurs sûrs existants avec
  timeouts/plafonds ; elles ne sont pas exécutées sans admission séparée. » Le
  coordinateur Claude a admis seul, et de façon bornée, cette vérification et
  son test (décision Q2 du 10 novembre, 21:35 UTC), en s'appuyant sur la phrase
  du mandat qui autorise « les fixtures Rust contrôlées et HTTP loopback dans
  la QA ».
- **Ce qui a été exécuté.** Uniquement dans le conteneur de QA, réseau `none`,
  contre deux émetteurs factices en loopback : la fixture du dépôt
  (`tests/support/oidc_fixture.rs`, non modifiée) et l'émetteur enregistreur du
  lot (`tests/diagnose_support/recording_issuer.rs`). Transport HTTP de
  démonstration seulement. Aucune requête HTTPS, aucun fournisseur d'identité
  réel, aucune exécution hors de ces tests. Sans `--active`, l'outil n'ouvre
  aucune connexion (`no_network_by_default`).
- **Ensemble de retrait, du plus récent au plus ancien.** `ed4c8be` (phrases
  des guides), `756dc9b` et `db73bfb` (tests), `af2438d` (correction Clippy du
  rapport d'identité), `e3ef698` (la fonction). Recalculé au commit validé :
  seul `db73bfb` s'applique sans conflit ; `b1001d8` a modifié depuis
  l'émetteur enregistreur, `dad29a1` l'aide, `469d625` les phrases des guides.
  Édition manuelle. La section du volume des guides dit « leave `--active`
  out » : à supprimer si la vérification est retirée et la section gardée.
- **Ce que le retrait retire.** L'option `--active`, dont `oidc-discovery`
  est la seule valeur ; les cinq codes `active.oidc_discovery_*` (`ok`,
  `transport_unavailable`, `provider_rejected`, `metadata_mismatch`,
  `keys_invalid`), créés par `e3ef698` ; la valeur `active_checks_only` de
  `mode.network`, que le même commit introduit ; le test
  `active_oidc_discovery_loopback` (`e3ef698`) et l'émetteur enregistreur
  (`756dc9b`) ; les passages des guides sur la vérification active. En sens
  inverse, `identity.idp_reachability_not_checked` redevient
  `identity.idp_reachability_not_available`, que `e3ef698` avait remplacé
  (l'autre code supprimé du point 4). `no_network_by_default`, créé à E-S1,
  reste. Il ne reste alors aucune vérification active : la phrase du mandat
  citée ci-dessus n'a plus d'objet dans ce lot.
- **État.** L'arbre sans la vérification active n'a jamais été compilé ni
  testé : `NOT_RUN`.

### 3. Trois propositions sur des fichiers partagés, à garder ou à retirer

| Proposition | Commits | Retrait, recalculé au commit validé |
|---|---|---|
| Image de contrôle : construire et copier `ai-guard-diagnose` à côté d'`agentsandbox` | `78a5aa4` (`images/control.Dockerfile`, +2 −1) | Retirable seul, sans conflit ; les guides restent vrais, leurs phrases sont conditionnelles depuis `755efe5` |
| Section « Inspect a prepared installation inside its volume » des deux guides | `f749265`, `755efe5` | `git revert 755efe5 f749265 78a5aa4`, dans cet ordre : `755efe5` propre, `f749265` laisse un conflit dans chaque guide de diagnostic (garder le côté du revert et supprimer la section), `78a5aa4` propre |
| Quatre lignes d'index : `README.md`, `README.fr.md`, `docs/setup/README.md`, `docs/setup/README.fr.md` | `26166e4`, puis `68bea8c`, `771dde4`, `1fc07b5`, `4ecd6dc` | `git revert 4ecd6dc 1fc07b5 771dde4 68bea8c 26166e4` : sans conflit, et les quatre fichiers retrouvent leur blob de la base. Les cinq se gardent ou se retirent ensemble (section 4) |

- **Conflits avec la branche d'intégration.** Ces propositions sont écrites
  contre la base. La branche `codex/human-brokers-validation-20261104` est
  passée de `3145315` à `4b1927f` pendant la journée. Fusion à trois voies du
  commit validé avec `4b1927f`, simulée hors du dépôt : **exactement trois
  fichiers en conflit**, `images/control.Dockerfile` (2 zones),
  `docs/setup/README.md` (1) et `docs/setup/README.fr.md` (1).
- **Résolution.** Dans le Dockerfile :
  `RUN cargo build --release --locked --bin agentsandbox --bin ai-guard-audit-checkpoint --bin ai-guard-diagnose`,
  puis les deux lignes `COPY` après celle d'`agentsandbox`
  (`ai-guard-audit-checkpoint`, puis `ai-guard-diagnose`). Dans les deux
  index : garder les deux lignes après la ligne Kubernetes (« Signed audit
  checkpoints », puis « Operator diagnostics »).
- **État.** L'arbre fusionné n'a jamais été compilé. La construction de
  l'image, tout conteneur de cette image et chaque commande `docker` de la
  section du volume sont `NOT_RUN` ; les guides le disent. `cargo build
  --release --locked` est `NOT_RUN` aussi.

### 4. La version 1 du rapport n'est pas figée

Le contrat JSON est annoncé « version 1 ». Rien n'a été publié : la branche n'a
jamais été poussée. La version 1 est aujourd'hui le catalogue du commit validé
(131 codes, comptés dans le catalogue du guide). Quatre choses bougent encore.

- **Une question d'axe ouverte.** `trust.decision_transport_invalid` est sur
  l'axe `trust`, alors que `inference.decision_transport_valid` et
  `inference.decision_transport_not_checked` sont sur l'axe `inference`. La
  règle de compatibilité des guides interdit de déplacer un code une fois la
  version 1 publiée : à confirmer ou à changer avant la première publication.
- **Deux codes supprimés avant toute publication.**
  `provider.selection_resolution_not_available` et
  `identity.idp_reachability_not_available` ont été créés par `a02a90e` et
  retirés par `8b52ed9` et `e3ef698`, alors que l'en-tête de
  `src/diagnostics/catalogue.rs` dit « A retired code stays in the catalogue ».
  À confirmer, ou les réintroduire comme codes retirés.
- **Trois énoncés vrais à la base et faux sur la branche d'intégration, sans
  qu'aucun test passe RED.** (1) `layout.legacy_commands_do_not_read_this_layout`
  et son action : sur cette branche `verify-audit` accepte
  `--viewer-token-file`, et le guide local a la section « Verify the audit for
  a SetupV2 tenant ». (2) `data.offline_inspection_not_available` (« This
  build has no offline read-only inspection of the stores ») : cette branche
  livre `ai-guard-audit-checkpoint`, qui vérifie une base d'audit ouverte en
  lecture seule. (3) Les lignes des deux guides qui répètent ces deux énoncés.
  Étape d'intégration en section 9 (I2).
- **Un miroir exact à la base seulement.** Les enveloppes du registre et du
  fichier OIDC sont restituées dans `src/diagnostics/gateway_files.rs`. Sur la
  branche d'intégration, `TenantFiles` a le champ `relay_retention_policy` : à
  la fusion le test de parité
  `diagnostics_parity_envelope_field_names_equal_the_loaders` passe RED, par
  conception, et tant que le miroir n'est pas mis à jour un registre qui
  déclare ce champ, que le gateway accepte, est rapporté
  `registry.json_invalid` (erreur, sortie 3). Étape d'intégration en
  section 9 (I1).

Les autres décisions, ouvertes ou prises par le coordinateur et renversables,
sont en section 8 : sortie 3 pour une sélection
automatique que le produit refuse, base `inferred` des trois codes « path »,
couple de commits `c7fd54c` et `16eda98`, lecture du critère Clippy pour un
fichier partagé, journaux bruts privés ou versionnés.

## Lecture rapide

- **Livré** : le binaire `ai-guard-diagnose` et le module `diagnostics`
  (15 fichiers Rust), un contrat JSON versionné et un rapport texte, une cible
  de 84 tests qui lance le vrai binaire, 8 tests de parité avec les chargeurs
  du gateway, 4 tests unitaires, les guides EN/FR (1062 lignes chacun) et dix
  fichiers d'exemples (neuf rapports documentés et un fichier de profils).
- **Validation finale** sur `469d6259c305c415695968334bf3a1eb5b355a5c` : neuf
  sélections exécutées, toutes RC 0, aucun échec, aucune relance (section 6).
- **Revue indépendante** : 53 constats, tous traités ; aucun écarté en silence
  (section 7).
- **À décider** : les quatre points ci-dessus et les questions de la
  section 8.
- **À faire à l'intégration** : section 12.

## 1. Identité de la livraison

| Élément | Valeur |
|---|---|
| Lot | E, `operator-diagnostics` |
| Branche | `claude/operator-diagnostics-20261110`, locale, jamais poussée par Claude |
| Worktree | `.worktrees/operator-diagnostics-20261110` |
| Base figée | `2cc29ee42ae3eb3cc8203538bf7a09aee2e26b57` |
| **Source validée (V)** | `469d6259c305c415695968334bf3a1eb5b355a5c`, 99 commits depuis la base, arbre propre |
| Tête de la branche | V, plus le ou les commits qui n'ajoutent que ce fichier. Ce document ne peut pas nommer son propre commit : l'index privé du handoff le nomme, et `git diff --name-only 469d6259c305c415695968334bf3a1eb5b355a5c HEAD` ne doit lister que `docs/engineering/evidence/2026-11-10/operator-diagnostics/README.md` |
| Gel de source validé | 2411 fichiers, manifeste `b8f0926459bc7d95c27e5da78567ffbe866339f5ef1df00f0cd9c3d6a6b36bbf`, égal au manifeste relevé après chaque exécution |
| Propriétaire | Claude : une session coordinatrice et un seul agent d'implémentation à la fois |
| Intégrateur | Codex : intégration, validations de composition, push, fermeture du worktree |
| Premier et dernier commit de V | 10 novembre 2026, 21:54:52 UTC ; 11 novembre 2026, 20:31:01 UTC |
| Guides utilisateur | [diagnostic EN](../../../../setup/diagnostics/README.md) / [FR](../../../../setup/diagnostics/README.fr.md) |

Déroulement, en UTC, d'après le journal de travail privé :

- 10 novembre : création du worktree, de la branche, du conteneur et du volume
  de QA ; conception, puis révision de la conception après trois critiques
  indépendantes (46 constats : 43 acceptés, 3 acceptés en partie) ; étapes E-S1
  et E-S2.
- 11 novembre, 00:47 à 03:00 : étapes E-S3 et E-S4.
- **Interruption.** Le workflow s'est arrêté après E-S4 : agents arrêtés sur
  « out of usage credits », et une mise en veille du Mac pendant la première
  tentative de E-S4. Le contrôle indépendant de E-S4 n'a pas tourné. Ce n'est
  ni un refus d'outillage ni un défaut du produit.
- 10:45 : reprise par le coordinateur. Étapes E-S4b, E-S5, E-S6, chacune avec
  un contrôle indépendant passé.
- 15:20 : résultat de la revue indépendante sur `26166e4`. Étapes de
  correction E-F1 à E-F4, puis E-F5 (points bloquants des contrôles) et E-F6
  (notes non bloquantes des contrôles).
- **Incident d'orchestration, 20:02.** Le workflow a lancé la validation
  finale sur `433eba9` avant l'insertion de E-F6 ; le coordinateur l'a arrêtée.
  Le journal `20261111T200056Z-v1-final-cli-redate.log` de ce lancement porte
  `rc=0` sans aucune ligne `test result:` : il n'établit rien et n'est cité
  nulle part comme résultat.
- 20:46 à 20:57 : validation finale V, entière, sur `469d625`.

Vérifications de l'arbre, refaites pour ce handoff au commit validé :

| Contrôle | Commande | Constat |
|---|---|---|
| Branche courante | `git rev-parse --abbrev-ref HEAD` | `claude/operator-diagnostics-20261110` |
| Arbre propre | `git status --porcelain --ignored` | 0 entrée |
| Base ancêtre de V | `git merge-base --is-ancestor 2cc29ee… 469d625…` | RC 0 |
| Historique linéaire | `git rev-list --count`, `--merges --count` | 99 commits, 0 fusion |
| Attribution | trailers `Co-Authored-By` | présents sur les 99 commits |
| Non poussée | `git branch -r --contains 469d625…` | aucune référence distante |
| Étendue du diff | `git diff --shortstat 2cc29ee… 469d625…` | 44 fichiers, 28539 lignes ajoutées, 1 retirée ; 35 fichiers ajoutés, 9 modifiés |
| Fichiers partagés | `git log 2cc29ee…..469d625… -- <fichier>`, un fichier à la fois | chacun n'est touché que par des commits `wire:` ; les neuf commits `wire:` ne touchent que des fichiers partagés |
| Zones interdites | `git diff --stat` filtré sur `Cargo.toml`, `Cargo.lock`, `src/main.rs`, `src/control_fs.rs`, `tests/support`, `src/operations.rs`, `src/runtime.rs`, `src/runtime`, `src/broker`, `src/audit`, `src/relay`, `src/runner.rs`, `src/oidc`, `front` | vide : aucun changement |
| Espaces | `git diff --check 2cc29ee… 469d625…` | vide |

## 2. Ce que l'outil est, ce qu'il n'est pas, et le périmètre exact

`ai-guard-diagnose` est un binaire Rust auto-découvert
(`src/bin/ai-guard-diagnose.rs`) au-dessus du module `agentsandbox::diagnostics`.
Il écrit un rapport texte, ou avec `--json` un document JSON de version 1.

- **Pas un service, pas une autorité.** Un processus court. Il n'accorde rien
  et aucun composant ne consomme son résultat.
- **Hors ligne par défaut.** Aucune connexion sans `--active oidc-discovery`.
  Une URL donnée en option est validée, jamais contactée.
- **Sans mutation.** Rien n'est créé, modifié, migré ni supprimé : ni base, ni
  verrou, ni credential, ni répertoire. Aucune base SQLite n'est ouverte ;
  l'axe des données reste à l'état des métadonnées de fichiers.
- **Aucun contenu de credential.** Les fichiers que les fichiers inspectés
  nomment comme clé, jeton, credential de fournisseur, enregistrement de
  préparation ou base sont décrits par leurs métadonnées, jamais ouverts. Un
  fichier donné par option est ouvert comme ce que l'option dit qu'il est : la
  limite et la façon dont le rapport la signale sont en section 9.
- **Ligne de commande seulement.** Aucune variable d'environnement, aucun
  fichier dotenv, aucun programme cherché dans `PATH`.
- **Il ne remplace pas les diagnostics existants** (`doctor`, `providers
  status`, `healthcheck`, `verify-audit`, `kubernetes doctor`, `broker
  recovery-status`) : il dit lequel lancer ensuite.

Codes de sortie : 0 (rapport sans `error` ni `unknown`), 3 (au moins un
`error`), 4 (aucun `error`, au moins un `unknown`), 2 (erreur d'usage), 1
(refus, aucun rapport). Un code 0 ne dit pas que l'installation démarre.

### Fichiers ajoutés et modifiés

`git diff --numstat 2cc29ee42ae3eb3cc8203538bf7a09aee2e26b57 469d6259c305c415695968334bf3a1eb5b355a5c` :
44 fichiers, dont 26 fichiers Rust (22 possédés, 4 partagés).

| Nature | Fichier | État | + / − |
|---|---|---|---|
| **Partagé**, raccord `wire:` | `README.fr.md` | modifié | 2 / 0 |
| **Partagé**, raccord `wire:` | `README.md` | modifié | 2 / 0 |
| **Partagé**, raccord `wire:` | `docs/setup/README.fr.md` | modifié | 1 / 0 |
| **Partagé**, raccord `wire:` | `docs/setup/README.md` | modifié | 1 / 0 |
| Possédé : guide | `docs/setup/diagnostics/README.fr.md` | ajouté | 1062 / 0 |
| Possédé : guide | `docs/setup/diagnostics/README.md` | ajouté | 1062 / 0 |
| Possédé : exemple documenté | `docs/setup/diagnostics/examples/providers.two-modes.json` | ajouté | 48 / 0 |
| Possédé : exemple documenté | `docs/setup/diagnostics/examples/report.first-run.txt` | ajouté | 161 / 0 |
| Possédé : exemple documenté | `docs/setup/diagnostics/examples/report.local-fresh.json` | ajouté | 622 / 0 |
| Possédé : exemple documenté | `docs/setup/diagnostics/examples/report.local-fresh.txt` | ajouté | 213 / 0 |
| Possédé : exemple documenté | `docs/setup/diagnostics/examples/report.prepared-v2.json` | ajouté | 962 / 0 |
| Possédé : exemple documenté | `docs/setup/diagnostics/examples/report.prepared-v2.txt` | ajouté | 318 / 0 |
| Possédé : exemple documenté | `docs/setup/diagnostics/examples/report.providers-explicit-subscription.json` | ajouté | 918 / 0 |
| Possédé : exemple documenté | `docs/setup/diagnostics/examples/report.providers-explicit-subscription.txt` | ajouté | 293 / 0 |
| Possédé : exemple documenté | `docs/setup/diagnostics/examples/report.providers-two-modes.json` | ajouté | 924 / 0 |
| Possédé : exemple documenté | `docs/setup/diagnostics/examples/report.providers-two-modes.txt` | ajouté | 305 / 0 |
| **Partagé**, raccord `wire:` | `images/control.Dockerfile` | modifié | 2 / 1 |
| Possédé : binaire | `src/bin/ai-guard-diagnose.rs` | ajouté | 35 / 0 |
| Possédé : module | `src/diagnostics.rs` | ajouté | 1619 / 0 |
| Possédé : module | `src/diagnostics/catalogue.rs` | ajouté | 1456 / 0 |
| Possédé : module | `src/diagnostics/engine.rs` | ajouté | 269 / 0 |
| Possédé : module | `src/diagnostics/fs_facts.rs` | ajouté | 446 / 0 |
| Possédé : module | `src/diagnostics/gateway_files.rs` | ajouté | 792 / 0 |
| Possédé : module | `src/diagnostics/identity.rs` | ajouté | 412 / 0 |
| Possédé : module | `src/diagnostics/layout.rs` | ajouté | 463 / 0 |
| Possédé : module | `src/diagnostics/output.rs` | ajouté | 338 / 0 |
| Possédé : module | `src/diagnostics/providers.rs` | ajouté | 673 / 0 |
| Possédé : module | `src/diagnostics/references.rs` | ajouté | 363 / 0 |
| Possédé : module | `src/diagnostics/render.rs` | ajouté | 804 / 0 |
| Possédé : module | `src/diagnostics/report.rs` | ajouté | 917 / 0 |
| Possédé : module | `src/diagnostics/tenants.rs` | ajouté | 342 / 0 |
| Possédé : module | `src/diagnostics/trust.rs` | ajouté | 223 / 0 |
| **Partagé**, raccord `wire:` | `src/gateway_config.rs` | modifié | 4 / 0 |
| Possédé : tests | `src/gateway_config_diagnostics_parity_tests.rs` | ajouté | 1093 / 0 |
| **Partagé**, raccord `wire:` | `src/lib.rs` | modifié | 1 / 0 |
| **Partagé**, raccord `wire:` | `src/provider.rs` | modifié | 9 / 0 |
| **Partagé**, raccord `wire:` | `src/provider/selection.rs` | modifié | 9 / 0 |
| Possédé : tests | `tests/diagnose_cli.rs` | ajouté | 10021 / 0 |
| Possédé : tests | `tests/diagnose_support/child.rs` | ajouté | 274 / 0 |
| Possédé : tests | `tests/diagnose_support/fixtures.rs` | ajouté | 613 / 0 |
| Possédé : tests | `tests/diagnose_support/fixtures/public-test-root.pem` | ajouté | 12 / 0 |
| Possédé : tests | `tests/diagnose_support/mod.rs` | ajouté | 68 / 0 |
| Possédé : tests | `tests/diagnose_support/recording_issuer.rs` | ajouté | 207 / 0 |
| Possédé : tests | `tests/diagnose_support/tree_manifest.rs` | ajouté | 180 / 0 |

Ce que ces fichiers lisent sans le modifier, et qui les lie au reste du dépôt :

- `tests/diagnose_cli.rs` compile par `#[path]` le fichier partagé
  `tests/support/oidc_fixture.rs`, et lit par `include_str!`
  `config/tenants.example.json`, `src/runtime/invocation.rs`,
  `src/control_fs.rs`, `src/audit/storage.rs` et `src/runner.rs`.
- `tests/diagnose_support/fixtures.rs` inclut
  `docs/setup/sso/examples/local-demo.json` ; le fichier de parité inclut
  `src/gateway_config.rs`, `docs/setup/sso/examples/gateway.https.json`,
  `local-demo.json` et le certificat public de test.
- `src/diagnostics/fs_facts.rs` inclut `src/control_fs.rs` sous `#[cfg(test)]`.

Un déplacement de l'un de ces fichiers casse la construction de ces cibles ;
une reformulation fait échouer le test de garde concerné, par conception.

Inchangés par le lot : `Cargo.toml`, `Cargo.lock` (aucune dépendance ajoutée),
`src/main.rs`, `src/control_fs.rs`, `src/operations.rs`, `src/runtime.rs`,
`src/runtime/**`, `src/broker/**`, `src/audit/**`, `src/relay/**`,
`src/runner.rs`, `src/oidc/**`, `tests/support/**`, `front/**`, et tous les
documents d'état de `docs/engineering/`. Aucun format persistant n'est touché.

## 3. Commits

Quatre-vingt-dix-neuf commits de la base au commit validé, puis le ou les
commits de ce handoff. Les étapes sont celles du journal de travail : E-S1 à
E-S6 construisent le lot, E-F1 à E-F6 traitent la revue indépendante et les
notes des contrôles. Les SHA complets et les fichiers de chaque commit sont
dans la liste privée des commits (section 11).

| N° | Commit | Date (UTC) | Étape | Objet |
|---:|---|---|---|---|
| 1 | `a02a90e` | 11-10 21:54 | E-S1 | feat: add the operator diagnostics report contract |
| 2 | `6adaf64` | 11-10 21:54 | E-S1 | **wire: declare the diagnostics module** |
| 3 | `b6182da` | 11-10 21:54 | E-S1 | feat: add the ai-guard-diagnose binary |
| 4 | `88aa03e` | 11-10 22:18 | E-S1 | fix: list an assumed input only with the check that uses it |
| 5 | `b46bf41` | 11-10 22:21 | E-S1 | test: exercise ai-guard-diagnose on synthetic installations |
| 6 | `32b27cd` | 11-10 22:21 | E-S1 | fix: name the list bounds of the report and default the output guard |
| 7 | `faa398d` | 11-10 23:15 | E-S2 | feat: inspect layouts and credential references |
| 8 | `1888c43` | 11-10 23:17 | E-S2 | feat: inspect the registry, the OIDC file and tenant plans |
| 9 | `13d099c` | 11-10 23:17 | E-S2 | **wire: run diagnostics parity tests beside gateway_config** |
| 10 | `ff7b73d` | 11-10 23:17 | E-S2 | test: cover layout edge states of ai-guard-diagnose |
| 11 | `fd5f384` | 11-10 23:23 | E-S2 | fix: never open a file below the key directories as a CA bundle |
| 12 | `fa216a9` | 11-10 23:26 | E-S2 | fix: clear two Clippy findings of the diagnostics inspection |
| 13 | `320e6a2` | 11-11 00:05 | E-S3 | feat: explain provider profiles and their references |
| 14 | `3b7a437` | 11-11 00:17 | E-S3 | feat: validate CA bundles and approve the engine program offline |
| 15 | `bbd54f5` | 11-11 00:22 | E-S3 | fix: keep the facts of the text report within its width |
| 16 | `40058f3` | 11-11 00:25 | E-S3 | fix: never open a key file given as the registry or the OIDC file |
| 17 | `bdf3510` | 11-11 00:39 | E-S3 | fix: reserve key files whatever option gives their directory |
| 18 | `667fc0f` | 11-11 00:41 | E-S3 | test: tie the closed list of files read to what is opened |
| 19 | `6c05f1c` | 11-11 00:41 | E-S3 | fix: label the verdicts of the restated envelopes as inferred |
| 20 | `1b3ee2a` | 11-11 00:41 | E-S3 | refactor: keep the output helpers private to the diagnostics module |
| 21 | `e68aa6f` | 11-11 02:52 | E-S4 | docs: add the diagnostics Quick Start in English and French |
| 22 | `4ff5a4c` | 11-11 02:53 | E-S4 | test: bind the diagnostics guides and their examples to the binary |
| 23 | `316f806` | 11-11 02:56 | E-S4 | docs: state who reads each layout and what a read changes |
| 24 | `70b5e61` | 11-11 10:59 | E-S4b | fix: never hash a reserved file given as the engine program |
| 25 | `c01de54` | 11-11 11:08 | E-S4b | fix: compare a file before it is opened and say when one was read too early |
| 26 | `b7a2b9a` | 11-11 11:13 | E-S4b | feat: say when the decision-engine transport was not judged |
| 27 | `874ab48` | 11-11 11:17 | E-S4b | fix: say what is wrong with an engine program path and a credential path |
| 28 | `b4a9389` | 11-11 11:21 | E-S4b | feat: list the authentication records of a prepared installation |
| 29 | `8a46e46` | 11-11 11:27 | E-S4b | test: bind the engine refusal texts to the product source |
| 30 | `2935df2` | 11-11 11:31 | E-S4b | fix: word the mode line by what the tool can know |
| 31 | `ea6cc4a` | 11-11 11:34 | E-S4b | docs: name the fixed names in the table of reserved files |
| 32 | `a95bf67` | 11-11 11:59 | E-S5 | **wire: expose the pure provider profile choice** |
| 33 | `8b52ed9` | 11-11 12:29 | E-S5 | feat: resolve the provider selection without opening a credential |
| 34 | `e3ef698` | 11-11 13:02 | E-S5 | feat: add the opt-in OIDC discovery check |
| 35 | `af2438d` | 11-11 13:14 | E-S5 | fix: clear a Clippy finding of the identity report |
| 36 | `b7ad456` | 11-11 13:14 | E-S5 | docs: say where the model variable of the selection example comes from |
| 37 | `78a5aa4` | 11-11 13:48 | E-S6 | **wire: build and ship ai-guard-diagnose in the control image** |
| 38 | `f749265` | 11-11 13:52 | E-S6 | docs: inspect a prepared installation inside its volume |
| 39 | `26166e4` | 11-11 13:52 | E-S6 | **wire: list the diagnostics guide in the manual indexes** |
| 40 | `e33cca9` | 11-11 15:27 | E-F1 | fix: register what a registry and an OIDC file name, whatever is described |
| 41 | `65a3bd9` | 11-11 15:31 | E-F1 | fix: register the target of a reference that is a symbolic link |
| 42 | `477d07f` | 11-11 15:35 | E-F1 | fix: list a registry as read where it is read |
| 43 | `8dfa6b5` | 11-11 15:39 | E-F1 | fix: say in its entry that a file was read before it was named |
| 44 | `68bea8c` | 11-11 15:39 | E-F1 | **wire: state in README.md which files the diagnostics tool never opens** |
| 45 | `771dde4` | 11-11 15:39 | E-F1 | **wire: state in README.fr.md which files the diagnostics tool never opens** |
| 46 | `1fc07b5` | 11-11 15:39 | E-F1 | **wire: state in docs/setup/README.md which files the diagnostics tool never opens** |
| 47 | `4ecd6dc` | 11-11 15:39 | E-F1 | **wire: state in docs/setup/README.fr.md which files the diagnostics tool never opens** |
| 48 | `bb89f6a` | 11-11 15:44 | E-F1 | fix: never show a path relative to the root of the file system |
| 49 | `d1a3673` | 11-11 15:46 | E-F1 | fix: escape the characters that a reader does not see |
| 50 | `f5c6318` | 11-11 15:48 | E-F1 | fix: report a policy refused at the assumed event bound as unknown |
| 51 | `4eb8061` | 11-11 15:50 | E-F1 | fix: say in the text report what a bounded list does not print |
| 52 | `0287159` | 11-11 15:53 | E-F1 | fix: count a tenant identifier once in the totals |
| 53 | `dad29a1` | 11-11 15:56 | E-F1 | fix: state at every entry point which files are never opened |
| 54 | `a2a6855` | 11-11 15:59 | E-F1 | fix: state the automatic rule where a subscription is refused |
| 55 | `60673f2` | 11-11 16:01 | E-F1 | test: fail a test whose oracle could not run |
| 56 | `c69094f` | 11-11 16:02 | E-F1 | test: exercise the clipping of long values and a failed write |
| 57 | `d25454f` | 11-11 16:12 | E-F1 | docs: tighten two guide sentences on root paths and the automatic choice |
| 58 | `c73f538` | 11-11 16:51 | E-F2 | fix: report a credential that its reader refuses for its path |
| 59 | `223a0d7` | 11-11 16:56 | E-F2 | fix: tell an absent registry from one that this account cannot read |
| 60 | `bb0320e` | 11-11 17:00 | E-F2 | fix: state what a plan mismatch allows once an audit stores the plan |
| 61 | `27c1fa4` | 11-11 17:03 | E-F2 | fix: give each refusal of the engine approval its cause and its action |
| 62 | `75ce006` | 11-11 17:06 | E-F2 | fix: name the fields of the gateway block that a refusal is about |
| 63 | `fcfc0ce` | 11-11 17:08 | E-F2 | fix: warn about the demonstration transport of an OIDC file |
| 64 | `4e2362c` | 11-11 17:12 | E-F2 | fix: name the local HTTP permission with a CA bundle as a cause |
| 65 | `08f6ae1` | 11-11 17:17 | E-F2 | fix: make three actions of the local mode followable |
| 66 | `c56e13d` | 11-11 17:21 | E-F2 | fix: tell apart the causes of a refused directory and of a refused bundle |
| 67 | `74b0503` | 11-11 17:23 | E-F2 | fix: state in the help the rules between options |
| 68 | `7ad4d5b` | 11-11 17:26 | E-F2 | fix: say when an option was given without effect |
| 69 | `c1fcf30` | 11-11 17:28 | E-F2 | fix: report a published installation without its audit as an error |
| 70 | `cb2fd5a` | 11-11 17:30 | E-F2 | fix: refuse the store names that the gateway refuses |
| 71 | `cda5e97` | 11-11 17:33 | E-F2 | fix: name the two file options for gateway files without setup records |
| 72 | `52af9f6` | 11-11 17:35 | E-F2 | fix: do not word a mode check as an acceptance by the gateway |
| 73 | `46f4582` | 11-11 18:13 | E-F3 | test: assert the cause that the gateway states where it refuses |
| 74 | `09c78a1` | 11-11 18:13 | E-F3 | test: pin three oracles that accepted more than they state |
| 75 | `db73bfb` | 11-11 18:13 | E-F3 | test: hold the port of the silent issuer for its whole case |
| 76 | `f27321d` | 11-11 18:13 | E-F3 | test: run the hostile environment against the provider and engine axes |
| 77 | `756dc9b` | 11-11 18:13 | E-F3 | test: record the request line and the header names of the active check |
| 78 | `07b0340` | 11-11 18:13 | E-F3 | test: make the binary report a refused registry and a refused OIDC file |
| 79 | `b9719ba` | 11-11 18:13 | E-F3 | test: inspect the stores of a running installation and of refused ones |
| 80 | `44b5c4b` | 11-11 18:13 | E-F3 | test: resolve the model of a profile as the product resolves it |
| 81 | `4721538` | 11-11 18:13 | E-F3 | test: bind the selection of run to the lines that build it |
| 82 | `8f4b9db` | 11-11 18:13 | E-F3 | fix: say which check refuses an authentication audit policy |
| 83 | `63867ee` | 11-11 18:13 | E-F3 | style: format one assertion of the bundle test as rustfmt does |
| 84 | `a457be9` | 11-11 18:28 | E-F3 | test: bind the message of a refused authentication policy to its wording |
| 85 | `0e00e5a` | 11-11 18:55 | E-F4 | docs: state the limits that the review left to the guides |
| 86 | `b6548b3` | 11-11 18:56 | E-F4 | docs: say what a default report prints and how to find its errors |
| 87 | `a881c60` | 11-11 18:56 | E-F4 | docs: say which launch the provider verdicts are computed for |
| 88 | `ed4c8be` | 11-11 18:57 | E-F4 | docs: tell the operator to read the target of the active check first |
| 89 | `755efe5` | 11-11 18:58 | E-F4 | docs: condition the container journey on the image and mark it not run |
| 90 | `de60a53` | 11-11 19:39 | E-F5 | fix: show a directory beside the root of the file system as itself |
| 91 | `69d2ce4` | 11-11 19:39 | E-F5 | test: run the default engine lookup under the hostile environment again |
| 92 | `433eba9` | 11-11 19:39 | E-F5 | test: show what the integrity refusal of an edited policy stands for |
| 93 | `c7fd54c` | 11-11 20:28 | E-F6 | fix: say what was read of a directory that holds only gateway files |
| 94 | `16eda98` | 11-11 20:30 | E-F6 | test: put the two cases of the gateway-files fix in their own tests |
| 95 | `1b6b1c3` | 11-11 20:30 | E-F6 | test: run a profile option that names the audit of a staging directory |
| 96 | `1c45ab4` | 11-11 20:30 | E-F6 | test: bind the provider-object message and the assumed event bound |
| 97 | `b1001d8` | 11-11 20:30 | E-F6 | test: end the recording issuer without joining a loop that was not woken |
| 98 | `a57855e` | 11-11 20:31 | E-F6 | fix: word the plan, key-file and path messages as the product behaves |
| 99 | `469d625` | 11-11 20:31 | E-F6 | docs: close the wording notes that the stage gates left on the guides |

Particularités à connaître avant un bisect ou un cherry-pick. L'historique
n'est pas réécrit : aucun rebase, reset ni amend depuis E-S2. Le reflog de la
branche ne porte que des entrées `commit`, la création de la branche et un
seul `commit (amend)`, celui du commit de tests de E-S1, fait avant
l'exécution finale de cette étape d'après le journal.

- **`c7fd54c` ne construit pas sa cible de test CLI.** Son
  `tests/diagnose_cli.rs` ne s'analyse pas : deux blocs de test ont été
  indexés quatre-vingt-dix et cent cinquante lignes trop bas. Ses fichiers du
  produit et ses guides sont ceux voulus. `16eda98`, le commit suivant, remet
  les deux blocs, inchangés, dans leurs tests : prendre les deux commits
  ensemble. La règle « chaque commit compile » n'est donc pas tenue pour
  `c7fd54c`, et elle ne peut l'être sans l'amend ou le reset que les mêmes
  règles interdisent. Décision ouverte en section 8.
- **Commits intermédiaires non validés.** V ne valide que `469d625`. D'après
  le journal : à la tête de E-S3 (`1b3ee2a`) deux clauses de test restaient
  RED par conception jusqu'aux guides de E-S4 ; les arbres de `09c78a1` à
  `8f4b9db` (neuf commits de E-F3) ne sont pas propres pour rustfmt, ce que
  `63867ee` corrige (non revérifié ici) ; `07b0340`, `b9719ba` et `44b5c4b`
  finissent `tests/diagnose_cli.rs` par une ligne vide ; `16eda98`, `1b6b1c3`
  et `1c45ab4` n'ont jamais été construits en QA (analyse syntaxique
  seulement).
- **Quatre messages de commit disent plus que le commit.** `07b0340` dit que
  le gateway refuse « the same file for the same cause » : c'est trop pour le
  site « a policy that cannot fund the plan », où le gateway rend le refus
  d'intégrité de l'audit, pas celui du financement. `bb89f6a` dit « never show
  a path relative to the root of the file system » : vrai seulement depuis
  `de60a53` pour `--config-dir /` donné avec `--installation-dir`. `f27321d`
  avait retiré, sans le dire, les deux passes sans `--runtime-program` ;
  `69d2ce4` les remet. `c7fd54c` dit que ses deux cas « run through the real
  binary » : ils ne se construisent qu'à partir de `16eda98`.

### Raccords `wire:`

Neuf commits `wire:`, chacun réduit à ses hunks de fichiers partagés
(`git show --stat` de chacun ne liste que des fichiers partagés). Les en-têtes
de hunk viennent de `git show -U0`.

| Commit | Fichier | Hunks | Ce qu'il raccorde | Retrait |
|---|---|---|---|---|
| `6adaf64` | `src/lib.rs` | `@@ -9,0 +10 @@`, +1 −0 : `pub mod diagnostics;` | La déclaration du module | Revert textuellement propre, mais le binaire, le module et les tests ne se construisent plus : non retirable sans retirer le lot |
| `13d099c` | `src/gateway_config.rs` | `@@ -422,0 +423,4 @@`, +4 −0 : `#[cfg(test)]`, `#[path = "gateway_config_diagnostics_parity_tests.rs"]`, `mod diagnostics_parity_tests;` et une ligne vide | **La seule inclusion des tests de parité.** Ils sont enfants du module de configuration du binaire gateway, seul endroit d'où ses chargeurs privés (`load`, `validate_policies`, `prepare_oidc_file`) sont appelables. Aucun code du produit ne change | Revert propre et le reste compile. Mais plus rien ne lie alors les enveloppes restituées aux chargeurs : une divergence devient silencieuse. Non facultatif tant que le miroir existe |
| `a95bf67` | `src/provider.rs` ; `src/provider/selection.rs` | `@@ -308,0 +309,9 @@`, +9 −0 ; `@@ -203,0 +204,9 @@`, +9 −0 | Le choix pur du profil, sans ouverture de credential (Q1) | Voir le point 1 en tête : revert propre, mais deux appelants ne compilent plus ; retirable seulement avec la fonction de sélection, par édition manuelle |
| `78a5aa4` | `images/control.Dockerfile` | `@@ -12 +12 @@`, +1 −1 (`--bin ai-guard-diagnose` ajouté à la ligne de build) ; `@@ -23,0 +24 @@`, +1 −0 (un `COPY`) | Proposition : l'image de contrôle porte l'outil ; point d'entrée et commande par défaut inchangés | Retirable seul, sans conflit |
| `26166e4` | `README.md`, `README.fr.md` ; `docs/setup/README.md`, `docs/setup/README.fr.md` | `@@ -26,0 +27,2 @@`, +2 −0 chacun ; `@@ -20,0 +21 @@`, +1 −0 chacun | Proposition : une phrase après le renvoi Kubernetes des README racine, une ligne en fin du tableau des guides | Seul : conflit dans les quatre fichiers (une zone chacun), ses lignes ayant été reformulées ensuite. Avec les quatre suivants : propre |
| `68bea8c` | `README.md` | `@@ -27 +27 @@`, +1 −1 | Reformule la ligne ajoutée par `26166e4` (constat `contracts-7`) : l'outil n'ouvre aucun fichier que les fichiers inspectés nomment comme credential, et un fichier donné par option est ouvert comme ce que l'option dit | Revert propre. Garder `26166e4` sans ces quatre commits fait échouer `entry_points_state_what_is_opened_as_the_report_does`, qui lit les quatre index à l'exécution (par lecture du test) |
| `771dde4` | `README.fr.md` | `@@ -27 +27 @@`, +1 −1 | Idem, version française | Idem |
| `1fc07b5` | `docs/setup/README.md` | `@@ -21 +21 @@`, +1 −1 | Idem, tableau des guides | Idem |
| `4ecd6dc` | `docs/setup/README.fr.md` | `@@ -21 +21 @@`, +1 −1 | Idem, version française | Idem |

Seules les lignes que le lot a lui-même ajoutées dans ces index ont été
reformulées. La phrase voisine de `docs/setup/README.md`, « Each guide
contains prerequisites, configuration, startup, expected endpoints, … », ne
décrit pas le guide de diagnostic ; elle n'a pas été retouchée.

## 4. Ensembles retirables, recalculés au commit validé

Les ensembles viennent de `git log 2cc29ee…..469d625… -- <chemin>` et d'une
recherche des symboles dans les diffs. Chaque retrait est simulé par
`git merge-tree --write-tree --merge-base=<commit> <arbre> <commit>^`, avec un
magasin d'objets hors du dépôt : le magasin du dépôt n'est que lu, aucune
référence, aucun index ni aucun worktree n'est touché. Une « zone » est un
marqueur de conflit dans le fichier produit ; « modification/suppression »
désigne un fichier que le revert supprime alors qu'il a changé depuis.

**Aucun arbre obtenu par un retrait n'a été compilé ni testé : `NOT_RUN`.**

Cette section remplace les lignes R1 et R2 de la liste consolidée du journal
(19:10 UTC, établie à `755efe5`) : l'étape E-F6 a reformulé des phrases que ces
retraits touchent, et « `a881c60` propre » comme « les trois premiers propres »
ne sont plus vrais à `469d625`. Le contrôle indépendant de E-F6 l'a relevé ;
ce handoff l'a resimulé.

| Ensemble | Commit | Revert seul sur `469d625` |
|---|---|---|
| R1, sélection (Q1) | `a881c60` | conflit : une zone dans chaque guide de diagnostic |
| | `b7ad456` | conflit : une zone dans chaque guide |
| | `8b52ed9` | conflit : 7 zones dans chaque guide ; `report.providers-two-modes.json` 2, `.txt` 1 ; `src/diagnostics.rs` 1 ; `catalogue.rs` 3 ; `providers.rs` 2 ; les deux `report.providers-explicit-subscription.*` en modification/suppression |
| | `a95bf67` | propre |
| R2, vérification active (Q2) | `ed4c8be` | conflit : une zone dans chaque guide |
| | `756dc9b` | conflit : `tests/diagnose_support/recording_issuer.rs` en modification/suppression (modifié par `b1001d8`) |
| | `db73bfb` | propre |
| | `af2438d` | conflit : une zone dans `src/diagnostics/identity.rs` |
| | `e3ef698` | conflit : 4 zones dans chaque guide ; `report.prepared-v2.txt` 1 ; `src/bin/ai-guard-diagnose.rs` 1 ; `gateway_files.rs` 1 ; `tests/diagnose_cli.rs` 1 |
| R3, image et section du volume | `755efe5` | propre |
| | `f749265` | conflit : 3 zones dans chaque guide (une seule après le revert de `755efe5`) |
| | `78a5aa4` | propre, seul comme après `755efe5` |
| R4, index | `4ecd6dc`, `1fc07b5`, `771dde4`, `68bea8c` | propres, chacun |
| | `26166e4` | conflit : une zone dans chacun des quatre index |
| R5, parité | `13d099c` | propre |
| R6, module | `6adaf64` | propre |

- **R1.** Retrait manuel. Outre les quatre commits du tableau, les diffs de
  dix commits ultérieurs ajoutent ou modifient des lignes qui emploient les
  options, le bloc ou les codes de la sélection (recherche refaite commit par
  commit sur `a95bf67..469d625`) : `8dfa6b5`, `dad29a1`, `a2a6855`, `c69094f`,
  `d25454f`, `c73f538`, `f27321d`, `44b5c4b`, `4721538` et `69d2ce4`.
  `dad29a1` a reformulé la phrase des guides sur les options de sélection et
  le message de `provider.selection_resolved`, et régénéré les exemples à
  abonnement explicite ; `d25454f` a reformulé la phrase des guides sur le
  choix automatique et `provider.api_billing_notice`. `469d625` a reformulé
  deux phrases des guides : celle que `a881c60` avait écrite, et de nouveau
  celle du choix automatique. Ce que le retrait retire et ce qu'il rouvre :
  point 1 en tête.
- **R2.** Retrait manuel, dans l'ordre du tableau. `dad29a1` (aide) et
  `469d625` (guides) mentionnent aussi la vérification active.
- **R3.** `git revert 755efe5 f749265 78a5aa4`, dans cet ordre : un conflit
  par guide au second pas, au paragraphe qui précède la section. Résolution :
  garder le côté du revert, c'est-à-dire la seule phrase « An installation
  that lives in a container volume, such as the one of the Compose SSO DEMO,
  can be inspected only where the tool exists. The control image of this build
  does not contain it: that journey is not covered by this guide. », et
  supprimer la section. `78a5aa4` peut aussi partir seul.
- **R4.** `git revert 4ecd6dc 1fc07b5 771dde4 68bea8c 26166e4` : cinq pas
  propres, et les quatre index retrouvent exactement leur blob de la base
  (identité vérifiée). Tous ou aucun. Indépendant de R3.
- **R5 et R6.** Voir le tableau des raccords : textuellement retirables, mais
  le premier délie le miroir et le second retire le lot.

## 5. Critères d'acceptation du lot E

Les phrases viennent de la section « LOT E » du mandat. La correspondance
entre un critère et ses tests est établie par le nom des tests et par les
entrées d'étape du journal ; le rédacteur n'a pas relu chaque assertion à
cette étape. Les tests cités sont dans `tests/diagnose_cli.rs` et font tous
partie des 84 tests réussis de V1 et de V2. « Écart » désigne ce qui reste
ouvert ; les identifiants L, I, P et N renvoient à la section 9.

**« Q1 »** marque les six lignes dont un test entier ou un code cité part si
le raccord fournisseur est retiré (point 1 en tête). D'autres lignes citent un
test dont une partie seulement des cas emploie la sélection
(`json_contract_round_trips`, `provider_references_from_metadata`,
`a_credential_refused_for_its_path_is_reported`,
`hostile_environment_is_ignored`, `documented_examples_are_the_real_output`) :
ces tests sont à éditer et leur ligne garde sa preuve, à ceci près que deux
des neuf fichiers de rapports documentés partent avec le raccord.

| Critère du mandat | Preuve | Écart |
|---|---|---|
| Outil Rust, binaire auto-découvert, module, contrat JSON versionné, sortie texte ; pas de daemon ni d'autorité | `src/bin/ai-guard-diagnose.rs`, `src/diagnostics.rs` et 13 enfants ; `json_contract_round_trips`, `report_is_stable_and_bounded`, `text_facts_stay_within_the_report_width` | Version 1 non figée (point 4 en tête) ; aucun test ne décode le rapport d'un autre build (L30) |
| Réutiliser les résolveurs et validateurs, ne pas les reproduire à la main | Demandés au produit : `provider::choose_profile`, `Runtime::with_program`, les validateurs OIDC, `PrivateFile::open` et `Directory::open_existing`. Restitués et liés par un test de garde : enveloppes du registre et du fichier OIDC (8 tests de parité, raccord `13d099c`), règle des noms de base (`store_names_that_the_gateway_refuses_are_refused`), textes de refus du moteur (`engine_refusal_reasons_follow_the_product_texts`), correspondance des options de `run` (`the_selection_of_run_is_built_as_it_is_restated`) | Les restitutions sont exactes à la base seulement (I1, I7) ; le test de garde de `run` vérifie la présence de lignes, pas leur sens (L39) ; propositions P2 à P6. **Q1** : `provider::choose_profile` et le test de garde de `run` partent avec le raccord |
| Mode par défaut hors ligne et sans mutation | `no_network_by_default`, `no_mutation_everywhere` (manifeste de l'arbre inspecté avant et après), `unsafe_target_posture_is_reported_not_repaired` | — |
| Aucune création ni migration de base, aucune ouverture de SQLite qui crée des sidecars | Aucune base n'est ouverte : `data.offline_inspection_not_available` ; `the_stores_of_a_running_installation_and_of_a_refused_one` (outil lancé à côté d'un processus gateway vivant, puis contrôle de non-mutation) | N9 ; l'énoncé devient faux sur la branche d'intégration (I2) |
| Distinguer installation historique et installation préparée SetupV2 ; ne pas prétendre qu'une commande legacy lit le nouveau layout | `fresh_historical_directory_is_explained`, `prepared_v1_and_v2_are_explained`, `legacy_command_does_not_read_prepared_layout`, `init_over_prepared_installation_is_mixed`, `setup_states`, `relocated_installation`, `layout_edge_states`, `gateway_files_without_setup_records_name_the_file_options` | I2 : `verify-audit` lit le layout préparé sur la branche d'intégration |
| Dire `unknown` ou `not_checked` quand rien n'a été observé | Statuts `unknown`, `not_checked`, `not_available` et leur section des guides ; `a_policy_refused_at_the_assumed_event_bound_is_not_an_observed_error`, `options_given_without_effect_are_said_so`, `an_absent_registry_is_told_from_an_unreadable_one` | L43 |
| Topologie | `first_run_states`, `prepared_v1_and_v2_are_explained`, limites `topology.kubernetes_launch_not_available` et `topology.gateway_arguments_not_persisted` | I6 : une option du gateway de plus sur la branche d'intégration |
| Identité locale et SSO | `prepared_v1_and_v2_are_explained`, `the_demonstration_transport_is_a_warning`, `gateway_block_refusals_name_their_fields`, `a_refused_oidc_file_is_reported_with_its_cause` | `identity.arbitrary_oidc_provider_not_qualified` ; L15 ; aucun fournisseur d'identité réel (N5) |
| Correspondance exacte des tenants et des plans | `tenant_set_and_plan_mismatch`, `prepared_v1_with_two_tenants_needs_a_plan`, `a_plan_mismatch_names_the_stored_plan`, `duplicate_identifiers_are_counted_once` | Le plan stocké dans l'audit n'est pas lu : `tenants.durable_plan_comparison_not_available` ; L36 |
| Références de credentials | `provider_references_from_metadata`, `registry_references_follow_the_gateway_reader`, `historical_roles_and_private_rule`, `a_credential_refused_for_its_path_is_reported`, `no_text_says_that_a_consumer_accepts_a_file_that_was_not_read` | L10, L14, L45, L46 |
| Client et mode, explicite ou automatique | `explicit_subscription_is_preserved`, `auto_resolution_and_billing_notice`, `selection_refusals_are_classified`, `the_model_of_a_profile_is_resolved_as_the_product_resolves_it` | **Q1** : dépend du raccord, les quatre tests cités partent avec lui (point 1 en tête) ; L18, L19 |
| Compatibilité connue et limites non qualifiées | Section `limits` de tout rapport (neuf limites du build, L27) ; section « Limits and non-qualified areas » des guides | — |
| Codex CLI et Claude Code, abonnement et API, inférence interne : axes indépendants ; SSO facultatif sans couplage IA | `axes_are_independent_under_faults` ; limites `inference.internal_profile_not_available`, `inference.local_model_backend_not_available`, `inference.model_tls_outputs_not_available` | Le profil d'inférence interne n'existe pas dans le produit à la base : l'outil le dit, il ne l'inspecte pas |
| Un abonnement demandé ne bascule jamais vers une API payante | `refused_subscription_never_becomes_api` (la moitié discriminante), `a_refused_subscription_states_the_automatic_rule_of_the_product`, `explicit_subscription_is_preserved` | **Q1** : dépend du raccord. Les trois tests cités partent avec lui, ainsi que les codes `provider.subscription_never_falls_back_to_api` et `provider.api_billing_notice` : sans le raccord ce critère n'a plus aucune preuve exécutée (point 1 en tête). Le dernier test ne discrimine pas seul : la règle automatique préfère déjà l'abonnement (L37) |
| Distinguer client absent, connexion requise, version non qualifiée | `provider.login_required` (produit par un test) ; `provider.client_presence_not_available`, `provider.client_version_not_available` et `provider.qualified_versions_not_available` | **Écart** : l'outil n'exécute aucun client et ne cherche rien dans `PATH`. Présence et version du client sont dites `not_available` ; elles ne sont pas détectées |
| Distinguer credential indisponible, CA ou TLS incorrects, configuration ambiguë ou incompatible | `provider.credential_missing`, `provider.credential_unsafe` ; `ca_bundles`, `explicit_trust_inputs`, `a_refused_bundle_is_told_from_an_absent_one`, `the_local_http_permission_with_a_bundle_is_named_as_the_cause` ; `provider_configuration_refusals`, `provider.selection_ambiguous`, `provider.selection_no_profile` | Aucune date de validité : un certificat expiré n'est pas détecté (L23) ; L22. **Q1** : les deux codes `provider.selection_*` cités partent avec le raccord, et `provider.credential_missing` comme `provider.credential_unsafe` ne sont alors des erreurs que pour un fichier à un seul profil |
| Distinguer moteur non vérifié et données non inspectées | `engine_program_is_approved_not_executed`, `engine_program_refusals`, `each_engine_refusal_names_its_cause`, `engine.state_not_checked` ; `data.offline_inspection_not_available` | Neuf des seize textes de `engine.program_refused` ne sont produits par aucun test (N4) |
| Aucun accès aux caches OAuth ni au keychain | Par lecture : aucun accès à l'environnement, aucun lancement de processus, aucun `CODEX_HOME` ni keychain dans `src/diagnostics.rs`, `src/diagnostics/**` et `src/bin/ai-guard-diagnose.rs` (recherche refaite à V ; contrôles de E-S6, E-F5 et E-F6). Par exécution : `hostile_environment_is_ignored` pose `CODEX_HOME` et `CLAUDE_CODE_OAUTH_TOKEN` parmi quinze variables et compare les rapports octet par octet ; `selection_never_reads_credentials`, `default_mode_never_reads_credentials` | Le test couvre les quinze variables qu'il pose ; le reste repose sur la lecture (L38). **Q1** : `selection_never_reads_credentials` part avec le raccord, et quatre des six passes de `hostile_environment_is_ignored` passent `--client` et `--auth-mode` |
| Ne pas inventer une version officiellement supportée | `provider.qualified_versions_not_available` : aucune version n'est annoncée | — |
| Vérifications actives derrière un flag explicite, adaptateurs sûrs, non exécutées sans admission séparée | `no_network_by_default`, `active_oidc_discovery_loopback` ; le client est celui du produit (`src/oidc/transport.rs`) | Admission du coordinateur seul (point 2 en tête) ; N5 |
| Aucun programme pris dans `PATH` | `hostile_environment_is_ignored` : six passes, dont deux où l'approbation regarde les emplacements fixes pendant que `PATH` désigne des programmes nommés `docker` et `podman` ; contrôle par mutation | Dans l'image de QA les emplacements fixes ne contiennent aucun moteur : l'approbation d'un programme trouvé là n'a pas été exécutée (N4) |
| Aucun environnement complet en sortie | Même test (canari absent des sorties) ; aucun accès à l'environnement dans le code du lot | L38 |
| Masquer avant troncature ; ne pas recopier une erreur contenant un secret, une clé JSON sensible ou une URL avec credential | `long_values_are_masked_before_they_are_clipped` (avec contrôle par mutation), `urls_and_values_are_redacted`, `identifiers_that_look_like_credentials`, `control_characters_cannot_forge_lines`, `invisible_characters_are_shown_escaped`, `selection_values_cannot_forge_or_leak` | Trois refus de sortie ne sont produits par aucun test (N12) ; plages Unicode non revérifiées contre la base officielle (N24) ; L28. **Q1** : `selection_values_cannot_forge_or_leak` part avec le raccord ; dans `long_values_are_masked_before_they_are_clipped`, la coupe à 512 octets est portée par `--client-profile` et le cas « masqué puis coupé », celui du contrôle par mutation, par `--model` : ces deux cas sont à réécrire sur une autre entrée |
| Chaque résultat donne son origine, sa portée et une action utile | `catalogue_is_safe_and_actionable` ; les tests d'action de E-F2 | L11, L12, L13, L15 |
| Tests du vrai binaire sur installations synthétiques neuves et incohérentes ; aucune mutation de l'arbre inspecté | 84 tests de `tests/diagnose_cli.rs` ; parseurs et moteurs réels, gateway réel en oracle, aucun faux statut d'API | Une seule plate-forme : Linux ARM64 en conteneur (N3) |
| Quick Start et exemples EN/FR, priorités de configuration, états inconnus | Guides EN/FR de 1062 lignes, mêmes titres, mêmes blocs ; `documented_examples_are_the_real_output` (les rapports documentés sont la sortie réelle du binaire), `guides_follow_the_binary` | Le parcours en conteneur et `cargo build --release` ne sont pas exécutés (N1, N2) ; rendu Markdown et relecture du français par un second lecteur non faits (N23) |
| Ni fournisseur d'identité réel ni appel IA | Aucun n'a été contacté | — |

## 6. Validation finale sur le commit validé

### Profil d'exécution

Conteneur `aig-claude-qa-operator-diagnostics-20261110`, harnais `qa.sh`
révision 3 (SHA-256
`5d32284abcd7815ee71a54debf55515228ce82caf1e698486326cfdddcf3cfde`, inchangé
pendant V), profil `QA_HARNESS.md` (187 lignes, SHA-256
`f2847593fc502647a60bf32b7f72fe4cefc6da6cd3a3c8f719f88e6c006b74de`).

- **Observé par le harnais** (journal `vfinal-platform`) : Linux
  `7.0.14-linuxkit` `aarch64`, Debian 12.15 ; `rustc 1.99.1 (58cf8f9c3
  2026-10-13)`, hôte `aarch64-unknown-linux-gnu`, LLVM 23.1.1 ; `cargo 1.99.1
  (5f94df478 2026-08-27)` ; UID 1000, `CapEff 0`, `NoNewPrivs 1`, `Seccomp 2` ;
  `memory.max` 6442450944 (6 Gio), `memory.swap.max` 0, `pids.max` 256,
  `cpu.max` 300000/100000 (3 CPU). Par `qa.sh status` : réseau `none`, racine
  en lecture seule.
- **Lu dans le profil, non réobservé** : image locale épinglée
  `sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`,
  `--cap-drop ALL`, `no-new-privileges`, `--init`, core 0, fsize 512 Mio,
  nofile 1024, `/tmp` tmpfs 16 Mio `noexec`, environnement `env -i` à liste
  fermée, Cargo `--frozen --offline`, la source entre par flux tar et aucun
  checkout n'est monté. Clippy 1.99.1 vient d'une archive publique vérifiée
  par empreinte, installée dans le scratch privé.
- **Source de chaque lancement** : `head=469d6259c305c415695968334bf3a1eb5b355a5c
  dirty_entries=0`, `frozen=commit:469d625…` (par `--rev`, `git archive`) pour
  V1 à V5, `frozen=worktree:469d625…` pour Clippy, dont la commande n'a pas
  d'option `--rev` (worktree propre, même manifeste) ; `frozen_dirty=0`, 2411
  fichiers, manifeste `b8f09264…6bbf` (valeur complète en section 1). Dans les
  neuf journaux du harnais, `source_post_manifest` est égal au manifeste du
  gel et `oom_kill` vaut 2 avant comme après.
- **Recompilation complète.** V1 est lancée avec `QA_REDATE_ALL=1` : les 2411
  fichiers sont datés de l'instant du gel (`redated=2411 kept_dates=0`) et la
  ligne `Compiling agentsandbox v0.1.0 (/workspace/source)` est présente. Les
  binaires du produit qu'emploient V1 et V2 sont donc construits depuis
  l'arbre du commit figé. Les lancements suivants gèlent le même contenu
  (`redated=0 kept_dates=2411`).
- `memory_peak=6442450944` figure dans chaque journal : c'est le compteur
  cumulé du conteneur, déjà à cette valeur avant V1. Il ne dit rien du pic
  d'un lancement.

### Exécutions

Chaque commande suit `qa.sh run operator-diagnostics <délai> <label> --rev
469d6259c305c415695968334bf3a1eb5b355a5c --`, sauf mention. Toutes les
exécutions ont été vérifiées pour ce handoff dans les journaux : en-tête,
lignes `test result:`, RC et empreinte.

| Sél. | Label | Commande | RC | Durée |
|---|---|---|---|---|
| V1 | `vfinal-v1-cli-redate` | `QA_REDATE_ALL=1` devant `qa.sh`, délai 3000 ; `cargo test -j 1 --frozen --offline --test diagnose_cli -- --nocapture --test-threads=1` | 0 | 160 s |
| V2 | `vfinal-v2-cli-parallel` | délai 500 ; `cargo test -j 1 --frozen --offline --test diagnose_cli -- --nocapture` (mode parallèle par défaut) | 0 | 33 s |
| V3 | `vfinal-v3-parity` | délai 550 ; `cargo test -j 1 --frozen --offline --bin agentsandbox diagnostics_parity` | 0 | 6 s |
| V4a | `vfinal-v4a-lib-diagnostics` | délai 3000 ; `cargo test -j 1 --frozen --offline --lib -- --nocapture the_root_of_the_file_system_is_no_display_root the_reader_walks_the_directories_before_it_can_open_a_file the_reader_is_asked_about_directories_and_opens_nothing a_registry_that_was_not_read_is_classified_from_its_metadata` | 0 | 89 s |
| V4b | `vfinal-v4b-lib-provider` | délai 3000 ; `cargo test -j 1 --frozen --offline --lib provider -- --nocapture --test-threads=1` | 0 | 1 s |
| V4c | `vfinal-v4c-provider-config` | délai 550 ; `cargo test -j 1 --frozen --offline --test provider_config -- --nocapture --test-threads=1` | 0 | 4 s |
| V5 | `vfinal-v5-check-all` | délai 3000 ; `cargo check -j 1 --frozen --offline --all-targets` | 0 | 54 s |
| V6 | `vfinal-v6-clippy` | `qa.sh clippy operator-diagnostics 3000 vfinal-v6-clippy -- -j 1`, soit `cargo clippy --frozen --offline --all-targets --message-format=short -j 1` | 0 | 55 s |
| V7 | rustfmt natif | `rustfmt --check --edition 2024` sur les 22 fichiers Rust possédés, hors harnais (analyse statique) | 0 | — |
| — | `vfinal-platform` | `--no-sync` ; relevé de la toolchain et des limites du conteneur, aucune sélection | 0 | 0 s |

| Sél. | Comptes exacts | Journal privé | SHA-256 |
|---|---|---|---|
| V1 | `tests/diagnose_cli.rs` : 84 sélectionnés, 84 réussis, 0 échoué, 0 ignoré, 0 filtré ; 0 ligne `ORACLE NOT_RUN` ; 39 lignes `OBSERVED gateway refusal` ; build 1 min 13 s | `20261111T204622Z-vfinal-v1-cli-redate.log` | `d4e21ded6df69dfdca0b5ede07284d672bba08134e1fb67b4b1db9fff1728e22` |
| V2 | 84 sélectionnés, 84 réussis, 0 échoué, 0 ignoré, 0 filtré ; 0 ligne `ORACLE NOT_RUN` ; aucune ligne `Compiling`, attendu : même binaire de test que V1 | `20261111T204937Z-vfinal-v2-cli-parallel.log` | `0393f52b9d198152a119927f19291cd2e82b2627a2a38a9af2d1788a8facdd7f` |
| V3 | Tests unitaires du binaire `agentsandbox` : 8 sélectionnés, 8 réussis, 0 échoué, 0 ignoré, 39 filtrés ; les 8 sont `gateway_config::diagnostics_parity_tests::*` | `20261111T205034Z-vfinal-v3-parity.log` | `f117056248fe5a283d32e8fb6264f00583a3319b72090afa5f3e1048ea8d058c` |
| V4a | Tests unitaires de la bibliothèque : 4 sélectionnés, 4 réussis, 0 échoué, 0 ignoré, 811 filtrés ; ce sont tous les `#[test]` de `src/diagnostics.rs` et `src/diagnostics/` | `20261111T205053Z-vfinal-v4a-lib-diagnostics.log` | `222c706f1f9a33accd1b6c89b79080b0513f5610923d7321ccc1cfa2ca789164` |
| V4b | Filtre `provider` : 9 sélectionnés, 8 réussis, 0 échoué, **1 ignoré** (`provider::bootstrap::tests::bounded_probe_fixture`, `#[ignore]` du produit dans un fichier que le lot ne modifie pas), 806 filtrés | `20261111T205238Z-vfinal-v4b-lib-provider.log` | `193460a7bd3f064fb2ff3eb538d754d07b5d8c1af9f14840aad705962a769fc4` |
| V4c | `tests/provider_config.rs` : 10 sélectionnés, 10 réussis, 0 échoué, 0 ignoré, 0 filtré | `20261111T205256Z-vfinal-v4c-provider-config.log` | `deddbe32856a226ab48dd59509efe0852f01cd66c67dcbe44e8a5c0d346efb35` |
| V5 | Aucun test ; 0 erreur ; ligne `Checking agentsandbox` présente ; 91 emplacements d'avertissement rustc, 0 dans les 26 fichiers Rust ajoutés ou modifiés par le lot | `20261111T205316Z-vfinal-v5-check-all.log` | `276bdd99556a946b21c1cb78bda279742eb8b74a963ada4e884fc82eee85df6e` |
| V6 | Aucun test ; 0 erreur ; 169 lignes de diagnostic localisées, comme la référence de la base ; ensemble identique une fois lignes et colonnes retirées ; 0 dans les 22 fichiers Rust possédés ; 1 dans un fichier partagé (voir ci-dessous) | `20261111T205435Z-vfinal-v6-clippy.log` | `dae75ba6301ed55d57c40d522c7dc379d33986f581b67cee4074c0cca271c404` |
| V7 | 22 fichiers propres : 21 par chemin (RC 0, sortie vide), `tests/diagnose_cli.rs` par l'entrée standard (sortie vide, puis `cmp` RC 0) | aucun journal : contrôle natif, sorties vides | — |
| — | Aucun test | `20261111T205724Z-vfinal-platform.log` | `fe076dbb0e2691156141210bba9d67cb1c5e5b299c78530c67c88c6cc5f27bf7` |

Référence Clippy de la base `2cc29ee`, produite par le coordinateur :
`20261110T195910Z-baseline-clippy-all-targets-j1.log`, RC 0, 169 lignes,
SHA-256 `379e5e9351bdcfc91dfad42928ec17fa528c6723967767f97caf030ea1f45e64`.

Les journaux restent privés (section 11). L'index privé du handoff recalcule
l'empreinte de chacun.

### Lecture de ces résultats

- **Oracles qui ne peuvent pas tourner.** Cinq oracles font échouer leur test
  quand l'environnement ne peut pas les exécuter : `never-read` (un montage
  qui n'enregistre pas les dates d'accès), `account-home`,
  `engine-refusal-with-a-system-error`,
  `setup-resume-on-unpublished-preparation` et
  `gateway-refusal-of-an-unreadable-key`. Pour l'accepter le temps d'un
  lancement, la variable `AI_GUARD_TEST_ORACLE_NOT_RUN` les nomme, séparés par
  des virgules : chacun imprime alors `ORACLE NOT_RUN <nom>` sur stderr, et
  c'est un `NOT_RUN` à rapporter, jamais un succès. **L'exécution validée ne
  pose pas cette variable** (l'environnement du harnais est une liste fermée
  qui ne la contient pas) **et n'a aucune ligne de ce genre : aucun oracle n'a
  été sauté.**
- **V4, trois lancements.** `src/provider.rs` et `src/provider/selection.rs`
  n'ont aucun test unitaire : le raccord du choix est couvert par le filtre
  `provider` de la bibliothèque (V4b : trois tests de `provider::bootstrap`,
  dont un ignoré, et six tests d'autres modules dont le nom contient
  « provider ») et par la cible d'intégration `tests/provider_config.rs`
  (V4c), où vivent les tests existants de la règle du choix.
- **V6, critère Clippy.** Le critère du lot est « zéro avertissement dans les
  fichiers ajoutés ou modifiés par le lot » ; le Clippy strict du dépôt
  (`-D warnings`) échoue déjà à la base et n'est pas exécuté. Une ligne est
  localisée dans un fichier partagé que le lot modifie :
  `src/gateway_config.rs:198:34: warning: this OR pattern can be rewritten
  using a range`. C'est le diagnostic de la base : même ligne 198, même texte,
  ligne identique aux deux commits ; le hunk du lot est aux lignes 423 à 426.
  Lu à la lettre, le critère compte donc 1 ; attribuable au lot, 0. V6 est
  classée PASS sur cette seconde lecture, celle des étapes précédentes.
  La décision du coordinateur du 11 novembre à 22:20 est consignée en
  section 8 ; Root ratifie cette lecture à l’intégration du 12 novembre.
  Elle ne constitue jamais un PASS Clippy strict.
- **V7, rustfmt.** `tests/diagnose_cli.rs` déclare
  `#[path = "support/oidc_fixture.rs"] mod controlled;`, fichier non possédé.
  Par chemin, rustfmt suit cette déclaration ; il a donc été contrôlé par
  l'entrée standard, où aucun module n'est suivi. Sur l'entrée standard
  `--check` imprime le diff mais sort avec 0 (calibré) : l'oracle est « sortie
  vide » plus une comparaison par `cmp`. Les étapes précédentes disent avoir
  lancé `rustfmt --check` sur ce fichier par chemin, ce qui contrôle aussi, en
  lecture seule, le fichier non possédé. Les quatre fichiers Rust partagés
  n'ont pas été passés à rustfmt. rustfmt sous Linux est absent de l'image.
- **Aucune relance, aucun code 75, aucune attente du créneau lourd, aucun
  `rc=124` ni `rc=137`.** Aucune durée ne dépasse sa phase de compilation et
  de test : le journal d'alimentation de l'hôte n'a pas été consulté.
- **Exécutions antérieures.** Chaque étape a ses propres exécutions, RED avant
  correction, GREEN après, contrôles par mutation jamais committés. Elles sont
  citées en section 7 par leur label, comme historique : seules les
  exécutions sur `469d625` qualifient `469d625`.

### PASS

| Élément | Preuve |
|---|---|
| Arbre : branche, propreté, base ancêtre, historique linéaire, zones interdites inchangées, `Cargo.toml` et `Cargo.lock` inchangés, fichiers partagés touchés par des commits `wire:` seulement | section 1 |
| `tests/diagnose_cli.rs`, séquentiel après recompilation complète : 84 / 84 | V1 |
| `tests/diagnose_cli.rs`, mode parallèle : 84 / 84, une exécution | V2 |
| Tests de parité avec les chargeurs du gateway : 8 / 8 | V3 |
| Tests unitaires de `src/diagnostics/**` : 4 / 4 | V4a |
| Filtre `provider` de la bibliothèque : 8 réussis sur 8 exécutés | V4b |
| `tests/provider_config.rs` (suite existante) : 10 / 10 | V4c |
| `cargo check --frozen --offline --all-targets` : RC 0, crate vérifiée | V5 |
| Clippy, critère du lot : aucun diagnostic nouveau, ensemble égal à la base | V6, avec la réserve de lecture ci-dessus |
| `rustfmt --check --edition 2024` natif : 22 fichiers Rust possédés sur 22 | V7 |

### FAIL

Aucun.

### BLOCKED

Aucun. Aucun appel d'outil n'a été refusé pendant la validation finale ni
pendant la rédaction de ce handoff.

### NOT_RUN

Aucune de ces lignes n'est un succès. La liste complète, avec l'origine de
chaque ligne, est en section 9 (N1 à N29).

| Élément | Raison |
|---|---|
| `provider::bootstrap::tests::bounded_probe_fixture` | ignoré par défaut dans le produit, antérieur au lot |
| Les 39 autres tests unitaires du binaire `agentsandbox`, les 811 (V4a) ou 806 (V4b) autres tests de la bibliothèque | filtrés |
| Suite complète : `cargo test` sans sélecteur, `cargo test --lib` sans filtre, `CARGO_INCREMENTAL=0 cargo test --locked` | non exécutés, par économie ; le voisinage est couvert par `cargo check --all-targets` |
| `cargo build --release --locked` | seul le profil debug est construit en QA |
| Construction de `images/control.Dockerfile` et parcours en conteneur des guides | aucun moteur n'est admis dans ce lot |
| macOS ; Linux x86_64 | toutes les exécutions sont Linux ARM64 dans le conteneur de QA |
| Moteur réel : Docker, Podman, Kubernetes | hors du profil admis |
| Fournisseur d'identité réel ; vérification active contre autre chose que les émetteurs factices en loopback | hors du profil admis |
| Fournisseur IA, modèle, Codex CLI, Claude Code | non requis par le lot |
| rustfmt sous Linux, `cargo fmt --all -- --check` ; rustfmt des quatre fichiers Rust partagés | composant absent de l'image ; fichiers partagés exclus par le profil |
| Clippy strict du dépôt (`-D warnings`) | échoue déjà à la base |
| Commits intermédiaires de la branche | V ne valide que `469d625` |
| Arbres obtenus par un retrait (R1 à R6) et arbre fusionné avec la branche d'intégration | simulés textuellement, jamais compilés |
| Tests du front | `NOT_APPLICABLE` : le lot ne touche aucun fichier sous `front/` |
| Luna | indisponible dans cette session |
| Contrôle indépendant de la dernière version de ce handoff | un premier contrôle a refusé `d038341` pour un point, corrigé par `234e457` ; un second contrôle a accepté `234e457` sans point bloquant (section 7). Le commit suivant, qui corrige les formulations relevées par ce second contrôle, est écrit par le coordinateur et n'a pas été contrôlé indépendamment |
| Relecture de l'identifiant de l'image dans le moteur ; journal d'alimentation de l'hôte (`pmset -g log`) | non faits pendant V |

## 7. Revue indépendante

### Revue de la conception, avant le code

Trois critiques indépendantes de la conception : 46 constats, 43 acceptés et 3
acceptés en partie, avec motif. Changements structurants de la révision 2 :
l'outil n'ouvre plus aucun credential dans aucun mode ; la posture est jugée
selon la règle du consommateur réel ; `not_checked` remplace `not_configured`
pour un axe non observé. L'implémentation s'en écarte là où le code l'exigeait ;
le journal consigne chaque écart.

### Contrôles indépendants par étape

Chaque contrôle est en lecture seule : il ne compile et n'exécute rien, il
relit Git, les fichiers et les journaux de QA.

| Étape | Commit jugé | Verdict |
|---|---|---|
| E-S1 | `32b27cd` | passé |
| E-S2 | `fa216a9` | passé |
| E-S3 | `1b3ee2a` | passé, 0 point bloquant ; ses notes sont traitées par E-S4b |
| E-S4 | `316f806` | **`NOT_RUN`** : le contrôle n'a pas tourné (interruption d'outillage). Ses vérifications ont été reprises par le contrôle de E-S4b (« CHECK 6 (E-S4) PASS ») et par la revue |
| E-S4b | `ea6cc4a` | passé |
| E-S5 | `b7ad456` | passé |
| E-S6 | `26166e4` | passé |
| E-F1 | `d25454f` | **refusé** : un point bloquant (`secret-hygiene-5`), réparé par `de60a53` |
| E-F2 | `52af9f6` | passé |
| E-F3 | `a457be9` | **refusé** : deux points bloquants (`tests-9` ; formulation de `tests-7` et `correctness-parity-6`), réparés par `69d2ce4` et `433eba9` |
| E-F4 | `755efe5` | passé |
| E-F5 | `433eba9` | passé |
| E-F6 | `469d625` | passé, aucun point bloquant |
| Handoff (ce document) | `d038341`, deux commits après V | **refusé** : un point bloquant. Le point 1 en tête ne disait pas que le retrait du raccord fournisseur retire aussi `provider.subscription_never_falls_back_to_api`, `provider.api_billing_notice` et toute la preuve exécutée du critère « Un abonnement demandé ne bascule jamais vers une API payante ». `234e457` le corrige, par un commit de plus qui ne touche que ce fichier |
| Handoff, second contrôle | `234e457`, trois commits après V | passé, aucun point bloquant. Ses remarques de formulation sont traitées par le commit suivant, écrit par le coordinateur, qui ne touche que ce fichier et **n'a pas été contrôlé indépendamment** (fin de section 9) |

Les contrôles de E-F1 à E-F4 ont tourné pendant l'étape suivante : leurs notes
non bloquantes n'avaient été reçues par aucune étape, d'où l'étape E-F6. Le
contrôle de E-F6 a tourné pendant la validation finale, sur le commit validé.
Les verdicts et les notes de tous les contrôles, de E-S1 à E-F6 et des deux
contrôles du handoff, sont dans le fichier privé `GATE_NOTES.md` ; ceux de
E-F6 et du handoff y ont été ajoutés par le coordinateur le 11 novembre à
22:14 UTC, après la rédaction des deux premières versions de ce document. Les
notes du contrôle de E-F6 et des deux contrôles du handoff qu'aucun commit ne
ferme sont reprises en fin de section 9.

### Revue du lot

Revue indépendante sur `26166e4` (39 commits), le 11 novembre : cinq angles de
lecture (hygiène des secrets, exactitude et parité, contrats, tests, guides
opérateur), puis vérification contradictoire par domaine. **53 constats : 50
confirmés, 1 incertain, 2 doublons, 0 réfuté, 0 non vérifié.** Deux sont élevés
après réévaluation (`secret-hygiene-3`, `operator-guides-3`) ; 26 étaient
marqués « à corriger avant handoff ». Pièces privées :
`REVIEW_RESULT_20261111.json`, `REVIEW_FINDINGS_20261111.md`,
`REVIEW_DIGEST_20261111.md`.

Disposition finale de chaque constat. Les labels `e-f…` sont ceux de journaux
privés de QA des étapes, cités comme historique. « RED » : le test échouait à
son assertion prévue avant la correction.

| Constat | Gravité vérifiée | Disposition | Commits | Preuve, limite ou renvoi |
|---|---|---|---|---|
| `secret-hygiene-1` | moyenne | corrigé | `477d07f` | `a_registry_read_to_classify_a_directory_is_listed_as_read` ; RED `e-f1-red-cli`, GREEN `e-f1-b-green-cli` |
| `secret-hygiene-2` | moyenne | corrigé | `8dfa6b5` | `the_entry_of_a_file_read_before_it_was_named_says_so` ; RED `e-f1-red-cli` et `e-f1-c-red-cli` |
| `secret-hygiene-3` | **élevée** | corrigé, avec limites | `e33cca9`, `65a3bd9` ; résidu `1b6b1c3` | `files_recorded_by_a_staging_directory_are_never_opened`, `the_target_of_a_reference_that_is_a_link_is_never_opened` ; RED par l'oracle des dates d'accès ; L2, L5, L8, L51 |
| `secret-hygiene-4` | faible | limite consignée | `0e00e5a` (guides) | L6 ; aucune course exercée (N13) ; P14 |
| `secret-hygiene-5` | faible | corrigé | `bb89f6a`, `d25454f`, `de60a53` | `the_root_of_the_file_system_is_no_display_root`, binaire et unitaire ; RED `e-f1-red-cli` puis `e-f5-red-cli` |
| `secret-hygiene-6` | faible | corrigé | `d1a3673` ; phrase `469d625` | `invisible_characters_are_shown_escaped`, treize classes ; N24 |
| `secret-hygiene-7` | faible, incertain | limite consignée | `0e00e5a` (guides) | L7 ; test décisif `NOT_RUN` (N7) ; P15 |
| `secret-hygiene-8` | faible | documenté | `ed4c8be`, `469d625` | L20 ; P8 |
| `correctness-parity-1` | moyenne | corrigé | `c1fcf30` | `a_published_installation_without_its_audit_is_an_error`, gateway réel ; N10 |
| `correctness-parity-2` | moyenne | corrigé | `f5c6318` ; garde `1c45ab4` | `a_policy_refused_at_the_assumed_event_bound_is_not_an_observed_error` ; L43 |
| `correctness-parity-3` | moyenne | corrigé | `4eb8061` | assertion de comptage dans `report_is_stable_and_bounded` |
| `correctness-parity-4` | doublon de `tests-1` et `tests-2` | test ajouté | `07b0340`, `b9719ba` | mêmes tests et journaux ; N8 |
| `correctness-parity-5` | faible | corrigé | `cb2fd5a` | `store_names_that_the_gateway_refuses_are_refused`, gateway réel ; L12 |
| `correctness-parity-6` | faible | corrigé, formulation rectifiée | `8f4b9db`, `a457be9`, `433eba9` | tests de parité par `load_oidc` ; L16, L17 ; N11 |
| `correctness-parity-7` | faible | corrigé | `cda5e97` ; `c7fd54c`, `16eda98` | `gateway_files_without_setup_records_name_the_file_options` ; L50 |
| `correctness-parity-8` | faible | limite consignée | `0e00e5a` (guides) | L9 ; P8 |
| `correctness-parity-9` | faible | corrigé | `7ad4d5b` | `options_given_without_effect_are_said_so` |
| `correctness-parity-10` | faible | corrigé ; partie facultative non prise | `52af9f6`, `a57855e` | `no_text_says_that_a_consumer_accepts_a_file_that_was_not_read` ; L14, L46 |
| `correctness-parity-11` | doublon de `contracts-8` | corrigé | `a2a6855` | même test |
| `correctness-parity-12` | faible | documenté, décision conservée | `a881c60` | L18 ; P9 |
| `correctness-parity-13` | faible | corrigé | `0287159` | `duplicate_identifiers_are_counted_once` |
| `contracts-1` | moyenne | travail d'intégration | aucun | I1 ; P2, P16 |
| `contracts-2` | moyenne | décision pour Codex | aucun | point 1 en tête ; section 4 (R1) ; P3 |
| `contracts-3` | faible | documenté | `a881c60` | L18 ; P8 |
| `contracts-4` | moyenne | travail d'intégration | aucun | I2 |
| `contracts-5` | faible | travail d'intégration | aucun | I3 ; point 3 en tête |
| `contracts-6` | faible | documenté | `0e00e5a` (guides) | L30 ; P7, P10 ; point 4 en tête |
| `contracts-7` | moyenne | corrigé | `68bea8c`, `771dde4`, `1fc07b5`, `4ecd6dc`, `dad29a1` | `entry_points_state_what_is_opened_as_the_report_does` ; I8 |
| `contracts-8` | moyenne | corrigé | `a2a6855`, `d25454f`, `469d625` | `a_refused_subscription_states_the_automatic_rule_of_the_product`, oracle `provider::prepare` |
| `contracts-9` | faible | traité par ce handoff | aucun | notes des contrôles : cette section ; contrôle de E-S4 seul `NOT_RUN` (N25) ; emplacement des notes : section 11 |
| `tests-1` | moyenne | test ajouté | `07b0340` | `a_refused_registry_is_reported_with_its_cause`, `a_refused_oidc_file_is_reported_with_its_cause` ; mutation `e-f3-mutation-cli` pour le test du registre seulement : le test OIDC n'a pas de contrôle par mutation propre, seulement le RED de message `e-f3-red-policy-message` ; N15 |
| `tests-2` | moyenne | test ajouté | `b9719ba` | `the_stores_of_a_running_installation_and_of_a_refused_one` ; N8, N9 |
| `tests-3` | moyenne | corrigé | `60673f2` | les oracles qui ne peuvent pas tourner font échouer le test (section 6) ; mutation du harnais `e-f1-mutation-oracle-fails-closed` |
| `tests-4` | moyenne | test ajouté | `44b5c4b` | `the_model_of_a_profile_is_resolved_as_the_product_resolves_it` ; mutation |
| `tests-5` | moyenne | test ajouté | `c69094f` | `long_values_are_masked_before_they_are_clipped` ; mutation ; N12 |
| `tests-6` | faible | corrigé | `09c78a1`, `63867ee` | trois oracles resserrés ; deux sans contrôle par mutation (N22) |
| `tests-7` | faible | corrigé, formulation rectifiée | `46f4582`, `433eba9` | chaque oracle de refus exige un fragment du texte du gateway ; L36 |
| `tests-8` | faible | corrigé | `db73bfb` | le port est tenu par un socket lié qui n'écoute pas ; N21 |
| `tests-9` | faible | corrigé | `f27321d`, `69d2ce4` | `hostile_environment_is_ignored`, six passes ; mutation `e-f5-mutation-engine-path` ; L38 |
| `tests-10` | faible | corrigé | `756dc9b`, `b1001d8` | émetteur enregistreur du lot ; mutation ; N5 |
| `tests-11` | faible | test ajouté | `4721538` | `the_selection_of_run_is_built_as_it_is_restated` ; L39 ; P6 |
| `operator-guides-1` | moyenne | corrigé | `bb0320e`, `a57855e` | `a_plan_mismatch_names_the_stored_plan`, gateway réel |
| `operator-guides-2` | moyenne | corrigé | `223a0d7` | `an_absent_registry_is_told_from_an_unreadable_one` ; N8 |
| `operator-guides-3` | **élevée** | corrigé, sans raccord dans `src/control_fs.rs` | `c73f538`, `a57855e` | `a_credential_refused_for_its_path_is_reported` et deux tests unitaires ; L10, L45 ; P4, P19 |
| `operator-guides-4` | moyenne | corrigé | `27c1fa4` | `each_engine_refusal_names_its_cause` et le test de garde des textes ; N4 |
| `operator-guides-5` | moyenne | corrigé, avec limite | `75ce006`, `1c45ab4` | `gateway_block_refusals_name_their_fields` ; L15 ; P5 |
| `operator-guides-6` | faible | corrigé | `08f6ae1` | `an_absent_directory_says_to_check_the_path_first`, `a_missing_key_of_a_historical_directory_names_init`, `a_historical_directory_with_an_oidc_file_states_the_single_tenant_rule` |
| `operator-guides-7` | faible | corrigé ; partie non prise | `c56e13d`, `a57855e` | `a_refused_directory_names_its_cause`, `a_refused_bundle_is_told_from_an_absent_one` ; L11, L47 |
| `operator-guides-8` | moyenne | corrigé | `4e2362c` | `the_local_http_permission_with_a_bundle_is_named_as_the_cause` |
| `operator-guides-9` | moyenne | corrigé | `fcfc0ce` | `the_demonstration_transport_is_a_warning` |
| `operator-guides-10` | faible | corrigé ; partie non prise | `74b0503`, `7ad4d5b` | `the_help_states_the_rules_between_options`, `options_given_without_effect_are_said_so` ; L13 |
| `operator-guides-11` | moyenne | documenté | `755efe5`, `469d625` | L26 ; N2 ; section 4 (R3) |
| `operator-guides-12` | faible | documenté en partie | `b6548b3`, `469d625` | L28, L29 ; reste en P12 |

Défauts trouvés en chemin, qu'aucun constat ne nommait (journal, entrées E-F1
et E-F6) :

- La ligne `omitted` du texte disait « entrys » : corrigé par `4eb8061`.
- Des phrases du catalogue parlaient pour tout le lancement et contredisaient
  un rapport qui signale une lecture (`provider.configuration_valid`,
  `provider.selection_resolved`, `data.offline_inspection_not_available`,
  trois actions) : corrigé par `dad29a1` et `8dfa6b5`.
- La référence d'AC d'un fichier OIDC qui nomme l'audit du même fichier était
  ouverte avant que cet audit soit enregistré : corrigé par `e33cca9`.
- `layout.unrecognized` disait « neither file was opened » alors qu'une option
  de fichier nommait ces mêmes fichiers et que le rapport les listait comme
  lus : reproduit RED par le vrai binaire, corrigé par `c7fd54c` et `16eda98`.
- Une clé dont le mode refuse la lecture à son propre compte : non corrigé,
  limite L46.

Codes de sortie qui ont changé pour un même arbre pendant les corrections
(E-F2) : credential conforme sous un répertoire que son lecteur refuse, 0 → 3 ;
installation préparée publiée sans son audit, 0 → 3 ; nom de base que le
gateway refuse, 0 → 3 ; registre d'un autre compte que ce compte ne peut pas
ouvrir, 3 → 4.

## 8. Décisions prises par le coordinateur, et questions ouvertes

**Ces décisions sont celles du coordinateur Claude. Aucune n'est une réponse
du mainteneur ni de Codex.** Chacune est datée dans le journal de travail
privé, sauf celle des journaux bruts, qui est consignée dans le profil de QA
privé (`qa/QA_HARNESS.md`).

| Décision | Date (UTC) | Contenu |
|---|---|---|
| Q1 | 10 nov., 21:35 | Raccord `provider::choose_profile` admis (point 1 en tête) |
| Q2 | 10 nov., 21:35 | Vérification active admise, bornée à la QA isolée et à la fixture d'émetteur (point 2 en tête) |
| Q3 | 10 nov., 21:35 | Par défaut : le raccord de parité `13d099c` est créé ; la décision revient à Codex à l'intégration |
| Q4 | 10 nov., 21:35 | Par défaut : la proposition d'image `78a5aa4` est créée ; sa construction reste `NOT_RUN` |
| Écarts de E-S2 acceptés | 11 nov., 00:35 | `--installation-dir` exclut `--tenants-file` et `--oidc-config` (erreur d'usage, code 2) ; code `layout.historical_passed_as_installation_dir` ; agrégation des constats par code, portée, variante, cible et détails ; entrées d'un répertoire staging non décrites, et le message le dit ; bundle d'AC OIDC traité avec l'identité |
| Reprise et étape E-S4b | 11 nov., 10:45 | Notes du contrôle de E-S3 traitées avant les incréments ; `authentication-creation.json` et `authentication-origin.json` décrits comme matériel de préparation, jamais ouverts ; contrôle de E-S4 repris par celui de E-S4b ; harnais révision 3 ; répertoires jetables laissés en place |
| Traitement de la revue | 11 nov., 15:20 | Tout constat confirmé est corrigé avec un test exécuté sur le vrai binaire, ou consigné comme limite nommée ; aucun n'est écarté en silence |
| Pas de nouveau raccord dans `src/control_fs.rs` | 11 nov., 15:20 | Pour `operator-guides-3`, ce fichier est hors de la liste du mandat : la correction interroge une fonction produit déjà accessible ; le raccord facultatif est une proposition pour Codex (P4) |
| `secret-hygiene-7` | 11 nov., 15:20 | La comparaison par identité de répertoire n'est ajoutée que si un test exécuté la prouve en QA ; à défaut, limite et test décisif décrits (L7, N7) |
| Statut d'une sélection automatique que le produit refuse | 11 nov., 15:20 | Aucun changement de code : erreur et sortie 3, même sans option de sélection, comme le prévoit la conception ; écrit dans les guides (L18). À confirmer (P9) |
| Journaux bruts privés | 11 nov., 16:45 | Les journaux de QA des lots D et E restent privés ; le handoff les cite par nom et SHA-256 |
| Base des trois codes « path » | 11 nov., 20:05 | `provider.credential_unsafe`, `provider.unselected_profile_reference_unsafe` et `reference.private_rule_violated` gardent la base `inferred`, avec un message qui dit que le verdict est celui de la marche du produit. L'inverser demande de nouveaux codes : laissé à Codex et au mainteneur (P19) |
| Énoncés de ce qui a été exécuté, gardés dans les guides | 11 nov., 20:05 | Section « What was not executed », paragraphe « What was executed » de la vérification active, lignes du tableau du moteur, « Every run of the tool was on Linux, on ARM64 », mentions « read in the code ». Ils sont vrais à ce commit. `AGENTS.md` place l'avancement et les preuves sous `docs/engineering/` : Codex ou le mainteneur peut les y déplacer, et ils sont à mettre à jour dès qu'une exécution de plus a lieu (P20) |
| Ordre de lecture de `--providers-file` et du fichier OIDC | 11 nov., 20:05 | Inchangé ; le cas est exécuté par un test et la phrase des guides est restreinte à ce qui est vrai (L51) |
| Fichier de profils d'un répertoire de configuration qui vaut `/` | 11 nov., 20:05 | Une demi-phrase dans les deux guides ; le cas reste `NOT_RUN` (N28) |
| Règle d'arrêt | 11 nov., 20:05 | Après E-F6, seuls les points bloquants sont corrigés ; toute autre remarque va au handoff comme limite |
| Lecture du critère Clippy pour un fichier partagé | 11 nov., 22:20 | V6 reste classée PASS : le critère « zéro avertissement dans les fichiers du lot » est lu « aucun diagnostic imputable au lot ». La seule ligne située dans un fichier que le lot touche, `src/gateway_config.rs:198:34`, est le diagnostic de la base, sur une ligne identique à la base, dans un fichier partagé que le lot n'a pas le droit de corriger (section 6, V6). À renverser si la lecture littérale est voulue (question 8) |
| Couple `c7fd54c` / `16eda98` | 11 nov., 22:20 | Gardé tel quel : Claude ne fusionne pas les deux commits, ce qui réécrirait l'historique et changerait les SHA que nomment les preuves de V. Codex peut les fusionner à l'intégration (question 7) |

Chaque entrée d'étape du journal liste en outre, sous « Écarts … à confirmer
ou renverser », les choix d'implémentation faits sans réponse consignée :
E-S4b, E-S5, E-S6, E-F1 à E-F6 et la validation finale (L44).

Questions laissées au mainteneur ou à Codex. Aucune réponse n'est consignée :

1. Ratifier ou refuser Q1 et Q2 (points 1 et 2 en tête). Refuser Q1 retire
   aussi les deux codes de facturation et toute la preuve exécutée du critère
   « Un abonnement demandé ne bascule jamais vers une API payante ».
2. Garder ou retirer les propositions d'image et d'index (point 3).
3. Avant toute publication du contrat : l'axe de
   `trust.decision_transport_invalid` (P7) et le sort des deux codes supprimés
   (P10).
4. Sortie 3 pour une sélection entièrement automatique que le produit refuse
   (P9).
5. Base `inferred` des trois codes « path », ou nouveaux codes (P19).
6. Un nouveau code d'erreur pour une clé que le compte du gateway ne peut pas
   lire (L46).
7. `c7fd54c` et `16eda98` : le coordinateur a gardé le couple tel quel. Le
   garder, ou fusionner les deux commits à l'intégration. L'arbre de la tête
   ne changerait pas, mais les SHA changeraient et les preuves de V
   nommeraient un autre commit. Le contrôle de E-F6 le juge non bloquant.
8. Lecture du critère Clippy pour le fichier partagé `src/gateway_config.rs` :
   confirmer ou renverser la lecture du coordinateur (section 6, V6).
9. Journaux bruts : les laisser privés, ou les versionner comme pour les trois
   lots précédents. Dans ce cas ce document, le patch, les manifestes et
   l'index sont à mettre à jour.
10. Déplacer ou non vers `docs/engineering/` les énoncés d'exécution que
    portent les guides (P20).

## 9. Risques, limites et travail d'intégration

Les identifiants sont ceux de la liste consolidée du journal de travail privé
(11 novembre, 19:10 UTC, complétée à 20:42 UTC), reprise ici ligne par ligne.
Les phrases anglaises entre guillemets sont celles des guides ou du journal ;
la phrase française miroir est au même endroit de `README.fr.md`. Trois lignes
de cette liste citaient des phrases que E-F6 a reformulées (L10, L20, L28) :
elles sont données ici telles que les guides les portent au commit validé.

### Limites de la protection « jamais ouvert »

Chacune vient du code. Toutes sont écrites dans les deux guides, section
« Files never opened », sauf mention.

| N° | Limite |
|---|---|
| L1 | « A file given by option that lies outside every inspected directory, and that no file read before it names, is opened as what the option says it is. » |
| L2 | L'ordre des lectures ne protège qu'un sens : un fichier ouvert avant que le fichier qui le nomme soit lu est signalé par `mode.credential_contents_read` = `detected_after_read` et par `reference.file_read_before_it_was_named`, pas évité |
| L3 | « A second name of a key file below `tenants` is not recognised when it stands for `tenants.json`: only the registry names those files, and the tool lists no directory. » |
| L4 | « A CA reference that points at a private file the tool cannot recognise as a key, a record or a store is read as the gateway would read it. » |
| L5 | Un registre ou un fichier OIDC qui n'est pas lu ou pas décodé ne nomme rien : les fichiers qu'il aurait nommés ne sont alors connus que par les noms réservés d'un répertoire inspecté |
| L6 | La comparaison avec les fichiers réservés et l'ouverture sont deux résolutions de chemin séparées. Un changement fait par le même compte, ou par root, entre les deux est hors de cette garantie, comme pour le produit. Aucune exécution n'a exercé un tel changement (N13) |
| L7 | **Volume insensible à la casse.** Un répertoire réservé est reconnu par son nom, écrit et résolu ; un fichier nommé par son chemin, son périphérique et son inode. Sur un volume qui ignore la casse, il n'a pas été vérifié qu'un fichier qu'aucun fichier inspecté ne nomme, conservé sous `secrets`, `mounted-secrets` ou `tenants`, et donné sous une autre casse de ce nom, est refusé. Test décisif en N7 |
| L8 | Un fichier du chemin final que ni le registre ni le fichier OIDC d'un répertoire staging ne nomment, tel un enregistrement de préparation du répertoire final, n'est pas reconnu (guides, « Layouts ») |
| L9 | Un fichier retenu parce qu'il est sous `secrets`, `mounted-secrets` ou `tenants` est rapporté `error`, sortie 3, quoi qu'en ferait la commande consommatrice : c'est un refus de l'outil, pas de cette commande |
| L10 | « For a file that conforms below a directory that the reader accepts and that is not a private 0700 directory, the tool does not check the file system or the extended access entries of the file itself: `reference.private_rule_not_evaluated` says so, and only the consuming command checks them when it opens the file. On macOS the walk of the reader also refuses a directory whose extended access entries permit a modification: the message names that cause, which is read in the product code and was not executed, since every run of the tool was on Linux. » |
| L31 | `mode.credential_contents_read` = `none` signifie « aucun détecté parmi les fichiers que nomment les fichiers inspectés » ; le nom du champ seul peut se lire comme absolu |
| L32 | Non écrit dans les guides. Le refus « provider configuration is unavailable or not private » est traité comme « non lu », alors que le lecteur privé peut le rendre après sa lecture bornée si le fichier change pendant la lecture ; le fichier est alors absent de `files_read_for_content` et de la comparaison finale. Même famille que L6 |
| L45 | Non écrit dans les guides. Le verdict « path » de `operator-guides-3` repose sur le littéral `private file bound exceeds the supported size limit` et sur l'ordre des étapes de `PrivateFile::open` dans `src/control_fs.rs`. Si l'intégration reformule ou réordonne ce fichier, tout credential conforme hors d'un répertoire 0700 devient une erreur (sortie 3). Deux tests unitaires de `src/diagnostics/fs_facts.rs` sont le fil de garde ; P4 retire la dépendance |
| L51 | Un fichier de profils est lu avant le fichier OIDC : quand `--providers-file` nomme l'audit d'authentification qu'enregistre le fichier OIDC, cet audit est lu comme un fichier de profils s'il fait au plus 65536 octets, et le rapport le dit par `reference.file_read_before_it_was_named`. Exécuté pour un audit de 24576 octets ; le refus par la taille d'un audit plus grand est lu, non exécuté |

### Autres limites du rapport et des tests

| N° | Limite |
|---|---|
| L11 | `layout.posture_refused` dit si le mode du répertoire est en cause, ou sinon un répertoire au-dessus, un lien ou le système de fichiers, sans distinguer ces trois causes. Mode et propriétaire observés ne sont pas imprimés |
| L12 | `data.parent_directory_not_private` couvre toutes les causes de la règle du répertoire privé et ne les distingue pas |
| L13 | Une mauvaise combinaison d'options imprime la phrase fermée `invalid command line`, qui ne dit pas quelle règle est enfreinte ; les trois règles sont dans `--help` et dans les notes de la section Options |
| L14 | Un fichier de clé vide ou trop grand, et un même fichier référencé pour deux identités, sont des refus certains du gateway que l'outil ne détecte pas ; ils ne sont couverts que par `reference.content_not_checked` et `registry.loader_acceptance_not_checked` |
| L15 | Pour l'objet fournisseur, le validateur du produit rend une seule valeur fermée : `provider_block_invalid` couvre toutes les règles de `gateway.provider` et l'outil ne dit pas laquelle a échoué (P5) |
| L16 | Le `ok` d'un registre ou d'un fichier OIDC couvre les règles restituées et les validateurs appelés ; il est toujours accompagné de `…loader_acceptance_not_checked`. Le fil de garde des enveloppes compare les noms de champs et un corpus de fichiers : un défaut, un alias ou un changement de type qu'aucun cas n'exerce n'est pas détecté |
| L17 | Observé par la fonction du chargeur : un gateway à qui l'on demande d'initialiser l'audit d'un fichier dont la politique ne finance pas le plan refuse après avoir créé le fichier d'audit, et le laisse en place ; le rapport n'en parle pas. Non exécuté par le binaire gateway (N11, P17) |
| L18 | Sans `--client`, `--auth-mode`, `--client-profile` ni `--model`, les constats du fournisseur et le code de sortie sont ceux d'un lancement géré qui omet ces options. `providers.selection.requested` dit quelles parties étaient automatiques ; elles ne figurent ni dans `inputs` ni dans `assumes` |
| L19 | « A selection, not a launch. » Ni présence du client, ni version, ni compte ; le credential du profil sélectionné n'est pas ouvert |
| L20 | **La vérification active contacte les hôtes que nomme le fichier inspecté.** « For a file on the `https_only` transport no address filter is applied to them: a host on a private network is contacted like any other. A file that declares the demonstration HTTP transport passes the offline rules only when its issuer is a loopback address literal or the fixed demonstration issuer […], with its endpoints below that issuer. Nothing is printed before the contact. » Les guides disent de lire `issuer` et `keys endpoint` sans `--active` d'abord |
| L21 | Un `ok` de la vérification active dit que ce fournisseur a répondu à ce client depuis cet hôte à cet instant. Il ne prouve ni une connexion, ni que le gateway joint le fournisseur depuis son propre réseau |
| L22 | Aucun verdict de transport sans bundle utilisable : `inference.decision_transport_not_checked` |
| L23 | **Aucune date de validité** : un certificat expiré n'est pas détecté (`trust.certificate_validity_period_not_available`) |
| L24 | Les motifs fermés dépendent de textes du produit ; un refus non reconnu a le motif `unclassified`, le verdict reste exact. Deux refus du moteur gardent `unclassified` par conception |
| L25 | Un seul compte, systèmes de fichiers locaux (`tool.linux_macos_posture_policy_only`) |
| L26 | Volumes de conteneur : inspectables seulement dans un conteneur d'une image de contrôle dont le Dockerfile construit et copie l'outil ; ces commandes n'ont pas été exécutées (N2) |
| L27 | Neuf limites du build, dans la section `limits` de tout rapport : `identity.arbitrary_oidc_provider_not_qualified`, `inference.internal_profile_not_available`, `topology.kubernetes_launch_not_available`, `layout.sibling_staging_not_inspected`, `tool.linux_macos_posture_policy_only`, `provider.qualified_versions_not_available`, `provider.client_presence_not_available`, `provider.client_version_not_available`, `provider.account_validity_not_available` |
| L28 | **Un rapport par défaut cache les chemins, pas les noms.** Il imprime les identifiants de tenants, les paramètres publics d'un fichier OIDC (émetteur, URL des endpoints, identifiants de clients, audience, libellé du fournisseur, origine web, redirection web, chemin de callback CLI, révision du mapping des claims), les identifiants de profils, les modèles, les noms de variables de credential et une URL de moteur de décision donnée en option : à ne partager que là où ces noms peuvent l'être |
| L29 | L'en-tête du rapport texte ne donne que des comptes : chercher `[error]` et `[unknown]`, ou filtrer `findings` par `status`. Aucun rapport documenté n'a de constat `unknown` : cette balise est lue dans le rendu |
| L30 | Contrat JSON : les types Rust du dépôt ne décodent que les rapports de leur propre build ; aucun test ne décode le rapport d'un autre build (point 4 en tête) |
| L33 | `identity.sso.idp_contacted` vaut `true`, et le texte dit « contacted », aussi quand rien n'a répondu ; les guides le définissent « whatever its result ». Laissé tel quel |
| L34 | Les variantes de texte du catalogue sont choisies par nom ; un nom mal orthographié retombe sur le texte générique sans erreur. Aucun type fermé |
| L35 | Avec des racines imbriquées, l'affichage prend la première racine qui correspond. Non documenté |
| L36 | Les oracles de refus du gateway lisent des textes du produit de la base, plusieurs en français : une intégration qui reformule un refus fait échouer le test à ce site, c'est son rôle. À deux sites, le texte du gateway n'est pas la cause du constat apparié : le tenant renommé s'arrête à la liaison de tenant de la base (`database tenant binding mismatch or unsupported schema`) ; une politique éditée sur un audit existant rend `authentication audit integrity failure; recovery required`, qu'elle finance le plan ou non |
| L37 | `explicit_subscription_is_preserved` ne discrimine pas seul ; la moitié discriminante est `refused_subscription_never_becomes_api` |
| L38 | Le test d'environnement hostile couvre les quinze variables qu'il pose et ne sélectionne pas le client `claude-code` ; qu'aucune autre variable ne soit lue repose sur la lecture du code |
| L39 | `the_selection_of_run_is_built_as_it_is_restated` vérifie la présence de huit lignes de `src/runner.rs`, pas leur sens : un changement ailleurs dans `run` qui modifie la sélection n'est pas détecté (P6) |
| L40 | Oracles qui échouent fermés et variable `AI_GUARD_TEST_ORACLE_NOT_RUN` : section 6. Ils sont cinq depuis E-F6 |
| L41 | Arbres intermédiaires : section 3, « Particularités » |
| L42 | Dans le harnais de test, `gateway_start` attend la fin du flux d'erreur du gateway après l'avoir arrêté : un descendant du gateway qui tiendrait ce flux ferait attendre le test. Aucun cas dans les 84 tests |
| L43 | Le partage entre `registry.policy_invalid` et `registry.policy_refused_at_assumed_event_bound` se fait par genre de politique, pas par cause : tout refus d'une politique `audit_capacity` ou `workload_audit` est rapporté `unknown` (sortie 4 s'il n'y a pas d'erreur) dès que la borne est supposée, même quand aucune borne de 1 à 1000000 ne l'accepterait |
| L44 | Choix d'implémentation sans réponse consignée : listes « à confirmer ou renverser » du journal (section 8) |
| L46 | **Clé illisible par le compte du gateway.** Une clé dont le mode refuse la lecture au compte du gateway n'est pas rapportée comme une erreur : avertissement `reference.token_file_accessible_to_other_accounts` et sortie 0 quand son mode donne aussi accès au groupe ou aux autres, aucun constat sinon. Le gateway ne peut pas l'ouvrir. Exécuté pour le mode 0044 avec le vrai gateway ; les autres modes sont lus. Le test fige cette sortie 0 : ce n'est pas une acceptation |
| L47 | La cause macOS (entrée d'accès étendue) est nommée par les trois textes « path » et par `layout.posture_refused` ; elle ne l'est pas par `data.parent_directory_not_private` ni par les énoncés positifs de la règle du répertoire privé. Toute la branche macOS est lue, jamais exécutée |
| L48 | `--config-dir / --installation-dir /` : le répertoire d'installation est montré `<config>`. Aucun chemin n'est imprimé ; par lecture, aucun test |
| L49 | Le dernier cas de `a_refused_oidc_file_is_reported_with_its_cause` fige la sortie 0 pour un fichier OIDC que le gateway refuse ensuite (politique valide qui n'est pas celle de l'audit). Cohérent avec `oidc.loader_acceptance_not_checked` ; à ne pas lire comme une acceptation (P17) |
| L50 | Les deux textes de `layout.unrecognized` pour des fichiers du gateway sans enregistrement de préparation ne disent rien de ce qui a été ouvert : la liste des fichiers lus le dit |

### Intégration

Relu pour ce handoff sur `4b1927f`, tête actuelle de
`codex/human-brokers-validation-20261104` (22 commits depuis la base ; le
journal avait lu `3145315`). Le commit de plus ne touche aucun fichier que le
lot modifie ou inclut ; dans `src/main.rs` il n'ajoute que la cible de
migration 13. Par `git diff` et `git grep` sur cette référence, en lecture
seule, les énoncés I1, I2, I3, I6 et P13 tiennent toujours, et restent
inchangés depuis la base : `src/lib.rs`, `src/provider.rs`, `src/provider/**`,
`src/runner.rs`, `src/control_fs.rs`, `src/audit/storage.rs`,
`src/runtime/invocation.rs`, `src/oidc/**`, `tests/support/oidc_fixture.rs`,
`config/tenants.example.json`, `README.md`, `README.fr.md`, `Cargo.toml` et
`Cargo.lock`. **Rien n'a été exécuté sur cette branche, et l'arbre fusionné
n'a jamais été compilé : `NOT_RUN`.**

| N° | Point d'intégration |
|---|---|
| I1 | **Miroir du registre.** Sur la branche d'intégration `TenantFiles` a `relay_retention_policy`. La fusion de `src/gateway_config.rs` est textuellement propre, puis `diagnostics_parity_envelope_field_names_equal_the_loaders` est RED par conception (12 noms contre les 11 de `TENANT_FIELDS`). À faire : ajouter le champ à la structure miroir et à `TENANT_FIELDS` ; appeler `RelayRetentionPolicyV1::validate` comme `validate_policies` (son refus y est `Politique de rétention du relais invalide`) ; ajouter le genre de politique `relay_retention` et sa valeur dans la ligne `policy` des deux guides ; ajouter un cas accepté et un cas refusé au corpus de parité (`sender.messages` 513, champ inconnu `burst`, comme le test du chargeur). Le diff de `src/gateway_config.rs` n'y touche aucune ligne OIDC, et `src/oidc/**` comme `src/audit/authentication/**` y sont inchangés ; `src/api/oidc.rs` y change (I6). Les validateurs de politiques des autres modules n'ont pas été relus |
| I2 | **Trois énoncés à reformuler** (point 4 en tête). À faire : décrire `verify-audit` avec son fichier viewer explicite ; garder `data.offline_inspection_not_available` pour ce qui reste non examiné ; ajouter un code `not_checked` sur l'axe des données dont `requires` nomme l'opération hors ligne explicite (ajouter un code est permis en version 1, changer le statut d'un code existant ne l'est pas) ; ajouter `ai-guard-audit-checkpoint` au tableau des diagnostics existants des deux guides ; régénérer les rapports documentés. Le guide de cet outil dit que sa lecture peut créer des fichiers `-wal`/`-shm` vides et un fichier de verrou à côté de la base : il reste une opération explicite distincte, `ai-guard-diagnose` n'ouvre toujours aucune base |
| I3 | **Trois conflits de fusion** et leur résolution : point 3 en tête. Les autres raccords (`6adaf64`, `13d099c`, `a95bf67`) et les hunks des README racine s'appliquent sans conflit. Le checkout principal a des modifications non committées de `README.md`, qui n'ont pas été lues. Preuve à faire par Codex : construire l'image depuis le Dockerfile fusionné, puis `--entrypoint ai-guard-diagnose … --version` |
| I4 | Raccord fournisseur : point 1 en tête |
| I5 | Notes des contrôles : section 7 |
| I6 | Sur la branche d'intégration le gateway a une option de plus, `--relay-retention-file`, et `OidcGateway::new` un refus de plus (« V4 authentication audit requires its durable capacity plan »). L'outil ne connaît ni l'une ni l'autre : `topology.gateway_arguments_not_persisted` nomme « Listening addresses, policy file and event bound », et les fragments de refus du gateway des tests n'ont pas été relus contre cette branche |
| I7 | Fichiers lus par `#[path]` et `include_str!` : section 2 |
| I8 | `entry_points_state_what_is_opened_as_the_report_does` lit les quatre index à l'exécution : garder `26166e4` sans les quatre commits `wire:` ultérieurs sur ces fichiers (`68bea8c`, `771dde4`, `1fc07b5`, `4ecd6dc`) le fait échouer ; les retirer tous les cinq (R4) le laisse passer (par lecture du test). Section 4, R4 |

### Propositions pour Codex

Aucune n'est implémentée, sauf mention.

| N° | Proposition |
|---|---|
| P1 | Garder ou non les fichiers partagés hors liste ou proposés : points 1 à 3 en tête |
| P2 | Structurel, long terme : déplacer les quatre enveloppes fermées du gateway dans la bibliothèque, pour que le chargeur et l'outil partagent une définition |
| P3 | `src/provider/selection.rs` : `let (profile, source) = choose_profile(config, selection)?;` dans `prepare`, pour qu'un seul chemin serve les deux. Jusque-là, un contrôle ajouté à `prepare` avant le choix n'est attrapé que par les tests dont l'oracle est `provider::prepare` |
| P4 | `src/control_fs.rs`, facultatif : `pub(crate) fn private_file_ancestry(parent: &Path) -> Result<()> { walk(parent, false, false).map(drop) }`, appelé par `fs_facts::reader_accepts_directory` à la place de `PrivateFile::open` avec un nom impossible et une borne hors plafond. Le test `the_reader_is_asked_about_directories_and_opens_nothing` reste ; le test d'ordre des étapes devient inutile |
| P5 | `src/oidc/config.rs` : un sous-motif fermé rendu par `OidcConfigV1::validate`, pour que `provider_block_invalid` dise quelle règle échoue |
| P6 | `src/runner.rs` et `src/provider/selection.rs` : une fonction du produit, par exemple `ProviderSelection::from_run_options(client: Option<&str>, auth_mode: Option<AuthPreference>, profile_id: Option<String>, model: Option<String>, transport: ClientTransport) -> anyhow::Result<Self>`, avec le refus `unknown client selection`, appelée par `run` et par `selection_request` ; le test de garde disparaît alors |
| P7 | Axe de `trust.decision_transport_invalid` : point 4 en tête |
| P8 | Ajouts permis plus tard en version 1, non faits : codes `not_checked` distincts pour les fichiers retenus sous un répertoire réservé ; `client` et `auth_mode` par défaut listés comme entrées d'origine `default` et nommés dans `assumes` ; origines contactées dans les détails des constats `active.*` ; décodeur tolérant, avec un test qui décode un code et un champ inconnus |
| P9 | Confirmer la sortie 3 pour une sélection entièrement automatique que le produit refuse |
| P10 | Confirmer la suppression des deux codes avant publication, ou les réintroduire comme retirés |
| P11 | Mêmes textes que I2 |
| P12 | Reste de `operator-guides-12` : pointeurs d'une ligne vers l'outil dans les sections de diagnostic des guides local, fournisseurs, SSO et entreprise (fichiers partagés, non écrits) ; lien vers `docs/setup/local-ai/TLS.md` depuis `inference.model_tls_outputs_not_available` ; tableau des fichiers Compose fournis vers les options de l'outil ; une ligne d'en-tête du rapport texte listant les codes `error` et `unknown` |
| P13 | Avec `78a5aa4` : `preflight_build_inputs` (`src/operations.rs`) liste `src/bin/ai-guard-provider.rs` et pas `src/bin/ai-guard-diagnose.rs`, à la base comme sur `4b1927f` ; un build par `agentsandbox build` sur une source sans ce fichier échouerait dans le moteur et non au précontrôle. Aucun profil Compose de diagnostic |
| P14 | Durcissement facultatif dans un fichier du lot : dans `read_registry`, `fstat` du descripteur ouvert et refus avant la première lecture quand son identité est réservée |
| P15 | Si le test décisif de N7 échoue : enregistrer périphérique et inode de chaque répertoire réservé dans `NeverOpened::reserve`, et refuser dans `contains` quand un ancêtre du candidat a cette identité |
| P16 | Durcissement facultatif : répéter le fil de garde des noms de champs dans `tests/diagnose_cli.rs` par `include_str!("../src/gateway_config.rs")`, pour qu'il ne dépende plus de `13d099c` |
| P17 | Hors du lot, à signaler : un gateway à qui l'on demande d'initialiser l'audit d'un fichier dont la politique ne finance pas le plan laisse un fichier d'audit partiel (L17) ; une politique d'audit éditée dans le fichier d'une installation déjà démarrée est refusée par « authentication audit integrity failure; recovery required », texte qui ne dit pas que le fichier a été édité |
| P18 | Formulations laissées telles quelles, parce que les changer régénère les rapports documentés : L33 ; la variante `reserved` de `providers.json` à son nom fixe, dont le texte dit « The given provider profile file » |
| P19 | Base `inferred` des trois codes « path » : section 8 |
| P20 | Énoncés d'exécution gardés dans les guides : section 8 |
| P21 | Messages de commit à ne pas reprendre tels quels : section 3 |

### Vérifications non exécutées

Chacune est `NOT_RUN`. Aucune n'est un succès.

| N° | Non exécuté |
|---|---|
| N1 | `cargo build --release --locked`, commande des guides et du Dockerfile |
| N2 | Construction de l'image de contrôle, tout conteneur de cette image, les commandes `docker inspect`, `docker volume inspect` et `docker run` de la section du volume, leurs équivalents Podman. Le montage en lecture seule, le compte `1000:1000` et le code de sortie rendu par le moteur sont établis par lecture |
| N3 | macOS : aucune exécution. Branches macOS de la politique de posture, marche du lecteur, l'outil lui-même |
| N4 | Moteur réel présent aux emplacements fixes ; l'approbation n'est exercée que sur un programme marqueur. Neuf des seize textes de `engine.program_refused` ne sont produits par aucun test (`path_form` pour un chemin absolu, `directory_unavailable`, `size_bound`, `link_outside_approved_roots`, `filesystem_unsupported`, `account_home`, `helper_directories`, `runtime_directory`, `extended_access_entry`) ; sept le sont par le vrai binaire |
| N5 | Fournisseur d'identité réel ; découverte en HTTPS ; `active.oidc_discovery_provider_rejected`, `active.oidc_discovery_keys_invalid` et la variante `not_completed`. Que le client n'emploie aucun proxy, ne suive aucune redirection et ne réessaie pas : lu dans `src/oidc/transport.rs`, pas observé |
| N6 | Fournisseur IA, modèle, Codex CLI, Claude Code, abonnement, clé API : rien n'est contacté ni qualifié |
| N7 | **Test décisif du volume insensible à la casse**, à exécuter une fois sur APFS insensible à la casse et une fois sous Linux au-dessus d'un montage insensible à la casse : fixture historique, un fichier privé 0600 `secrets/extra` que rien ne nomme, puis `--config-dir <config> --jev-ca-file <config>/SECRETS/extra --json`. Attendu : `trust.ca_reference_is_a_credential_reference`, entrée `trust` `read: false`, date d'accès de `secrets/extra` inchangée. La QA est sensible à la casse (sonde `e-f1-case-probe`) |
| N8 | Deux comptes : un registre que ce compte ne peut pas ouvrir (classification testée unitairement seulement) ; une base ou un répertoire de base d'un autre compte ; des métadonnées illisibles |
| N9 | Axe des données : un sidecar `-journal` ; pour l'audit d'authentification, tout refus autre que son absence ; que le gateway ait servi des requêtes pendant l'inspection (un seul lancement à côté d'un processus gateway vivant, aucune requête adressée) |
| N10 | `setup resume` sur une installation publiée sans son audit d'authentification |
| N11 | Le gateway lancé avec `--initialize-oidc-audit` sur un fichier dont la politique ne finance pas le plan |
| N12 | Les refus `ProtectedOutput`, `OutputTooLarge` et `Internal` : implémentés, cités dans les deux guides, produits par aucun test |
| N13 | La modification concurrente de L6 |
| N14 | Chemins enregistrés par un répertoire staging : `--tenants-file` et `--oidc-config` (refusés avec `--installation-dir`) et `providers.json` à son nom fixe ne sont pas exécutés contre eux |
| N15 | `registry.policy_invalid` pour une borne d'événements hors de 1 à 1000000 (la ligne de commande l'empêche) |
| N16 | Arbres après retrait : section 4 |
| N17 | Arbre fusionné avec la branche d'intégration : jamais construit ; le fil de garde de parité y est RED par conception ; le test de garde de `run` et les fragments de refus du gateway n'y ont pas été exécutés |
| N18 | Suite complète du dépôt et `cargo test --lib` sans filtre |
| N19, N20 | Levés par la validation finale : parité, tests unitaires, `cargo check --all-targets`, Clippy, mode parallèle et recompilation complète ont été exécutés sur `469d625` (section 6) |
| N21 | Stabilité du mode parallèle : une exécution par tête d'étape et une sur le commit validé, ce qui n'établit pas la stabilité. Utiliser `--test-threads=1` pour interpréter un échec |
| N22 | Contrôles par mutation absents : T06, T16, `tests-7`, `tests-8`, partie (a) de `correctness-parity-6` |
| N23 | rustfmt sous Linux ; rendu Markdown des guides et des index ; relecture du français par un second lecteur |
| N24 | Plages Unicode `Default_Ignorable_Code_Point` de l'échappement : non revérifiées contre la base officielle |
| N25 | Contrôle indépendant de E-S4 seul |
| N26, N28 | Fichier de profils à son nom fixe dans un répertoire de configuration qui vaut `/` : affiché `[path not shown]` d'après le code ; la QA ne peut pas atteindre la branche, `/` y appartient à un autre compte |
| N27 | Trous de test nommés par les contrôles, encore ouverts : le texte `registry` de `reference.inspector_identity_differs` et de `reference.metadata_unavailable` ; les sidecars de l'audit d'authentification, imprimés mais non assertés par le test des bases ; T16 ne scrute que stdout ; `rustfmt` natif, jamais relancé par un contrôle ; pour les commits intermédiaires de E-F2, le manifeste ne lie l'arbre testé au commit que pour `c73f538`, `223a0d7` et la tête |
| N29 | La branche « réveil échoué » de `RecordingIssuer::drop` ; la cause macOS des quatre textes de L47 ; un gateway initialisé sur un fichier écrit à la main dont le plan a été changé avant la première initialisation ; un audit de plus de 65536 octets donné comme fichier de profils ; les modes 0000 et 0200 d'une clé ; `16eda98`, `1b6b1c3` et `1c45ab4` en QA |

### Notes du contrôle de E-F6 qu'aucun commit ne ferme

| N° | Note | Suite donnée |
|---|---|---|
| G1 | Les lignes R1 et R2 de la liste consolidée ne sont plus vraies à `469d625` | Resimulé : section 4 |
| G2 | Trois lignes de la liste citaient comme exactes des phrases reformulées (L10, L20, L28) | Ce document cite les guides au commit validé |
| G3 | `c7fd54c` ne construit pas sa cible de test | Section 3 ; question 7 de la section 8 |
| G4 | Un audit de plus de 65536 octets donné par `--providers-file` est tout de même **ouvert** par le chargeur du produit : `read_private` (`src/control_fs.rs`) ouvre le fichier et lit ses métadonnées avant le refus par la taille. Aucun octet n'est lu ; le rapport donne `provider.file_unavailable_or_not_private`, `mode.credential_contents_read` reste `none` et aucune détection n'est rapportée. Les guides disent « refused before any read » sans dire qu'il est ouvert. Par lecture | Limite. Ajout suggéré par le contrôle : « a larger audit is opened by the loader and refused for its size before any byte is read » |
| G5 | La lecture du fichier OIDC enregistre l'audit avec ses sidecars `-wal`, `-shm` et `-journal` : ils sont dans la même exception que L51 | Limite |
| G6 | « is read … if it is at most 65536 bytes » suppose aussi la règle du fichier privé (mode 0600, un seul lien, propriétaire). La puce de l'ordre des lectures dit « is read as a profile file » sans la condition de taille : elle dit lu ce qui peut ne pas l'être | Limite ; aucun contenu n'est lu au-delà de ce que disent les guides |
| G7 | Le cas de `a57855e` fige la sortie 0 pour un registre dont le gateway refuse la clé | L46 : ce n'est pas une acceptation |
| G8 | Le test lie la borne supposée à `DEFAULT_MAX_EVENTS` et au message ; le texte du catalogue reste un littéral, et l'égalité avec le défaut du gateway n'est pas l'objet de ce test | Limite |
| G9 | Nuances de preuve de E-F6 : son RED a employé des binaires du produit que Cargo jugeait frais depuis le build interrompu de `433eba9` ; son exécution finale n'a recompilé que le binaire de test | Sans effet sur V : V1 recompile tout depuis le commit figé |
| G10 | Non vérifié par ce contrôle : que les 1381 premières lignes du journal soient identiques octet pour octet après l'ajout ; `rustfmt` natif | N27 ; V7 a relancé rustfmt sur le commit validé |

### Notes du premier contrôle du handoff

Ce contrôle a jugé `d038341`, en lecture seule. Il a refait par script, contre
Git et les journaux, le tableau des fichiers, celui des commits, les neuf
raccords, les empreintes des journaux cités, les simulations de retrait et de
fusion, et les pièces privées, sans y trouver d'écart. Ses remarques :

| N° | Note | Suite donnée |
|---|---|---|
| H1 | Point bloquant : le point 1 en tête sous-estimait ce que retire le raccord fournisseur (deux codes et toute la preuve exécutée d'un second critère du mandat) | Corrigé dans cette version : point 1 en tête, section 5 (lignes « Q1 ») |
| H2 | La liste R1 des commits ultérieurs omettait `dad29a1` et `d25454f` | Corrigé : section 4, liste refaite commit par commit |
| H3 | Le point 2 ne disait pas ce que retire le retrait de la vérification active | Corrigé : point 2 en tête |
| H4 | La décision du 11 novembre, 16:45 UTC, sur les journaux bruts est consignée dans le profil de QA, pas dans le journal de travail | Corrigé : section 8 |
| H5 | Les énoncés sur la branche d'intégration valent pour `4b1927f` : si elle bouge, ils sont à relire | Limite. Relu à 21:53 UTC pour cette version : la branche locale `codex/human-brokers-validation-20261104` et sa référence de suivi distante sont toujours à `4b1927f` (aucun `git fetch` n'a été fait pour cette version) |
| H6 | Non vérifié par ce contrôle : V7 (rustfmt natif, sans journal) ; que `c7fd54c` ne s'analyse pas (seuls le message et le diff de `16eda98` ont été lus) ; les octets de fin de trois commits de E-F3 ; le contenu du volume de QA ; les journaux `e-f…` au-delà de leur empreinte ; la correspondance de chaque critère avec des assertions, que la section 5 dit fondée sur le nom des tests | Limite : ces points reposent sur les entrées d'étape du journal et sur les contrôles d'étape, pas sur ce contrôle |
| H7 | Les trois handoffs précédents versionnaient leurs journaux bruts ; ce lot les garde privés | Décision du coordinateur (section 8) ; question 9 de la même section |

### Notes du second contrôle du handoff

Ce contrôle a jugé `234e457`, en lecture seule, et l'a accepté sans point
bloquant, après avoir refait les mêmes comparaisons que le premier. Ses
remarques sont traitées par le commit qui suit `234e457`, écrit par le
coordinateur :

| N° | Note | Suite donnée |
|---|---|---|
| K1 | L'introduction des quatre points disait que les trois premiers concernent des fichiers que le lot ne possède pas ; c'est vrai du premier et du troisième seulement | Corrigé : en tête |
| K2 | Deux déclarations de contrôles n'étaient pas reprises en section 10 : les copies de travail du contrôle de E-F3 et les déclarations de procédé du contrôle de E-F6 | Corrigé : section 10 |
| K3 | La ligne `tests-1` citait la mutation `e-f3-mutation-cli` pour deux tests ; seul le test du registre a un contrôle par mutation | Corrigé : section 7 |
| K4 | La ligne `contracts-9` citait un identifiant « O4 » que ce document ne définit pas | Corrigé : renvoi à la section 11 |
| K5 | I8 omettait que le retrait des cinq commits d'index laisse passer le test | Corrigé : I8 |
| K6 | Le point 1 ne disait pas que les deux rapports `report.providers-two-modes.*` sont à régénérer après un retrait ; le point 2 ne nommait pas la valeur `active_checks_only` de `mode.network` | Corrigé : points 1 et 2 en tête |
| K7 | La section 11 ne listait ni le sous-répertoire `understanding` des notes ni `make_handoff.sh` | Corrigé : section 11 |
| K8 | Non vérifié par ce contrôle : V7 (rustfmt natif, sans journal) ; le contenu du volume de QA ; la chronologie et la liste des refus d'outil au-delà de leurs entrées du journal ; les journaux d'étape `e-f…` au-delà de leur empreinte, sauf huit qu'il a ouverts ; que `c7fd54c` ne s'analyse pas, confirmé par lecture seulement | Limite |
| K9 | Les corrections K1 à K7 et les deux décisions du coordinateur datées de 22:20 UTC (section 8) sont écrites après ce contrôle | Ce dernier commit n'a pas été contrôlé indépendamment |

## 10. Outillage

- **QA isolée.** Compilation et tests uniquement par le harnais `qa.sh`, dans
  le conteneur du lot : image locale épinglée par son empreinte, réseau
  `none`, racine en lecture seule, toutes capacités retirées,
  `no-new-privileges`, UID 1000, 3 CPU, 256 PID, mémoire sans swap,
  environnement à liste fermée, délai borné par exécution. Aucun Cargo, rustc,
  Node ni binaire du projet n'a été exécuté nativement. Le disque du volume
  n'a pas de quota noyau : le harnais mesure `/workspace` avant chaque
  exécution et refuse au-delà de 14 Gio, borne logicielle.
- **Révisions du harnais.** Révision 1 à la création. Révision 2 (11
  novembre, 00:25 UTC) : un gel ne ramène plus la date d'un fichier inchangé à
  celle de l'archive, défaut qui pouvait faire passer pour frais un artefact
  construit depuis un autre contenu ; `QA_REDATE_ALL=1` force une
  recompilation complète ; un journal de révision 1 terminé en quelques
  secondes sans ligne `Checking` ou `Compiling` de la crate n'établit rien.
  Révision 3 (10:45 UTC) : le conteneur du lot passe de 4 à 6 Gio, et les
  commandes lourdes des lots D et E attendent un créneau partagé (code 75 si
  le créneau reste occupé : ni PASS ni FAIL). Note de 20:06 UTC : un lancement
  tué côté hôte peut porter `rc=0` avec une sortie tronquée.
- **Limites d'environnement constatées.** La référence Clippy à deux jobs a
  fini `rc=101` avec un `oom_kill` au plafond de 4 Gio : limite du harnais,
  pas un résultat du produit ; elle passe avec `-j 1`. Le compteur `oom_kill`
  du conteneur vaut 2 et n'a pas bougé pendant V.
- **Luna** n'est pas disponible dans cette session : les tests ont été lancés
  par des agents Claude, de façon ciblée.
- **Analyse statique native.** `rustfmt 1.10.0-stable` sur les fichiers
  possédés seulement. Hors QA, d'après le journal, les étapes n'ont lancé
  nativement que `git`, `grep`, `sed`, `awk`, `cmp`, `shasum`, `rustfmt` et
  `python3 -I` sur des scripts de texte sans code du projet.
- **Aucun fournisseur IA, modèle, fournisseur d'identité, moteur ni secret
  réel.** Les clés réelles, les fichiers `.env`, les caches OAuth et le
  keychain n'ont jamais été lus. Les credentials des tests sont synthétiques ;
  les clés des installations de test sont créées par le produit dans le
  conteneur et ne sont ni affichées ni copiées.
- **Pour ce handoff.** Aucune exécution de QA, aucune compilation. Lecture
  seule de Git ; simulations de retrait et de fusion dans un magasin d'objets
  hors du dépôt ; `docker ps -a` et `docker volume ls` filtrés sur le nom du
  lot, pour relever des noms.

Appels d'outil refusés consignés au journal. Ce sont des garde-fous d'usage de
l'outil Bash, pas des refus de sûreté ; aucune exécution n'a été relancée pour
autant :

- E-S2 : « Un appel Bash a été bloqué par la règle « sleep suivi d'une
  commande » et remplacé par une attente `until` » (le journal ne garde pas le
  message exact).
- E-F1 : `sleep 45` suivi d'une lecture, message consigné « Blocked … use
  Monitor with an until-loop » (les points de suspension sont du journal).
- E-F2 : `sleep 100` suivi d'une lecture, même message consigné.

Des appels Bash en lecture seule ont d'abord reçu « no verdict » du mode
automatique, leur entrée ayant été réécrite par un hook ou un appel voisin
ayant changé (conception, E-S1, E-S3, contrôles de E-F5 et de E-F6) ; chacun
a été réémis une fois à l'identique, comme l'outil le demande. Les deux
opérations de fixtures broker refusées que Codex a consignées sont hors du
périmètre de ce lot : elles n'ont été ni réessayées ni reformulées.

Écarts de procédé déclarés : le contrôle de E-S5 a lancé `git fsck
--lost-found`, qui écrit dans le répertoire Git partagé (section 11) ; une
commande de E-F1, échouée avant tout effet, un appel de lecture de E-F6 et un
appel de lecture de ce handoff contenaient un `cd` contraire à la consigne,
sans effet. D'après leurs propres déclarations : le contrôle de E-F3 a d'abord
écrit huit copies de travail à la racine du scratchpad de session, que les
agents partagent, puis les a déplacées dans un sous-répertoire ; les contrôles
de E-F6 et du handoff ont écrit dans ce même scratchpad des copies de travail
et les magasins d'objets de leurs simulations, et employé `python3 -I` pour
une comparaison de texte ; plusieurs de leurs appels de lecture commençaient
par `cd`. Ce scratchpad n'est pas durable. Rien n'a été écrit dans un checkout
ni dans le magasin d'objets du dépôt.

## 11. Ressources laissées par le lot et inventaire

Ressources de QA, relevées en lecture seule le 11 novembre vers 21:05 UTC.
Rien n'a été arrêté ni supprimé ; leur arrêt et leur suppression reviennent au
coordinateur ou à Codex.

| Ressource | État constaté |
|---|---|
| Conteneur `aig-claude-qa-operator-diagnostics-20261110` | en cours d'exécution (« Up 25 hours »), image `2115d4321fa7`, créé le 10 novembre à 19:51:51 UTC, aucun port publié. Créé avec une durée de 48 h d'après le profil : il s'arrête seul vers le 12 novembre à 19 h UTC ; `qa.sh up operator-diagnostics` le recrée sur le même volume |
| Volume `aig-claude-qa-operator-diagnostics-20261110-scratch` | présent, pilote `local` ; c'est le seul volume du lot. D'après le profil et les journaux de V, il est monté sur `/workspace` et porte le dernier gel de source (`469d625`), la cible Cargo du lot et le Clippy installé par le harnais ; son contenu n'a pas été relu pour ce handoff |
| Répertoires jetables dans `/workspace/tmp` | Liste du journal `e-f4-list-scratch` (19:02 UTC) : `s2show-1`, `s4doc-1`, `s4doc-2`, `s4doc-3`, `s4b-1`, `s4b-2`, `s5-1` à `s5-5`, `f1-doc-j-1`, `f1-doc-k-1`, `f1-case-probe-1`, `f2-doc-1`, `f2-doc-2`, `f2-doc-3`, et une entrée `atime-probe.4963` dont le journal ne donne pas l'origine. Ce sont des installations synthétiques créées par les vrais `init` et `setup prepare`, avec des clés aléatoires du produit jamais affichées. Les étapes suivantes déclarent n'en avoir ajouté aucun ; la liste n'a pas été relue depuis |
| Credentials | aucun : aucun credential réel n'est monté dans le conteneur ni présent dans le worktree |

Certificat public de test des fixtures :
`tests/diagnose_support/fixtures/public-test-root.pem`, 672 octets, un bloc
`CERTIFICATE`, aucune ligne `PRIVATE KEY`, SHA-256
`18ecf8ec78ed03ad61b526c14d71770434132ae98169d6bd17b439b13392322a` (recalculé
au commit validé). Produit une seule fois dans la QA le 10 novembre par
`openssl req -x509 … -keyout /dev/null` : **aucune clé privée n'existe**, elle
a été écrite dans `/dev/null`. Committé par `b46bf41`, jamais régénéré.

Données privées, hors Git et hors `/tmp`, dans le répertoire d'ingénierie
privé du mainteneur, sous `claude-lots-DE-20261110/` (fichiers en mode 600) :

- `qa/logs/operator-diagnostics/` : 289 journaux bruts du harnais et son
  fichier marqueur `.last-sync`. Ils ne sont pas versionnés ; ce document les
  cite par nom et SHA-256.
- `notes/operator-diagnostics/` : `DESIGN.md`, `WORKLOG.md` (journal de
  travail), `GATE_NOTES.md`, les trois pièces de la revue,
  `FIX_RESULT_20261111.json` (résultat structuré des étapes de correction, de
  la validation finale et du handoff, déposé par le coordinateur) et le
  sous-répertoire `understanding/` (cinq notes de lecture du dépôt écrites
  avant la conception).
- `handoff/operator-diagnostics/` : le patch contre la base, la liste des
  commits avec leurs fichiers, les manifestes pré et post (une ligne par
  fichier de l'arbre de la base et de la tête ; pour chaque chemin du diff,
  son état et le SHA-256 de son contenu avant et après), la liste des
  empreintes de tous les journaux du lot, `make_handoff.sh` (le script qui
  produit ces pièces : il lit Git et les journaux et n'écrit que dans ce
  répertoire), et `INDEX.md`, qui nomme le commit de tête et donne le SHA-256
  de chaque pièce et de chaque journal cité ici. Ce répertoire n'entre jamais
  dans Git.

Le patch est le `git diff --binary` de la base à la tête de la branche du
lot : il ne contient que le diff du lot, donc aucune copie du travail en cours
de Codex et aucun des anciens tests auxiliaires Keycloak, Qwen ou PKI. Une
recherche de texte dans le patch, consignée dans l'index privé, n'y trouve
aucun chemin du poste hôte, pour seuls blocs « PRIVATE KEY » trois littéraux
de test synthétiques d'un octet (`AA==`), et pour seules chaînes de la forme
d'un jeton les suites alphabétiques synthétiques des tests de masquage. Une recherche de texte n'est pas une preuve d'absence de tout
secret ; aucun secret réel n'a été lu ni employé par le lot (section 10).

Répertoire Git partagé, hors du worktree : `.git/lost-found` (141 fichiers),
écrit par le contrôle de E-S5 ; des objets non référencés, dont deux arbres
écrits par `git merge-tree --write-tree` à E-S5 et le commit `afd028c` que
l'amend de E-S1 a remplacé. Le lot n'a rien supprimé ; le retrait est laissé à
l'intégrateur. Un stash existe dans le dépôt partagé (« ai-guard relay draft
retained after reviewed integration c762a24 ») : il est étranger au lot et n'a
pas été touché.

Inventaire du worktree au commit validé :

| Catégorie | Constat |
|---|---|
| Fichiers suivis | 2411 ; 2412 avec ce document |
| Modifications suivies | aucune |
| Fichiers non suivis et ignorés | aucun : `git status --porcelain --ignored` est vide. Aucune cible Cargo, aucun `node_modules`, aucune base, aucun credential dans le worktree |
| Montage du worktree | aucun : la source entre dans le conteneur par flux tar |

**Le worktree n'est pas à retirer par Claude.** Aucun merge, rebase, push ni
suppression n'a été fait.

## 12. Ce qui reste à Codex

1. **Décider** les quatre points en tête et les questions de la section 8.
2. **Intégrer** la branche : résoudre les trois conflits (point 3), mettre à
   jour le miroir du registre et le corpus de parité (I1), reformuler les
   trois énoncés devenus faux et régénérer les rapports documentés (I2),
   relire les fragments de refus du gateway et le test de garde de `run`
   contre la branche d'intégration (I6, N17).
3. **Valider la composition** : rien n'a été compilé ni testé sur un arbre
   fusionné ou réduit.
4. **Exécuter, s'il le souhaite, les `NOT_RUN`** qui comptent pour une
   qualification : `cargo build --release --locked` (N1) ; l'image de contrôle
   et le parcours en conteneur (N2) ; macOS et le test décisif du volume
   insensible à la casse (N3, N7) ; un moteur réel (N4) ; un fournisseur
   d'identité réel en HTTPS (N5) ; la suite complète du dépôt (N18).
5. **Mettre à jour les documents d'état** de `docs/engineering/` : ce lot n'en
   modifie aucun et ne ferme aucun gate.
6. **Pousser**, puis **fermer le worktree** après intégration et preuve
   distante, en conservant ailleurs les journaux privés encore utiles. Les
   ressources de QA et le contenu de `.git/lost-found` restent à traiter.

## Intégration Codex — 12 novembre 2026, source candidate

Les remarques et questions historiques ci-dessus restent rattachées à leurs
commits et preuves Claude ; elles ne sont pas réécrites ni transférées au gel
composé. Root a ratifié le helper pur Q1, la conservation de l’abonnement
explicite sans repli API payant, et Q2 uniquement derrière l’opt-in discovery.
Aucun contact IdP n’est effectué pendant cette composition. L’image conserve
les trois binaires agentsandbox, checkpoint et diagnose ; les deux index de
guides conservent les entrées publiques D et E.

CEIR-01 ajoute le miroir typé relay_retention_policy, sa validation réelle et
le corpus de parité null/valide/sender.messages=513/champ burst. CEIR-02 borne
les messages aux connexions SQLite que ce diagnostic n’ouvre jamais, documente le viewer
explicite de verify-audit et signale séparément la vérification signée volontaire
comme not_checked. La première baseline JSONv1 publique sera le gel composé
corrigé ; les axes actuels, dont Trust, sont conservés, les deux identifiants
supprimés avant publication restent réservés. CEIR-03 renvoie à la décision
Clippy de 22:20 : aucun nouveau diagnostic imputable, sans PASS strict inventé.

Ces corrections et leurs nouvelles régressions sont des sources candidates :
compilation, tests de composition, régénération des neuf rapports d’exemple et
Clippy sont NOT_RUN ici. Les exemples d’origine restent conservés pour être
régénérés à partir du vrai binaire en QA. Les résultats historiques de V restent
limités à V ; aucun moteur, image release, IdP ou parcours fournisseur n’est
qualifié par cette composition. Le second regard puis la QA ciblée reviennent
à l’intégrateur avant publication.
