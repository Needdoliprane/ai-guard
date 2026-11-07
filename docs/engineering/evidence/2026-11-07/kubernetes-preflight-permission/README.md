# Permission create/pods du préflight Kubernetes — 7 novembre 2026

**25 fonctions logicielles uniques PASS au gel40**, avec compilation toutes
cibles RC0. Le RED38 établit l'absence du raccord local SSAR avant dry-run ;
il ne démontre aucun contournement de RBAC sur un cluster réel. Un enfant
interne ignoré par le harness principal n'ajoute pas de fonction PASS ou de
fonction produit manquante. Aucun gate production n'est clos par ce lot.

## Sources et périmètre

Base du worktree `codex/kubernetes-preflight-20261107` :
`c33ee6c63839dd4c473e1f882138a2e41cb096c1`. Sources validées avant publication ;
les références publiées sont consignées dans CURRENT_STATE/VALIDATION.
Le [manifeste](source.sha256) contient neuf ancres Cargo/code/tests du gel40,
puis les deux guides finalisés après les tests. Cette page, son manifeste et
les mises à jour des guides ne sont pas présentés comme des entrées des
exécutions historiques.

Au gel38, seul le fichier de tests unitaires était ajouté au comportement
initial. Son SHA-256
`064d0f6ef0ce4d6add900a56f70c35bfbf61e4410d778839f811306e493a4a20`
reste identique au gel40 : les tests RED n'ont pas été réécrits pour le correctif.
Les premières ancres applicatives étaient :

| Fichier | SHA-256 au RED38 |
|---|---|
| `src/runtime/kubernetes/prerequisites.rs` | `624b0a77d98c22bc0b2c056ab8aff82ccc15ca98b8994073152ae918e652c51f` |
| `src/runtime/kubernetes/preflight.rs` | `15f1099c1445141aa7e41d5d06b23525fcd11fe5cee699aadf3dd261c9ef5199` |
| `src/runtime/kubernetes.rs` | `0e68fc632e042c020fc220f8a389dca9576ad16ac27217ab72bfc1a66f3937e4` |
| `src/kubernetes_cli.rs` | `f77636bc7edd626579e8136a5b42ef754a56a637e7493805a6a41944d26dedc5` |

Le raccord réutilise les ancres NamespaceUID et le transport SSAR existants.
Le préflight vérifie d'abord les deux Namespaces actifs, demande uniquement
`create` sur `pods` core `v1` dans le Namespace retenu, puis relit les ancres
avant le guard et le POST Pod. La même identité Kubernetes, autorité HTTPS et
confiance TLS sont conservées. Aucun tenant libre, label worker, impersonation,
nouveau rôle, credential, service, crate ou stockage n'est introduit.

Seul `Allowed` sans erreur d'évaluation passe. Les avis Denied et Unknown,
dont une erreur d'évaluation, donnent les catégories fermées
`PreflightPermissionDenied` et `PreflightPermissionUnconfirmed`. Le CLI ajoute
seulement les codes d'audit `preflight_permission_denied` et
`preflight_permission_unconfirmed`. Les raisons distantes ne sont pas exposées.
Les refus du vrai POST sont toujours propagés, sans retry ni succès inventé.

Les deux fixtures d'intégration répondent seulement à la SSAR create/pods
exacte au Namespace `tenant-a`, après vérification du corps, des credentials
et de l'absence d'impersonation. Le peer CLI vérifie aussi l'audit demandé
durable avant la question. Ce sont des permissions de fixture déclarées,
pas un helper qui autorise arbitrairement toute demande.

Le chemin nominal compte huit requêtes : deux GET Namespace initiaux, une
SSAR, deux GET avant guard, un POST Pod et deux GET finaux. Le guard est
consommé après cinq requêtes ; un vrai POST rejeté termine après six. Le
`doctor` GET-only et son diagnostic opt-in de douze SSAR restent inchangés.
Held-CREATE, inconnues, quotas, réémission, start/release et factory d'activation
ne sont pas modifiés.

## Recettes et résultats constatés

Luna a exécuté les sélections dans la QA AE admise. Les commandes suivantes
sont des reproductions équivalentes des sélections enregistrées ; elles ne
reconstituent pas un argv complet absent des journaux. Chaque exécution reste
attachée à son gel et à sa cible de compilation.

```sh
# RED38 puis GREEN40 : trois parents, cinq scénarios métier.
cargo test --locked --offline --lib runtime::kubernetes::prerequisites_tests::preflight_ -- --test-threads=1 --nocapture
# GREEN40 : cibles d'intégration entières, pas seulement un helper.
cargo test --locked --offline --test kubernetes_transport -- --test-threads=1 --nocapture
cargo test --locked --offline --test kubernetes_preflight_cli -- --test-threads=1 --nocapture
cargo check --all-targets --locked --offline
```

| Gel/sélection | Résultat | Compilation / tests |
|---|---|---|
| RED38, trois parents préflight | RC101, 0 PASS / 3 FAIL, 592 filtrés | 1 min 51 s / 1,26 s |
| GREEN40, trois parents préflight | RC0, 3 PASS / 0 FAIL, 592 filtrés | 1 min 57 s / 1,32 s |
| GREEN40, transport | RC0, 13 PASS / 0 FAIL / 1 enfant interne ignoré | 51,39 s / 15,80 s |
| GREEN40, CLI | RC0, 9 PASS / 0 FAIL | 4,73 s / 4,54 s |
| GREEN40, toutes cibles | RC0, compilation seule, aucun test supplémentaire | 1 min 05 s |

Le RED38 compile et atteint les oracles, sans défaut de setup : aucune SSAR
sur le parcours Allowed ; remplacement UID non observé après avis puisqu'il
n'était pas demandé ; les trois cas Denied/NoOpinion/evaluationError atteignent
guard et POST Pod. Le vecteur réel est, pour chacun, avis 0/guard 1/Pod 1, au lieu
de refus/avis 1/guard 0/Pod 0. Le peer n'applique pas de RBAC réel au POST :
l'oracle mesure ce raccord local, pas un bypass du serveur Kubernetes.
Ces cinq scénarios appartiennent à trois fonctions,
pas à cinq tests PASS supplémentaires.

Au gel40, les trois parents prouvent le refus avant guard/Pod, la seule SSAR
exacte avant dry-run autorisé, et le refus du changement UID intervenu pendant
l'avis. Les treize fonctions transport conservent notamment refus du guard,
credentials changés, erreurs du vrai POST, absence de retries/redirections,
profil d'admission fermé et capacité libérée après annulation. Les neuf tests
CLI lancent réellement le binaire dans la QA contre un gateway/SQLite et un
peer TLS déclaratif : ordre des audits, identité serveur, refus viewer et
tenant libre, protection des secrets, substitution de fichier et résultat
inconnu après erreur restent vérifiés.

`proxy_environment_child` est ignoré par la sélection transport principale.
Son parent `ambient_proxy_variables_are_ignored_by_the_real_client` l'invoque
explicitement avec `--exact proxy_environment_child --ignored`, environnement
effacé, puis vérifie sa sortie. Ce helper interne ne constitue ni une
quatorzième fonction produit PASS, ni une fonction produit absente. Le total
est **3 + 13 + 9 = 25 fonctions uniques**, sans double comptage du helper ni
des scénarios internes.

Les journaux préflight signalent deux avertissements bibliothèque-test. Les
cibles transport et CLI signalent chacune **61 avertissements bibliothèque**.
Le check toutes cibles conserve **61 bibliothèque, deux
`human_sso_gateway_trust` et deux bibliothèque-test**. Les deux derniers ne
résument donc pas tous les avertissements du build. Aucun Clippy ou build sans
avertissement n'est crédité à ce lot.

## Pièces vérifiées et rétention

Les cinq journaux ont été relus et leurs empreintes vérifiées dans
`/private/tmp/ai-guard-cold-drain-logs-20261106.nXx9XJ/`. Ils restent privés ;
aucun journal brut, base, certificat, credential ou exécutable n'est copié dans
ce dossier public. Cette passe documentaire n'effectue pas leur archivage
privé durable.

| Fichier | SHA-256 |
|---|---|
| `kubernetes-preflight-repro38-ae.log` | `fcff8b67c00992f93a96eff1dc9704b2daea8509a1a70696dfefd69617aa2cd7` |
| `kubernetes-preflight-fix40-preflight-ae.log` | `a5cb82682d3fd9764ac1c4e6257c8f713c9b4519da78d0c27a3abca45f7b9a58` |
| `kubernetes-preflight-fix40-transport-ae.log` | `97e4c8fc15e36ca1d96935fad074a22b7c9e30d636d945bdb5d00fb8a462359a` |
| `kubernetes-preflight-fix40-cli-ae.log` | `22fd3286946af71f3d3ba178c9be897e25bd084b1818b5426cfbd5ad00587985` |
| `kubernetes-preflight-fix40-check-ae.log` | `eebaa2aa5ee1a0a6fc82a30c4ea2bc235ae2dad8229651ae148ff33518cdb9b6` |

La QA AE utilise l'image épinglée
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`.
Le profil admis Root efface l'environnement ambiant, emploie les dépendances
publiques verrouillées hors ligne et des sources en lecture seule. La pièce
`qa-ae-inspect.json`, vérifiée avec SHA-256
`76cc49fd2fd184907b30ae87deeab79860e8d0a703cb5bd6ddc8fff03cdc5a5b`,
confirme notamment UID/GID 1000, réseau `none`, racine readonly, quatre CPU,
8 Gio sans swap additionnel, 256 PIDs et scratch 4 Gio. Ces limites configurées
ne mesurent pas les besoins du produit et ne qualifient pas son confinement.
Aucun Cargo, build script, test, CLI projet, Docker ou cluster n'a été lancé
par l'agent chargé de cette passe documentaire.

## Limites et critères encore ouverts

Une SSAR est un avis pour l'identité, l'action et le scope interrogés à cet
instant, pas une autorisation atomique du POST futur. La relecture des ancres
détecte certaines divergences sans empêcher un remplacement ou une révocation
après la dernière observation. Le vrai serveur reste responsable de son
autorisation et de son admission. Les timeouts par requête et l'expiration du
guard sont conservés ; aucune nouvelle borne globale ou garantie physique
d'échéance n'est établie par les compteurs.

Aucun cluster réel n'est utilisé par les recettes admises : les réponses
Kubernetes, UID, états et permissions du peer TLS sont déclarés. Aucune
qualification RBAC, CNI, exclusivité, stabilité NamespaceUID concurrente,
cleanup, admission/start/release, worker ou session exécutable n'en découle.
L'accès à la SSAR doit être disponible pour l'opérateur ; le produit ne crée
aucun rôle et n'élargit aucun droit. L'activation reste fermée, les inconnues
et quotas held-CREATE conservés. Les effets cluster et la qualification de ces
parcours restent NOT_RUN ; aucun gate n'est clos.

Voir les guides [EN](../../../../setup/kubernetes/README.md) et
[FR](../../../../setup/kubernetes/README.fr.md), le
[contrat d'admission](../../../KUBERNETES_ADMISSION.md), les
[gates](../../../PRODUCTION_READINESS.md) et la
[validation centrale](../../../VALIDATION.md).
