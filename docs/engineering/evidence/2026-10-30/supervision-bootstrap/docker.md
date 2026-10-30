# Qualification Docker ciblée — bootstrap du contrôle

Date : 2026-10-30. Exécuteur : Luna. Périmètre : bootstrap du gateway uniquement, sans worker, modèle, fournisseur, test E2E général ou modification du dépôt.

## Résultat

**PASS au périmètre Docker Desktop Linux/ARM64 pour le bootstrap neuf**, sur les deux tenants synthétiques. Le témoin non préparé est refusé avant toute création de DB parce que Docker a créé `/run/secrets` en `root:root 0755`. L’image préparée démarre le gateway sous UID1000, passe son healthcheck interne, et le probe HTTP basé sur `AuditClient` confirme pour A et B le rôle `runner`, le schéma3 actif et des namespaces différents. Deux DB distinctes et deux backups privés sont observés; le format pré-migration2 découle de la construction du produit et d’une vérification native distincte, pas d’une relecture SQLite indépendante dans cet essai Docker. Les deux conteneurs runtime sont arrêtés et conservés, avec leurs volumes et images.

Ce PASS ne vaut pas qualification du Dockerfile multi-stage complet, d’un worker, d’une supervision autonome, d’un hôte Linux natif, de x86_64 ou de Podman.

## Source et environnement

- Worktree `/Users/louis/Documents/ai-guard/.worktrees/broker-service`, base `f46a23e`; changements préexistants préservés, aucun fichier du dépôt édité.
- Archive source figée SHA-256 `79407466e2f2417389e7d32faad67207349959024f82080d61c07990a8339275`.
- Manifeste112 SHA-256 `10d581fd0e445e9d3647e17b59b098245ea401d1d1b32619eb7f9cbbf4e44a68`; vérifié avant build et recopié depuis le builder arrêté avec **112/112 entrées conformes**. Contexte build isolé : ces 112 sources plus `policies/default.json`; aucun `.env` ou credential réel.
- Docker serveur `29.9.2`, client `29.2.1`; daemon Linux/aarch64, noyau VM `7.0.14-linuxkit`.
- Builder local `ai-guard-broker-journal-linux:qa-20261030`, ID `sha256:bd409e1b1126ec4a0c5313b63f5829fc2d96037789b4481fc8e4e5ab61df4821`. Runtime de base `agentsandbox-control:qa-20261030-engine-logs-v2-final-c53bd8`, ID `sha256:33313bfcc385be5056e722cafd891f943e5cdb2ddf3849035a7767d2f1dcf543`.
- Le premier build release a réussi dans un conteneur éphémère avec `--rm`, mais le volume Docker anonyme `/build/target` (déclaré par l’image builder) a été supprimé avec le conteneur avant extraction; ce n’était pas seulement la couche writable. Le snapshot commit ultérieur excluait aussi ce volume. Cet incident de harnais n’est pas une preuve. Le build repris avec handle conservé `ag-bootstrap-builder-20261030`, ID `717b15a0ba0aca40a9f098cbedb404676b40e0bc27825330bfa5575f92e7409`, a fini RC0 en 62s. Binaire release ELF AArch64 copié depuis ce handle : SHA-256 `764b91d0297669c0c07db094dd46fb4874f5145bb8966f330a62138b9cc94e9e`.
- Le probe externe Rust `bootstrap_qualification_probe.rs` utilise `AuditClient::new`, lit les tokens par chemin et n’imprime aucune valeur. Release du produit utilise `lto="thin"`; le probe a été compilé par `rustc` avec `-C lto=thin`, sans crate ni réseau. ELF AArch64 SHA-256 `9db646573aa00457b4f1fb7b6c775969d3a3057ae73d1ec1b0587b9daed8c80e`. Deux essais de linkage antérieurs avec le profil linker par défaut puis lld ont échoué; le profil ThinLTO correct a compilé en 1,7s. Ces tentatives sont des incidents du harnais, non des échecs produit.
- Le runtime source n’avait pas `/run/secrets`; `/data` initial appartenait à UID/GID1000 mode0700.

## Images et exécutions

Images comparatives construites hors réseau (`--pull=false --network=none`) sur le tag runtime local contrôlé. Les tags originaux non modifiés et les dérivés contenant le même probe sont conservés.

| Variante | Image / ID | Résultat |
|---|---|---|
| Témoin non préparé | `ai-guard-supervision-bootstrap-unprepared:qa-20261030`, `sha256:b79b569dc068b2bc5ecf3aca0aaa5cb88fbead783fbd0d72111625f9d91904ab` | Image de comparaison |
| Témoin + probe | `ai-guard-supervision-bootstrap-unprepared-probe:qa-20261030`, `sha256:90a9ee55fc358425b3cc70df3441cfcf56b298697874d117bbeef74c043808d9` | Container `cd4bb90f84659bd1a1292b112a5047d590a3c3122c23d6c33413d4b984e1e21b`, exit1. Refus `supervised backup requires private credential directories`; inspection de `/run/secrets`: UID0, mode0755. Copie post-run du volume DATA : seulement les répertoires `a`, `b`, `backups`, aucun fichier DB/backup. |
| Préparée | `ai-guard-supervision-bootstrap-prepared:qa-20261030`, `sha256:da53d03a2dd02d876b688e34a2628930ab893199083bd4a98cd3807625bc7842` | Comparaison avec mkdir/chown/chmod du parent secrets |
| Préparée + probe | `ai-guard-supervision-bootstrap-prepared-probe:qa-20261030`, `sha256:63139bcbb9afdc13f3401d5c9472fe91f45b539853e3eceed8589ee91df87698` | Container `6481d50b8e05cfddf8379f373c8791d231f50c9876bd10cecc9239e39a4509cb`, arrêt propre exit0 |

Le gateway corrigé a passé `agentsandbox healthcheck --url http://127.0.0.1:8787/healthz` via `docker exec` (RC0). Le probe loopback a répondu RC0 : `tenant-a` et `tenant-b`, role `runner`, `schema=3`, `active=true`; namespaces UUID non nil et distincts. Le serveur produit les champs sous forme UUID, mais le probe vérifie seulement longueur36, absence du sentinelle nil et différence entre les deux chaînes; il n’effectue pas de parsing UUID complet. Il a confirmé deux backups, owner1000, mode0600. Le log gateway rapporte deux tenants et quatre événements d’initialisation.

Inspection du conteneur corrigé : utilisateur1000:1000; `ReadonlyRootfs=true`; `NetworkMode=none`; `CapDrop=[ALL]`; `SecurityOpt=[no-new-privileges]`; `NanoCpus=2000000000`; `Memory=536870912`; `PidsLimit=256`; aucun port publié. DATA est l’unique montage rw. Registre et quatre secrets synthétiques sont montés individuellement en ro. Le probe a observé `/run/secrets` UID1000 mode0700; le registre et les quatre projections UID1000 mode0444. Les chemins DB `/data/a/audit.sqlite` et `/data/b/audit.sqlite` existent et appartiennent à UID1000 mode0600; ce constat est limité aux métadonnées de fichiers et au protocole HTTP, sans relecture SQLite/PRAGMA. `/data/backups` est UID1000 mode0700; les deux backups sont non vides, nlink1 et mode0600. Le lien entre tenant et nom de backup n’a pas été vérifié indépendamment dans cette exécution Docker. Le dossier `/tmp/.../corrected-data-copy` contient les DB, backups et fichiers de verrou après arrêt. Les conteneurs, volumes, images et le builder sont conservés; aucun QA ou service existant n’a été arrêté.

## Statuts

- **PASS** — source/manifeste112, compilation Linux release, images comparatives locales hors ligne.
- **PASS** — témoin : refus de configuration avant toute création DB; parent secrets constaté root0755 et volume DATA sans fichier.
- **PASS** — corrigé : permissions, santé HTTP réelle, réponses authentifiées A/B, schéma3 actif, namespaces séparés, DB distinctes et deux backups privés.
- **NOT_RUN** — interruptions/crash/reprise électrique, worker/confinement runtime, service persistant/gardien, charge100, intégrations cloud/Kubernetes, fournisseur/modèle, navigateur/E2E général.
- **NOT_APPLICABLE** — déploiement QA, aucun déploiement QA n’étant prévu dans ce lot.
- **NOT_RUN** — hôte Linux natif, x86_64 et Podman; ces qualifications restent des exigences produit et ne sont pas couvertes par la VM Docker Desktop Linux/ARM64.

## Limite sur le Dockerfile complet

Les images de test sont des variantes construites depuis l’image runtime locale déjà disponible. L’image préparée reproduit les lignes `mkdir -p /run/secrets`, `chown 1000:1000`, `chmod 0700`, puis copie le même binaire et le probe. Le Dockerfile multi-stage complet `images/control.Dockerfile` n’a pas été construit, car ses étapes Node/Rust et `apt-get` demanderaient des images/dépendances supplémentaires; aucun téléchargement n’a été tenté. La preuve qualifie ici le comportement runtime des lignes de préparation du parent secrets et du bootstrap sur cette VM, pas la recette de build complète.

Le premier rapport incomplet et les traces conservées restent dans le dossier privé `/tmp/ai-guard-supervision-bootstrap-qualified.zZ7woB` (mode0700). Index sélectif : `evidence-index.txt`. Gateway logs : `witness.log`, `corrected.log`; sortie healthcheck : `healthcheck.stdout`, `healthcheck.exit-code`; sortie probe : `probe.stdout`, `probe.exit-code`; inspect final : `runtime-inspect.json`; modes/owners observés : `corrected-metadata.stdout`. Manifeste et vérifications source112 : `source.sha256`, `source-reverify.log`, `builder-source-reverify.log` (112/112 OK), sources recopiées depuis le builder arrêté : `builder-source-recopy/`. Dockerfiles comparatifs exacts : `image-context/Dockerfile.unprepared`, `image-context/Dockerfile.prepared`, `image-context/Dockerfile.unprepared.probe`, `image-context/Dockerfile.prepared.probe`.

Les artefacts Linux sont `linux-artifacts/agentsandbox` (SHA-256 `764b91d0297669c0c07db094dd46fb4874f5145bb8966f330a62138b9cc94e9e`), `linux-artifacts/libagentsandbox.rlib` (SHA-256 `dd446b353c38a3d2bd374edc5854bb94ecd2fc8877ee26336c520722505485fc`) et `probe-output/bootstrap-probe` (SHA-256 `9db646573aa00457b4f1fb7b6c775969d3a3057ae73d1ec1b0587b9daed8c80e`). Les copies du binaire d’application recopiées depuis les conteneurs arrêtés sont `runtime-binaries/witness-linux` et `runtime-binaries/corrected-linux`; le manifeste `runtime-binaries.sha256` prouve que les deux correspondent au hash Linux `764b91...`; la copie du probe est `runtime-binaries/probe-linux`.

Les artefacts macOS présents avant ce travail sont séparés et inchangés : `agentsandbox-macos` (SHA-256 `3188c3a6848925a2995d7096fe2f5433d4a609e1eb7daa2c009e70dea5a19b39`) et `libagentsandbox-macos.rlib` (SHA-256 `10d191a1c4e457b57bc358ae9698241ec69b0701036c5da46ee60f87e09376d4`). Les fichiers à la racine `agentsandbox` et `libagentsandbox.rlib` sont également les artefacts natifs préexistants; ils ne sont pas des résultats Linux. Aucun token n’est reproduit dans le rapport. Les namespaces imprimés dans `probe.stdout` sont des observations; les assertions complètes du probe sont décrites ci-dessus.
