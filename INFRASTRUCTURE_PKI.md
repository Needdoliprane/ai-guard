# Infrastructure, transport et PKI

Ce document applique principalement G01, G02, G05, G06, G09 et G11 de [PRODUCTION_READINESS.md](PRODUCTION_READINESS.md). Il distingue l'exploitation locale existante, les exigences de la cible cloud et les décisions d'infrastructure encore inconnues. **Il n'autorise aucun déploiement, aucune connexion NAS, aucune émission de certificat ni aucune rotation.**

Les informations sont **Confirmé**, **Supposé** ou **À vérifier**. Les résultats d'essais emploient séparément `PASS`, `FAIL`, `BLOCKED`, `NOT_RUN` et `NOT_APPLICABLE`. Aucune qualification TLS/mTLS, Kubernetes ou NAS nouvelle n'est fournie ici.

Références : [contrats cloud](CLOUD_TENANCY.md), [tests local/cloud](LOCAL_CLOUD_TESTS.md), [mesure d'usage](USAGE_METERING.md), [opérations locales](../operations.md), [API](../api.md) et [stratégie de tests](TEST_STRATEGY.md).

## 1. Profils d'exploitation

| Profil | Infrastructure nécessaire | Transport / identité | Applicabilité de la PKI |
|---|---|---|---|
| Local, un hôte de confiance | CLI Rust, Docker ou Podman, gateway/dashboard, volumes locaux ; navigateur loopback | Bearer viewer/runner distincts ; HTTP loopback et réseau de contrôle interne dans le déploiement courant | PKI locale `NOT_APPLICABLE` si aucun accès distant n'est exposé |
| Local, qualification multi-worker | Trois workers A1/A2/B1, contrôle tenant/grants et relais Rust implémentés, Compose et smoke réel ignoré | Credentials distincts liés au rôle/tenant/worker, jamais identité déduite de l'adresse réseau | Tests logiciels sans NAS ; moteurs réels `NOT_RUN`, aucun mTLS revendiqué |
| LAN explicitement demandé | Endpoint, DNS, firewall, terminaison TLS et politique d'origines à choisir | HTTPS, auth applicative maintenue ; mTLS pour les flux de service retenus | G11 applicable ; profil actuellement `BLOCKED` |
| Cloud multi-client | Contrôleur Rust de confiance, adaptateur Kubernetes, stockage et CNI qualifiés, workers exclusifs, quotas et ledger | HTTPS client/API ; mTLS pour les communications de service/worker, associé aux ACL tenant et grants | G11 obligatoire pour ce profil ; CA indépendante d'un NAS possible |

La version cloud réutilise le domaine et les contrats Rust autant que possible. Le broker local pilote Docker/Podman ; le contrôleur cloud utilise l'adaptateur Kubernetes avec une autorité minimale. Les workers ne reçoivent ni le secret runner de contrôle ni une identité d'administration du cluster. Kubernetes, haute disponibilité et PKI ne deviennent pas obligatoires pour exploiter le profil local sur un seul hôte.

L'architecture cloud doit traiter des clients qui ne se font pas confiance. Le registre actuel associe des credentials distincts au tenant et au rôle, et chaque base SQLite est liée à son tenant. Cela apporte un contrôle logique réel, mais ne fournit ni orchestration Kubernetes, ni PKI, ni facturation financière. L'exposition réseau et les droits doivent être qualifiés avant mise à disposition ; ouvrir les ports de la configuration locale ne constitue pas cette qualification.

## 2. État confirmé du dépôt

### Contrôle et stockage locaux

Les fichiers [Compose Docker](../../deploy/docker/compose.yaml) et [Podman](../../deploy/podman/compose.yaml) publient `127.0.0.1:8787` et `127.0.0.1:4000`. Le réseau de contrôle est interne. Seule la gateway monte le volume d'audit ; le dashboard reçoit seulement le secret viewer. Les services sont UID 1000, sans capacités et avec racine en lecture seule. Il s'agit de **configuration confirmée**, dont l'application par les moteurs reste `NOT_RUN` dans la baseline.

`--container-listen` permet l'écoute interne `0.0.0.0` attendue par Compose ; ce drapeau n'est pas une option d'exposition Internet sécurisée. Les serveurs de [main.rs](../../src/main.rs) utilisent une écoute TCP HTTP, sans options certificat/clé/mTLS. Le contrôle d'origine de [api.rs](../../src/api.rs) accepte les origines loopback prévues par l'usage local. Une origine DNS LAN/cloud exige une politique explicite et testée, pas la désactivation de ce contrôle.

La gateway utilise SQLite WAL et un volume local dans ce déploiement. La sauvegarde cohérente, la restauration, la migration et la rétention restent des gates G06. Ne pas déplacer la base active sur un partage NAS ni changer son filesystem sans qualifier les garanties de stockage et le comportement WAL. Une sauvegarde vers un stockage distinct est un parcours différent de l'hébergement de la base active.

### Secrets et maintenance

Les originaux `secrets/runner` et `secrets/viewer` restent `0600`. Les projections `mounted-secrets/*` sont `0444` sous un parent privé `0700` et seules ces projections sont montées en lecture seule. `init` valide ces emplacements et rafraîchit les projections atomiquement. Les projections sont exclues de la copie du workspace. Voir [operations.rs](../../src/operations.rs) et [operations.md](../operations.md).

`up` recrée gateway et dashboard pour remonter les nouveaux fichiers et recharger les clés. **Répéter `up` interrompt les connexions HTTP/SSE, même sans changement de secret.** Un job actif peut échouer à sa prochaine écriture d'audit. Pour une rotation locale : terminer les jobs, remplacer uniquement les sources privées en conservant leurs droits, exécuter le parcours `init`/`up` documenté, vérifier la disponibilité puis l'invalidité des anciennes clés, et reconnecter le navigateur. Les volumes d'audit et de travail ne sont pas supprimés.

Les jetons actuels n'expirent pas automatiquement. Une rotation de Bearer, un renouvellement de certificat serveur et une révocation de certificat worker sont trois opérations différentes ; chacune nécessite ses preuves et son effet documenté sur les connexions existantes.

### Flux sortants

Le runner refuse le HTTP distant pour son audit et désactive les redirections. Le client Jev utilise HTTPS, avec une exception HTTP loopback explicite destinée aux intégrations contrôlées. Le déploiement Jev optionnel ajoute un réseau sortant à la gateway ; il ne filtre pas les destinations. Les workers restent en `none` sauf demande explicite `observe` + `bridge`. Le mode bridge ne constitue pas un réseau de tenants autorisé par grants. Sources : [runner](../../src/runner.rs), [Jev](../../src/jev.rs), [override Jev](../../deploy/docker/compose.jev.example.yaml).

La [fixture à trois workers](../../deploy/local-cloud/compose.yaml) est un parcours distinct : chaque worker rejoint son propre réseau interne, la gateway sert de relais, aucun secret runner n'est monté dans un worker. `peer-send` et `peer-inbox` acceptent HTTPS ou loopback ; `--allow-test-http` autorise explicitement HTTP vers l'alias `gateway` ou une IP privée du test isolé. Cette option ne désactive jamais TLS et ne qualifie pas un transport cloud. Les grants actuels portent sur des messages JSON et un channel, pas sur un mesh TCP.

Les options actuelles ne fournissent pas de configuration explicite de CA privée ou de certificat client pour tous ces chemins. L'ajout d'une CA ou du mTLS doit être implémenté et testé dans le composant concerné, sans prétendre que modifier le magasin du navigateur modifie aussi celui du CLI ou des conteneurs.

## 3. Contrat réseau de la cible cloud

Chaque worker appartient à un seul tenant. La décision de propriété vient du contrôleur authentifié et persiste avec la session ; elle ne dépend pas d'un champ que le worker peut falsifier. Les namespaces/labels doivent refléter cette décision sans devenir l'unique contrôle d'autorisation.

Le contrat de communication est **default-deny** :

1. Sans grant, A1 ne joint ni A2 ni B1.
2. Un grant approuvé autorise A1→A2 pour une cible, une direction, un service/port/protocole et une durée bornés.
3. A1→B1 reste interdit. Le grant est borné au tenant ; une requête inter-tenant est refusée.
4. Le retrait ou l'expiration du grant rétablit le refus, avec une sémantique publiée pour les connexions déjà ouvertes et un délai de propagation mesuré.
5. L'API vérifie également l'identité et l'ACL. Une connexion réseau autorisée ou un certificat signé par la CA ne donne pas accès à toutes les ressources du tenant ou du cluster.

Les sorties nécessaires aux fournisseurs, DNS et services de contrôle sont inventoriées et autorisées selon un profil explicite. Une règle large de sortie, si un cas particulier l'exige, doit être visible et qualifiée ; elle ne reçoit pas le nom de filtrage fin. Tester IPv4, IPv6, adresses directes et voies de contournement de l'environnement retenu.

Pour Kubernetes, qualifier ensemble version du cluster, runtime, CNI, NetworkPolicy, RBAC, admission, volumes et identités. L'acceptation d'une ressource par l'API ne démontre pas son effet. Le scénario local Docker/Podman A1/A2/B1 valide ses propres mécanismes ; une preuve Kubernetes demande un cluster local avec CNI effectif, puis une qualification de la plateforme cloud choisie. Aucun cluster ou service cloud n'est à déployer pendant ce travail documentaire.

## 4. Fiche de transport et d'identité à renseigner

Avant l'activation LAN/cloud, le responsable de plateforme fixe les paramètres suivants. Toutes les valeurs non fournies restent **À vérifier** ; aucun FQDN, produit, port, durée ou mécanisme n'est déduit d'un exemple.

| Paramètre | Décision / preuve attendue |
|---|---|
| Profil et endpoints | URLs/FQDN/ports exacts, exposition publique ou privée, flux navigateur/API, contrôleur/worker et inter-worker autorisés |
| Terminaison TLS | Composant retenu, version, emplacement de la terminaison, chiffrement de chaque tronçon, configuration des origines et headers de proxy |
| Identités | Autorité d'authentification, mapping immuable tenant/worker/service, rôles et droits ; authenticité indépendante des champs payload |
| CA et chaîne | Racine/intermédiaires publics, empreintes vérifiées par canal approuvé, chaîne servie, séparation des environnements |
| SAN | Liste exacte des DNS/IP/identités autorisés ; pas de wildcard ajouté par commodité ; vérification du nom réellement contacté |
| Clés et algorithmes | Politique choisie, génération et garde des clés privées, permissions, stockage, éventuel matériel sécurisé ; aucune clé dans argv/logs/images |
| Validité | TTL de chaque type de certificat, seuil et fenêtre de renouvellement, synchronisation d'horloge, comportement en cas d'expiration |
| Révocation | Mécanisme retenu, disponibilité CRL/OCSP ou autre contrôle, propagation, fermeture/réautorisation des connexions existantes, traitement d'une autorité indisponible |
| mTLS | Vérification du serveur et du client, usages des certificats, correspondance identité→tenant/grant, comportement avec certificat absent/invalide/révoqué |
| Rotation | Responsable, fenêtre, cohabitation contrôlée des clés/CA si nécessaire, rollback, invalidation de l'ancien matériel et journal de décision |
| Exploitation | Monitoring d'expiration et d'échec, alertes actionnables, restauration et test régulier sans secrets dans les rapports |

Le TLS navigateur n'impose pas un certificat client à chaque utilisateur. Le mTLS de service identifie un pair ; les ACL du produit restent nécessaires pour sa ressource et son tenant. Les choix de CA, d'automatisation et de disponibilité n'imposent pas un NAS particulier.

## 5. NAS ou service de CA : aucune hypothèse implicite

Un NAS peut éventuellement fournir un stockage de sauvegarde ou un service de certificats. Ces rôles sont indépendants. **Aucun endpoint, modèle/version de NAS, API de CA, méthode d'authentification, chaîne de confiance, SAN, TTL ou mécanisme de révocation n'a été fourni ou validé.** Il est interdit d'en conclure qu'une API d'émission existe ou qu'une CA y est administrable.

| Information à demander avant toute connexion | Pourquoi elle est nécessaire |
|---|---|
| Propriétaire, produit, modèle et version du service | Identifier le périmètre autorisé et le contrat réellement disponible |
| URL/FQDN, port et API/version précis | Éviter discovery, scans ou requêtes sur un endpoint deviné |
| Fonction demandée | Distinguer lecture de documentation/état, émission d'un certificat serveur/worker, révocation, rotation et stockage de sauvegarde |
| Méthode d'authentification et rôle minimal | Définir la transmission sûre des credentials et limiter l'autorité |
| CA/chaîne publiques et méthode de vérification | Établir la confiance sans accepter aveuglément le premier certificat présenté |
| SAN, usage, TTL et renouvellement | Préparer une demande de certificat concrète et reviewable |
| Révocation et disponibilité | Définir le comportement en cas de clé compromise ou d'autorité indisponible |
| Fenêtre d'intervention et rollback | Maîtriser les effets sur services et connexions actifs |

### Mandat requis pour Codex

Codex prépare d'abord la fiche, les changements envisagés et les vérifications sans agir sur le service. **Avant une connexion au NAS, une émission ou une rotation, il demande une autorisation précise** couvrant l'endpoint, l'identité utilisée, les opérations, les SAN/TTL et la fenêtre. Un mandat antérieur couvrant exactement l'action reste valable ; ce document n'en fournit aucun. Une autorisation de lecture ne vaut pas autorisation d'émission, de modification de confiance ou de rotation.

Aucune découverte réseau, connexion SSH, installation de CA dans un magasin de confiance, émission, révocation ou rotation ne doit être déclenchée sur la seule base de cette documentation. La demande à l'utilisateur explique les paramètres manquants et l'effet concret attendu ; elle ne demande pas une approbation vague d'« intégrer la PKI ».

### Invariants de gestion des clés

- **Ne jamais exporter la clé privée d'une CA** vers ai-guard, un worker, un rapport, un chat ou une archive de livraison. Le service de CA signe une demande ; seule la chaîne publique nécessaire à la vérification est distribuée.
- Générer et conserver les clés privées de certificats applicatifs dans le périmètre prévu ; transmettre uniquement le CSR lorsque c'est le contrat retenu. Ne pas les incorporer aux images ni les recopier pour faciliter un diagnostic.
- Ne jamais désactiver la validation des certificats ou des noms : pas de `trust-all`, d'acceptation universelle, d'option insecure ni de contournement du SAN. Un échec de confiance donne `BLOCKED` ou `FAIL` selon le test, pas une autorisation de poursuivre sans vérification.
- Ne pas considérer une signature CA valide comme une autorisation tenant. Vérifier identité, rôle, destination et grant séparément.
- Conserver comme preuves des empreintes, numéros de série, dates, paramètres non secrets et décisions. Expurger tokens, clés, credentials et données client.

## 6. Qualification TLS/mTLS et révocation

Ces tests sont **à implémenter et `NOT_RUN`** après choix de l'infrastructure ; ils n'existent pas comme commandes Cargo dans le dépôt courant.

| Essai | Résultat requis | Gates |
|---|---|---|
| Chaîne et nom valides | Connexion acceptée pour l'identité et la route autorisées uniquement | G05, G11 |
| CA inconnue, mauvais SAN, certificat expiré ou pas encore valide | Refus sans retry insecure ni envoi du secret applicatif à une destination non approuvée | G05, G11 |
| Certificat client absent, falsifié ou d'un autre tenant | Refus mTLS ou ACL, aucun événement/volume/usage de l'autre client exposé | G02, G04, G08, G11 |
| Retrait d'un grant | Refus A1→A2 après délai documenté, y compris traitement explicite des flux déjà ouverts ; A1→B1 toujours refusé | G05, G09, G11 |
| Révocation et service de vérification indisponible | Politique publiée appliquée ; aucune prétention de révocation active si elle n'est pas vérifiée par les clients | G05, G11 |
| Renouvellement/rotation | Nouveau matériel actif, ancien refusé au terme de la transition, audit préservé, interruption et rollback mesurés | G06, G09, G11 |
| Proxy et SSE | Auth, origine et CSP maintenues, flux et curseur fonctionnels, export interrompu signalé, aucun token dans URL/logs | G08, G11 |
| Redémarrage contrôleur/worker | Identité et tenant restent liés ; grants expirés non réactivés, ledger non doublé | G06, G09, G11 |

Pour le profil local loopback sans PKI, ces essais de certificats sont `NOT_APPLICABLE` avec cette justification ; les tests d'authentification et de confidentialité locales restent obligatoires. Pour le cloud, l'absence d'infrastructure ou d'implémentation les rend `BLOCKED`, pas hors périmètre.

## 7. Exploitation, sauvegarde et capacité

Une production locale peut être fiable avec arrêt planifié et sauvegarde à froid cohérente ; elle ne nécessite pas de HA. Avant toute procédure, définir ce qui est sauvegardé : audit, métadonnées de sessions, volumes utiles, identité des tenants et ledger pour le profil cloud, configurations et version de schéma. Les secrets suivent un circuit de protection distinct ; ils ne sont pas joints au dossier de test. Une copie de fichier SQLite actif n'est pas une procédure de restauration qualifiée.

Le profil cloud doit publier sa capacité réellement testée, ses quotas, son domaine de panne et ses engagements de reprise. Les 100 workers/client sont un objectif à mesurer, pas une taille déduite de trois workers locaux. Les réglages de ressources, alertes de disque et RPO/RTO proposés dans [TEST_STRATEGY.md](TEST_STRATEGY.md) restent des hypothèses jusqu'aux essais.

L'utilisation éventuelle d'un NAS ne dispense d'aucun exercice de restauration et ne rend pas la release dépendante de cet équipement si une autre solution répond aux gates. Le dossier de release doit pouvoir expliquer les flux, les identités, les preuves et les opérations de maintenance sans contenir de secret.
