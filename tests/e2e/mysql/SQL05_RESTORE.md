# SQL-05 · Restauration logique MySQL après un claim Legacy

Conducteur préparé sur `41863d96`. Statut des deux profils **NOT_RUN**. Cette
recette vérifie la restauration du Store après un claim commité. La coupure
gateway, les claims avant commit, le fencing physique et `owned_sources`
gardent leurs propres verdicts. [Contrat SQL-05](GATEWAY_FIXTURE.md#sql-05-after-a-committed-legacy-claim--sql-05-après-commit-dun-claim-legacy)

## Préparer une paire jetable par profil

1. Dans une future fenêtre d'exécution autorisée, utiliser le contexte
   `desktop-linux`, le [Compose existant](compose.yaml) et la
   [recette TLS/mTLS](README.fr.md). Jouer `mysql-password`, puis
   `mysql-mtls`, avec une paire neuve à chaque passage. Réutiliser l'image
   MySQL 8.4.13 déjà disponible pour la plateforme et vérifier son digest
   avant démarrage. Le manifeste ARM64 de la recette est
   `sha256:f4539b86c82f2c168fc95b439448bde3b9f48e8460344ce41e39081867d2b58e`.
2. Appliquer deux fois la préparation privée de la recette. Relever pour
   chaque serveur son projet Compose neuf, son répertoire canonique hors
   dépôt, son port loopback libre, son nom de conteneur et son volume neuf.
   Utiliser un sous-répertoire privé neuf par passage pour le dump, le
   témoin et les diagnostics. Appliquer `umask 077` et désactiver le
   traçage shell. Démarrer uniquement le profil retenu dans chaque projet.
   Vérifier la publication exacte `127.0.0.1:<port>` des deux conteneurs
   et relever leurs `@@server_uuid` distincts.
3. Exécuter sur chaque port les témoins de transport de la recette, avec sa
   propre CA et ses propres accès. Exiger une connexion `VERIFY_IDENTITY`
   sur `localhost`, ainsi que le refus de CA et de nom erronés. Pour mTLS,
   encadrer le refus sans certificat par deux connexions réussies avec
   certificat. Un serveur indisponible interrompt le passage.
4. Préparer la source avec `prepare_gateway_fixture witness` et ses six DDL.
   Noter le chemin de son JSON privé. Avec les paramètres Compose et les
   fichiers TLS de la cible, appeler `prepare_gateway_fixture witness "$namespace"`,
   où `namespace` vient du JSON source. Le helper prépare un schéma vide,
   deux nouveaux mots de passe et un JSON privé lié à la cible. Vérifier que
   le schéma contient zéro table avant import. [Préparation de fixture](GATEWAY_FIXTURE.md#create-a-fresh-target--créer-une-cible-neuve)

## Commit, sauvegarde, import et rejeu

Réutiliser un libtest précompilé dont la provenance correspond au gel
`41863d96` et qui inclut `mysql-storage`. Vérifier que son inventaire
contient exactement le sélecteur ci-dessous. En l'absence de ce binaire,
laisser l'exécution `NOT_RUN` jusqu'à autorisation de build. Dans les
commandes futures, renseigner les chemins absolus `libtest`, `source_json`,
`target_json` et `run_dir`, puis les noms vérifiés `source_container` et
`target_container`. Le répertoire `run_dir` doit être neuf et privé.

```sh
run_phase() {
  env -i PATH=/opt/homebrew/bin:/usr/bin:/bin \
    AI_GUARD_MYSQL_RESTORE_CONFIG="$2" \
    AI_GUARD_MYSQL_RESTORE_WITNESS="$run_dir/witness.json" \
    AI_GUARD_MYSQL_RESTORE_PHASE="$1" \
    "$libtest" --ignored --exact --test-threads=1 --show-output \
    worker_lifecycle::restored_mysql_legacy_claim_retains_receipt_and_quota
}
```

1. Exécuter `run_phase prepare "$source_json"` et consigner sa sortie dans
   le répertoire privé. Exiger le code 0, exactement un test réussi et
   `SQL05_SOURCE_CLAIM_PREPARED`. Garder le témoin au chemin d'origine.
   Attendre la fin du processus et maintenir la source sans autre écrivain
   pendant la sauvegarde. En cas d'échec, conserver le diagnostic et
   reprendre sur une paire neuve après correction.
2. Sauvegarder le seul schéma source. Le helper ci-dessous reprend le
   fichier d'options temporaire et le socket d'administration de la
   fixture. Exécuter le bloc depuis le répertoire de travail de la
   recette. Vérifier chaque statut avant l'étape suivante.

```sh
fixture_sql() {
  local container=$1
  shift
  docker --context desktop-linux exec -i "$container" sh -eu -c '
    umask 077
    options=$(mktemp)
    trap '\''unlink "$options"'\'' EXIT
    IFS= read -r root_password < /run/secrets/root_password
    printf "[client]\nuser=root\npassword=%s\n" "$root_password" > "$options"
    unset root_password
    command=$1
    shift
    "$command" --defaults-file="$options" --no-login-paths --protocol=SOCKET "$@"
  ' sh "$@"
}

backup_restore() (
  set -eu
  umask 077
  db=$(jq -er '.database | select(test("^qa_audit_[0-9a-f]{32}$"))' "$source_json")
  test "$db" = "$(jq -er '.database' "$target_json")"
  test "$source_container" != "$target_container"
  fixture_sql "$source_container" mysqldump --single-transaction --quick \
    --no-tablespaces --set-gtid-purged=OFF --skip-add-drop-table "$db" \
    </dev/null >"$run_dir/backup.sql" 2>"$run_dir/dump.err"
  printf 'dump_exit=0\n'
  test -s "$run_dir/backup.sql"
  (cd "$run_dir" && shasum -a 256 backup.sql >backup.sha256)
  (cd "$run_dir" && shasum -a 256 -c backup.sha256)
  fixture_sql "$target_container" mysql --batch --database="$db" \
    <"$run_dir/backup.sql" >"$run_dir/import.out" 2>"$run_dir/import.err"
  printf 'import_exit=0\n'
)
```

3. Exécuter `backup_restore`. Consigner son statut et les jalons dump/import.
   L'import doit utiliser ce fichier, avec le schéma cible vide confirmé à
   l'étape de préparation. Le dump contient le DDL et les données du schéma
   de recette. Identités serveur, comptes globaux et clés TLS sont gérés
   séparément par chaque projet. Après un échec d'import, conserver les
   diagnostics et utiliser une destination neuve pour un nouveau passage.
4. Arrêter le service source en reprenant précisément son projet, son
   répertoire privé et son port dans Compose. Vérifier son état arrêté et
   le refus TCP sur son port loopback avant de lancer `run_phase verify
   "$target_json"`. Consigner la sortie dans le répertoire privé. Exiger
   le code 0, exactement un test réussi et
   `SQL05_RESTORED_LEGACY_CLAIM_VERIFIED`. Le test exige le reçu déjà présent,
   le rejeu `AlreadyConsumed` avec reçu identique, la conservation de la
   tête d'audit, de la tentative, de sa révision et du quota réservé, puis
   le refus d'une admission distincte par saturation du quota.

## Preuve et fin de passage

Conserver un reçu expurgé par profil avec le commit, l'empreinte du libtest,
le digest de l'image, les UUID serveur, les ports, les verdicts de transport,
les statuts de préparation/dump/import/vérification, le contrôle du digest
du dump et l'observation de l'arrêt de la source. Garder le dump, son
empreinte, le témoin, les configurations et les sorties brutes dans le
répertoire privé. Un échec de prérequis laisse les étapes non jouées
`NOT_RUN`. Un succès du test seul laisse la restauration non qualifiée
tant que le dump, l'import et l'isolement de la source ne sont pas attestés.

En fin de passage, y compris après échec, arrêter les services de recette
encore actifs à partir de leurs paramètres Compose respectifs. Vérifier
l'état arrêté et la fermeture des deux ports, puis consigner le sort des
volumes et des pièces privées. Toute suppression ultérieure porte
uniquement sur les ressources jetables identifiées et autorisées.
