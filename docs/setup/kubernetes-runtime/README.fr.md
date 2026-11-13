# Runtime de session Kubernetes V2 : état, mécanismes de reprise et plan Docker Desktop

[English](README.md) · [Français](README.fr.md) · [Guide Kubernetes](../kubernetes/README.fr.md)

**Aucune session Kubernetes ne peut être lancée avec ce produit.** Il n'existe
ni option `--runtime kubernetes`, ni commande CLI de session, ni activation de
production. Ce guide décrit le code du runtime de session V2 présent dans les
sources Rust, ce qu'un opérateur peut faire aujourd'hui, et le plan qui le
qualifiera plus tard sur le cluster Kubernetes à un nœud de Docker Desktop. Ce
plan **n’a pas été exécuté sur un cluster réel**. État du 13 novembre : les 144 parents logiciels Kubernetes sélectionnés passent dans la composition QA Linux ARM64 ; les résultats et limites figurent dans les [preuves courantes](../../engineering/evidence/2026-11-13/claude-composition/README.fr.md). Le [registre initial](../../engineering/evidence/2026-11-12/kubernetes-v2-runtime/README.md) conserve son état de livraison daté.
Le contrat de référence est le [contrat d'admission Kubernetes](../../engineering/KUBERNETES_ADMISSION.md).

## 1. État par couche

| Couche | Ce qui existe | Qui peut l'appeler aujourd'hui | Vérifié |
|---|---|---|---|
| A. Entrée de bibliothèque avec données réelles | `KubernetesRuntime::inspect_session_workspace_claim(name)` : nom vérifié comme label DNS avant toute requête, puis GET des deux ancres de namespace, du PVC et à nouveau des ancres. Un nom invalide est signalé comme `InvalidAuthority` avant toute requête, un claim qui ne remplit pas les exigences de la section 3 comme `InvalidResponse`, et un PVC absent comme `ApiRejected(404)`. | Du code Rust qui détient un `KubernetesRuntime`. **Aucun binaire du produit ne l'appelle** ; une entrée CLI en lecture seule, qui associerait les deux premières erreurs à des messages dédiés, n'est qu'un patch proposé. | Logiciel ciblé PASS ; cluster NOT_RUN |
| B. Mécanismes purs (internes à la crate) | Rendu exact de la requête de Pod V2, projection de la réponse admise, parseur exact d'observation, enregistrements du journal. | Code de la crate seulement. | Logiciel ciblé PASS ; cluster NOT_RUN |
| C. Cycle de vie avant effet (interne à la crate) | Chaîne de rétention du journal, réouverture en observation seule, abandon avant tout effet, reprise par un successeur, rejeu du reçu, rapport de réconciliation en observation seule. | **Personne en production.** Le Store du gateway émet les confirmations alors que le broker garde le journal ; les routes broker qui les relieraient n'existent pas encore (demande Root R2). | Logiciel ciblé PASS ; cluster NOT_RUN |
| D. Mécanismes d'effet (fermés) | CREATE retenu des octets exacts, PATCH de levée du gate avec opérations JSON-Patch `test`, DELETE avec précondition d'UID. | Une garde scellée dont la seule implémentation existe dans les tests, liée à un listener loopback possédé. Aucun chemin du produit n'atteint un effet. | Logiciel ciblé PASS ; cluster NOT_RUN |
| E. Raccords et qualification manquants | Demandes typées R1–R11 au propriétaire du contrôle ; ce plan Docker Desktop. | — | non commencé |

L'attachement à un Pod, le mode d'attente du worker, les permis d'effet, le
handoff, le démarrage, l'observation terminale et le règlement du quota après
effet **ne sont pas implémentés**. Rien dans ce guide n'est qualifié, et il ne
ferme aucun gate de production.

Les résultats logiciels concernent uniquement les sélections du [relevé typé](../../engineering/evidence/2026-11-13/claude-composition/TESTS.json), pas toutes les combinaisons de chaque couche. Global RC99 : un sous-cas release autre UID reste NOT_RUN. Aucun effet produit S5 ouvert. S2b attach reste actif chez Claude et non intégré ; les questions Q1/Q6 héritées ci-dessous restent historiques, distinctes du nouveau dispatch.

## 2. Ce qu'un opérateur peut faire aujourd'hui

- **Fermer une réservation qui n'a émis aucun effet** avec la route Runner
  `control-recovery` existante du contrôle central. Elle est décrite dans
  [Configurer les politiques de réservation de session V2](../kubernetes/README.fr.md#configurer-les-politiques-de-réservation-de-session-v2)
  et dans le [contrat d'API](../../api.md#récupération-contrôle-des-réservations-kubernetes-v2--kubernetes-session-control-recovery-v2).
  Ce travail ne modifie pas cette route.
- **Inspecter l'autorité Kubernetes** avec la commande existante en lecture
  seule `agentsandbox kubernetes doctor` (voir le [guide Kubernetes](../kubernetes/README.fr.md#lancer-le-diagnostic-cli-en-lecture-seule)).
- L'abandon, la reprise par un successeur, l'inspection du claim de workspace
  et le rapport de réconciliation sont des **mécanismes de bibliothèque en
  attente de R2**. Ce ne sont pas des procédures opérateur, et aucune commande
  n'est documentée pour eux.

Ne pas modifier, renommer ou supprimer les fichiers d'un répertoire de journal
V2 pour « débloquer » une session. Le journal est une preuve ; un refus signale
qu'une réconciliation est nécessaire.

## 3. Concepts

- **Réservation.** Le Store central du tenant tient une réservation V2 dans
  l'état `Reserved` (quota retenu, fence d'effet `NeverIssued`) ou
  `ClosedBeforeEffect` (quota libéré). Il n'existe pas encore d'état central
  pour un effet émis (demande Root R1).
- **Journal de préparation.** Un répertoire privé du broker qui retient, à
  partir d'une vraie confirmation centrale, l'intention exacte de la réservation
  et sa position d'audit. Une réouverture exige une nouvelle vraie confirmation
  du même état réservé.
- **Claim de workspace par tentative.** Le workspace est un
  PersistentVolumeClaim provisionné par l'opérateur et nommé
  `ai-guard-ws-<id de tentative, 32 chiffres hexadécimaux>`. Il doit être
  `Bound`, utiliser `ReadWriteOncePod`, être un volume de système de fichiers,
  n'avoir aucune source de données et ne pas être en cours de suppression. Un
  claim est à usage unique : il appartient à une tentative, et la réutilisation
  de son UID par une autre réservation sera refusée dès que le contrat central
  existera (R1). La première observation retenue fait foi : une lecture
  ultérieure du même PVC avec un nouveau `resourceVersion` n'est pas retenue à
  nouveau. La requête est rendue à partir du claim retenu, et proposer une
  lecture plus récente du même PVC est un `Conflict`.
- **Requête exacte.** La requête de Pod est rendue de façon déterministe à
  partir de l'intention confirmée, de l'empreinte de préparation et du claim
  retenu. Sa commande est le mode d'attente du binaire existant
  `ai-guard-provider` (`kubernetes-session-wait-v2`, orthographe en attente de
  la décision Root Q6), jamais une invocation métier. Elle n'a ni
  environnement, ni Secret, ni jeton, ni chemin hôte, ni jeton de
  service-account, et porte un scheduling gate.
- **Chaîne d'effets.** CREATE (retenu derrière le gate), levée du gate,
  attachement, démarrage et arrêt exigent chacun une preuve centrale distincte
  qui n'existe pas encore. « Autorisé à créer un Pod » ne signifie jamais
  « autorisé à le démarrer ».

## 4. Inventaire du journal r2

| Fichier | Borne | Contenu | Exige |
|---|---|---|---|
| `.session-v2.lock` | 0 octet | verrou d'écriture (`flock`) | — |
| `prepared-v2.json` | 96 Kio | intention confirmée, engagements, position d'audit centrale | verrou |
| `workspace-v2.json` | 8 Kio | le claim retenu pour cette tentative | préparation |
| `pod-v2.json` | 32 Kio | octets exacts de la requête ; valides seulement si un nouveau rendu les reproduit | claim de workspace |
| `abandoned-v2.json` | 4 Kio | la fermeture avant effet retenue (`preparation_abandoned`, révision 1) | préparation |

Règles :
- Chaque fichier est créé de façon exclusive puis synchronisé ; le produit ne
  réécrit, ne renomme et ne supprime jamais un fichier. Un nom inconnu, y
  compris un futur checkpoint d'effet, rend le journal `Corrupt`, et il est
  préservé.
- Un propriétaire vivant d'un journal préparé rend tout autre ouvreur `Busy`,
  jamais `Corrupt`. Un verrou sans `prepared-v2.json` est jugé sans prendre son
  verrou : pendant la création par un autre processus, c'est un `Corrupt`
  transitoire ; laissé seul, il reste `Corrupt` et n'est jamais réparé.
- Une lecture en observation seule ne crée aucun fichier et libère le verrou
  avant toute requête réseau. Un répertoire vide n'est pas un journal. Elle
  tient le verrou pendant qu'elle copie les fichiers : une réouverture
  concurrente par le propriétaire peut donc être `Busy` de façon transitoire ;
  réessayer.
- Chaque enregistrement est vérifié par rapport à ses prédécesseurs et au
  rendu : un enregistrement d'une autre tentative ou d'une autre préparation,
  ou une requête que le nouveau rendu ne reproduit pas, est `Corrupt` et
  préservé. Le journal ne peut pas authentifier un enregistrement réécrit par
  le même utilisateur du système d'exploitation ; par exemple, l'UID du claim
  dans `workspace-v2.json` n'est pas porté par les octets de la requête, qui
  ne lient le claim que par son nom.
- Après `abandoned-v2.json`, aucun claim ni aucune requête n'est retenu. Une
  fermeture retenue est une barrière locale, pas une fermeture centrale : le
  quota reste retenu jusqu'à ce que le Store valide la fermeture.
- Un binaire plus ancien refuse un journal qui contient les nouveaux noms
  (`Corrupt`). Il n'y a ni migration ni réécriture d'un journal.

## 5. Abandon avant tout effet et reprise par un successeur (mécanismes de bibliothèque)

L'abandon retient d'abord la fermeture localement avec un identifiant
d'opération stable, revalide ensuite tout le journal, puis appelle l'API
existante du Store. L'incarnation fournie par l'appelant sélectionne seulement
l'autorisation existante à essayer ; le Store revérifie le broker authentifié,
le tenant, le namespace et l'historique.

Pour un successeur, le Store vérifie seulement que la politique de reprise
qu'on lui fournit approuve le broker authentifié, et enregistre l'empreinte de
cette politique. **La politique de reprise est une donnée de confiance :** le
Store ne la compare à aucune configuration. Un appelant de production (R2) doit
la prendre dans le `session_recovery_policy` du tenant de la configuration du
gateway, jamais dans une requête du broker, un fichier de journal ou un worker.

| Résultat | Signification | Effet central |
|---|---|---|
| `Closed` | Cet appel a validé la fermeture avant effet. | `ClosedBeforeEffect` révision 2 ; le quota est libéré par le compare-and-set validé. |
| `Replayed` | La même fermeture était déjà validée ; le reçu historique est renvoyé (même incarnation, ou consultation du reçu par le successeur). | aucun, pas de nouvel événement |
| `SuccessorRequiresRecoveryPolicy` | Le journal appartient à une autre incarnation et aucune politique de reprise du tenant n'a été fournie. | aucun appel au Store ; rien n'est retenu |
| `NoRetainedAbandonment` | La vue en observation seule ne contient aucune fermeture retenue. Un successeur ne peut pas en retenir aujourd'hui (R11) ; utiliser la route Runner. | aucun appel au Store |
| `ClosedCentrallyByAnotherOperation` | Même incarnation : une observation montre la réservation fermée par une autre opération, dont l'identifiant est rapporté. | déjà fermée par cette opération |
| `SuccessorLookupNotFound` | Successeur : le Store ne connaît ni la réservation ni l'opération retenue. Un successeur ne peut pas observer une réservation fermée ; ce résultat reste donc ambigu. | aucun |

Un nom de checkpoint d'effet ou un enregistrement remplacé est refusé avec
`Corrupt` avant tout appel au Store. Un autre broker du même Store est refusé
par le Store (`PermissionDenied`). Une nouvelle tentative après une réponse
perdue est sûre : l'identifiant d'opération est retenu. Un délai dépassé, un
Pod absent ou une réponse manquante ne libère jamais le quota.

## 6. Rapport de réconciliation en observation seule (mécanisme de bibliothèque)

Le rapport lit un journal en observation seule, refuse une autre autorité
Kubernetes avant toute requête (`InvalidContext`), puis inspecte les noms de
session d'une requête retenue avec des requêtes GET seulement (ancres, Pod,
PVC, ancres). Il ne fait aucun appel au Store, aucune mutation, aucune
adoption et aucun changement de quota. Un journal sans requête retenue
n'émet aucune requête.

| Interprétation | Signification |
|---|---|
| `NoRequestRetained` | Aucune requête exacte ni aucune fermeture n'est retenue ; les noms de session n'ont donc **pas été inspectés** : la présence d'un objet à ces noms est inconnue. |
| `NoEffectIssuedLocally` | La requête exacte est retenue et ses noms ont été inspectés : rien au nom du Pod, et l'UID du claim retenu, sans suppression en cours, au nom du claim. Ce répertoire ne contient aucun checkpoint d'effet. Un autre répertoire ou processus pourrait exister pour la même réservation ; la fence centrale fait foi. |
| `ObjectPresentAtSessionName` | Un objet existe sous le nom de Pod dérivé. Son propriétaire est inconnu ; rien n'est adopté. |
| `WorkspaceClaimChanged` | Le PVC au nom du claim est absent, porte un autre UID que le claim retenu ou est en cours de suppression. |
| `AbandonmentRetained` | Une fermeture est retenue localement ; reprendre l'abandon. Le rapport n'observe pas l'état central. |

Le champ `names` du rapport n'est présent que lorsque les noms ont été
inspectés ; sinon il est absent du rapport sérialisé (et non `null`).

## 7. Modèle de sécurité

- **Ce qui n'est pas une identité :** un label, un nom de Pod, un écho de nonce,
  un enregistrement relu, un booléen ou un UID déclaré. Une observation de Pod
  lie l'UID exact, l'empreinte de spec et l'identité du conteneur ; un autre UID
  sous le même nom est une autre instance.
- **Attachement (proposition seulement, non implémentée) :** un attachement
  limité au préambule, encadré par deux observations exactes, qui n'envoie que
  le Hello non secret avant la seconde vérification. Ce n'est pas une
  précondition atomique ; il repose sur des hypothèses énoncées (UID jamais
  réutilisé, unicité des noms, serveur d'API et kubelet honnêtes, GET le plus
  récent). Chaque attachement terminerait le worker. Il attend la décision
  Root Q1.
- **Les logs du nœud conservent la sortie du worker**, y compris les
  métadonnées du préambule. Aucun secret ne doit transiter du worker vers le
  broker.
- **Stockage :** un claim `ReadWriteOncePod` par tentative ; aucun chemin hôte
  dans la spec du Pod. Sur Docker Desktop, le provisionneur par défaut devrait
  adosser chaque volume à un répertoire du système de fichiers de l'hôte du
  nœud (à vérifier, EXT-10b) : les données du workspace se trouvent alors sur
  l'hôte. `ReadWriteOncePod` et la capacité observée ne sont pas une limite
  appliquée en octets ou en inodes.
- **Réseau :** le profil V2 ne déclare aucun réseau ; `dnsPolicy: None` avec
  `127.0.0.1` n'est pas une application de politique. Ce travail ne crée ni ne
  vérifie aucune NetworkPolicy : sans politique de refus par défaut appliquée
  en entrée et en sortie dans le Namespace du tenant, un Pod libéré peut
  joindre n'importe quelle adresse, y compris les services du cluster, le
  nœud, les autres tenants et les endpoints de métadonnées. L'application reste
  non démontrée (G05/G07/G11), et l'étape 5 du plan reste `BLOCKED` tant que
  la sonde observée d'application par le CNI de l'étape 4a du plan n'a pas
  réussi.
- **Credentials :** le broker garde les credentials Kubernetes ; le Pod n'a ni
  montage automatique de jeton, ni environnement, ni Secret, ni secret en argv.
- **Permissions futures du broker** (étape 1 du plan ci-dessous, non
  provisionnées par ce travail) : get sur les deux Namespaces, get sur les PersistentVolumeClaims,
  get/create/patch/delete sur les Pods du Namespace du tenant, et la permission
  d'attachement dont le verbe reste à confirmer (EXT-8). Ni exec, ni logs, ni
  Secrets. RBAC ne peut pas restreindre create par nom d'objet ou par spec.
- **Tenancy :** le tenant provient uniquement de l'identité authentifiée.
  Refuser au chargement de la configuration deux tenants qui partagent une
  autorité ou un namespace (R10) est un prérequis de tout attachement.

## 8. Plan futur de qualification Docker Desktop à un nœud (non exécuté)

Rien ci-dessous n'a été exécuté. **Chaque étape qui modifie le cluster exige
une autorisation explicite du mainteneur**, que ce travail n'accorde pas.
Chaque étape consigne son oracle et ses preuves, et s'arrête au premier oracle
discriminant, refus ou plafond.

### 8.1 Prérequis à consigner

- OS de l'hôte, version et architecture (par exemple macOS ARM64) ; version de
  Docker Desktop avec Kubernetes activé.
- `kubectl config current-context` doit afficher `docker-desktop` ; consigner
  le client `kubectl version` et le garder dans la
  [politique officielle d'écart de versions](https://kubernetes.io/releases/version-skew-policy/) (EXT-13).
- `gitVersion` du serveur selon `agentsandbox kubernetes doctor`. Le profil
  d'inspection locale épingle Pod Security `v1.36`
  ([profil](../../../deploy/kubernetes/local-inspection.json)) ; toute autre
  version du serveur exige d'abord une revue des épinglages, et l'activation
  reste `BLOCKED`.
- Le plugin CNI et sa version, et s'il applique les NetworkPolicy : des faits
  à vérifier auprès des sources officielles, jamais supposés. Une application
  documentée n'est pas une observation ; l'étape 4a du plan l'observe. Sans
  application, les étapes 5 à 12 restent `BLOCKED`.
- Une image worker construite localement avec les patches proposés 0001 (mode
  d'attente) et 0002 (propriété du provider) appliqués, référencée par digest et
  présente dans le magasin d'images du nœud (`imagePullPolicy: Never`).
- `agentsandbox` construit uniquement dans l'environnement de QA isolé admis.

### 8.2 Prérequis de stockage

- Une StorageClass dont le provisionneur prend en charge `ReadWriteOncePod`. La
  [documentation des modes d'accès](https://kubernetes.io/docs/concepts/storage/persistent-volumes/#access-modes)
  de Kubernetes devrait le restreindre aux volumes CSI (EXT-10a, à vérifier pour
  v1.36). La classe par défaut `hostpath` de Docker Desktop devrait utiliser un
  provisionneur non CSI (EXT-10b, à vérifier) ; l'étape 3 est donc
  probablement `BLOCKED` avec la classe par défaut.
- [`volumeBindingMode`](https://kubernetes.io/docs/concepts/storage/storage-classes/#volume-binding-mode)
  doit valoir `Immediate`. Avec `WaitForFirstConsumer`, un claim n'est jamais
  `Bound` avant le rendu de la requête ; le plan consigne alors `BLOCKED`. Lier
  le claim après CREATE changerait l'engagement de la requête et relève de la
  question Root Q9.

### 8.3 Préparation (mutations de l'opérateur, autorisation requise) et vérifications en lecture seule du produit

Les étapes du plan sont numérotées de 0 à 12, plus l'étape 4a, qui doit
réussir avant l'étape 5 ; elles sont sans rapport avec les décisions Q1–Q9 de
la section 10. Les étapes 0 et 2 sont en lecture seule ; les étapes 1 et 3
modifient le cluster, et chacune exige une autorisation explicite du
mainteneur.

| Étape | Action | Oracle et preuves | Arrêt |
|---|---|---|---|
| 0 | Consigner l'environnement de 8.1. | Versions et contexte conservés. | Contexte autre que `docker-desktop`. |
| 1 | (Exige une autorisation explicite du mainteneur, non accordée par ce travail.) Provisionner le Namespace du tenant avec Pod Security `restricted` `v1.36`, une NetworkPolicy de refus par défaut en entrée et en sortie, un ServiceAccount sans montage automatique de jeton et un Role portant les permissions de la section 7, à partir d'un modèle revu à ce moment. `delete` n'est accordé que pour l'étape d'arrêt ultérieure. | Objets créés et leurs UID conservés. Une NetworkPolicy créée ne prouve pas son application (étape 4a). | Toute création partielle : inspecter, ne jamais supprimer puis recréer. |
| 2 | `agentsandbox kubernetes doctor --check-permissions` avec l'identité du broker. | Rapport conservé. Ses douze revues d'accès fixes incluent `patch pods`, `update pods`, `delete pods` et `create pods/attach` ; reste à confirmer que `create` est bien le verbe vérifié pour un upgrade WebSocket d'attachement (EXT-8). `delete pods` doit rester refusé jusqu'à l'étape d'arrêt ; `update pods`, exec, logs, Secrets, bindings et demandes de jeton doivent être refusés. | Tout refus des revues, ou l'une de ces permissions refusées accordée. |
| 3 | (Exige une autorisation explicite du mainteneur, non accordée par ce travail.) L'opérateur crée un PVC `ReadWriteOncePod` nommé pour une tentative ; `inspect_session_workspace_claim` le renvoie `Bound` avec son UID. Cela exige le patch proposé 0004 ; le profil de QA admis n'a pas de réseau (`--network none`), donc une exécution de QA contre un cluster exigerait un profil réseau admis séparément. | Projection du claim conservée. | Classe sans RWOP ou `WaitForFirstConsumer` : `BLOCKED`. |

### 8.4 Suite d'effets (toute BLOCKED)

Les étapes 4 à 12 restent `BLOCKED` tant que les demandes Root R1–R7, les
décisions Q1 et Q6 et une autorisation explicite n'existent pas : étape 4
CREATE retenu, y compris la vérification que le 201 admis passe la projection
fermée de réponse admise (booléens faux omis et formes des quantités,
EXT-14) ; étape 5 levée du gate avec préconditions et refus d'un
`resourceVersion` périmé, qui exige aussi l'étape 4a ; étape 6 attachement
limité au préambule avec sortie 66 du worker observée ; étape 7 détection
d'un Pod remplacé et d'un namespace recréé ; étape 8 un second attacheur et
une levée du gate par une autre identité ; étape 9 confirmation réseau depuis
le Pod de session libéré (DNS, IP directe, IPv6, tunnel ; gates
G05/G07/G11) ; étape 10 arrêt avec précondition d'UID et observation
terminale ; étape 11 règlement du quota ; étape 12 nettoyage exact du Pod et
du PVC par UID.

L'étape 4a observe l'application par le CNI avant toute levée du gate. Elle
n'emprunte aucun chemin d'effet du produit : elle exige l'étape 1 (sa
politique), et non les demandes et décisions Root ci-dessus. Elle reste
`BLOCKED` tant que sa propre autorisation explicite du mainteneur n'existe
pas, distincte de celles des étapes 1 et 4, et doit réussir avant l'étape 5.

| Étape | Action | Oracle et preuves | Arrêt |
|---|---|---|---|
| 4a | (Exige sa propre autorisation explicite du mainteneur, non accordée par ce travail.) L'opérateur, et non l'identité du broker, crée deux Pods hors session à partir des mêmes manifeste et image, revus à ce moment, qui ne diffèrent que par leur nom et leur Namespace. Aucun n'a de jeton de service account ni de contenu de session, et tous deux sont nommés hors du préfixe `ai-guard-session-` : un Pod de sonde dans le Namespace du tenant, sous la politique de refus par défaut de l'étape 1, et un Pod de référence dans un Namespace de référence distinct, de même niveau Pod Security et sans NetworkPolicy. Depuis les deux Pods, il sonde les cibles listées ci-dessous, avec le même outil, le même protocole, le même port et le même délai. Le retrait des deux Pods et du Namespace de référence par leurs UID, une fois les preuves conservées, relève de la même demande d'autorisation. | Une cible n'est discriminée que si le Pod de référence l'atteint. L'étape 4a réussit lorsque chaque cible est refusée depuis le Pod de sonde, que les contrôles positifs réussissent et que les cibles DNS (53/UDP et 53/TCP), Service `kubernetes`, nœud et IPv4 sont toutes discriminées. Une cible que le Pod de référence n'atteint pas est consignée comme non discriminée, jamais comme une preuve d'application. Les preuves listées ci-dessous sont conservées. | Une cible atteinte depuis le Pod de sonde : la politique n'est pas appliquée pour cette destination, et les étapes 5 à 12 restent `BLOCKED`. Un contrôle positif en échec, ou une cible requise non discriminée : l'étape 4a n'a pas réussi, et les étapes 5 à 12 restent `BLOCKED`. Toute création partielle : inspecter, ne jamais supprimer puis recréer. |

Cibles, sondes et preuves de l'étape 4a :

- **Cibles**, chacune avec un listener connu avant l'étape : l'IP du Service
  DNS du cluster (en général le Service `kube-dns`, à confirmer sur le
  cluster) sur 53/UDP et 53/TCP ; l'IP du Service `kubernetes` sur 443/TCP ;
  l'InternalIP du nœud sur un port dont le listener est connu ; une adresse
  IPv4 et une adresse IPv6 d'un listener hors du Namespace du tenant, choisi
  et revu à ce moment ; et `169.254.169.254` sur 80/TCP. L'adresse, le port
  et le protocole de chaque cible sont consignés. Tout objet créé pour un tel
  listener relève de la même demande d'autorisation et est retiré par son UID.
  Un cluster sans chemin IPv6, ou sans listener à `169.254.169.254`, laisse
  cette cible non discriminée : ce fait est consigné, et l'étape 9 la couvre
  toujours.
- **Référence.** Chaque cible est sondée depuis le Pod de référence, dans la
  même demande d'autorisation et à un moment proche de la tentative depuis le
  Pod de sonde. Sans référence consignée, une cible n'est pas discriminée, et
  son échec n'est jamais une preuve d'application.
- **Sondes** : une connexion ou une négociation (handshake) seulement. Une
  sonde TCP ouvre une connexion et la ferme sans envoyer de données
  applicatives : ni chemin de requête, ni en-tête, ni corps, et jamais de
  requête vers un chemin de credentials de métadonnées à `169.254.169.254`.
  La sonde 53/UDP, qui n'a pas de connexion, envoie une seule requête DNS pour
  un nom fixe revu à ce moment. Chaque tentative consigne son résultat :
  connexion établie ou réponse reçue, refus, ou délai expiré.
- **Contrôles positifs.** À l'intérieur du Pod de sonde, le même outil sonde
  un listener de ce Pod avec chaque protocole qu'utilisent les sondes. Un
  contrôle montre seulement que l'outil fonctionne, jamais qu'une cible est
  joignable. Qu'une NetworkPolicy s'applique au trafic loopback d'un Pod vers
  lui-même, ou au trafic entre un Pod et son nœud, est un fait à vérifier dans
  la [documentation officielle des NetworkPolicy](https://kubernetes.io/docs/concepts/services-networking/network-policies/),
  jamais supposé ; quoi qu'indique cette documentation, une cible atteinte
  depuis le Pod de sonde est un arrêt. Pod Security `restricted` devrait
  exiger le retrait de toutes les capabilities, `NET_RAW` compris (à vérifier
  dans les [Pod Security Standards](https://kubernetes.io/docs/concepts/security/pod-security-standards/)
  officiels) ; aucune sonde ni aucun contrôle n'utilise donc ICMP.
- **Preuves.** Sont conservés : les UID des Namespaces du tenant et de
  référence ; la liste des NetworkPolicy de chaque Namespace, avec l'UID et le
  `resourceVersion` de chaque politique ; les UID des deux Pods ; chaque
  commande et sa sortie. Chaque sortie est relue avant d'être conservée, et
  tout credential, jeton ou autre contenu ressemblant à un secret est masqué ;
  le registre nomme chaque masquage.

`BLOCKED` sans décision supplémentaire : un audit indépendant du cluster,
l'application par le CNI et l'exclusivité des acteurs. L'étape 5 dépend de
l'étape 4a : elle reste `BLOCKED` tant que l'étape 4a n'a pas réussi, car une
NetworkPolicy créée ne permet pas à elle seule une levée du gate. Juste avant
la levée, l'opérateur relit en lecture seule le Namespace du tenant et ses
NetworkPolicy ; l'étape 5 refuse, et l'étape 4a est refaite sous une nouvelle
autorisation, si l'UID du Namespace, la liste des NetworkPolicy ou l'un de
leurs UID ou valeurs de `resourceVersion` diffère de ce que l'étape 4a a
conservé. Cette relecture n'est pas une précondition atomique de la levée ;
l'exclusivité des acteurs reste `BLOCKED`. L'étape 9 reste la confirmation
depuis le Pod de session libéré lui-même, y compris pour les cibles que
l'étape 4a a laissées non discriminées.

## 9. Limites et gates ouverts

- Non qualifié : tout comportement réel de Kubernetes (CREATE, gate, patch,
  attachement, suppression, Pod Security, verbes RBAC, `ReadWriteOncePod`,
  encodage des booléens omis, formes des quantités, persistance des logs),
  Docker Desktop, l'application de NetworkPolicy, de CSI et des cgroups, le mode
  d'attente dans une image, tout permis d'effet, l'échéance durable, le handoff
  et le démarrage, le règlement du quota après effet, et la montée à
  100 workers.
- Les peers de test en loopback répondent avec des fixtures déclarées ; ils ne
  qualifient jamais un serveur d'API Kubernetes, un provisionneur ou
  `ReadWriteOncePod`.
- Les gates G05, G07 et G11 restent ouverts, G11 `BLOCKED`. Le produit n'est
  pas prêt pour la production, et l'absence de chemin d'effet est la barrière
  qui garde le lancement Kubernetes fermé.

## 10. Points ouverts cités dans ce guide

Ces identifiants viennent de la conception et du contrat du lot, conservés
hors de ce dépôt. Chaque point est ouvert : ce travail n'en décide ni n'en
livre aucun.

- **Root** : le propriétaire des contrats du contrôle central et des fichiers
  partagés entre les lots de travail. Il décide les demandes R1–R11 et les
  questions Q1–Q9.
- **R1** : états centraux d'effet V2 et leur migration, y compris la règle
  selon laquelle un UID de claim n'est jamais réutilisé et la liaison centrale
  de l'empreinte de la requête et de l'UID du claim.
- **R2** : routes HTTP du broker et confirmation vivante reçue par HTTP ;
  elles donneraient un appelant de production aux mécanismes du cycle de vie.
- **R3** : une échéance durable armée avant la levée d'un gate, indépendante
  de la CLI.
- **R4** : les trames après le `Ready` du mode d'attente (handoff, démarrage,
  résultat déclaré).
- **R5** : la politique de stockage des claims de workspace des sessions.
- **R6** : les modifications de fichiers hors de ce lot, livrées uniquement
  comme patches proposés.
- **R7** : la capacité d'activation de production, qui reste fermée.
- **R8** : la coordination avec la prochaine version du schéma central
  (demande d'information).
- **R9** : la confirmation que `pods/exec` reste exclu.
- **R10** : le refus, au chargement de la configuration, de deux tenants qui
  partagent une autorité ou un namespace Kubernetes.
- **R11** : une confirmation de successeur pour rouvrir le journal d'une autre
  incarnation.
- **Q1** : savoir si un attachement limité au préambule, encadré par deux
  observations exactes, est un argument d'identité d'attachement acceptable.
- **Q6** : l'orthographe et le point d'entrée du mode d'attente du worker
  (`kubernetes-session-wait-v2`).
- **Q7** : la règle du nom de claim par tentative et la règle de
  non-réutilisation des UID de claim.
- **Q9** : la conduite à tenir si la seule StorageClass utilise
  `WaitForFirstConsumer`.
- **EXT-8** : le verbe RBAC vérifié pour l'upgrade WebSocket d'attachement.
- **EXT-10a/b** : la restriction de `ReadWriteOncePod` aux volumes CSI ; le
  type de provisionneur de la classe `hostpath` par défaut de Docker Desktop et
  son adossement à un répertoire du nœud.
- **EXT-13** : la politique d'écart de versions de kubectl.
- **EXT-14** : la façon dont Kubernetes v1.36 encode les booléens faux omis et
  les quantités canoniques dans un Pod admis.
