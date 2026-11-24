# Gateway SQL-06 fixture · Fixture gateway SQL-06

Use one already authorized, disposable Docker Desktop MySQL 8.4 server prepared with the [TLS or mTLS recipe](README.md). Run one profile at a time. Each test invocation needs fresh schemas. The gateway tests use distinct, namespace-bound SQL accounts on the same loopback endpoint, with a read-only service account for the witness. The historical Store and probe tests still use their own fixtures.

Utiliser un serveur MySQL 8.4 Docker Desktop jetable, déjà autorisé et préparé avec la [recette TLS ou mTLS](README.fr.md). Exécuter un profil à la fois. Chaque test demande des schémas neufs. Les tests du gateway utilisent des comptes SQL distincts liés à leurs namespaces sur le même endpoint loopback, avec un compte de service en lecture seule pour le témoin. Les tests historiques du Store et de la sonde gardent leurs fixtures.

## Create a fresh target · Créer une cible neuve

From the source checkout, set `QA_MYSQL_PROFILE` to `mysql-password` or `mysql-mtls`. Keep the project name, private material directory and loopback port from the running QA profile. The commands use `jq`, `uuidgen` and OpenSSL to create names and private references for this invocation. The SQL client stops on the first error. Credentials and raw diagnostics stay in the private directory. The maintenance role can prepare only its own schema.

Depuis le checkout source, définir `QA_MYSQL_PROFILE` à `mysql-password` ou `mysql-mtls`. Reprendre le projet, le répertoire privé et le port loopback du profil QA en cours. Les commandes utilisent `jq`, `uuidgen` et OpenSSL pour créer les noms et références privées de cette invocation. Le client SQL s'arrête à la première erreur. Les credentials et diagnostics bruts restent dans le répertoire privé. Le compte de maintenance prépare uniquement son schéma.

```sh
prepare_gateway_fixture() (
  set +x
  set -eu
  umask 077
  qa_role=$1
  case "$qa_role" in writer|witness) ;; *) return 2 ;; esac
  case "$QA_MYSQL_PROFILE" in
    mysql-password) qa_requirement=SSL ;;
    mysql-mtls) qa_requirement=X509 ;;
    *) return 2 ;;
  esac
  qa_container="${QA_MYSQL_PROJECT:?}-${QA_MYSQL_PROFILE}-1"
  qa_port=${QA_MYSQL_PORT:-13306}
  test "$(docker --context desktop-linux port "$qa_container" 3306/tcp)" = "127.0.0.1:$qa_port"

  qa_base=$(cd "${QA_MYSQL_PRIVATE_DIR:?}" && pwd -P)
  qa_target=$(mktemp -d "$qa_base/gateway-${qa_role}.XXXXXX")
  restore_namespace=${2:-}
  if test -n "$restore_namespace"; then
    test "$qa_role" = witness
    printf '%s\n' "$restore_namespace" | grep -Eq '^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$'
  fi
  qa_namespace=$(uuidgen | tr '[:upper:]' '[:lower:]')
  qa_namespace=${restore_namespace:-$qa_namespace}
  qa_hex=$(printf '%s' "$qa_namespace" | tr -d '-')
  qa_tag=$(printf '%s' "$qa_hex" | cut -c1-24)
  qa_db="qa_audit_$qa_hex"
  qa_grant_db="qa\\_audit\\_$qa_hex"
  qa_service="qa_s_$qa_tag"
  qa_maintenance="qa_m_$qa_tag"
  openssl rand -hex 32 > "$qa_target/service.password"
  openssl rand -hex 32 > "$qa_target/maintenance.password"
  install -m 600 "$qa_base/ca/ca.pem" "$qa_target/ca.pem"
  qa_cert=
  qa_key=
  if test "$qa_requirement" = X509; then
    qa_cert=client.pem
    qa_key=client.key
    install -m 600 "$qa_base/client/probe.pem" "$qa_target/$qa_cert"
    install -m 600 "$qa_base/client/probe.key" "$qa_target/$qa_key"
  fi

  {
    printf 'CREATE DATABASE `%s`;\n' "$qa_db"
    if test -z "$restore_namespace"; then
      printf 'USE `%s`;\n' "$qa_db"
      cat schema/mysql/audit-v1.sql schema/mysql/decisions-v1.sql \
        schema/mysql/workers-v1.sql schema/mysql/worker-lifecycle-v1.sql \
        schema/mysql/supervision-v1.sql schema/mysql/worker-supervision-v1.sql
    fi
    IFS= read -r qa_service_password < "$qa_target/service.password"
    IFS= read -r qa_maintenance_password < "$qa_target/maintenance.password"
    printf "CREATE USER '%s'@'%%' IDENTIFIED BY '%s' REQUIRE %s;\n" \
      "$qa_service" "$qa_service_password" "$qa_requirement"
    printf "CREATE USER '%s'@'%%' IDENTIFIED BY '%s' REQUIRE %s;\n" \
      "$qa_maintenance" "$qa_maintenance_password" "$qa_requirement"
    unset qa_service_password qa_maintenance_password
    printf "GRANT SELECT ON \`%s\`.* TO '%s'@'%%';\n" "$qa_grant_db" "$qa_service"
    printf "GRANT SELECT, INSERT, UPDATE, CREATE, REFERENCES ON \`%s\`.* TO '%s'@'%%';\n" \
      "$qa_grant_db" "$qa_maintenance"
    if test "$qa_role" = writer; then
      printf "GRANT INSERT ON \`%s\`.ai_guard_audit_event TO '%s'@'%%';\n" "$qa_db" "$qa_service"
      printf "GRANT UPDATE ON \`%s\`.ai_guard_audit_state TO '%s'@'%%';\n" "$qa_db" "$qa_service"
    fi
  } > "$qa_target/create.sql"
  trap 'unlink "$qa_target/create.sql"' EXIT
  docker --context desktop-linux exec -i "$qa_container" sh -eu -c '
    umask 077
    options=$(mktemp)
    trap '"'"'unlink "$options"'"'"' EXIT
    IFS= read -r root_password < /run/secrets/root_password
    printf "[client]\nuser=root\npassword=%s\n" "$root_password" > "$options"
    unset root_password
    mysql --defaults-file="$options" --protocol=SOCKET --batch
  ' < "$qa_target/create.sql" > "$qa_target/admin.out" 2>&1

  jq -n --argjson port "$qa_port" --arg db "$qa_db" --arg namespace "$qa_namespace" \
    --arg service "$qa_service" --arg maintenance "$qa_maintenance" \
    --arg cert "$qa_cert" --arg key "$qa_key" '
    {host:"localhost",port:$port,database:$db,namespace:$namespace,username:$service,
     maintenance_username:$maintenance,password_file:"service.password",
     maintenance_password_file:"maintenance.password",ca_file:"ca.pem",
     client_cert_file:(if $cert == "" then null else $cert end),
     client_key_file:(if $key == "" then null else $key end)}
  ' > "$qa_target/fixture.json"
  printf '%s\n' "$qa_target/fixture.json"
)
```

The escaped underscores in the schema grants constrain the privileges to the literal schema name. The witness check plans an update to verify its refusal without executing the update. [MySQL GRANT](https://dev.mysql.com/doc/refman/8.4/en/grant.html) · [MySQL EXPLAIN](https://dev.mysql.com/doc/refman/8.4/en/explain.html)

Les underscores échappés dans les GRANT réservent les droits au nom littéral du schéma. Le contrôle du témoin demande le plan d'une mise à jour pour vérifier son refus sans l'exécuter. [MySQL GRANT](https://dev.mysql.com/doc/refman/8.4/en/grant.html) · [MySQL EXPLAIN](https://dev.mysql.com/doc/refman/8.4/en/explain.html)

## Run an exact case · Exécuter un test exact

```sh
qa_config_a=$(prepare_gateway_fixture writer) || exit 1
qa_config_b=$(prepare_gateway_fixture witness) || exit 1
export AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A="$qa_config_a"
export AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B="$qa_config_b"
cargo test --locked --offline --features mysql-storage --test mysql_audit_store -- \
  --ignored --exact identity::gateway::gateway_appends_to_the_prepared_target_and_replays_after_restart \
  --test-threads=1
```

| Exact suffix under `identity::gateway::` | Fresh targets · Cibles neuves |
| --- | --- |
| `gateway_requires_identity_and_workload_projections_before_listening` | A |
| `gateway_refuses_wrong_mysql_material_and_writes_no_business_row` | A |
| `gateway_appends_to_the_prepared_target_and_replays_after_restart` | A + B |
| `gateway_commits_ordinary_batch_and_replays_after_restart` | A + B |
| `gateway_reports_unverified_selected_mysql_audit_and_recovers` | A + B |
| `gateway_retains_verified_neighbor_collisions_during_projection_failure` | A + B |
| `gateway_issues_sqlite_workload_credential_when_mysql_neighbor_verifies` | B. SQLite A is created by the test. SQLite A est créé par le test. |
| `the_control_guard_refuses_inside_the_append_transaction` | A |

Use an absolute, non-symlink `TMPDIR` and the same source revision for the gateway binary and tests. The refused-material case accepts file references in `AI_GUARD_MYSQL_GATEWAY_OTHER_CA`, `AI_GUARD_MYSQL_GATEWAY_EXPIRED_CERT` with `AI_GUARD_MYSQL_GATEWAY_EXPIRED_KEY`, and `AI_GUARD_MYSQL_GATEWAY_FOREIGN_CERT` with `AI_GUARD_MYSQL_GATEWAY_FOREIGN_KEY`. Record an omitted optional case as `NOT_RUN`. Keep the source SHA, profile, server version and digest, exact selector, exit status, and redacted assertions. Keep all private files and raw output out of the receipt.

Utiliser un `TMPDIR` absolu sans lien symbolique et la même révision pour le binaire gateway et les tests. Les références facultatives indiquées ci-dessus activent les refus de CA, de certificat expiré ou de certificat étranger. Inscrire `NOT_RUN` pour tout cas facultatif omis. Conserver le SHA source, le profil, la version et le digest du serveur, le sélecteur exact, le code de sortie et les assertions expurgées. Garder les fichiers privés et les sorties brutes hors du reçu.

## Read-only origin preflight · Prévol d’origine en lecture seule

Prepare fresh A with `writer` and fresh B with `witness` for each profile. Use a probe built at the test’s source revision. The test selects the read-only service account for B, checks the exact receipt for the expected head, then supplies a different head and checks for a refusal without a receipt. After each invocation it verifies that no session holds the expected lock. It compares both audits before and after the invocations. Both profiles use the existing audit schema. Owned formats 14 and 15 require separate acceptance tests.

Pour chaque profil, préparer A avec `writer` et B avec `witness`, tous deux neufs. Utiliser une sonde construite à la révision du test. Le test sélectionne le compte de service en lecture seule de B, vérifie le reçu exact de la tête attendue, puis soumet une tête différente et vérifie le refus sans reçu. Après chaque appel, il vérifie qu’aucune session ne détient le verrou attendu. Il compare les deux audits avant et après les appels. Les deux profils utilisent le schéma d’audit existant. Les formats owned 14 et 15 exigent des tests d’acceptation distincts.

```sh
cargo build --locked --offline --features mysql-storage --example mysql_storage_probe
AI_GUARD_MYSQL_REGISTRY_QA_BIN="$PWD/target/debug/examples/mysql_storage_probe" \
  cargo test --locked --offline --features mysql-storage --test mysql_audit_store -- \
  --ignored --exact identity::prepared_registry_probe_verifies_owned_origin_and_preserves_audits \
  --test-threads=1
```

## SQL-05 after a committed Legacy claim · SQL-05 après commit d’un claim Legacy

Run one pair of disposable MySQL 8.4.13 servers per transport profile, using the
same pinned image and private TLS or mTLS recipe as the outage verification. Use
distinct projects, fresh volumes, different loopback ports and distinct MySQL
server UUIDs. Verify the server name and chain and, for mTLS, confirm refusal
without the client certificate. Use the fixture above to prepare the source
schema, with its six DDL inputs. The oracle writes through the schema-scoped
maintenance account and keeps all configuration and witness files in a private
canonical directory. Keep the witness path unused before preparation.

Pour chaque profil TLS ou mTLS, utiliser deux serveurs MySQL 8.4.13 jetables issus
de l’image épinglée de la recette de coupure. Les projets, volumes neufs, ports
loopback et UUID serveur sont distincts. Vérifier le nom et la chaîne du serveur
et, en mTLS, le refus sans certificat client. Préparer le schéma source avec les
six DDL de la fixture ci-dessus. L’oracle écrit avec le compte de maintenance
limité à ce schéma. Placer les configurations et le témoin dans un répertoire
privé canonique. Le chemin du témoin doit être neuf.

Set `AI_GUARD_MYSQL_RESTORE_CONFIG` to the private fixture JSON for the active
phase and `AI_GUARD_MYSQL_RESTORE_WITNESS` to the absolute path of the same
private witness across phases. Run the exact selector twice, first with
`AI_GUARD_MYSQL_RESTORE_PHASE=prepare` and then with
`AI_GUARD_MYSQL_RESTORE_PHASE=verify` after the external restore.

Renseigner `AI_GUARD_MYSQL_RESTORE_CONFIG` avec le JSON privé de la phase et
`AI_GUARD_MYSQL_RESTORE_WITNESS` avec le chemin absolu du même témoin privé
pour les deux phases. Jouer le sélecteur exact avec
`AI_GUARD_MYSQL_RESTORE_PHASE=prepare`, puis avec
`AI_GUARD_MYSQL_RESTORE_PHASE=verify` après la restauration externe.

```sh
cargo test --locked --no-default-features --features mysql-storage --test mysql_audit_store -- \
  --ignored --exact --test-threads=1 --show-output \
  worker_lifecycle::restored_mysql_legacy_claim_retains_receipt_and_quota
```

1. `prepare` commits a Legacy `StartClaim` and writes the witness at
   `StartIssued` before any runtime observation. Require one passing selector
   and `SQL05_SOURCE_CLAIM_PREPARED`. Quiesce the source writers. Take a
   consistent logical backup of its fixture schema, including DDL and all data.
2. Import the actual backup into the fresh destination instance under the same
   schema name. Recreate the namespace-bound fixture accounts with fresh private
   credentials and the same schema-local grants and SSL/X509 requirements.
   Keep the original namespace in the destination JSON. Never import global
   MySQL users, server identity or TLS keys. Keep backup/restore exit statuses
   and the private backup digest, then stop the source and verify that its port
   is unreachable before starting `verify`. Provide only destination access
   to that process.
3. Require one passing selector and `SQL05_RESTORED_LEGACY_CLAIM_VERIFIED`.
   The restored original receipt must already exist. Exact replay returns
   `AlreadyConsumed` with an identical receipt and preserves the verified audit
   head, attempt, revision and held worker/CPU/memory. A distinct admission must
   then be refused for quota saturation. Collect a separate receipt per profile
   with the backup/restore evidence and fixture cleanup outcome.

1. `prepare` commite le claim au point `StartIssued` avant toute observation
   runtime. Exiger un test réussi et `SQL05_SOURCE_CLAIM_PREPARED`. Arrêter les
   écritures puis capturer une sauvegarde logique cohérente du DDL et des
   données du seul schéma de recette.
2. Importer cette sauvegarde sur le serveur neuf sous le même nom de schéma.
   Recréer les comptes de fixture avec des credentials neufs et les mêmes
   privilèges et contraintes SSL/X509. Conserver le namespace dans le JSON
   cible. Exclure comptes globaux, identité serveur et clés TLS du transfert.
   Conserver les statuts de sauvegarde et de restauration et l’empreinte privée
   du dump, puis arrêter la source et vérifier que son port est inaccessible
   avant `verify`. Donner à ce processus les seuls accès de la cible.
3. Exiger un test réussi et `SQL05_RESTORED_LEGACY_CLAIM_VERIFIED`. Le reçu
   original doit préexister au rejeu. Le rejeu exact rend `AlreadyConsumed`
   avec le même reçu, préserve la tête d’audit vérifiée, la tentative, sa
   révision et le quota worker/CPU/mémoire. Une admission distincte est ensuite
   refusée pour saturation. Conserver un reçu par profil avec les preuves de
   sauvegarde/restauration et le résultat du nettoyage.

The restore selector checks database state. A restoration result also requires
the external backup and import evidence. Preparation, SQL seeding of the
destination or a reopening of the source cannot establish a restoration PASS.
This slice covers Legacy after a claim, with synthetic runtime observations
limited to creation metadata. Pre-claim backups, physical fencing and
`owned_sources` remain separate.

Le sélecteur vérifie l’état de la base. Un PASS de restauration exige aussi
les preuves externes de sauvegarde et d’import. La préparation, l’amorçage
SQL de la destination ou la réouverture de la source ne produisent aucun PASS
de restauration. Cette tranche couvre Legacy après claim, avec des observations
runtime synthétiques limitées aux métadonnées de création. Les sauvegardes
antérieures au claim, le fencing physique et `owned_sources` restent distincts.
