# Checkpoint de supervision V2 — vérifications logicielles GEL5

Base `3e2cffa86c0d0c4784507e78f289419a9e39890f`, worktree `broker-service`. Le [contrat](../../../IMPLEMENTATION_PLAN.md) porte le besoin, les propriétaires et l'acceptation. [Implémentation](implementation.md), [rapport Luna](tests.md), [revue indépendante](review.md). Le format conserve des faits de reprise privés ; il ne livre aucun service permanent ou droit d'exécution.

Verdict final indépendant **CLOSE** au périmètre logiciel/docs/preuves GEL5, BCP-01 à06 traités, aucun constat bloquant résiduel dans ce lot. Le reviewer a recoupé source, oracles, logs et artefacts sans exécuter de Cargo supplémentaire. Aucun gate global n'est fermé par ce verdict.

## Comportement livré

Révisions V2 avec domaine de hash distinct au sein du journal existant, payload runner V1 conservé, neuf variantes de requêtes/réponses du contrôle. Rev0 reste V1 exactement initiale, plans1/2/3, scope/bindings/accepted/current et leurs bytes restent inchangés. Les chaînes confirmées sont V1+V2* ; promotion interdite si un managed a existé dans le préfixe V1. Après promotion, advance V1 refuse même un ancien replay.

Requêtes pending avant réponse, helpers préparés avant owned, listes append-only et réponses connues immuables. CAS et replay historique utilisent la même publication exclusive/fsync/pointer/plancher. Les liens de namespace, intention, admission, parent historique, armements/claims/helpers/export sont recoupés. Résultats worker dérivés du reducer pur Store réutilisé, aucune machine d'états dupliquée ni SQL changé. Managed préparatoire accepté avant réponse d'admission ou après refus, sans Worker/effet autorisé. Scope/receipts restent des données à recouper avec l'identité authentifiée du futur service.

## Vérifications constatées

| Commande/cible | Résultat GEL5 |
|---|---|
| `cargo test --release --locked --test broker_checkpoint` | [**6PASS**](gel5/release-full-target.log), graphe complet neuf variantes et quatre purposes helper, Store/journal réels ; 17,59s de test, compilation52,72s. |
| Même cible debug, graphe filtré | [**5PASS**, 1filtré](gel5/expanded-quick-fixed-replay.log). Le filtré n'est pas une réussite supplémentaire. |
| `broker_journal`, `supervision_store`, `journal_sqlite` | [12](gel5/broker-journal-canary.log)+[9](gel5/supervision-store-canary.log)+[1](gel5/journal-sqlite-canary.log) **PASS**. |
| `cargo test --locked --lib broker::journal::tests` | [**4PASS**,2ignorés](gel5/journal-unit-canary.log), dont un self-test de harnais ; les enfants ignorés sont invoqués par leurs parents et ne comptent pas en plus. |
| Formatage / Clippy strict | [fmt](gel5/fmt-expanded-final.log) et [Clippy](gel5/clippy-expanded-final.log) **PASS**. Deux exceptions locales motivées pour les enums de snapshots bornés, une pour le helper de fixture à14arguments ; aucune désactivation globale. |
| `cargo build --release --locked` | [**PASS**](gel5/release-build.log),0,12s cache après la compilation release de la cible. |

Les 26canaris et les cinq rapides ne s'ajoutent pas aux rejeux/children pour gonfler les comptes. Environnement macOS27.2/ARM64, Rust/Cargo1.99.1 ; aucun moteur/modèle. Couverture nouvelle batch/generic/bindingNone. Décisions, admissions, quotas/refus, claims/receipts et audit proviennent du vrai Store SQLite ; autorité, inspections, owned, observations, horloges et effets runtime sont explicitement synthétiques. Les mutations adverses de réponses sont des données volontairement altérées, jamais attribuées au Store.

Le test final couvre aussi : admission pending/refusée avec managed préparé, refus worker/helper avant ACK avec naissance/binding valides, révision ACK unique, V1 managed→None refusé à promotion, approbation préservée, V2 ultérieure puis replay historique sans rollback, ancien current restauré refusé par deux clones partageant le plancher, current correct restauré avant reopen. Les preuves détaillées ne remplacent ni l'authentification IPC ni des effets runtime.

## Sources et artefacts finaux

[Manifeste115](source.sha256) vérifié depuis la racine du dépôt, SHA `92bb5de2ecaf7275a941328b5b4a8aa4fc9a7ea7850d37092e6e768b973b3ae2`. Les [neuf empreintes avant/après build](gel5/release-build-before.sha256) sont identiques et conformes aux sources courantes ; autres entrées compiler inchangées de la base. Source/test gelés et copie complète hors target dans `/tmp/ai-guard-broker-checkpoint-qualified.VkmKX7`, parent0700/fichiers0600. Archive source SHA `2a0b97cc4b047c95bb17ebcf45837ad51a2ba0656063546ce718ba51ddfbaebf`.

| Artefact privé macOS | SHA256 |
|---|---|
| `agentsandbox-macos` | `b360af55dc6d6b75dd9d9f9ead2d59eecda0428cbe7f68b5cfa833b0a0911f98` |
| `libagentsandbox-macos.rlib` | `68289f51d52eea893f91a52b7a25f632f71564fb12288d8e7b2ea3ce9042a7c5` |
| `broker-checkpoint-test-macos` | `e2966caede8524ea7eef8cd8682ea0b0512e82602874a495d2960bfd385d972e` |

Les [hashes des copies Luna](gel5/release-artifacts.sha256) concordent. Aucun binaire/base/credential ajouté au dépôt. Les artefacts bootstrap antérieurs restent préservés. Le manifeste des preuves SHA256SUMS, distinct, est vérifié depuis ce dossier après gel documentaire.

## Pièces préparatoires

- [Design initial](design-initial.md), conservé à son SHA d'origine ; il doit être lu avec les corrections obligatoires de la [revue de conception](design-review.md) et du contrat actif. La revue de conception ne vaut pas revue d'implémentation.
- [Plan initial des tests Luna](test-plan-initial.md), avant fixation de l'API ; ses signatures comportementales ne supplantent pas le contrat actif.
- [RED de compilation](red/cargo.stderr) : module et méthodes V2 absents, RC101 attendu. Aucun test exécuté dans ce RED, aucune vulnérabilité reproduite.
- [Golden V1](golden-v1/rev0.json) produit par un [probe Rust](golden-v1/probe.rs) lié à la bibliothèque macOS déjà qualifiée du bootstrap, SHA `10d191a1c4e457b57bc358ae9698241ec69b0701036c5da46ee60f87e09376d4`. Pas de Cargo ni writer V2 pour créer cette référence. SHA du JSON `9d92b7117f72e976f61abacce7bc71c0fdace0cb217ec9796259e4b8f73fa046`, head avec domaine V1 `b49fc25a9999c1e194fe4d47cec4475f5534b4d7d8c75f5eb4705127907103ad`. Incidents de harnais distingués dans le rapport final Luna.

## Écarts corrigés et gels intermédiaires

La [revue intermédiaire du gel1](review-initial.md) et les [RED groupés](red-gel1/mutation-red.log) conservent les défauts avant correction. Quatre reproductions distinctes ne sont pas quatre fonctions de test supplémentaires : plusieurs assertions ont été atteintes successivement dans le même parcours SQLite, avec leur source de test propre.

- Mutation Unknown au motif vide : le Store réel refuse sans changer sa tête, le journal accepte la requête pending.
- Admission pending non approuvée sous runner approuvé : un vrai reçu Decision Initial/Allow ne rend pas ces demandes égales.
- [Owned imbriqué derrière enum](red-gel1/serde-red-details.log) : from_bytes et Deserialize acceptent des champs inconnus null dans owned/observation, bien que le roundtrip non modifié fonctionne. Snapshot runtime synthétique, Worker issu du Store ; pas de claim ou ACK inventé.
- [Résultat Prepare altéré](red-gel1/prepare-ack-red.log) : vrai ACK positif, puis variante modifiant seulement état Terminal et quota_held acceptée. Le receipt inchangé n'en fait pas un vrai terminal.

Gel2 ferme ces quatre écarts et les branches d'ACK contradictoires (BCP-05). Le [rejet préparatoire BCP-06](gel2-partial/pending-managed-red-final.log) est reproduit puis fermé au gel3, corroboré par le vrai producteur qui écrit managed/archive avant RPC. Le premier PASS4 du gel2 reste partiel ; le gel5 ci-dessus couvre les nouveaux oracles. Le [rapport initial](implementation-initial.md) décrit son propre gel et ne constitue pas une garantie rétroactive des corrections.

Le parcours debug gel3 a pris230,66s. Une capture unique `/private/tmp/ai-guard-broker-checkpoint-v2-qualification/gel3/graph-debug.sample.txt` (3996785octets, SHAd8bd7b9bb1630ed38f8048b6c4afe18d92b767b1f93563e944c821bcfa6d6cc5) situe le coût dans les revalidations/canonicalisations ; aucun échantillon ne prouve à lui seul l'absence de boucle. Gel4 retire uniquement le second décodage V2 redondant, en conservant les Deserialize stricts, l'égalité canonique du record complet, les liens/transitions/FS/caps/durabilité et le chemin V1. Aucun cache de fichiers introduit.

Mesures ponctuelles du graphe release :22,39s gel3,16,71s gel4,17,59s gel5 avec oracles finaux. Ce ne sont ni statistiques de performance ni qualification100workers. Les deux sources applicatives intermédiaires différentes sont [conservées et hashées](intermediate-app-sources/journal-gel3.rs.txt). **Limite :** le test exact intermédiaire e0f n'a pas été copié avant édition et son inversion privée n'a pas retrouvé son SHA. Binaries/logs/hashes sont conservés, mais la reconstruction bit-à-bit de ce test historique n'est pas établie. La qualification finale repose sur les sources/test GEL5 entièrement archivés, pas sur cette reconstruction partielle.

Un rejeu complet debug a été interrompu explicitement après un FAIL de fixture Busy : un second open tentait de reprendre le writer lock déjà vivant. Clone est l'oracle correct pour des handles partagés. Le [rapport](tests.md) conserve les FAIL/interruptions et leurs causes ; aucun de ces passages n'est compté PASS et aucune politique applicative n'est assouplie pour les fermer.

## Préparation du chantier suivant, non implémentée

Les [notes de lecture D5](design-next-not-implemented/d5-source-review.md) et la [piste officielle Moby](design-next-not-implemented/d5-official-source-notes.md) décrivent les incertitudes d'un start retardé face au stop. Aucun fence irréversible, nouveau contrat de suppression préterminale ou terminal/usage n'est livré ici. La correspondance entre les sources publiques master et le moteur local déclaré n'est pas établie ; aucune garantie transférée à Docker29.9.2 ou Podman.

**NOT_RUN** : effets workers, autres modes/fournisseurs V2, Linux natif/VM pour ce code V2, Podman, Kubernetes/cloud, IPC/service/ordonnanceur/gardien, worker après mort CLI, fencing D5, panne électrique et charge100. Frontend/navigateur non concernés par ce delta et non rejoués. Les hooks communs du journal restent vérifiés par leurs canaris ; aucune nouvelle qualification exhaustive des crashs V2 n'est déduite. Le plancher est partagé entre handles vivants et ne promet aucun anti-restore supplémentaire après restart. QA, défaut2 et facturation inchangés ; G09 demeure FAIL et les12gates ouverts.
