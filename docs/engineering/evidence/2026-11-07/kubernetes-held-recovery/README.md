# Reprise du CREATE Kubernetes retenu — 7 novembre 2026

Douze fonctions logicielles uniques PASS sur les gels identifiés ci-dessous :
neuf fonctions du coordinateur au gel02 et trois du module CLI au gel03.
Le check toutes cibles RC0 appartient au gel02. Ce dossier conserve aussi les
RED de l'absence initiale et le FAIL du module CLI avant correction de son
oracle. Il ne qualifie ni admission, démarrage de Pod, release ou session
Kubernetes, et ne clôture aucun gate production.

## Source et changements

Base du worktree `codex/kubernetes-held-flow-20261107` :
`2ade3b0b17ea3bd6620da6d1bf88ce322f233d6c`. Sources validées avant publication ;
les références publiées sont consignées dans CURRENT_STATE/VALIDATION.
Le [manifeste](source.sha256) identifie les cinq entrées Cargo/Rust
du gel03 et les deux guides finalisés après validation. Ces guides, cette page
et son manifeste ne sont pas présentés comme des entrées des tests historiques.

| Gel | `src/kubernetes_cli.rs` | Coordinateur distant | Tests du coordinateur distant |
|---|---|---|---|
| repro01, nouveaux tests et comportement initial | `f46c3f9702b2b258b8ed444adfc2f341b3ebaea8cc67cf5c69129e4b5f3daae3` | `f3797cec328ee60d9c2effff01e8128b7976edae869c23f3075036801928b1de` | `407090d340097964003df71e6d243810884d8a45032fad0baed8a796dee42dd7` |
| fix02, reprise et deux canaris supplémentaires | `7d1a1d57198fe4162d21b1e623ed85a8d01cdb3e33ef1c7e32fe011b87a5e648` | `d4ef7667a6e95c62db63f413d4f5d2526feda4c62b29cd7f9c8684bcca6c47d7` | `27dc0615ad1eb4969e18195bbe1ec353905f70f4f91ba5b01722eca7e1893960` |
| fix03, seul oracle de test corrigé | `f77636bc7edd626579e8136a5b42ef754a56a637e7493805a6a41944d26dedc5` | identique à fix02 | identique à fix02 |

Les empreintes repro01/fix02 ont été relevées au gel transmis à Root ; chaque
journal de recette porte la vérification Root/Luna `source_manifest: PASS 5/5`.
Cette passe documentaire a relu les journaux, vérifié leurs empreintes et les
sept fichiers du manifeste final. Les hashes Cargo sont inchangés entre les gels.

Le coordinateur distant peut maintenant reprendre un journal relu sans
candidat et sans `PendingHeldCreateClaim` ni capacité d'activation. Il effectue
le GET du claim historique exact. Un claim absent reste absent. Un claim trouvé
est retenu en observation seulement, puis un candidat `ownership_unknown`, motif
`recovered_after_claim`, est durable avant soumission Result. Ses identités,
révision et epoch restent celles du claim historique. Une réponse CREATE perdue
avant rétention n'est pas reconstruite depuis un Pod de même nom ou de même spec.

Le CLI `kubernetes reconcile-held-create` relie ce chemin à un journal existant
et à une configuration privée de broker. Le journal fournit des attentes de
scope ; l'identité authentifiée du contrôle choisit le tenant et doit confirmer
le tuple complet tenant/namespace de contrôle/broker/credential/incarnation.
Le CLI n'instancie pas de `KubernetesRuntime` et n'utilise pas de credential
Kubernetes. Son succès exige confirmation centrale et ACK local durable. Le
rapport observé porte `retained_result: ownership_unknown`,
`unknown_reason: recovered_after_claim`, `current_runtime_state: not_observed`
et `admission_available: false`.

Les procédures et limites figurent dans les guides [EN](../../../../setup/kubernetes/README.md)
et [FR](../../../../setup/kubernetes/README.fr.md). Le
[contrat d'admission](../../../KUBERNETES_ADMISSION.md), les
[gates](../../../PRODUCTION_READINESS.md) et la
[stratégie de test](../../../TEST_STRATEGY.md) restent applicables.

## Recettes observées

Luna a exécuté les sélections dans les environnements admis QA AB puis QA AC.
Les journaux conservent sélection, vérification du manifeste, sortie réelle,
durées et code terminal. Les commandes ci-dessous sont des commandes équivalentes
de reproduction, pas une reconstruction d'un argv historique absent des logs.
Elles supposent le même gel et un environnement de validation explicitement admis.

```sh
# repro01 : absence initiale, RED attendu, deux sélections.
cargo test --locked --offline --lib remote_recovery_ -- --test-threads=1 --nocapture
cargo test --locked --offline --lib reconcile_held_create_cli_reopens_exact_journal_without_kubernetes_credentials -- --test-threads=1 --nocapture
# fix02 : coordinateur complet, parent CLI puis compilation toutes cibles.
cargo test --locked --offline --lib runtime::kubernetes::admission::remote_coordinator_tests:: -- --test-threads=1 --nocapture
cargo test --locked --offline --lib reconcile_held_create_cli_reopens_exact_journal_without_kubernetes_credentials -- --test-threads=1 --nocapture
cargo check --all-targets --locked --offline
# Module CLI : même sélection au gel02 FAIL puis au gel03 PASS.
cargo test --locked --offline --lib kubernetes_cli:: -- --test-threads=1 --nocapture
```

| Gel et sélection | Résultat observé | Compilation / tests | Clé du journal |
|---|---|---|---|
| repro01, `remote_recovery_` | RED RC101 : 0 PASS, 3 FAIL, 585 filtrés | 1 min 48 s / 1,07 s | red-coordinator |
| repro01, parent CLI de reprise | RED RC101 : 0 PASS, 1 FAIL, 587 filtrés | 0,51 s / 0,26 s | red-cli |
| fix02, module coordinateur | PASS RC0 : 9 PASS, 0 FAIL, 581 filtrés | 2 min 02 s / 4,97 s | 02-coordinator |
| fix02, parent CLI de reprise | PASS RC0 : 1 PASS, 0 FAIL, 589 filtrés | 0,35 s / 0,41 s | 02-cli |
| fix02, toutes cibles | PASS RC0, aucun test exécuté par ce check | 1 min 04 s | 02-check |
| fix02, module CLI | FAIL RC101 : 2 PASS, 1 FAIL, 587 filtrés | 0,43 s / 0,47 s | 02-module |
| fix03, module CLI | PASS RC0 : 3 PASS, 0 FAIL, 587 filtrés | 1 min 51 s / 0,41 s | 03-module |

Les trois RED du coordinateur atteignent le refus `InvalidContext` de
`resume_result` avant la réconciliation attendue : journal prepared sans
candidat malgré un claim central commité, claim manquant et réponse CREATE
perdue avant rétention. Le RED CLI reçoit `InvalidSubcommand` pour
`reconcile-held-create`. Ces quatre échecs établissent le segment manquant,
pas quatre vulnérabilités ou un contournement de confinement.

Le FAIL `02-module` est conservé :
`preflight_scope_rejects_unknown_versions_roles_protocols_and_namespaces`
échoue avec `accepted future schema`. Le cas négatif traitait le schéma 10
comme futur, alors que le prédicat accepte explicitement 3–10 et que le parent
positif couvre déjà 10. Fix03 remplace uniquement 10 par 11 dans ce cas négatif ;
il ne modifie aucune règle produit. Les trois fonctions du module passent
ensuite, avec 10 toujours positif et 11 refusé.

Le parent CLI de reprise, déjà PASS dans `02-cli`, apparaît aussi dans les
modules 02/03 : il est compté une seule fois. Les neuf fonctions remote et les
trois fonctions CLI forment douze fonctions uniques, pas treize. Aucune suite
complète ou recette toutes cibles n'a été relancée au gel03 ; les neuf PASS
remote et le check restent attribués au gel02, dont ces deux fichiers remote
sont byte-identiques au gel03.

Le check02 signale 61 avertissements bibliothèque, deux pour
`human_sso_gateway_trust` et deux bibliothèque-test. Les sélections test
signalent deux avertissements bibliothèque-test. Aucun résultat Clippy ni
compilation sans avertissement n'est crédité à ce dossier.

Les oracles comprennent l'absence de nouveau claim, de second POST CREATE ou
de GET Kubernetes/adoption par nom lors de la reprise après réponse non retenue ;
le maintien du quota ; le candidat et son premier refus exacts après perte
d'ACK ; le vrai refus HTTP 400 distinct du criblage local ; la course claim
rev2/epoch1 puis cancel rev3/epoch2 avec refus `stale_epoch` sans rebasage,
ACK ou événement central ajouté ; et une vraie fermeture/réouverture du Store
inactif, sans politique, suivie de Reconcile sans réactivation.

## Journaux privés et empreintes

Les fichiers ci-dessous sont localisés hors Git dans
`/private/tmp/ai-guard-cold-drain-logs-20261106.nXx9XJ/` et ont été relus et
hashés lors de cette passe. Aucun journal brut, base SQLite, credential,
certificat de fixture ou exécutable n'est copié dans ce dossier public.
Ces empreintes identifient les pièces vérifiées ; cette passe documentaire
n'effectue pas leur archivage privé durable.

| Clé | Fichier | SHA-256 |
|---|---|---|
| red-coordinator | `kubernetes-recovery-repro01-coordinator-ab.log` | `b666061c14ce6ff1eb8061c8cb6e432cd7eb3bb845450570c50d2f1974f13c34` |
| red-cli | `kubernetes-recovery-repro01-cli-ab.log` | `6c7bdda5e5de8486ab82aeef9b9102eed8864c847a4ec3bc4c56c811d7f1ecfa` |
| 02-coordinator | `kubernetes-recovery-fix02-coordinator-ac.log` | `913721abf17a6adf914ed60dac655ffb3bb7ded765bdc7446e1c9de9633bbb47` |
| 02-cli | `kubernetes-recovery-fix02-cli-ac.log` | `12f5eb299825bf3a6f81167ed110b29f99fce7e1b4705ec4c473ad03bc80764c` |
| 02-check | `kubernetes-recovery-fix02-check-ac.log` | `9836b39d42164a65724fb9f239903e5d0cff7c50727114370141c77c45e1def6` |
| 02-module | `kubernetes-recovery-fix02-cli-module-ac.log` | `0842c013b1b6fef1affc6af9921b8e365b0a75c7f580ad5ff4701abc60d3a454` |
| 03-module | `kubernetes-recovery-fix03-cli-module-ac.log` | `f8e2cda002c610282de9c8f6c0b16d41c8732cecdf46d2f1ac220a6095bf4500` |

## Environnement, incident natif et limites

Les configurations QA AB/AC retenues identifient l'image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`,
UID/GID 1000, réseau `none`, racine readonly, capacités `ALL` supprimées,
`no-new-privileges`, quatre CPU, 8 Gio de RAM sans swap additionnel et 256 PIDs.
Ce sont des paramètres de la QA, pas une mesure des besoins du produit ni une
qualification de son confinement. Les empreintes des pièces d'inspection lues
sont `be20b9c03c041289448548df2f81ee00321c73a8b5d60561b6bb7ba61897706b`
(`qa-ab-inspect.json`) et
`542ec22182cc8c18fccc9ac7931e3a7bc6703845932b1e50b625efd2dd0a50dd`
(`qa-ac-inspect.json`). Aucun moteur ou outil cluster n'a été invoqué par cette
passe documentaire.

Avant la recette admise, Luna a signalé un démarrage de compilation Cargo sur
l'hôte Darwin, interrompu RC130 après rappel de la frontière de validation.
Aucun test n'a été observé exécuté dans cette tentative. Cela ne prouve pas
l'absence d'exécution native : les build scripts du projet ou des dépendances
ont pu s'exécuter pendant la compilation. L'incident repose sur le compte rendu
Luna de la session ; aucun journal natif brut correspondant n'a été localisé
dans le dossier ci-dessus par cette passe. Il n'est compté ni comme RED produit
ni comme PASS, et aucune isolation QA AB/AC ne lui est attribuée.

Les recettes admises emploient vrais HTTP/SQLite/fichiers et un pair TLS local
possédé. Celui-ci renvoie des faits Pod déclarés et ne crée aucun Pod ou worker.
La perte de réponse avant candidat est injectée en abandonnant le résultat
avant sa rétention durable. Ce n'est pas une panne physique ou un crash de
processus qualifié. Aucun appel cluster, cloud, fournisseur ou modèle n'est
établi par ces tests.

L'activation de production reste BLOCKED : NamespaceUID stable, permissions,
exclusivité, admission et confinement doivent encore être prouvés. Start,
release, session exécutable, transfert de workspace, arrêt/quiescence et
réconciliation physique restent non qualifiés. Les effets d'un vrai cluster
Docker Desktop, ses CNI/RBAC/admission et les autres plateformes restent
NOT_RUN. Le CLI confirme un rapport broker et un reçu durable, pas un état
runtime indépendant ; une inconnue ne libère aucun quota et ne produit aucune
mesure d'usage zéro. Voir la [validation centrale](../../../VALIDATION.md).
