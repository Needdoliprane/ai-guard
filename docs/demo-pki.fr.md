# Une PKI locale pour les essais

[English](demo-pki.md) · [Français](demo-pki.fr.md) · [Configuration](setup/README.fr.md)

Créer une chaîne de test demande une seule commande depuis le dépôt :

```sh
cargo run --locked --bin ai-guard-demo-pki -- --out .agentsandbox/demo-pki
```

Prérequis : la toolchain Rust du dépôt et un exécutable OpenSSL/LibreSSL de confiance à `/usr/bin/openssl`. `--openssl /chemin/absolu/openssl` permet d'en sélectionner un autre explicitement. Aucun programme n'est recherché dans le `PATH`. La commande ne contacte aucun service de CA et n'installe rien dans les magasins de confiance.

Le dossier de sortie doit être **nouveau**, même si un ancien dossier est vide. Les parents sont contrôlés avec les règles du stockage privé ; les liens et traversées sont refusés. La sortie est privée (`0700`), ses fichiers sont `0600`. Une erreur conserve la sortie partielle : examiner le diagnostic puis choisir un autre dossier, sans effacer les clés existantes.

La configuration par défaut couvre `keycloak.ai-guard.test`, `llm.ai-guard.test`, `localhost`, `127.0.0.1` et `::1`. Pour remplacer les noms DNS et choisir la durée du certificat serveur :

```sh
cargo run --locked --bin ai-guard-demo-pki -- \
  --out .agentsandbox/demo-pki-autre \
  --dns keycloak.ai-guard.test --dns llm.ai-guard.test --days 14
```

Au maximum huit noms DNS ASCII, sans wildcard, et une durée serveur de 1 à 30 jours sont admis. Les IP loopback restent présentes. La racine dure 90 jours, l'intermédiaire 60 ; le certificat serveur dure 7 jours par défaut. Les clés RSA 2048 et les signatures SHA-256 servent exclusivement à cet environnement de test.

| Fichier | Destination |
|---|---|
| `root-ca.pem` | AC publique à approuver explicitement dans le client de test |
| `server-chain.pem` | Chaîne servie : certificat serveur puis intermédiaire |
| `server.key` | Clé privée du serveur de test concerné uniquement |
| `root-ca.key`, `intermediate-ca.key` | Clés privées des AC, à conserver dans ce dossier privé |
| `intermediate-ca.pem`, CSR et `pki.cnf` | Éléments locaux de génération et de diagnostic |
| `READY` | Marqueur final ; vérifier aussi que la commande s'est terminée avec succès |

Ne monter que la clé serveur et sa chaîne dans le service destinataire ; un conteneur d'un autre UID aura besoin d'une projection privée préparée avec les droits appropriés. Aucune clé d'AC ne va dans une image, un worker ou un rapport. Les clients ne reçoivent que la racine publique. Le dossier `.agentsandbox/` est exclu de Git et de la copie de travail des agents.

Cette commande ne change pas le DNS. Pour les noms locaux, une entrée loopback dans `/etc/hosts` doit correspondre au certificat et à l'issuer configuré, en préservant les entrées existantes. Le profil [Keycloak de découverte](sso.md#profil-keycloak-de-découverte) HTTP et les services existants ne passent pas automatiquement à HTTPS après génération : les points TLS et la confiance de chaque client se configurent et se vérifient séparément.

Examiner le certificat public et vérifier sa chaîne :

```sh
/usr/bin/openssl x509 -in .agentsandbox/demo-pki/server.pem -noout -subject -issuer -dates
/usr/bin/openssl verify -CAfile .agentsandbox/demo-pki/root-ca.pem -untrusted .agentsandbox/demo-pki/intermediate-ca.pem .agentsandbox/demo-pki/server.pem
```

Pour renouveler, générer dans un autre répertoire neuf, valider la chaîne puis projeter uniquement les fichiers utiles et suivre le rechargement explicite du service. Conserver les anciens éléments jusqu'à vérification de la transition ; protéger les clés d'AC indépendamment des volumes applicatifs.

Les instruments auxiliaires restent locaux et ignorés. Les résultats exécutés sont consignés dans [VALIDATION.md](engineering/VALIDATION.md). Ce générateur n'exploite pas une AC de production et ne fournit pas de renouvellement/révocation automatisés ; consulter le [guide entreprise](setup/enterprise/README.fr.md) et le [contrat d'infrastructure](engineering/INFRASTRUCTURE_PKI.md).
