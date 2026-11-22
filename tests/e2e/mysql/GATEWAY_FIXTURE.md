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
  qa_namespace=$(uuidgen | tr '[:upper:]' '[:lower:]')
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
    printf 'CREATE DATABASE `%s`;\nUSE `%s`;\n' "$qa_db" "$qa_db"
    cat schema/mysql/audit-v1.sql schema/mysql/decisions-v1.sql \
      schema/mysql/workers-v1.sql schema/mysql/worker-lifecycle-v1.sql \
      schema/mysql/supervision-v1.sql schema/mysql/worker-supervision-v1.sql
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
| `gateway_retains_verified_neighbor_collisions_during_projection_failure` | A + B |
| `gateway_issues_sqlite_workload_credential_when_mysql_neighbor_verifies` | B. SQLite A is created by the test. SQLite A est créé par le test. |
| `the_control_guard_refuses_inside_the_append_transaction` | A |

Use an absolute, non-symlink `TMPDIR` and the same source revision for the gateway binary and tests. The refused-material case accepts file references in `AI_GUARD_MYSQL_GATEWAY_OTHER_CA`, `AI_GUARD_MYSQL_GATEWAY_EXPIRED_CERT` with `AI_GUARD_MYSQL_GATEWAY_EXPIRED_KEY`, and `AI_GUARD_MYSQL_GATEWAY_FOREIGN_CERT` with `AI_GUARD_MYSQL_GATEWAY_FOREIGN_KEY`. Record an omitted optional case as `NOT_RUN`. Keep the source SHA, profile, server version and digest, exact selector, exit status, and redacted assertions. Keep all private files and raw output out of the receipt.

Utiliser un `TMPDIR` absolu sans lien symbolique et la même révision pour le binaire gateway et les tests. Les références facultatives indiquées ci-dessus activent les refus de CA, de certificat expiré ou de certificat étranger. Inscrire `NOT_RUN` pour tout cas facultatif omis. Conserver le SHA source, le profil, la version et le digest du serveur, le sélecteur exact, le code de sortie et les assertions expurgées. Garder les fichiers privés et les sorties brutes hors du reçu.
