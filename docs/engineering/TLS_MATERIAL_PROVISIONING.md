# Import contrôlé des matériaux TLS du modèle

Décision de conception du1er novembre2026, dans le contrôle Rust existant.
**À implémenter et qualifier.** Ce contrat ne fournit pas encore de commande.
Il traite le refus réel des projections Docker Desktop, sans élargir les
filesystems autorisés par `control_fs` et sans modifier PrivateFile ou SQLite.

## Frontière et choix

Le contrôle hôte valide trois sources privées puis transmet leurs snapshots sur
stdin à un préparateur ponctuel du même binaire/image de contrôle. Celui-ci
crée des fichiers privés dans deux volumes locaux neufs, sur un filesystem
effectivement accepté par le lecteur existant. Aucun bind TLS hôte chez les
consommateurs. Aucun nouveau service, moteur dans un conteneur, réseau,
privilège, autorité métier ou stockage d'audit ; c'est une préparation explicite
de configuration dans l'architecture autorisée. Une exigence de root runtime,
socket monté, daemon supplémentaire ou clé privée d'AC sortirait de ce contrat.

La variante bind reste disponible sur les filesystems réellement supportés.
Le choix volume est explicite ; aucune réparation de permissions, migration
d'une installation, rotation, adoption de volume existant ou fallback caché.

## Interface opérateur à livrer

`agentsandbox model-tls provision` reçoit les références `--ca-file`,
`--cert-file`, `--key-file`, `--server-name`, une image de contrôle immuable
`--image sha256:…`, et un `--output-dir` exclusivement neuf. Le runtime,
son programme/endpoint et l'accord rootful suivent les options de contrôle
existantes ; aucun moteur n'est résolu depuis un PATH non approuvé.

Le résultat est un manifeste privé immuable et un fichier `volumes.env` de
sélection publique : génération, noms des deux volumes, image et nom serveur.
Les exemples Compose concernés consomment réellement ces mêmes valeurs.
La commande ne lance ni modèle, gateway, worker ou requête de décision.

## Sources, paquet et réception

- Sources euid/0600/nlink1 et parents privés vérifiés par FD sur filesystem/ACL
  supportés ; pas de symlink, FIFO, réouverture libre ni changement des sources.
  Bornes avant lecture : CA65536, chaîne65536, clé16384octets. Revalidation
  avant/après transfert ; aucune clé dans argv/env/Debug/log ou capture.
- Valider PEM T2, paire chaîne/clé, chaîne de confiance, nom et dates du leaf
  offline avec le provider rustls existant. Leaf CA refusé. Refuser également
  une clé leaf dont le SPKI correspond à un certificat d'AC/intermédiaire fourni.
  Comparer les SPKI via le même `rustls::server::ParsedCertificate`, sans
  mélanger les encodages de SigningKey et de TrustAnchor.
  Ce contrôle ne prétend pas connaître les usages externes d'une clé absents
  du paquet ni fournir révocation OCSP/CRL.
- DTO V1 fermé sur stdin : version, génération UUID, nom serveur et exactement
  trois PEM. Wire≤512KiB avant parsing, limites décodées ci-dessus, doublons,
  champs inconnus, document incomplet et données après le document refusés.
  Aucun tar, unpack, chemin ou mode choisi par le paquet. Pas de Debug dérivé.
- Le sous-programme interne caché reçoit uniquement ce stdin et utilise deux
  destinations constantes : `/data` pour CA, `/run/secrets` pour identité.
  Il revalide le paquet entier avant toute écriture, puis les deux racines
  euid/0700/filesystem/ACL et leur vacuité. Aucun chmod/chown de rattrapage.
- Noms constants : `root-ca.pem` dans CA ; `server-chain.pem` et `server.key`
  dans identité. Création exclusive FD/0600/nlink1, sync fichiers ET parents,
  relecture par le reader TLS strict et nouvelle validation crypto avant ACK.

## Runtime, propriété et activation

Le runtime approuvé crée deux noms UUID neufs avec génération/rôle contrôlés.
Contrôler absence avant création puis driver local, options vides, labels et
identité réellement obtenus ; `volume create` seul ne prouve pas l'exclusivité.
L'administrateur du moteur et le même compte de contrôle restent de confiance.

Préparateur : UID1000, network none, rootfs RO, caps ALL retirées,
no-new-privileges, CPU1/RAM192MiB/PID64, aucun bind/socket/credential de contrôle.
Image immuable inspectée avant usage, jamais de pull silencieux. Les deux
volumes neufs sont les seules données montées. Le binaire hôte possède stdin,
stdout/stderr bornés et le processus ; budget global60s, import/ACK15s.
Ce sont des budgets d'orchestration/IO annulables : ils ne préemptent pas un
syscall filesystem bloqué et ne garantissent pas la réponse du moteur.
Le transport possède ses collectors/writer : pas de réemploi inchangé de
`execute_created`, pas de JoinHandle seulement droppé après timeout. Joindre
ou annuler puis joindre les opérations locales à toute sortie ; stdin interne
annulable, sans thread de lecture bloquant qui empêcherait sa terminaison.
Échec/timeout : arrêter seulement ce préparateur neuf, conserver ses volumes
et un diagnostic fermé. Arrêt distant non confirmé reste explicitement inconnu,
avec identifiants conservés ; ce n'est ni un cleanup réussi ni une activation.
Avant publication, aucun manifeste utilisable n'est exposé. Tout stderr
arbitraire et tout paquet/clé sont exclus des erreurs et preuves publiques.

ACK fermé≤4KiB lié à génération, nom serveur et empreintes des certificats
publics. L'hôte exige exit0, compare l'ACK et revalide sources/volumes avant
publication durable du couple manifeste/`volumes.env` : préparer et synchroniser
les deux fichiers dans un staging privé, puis publier exclusivement le répertoire
final neuf en une seule transition de namespace, sans écrasement. Un crash entre
deux fichiers ne doit pas exposer de sélecteur final. Sync du parent avant ACK ;
échec après rename mais avant sync = publication/durabilité inconnue, conservée
et interdite d'activation par le parcours normal, jamais un faux rollback réussi.
Aucun digest de clé privée dans le reçu. Pas d'atomicité multi-volume prétendue :
un crash avant publication conserve l'état incomplet, sans activation ni réécriture.

Gateway : CA seule RO ; relais TLS : identité RO et CA RO ; modèle et workers :
aucun de ces volumes. Les credentials et le volume d'audit du contrôle restent
séparés. Nom serveur cohérent entre validation, alias réseau, URL gateway et
healthcheck ; aucun paramètre silencieusement ignoré. Arrêt, reprise et retour
au manifeste précédent sont explicites ; renouvellement/rétention exigent leur
propre procédure, aucune suppression automatique de volume ou de certificat.

## Acceptation

Régressions ciblées sur vrais fichiers/crypto : permissions/ACL/liens/tailles et
substitution, forme du paquet et PEM, clé non correspondante/leaf CA/clé d'AC,
destination non privée/non vide, ACK substitué et absence d'activation partielle.
Luna exécute les tests pertinents, sans campagne fournisseur ou E2E systématique.
Revue indépendante du handoff, durabilité, erreurs et cleanup avant intégration.
Guides EN/FR, exemples env et variantes Compose évoluent dans le même lot.
Vrai import Docker Desktop puis santé HTTPS sans génération pour cette portée ;
Podman séparé. G09/G11, rotation/révocation, inférence intégrée et qualité du
modèle restent ouverts jusqu'à leurs propres preuves.
