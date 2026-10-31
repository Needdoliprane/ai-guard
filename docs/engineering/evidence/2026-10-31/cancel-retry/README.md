# Rejeu d'un Cancel durable — preuve logicielle ciblée

31 octobre 2026, macOS27.2/Darwin27.2.0 ARM64, Rust1.99.1. Base `44e5ec32405cc5c9d1efb8ddcaa01dafaf5375a3`, worktree `broker-service`. **PASS logiciel ciblé**, sans qualification de worker réel. G09 conserve son FAIL et aucun gate global n'est clôturé.

## Comportement corrigé

Une commande déjà durable dont la première livraison échouait renvoyait son reçu au rejeu sans retenter l'arrêt. La correction ne réutilise pas la liste de toutes les Arms du head actuel : chaque Cancel garde la frontière de naissance de ses opérations. Un ACK tardif d'une ancienne Arm reste inclus ; un export né après cette commande reste exclu, même après un Cancel plus récent.

Le writer vivant produit une projection STOP typée à partir des métadonnées et de la chaîne vérifiées, sans ouvrir le blob de credentials ni produire de job métier ou permission de start. Le FD de lease partagé, les planchers anti-retour et les limites du job/du cache d'observation sont conservés. Cette projection porte sur le job demandé, pas sur l'inventaire global du namespace. Les métadonnées lues et leurs parents sont synchronisés avant retour live.

Le handler réessaie la livraison du Cancel exact sans nouvelle commande, invalidation de génération ou acquisition du gate d'exécution. Le port du gardien conserve l'intention avant RPC, parcourt les clients de remplacement même si le dernier est failed, et refuse d'insérer une cible inconnue après fuse. Register ObserveOnly puis Cancel utilisent les bindings/deadlines d'origine et la génération maximale de cette seule cible. Fanout source borné à16, budget total45s. Les cibles connues sont tentées avant le refus de complétion d'un job partiel. Un ACK de livraison n'est pas un terminal, une absence observée ou une libération de quota.

## RED, erreurs de harnais et correction de revue

| Tentative | Résultat constaté et conservé |
|---|---|
| Handler avant correction | **FAIL attendu** : une seule tentative, au lieu de deux après rejeu ; [log](red-service.log), [151 fichiers source](red-source.sha256), snapshot `/private/tmp/ai-guard-cancel-service-red.2AjWWI`. Vrai Registry/Journal/Writer, seam cfg(test) de livraison seulement. |
| Première compilation intégrée | **FAIL avant tests** : import `LocalCommandRequestV1` manquant et ancien accès au champ `CancellingJob.generation` dans un test ; [log](compile-failure.log), [gel152](compile-failure-source.sha256). Import et getter corrigés sans changer les validations. |
| Première préparation du canari natif | **FAIL de fixture** : Register normal peut attendre44s, mais la lecture du test expirait à3s ; [log](native-fixture-failure.log), [gel](native-fixture-failure-source.sha256). Diagnostic child conservé dans `/private/tmp/cr-pQleDS/child.stderr`, vide. Le nouveau test borne volontairement la lecture à100ms, exige exactement Unavailable puis vérifie les slots réservés : aucun simple accept de toute erreur. Délais produit inchangés. |
| Première fixture de binding manuel | **FAIL de fixture** : `Incoherent`, car un credential manuel exige observe/bridge et la cohérence de la demande ; [log](manual-fixture-failure.log). [Diff du seul corps corrigé](gel4-to-final-checkpoint.diff), sans assouplissement du validateur. |
| Revue indépendante | **P1 source corrigé** : le premier port ne marquait que le cache racine ; une cible connue seulement dans un remplacement failed perdait l'intention de Cancel. Le parcours borné de la chaîne et sa régression sont relus dans la [revue d'intégration avant/après](integration-review.md). La régression native verte vérifie le fix ; aucune exploitation sur worker réel n'est revendiquée. |

La [revue de projection](projection-review.md) et la [revue d'intégration](integration-review.md) ne signalent plus de P0/P1 bloquant dans leurs périmètres relus. Le probable blocage Serialize de la fixture signalé pendant la revue a été corrigé par comparaison des getters ; aucun DTO reçu n'a été rendu sérialisable pour le test. Les auteurs des revues n'ont exécuté aucun Cargo.

## Commandes et résultats GREEN

Luna est l'unique exécuteur. Sources du worktree `broker-service`, cache `CARGO_TARGET_DIR=../broker-supervision/target`. Commandes séquentielles, sans suite globale ni fournisseur.

| Commande | Résultat et portée |
|---|---|
| `cargo test --locked --lib broker::enforcer::client::retained_cancel_tests::retained_cancel_real_ipc_retries_without_seeding_or_targeting_later_slots -- --exact --nocapture` | **PASS1/1**,0,64s ; [log gel4](green-native.log). Vrai child/horloge/IPC privés, auth/nonce/UID, Register/Slots/Cancel, perte de réponse, max distant7 sans seed du job1, sentinelle non ciblée, bindings divergents, remplacement failed et absence d'insertion après fuse. Actors NotReady, programme moteur inexistant. |
| `cargo test --release --locked --test broker_checkpoint sqlite_store_receipts_cover_all_supervision_operation_variants -- --exact --nocapture` | **PASS1/1**,46,25s ; [log gel4](green-checkpoint-variants.log). Store SQLite et Journal réels, receipts/corrélations C1/C2, ACK tardif pré-Cancel, export post-C2, plancher anti-retour et reader de récupération. Observations runtime synthétiques. |
| `cargo test --release --locked --test broker_checkpoint cancel_projection_retains_manual_bound_arm_after_blob_loss -- --exact --nocapture` | **PASS1/1**,1,18s ; [log final](green-manual-checkpoint.log). Suppression du seul blob synthétique possédé par le test, lecture métier refusée, projection/receipt/ancienne Arm conservées par le writer, diagnostic incomplet et refus d'un ID non-Cancel. |
| `cargo test --locked --lib broker::service -- --nocapture` | **PASS11/11**,0,40s ; [log final](green-service.log). Rejeu redélivré, reçu/head/génération/gate conservés, nouveaux Cancel invalidés avant attente et canaris identité/export. Le seam du handler ne produit aucune observation runtime. |
| `cargo fmt --all -- --check` | **PASS**, code0 ; [log](fmt.log). |
| `cargo clippy --locked --all-targets -- -D warnings` | **PASS**, code0 sans diagnostic ; [log](clippy.log). |
| `cargo build --release --locked` | **PASS**, code0 ; [log](build-release.log). |

**14 fonctions uniques**, sur deux gels reliés ci-dessous. Le helper enfant ignoré lancé par le parent natif n'est pas additionné. Les répétitions et les FAIL ne sont pas comptés PASS. Le natif et le graphe principal n'ont pas été relancés après la correction de la seule fixture manuelle ; leur code et les sources applicatives sont restés identiques.

## Gels et artefacts

[Manifeste gel4](gel4-source.sha256), SHA-256 `0cb591b6569417577838b7f6baa7e42a77fb62bf83156ccdc621d18e05011fb8` ; [manifeste final](final-source.sha256), SHA-256 `c9760b1b576effaab9e493ee1f0656982fe832696fafb809f479477ead337b35`. Chacun décrit152 vrais fichiers, sans s'inclure. Le seul hash différent est `tests/broker_checkpoint.rs` ; le [diff](gel4-to-final-checkpoint.diff) ne touche que la nouvelle fixture manuelle, après le corps du graphe principal. Captures privées : `/private/tmp/ai-guard-cancel-gel4.P4xvU2` et `/private/tmp/ai-guard-cancel-manual-retry.dGSQlL`.

Root a vérifié le checkout final et l'archive extraite : [152/152](verify-checkout.log), [152/152](verify-archive.log),152 fichiers réguliers sans manifeste interne. Archive `/private/tmp/ai-guard-cancel-final-proof.ceL2DR/source-archive.tar.gz`, SHA-256 `e9b824a71ba0a96e3bf5d62c140e67eac4052ee2fb015741e80c8d8423bee3ad`. Copies natives privées hors cache, [empreintes](artifacts.sha256) ; `agentsandbox` : `8d996a17dca8e6b029e36c60bb4a6a0f985c7791d381cdc627aa24a3fb35ea74`. Aucune compilation byte-identique n'est promise.

Avant push :358 cibles de liens locaux dans10 Markdown, aucune manquante sur ce checkout ; ni ancres, ni réponses HTTP, ni portabilité des captures à chemins absolus qualifiées.43 fichiers indexés contrôlés : aucun binaire/symlink, chemin interdit, pattern suspect ou clé connue ; zéro chunk de scanner en erreur. Whitespace vérifié sur sources et documentation éditées, journaux conservés bruts. Le checkout principal et les éditions du mainteneur restent préservés.

## Écarts obligatoires et prochaine priorité

La projection blob-free fonctionne avec un writer déjà ouvert. **Le premier Cancel lorsque le blob manque, l'ouverture du writer strict impossible après redémarrage et le traitement automatique de tous les résultats append inconnus restent à implémenter/qualifier.** Ce lot ne transforme pas ces cas en succès. Le cache pending/Unknown conserve ses refus tant qu'un status/recovery vérifié n'a pas confirmé le fait durable.

Le port direct et la reprise dont le marqueur était déjà présent ne seedent pas le job. Une arrivée concurrente du marqueur pendant une reprise générique n'a pas de garantie linéarisable « aucun seed » ; le client failed a déjà fusé ses guards. Aucun arrêt indu supplémentaire n'est établi par cette course, dont la portée reste publiée dans la revue.

**NOT_RUN :** worker/runtime CLI réel, deadline après perte du CLI/broker, récupération complète de processus, saturation du fanout multi-jobs, Docker/Podman/Kubernetes, autres UID/plateformes, pannes de stockage, frontend/navigateur, SSO et Keycloak. Aucune commande moteur ni API modèle exécutée. Le premier Cancel/STOP-only sans writer métier est le prochain P1, puis la verticale réelle service/worker. Gardien TTY, D5/disarm, rétention physique et charge100 restent obligatoires. G09 reste FAIL,0/12 gates fermés ; facturation désactivée et `billable=false` inchangé.
