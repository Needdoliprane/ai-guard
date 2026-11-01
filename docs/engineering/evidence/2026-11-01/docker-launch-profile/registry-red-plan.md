# Première phase RegistryV2 — test RED gelé, non exécuté

Checkout exclusif docker-launch-profile, HEAD `82f00d751ed233e1282505000891bad49164ffd0` conservé. Un seul fichier modifié : `src/broker/service/profile.rs`, ajout d'un module cfg(all(test,unix)) et d'un seul test. Aucun corps de parser/production, API, autre source ou worktree modifié. Aucun Cargo/test/build/moteur/réseau/credential réel, commit/push ou autre agent lancé.

Test exact : `broker::service::profile::tests::registry_v2_accepts_explicit_minimal_docker_start_profile`.

Le test crée une racine synthétique0700 et deux fichiers JSON0600 via Directory::persistent_create, persistent_sync et Directory::sync. Il charge d'abord le registreV1 valide et vérifie que Registry::profile trouve strict-docker. V2 est construit depuis ce MÊME objet JSON : seul version devient2 et le choix explicite `{version:1,mode:docker_minimal}` est ajouté au profil. Token/programme/config/source/export et endpoints ne sont que références synthétiques inexistantes ; aucun de ces fichiers ou services n'est ouvert. Seuls les JSON neufs sont lus par Registry::load. Les diagnostics de fixture sont fermés, sans valeur/path.

Refus attendu sur le code actuel : Registry::load de V2 rejette la forme nouvelle, après le contrôle positifV1. L'assertion qui doit échouer est `valid RegistryV2 with explicit minimal Docker start profile must load`. Un échec antérieur signifie fixture ou prérequis à corriger, pas RED accepté. Aucune prétention de runtime/auth/profil durable par ce test de parser.

Commandes à confier à Luna après fin du build courant et gel vérifié, une seule exécution :

```sh
# cwd: /Users/louis/Documents/ai-guard/.worktrees/docker-launch-profile
CARGO_INCREMENTAL=0 cargo test --locked --lib broker::service::profile::tests::registry_v2_accepts_explicit_minimal_docker_start_profile -- --exact --nocapture --test-threads=1
```

SHA avant : `aac35b496451ee4317a3adb1edcbbe1f7f7b8f2585c09d04b82eebbcad54b532`.
SHA gel RED : `54498cd3585de8fa03efeef2b831562a9c6add5a45baf97eef339dc507c8bfe5`.

Rustfmt direct1.99.1 ciblé et git diff --check : RC0. Diff93ajouts/0retrait, exclusivement cfg(test). Exécution du RED : NOT_RUN par l'implémenteur. Arrêt après gel ; attente du résultat Luna puis GO Root avant implémentation.
