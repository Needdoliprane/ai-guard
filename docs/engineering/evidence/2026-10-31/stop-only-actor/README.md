# Acteur STOP restreint — preuve logicielle ciblée

31 octobre 2026, macOS27.2 ARM64/Rust1.99.1, base `44324a7d2755b9b174f49dc32b65d47bcda6ce2e`, worktree `broker-service`. **PASS logiciel ciblé**, sans qualification de worker réel. G09 conserve son FAIL et aucun gate global n'est clôturé.

## Défauts reproduits

Luna a exécuté `cargo test --locked --lib broker::service::server::cancel_retry_tests -- --nocapture` avant correction. Résultat **FAIL attendu** : 1 PASS, 2 FAIL, sans erreur de compilation. Le premier Cancel ne devenait pas durable lorsque le blob manuel avait disparu. À froid, un sibling privé invalide empêchait la livraison connue, car le handler exigeait le writer métier complet. Vrais Store, Journal et Writer ; Arms avec observations moteur synthétiques et seam cfg(test) de livraison bornée, aucune observation moteur réelle. [Log RED](red-cancel.log), [sources](red-source.sha256) ; snapshot privé `/private/tmp/ai-guard-stop-red.Kpcsib` conservé. SHA-256 du log RED : `af06e977c61d1205529cde3efba9ee0569db3f0f949ae7d8f65f842edfbf15a9`.

## Contrat du changement

Une seule file et une seule lease, capacité STOP sans conversion métier ; append limité au Cancel, lecture confirmée sans blob fournisseur, compte physique exact avant nouvelle publication et reçu historique au rejeu. La voie métier conserve ses contrôles stricts avant/après CAS. Le diagnostic séparé nomme la source journal et l'absence de requête runtime. [Décision et compatibilité](../../../DECISIONS.md#31-octobre--acteur-stop-restreint-et-diagnostic-de-reprise).

## Résultats

Luna est l'unique exécuteur Cargo. Cache explicite `CARGO_TARGET_DIR=../broker-supervision/target`, commandes séquentielles, sans suite globale. Root a relu les sorties et RC puis vérifié sources et archive extraites.

| Commande | Résultat et portée |
|---|---|
| `cargo test --locked --lib broker::service:: -- --nocapture` | **PASS gel3 :17** ; [log](gel3-service.log). Dont premiers Cancel/replay/metadata, invalidation, inputs d'export et auth. Les deux auth sont rejoués avec le troisième canari au gel final ; union service18 fonctions. |
| `cargo test --locked --lib broker::journal::tests -- --nocapture` | **PASS gel3 :7 parents**,2 helpers ignorés non comptés ; [log](gel3-journal.log). Publication/fsync/faults/SIGKILL/locks, refus Full après perte du blob au hook CurrentDurable, admission Full refusée sur faute après barrières et même lease conservée. |
| `cargo test --locked --lib broker::service::server::cancel_retry_tests::stop_auth_tests:: -- --nocapture` | **PASS final :3** ; [log](auth-and-connection.log). Vrai HTTP/Axum/Store2 tenants ; runner/tenant/namespace/schema/inactivité, exactement1 appel identity par requête, credential vide sans HTTP ni namespace créé. Connexion Unix réelle, frames et tâche `Service::connection`, refus vide sans boxing de dispatch dans ce canari. |
| `cargo test --locked --test broker_writer --test broker_service -- --nocapture` | **PASS final :3+1** ; [log](writer-and-cli.log). Factory unique, lease inode conservée jusqu'au dernier handle, hash de rejet transmis au nouvel acteur, aide du vrai CLI. |
| `cargo test --locked --lib broker::writer::tests -- --nocapture` | **PASS final :3** ; [log](writer-ownership.log). Saturation/timeout/rejets, et vrai Cancel en queue dont la réponse expire : Unknown sans input rendu, second UUID NotQueued avec input rendu, reprise sans doublon puis seconde commande exacte. |
| `cargo test --release --locked --test broker_outbox v4_survives_command_and_lifecycle_updates_and_direct_v3_downgrade_is_refused -- --exact --nocapture` | **PASS final :1**,1,75s hors compilation ; [log](outbox-v4.log). Collisions event/commande refusées, événements V4 conservés et Cancel rejoué sans nouvelle révision ; pas de reprise des500 événements. |
| `cargo fmt --all -- --check` | **PASS final**, RC0, [log](fmt.log) vide conforme. |
| `cargo clippy --locked --all-targets -- -D warnings` | **PASS final**, RC0, [log](clippy.log). |
| `cargo build --release --locked` | **PASS final**, RC0, [log](release.log), binaires conservés hors cache ; aucune image construite. |

**33 fonctions uniques**, pas une capacité33 workers. Les auth rejoués, le négatif exact diagnostic et les enfants de tests ne sont pas additionnés. Le [delta gel3 → final](gel3-to-final-tests.diff) modifie uniquement trois fichiers de tests : auth, `broker_writer` et `broker_outbox`. Les15 autres fonctions service et7 parents journal inchangés ne sont pas relancés après ces seuls changements de fixtures. Le canari de connexions vides n'est ni une preuve de tous les chemins authentifiés ni de PrivateListener/UID/découverte.

## Échecs et revue conservés

- [Première sélection](fixture-path-failure.log) :15 PASS,2 FAIL de fixtures utilisant un chemin temporaire macOS non canonique. Les contrôles anti-symlink ont refusé `/var` ; seules les racines de tests ont été canonisées, aucun garde produit assoupli.
- [Deuxième sélection](matrix-stack-failure.log) : SIGABRT/stack overflow dans la matrice d'authentification debug, interrompue. Boxing local du futur dans la matrice, sans augmentation de pile globale. Mesure ultérieure : dispatch107488 octets, connection107600 ; ce sont des tailles de types, pas des limites opérationnelles. Le refus vide via vraie connection est ensuite testé sans ce boxing. Aucun risque de pile sur l'ensemble des chemins produit n'est déclaré clos par cette seule preuve.
- [Compilation d'intégration](private-helper-compile-failure.log) : un test appelait le helper canonical privé. Correction des seules sérialisations de fixtures par serde_json, sans rendre l'API publique ni retirer les assertions.
- Deux erreurs de shell des captures gel3/4 (`tee` absent du PATH) n'ont pas de RC de test/build interprétable ; notes [service](service-shell-attempt.note) et [build](build-shell-attempt.note) conservées. Seules les exécutions complètes avec log et RC0 fondent les PASS.
- [Revue indépendante](integration-review.md) : trois écarts de contrat détectés en source et corrigés, sans qualifier une exploitation runtime : double admission depuis le même rapport, barrières de durabilité Full manquantes, hash de rejet perdu lors du transfert continu de lease. Canaris correspondants verts. Aucun P0/P1 restant identifié dans le périmètre relu ; cette revue ciblée n'est pas un nouvel audit global.

## Gels et artefacts

[153 sources finales](final-source.sha256), vérifiées avant/après Cargo puis153/153 sur le checkout et l'extraction de l'archive par root. [Gel3](gel3-source.sha256) conservé pour les canaris non rejoués. Capture finale `/private/tmp/ai-guard-stop-gel4.m2XvQt`, extraction root `/private/tmp/ai-guard-stop-source-verify.OhmaAJ` ; archives/binaires privés hors Git, pas de promesse de rebuild byte-identique.

- Manifeste final : `62c30435024349117f08e03fc0c12533b3a431352675b70f14dd4e0390cca348`.
- Archive source : `9ad077dc1baef204cef6234603524418c0bba8b6b1034c2406ce8ba411373edf`.
- `agentsandbox` : `0e774fc2a09b0b1b906e82c4bb4cb7fefcfba469aaaa3cda816b1a5bccdc1f5a`.
- `ai-guard-provider` : `40fadb84ce476e05b593adbdcd8edc4919bcafbf23f41370761669f1487f16f3`.

## Qualification restante

Docker est joignable et des images locales sont disponibles ; seules des commandes de lecture version/info/inspect ont été faites dans ce cycle. Ce constat ne prouve ni démarrage, ni arrêt, ni deadline, ni reprise. Nouvelle session réelle, perte du client, indisponibilité/reprise du moteur, D5/disarm, Podman/Linux, scénario A1/A2/B1 et capacité100 restent ouverts. Aucun appel fournisseur, test frontend, navigateur, E2E global, service OS ou action cloud pour ce lot.

La mesure allocation CPU/RAM reste non facturable (`billable=false`). SSO/Keycloak/PKCE, identités nominatives, TTY et les autres exigences du produit restent à livrer/qualifier. Aucun fichier, volume, credential ou checkout principal préexistant modifié pour obtenir ces résultats. **G09 reste FAIL,0/12 gates fermés.** Prochaine priorité : verticale Docker réelle du broker avec arrêt/reprise, en traitant explicitement le cycle du gardien et D5.
