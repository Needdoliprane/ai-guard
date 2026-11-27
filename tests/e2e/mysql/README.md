# MySQL 8.4 recipes over TCP

French version: [README.fr.md](README.fr.md). Both versions carry the same commands.

This self-contained recipe checks the two authentication modes of the classic MySQL protocol against a disposable local server. The server is a test fixture: in production the MySQL server, its certificates and its accounts are operated by the customer, and ai-guard only qualifies its own connector and gateway against them. The gateway wiring and the application-level qualification (SQL-02 to SQL-06) are described in [the test strategy](../../../docs/engineering/TEST_STRATEGY.md#stockage-sql-configurable--à-qualifier); passing this preflight alone qualifies nothing. [Architecture contract](../../../docs/engineering/ARCHITECTURE_CONTRACT.md#stockage-sql-par-tenant)

| Profile | QA account | Client settings |
| --- | --- | --- |
| `mysql-password` | Password and `REQUIRE SSL` | Server verification, no client certificate |
| `mysql-mtls` | Password and `REQUIRE X509` | Same server verification, plus client certificate and key |

Both profiles require TLS over TCP. With `REQUIRE X509` the server accepts a valid client certificate signed by the configured CA, and the password is still required. The `qa_probe` account has no rights on any schema and the root account is local to the container. [MySQL TLS](https://dev.mysql.com/doc/refman/8.4/en/using-encrypted-connections.html) · [MySQL ALTER USER](https://dev.mysql.com/doc/refman/8.4/en/alter-user.html)

## Prepare the recipe

You need Docker Compose, the MySQL 8.4 client and OpenSSL 3. Set `QA_MYSQL_IMAGE_DIGEST` to the 64 hexadecimal characters of the manifest digest of the official MySQL 8.4 image for your platform, **already present locally**: Compose never pulls it (`pull_policy: never`), so the image stays pinned by digest. Pull it once, by digest, from a network you trust, for example `docker pull mysql@sha256:<digest>` after checking the digest against the registry. Each recipe uses a new Compose project and two distinct data volumes. The declared ceiling is one CPU and 1 GiB for the active server. Start one profile at a time. [Compose pull policy](https://docs.docker.com/reference/compose-file/services/#pull_policy)

From the repository root, prepare a private directory outside the checkout. The self-signed CA is dedicated to this recipe. Each server certificate carries the SAN of the expected TCP name: `localhost` on the workstation, or the service name on the project network. The second CA and its client certificate exist only for the refusal cases.

```sh
umask 077
export QA_MYSQL_PRIVATE_DIR="$(mktemp -d /tmp/ai-guard-mysql.XXXXXX)"
export QA_MYSQL_PROJECT="ai-guard-mysql-$(openssl rand -hex 5)"
qa="$QA_MYSQL_PRIVATE_DIR"
mkdir -m 700 "$qa/ca" "$qa/other-ca" "$qa/password" "$qa/mtls" "$qa/client"
printf '[req]\ndistinguished_name=dn\n[dn]\n' > "$qa/req.cnf"

make_ca() {
  openssl req -config "$qa/req.cnf" -x509 -newkey rsa:3072 -sha256 -days 7 -noenc \
    -subj "/CN=$2" -keyout "$qa/$1/ca.key" -out "$qa/$1/ca.pem" \
    -addext 'basicConstraints=critical,CA:TRUE,pathlen:0' \
    -addext 'keyUsage=critical,keyCertSign,cRLSign'
}

issue_leaf() {
  leaf_path="$qa/$1"
  openssl req -config "$qa/req.cnf" -new -newkey rsa:2048 -sha256 -noenc -subj "/CN=$2" \
    -keyout "$leaf_path.key" -out "$leaf_path.csr"
  printf '%s\n' 'basicConstraints=critical,CA:FALSE' \
    'keyUsage=critical,digitalSignature,keyEncipherment' \
    "extendedKeyUsage=$4" > "$leaf_path.ext"
  if [ "$#" -eq 5 ]; then
    printf 'subjectAltName=%s\n' "$5" >> "$leaf_path.ext"
  fi
  openssl x509 -req -in "$leaf_path.csr" -CA "$qa/$3/ca.pem" \
    -CAkey "$qa/$3/ca.key" -CAcreateserial -days 3 -sha256 \
    -extfile "$leaf_path.ext" -out "$leaf_path.pem"
}

make_ca ca 'ai-guard MySQL QA'
make_ca other-ca 'ai-guard MySQL QA negative'
issue_leaf password/server mysql-password ca serverAuth 'DNS:localhost,DNS:mysql-password'
issue_leaf mtls/server mysql-mtls ca serverAuth 'DNS:localhost,DNS:mysql-mtls'
issue_leaf client/probe qa-probe ca clientAuth
issue_leaf client/other-probe qa-other-probe other-ca clientAuth

for profile in password mtls; do
  openssl rand -hex 24 > "$qa/$profile/root.password"
  openssl rand -hex 24 > "$qa/$profile/probe.password"
  openssl verify -CAfile "$qa/ca/ca.pem" -purpose sslserver \
    -verify_hostname localhost "$qa/$profile/server.pem"
done
openssl verify -CAfile "$qa/ca/ca.pem" -purpose sslclient "$qa/client/probe.pem"
```

Keep both CA keys on the issuing workstation. Compose mounts only the public CA, the chosen server's certificate and key, and the two passwords of that profile. For an isolated client, use a distinct account or a container that receives only the public CA certificate, the `qa_probe` password and, for mTLS, its own key pair. The server volume and key stay with the MySQL service. [Repository PKI policy](../../../docs/engineering/INFRASTRUCTURE_PKI.md) · [OpenSSL req](https://docs.openssl.org/3.0/man1/openssl-req/) · [OpenSSL x509](https://docs.openssl.org/3.0/man1/openssl-x509/)

Compose secrets from files are read-only mounts. Adapt the readability of the mounted files only to the effective MySQL UID of the image. With Docker Desktop and these host directories at `0700`, the server-side QA files below can be made `0444` so the recipe does not depend on UID/GID remapping. CA keys and client keys keep their private mode. [Compose secrets](https://docs.docker.com/reference/compose-file/services/#secrets)

```sh
chmod 0444 "$qa/ca/ca.pem"
for profile in password mtls; do
  chmod 0444 "$qa/$profile/server.pem" "$qa/$profile/server.key" \
    "$qa/$profile/root.password" "$qa/$profile/probe.password"
done
```

## Run the two profiles in turn

The server publishes only `127.0.0.1:${QA_MYSQL_PORT:-13306}` on the host. For the native client, `localhost` with `--protocol=TCP` forces TCP and validates the SAN. A client already on the project network can use the selected service name and port 3306 with no server mount. Any other host needs a private endpoint and a SAN approved by its owner. [MySQL client TLS modes](https://dev.mysql.com/doc/refman/8.4/en/connection-options.html)

The profile SQL runs after the QA account is created, while a new volume initializes and before TCP listens. An existing volume keeps its configuration. [MySQL 8.4 entrypoint](https://github.com/docker-library/mysql/blob/master/8.4/docker-entrypoint.sh)

The helpers below run the checks without prompts and without a password in the client's arguments or environment. The options file and detailed output stay in a private temporary directory that is removed at the end of each check. A dash as the secret selects a deliberately wrong password, distinct from the generated hexadecimal ones. The positive check requires the exact account, an SQL result, a TLS session and an active `require_secure_transport`. Each refusal requires the expected code and an empty SQL output. When the client turns TLS off with the right password, MySQL may answer `1045` from the `REQUIRE SSL` or `REQUIRE X509` check before the global `3159` guard, so positive witnesses bracket that case. [MySQL option files](https://dev.mysql.com/doc/refman/8.4/en/option-files.html) · [MySQL server variables](https://dev.mysql.com/doc/refman/8.4/en/server-system-variables.html#sysvar_require_secure_transport)

```sh
compose_mysql() { docker compose -f tests/e2e/mysql/compose.yaml "$@"; }
mysql_case() (
  set +a +x
  umask 077
  unset MYSQL_PWD password
  export LC_ALL=C
  label=$1 expected=$2 secret_file=$3 tls_mode=$4
  shift 4
  if [ "$secret_file" = "-" ]; then
    password=invalid-qa-fixture
  else
    IFS= read -r password < "$secret_file" || return 1
    case "$password" in ''|*[!0-9a-f]*) return 1 ;; esac
    [ "${#password}" -eq 48 ] || return 1
  fi
  probe_dir="$(mktemp -d "$qa/client/query.XXXXXX")" || return 1
  trap 'rm -f "$probe_dir/options.cnf" "$probe_dir/out" "$probe_dir/err"; rmdir "$probe_dir"' EXIT
  printf '[client]\npassword=%s\n' "$password" > "$probe_dir/options.cnf" || return 1
  unset password
  code=0 mysql_errno=unknown
  mysql --defaults-file="$probe_dir/options.cnf" --no-login-paths \
    --protocol=TCP --connect-timeout=5 --batch --skip-column-names \
    --host="${QA_MYSQL_HOST:-localhost}" \
    --port="${QA_MYSQL_CLIENT_PORT:-${QA_MYSQL_PORT:-13306}}" \
    --user=qa_probe --ssl-mode="$tls_mode" "$@" \
    --execute="SELECT 'qa-ok', CURRENT_USER(), VERSION(), IF(@@GLOBAL.require_secure_transport, 'secure-required', 'secure-not-required'); SHOW SESSION STATUS LIKE 'Ssl_cipher'; SHOW SESSION STATUS LIKE 'Ssl_version';" \
    > "$probe_dir/out" 2> "$probe_dir/err" || code=$?

  if [ "$expected" = ok ] && [ "$code" -eq 0 ]; then
    if proof=$(awk -F '\t' '
      $1 == "qa-ok" && $2 == "qa_probe@%" && $3 ~ /^8\.4\./ && $4 == "secure-required" { version = $3 }
      $1 == "Ssl_cipher" && length($2) { cipher = 1 }
      $1 == "Ssl_version" && $2 ~ /^TLSv1\.[23]$/ { tls = $2 }
      END {
        if (NR != 3 || !version || !cipher || !tls) exit 1
        print version, tls
      }
    ' "$probe_dir/out"); then
      printf '%s PASS %s\n' "$label" "$proof"
      return 0
    fi
  elif [ "$code" -ne 0 ] && [ ! -s "$probe_dir/out" ]; then
    mysql_errno=$(awk '$1 == "ERROR" && $2 ~ /^[0-9]+$/ { print $2; exit }' "$probe_dir/err")
    case "$expected:$mysql_errno" in
      server-tls:2026|credentials:1045|client-cert:2026|client-cert:1045|plaintext:1045|plaintext:3159)
        printf '%s PASS refused=%s\n' "$label" "$mysql_errno"
        return 0 ;;
    esac
  fi
  printf '%s FAIL unexpected result (exit=%s errno=%s)\n' "$label" "$code" "${mysql_errno:-unknown}" >&2
  return 1
)
wait_mysql_ready() {
  attempts=0
  until mysql_case "$@" 2>/dev/null; do
    attempts=$((attempts + 1))
    if [ "$attempts" -ge 20 ]; then
      printf '%s\n' 'MySQL readiness not confirmed' >&2
      return 1
    fi
    sleep 1
  done
}
compose_mysql --profile mysql-password config --quiet
compose_mysql --profile mysql-mtls config --quiet
```

Run the password profile first from the native client, and check that it is stopped before starting mTLS. The `127.0.0.1` alias reaches the same port but matches no SAN of the certificate, which is the wrong-name case.

```sh
(
  set -eu
  export QA_MYSQL_HOST=localhost QA_MYSQL_CLIENT_PORT="${QA_MYSQL_PORT:-13306}"
  secret_file="$qa/password/probe.password"
  trap 'compose_mysql --profile mysql-password stop mysql-password' EXIT
  compose_mysql --profile mysql-password up -d --pull never mysql-password
  wait_mysql_ready password-positive ok "$secret_file" VERIFY_IDENTITY --ssl-ca="$qa/ca/ca.pem"
  mysql_case password-wrong-ca server-tls "$secret_file" VERIFY_IDENTITY --ssl-ca="$qa/other-ca/ca.pem"
  QA_MYSQL_HOST=127.0.0.1 mysql_case password-wrong-name server-tls "$secret_file" VERIFY_IDENTITY --ssl-ca="$qa/ca/ca.pem"
  mysql_case password-wrong-password credentials - VERIFY_IDENTITY --ssl-ca="$qa/ca/ca.pem"
  mysql_case password-no-tls plaintext "$secret_file" DISABLED
  mysql_case password-positive-again ok "$secret_file" VERIFY_IDENTITY --ssl-ca="$qa/ca/ca.pem"
  compose_mysql --profile mysql-password stop mysql-password
  trap - EXIT
)
```

```sh
(
  set -eu
  export QA_MYSQL_HOST=localhost QA_MYSQL_CLIENT_PORT="${QA_MYSQL_PORT:-13306}"
  secret_file="$qa/mtls/probe.password"
  client_cert="$qa/client/probe.pem"
  client_key="$qa/client/probe.key"
  trap 'compose_mysql --profile mysql-mtls stop mysql-mtls' EXIT
  compose_mysql --profile mysql-mtls up -d --pull never mysql-mtls
  wait_mysql_ready mtls-positive ok "$secret_file" VERIFY_IDENTITY \
    --ssl-ca="$qa/ca/ca.pem" --ssl-cert="$client_cert" --ssl-key="$client_key"
  mysql_case mtls-no-client-cert credentials "$secret_file" VERIFY_IDENTITY --ssl-ca="$qa/ca/ca.pem"
  mysql_case mtls-wrong-client-ca client-cert "$secret_file" VERIFY_IDENTITY \
    --ssl-ca="$qa/ca/ca.pem" --ssl-cert="$qa/client/other-probe.pem" --ssl-key="$qa/client/other-probe.key"
  mysql_case mtls-wrong-server-ca server-tls "$secret_file" VERIFY_IDENTITY \
    --ssl-ca="$qa/other-ca/ca.pem" --ssl-cert="$client_cert" --ssl-key="$client_key"
  mysql_case mtls-wrong-password credentials - VERIFY_IDENTITY \
    --ssl-ca="$qa/ca/ca.pem" --ssl-cert="$client_cert" --ssl-key="$client_key"
  mysql_case mtls-positive-again ok "$secret_file" VERIFY_IDENTITY \
    --ssl-ca="$qa/ca/ca.pem" --ssl-cert="$client_cert" --ssl-key="$client_key"
  compose_mysql --profile mysql-mtls stop mysql-mtls
  trap - EXIT
)
```

The last positive check must pass again after each series. A generic disconnect, a timeout or an availability failure validates no expected refusal. Keep the profile, the server and TLS versions and the result categories. QA volumes remain available after the stop. [TLS session verification](https://dev.mysql.com/doc/refman/8.4/en/using-encrypted-connections.html)

The [gateway SQL-06 fixture](GATEWAY_FIXTURE.md) prepares fresh schemas and dedicated accounts for the application tests.

## Fresh owned MySQL publication (QA)

Use two private files in the existing tenant registry format. Pass the operator
registry as `--registry` and the registry used at startup by the gateway of
the same installation as `--gateway-registry` (the gateway's `--tenants-file`).
Before connecting, the CLI checks that the selected tenant has the same
namespace, host, port and database in both files and different SQL usernames.
This check covers the supplied gateway registry. Use a dedicated empty schema
and keep other writers and schema administration off it during publication.

The CLI opens only the operator's authentication files. Both TLS profiles
require a password file and verify the server name and certificate chain
against the configured CA. For mTLS, also reference the operator's client
certificate and key. Before publication, the connection verifies that the
server selected the expected operator username.
[Operator account and target requirements](OWNED_PREPARATION.md#compte-opérateur-du-prévol)

Obtain the expected server UUID independently from a trusted administrative
session on the intended server (`SELECT @@GLOBAL.server_uuid`). Assign the
operation, installation and source UUIDs once and retain them with the chosen
format. Use canonical lowercase, hyphenated, non-nil UUIDs. The example selects
format 15. Use `--schema-version 14` for format 14.

```sh
agentsandbox prepare-mysql-owned \
  --registry "${QA_MYSQL_OPERATOR_REGISTRY:?}" \
  --gateway-registry "${QA_MYSQL_GATEWAY_REGISTRY:?}" --tenant "${QA_MYSQL_TENANT:?}" \
  --schema-version 15 --server-uuid "${QA_MYSQL_SERVER_UUID:?}" \
  --operation-id "${QA_MYSQL_OPERATION_ID:?}" \
  --installation-id "${QA_MYSQL_INSTALLATION_ID:?}" \
  --source-id "${QA_MYSQL_SOURCE_ID:?}"
```

On success, stdout contains one verified JSON receipt. After a lost receipt,
repeat the command with the same target, format and UUIDs. On a nonempty
schema, this is a read-only check on the locked SQL session. It verifies the
manifest, the exact operation anchor, the tenant and namespace, the initial
audit chain and head, and the empty projections. An exact match returns the
same publication head. An incomplete, foreign or divergent target, or a
target that has advanced beyond initial publication, is refused and preserved.

The command performs no automatic SQLite or MySQL migration. Gateway
admission, including its owner epoch, and restoration require separate
qualification. CLI qualification is pending.
[Existing publication evidence](../../../docs/engineering/VALIDATION.md#27-novembre-2026--publication-unique-mysql-owned-14-et-15)

## What this preflight does not cover

It proves that the server, accounts and certificates behave as the gateway tests expect. It does not prove ai-guard behaviour: the gateway startup with each profile, the refusal of a wrong password, CA, server name or client certificate with no business audit row, and the SQL-04 to SQL-06 application scenarios are separate verdicts recorded in `docs/engineering/`. An expired client certificate is not issued by the helpers above: issue one with a validity window in the past (for example with `openssl ca -startdate/-enddate`) in a private directory when that case is needed.
