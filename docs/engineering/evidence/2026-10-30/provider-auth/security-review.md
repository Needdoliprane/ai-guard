# Revue indépendante ciblée — fournisseurs et authentification

Date : 30 octobre 2026. Candidate figée : `/tmp/ai-guard-provider-review.fL91yK`.

**Verdict de cette lecture : aucun P0/P1 confirmé ; trois P2 ci-dessous restent à traiter. Aucune qualification fournisseur réelle ou release n'est prononcée.** Les constats sont démontrés par le code, sans exécution de reproduction par le relecteur. Une admission V1 ne constitue pas une fuite des logs moteur démontrée : deux protections du runtime bloquent son premier démarrage.

## Périmètre et preuves

Lecture de `provider*`, bootstrap/bin, modèle/policy/admission/audit, runner/managed, operations, Dockerfile et tests concernés ; contrats d'architecture, règles et gouvernance applicables. Guidage security-audit limité à la revue demandée, sans campagne globale Daybreak, cloud ou fournisseur. Aucun secret réel, fichier `.env` du dépôt, cache OAuth ou keychain ouvert. Aucun code produit/document du dépôt modifié, aucun agent créé, aucun test/binaire/image exécuté par cette revue.

Le SHA-256 de `candidate.sha256` est `ecd4be78628aaee7e4e282a6322a318b9380939f81c738cbbe623d5ebd8faf22`. Vérification effectivement exécutée : `shasum -a 256 -c candidate.sha256`, exit 0, les 20 entrées concordent. Les références de lignes ci-dessous désignent ce gel, pas le checkout mutable.

Fichiers déterminants :

- `src/control.rs` : `b993907b072b02a0548733ddd02fee8f79fec98c976fbcdde5970e6853536766`.
- `src/runner.rs` : `0c24595173ee31c9263233625d3e82787ced77f8dc5e5bd77fccb4a3ab8d131f`.
- `src/provider/selection.rs` : `8fd5321e8dd4b8d6492535fde7745e1e44f7bde4d4b11d5ef9e7b58f609d9b38`.
- `src/provider/bootstrap.rs` : `a0bafe6965c7d66f62811aad4242f60441b55b91a1921bde4c1ae9df4a4f4340`.

Les rapports joints annoncent 72 régressions contrat, 14 tests provider purs et 2 tests de processus canaris PASS. Ce sont des preuves rapportées, recoupées avec leurs assertions lorsque pertinent ; cette revue ne les a pas réexécutées et ne les transforme pas en preuve moteur ou fournisseur. Le test historique `tests/provider_authorization.rs:82` compare les octets JSON exacts et le hash épinglé du vieux `DecisionRequest` ; il conserve l'absence des champs optionnels, pas un `null` ajouté.

## PRV-01 — P2 : nouvelle admission fournisseur batch acceptée avec le preset historique V1

**Confirmé par lecture**, reproduction comportementale ciblée à exécuter par le propriétaire des tests.

Lieu : `src/control.rs:428–439`, lien admission/requête `:126–132`. Le contrôle impose l'égalité de `is_interactive()`, mais pas `requires_no_engine_logs()`. Pour une résolution batch, `FixedSandboxV1` et `FixedSandboxV2` satisfont donc le même prédicat. La validité du contexte policy ne distingue pas ces presets.

Préconditions : acteur runner authentifié capable de présenter une admission cohérente avec une vraie décision durable ; contexte fournisseur batch valide, profil observe et bridge. Le viewer/worker non autorisé n'acquiert aucun droit par ce défaut.

Reproduction minimale : reprendre `managed_decision_and_worker_reopen` dans `tests/provider_authorization.rs:171`, avec ses canaris et son vrai Store ; supprimer uniquement l'affectation V2 à la ligne 193 afin de conserver le V1 de la fixture, en gardant toutes les autres liaisons request/receipt/argv/provider/binding/names. `AdmissionV1::validate` ne dispose d'aucun prédicat qui rejette cette variante ; le chemin d'admission peut la réserver comme les anciennes admissions V1. Ajouter une assertion explicite de refus avant réservation et contrôler l'absence de quota/worker créé.

Impact : contrat d'admission incohérent, réservation/quota conservé pour un nouveau parcours qui ne pourra pas démarrer, et acceptation durable d'un preset ne satisfaisant pas l'exigence NoLogs de cette nouvelle feature. **Pas de premier start V1 ni fuite moteur établis** : `src/runtime.rs:116` refuse la création sans NoLogs ; `src/runner/managed.rs:1424–1447` refuse aussi le premier start du preset historique. La CLI normale construit V2/V3 (`managed.rs:1296`), ce qui réduit encore la portée au contrat serveur.

Correction attendue : pour `provider_resolution: Some`, exiger un preset NoLogs compatible avec le transport ; conserver la lecture/reprise historique `None` et ses hashes. Régression : admission gérée V1 refusée, V2 batch/V3 TTY acceptés, anciennes admissions historiques toujours relisibles selon leur politique de reprise existante.

## PRV-02 — P2 : configuration relative acceptée par status, refusée par run

**Confirmé par lecture**, défaut de parcours sans fuite ni contournement prouvé.

Lieu : `src/runner.rs:547–551` désérialise puis appelle directement `ProviderConfigV1::validate`. À l'inverse, `src/operations.rs:227` utilise `load_config`, qui ancre les références relatives au parent privé du fichier (`src/provider/selection.rs:119–131`). `validate_config` exige ensuite des chemins absolus (`selection.rs:81`).

Préconditions/reproduction : configuration privée valide avec un profil API et `credential.path = "credentials/provider.env"`, fichier canari privé à cet emplacement sous le parent de `providers.json`, projet distinct. `providers status --providers-file ...` accepte la configuration et le test core de chargement relatif est cohérent avec ce résultat. Le même fichier fourni à `run --client codex --auth-mode api --profile observe --network bridge --prompt ...` échoue à `providers.validate()` avec `credential source requires an absolute path`, avant tout accès réseau/admission.

Impact : une configuration officiellement prise en charge par le chargeur/status n'est pas utilisable pour lancer le parcours, alors que le rapport core annonce les références relatives prises en charge. Le refus est sûr ; aucun lookup CWD dangereux n'est observé.

Correction attendue : partager la normalisation depuis les octets du même `PrivateFile` retenu, tout en préservant le contrôle d'exclusion physique du fichier config et sa revalidation. Éviter de résoudre deux fois indépendamment un fichier mutable. Test runner : même profil relatif accepté depuis un CWD différent, traversée/symlink refusés, substitutions toujours refusées avant admission.

## PRV-03 — P2 : fonction publique sûre avec précondition d'absence de threads non portée par son API

**Confirmé par lecture du contrat Rust ; aucun crash/UB exécuté.** Pas d'exploitation du binaire livré démontrée.

Lieu : `src/provider/bootstrap.rs:513–521`. `pub fn run_from_environment` est appelable depuis la bibliothèque avec une API Rust sûre, mais exécute `unsafe { std::env::remove_var(...) }`. Son commentaire affirme que l'appelant est un entrypoint autonome sans thread ; rien dans la signature publique ne garantit cette précondition.

Cas minimal : un consommateur de la bibliothèque peut appeler cette fonction dans un processus déjà multithread pendant qu'un autre thread ou une bibliothèque native consulte l'environnement. L'hypothèse qui justifie le bloc unsafe n'est alors pas assurée. Le problème existe avant même une erreur de parsing ou d'ouverture du home, puisque la mutation précède ces opérations. Il n'est pas nécessaire de lancer un fournisseur pour atteindre cette mutation ; une tentative de démonstration d'UB n'est pas un test de non-régression fiable.

Impact : frontière de sûreté mémoire/processus mal exprimée pour les consommateurs de la bibliothèque. Le binaire actuel `src/bin/ai-guard-provider.rs` appelle effectivement avant la création du runtime, donc ce constat ne signifie pas que le parcours CLI livré est multithread à cet instant.

Correction attendue : réaliser la suppression dans le `main` autonome avant tout thread et passer le payload explicitement au code bibliothèque, ou rendre l'API explicitement unsafe avec son contrat complet. Une simple doc sur une fonction sûre ne suffit pas à porter cette obligation. Vérifier ensuite que les clients lancés reçoivent toujours un environnement reconstruit sans variable bootstrap.

## Contrats recoupés sans défaut bloquant confirmé

- Sélection : choix explicite/profil cohérent ; ambiguïté refusée ; abonnement sélectionné manquant refusé sans tentative API. Les canaris des deux fournisseurs ne se mélangent pas dans le payload. Cela ne prouve pas encore l'authentification native effective.
- Liaison publique : résolution, client/auth/transport/preset version/profil/révision/modèle/UUID et noms figés participent au hash ; `validate_argv` compare la forme canonique. L'admission compare la résolution exacte à la demande. Le lookup/replay audit relit un type validé et refuse les valeurs null/inconnues au lieu de rétrograder en historique.
- Liaison privée : le plan3 conserve le binding privé et son digest ; approbation ne résout pas Auto à nouveau. Reprise avant premier start exige les mêmes octets et le digest admis ; claim consommé n'est pas réarmé. Le masquage optionnel lors de lecture/stop/export peut ignorer un binding absent, tandis que la voie de nouveau start l'exige toujours.
- Confidentialité des structures : pas de valeur/digest/path privé dans `ProviderResolutionV1`; `ProviderCredential`, `BootstrapExecution` et `PrivateFile` ont des Debug opaques. Le fichier contenant payload et dictionnaire est privé 0600, sous le contrôle retenu, avec digest privé ; sa rétention reste un écart documenté.
- Sources : `PrivateFile` borne, contrôle les métadonnées et retient les identités ; le handoff utilise les octets photographiés, pas un reopening tardif de la source tournée. Le runner limite les credentials internes au projet à la racine `.env` exclue ; il est plus strict que le core pour `.env.*`, sans créer une voie d'export démontrée. Les credentials de contrôle inclus dans les valeurs fournisseur sont refusés.
- Bornes : le payload env est vérifié à 64 KiB avant la décision. Pour les nouveaux profils, les tokens Codex cumulés sont bornés par le JSON compact de 48 KiB, le payload par 64 KiB, plus au maximum trois secrets contrôle de 4 KiB : la somme reste sous les 192 KiB/1024 motifs de `DisplayFilter`, même si sa construction intervient plus tard. Aucun dépassement admis du dictionnaire n'est démontré pour ces nouveaux profils.
- Bootstrap : argv fermés séparés, chemins absolus des binaires, environnement reconstruit, seules valeurs du mode choisi, état neuf privé hors workspace, aucun shell hôte. Sondes sorties bornées, timeout et diagnostics fixes ; login API stdin privé sans capture persistée ; environnement bootstrap absent des commandes natives. Les secrets restent accessibles au code worker, ce qui est une limite explicite du modèle.
- Image : build natif isolé puis copie du binaire, pas de copie des références privées/credentials ; aucun nouveau privilège/socket/montage ajouté par ce Dockerfile. Lecture seule, build de l'image **NOT_RUN** par cette revue.

## Limites et acceptation restante

Les huit combinaisons des tests sont des préparations canaris. Les quatre chemins d'authentification dynamiques réels, leurs transports, l'image construite et le refus adverse des routes/settings projet sur les versions natives réellement épinglées restent **NOT_RUN** ici. Les sondes acceptent une forme de version et la présence de flags ; elles n'attestent pas les effets de `forced_login_method`, des routes CLI ou des sources de settings. Leur vérification documentaire ne remplace pas le test adverse du client exact.

Le doctor de l'image arrive après admission ; l'import/login/status opérateur complet et la rotation/refresh OAuth restent ouverts. Des tokens rafraîchis inconnus du dictionnaire initial ne sont pas couverts par le masquage littéral initial. Ni médiation transitive, ni abonnement/API interchangeable, ni qualification de conversation ne sont déduits des tests purs.

Les réserves antérieures ne changent pas : appels async d'audit/inspection attendus dans la boucle runtime pouvant retarder les deadlines/pump, absence de timeout autonome après SIGKILL broker, absence de témoin simultané d'isolation face à un autre worker dans les preuves TTY considérées. Elles ne sont pas présentées comme de nouvelles régressions de cette feature.

Acceptation de cette candidate : **revue source effectuée, P2 ouverts ; qualification runtime/provider non acquise**. Une correction devra être recoupée sur un nouveau gel et accompagnée de régressions ciblées, en conservant les anciens résultats et limites.

## Addendum — clôture ciblée PRV-01 / PRV-02 / PRV-03

Date : 30 octobre 2026. Nouveau gel `/tmp/ai-guard-provider-prv-fixed.vWMHOh`, comparé au premier `/tmp/ai-guard-provider-review.fL91yK`. Le verdict précédent reste celui de son gel ; **les trois P2 sont désormais fermés sur le nouveau gel**, par lecture du delta et recoupement des preuves logicielles ciblées. Aucun nouveau bloqueur confirmé dans ces trois corrections.

Manifest courant `candidate.sha256` : `470459bb3645ccdd7410c6014d26bd5a9ee75aa6c42e99b2b3f48db04da432e5`. Contrôle effectué par le relecteur : les 22 entrées de `shasum -a 256 -c candidate.sha256` concordent. Aucun test, moteur ou fournisseur exécuté par cette relecture ; aucune source modifiée et aucun secret réel ouvert. Les nouvelles consignes de gouvernance ont été prises en compte ; elles n'autorisent pas à présenter les parcours non exécutés comme qualifiés.

| Constat | Correction recoupée | Verdict |
|---|---|---|
| PRV-01 | `src/control.rs:436` exige maintenant `requires_no_engine_logs()` uniquement sous `provider_resolution: Some`. Le contrôle du transport reste appliqué. `Store::admit_worker` appelle `admission.validate()` avant sa mutation (`src/audit/control.rs:615`) : le V1 géré est donc refusé avant réservation, tandis que les admissions historiques `None` ne changent pas de contrat. La régression `legacy_preset` dans `tests/provider_authorization.rs` fait varier uniquement le preset d'une admission gérée par ailleurs valide. | **FERMÉ**. Reproduction antérieure avec vraie admission acceptée conservée dans le rapport de reproductions ; nouvelle régression et hash/JSON historique PASS dans le log post-correctifs. Aucun lancement moteur V1 n'avait été démontré. |
| PRV-02 | `src/runner.rs:547` appelle `provider::load_config_source` sur le même `PrivateFile` déjà contrôlé physiquement. `src/provider/selection.rs:110` revalide ce handle, normalise depuis son parent, valide puis revalide ; `load_config(path)` délègue au même code. Aucun second fichier indépendant ne fournit les octets. Le test runner prépare réellement le plan et son handoff canari depuis un chemin relatif, avec un CWD distinct, et vérifie l'exclusion de l'autre clé. | **FERMÉ**. RED runner conservé ; 11 tests runner PASS rapportés avec hash correspondant. Le test core de source retenue et substitution est aussi PASS dans le log post-correctifs. |
| PRV-03 | `src/provider/bootstrap.rs:513–525` lit l'environnement sans le modifier ; le bloc `unsafe remove_var` a disparu. Les sondes, login et exec final conservent chacun `env_clear()` suivi de l'allowlist reconstruite (`:301`, `:450`, `:541`). La variable bootstrap ne fait partie d'aucune de ces listes. | **FERMÉ**. RED de la régression isolée conservé dans le rapport core ; GREEN recoupé dans le log post-correctifs. L'environnement du processus appelant reste intact sur erreur, sans précondition mono-thread cachée liée à une mutation globale. |

Pour PRV-03, conserver l'entrée héritée dans le bootstrap jusqu'à `exec` n'est pas une nouvelle promesse de confidentialité : les credentials sont déjà accessibles au code worker et aucune garantie d'effacement mémoire ou `/proc` n'était établie. Le nouveau commentaire le dit explicitement. La régression lance un processus de test Rust isolé, ne provoque pas d'UB et n'appelle aucun client fournisseur. Elle prouve l'invariance de l'environnement sur erreur ; l'absence de la variable chez les descendants est également recoupée dans le code `env_clear`/allowlist, sans prétendre à une preuve native supplémentaire.

Empreintes des fichiers de correction :

```text
7f2422c1bd507f66793b03085310e4b433e90c9feb0909b834f02711448b05b6  src/control.rs
57e2ed46cd65499ff6923ab2d45e750b53e40d83677e4795fe9d18f125275fb6  src/runner.rs
5ea5a80ab74c51fc9d66a633e2202031569769591a749bd8a6473ba2058436b9  src/provider.rs
f478c5dd7bd1378f71015c616715be36b5755871d90c7e2af49f90a6e1a3b0da  src/provider/selection.rs
3a74372d2fd8be2e90218dae6d8b359037de589c204311ee48216543d9085921  src/provider/bootstrap.rs
63edd7a7215d19a4c4a4571ee7c964d91bec0d8d918f76d389b65011d9ec76ad  tests/provider_authorization.rs
86de4f8219510e621213461f67b07f35cb00ee9ecacff60e16e06c92e15fc637  tests/provider_config.rs
ca8a3a3c1eddfbf86cbf639c3f51a4dc1478724f1831ed32516546a7a99037d4  tests/provider_bootstrap.rs
```

Preuves relues : rapports `ai-guard-provider-review-repros.md`, `ai-guard-provider-core-prv02-prv03.md`, addendum runner et `ai-guard-provider-post-review-software.md` dans le gel ; logs originaux `tests.log`, `fmt.log`, `clippy.log`, `build.log` et manifest après vérifications dans `/tmp/ai-guard-provider-post-review-logs`.

Le log ciblé établit **25 PASS, 0 FAIL, 1 fixture ignorée** : authorization 6, **config 9**, **bootstrap 6 + 1 ignorée**, runner 4. Le rapport logiciel copié dans ce gel inverse les libellés config/bootstrap et attribue l'ignore au mauvais fichier ; erreur documentaire signalée à root, sans changer le total ni le résultat. La fixture ignorée est le helper appelé par la régression isolée, pas une qualification fournisseur réussie. Les logs Clippy all-targets/release se terminent avec succès ; fmt est vide et son exit 0 est consigné par le rapport d'exécution. Le manifest initial de cette campagne était vide suite à une erreur d'invocation : aucune comparaison avant/après de cette campagne n'est inventée. Le manifest après campagne et les hashes du nouveau gel concordent.

La campagne complète historique 388 PASS / 8 ignorés demeure attachée au gel antérieur, sans extrapolation aux correctifs. Les authentifications natives, route/settings projet adverses, refresh OAuth et login abonnement restent non qualifiés par ces preuves. Les réserves runtime sur les awaits d'ACK/inspection et les autres limites préexistantes restent ouvertes. **La réserve flood du harnais était déjà fermée et n'est pas rouverte ici.** La fermeture des trois P2 ne qualifie ni la release ni les quatre parcours fournisseur.
