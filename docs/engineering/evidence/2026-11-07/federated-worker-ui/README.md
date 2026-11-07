# Vue humaine workers/brokers — 7 novembre 2026

**59 fonctions frontend uniques PASS sur le gel `federation-ui-fix03` ;
trois scénarios navigateur PASS sur le gel `federation-ui-browser45`.**
Le typecheck TypeScript 5.9.3 du gel03 est RC0. Sa sélection principale comporte 56 PASS et
trois SKIP faute de réponses RAW Rust ; ces trois fonctions passent ensuite
avec le handoff RAW contrôlé dans la même QA et sur le même gel frontend.
Les deux oracles Node `headers403` sont déjà inclus dans les 56 PASS. Le build
Vite du gel42 passe en QA AI ; le gel45 conserve exactement ses sources
applicatives et corrige seulement le harnais navigateur. Les résultats03 ne
sont pas présentés comme une réexécution45. Aucun gate production n'est clos.

## Source, contrat et revue

Base du worktree `codex/federation-ui-20261107` :
`a419df947075266e20c3a3427ec2e1c03ee75d38`.
Root a figé la source readonly dans
`/private/tmp/ai-guard-audit-followup-inputs-20261106.xu8TFf/inputs/federation-ui-fix03/`.
Le [manifeste historique03](source03.sha256) conserve les 43 empreintes du
frontend complet de ce gel, dont les neuf ancres package/source/tests du
manifeste court. Le [manifeste courant](source.sha256) reprend les 43 entrées
figées de `federation-ui-browser45-build-files.sha256`, puis les trois guides
API/Workers EN/FR synchronisés après les essais, soit 46 entrées. Seul
`front/tests/browser_human.mjs` diffère entre les manifestes frontend03 et45 ;
les sources applicatives sont identiques. Les documents et cette page de
preuve ne sont pas présentés comme des entrées exécutées par les tests.

L'onglet **Fédération** exige le même contexte administration proposé par Rust
avec `read_worker_registry` et `read_broker_registry`. Il lit uniquement
`/api/v1/human/federation/workers`, sans accès moteur ni credential runner.
L'ancien onglet Workers reste disponible sur son seul droit existant.
Le [lot API](../federated-worker-view/README.md), le
[contrat](../../../../api.md#composition-humaine-workersbrokers--human-federation-registry-composition)
et les guides [EN](../../../../setup/identity/WORKERS.md#compose-worker-and-broker-pages)/[FR](../../../../setup/identity/WORKERS.fr.md#composer-les-pages-workers-et-brokers)
définissent les droits et la composition côté contrôle.

Le DTO V1 fermé conserve `source:tenant_federation_registry`,
`consistency:independent_snapshots`, `coverage:partial` et
`runtime_inventory:unknown`. Workers garde `snapshot_per_page`, brokers
`live_keyset` ; chaque page conserve son évaluation, son reçu et sa pagination.
Le client borne chaque projection à 256 Kio et le transport composé à
528384 octets. Il valide les deux contextes/versions avant de publier ensemble
les pages. Les positions ne changent qu'après succès de toute la composition.

Les tableaux et détails existants montrent aussi les brokers sans association
dans la page workers. Les liens restent des associations historiques, sans
preuve d'exécution par un broker physique. Unseen, bail périmé ou révocation
ne deviennent ni zéro activité, ni arrêt de worker, ni libération de quota.
Les deux reçus décrivent des lectures enregistrées, pas leur livraison ou un
effet runtime. Aucune consommation réelle, facture ou propriété personnelle
n'est inventée. L'interface conserve le français existant ; la traduction
globale anglaise reste un écart ouvert.

Root rapporte la revue source indépendante finale acceptée après correction
du blocage403 ci-dessous, puis sa lecture du correctif d’horloge du harnais45.
Le [suivi d’audit](../../../AUDIT_FOLLOWUP.md#audit-final-run-1) conserve ses
constats et limites ; ce lot n’en réécrit aucun verdict historique.

## RED 403 et correction bornée

Au gel `federation-ui-headers403-repro02`, l'oracle public attend le refus 403
après livraison de ses headers et un tour de boucle événementielle, sans
libérer le corps ni l'accusé de cancellation. Il échoue réellement à la ligne
165 : `undefined !== 403`, 0 PASS/1 FAIL. Le client attendait le corps d'erreur
avant de rejeter ; le retrait des anciennes données pouvait ainsi attendre la
fin du corps. Ce RED est conservé séparément du premier gel UI.

Fix03 ajoute le refus immédiat des 403 courants pour **identity et le nouvel
endpoint Fédération seulement**. Le diagnostic fixe, borné, indique que le
motif détaillé n'a pas été lu et propose de relire les accès/vérifier les
permissions. Son code reste `permission_denied`, sans fabriquer une révocation
de contexte. La cancellation n'est pas attendue. Le catch Fédération purge
ses pages, positions et détails ; une revalidation identité refusée purge le
contexte affiché. Le credential humain est conservé, contrairement au 401.
Les générations et annulations existantes empêchent la republication tardive.

Les deux tests `headers403` sont byte-identiques entre RED02 et fix03 :
`human-federation.test.mjs` conserve
`91fdd21f22394f751cb35e0046a619cd24663259068b704cc477e3f56a0334e6`.
Le harnais navigateur03 historique porte
`86066874e04431d3a557386a1c389c5349e060a00ab8243c17262848b5112de7`.
Son gel n’avait aucune exécution navigateur créditée. Le gel42 active
`chromiumSandbox:true` et retire l’argument `--no-sandbox` ; le gel45 ajoute
uniquement la cohérence de l’horloge de fixture décrite ci-dessous. Le harnais
final porte `0142106286b938a0f79ba39c06349c51ea67fba0b47248fb31b68c061a408ffd`.
Les deux GREEN Node prouvent l’ordre de notification du client et la
conservation du credential ; les preuves DOM/navigateur sont distinctes.

**Écart restant : les autres routes 403 dépendent encore de leur corps.**
Leurs erreurs structurées, dont `context_revoked` explicite du Workers
historique, les exports et SSE n'ont pas été refondus dans ce correctif.
Aucune fermeture générale du retrait immédiat des droits n'est revendiquée.

## Résultats observés et comptage

Luna a exécuté les sélections ciblées dans QA AD ; Root a ensuite réalisé le
handoff RAW dans cette même QA. Les journaux conservent les noms et sorties
réels, mais leurs en-têtes parent ne sont pas un argv complet. Les codes
terminaux RC0 ci-dessous ont été vérifiés et rapportés par Root/Luna ; le
journal du typecheck est silencieux après son en-tête, sans diagnostic TS.

| Sélection / gel | Résultat | Durée observée | Clé du journal |
|---|---|---:|---|
| RED02, `headers403 federation` | 0 PASS, 1 FAIL | 133,60 ms | red |
| fix03, les deux `headers403` | 2 PASS, RC0 | 98,56 ms | headers |
| fix03, sélection principale B | 59 tests : 56 PASS, 3 SKIP, RC0 | 1141,77 ms | unit |
| fix03, TypeScript 5.9.3 `--noEmit` | RC0, aucun test ajouté | non consignée dans ce log | typecheck |
| backend `federation-fix03`, parents `legacy_` | 3 PASS, 14 filtrés, RC0 | build 0,57 s ; tests 1,14 s | raw-rust |
| UI fix03, `human-workers-wire.test.mjs` avec RAW | 3 PASS, 0 SKIP, RC0 | 66,74 ms | raw-node |

B couvre `human-api.test.mjs`, `human-federation.test.mjs`,
`human-model.test.mjs`, `human-workers.test.mjs` et
`human-workers-wire.test.mjs`. Les deux PASS headers sont un sous-ensemble de
B, pas deux fonctions supplémentaires. Le handoff exécute exactement les trois
fonctions RAW auparavant ignorées : **56 + 3 = 59 fonctions frontend uniques
PASS**, pas 61. Les trois parents Rust sont une réexécution de parents déjà
comptés dans les 17 + 2 du lot backend ; ils ne portent pas ce compteur à 22.

Le premier gel `federation-ui-fix01` conserve son résultat historique distinct :
57 tests, 54 PASS, 3 SKIP. Ces PASS ne sont ni transférés au gel03 ni additionnés
à ses résultats. Le journal de réexécution Rust émet 119 avertissements
bibliothèque ; aucun build sans avertissement ni résultat Clippy n'est déduit.

Les commandes suivantes sont des **équivalents de reproduction dans la QA
admise**, pas des commandes exécutées par cette passe documentaire ni une
reconstruction d'argv absent des logs. Les commandes Node supposent le
répertoire `front/` du gel readonly concerné.

```sh
node --test --test-isolation=none --test-name-pattern=headers403 tests/human-federation.test.mjs
node --test --test-isolation=none tests/human-api.test.mjs tests/human-federation.test.mjs tests/human-model.test.mjs tests/human-workers.test.mjs tests/human-workers-wire.test.mjs
node /inputs/tool-typescript-5.9.3/bin/tsc --noEmit -p /inputs/federation-ui-fix03/front/tsconfig.json
```

Le handoff utilise
`AIGUARD_HUMAN_WORKERS_WIRE_DIR=/scratch/human-worker-wire-20261107-ad`
pour les trois parents Rust `--test human_workers_api legacy_`, puis uniquement
`human-workers-wire.test.mjs` côté Node, avec la même variable et le même gel
UI03. Le target backend déjà chaud est
`/scratch/target-federation-fix03-ad`. Les vraies réponses HTTP Rust vérifient
UUID nil historique, date runtime inconnue typée et code i64 exact
`9007199254740993` dans le parseur produit. Leurs JSON sont restés dans le
scratch privé, sans promotion dans Git ; leur suppression avec la QA est
rapportée par Root. Aucun handoff RAW de l'enveloppe composée n'est revendiqué.

## Build et navigateur : gels42 et45

Les essais suivants utilisent le même programme applicatif que03 et des
fixtures HTTP loopback, sans gateway réel ni fournisseur. Les blocages du
sandbox Chromium précèdent tout oracle produit ; ils ne sont pas des PASS.

| QA / gel | Résultat observé | Durée | Pièce privée |
|---|---|---:|---|
| AG /42 | Build RC0 ; navigateur BLOCKED, namespaces/sandbox indisponibles | build5s ; navigateur0s | `federation-ui-browser42-{build,federation}-ag.log` |
| AH /42 | Build RC0 ; navigateur BLOCKED, refus `sys_chroot` | build3s ; navigateur1s | `federation-ui-browser42-{build,federation}-ah.log` |
| AI /42 | Smoke sandbox RC0 ; typecheck + Vite7.3.7 RC0 | 3s +3s | smoke, build-ai |
| AI /42 | Scénario `federation` PASS | 3s | federation42 |
| AI /42 | `federation-403-headers` FAIL à la reprise fraîche ; défaut d’horloge de fixture | 9s | headers42 |
| AI /45 | `federation` PASS, RC0 | 4s | federation45 |
| AI /45 | `federation-403-headers` PASS, RC0 | 1s | headers45 |
| AI /45 | `federation-403-headers-identity` PASS, RC0 | 1s | identity45 |

Le FAIL42 survient après les oracles de retrait des pages/détails/positions
avant libération du corps403. Le journal conserve une reprise HTTP autorisée,
sans aucun des deux curseurs, et les données restaurées avec « Données à
actualiser ». Le harnais avançait `Date.now()` de31s mais la date de lecture
applicative venait de `new Date()` natif : la lecture fraîche apparaissait
immédiatement vieille de plus de30s. Le correctif45 synchronise `Date.now()`,
`Date()` et la construction sans argument sur la même horloge de fixture ;
les constructions explicites, parse/UTC et les timers natifs sont conservés.
Aucun comportement applicatif ni délai d’oracle n’a été modifié. Le premier
run42 omettait aussi le cwd réel alors que son en-tête annonçait
`/scratch/front42` ; le script était invoqué par chemin absolu. Ce défaut de
compte rendu est conservé, sans lui attribuer la cause du FAIL.

Les trois exécutions45 partent de `/scratch/front42`, avec le bundle42
inchangé et le harnais45 contrôlé par son SHA ; Root a vérifié l’option cwd
effective. Les deux scénarios headers403 conservent les plafonds de1s après
les headers, le corps laissé ouvert, le retrait des détails et pages, la
conservation du credential, la reprise « Lecture réussie » et les deux
curseurs remis à zéro. Le scénario identité vérifie aussi l’absence de
résurrection par200 tardif. Le scénario général couvre pagination
indépendante, erreurs503, vide/dense, provenance, retour du focus et retrait
des capacités.

Les en-têtes abrègent l’enveloppe de timeout (`timeout120s:`) ; ils ne sont
pas un argv shell littéral. Les commandes cibles ci-dessous sont les appels
Node à reproduire **uniquement dans la QA admise**, cwd `/scratch/front42`,
avec l’environnement vide/PATH fixe, les outils offline préparés et les
plafonds parent180s pour le build,120s par scénario :

```sh
node node_modules/typescript/bin/tsc --noEmit
node node_modules/vite/bin/vite.js build
AI_GUARD_HUMAN_SCENARIO=federation AI_GUARD_HUMAN_ARTIFACTS=/scratch/artifacts-federation45 node tests/browser_human.mjs
AI_GUARD_HUMAN_SCENARIO=federation-403-headers AI_GUARD_HUMAN_ARTIFACTS=/scratch/artifacts-federation40345 node tests/browser_human.mjs
AI_GUARD_HUMAN_SCENARIO=federation-403-headers-identity AI_GUARD_HUMAN_ARTIFACTS=/scratch/artifacts-identity40345 node tests/browser_human.mjs
```

Root rapporte14PNG conservés uniquement dans le scratch QA du scénario45,
sans copie hôte ni ajout Git. La pièce `ui45-visual-metadata-ai.log` conserve
pour chacun type régulier, nombre de liens1, taille et SHA. Root a examiné
en mémoire les trois rendus ci-dessous, pas l’ensemble des14captures.
Le scénario vérifie aussi le
débordement global aux largeurs1440/390/320px et les commandes hors du contenu.
Ces contrôles et captures sur fixtures ne constituent ni audit WCAG complet,
ni preuve Web contre une authentification locale/OIDC réelle.

| Capture relue sous `/scratch/artifacts-federation45/` | SHA-256 |
|---|---|
| `human-brokers-federation-dense-1440-dark.png` | `79238fb1500b2d9240939a959673580632d6079ff92d7dd26e20ddd3d5417159` |
| `human-brokers-federation-dense-320-light.png` | `5442e11815ce3b1a0832e9a92ced56d594715d77e41b53f6c06cda0f253ccd3e` |
| `human-brokers-federation-forbidden-320-dark.png` | `8f7a05e202f12041cb4d16cb2bdad026b5dd22cb773887a60185a60a5aee221e` |

## Manifestes et pièces privées

Les manifestes d'entrée sont sous
`/private/tmp/ai-guard-audit-followup-inputs-20261106.xu8TFf/inputs/`.
La passe documentaire03 consignait la concordance des entrées frontend et
des132fichiers du package TypeScript5.9.3. Cette mise à jour recalcule les
empreintes des manifestes, vérifie les43entrées frontend45 contre le worktree
et les46entrées du manifeste final, et conserve le manifeste03 à l’identique.
Ces contrôles de fichiers restent distincts du build42 AI ci-dessus.

| Manifeste | Entrées | SHA-256 |
|---|---:|---|
| `federation-ui-fix03-files.sha256` | 9 | `c498cea92ea3a8f8eede294d27b9557d8958f2ae7b8a527bd206f29a5ba7ac4f` |
| `federation-ui-fix03-build-files.sha256` | 43 | `b3969c4f44a3b2796129bfdaa4e46115e8c612d7cb44cf747f9a528318977e2d` |
| `federation-ui-browser45-build-files.sha256` | 43 | `55cf014b452921c04622de3de0947532a1c779995595e3dfa2d8883542afe997` |
| `federation-ui-headers403-repro02-files.sha256` | 9 | `bb89ace0078893539c6952636093d5e60000d8e3252f82c3935fcb7b53d4a355` |
| `tool-typescript-5.9.3-files.sha256` | 132 | `114c4dd5125edfece5647eaf308005cbe3d4c97ff8709204010bc09b8726a444` |
| `federation-fix03-files.sha256` | 8, backend | `da4cebe1e85c5fe5edb2031128b7633f613ad9a1f451bf6543065462c43f55a3` |

Les pièces suivantes restent hors Git dans
`/private/tmp/ai-guard-cold-drain-logs-20261106.nXx9XJ/`.
Les résultats AD restent ceux de la passe historique03. Cette mise à jour a
relu les sorties42/45, l’inspectionAI et les métadonnées PNG/processus, puis
recalculé les empreintes des pièces citées.
Aucun journal brut, JSON RAW, base, credential ou exécutable n'est copié ici.
Ces empreintes ne constituent pas un archivage privé durable.

| Clé | Pièce | SHA-256 |
|---|---|---|
| historique 01 | `federation-ui-fix01-unit-ad.log` | `534ec251d9dcdaf9a211e61c2aa0531794d13d243a03066b17c6b3eefd9d4c03` |
| red | `federation-ui-headers403-repro02-ad.log` | `00f4e9168798c803671dc6d8b079560a5b8a8ed22da0c8bdb436d3df9decfed9` |
| headers | `federation-ui-fix03-headers-ad.log` | `60a21228d081f103a9571b69e17381b51be5177898267749350d2715dcb5912d` |
| unit | `federation-ui-fix03-unit-ad.log` | `a329daf79befc2601131bb88b86d83f284ae950af858707613e36d3d096d10fb` |
| typecheck | `federation-ui-fix03-typecheck-ad.log` | `a51b37dd6e88ffcd4cd80a12658108a06b191bf8723d3e89007fc4e02cc8e5f7` |
| raw-rust | `federation-wire-rust-ad.log` | `141af115964c94ce2809cdd2bc3e36f8a59479c32320a6d330570a48ded1d94a` |
| raw-node | `federation-wire-node-ad.log` | `a42af044184aa264c58ff6e135250085abf8a5699165f9302c2997fa2bf45391` |
| inspection initiale | `qa-ad-inspect.json` | `6b994d50362e8f50056ce0c676a3d85ab0b5e7c68bb43ce3549075e755ab1d43` |
| inspection finale | `qa-ad-final-inspect.json` | `9b86bddbdf5a849c48ad782871415b749cd7ce40374d486bb28da782e7b2b633` |
| AG bloqué | `federation-ui-browser42-federation-ag.log` | `02f65491b29bd7f7b218f3697ad49e24fae12bcb4ea151be1e349694f8b4767a` |
| AH bloqué | `federation-ui-browser42-federation-ah.log` | `c13b0cc607e6c72dddbe6fc4fac4edf65624ab7ace4bccaf1009b4c1ab2e77b8` |
| smoke | `frontend-ai-sandbox-smoke.log` | `f4059c76e1ea48faba11ad97a5492ac3babe1f1064c183b81bdc76af28349567` |
| build-ai | `federation-ui-browser42-build-ai.log` | `45d92180e12e5aa93585e1a706bd3c4b4defbd16faeb8741a5818c6e79797816` |
| federation42 | `federation-ui-browser42-federation-ai.log` | `1a27a897f1467a34bc6d44b70afd7291760045d3c1d50853df62ccfe20276b48` |
| headers42 | `federation-ui-browser42-403-ai.log` | `31aa679bb17ea516c89f3575bec581528226b59ba443f350e8612a2fb4e44e00` |
| federation45 | `federation-ui-browser45-federation-ai.log` | `dbbbbb6e7a651c432876bdd51ee588715807a57430c736495764dc538c919422` |
| headers45 | `federation-ui-browser45-403-ai.log` | `d18be4baaffea95b34c6ac8e523c3ed5e6ca0b4e56766df314d9bee24d462fba` |
| identity45 | `federation-ui-browser45-identity403-ai.log` | `41e9915c66ae2f784fbc6580ef15ff9ac8b45caae370d6a9b5d4e6853c1d3ff6` |
| inspection AI | `qa-ai-inspect.json` | `48c01e47916446132c57d0f3355208445983b46ec1a970186e899c18423dbb13` |
| métadonnées AI/PNG | `ui45-visual-metadata-ai.log` | `a9eefe3ec742357c270c2d7f0333f396cc2b8424f0312319fbe0be7d9d9c86f1` |

## Environnements et limites

QA AD identifie le CID
`a249cc050ac1e253781444070f789bbd2cbf3cede30dcb2547baf59a7d5b8b79`,
plateforme Linux, image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`,
UID/GID 1000, réseau `none`, aucun port publié, racine et montages sources/
registre readonly, capacités ALL retirées, non privilégié et
`no-new-privileges`. L'image déclare Node 24.22.2 ; cette passe ne sonde pas
les versions par exécution. L'entrypoint `env -i` à PATH fixe et le profil Root
documentent l'environnement vide/offline, sans inspection nouvelle de chaque
processus de test.

Les limites configurées sont 4 CPU, 8 Gio de RAM sans swap additionnel,
256 PIDs, scratch 4 Gio/65536 inodes, 512 Mio par fichier, 1024 FD et aucun
core dump. Ce ne sont ni des besoins produit mesurés ni une qualification de
confinement worker. Root rapporte un arrêt volontaire après les quatre
campagnes, puis la suppression du CID exact. L'inspection finale constate
`exited`, code 137, `OOMKilled:false` ; elle n'établit pas à elle seule la cause
de l'arrêt. Ce code du conteneur n'est pas le code terminal des tests.

La QA navigateur AI est distincte : CID
`fb025b365f35a6128c1bcc5e3ef1f370149a6a7656bb9f074a41737ff3a30129`,
image inspectée
`sha256:2dee255e66f9985e1623c12a6f77535bb7487e7bd00a0b0ad82f98688ae60f4d`,
référence publique épinglée
`mcr.microsoft.com/playwright@sha256:f609973e8fd93f38a5ddfd706a65805294247dae8d81b2541c0afe2787746197`.
L’inspection conserve UID/GID1000, réseau `none`, aucun port publié, racine
readonly, sources et cache public montés readonly, capacités ALL retirées,
aucune ajoutée, non privilégié et `no-new-privileges`. Le scratch privé est
limité à3Gio/65536inodes ;1CPU,2Gio RAM sans swap additionnel,256PIDs,
256Mio par fichier,1024FD et zéro core. Ces limites ne sont pas des besoins
produit mesurés. L’inspection initiale constate `running`, `OOMKilled:false` ;
elle ne constitue pas une inspection finale ou une preuve de nettoyage.

Disposition finale constatée ensuite par le parent : AI est arrêtée volontairement
à `2026-11-07T18:28:40.365Z`, code137 et OOMKilled=false, puis le conteneur exact
est supprimé. Les quatorze PNG restaient uniquement dans son scratch et sont
détruits avec celui-ci ; trois avaient été examinés en mémoire, sans copie sur
l'hôte. Journaux et métadonnées parent sont conservés. Inspection finale privée
SHA `d1ef05ec101843bde9cb4882b0dba08e9cd6ac40ceaba194ae9213c037f8793b`.
Ce code d'arrêt ne remplace pas les codes des tests et ne les invalide pas.

Le profil seccomp Playwright officiel1.64.1, SHA
`cc3e61cabda6bbc1e53e54d27ba4d55a9d3be829b6dd1a596f4a7b31b1cc7849`,
autorise les namespaces mais AH échoue encore sur `chroot`. AI emploie ce
profil avec seulement une règle d’autorisation `chroot` supplémentaire
(et reformatage JSON), SHA
`b44b83f4eb9655ed3f3201c670b72ed518dfef17730cdd7aa11325e17aececbd`.
Les deux fichiers privés sont dans
`/private/tmp/ai-guard-frontend-public-cache-20261107-1/`, noms
`playwright-v1.64.1-seccomp.json` et
`playwright-v1.64.1-seccomp-rootless-chroot.json`.
Le smoke ouvre et ferme réellement Chromium avec `chromiumSandbox:true`.
Aucun lancement sans sandbox ni capacité extérieure ajoutée n’est crédité.
Le relevé parent `ui45-visual-metadata-ai.log` constate UID/GID1000, les cinq
ensembles `CapInh/CapPrm/CapEff/CapBnd/CapAmb` à zéro, `NoNewPrivs:1`,
`Seccomp:2` et les compteurs cgroup `oom`/`oom_kill` à zéro dans le processus
extérieur. Cela ne remplace pas une qualification du confinement worker.

Les résultats ne qualifient pas un parcours Web local/OIDC réel, les transports
brokers multi-hôtes, la déconnexion/reprise physique, Docker/Podman/Kubernetes,
l'attribution personnelle/délégation, la capacité 100 workers ni une release
cloud. La vue à 360° complète et les autres routes 403 restent des écarts.
L'inventaire runtime et la consommation restent inconnus, l'allocation non
facturable. G09 reste FAIL et aucun
[gate global](../../../PRODUCTION_READINESS.md) n'est clos. Aucun Node, Cargo,
build, navigateur, moteur, réseau ou fournisseur n'a été invoqué pendant cette
passe documentaire.
