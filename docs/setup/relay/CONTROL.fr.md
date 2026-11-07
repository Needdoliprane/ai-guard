# Propriétaire privé du contrôle — composant logiciel

Ce composant prolonge la [direction de transport approuvée](../../engineering/RELAY_RUNTIME_HANDOFF.md).
Il ne fournit ni transport runtime utilisable ni commande de lancement worker.
Le [helper existant](HELPER.fr.md) et le [client HTTP](README.fr.md) conservent
leurs qualifications distinctes. La [preuve du composant](../../engineering/evidence/2026-11-07/relay-control-owner/README.md)
sépare les vérifications exécutées des tests seulement présents en source.

Le gel `relay-owner66` passe 12 tests propriétaire, 13 canaris client existants
et 6 canaris de profils existants dans QA AL : 31 fonctions distinctes. Les
tests propriétaire utilisent sockets, HTTP et fichiers privés réels avec des
reçus HTTP déclarés ; les canaris client/profils exercent gateway et SQLite.
Cela ne rend pas le transport worker utilisable. La compilation toutes cibles
passe sur le même code dans QA AO. Clippy strict du gel66 reste NOT_RUN ; la
preuve conserve les sources et limites distinctes.

`src/relay/channel/control.rs` est privé. Aucun constructeur d'admission
production n'est exposé ; son contrôle d'admission production refuse tous les
échanges. Seuls les tests peuvent fournir une admission nommée synthétique.
Aucune frame ne choisit tenant, endpoint source, credential, origine, route HTTP
ou autorité runtime.

Le propriétaire emprunte un owner IO retenu, conserve le client workload dans le
contrôle et appelle seulement `send`, `inbox`, `ack` avec le profil fixe
`ControlIngress`. Hello/Ready, canal, epoch, réutilisation des correlations,
plafond de 1024 échanges et corrélation des réponses utilisent le protocole fermé
existant. Un seul échange est admis. Un second préfixe ou EOF pendant HTTP ferme
l'admission sans lire un second corps JSON. Un reçu confirme l'acceptation
serveur, jamais l'exécution du destinataire, une fin de worker ou une libération
de quota.

Avant le préfixe d'un échange, le contrôle réserve une unité logique exprimée en
3 Mio dans des compteurs tenant et global partagés. Cette même réservation reste
tenue pendant la contre-pression de sortie et les IO en attente. Le service doit
fournir les mêmes compteurs à tous les owners
concernés, avec les plafonds approuvés de 32 Mio par tenant/128 Mio global ; de
nouveaux compteurs par owner annuleraient la borne d'admission. Ces unités ne
mesurent pas la somme des buffers vivants : inbox décodée de 2 Mio, valeurs JSON
et frame encodée d'environ 2 Mio peuvent coexister. Les bornes DTO/frame/profondeur
restent distinctes ; mémoire totale et pic RSS sont NOT_RUN. Les dénominations
3/32/128 Mio ne sont ni caps RAM ni capacité mesurée. Aucune factory de budget
production n'est exposée ici.

## Records privés `relay_v1`

Le format réemploie `Directory`/`PrivateFile` dans un namespace métier du broker
dédié, hors bases, source, workspaces et exports. Aucune nouvelle base, aucune
réinterprétation des checkpoints d'exécution et aucun corps métier dans l'outbox
d'audit. Le parent doit déjà appartenir au contrôle de confiance. La création
exige un enfant `relay_v1` neuf ; la réouverture ne crée aucun élément manquant
et ne répare aucun namespace partiel.

| Record | Signification |
|---|---|
| `.owner.lock` | Format/version immuable, retenu avec flock exclusif. |
| `scope.json` | `relay_v1`, version 1 et `JournalScopeV1` existant exacts, canoniques. |
| `op-UUID/request.json` | Format/version, endpoint, credential ID et send ou ACK typé exact. |
| `op-UUID/dispatch.json` | Référence SHA-256 durable de la requête avant tout dispatch HTTP. |
| `op-UUID/receipt.json` | Premier reçu validé référant la même requête, durable avant succès local. |

Les fichiers privés restent immuables et retenus par FD ; chaque création
synchronise fichier et parent. Le namespace initial synchronise aussi sa chaîne
d'ancêtres. Entrée inconnue, record manquant, version non supportée, remplacement,
liens/modes interdits ou erreur d'écriture ferment l'owner. Rien n'est supprimé
ou écrasé pour réussir une reprise. Ces hashes ne sont pas des checkpoints
signés et ne détectent pas la restauration d'un ancien namespace complet entre
deux redémarrages.

La clé d'opération est l'UUID seul dans cette portée tenant. Réutilisation entre
actions, credential IDs, endpoints ou contenus divergents : conflit. Révision
du credential et epoch du canal ne composent pas la clé : une rotation du même
credential ID ne crée aucun nouvel espace d'idempotence serveur. Un autre
credential ID ne reprend pas la première opération. HTTP valide toujours binding
et permissions actuels sur chaque replay explicite ; un reçu historique ne vaut
jamais nouvelle autorisation de livraison.

Les deux helpers de préparation du client valident DTO, corps décodé et cible
HTTP finale fixe avant de créer le répertoire d'opération. Les secrets connus
sont aussi cherchés dans le record encodé complet et son JSON décodé. La preuve
retenue reste en mémoire du contrôle, hors records et frames. Cela ne constitue
pas une DLP exhaustive.

La primitive commune d'écriture durable vérifie tous les records, dont le lock
constant et le digest de dispatch généré. Les noms des répertoires d'opérations
sont aussi contrôlés avant création. Une métadonnée identique à un secret connu
est refusée, même si le CLI normal génère un autre format de preuve.

Au plus 1024 identités d'opérations et 64 Mio de stockage encodé réservé sont
admis par portée, dont 32 Kio par opération pour dispatch/reçu. Au plus une
mutation non confirmée par endpoint ; son replay exact conserve sa réservation
même au plafond. Reconnexion et rotation ne réinitialisent rien. Aucun pruning
ni replay automatique.

Sans dispatch, la requête est préparée. Avec dispatch sans reçu validé, elle
reste non résolue après réouverture. Un refus courant n'efface jamais une
ancienne issue inconnue. Ce segment conserve prudemment **tout** dispatch non
confirmé, y compris un premier refus HTTP certain ; la classification des refus
et une procédure opérateur explicite de rapprochement restent requises avant
disponibilité production. Un nouvel UUID, TTL, start ou grant ne contourne pas
cette réservation.

## IO bloquantes et propriété

Validation, parsing et synchronisation du journal de l'owner async s'exécutent
dans des segments `spawn_blocking`. Des permits tenant/global distincts et
partagés sont acquis sans file d'attente avant création de tâche. Chaque segment
queued/running possède permits, réservation mémoire, FDs/flock du journal et un
vrai `Arc<RoleLease>`. La configuration par défaut du pool n'est pas cette borne.

Le handle reste dans le mutex de l'`IoOwner` retenu pendant l'attente. Annuler
`serve` ou son observateur libère l'attente, pas ce handle ni ses ressources. Le
registre service doit conserver cet owner jusqu'à jonction explicite. Close,
shutdown de sortie et jonction partagent une échéance de fermeture de cinq
secondes. Une jonction inachevée rend `NeedsReconciliation` ; ni remplacement,
replay ni libération du rôle retenu n'en découlent. Aucun repli IO synchrone.
Une requête HTTP déjà dispatchée peut encore committer après fermeture locale.

## Intégration obligatoire avant disponibilité

Le service doit encore raccorder registre propriétaire et verrou de rôle
exclusif existants, conserver les budgets partagés, préparer les records durables
de canal/epoch, obtenir un permis relay-exec distinct et valider binding workload
et incarnation runtime courants avant chaque dispatch. Le point d'admission
fermé ne sera remplacé que par ce parcours concret, avec réservation d'audit
obligatoire. Aucun permis start consommé, frame worker ou callback public ne
fournit cette autorité.

Child runtime non TTY exact, drainage stderr, validation image/profil du helper,
révocation et fermeture de processus restent obligatoires. Docker, Podman,
Kubernetes, trois vrais workers, fournisseurs, confinement réseau et gates
production ne sont pas qualifiés par ce composant. STOP/enforcer reste
indépendant ; ce module ne crée ni observation terminale ni mesure d'usage.
