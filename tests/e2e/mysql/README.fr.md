# Recettes MySQL 8.4 sur TCP

Cette recette autonome vérifie deux modes d’authentification sur le protocole MySQL classique. Ce serveur est une fixture jetable : en production, le serveur MySQL, ses certificats et ses comptes sont exploités par le client. L’intégration avec le gateway et les scénarios SQL-02 à SQL-06 sont des verdicts distincts, consignés dans `docs/engineering/`. La version anglaise est [README.md](README.md). [Contrat d’architecture](../../../docs/engineering/ARCHITECTURE_CONTRACT.md#stockage-sql-par-tenant)

| Profil | Compte QA | Paramètres client |
| --- | --- | --- |
| `mysql-password` | Mot de passe et `REQUIRE SSL` | Vérification du serveur, sans certificat client |
| `mysql-mtls` | Mot de passe et `REQUIRE X509` | Même vérification du serveur, plus certificat client et clé |

Les deux profils exigent TLS sur TCP. Avec `REQUIRE X509`, le serveur accepte un certificat client valide signé par la CA configurée. Le mot de passe reste requis. Le compte `qa_probe` est créé sans droits sur un schéma et le compte root est local au conteneur. [MySQL TLS](https://dev.mysql.com/doc/refman/8.4/en/using-encrypted-connections.html) · [MySQL ALTER USER](https://dev.mysql.com/doc/refman/8.4/en/alter-user.html)

## Préparer la recette

Prévoir Docker Compose, le client `mysql` 8.4 et OpenSSL 3 déjà disponibles. Fournir `QA_MYSQL_IMAGE_DIGEST` avec les 64 caractères hexadécimaux du digest de manifeste de l’image officielle MySQL 8.4 déjà disponible localement pour la plateforme. Chaque recette utilise un projet neuf et deux volumes de données distincts. Le plafond déclaré est d’un CPU et d’un Gio pour le serveur actif. Lancer un seul profil à la fois. [Politique de tirage Compose](https://docs.docker.com/reference/compose-file/services/#pull_policy)

Depuis la racine du dépôt, préparer un répertoire privé hors du checkout. La CA autosignée est propre à cette recette. Les certificats serveur ont chacun les SAN du nom TCP prévu, `localhost` sur le poste ou le nom de service sur le réseau du projet. La seconde CA et son certificat client servent uniquement aux refus.

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

Garder les deux clés de CA sur le poste émetteur. Compose monte uniquement la CA publique, le certificat et la clé du serveur choisi, et les deux mots de passe de ce profil. Pour un client isolé, utiliser un compte distinct ou un conteneur auquel on remet uniquement le certificat public de CA, le mot de passe `qa_probe` et, pour le mTLS, sa propre paire. Le volume et la clé serveur restent au service MySQL. Le certificat client négatif et son matériel restent dans le périmètre QA. Les commandes loopback exécutées par le compte de préparation vérifient le protocole avec ce compte. [Politique PKI du dépôt](../../../docs/engineering/INFRASTRUCTURE_PKI.md#invariants-de-gestion-des-clés) · [OpenSSL req](https://docs.openssl.org/3.0/man1/openssl-req/) · [OpenSSL x509](https://docs.openssl.org/3.0/man1/openssl-x509/)

Les secrets Compose provenant de fichiers sont des montages en lecture seule. Adapter la lisibilité des seuls fichiers montés à l’UID MySQL effectif de l’image. Avec Docker Desktop et ces répertoires hôte en `0700`, on peut rendre lisibles en `0444` les seuls fichiers serveur QA ci-dessous pour éviter de dépendre du remappage UID/GID. Les clés de CA et clés clients gardent leur mode privé. [Mode et UID des secrets Compose](https://docs.docker.com/reference/compose-file/services/#secrets)

```sh
chmod 0444 "$qa/ca/ca.pem"
for profile in password mtls; do
  chmod 0444 "$qa/$profile/server.pem" "$qa/$profile/server.key" \
    "$qa/$profile/root.password" "$qa/$profile/probe.password"
done
```

## Exécuter les deux profils à tour de rôle

Le serveur publie uniquement `127.0.0.1:${QA_MYSQL_PORT:-13306}` sur l’hôte. Pour le client natif, `localhost` avec `--protocol=TCP` force TCP et valide le SAN. Un client déjà présent sur le réseau du projet peut utiliser le nom du service sélectionné et le port 3306, sans montage serveur. Une recette depuis un autre hôte nécessite un endpoint privé et un SAN approuvés par son propriétaire. [Modes TLS du client MySQL](https://dev.mysql.com/doc/refman/8.4/en/connection-options.html)

Le SQL de profil s’exécute après la création du compte QA, pendant l’initialisation du volume neuf avant l’écoute TCP. Un volume existant conserve sa configuration. [Entrypoint MySQL 8.4](https://github.com/docker-library/mysql/blob/master/8.4/docker-entrypoint.sh)

Définir les aides ci-dessous pour exécuter les contrôles sans saisie et sans mot de passe dans les arguments ou l’environnement du client. Le fichier d’options et les sorties détaillées restent dans un répertoire temporaire privé supprimé en fin de contrôle. Le tiret en entrée sélectionne un mot de passe volontairement faux, distinct de ceux générés en hexadécimal. Le contrôle positif exige le compte exact, un résultat SQL, une session TLS et `require_secure_transport` actif. Chaque refus exige le code attendu et une sortie SQL vide. Lorsque le client coupe TLS avec le bon mot de passe, MySQL peut renvoyer `1045` au contrôle de `REQUIRE SSL` ou `REQUIRE X509` avant la garde globale `3159`. Les témoins positifs encadrent cet essai. [Fichiers d’options MySQL](https://dev.mysql.com/doc/refman/8.4/en/option-files.html) · [Variables serveur MySQL](https://dev.mysql.com/doc/refman/8.4/en/server-system-variables.html#sysvar_require_secure_transport) · [Ordre des contrôles MySQL 8.4.13](https://github.com/mysql/mysql-server/blob/mysql-8.4.13/sql/auth/sql_authentication.cc#L3955-L3993)

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

Lancer d’abord le profil mot de passe depuis le client natif. Vérifier son arrêt avant le mTLS. L’alias `127.0.0.1` atteint le même port mais ne correspond à aucun SAN du certificat.

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

Le dernier contrôle positif doit repasser après chaque série. Une déconnexion générique, un timeout ou un échec de disponibilité ne valident aucun refus attendu. Conserver le profil, les versions serveur et TLS et les catégories de résultat. Les volumes QA restent disponibles après l’arrêt. [Vérification de la session TLS](https://dev.mysql.com/doc/refman/8.4/en/using-encrypted-connections.html)
