# Raccord du broker permanent — checkpoint de développement

31 octobre 2026, macOS ARM64, Rust 1.99.1 ; base `c4c4d2a7560d31768da08d8ad9335f4f08667344`, worktree `broker-service`. **PASS logiciel ciblé ; verticale runtime encore ouverte.** Le CLI par défaut, la QA existante, le schéma par défaut et `billable=false` restent inchangés. G09 conserve son FAIL historique ; aucun des 12 gates globaux n'est clôturé.

## Comportements et preuves

Le service Rust explicite réunit profils de confiance, authentification fraîche au contrôle avant les données du caller, IPC Unix privé, writer borné et exécution batch détachée du client. Le gardien du même binaire a son propre processus et conserve les données des arrêts. Source et parent d'export sont des capacités FD ; la destination est neuve. Les événements V4 sont conservés avant livraison et ACKés ensuite dans la chaîne du Journal. Ces implémentations ne prouvent pas encore le parcours complet sur moteur.

La revue a identifié puis fait corriger : ancien Cancel appliqué à un export postérieur au restart ; admission sans contrôle de santé courant du gardien ; collisions entre identités de commandes/événements/exports, y compris la course input FS/Cancel ; inventaire de bindings incomplet non signalé ; historique managed V1 sans Arm traité comme complet. Le lecteur STOP distinct retrouve les chaînes saines malgré un sibling incorrect ou un blob perdu et ferme les admissions si l'inventaire est incomplet. Le chargement métier garde ses refus stricts. [Revue du service](service-review.md), [recoupe STOP et identités](stop-recovery-review.md), [revue indépendante enforcer/workspace](enforcer-workspace-review.md) et [revue de documentation/provenance](docs-review.md). Ce sont des revues d'ingénierie ciblées, pas le nouvel audit Daybreak Blue. Les deux remarques documentaires ont leurs dispositions : liste initiale du plan datée comme historique, chemins privés du rapport STOP identifiés comme provenance et index de sources courantes fourni séparément.

40 fonctions uniques ciblées ont été exécutées dans leurs gels documentés. Les replays et sous-processus ne sont pas additionnés. Les événements, autorités et snapshots des tests de contrats sont explicitement synthétiques ; leurs fichiers Journal et Store SQLite sont réels.

| Commande depuis la racine | Résultat constaté | Portée |
|---|---|---|
| `cargo test --locked --test broker_local_ipc --test broker_service --test broker_writer -- --nocapture` | PASS : 3 + 1 + 1 | IPC réel, aide du vrai CLI, writer/rejet durable |
| `cargo test --locked --test broker_enforcer -- --nocapture` | PASS : 1 | Vrai child natif vide, SIGSTOP/SIGCONT, même epoch et shutdown ; aucun worker |
| `cargo test --locked --lib broker::writer::tests -- --nocapture` | PASS : 2 | Saturation avant queue rend le ticket ; timeout après enqueue reste inconnu et commit tardif réel |
| `cargo test --locked --lib broker::service:: -- --nocapture` | PASS : 10 | États, secrets, inputs d'export et concurrence du verrou d'identité |
| `cargo test --locked --lib authorized_workspace_tests -- --nocapture` | PASS : 4 | Vrais FD/FS, substitution d'ancêtre, copie/export, destination existante et archives hostiles |
| `cargo test --release --locked --test broker_outbox -- --nocapture` | PASS : 5 | 500 événements en deux révisions, cap512, replays/divergences/secrets/contexte et V4 |
| `cargo test --release --locked --test broker_checkpoint -- --nocapture` | PASS : 6 | Chaîne V1–V3, Store réel, reader STOP, sibling incorrect, causalité Cancel/Export et legacy |
| `cargo test --locked --test broker_journal manual_binding_canonicalizes_source_and_reloads_without_the_source_path -- --exact --nocapture` | PASS : 1 | Blob durable supprimé ; projection conservée, chargement métier refusé |
| `cargo test --locked --test broker_journal provider_binding_preserves_exact_record_and_reports_actual_handoff_digest -- --exact --nocapture` | PASS : 1 | Entrée binding supplémentaire ; incomplet conservé, ouverture métier Corrupt |
| `cargo test --locked --lib broker::journal::tests -- --nocapture` | PASS : 5 parents ; 2 helpers ignorés | Faults, fsync, SIGKILL et locks ; le helper lancé explicitement n'est pas recompté |
| `cargo fmt --all -- --check` | PASS | Gel final |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS | Aucun avertissement |
| `cargo build --release --locked` | PASS | Binaires natifs ; pas une construction d'image |

Après les corrections mécaniques Clippy, seuls IPC3, service10 et outbox5 ont été rejoués : [journaux finaux](logs/). Les autres résultats gardent leur gel et leur portée. Aucun frontend, modèle, Docker, Podman, Kubernetes ou E2E global exécuté pour ce checkpoint.

## Échecs conservés et provenance

Les journaux `fail-*` conservent les tentatives : champ de ticket privé/mutabilité/imports de fixtures, bootstrap direct du child sans son fichier `auth`, attente V4 alors que le premier append reste V3, secret testé sur une outbox déjà pleine, oracle qui unwrap l'ouverture métier refusée, et Clippy. Les corrections du harnais ne modifient pas les assertions métier : le test natif crée seulement sa capability neuve, garde son child, puis reprend/shutdown via le client original pour éviter un autre watcher. Aucun credential existant n'est utilisé.

Les premières captures annonçaient150 lignes parce que leur manifeste s'incluait lui-même. Elles sont conservées ; ce compteur était incorrect pour les sources. Le [manifeste corrigé externe](source.sha256) comporte **149 fichiers** Cargo/toolchain/src/tests, vérifiés149/149 contre le snapshot, le checkout et l'archive extraite. [Correction détaillée](provenance.md). Aucun test ou binaire n'a été modifié pour corriger ces métadonnées.

Manifeste SHA256 : `b5a59c6b7b20650804778f018aff785a6328235b8bb854f2940a68c91be6cf19`. Archive privée `/private/tmp/ai-guard-final-canaries-proof.7zXKhV/source-archive.tar.gz` : `1648d46b21128b7f2dc86037229d0026a75ae4cd89cfbba824da62e0f2fafea2`,149 fichiers réguliers, aucun manifeste, AppleDouble, secret, frontend, cache ou binaire. Les exécutables restent hors Git, copiés et comparés dans `/private/tmp/ai-guard-release-binaries.kKFaso/bin/` :

- `agentsandbox` : `22a059b8f7d57437d4aa4447cfe25e5d2833980cea6fb9e3fa53cd4651c231c7`.
- `ai-guard-provider` : `a0ea9da6af195d61cc588cd45aee553f199371993a9516b8cb8bb0b69b5f5ade`.

Ces hashes identifient des copies vérifiées, sans promesse de build reproductible bit à bit.

La demande SSO arrivée pendant ce cycle a été rapprochée des ajouts simultanés du mainteneur dans main. Le [cahier produit](../../../../sso.md) est repris à l'identique et le [contrat technique](../../../SSO_OIDC.md) lui est subordonné. [Table de préservation et fusion documentaire](docs-integration.md) : neuf fichiers main vérifiés inchangés, dix candidats documentaires relus, aucun source/binaire modifié. Les modifications de main et son bloc mémoire transitoire sont préservés ; seul le candidat Git isolé exclut ce bloc. Cette documentation n'ajoute aucun code SSO, profil Keycloak utilisable ou essai OIDC réel. L'identité locale nominative, l'essai préconfiguré et la traçabilité complète restent des exigences à implémenter.

Contrôles de publication du checkpoint : 82 fichiers indexés examinés, aucun binaire, symlink, chemin interdit, suspect de pattern ou clé connue ;423 liens relatifs vérifiés dans18 Markdown, aucun target manquant. Les journaux Cargo sont conservés bruts, y compris leurs lignes finales vides ; le contrôle whitespace s'applique au code et aux documents édités. Le checkpoint est destiné à la branche isolée `codex/broker-service`, avec main et les éditions concurrentes préservés.

## Qualification encore requise

`NOT_RUN` : bootstrap public parent, vrai service/API avec identités A/B, slots non vides, deadlines après crash du CLI/broker, gardien TTY, reprise des stops perdus et matrice Docker/Podman/Linux. D5/disarm reste absent : les slots sont conservés, une absence/Created/timeout n'est pas un fence. Le replay d'une annulation ACKée dont le stop RPC a échoué doit encore démontrer sa convergence sans tuer un export postérieur. Rétention et caps physiques, capacité100 et cloud restent ouverts.

La publication exclusive d'un export ne constitue pas une preuve de fsync de l'arborescence et du parent après panne électrique. La borne de fichiers retenus ne qualifie pas le travail total d'énumération des entrées exclues. Ces points signalés par la revue restent des écarts de qualification à traiter, pas des garanties livrées. Aucun compte fournisseur ou infrastructure externe utilisé. Le rappel Daybreak Blue attend les scénarios réels d'arrêt/reprise de workers.
