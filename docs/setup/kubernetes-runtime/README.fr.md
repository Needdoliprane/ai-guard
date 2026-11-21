# Runtime de session Kubernetes V2 : état, mécanismes de reprise et plan Docker Desktop

[English](README.md) · [Français](README.fr.md) · [Guide Kubernetes](../kubernetes/README.fr.md)

**Aucune session Kubernetes ne peut être lancée avec ce produit.** Il n'existe
ni option `--runtime kubernetes`, ni activation de production. La CLI R2
prépare, inspecte et abandonne des réservations avant tout effet runtime
([section 12](#12-http-broker-r2-et-cli-avant-effet)).
Ce guide décrit le code du runtime de session V2 présent dans les
sources Rust, ce qu'un opérateur peut faire aujourd'hui, et le plan qui le
qualifiera plus tard sur le cluster Kubernetes à un nœud de Docker Desktop. Ce
plan **n’a pas été exécuté sur un cluster réel**. Les [preuves du 13 novembre](../../engineering/evidence/2026-11-13/claude-composition/README.fr.md) consignent 144 parents logiciels Kubernetes sélectionnés PASS sur leur source exacte, avec un RC99 global et un écart de sous-cas explicite. Le [registre initial](../../engineering/evidence/2026-11-12/kubernetes-v2-runtime/README.md) conserve son état de livraison daté.
Le contrat de référence est le [contrat d'admission Kubernetes](../../engineering/KUBERNETES_ADMISSION.md).

La source courante compose le lot S2b : attachement limité au préambule,
durcissement et dispatch du mode d'attente du worker
([section 11](#11-attachement-limité-au-préambule-et-mode-dattente-du-worker-lot-s2b)).
Les [preuves ciblées du 14 novembre](../../engineering/evidence/2026-11-14/kubernetes-attach-composition/README.fr.md)
consignent build/check RC0 et 11 parents uniques PASS sur
`1e30e5e8546ab614eba7849789a1a0468cbc2843` : quatre contrats de vrais flux
avec peers synthétiques et sept cas de subprocess stdio réels du
binaire Rust normal `ai-guard-provider`. Seuls son attente et ses arguments
sont exercés ; aucun client ou modèle IA. Les 121 PASS distincts sur
`be6b12dce7f74e2062f7ad5524e12818bf7313b5` restent historiques et ne sont
pas rejoués sur `1e30e5e`. La [campagne composée inspection sur `45179db`](../../engineering/evidence/2026-11-14/owned-policy-inspection/README.fr.md)
réexécute ces quatre contrats et sept cas stdio parmi ses 47 parents ciblés.
Elle ne rejoue pas les 121 tests historiques et ne qualifie aucun cluster ;
les comptes de ces sources ne sont pas additionnés. Seul le contrat 1 utilise un upgrade
HTTPS vers un peer TLS loopback synthétique ; les contrats 2–4 utilisent des
duplex mémoire, sans preuve TCP/TLS. Aucun parcours de session du produit
n'atteint l'attachement.

## 1. État par couche

| Couche | Ce qui existe | Qui peut l'appeler aujourd'hui | Vérifié |
|---|---|---|---|
| A. Entrée de bibliothèque avec données réelles | `KubernetesRuntime::inspect_session_workspace_claim(name)` : nom vérifié comme label DNS avant toute requête, puis GET des deux ancres de namespace, du PVC et à nouveau des ancres. Un nom invalide est signalé comme `InvalidAuthority` avant toute requête, un claim qui ne remplit pas les exigences de la section 3 comme `InvalidResponse`, et un PVC absent comme `ApiRejected(404)`. | Du code Rust qui détient un `KubernetesRuntime`. **Aucun binaire du produit ne l'appelle** ; une entrée CLI en lecture seule, qui associerait les deux premières erreurs à des messages dédiés, n'est qu'un patch proposé. | Sélection historique PASS sur sa source ; mécanismes plus larges et cluster non qualifiés par la sélection 45179db |
| B. Mécanismes purs (internes à la crate) | Rendu exact de la requête de Pod V2, projection de la réponse admise, parseur exact d'observation, enregistrements du journal. | Code de la crate seulement. | Sélection historique PASS sur sa source ; mécanismes plus larges et cluster non qualifiés par la sélection 45179db |
| C. Cycle de vie avant effet | Chaîne de rétention du journal, confirmation HTTP vivante et réouverture, abandon avant tout effet, reprise par un successeur et rejeu du reçu. | HTTP broker R2 et commandes `kubernetes session prepare`, `status`, `abandon` pour schémas11–13. Le port broker natif Source17 reste indisponible. Le rapport de réconciliation runtime reste interne. | Raccords R2 et CLI : NOT_RUN ; les sélections historiques restent liées à leur source |
| D. Mécanismes d'effet (fermés) | CREATE retenu des octets exacts, PATCH de levée du gate avec opérations JSON-Patch `test`, DELETE avec précondition d'UID ; attachement S2b limité au préambule. | Une garde scellée dont la seule implémentation existe dans les tests, liée à un listener loopback possédé. Aucun chemin du produit n'atteint un effet. | Quatre contrats attach PASS sur 1e30e5e puis sur 45179db ; autres sélections liées à leur source ; effets réels et cluster NOT_RUN |
| Mode d'attente worker S2b | `ai-guard-provider` sélectionne le mode fixe avant le parcours fournisseur. Il échange Hello/Ready, puis refuse tout octet supplémentaire. | Uniquement l'entrée du mode d'attente du binaire existant ; aucun lanceur de session. | Sept cas stdio du binaire normal PASS sur 1e30e5e puis sur 45179db ; image worker et cluster NOT_RUN |
| E. Raccords et qualification manquants | Demandes typées R1–R11 au propriétaire du contrôle ; ce plan Docker Desktop. | — | non commencé |

Les permis d'effet, le handoff, le démarrage, l'observation terminale et le
règlement du quota après effet **ne sont pas implémentés**. S2b implémente
seulement le préambule et le mode d'attente en source. Cela ne ferme aucune
qualification du runtime Kubernetes ni aucun gate de production.

Les résultats du 13 novembre concernent uniquement les sélections du [relevé typé](../../engineering/evidence/2026-11-13/claude-composition/TESTS.json), pas toutes les combinaisons de chaque couche. Global RC99 : un sous-cas release autre UID reste NOT_RUN. Aucun effet produit S5 ouvert. Q1/Q6 sont décidées pour le seul code du préambule S2b ; elles n'autorisent pas l'activation.

## 2. Ce qu'un opérateur peut faire aujourd'hui

- **Fermer une réservation qui n'a émis aucun effet** avec la route Runner
  `control-recovery` existante du contrôle central. Elle est décrite dans
  [Configurer les politiques de réservation de session V2](../kubernetes/README.fr.md#configurer-les-politiques-de-réservation-de-session-v2)
  et dans le [contrat d'API](../../api.md#récupération-contrôle-des-réservations-kubernetes-v2--kubernetes-session-control-recovery-v2).
  Ce travail ne modifie pas cette route.
- **Inspecter l'autorité Kubernetes** avec la commande existante en lecture
  seule `agentsandbox kubernetes doctor` (voir le [guide Kubernetes](../kubernetes/README.fr.md#lancer-le-diagnostic-cli-en-lecture-seule)).
- **Inspecter un claim de workspace existant** avec la commande en lecture
  seule `agentsandbox kubernetes inspect-workspace --claim-name "$QA_KUBE_CLAIM"`.
  Fournir les mêmes options explicites d'autorité, d'AC et d'authentification
  que pour `doctor`. L'identité doit disposer du GET sur le PVC nommé dans
  le namespace de l'autorité. Un nom de PVC contenant des données
  d'authentification protégées est refusé avant la première requête.
  La commande distingue un claim Pending, un mode d’accès incompatible et
  une classe de stockage non nommée. Elle revérifie les ancrages de l’autorité
  avant de rendre ces diagnostics. Ses lectures portent sur le PVC nommé et
  les ancrages de l’autorité.
  Le JSON renvoie la projection vérifiée du claim,
  son UID et `admission_available: false`.
  [Commandes de rejeu](../../../tests/e2e/README.md#rejouer-le-diagnostic-et-le-prévol-kubernetes-sur-docker-desktop)
- Préparer/réouvrir, lire l'état central et abandonner une réservation de
  session avant effet avec la [CLI R2](#12-http-broker-r2-et-cli-avant-effet).
  Ces commandes n'appellent pas l'API Kubernetes. Le rapport de
  réconciliation runtime reste un mécanisme de bibliothèque.

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
  `ai-guard-provider` (`kubernetes-session-wait-v2`, fixé par la décision
  Root Q6), jamais une invocation métier. Elle n'a ni
  environnement, ni Secret, ni jeton, ni chemin hôte, ni jeton de
  service-account, et porte un scheduling gate.
- **Réponse au CREATE.** Un HTTP `201` de Kubernetes doit satisfaire les
  vérifications de correspondance exacte avec la requête de Pod retenue avant
  de produire une cible typée, non duplicable. L'acceptation HTTP n'est ni un
  état `Running` observé, ni une preuve indépendante de l'identité du Namespace,
  du cluster ou des droits RBAC. L'exécution en production et le chemin broker
  natif Source17 restent indisponibles.
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
- **Attachement (préambule seulement, en source, fermé) :** Q1 accepte
  l'argument encadré pour le seul code du préambule. Seul le Hello non secret
  voyage avant la seconde observation. Cela ne constitue ni une précondition
  atomique ni une preuve d'identité ; la section 11 donne ses hypothèses et
  limites. Toute tentative initiée doit être traitée comme finale pour le
  Pod. Aucun chemin du produit ne l'atteint.
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

Le plan complet ci-dessous reste à exécuter. Une recette plus étroite sur
Docker Desktop v1.36.1 a vérifié le DNS sur TCP entre deux pods du même nœud,
avec refus distinct en entrée et en sortie, puis reprise. L'étape 4a et la suite
d'effets de session demandent leurs propres observations.
[Rejouer la recette NetworkPolicy locale](../../../tests/e2e/README.md#vérifier-lapplication-des-networkpolicy-locales)

**Chaque étape qui modifie le cluster exige
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
- Une image worker construite localement à partir de la source composée du
  mode d'attente, avec propriétaire et mode exécutable du bootstrap inspectés,
  référencée par digest et présente dans le magasin d'images du nœud
  (`imagePullPolicy: Never`). Le dispatch est désormais raccordé en source ;
  le build d'image et son exécution restent NOT_RUN. L'ancien patch proposé
  de propriété de l'image n'est pas appliqué ici.
- `agentsandbox` construit uniquement dans l'environnement de QA isolé admis.

### 8.2 Prérequis de stockage

- Une StorageClass dont le provisionneur prend en charge `ReadWriteOncePod`.
  Kubernetes v1.36 réserve ce mode aux volumes CSI. Sur le cluster Docker
  Desktop v1.36.1 inspecté, `hostpath` et `standard` utilisent tous deux
  `rancher.io/local-path` avec `WaitForFirstConsumer`. Le cluster ne déclarait
  aucun pilote CSI, donc la recette de stockage réelle est `BLOCKED`.
  [Modes d'accès Kubernetes v1.36](https://v1-36.docs.kubernetes.io/docs/concepts/storage/persistent-volumes/#access-modes)
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
| 3 | (Exige une autorisation explicite du mainteneur, non accordée par ce travail.) L'opérateur crée un PVC `ReadWriteOncePod` nommé pour une tentative. La commande `kubernetes inspect-workspace` vérifie en lecture seule qu'il est `Bound` et retourne son UID. | Projection du claim conservée. | Classe sans RWOP ou `WaitForFirstConsumer` : `BLOCKED`. |

### 8.4 Suite d'effets (toute BLOCKED)

Les étapes 4 à 12 restent `BLOCKED` tant que les demandes Root R1–R7
manquantes, les permis d'effet S5 et une autorisation explicite n'existent
pas. Q1 et Q6 sont décidées pour le seul code du préambule S2b. La future
suite comprend : étape 4 CREATE retenu, y compris la vérification que le 201 admis passe la projection
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
  d'attente dans une image worker, tout permis d'effet, l'échéance durable, le handoff
  et le démarrage, le règlement du quota après effet, et la montée à
  100 workers.
- Les peers de test en loopback répondent avec des fixtures déclarées ; ils ne
  qualifient jamais un serveur d'API Kubernetes, un provisionneur ou
  `ReadWriteOncePod`.
- Les bornes logicielles S2b, l'interopérabilité WebSocket, `stdinOnce`, les
  signaux du worker et la rétention des logs du nœud restent non mesurés sur
  cluster. Sept cas stdio de processus passent sur `1e30e5e`, pour la seule
  attente et les arguments de l'adaptateur Rust normal. Les mêmes sept cas
  passent dans la composition inspection `45179db`, sans qualification
  runtime plus large.
- Les gates G05, G07 et G11 restent ouverts, G11 `BLOCKED`. Le produit n'est
  pas prêt pour la production, et l'absence de chemin d'effet est la barrière
  qui garde le lancement Kubernetes fermé.

## 10. Points ouverts cités dans ce guide

Ces identifiants viennent de la conception et du contrat du lot, conservés
hors de ce dépôt. Q1 et Q6 sont décidées pour le seul lot S2b ; les autres
points ci-dessous restent ouverts. Ce guide ne prend aucune nouvelle décision
du contrôle.

- **Root** : le propriétaire des contrats du contrôle central et des fichiers
  partagés entre les lots de travail. Il décide les demandes R1–R11 et les
  questions Q1–Q9.
- **R1** : états centraux d'effet V2 et leur migration, y compris la règle
  selon laquelle un UID de claim n'est jamais réutilisé et la liaison centrale
  de l'empreinte de la requête et de l'UID du claim.
- **R2** : les routes broker et la confirmation vivante ont désormais un
  appelant avant effet pour schémas11–13 en source (section 12, tests NOT_RUN).
  Le port broker authentifié/transaction natif Source17 reste à implémenter.
- **R3** : une échéance durable armée avant la levée d'un gate, indépendante
  de la CLI.
- **R4** : les trames après le `Ready` du mode d'attente (handoff, démarrage,
  résultat déclaré).
- **R5** : la politique de stockage des claims de workspace des sessions.
- **R6** : les modifications hors du lot initial, livrées comme patches
  proposés. Le dispatch du mode d'attente dans le binaire est désormais
  composé ; les modifications proposées de propriété de l'image et de CLI
  d'inspection du claim en lecture seule ne sont pas appliquées ici.
- **R7** : la capacité d'activation de production, qui reste fermée.
- **R8** : la coordination avec la prochaine version du schéma central
  (demande d'information).
- **R9** : la confirmation que `pods/exec` reste exclu.
- **R10** : le refus, au chargement de la configuration, de deux tenants qui
  partagent une autorité ou un namespace Kubernetes.
- **R11** : une confirmation de successeur pour rouvrir le journal d'une autre
  incarnation.
- **Q1** (décidée pour S2b) : l'argument encadré est accepté pour le seul
  code du préambule, sous les hypothèses de la section 11. Il ne prouve aucune
  identité et n'autorise aucun effet ; la garde reste scellée.
- **Q6** (décidée pour S2b) : le mode d'attente est dans `ai-guard-provider`,
  sélectionné par le seul argument `kubernetes-session-wait-v2` avant la
  lecture de l'environnement fournisseur. Tout argument supplémentaire est
  refusé avec la sortie 64 sans repli.
- **S5** : permis d'effet et implémentation de production de la garde scellée,
  avec checkpoint durable avant chaque effet ; bloqué sur R1–R3 et R7.
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

## 11. Attachement limité au préambule et mode d'attente du worker (lot S2b)

Le [propriétaire de l'attachement](../../../src/runtime/kubernetes/session_attach.rs),
l'[adaptateur WebSocket](../../../src/runtime/kubernetes/session_attach_codec.rs)
et le [mode d'attente du worker](../../../src/runtime/kubernetes/session_worker.rs)
existent en source. Le dispatch est raccordé au binaire existant
[`ai-guard-provider`](../../../src/bin/ai-guard-provider.rs).
Quatre contrats de vrais flux et sept cas stdio de processus du binaire normal
passent sur `1e30e5e` ; [les preuves](../../engineering/evidence/2026-11-14/kubernetes-attach-composition/README.fr.md)
en conservent le périmètre exact. Les 121 PASS historiques sur `be6b12d` ne
sont pas rejoués. La composition inspection `45179db` répète ces onze
contrôles ; sa [preuve propre](../../engineering/evidence/2026-11-14/owned-policy-inspection/README.fr.md) conserve la même portée logicielle. Aucun cluster, vrai worker ou client/modèle IA n'est exercé.
La garde d'effet n'a pas d'implémentation de
production ; aucune commande d'attachement de session, option d'activation
ou procédure opérateur n'existe.

### 11.1 Une tentative : Hello, Ready, observations, fermeture

L'attachement ne transporte que le préambule du bootstrap, sans donnée métier.

1. Vérifier les UID de `kube-system` et du Namespace du tenant, actifs et sans
   suppression en cours.
2. Observer l'UID exact du Pod retenu, l'empreinte de spec, le nœud, l'ID du
   conteneur worker en cours d'exécution, son heure de démarrage et son ID
   d'image, zéro redémarrage et aucune date de suppression. La phase du Pod
   seule ne fait pas preuve.
3. Vérifier la garde scellée, le drapeau d'annulation et l'échéance absolue
   avant d'initier une requête.
4. Initier un unique upgrade WebSocket vers le sous-resource attach du Pod
   nommé, avec les mêmes endpoint HTTPS, AC et credential que les observations.
   Les redirections sont refusées. Valider la réponse 101 et le seul
   sous-protocole `v5.channel.k8s.io` ; extensions et replis sont refusés.
5. Envoyer uniquement Hello sur stdin ; accepter un Ready correspondant sur
   stdout. Ils portent identifiants et empreintes non secrets. Tout octet
   bootstrap supplémentaire sur stdout est refusé ; le worker refuse tout
   nouvel octet stdin.
6. Observer à nouveau le Pod et comparer tous les champs de l'étape 2 ;
   revérifier les ancres de Namespace. Une contention locale des slots de
   requête fait reprendre ces lectures dans la même échéance, sans envoyer
   la requête refusée.
7. Terminer le flux et retenir le résultat local, quel que soit le verdict.

L'ordre normal est ancres, Pod, attach, Pod, ancres. Deux observations égales
permettent de rapporter le préambule comme **métadonnées déclarées**.
Ready est un écho : il ne prouve ni identité de processus ni exécution. Le
rapport local de tentative ne remplace pas l'audit indépendant durable
manquant des effets du produit.

Q1 accepte cet argument pour le seul code du préambule. Il suppose des UID
jamais réutilisés, des noms uniques, un serveur d'API et un kubelet honnêtes,
un attachement qui reste lié à son conteneur et une seconde observation la
plus récente. Aucune de ces hypothèses n'est vérifiée ici. L'attachement n'a
aucune précondition atomique d'UID de Pod : il adresse un nom entre deux
lectures distinctes. Si une hypothèse échoue, Hello peut atteindre un autre
processus. Aucun secret, credential, argv métier ou contenu du workspace ne
voyage avant ou après lui.

### 11.2 Propriété, annulation et fins d'échange

`stdinOnce: true` rend toute tentative **initiée** finale pour le Pod, même
sans réponse d'upgrade réussie. Il n'existe ni reconnexion ni second
attachement. La reprise exige une nouvelle réservation, un nouveau Pod et un
nouveau claim de workspace par tentative. Un refus avant initiation n'a pas
touché le Pod.

Dès l'initiation HTTP, une tâche possède l'upgrade en attente, puis le flux
et son slot de capacité. La perte de l'appelant n'annule pas la requête et
ne rend pas le slot : le propriétaire attend la réponse ou l'échéance de
handshake, puis la fin effective du flux. Sans seconde observation de
l'appelant, la fin reste sans identité. L'arrêt du runtime abandonne ce que
le propriétaire détient et consigne un motif d'arrêt du propriétaire ;
abandonner un propriétaire n'est jamais un succès.

L'annulation est vérifiée avant initiation et sondée environ toutes les
50 ms pendant l'attente de Ready ou de la seconde observation. C'est une
constante source, pas une latence mesurée. Elle n'interrompt ni upgrade en
attente ni écriture que le peer n'absorbe pas. Cette écriture attend
l'échéance et finit en `AttachDeadline`, avant ou après Ready ; une demande
d'annulation pendant l'écriture est conservée séparément comme cause. Une
trame potentiellement coupée rend le flux inutilisable : aucune autre trame
n'est envoyée.

Le transport vérifie l'échéance absolue de façon synchrone après construction
de la requête et validation de la garde, juste avant la marque d'initiation
et le premier poll HTTP. Une requête périmée retourne `AttachDeadline` avec
`request_initiated=false`, sans consommer l'unique attachement du Pod. La
nouvelle régression transport de cette frontière est `NOT_RUN` sur la source
courante ; les résultats historiques ci-dessus ne couvrent pas ce changement.

Sur un flux utilisable, le propriétaire ferme stdin avec le message v5
`[255, 0]`, puis envoie une fermeture WebSocket et arrête le transport.
Chaque étape a sa propre limite d'une seconde et peut suivre l'échéance de
l'encadrement. Après une fermeture peer bien formée, seule la réponse de
fermeture de la bibliothèque est écrite ; après un transport en échec ou une
trame coupée, seul l'arrêt borné reste possible. Une fermeture malformée
refusée par la bibliothèque laisse l'écriture utilisable : la fermeture de
stdin et le code 1002 sont encore tentés. Le slot revient après la fin du
propriétaire. Aucune de ces fins n'est une preuve terminale observée du Pod.

| Résultat | Signification |
|---|---|
| `AttachHandshakeRefused` | L'upgrade initié a renvoyé un statut, des en-têtes, un sous-protocole ou une extension inattendus. Aucun flux ni aucune trame. |
| `AttachProtocol` | Trame, canal ou forme de statut refusés, ou borne de contenu dépassée ; tout octet bootstrap supplémentaire après Ready. |
| `AttachIdentityUnproven` | Pas de deux observations égales autour d'un Ready accepté : divergence, échec d'observation, perte de l'appelant, annulation, fin par le peer, arrivée d'un statut ou autre échec du bootstrap. Une échéance après Ready sans écriture en attente a aussi ce résultat. |
| `AttachCapacity` | Les deux slots d'attachement du runtime sont occupés. Cela ne couvre pas un abandon de l'allocateur dans le codec. |
| `AttachDeadline` | Échéance invalide ou dépassée, timeout d'upgrade ou de handshake avant Ready, ou écriture encore en attente à l'échéance. |
| Erreurs existantes avant requête / de transport | Précondition locale, garde ou annulation précoce peuvent renvoyer `EffectRefusedBeforeSend` ; les observations, `InstanceReplaced`, `ApiRejected` ou `AuthorityChanged`. Une réponse d'upgrade perdue peut renvoyer `TransportUnavailable` ou `Timeout` ; le marqueur d'initiation rend toujours cette tentative finale. |

Une fermeture peer est une fin par le peer seulement sans corps ou avec le
code 1000 ou 1001. Tout autre code, corps malformé ou motif UTF-8 invalide est
refusé. EOF ou échec du transport sont aussi des fins par le peer. Aucune ne
prouve une identité ou un succès.

### 11.3 Bornes logicielles configurées, pas une capacité mesurée

| Borne | Valeur |
|---|---|
| Trame WebSocket et message réassemblé | 64 Kio chacun, octet de canal compris |
| Une lecture / tampon initial de lecture | 4 Kio ; le tampon de trame peut croître |
| File d'écriture | Une trame client, 4 111 octets ; aucun regroupement d'écritures |
| Stderr worker | 16 Kio comptés puis écartés par ai-guard |
| Document de statut | 4 Kio, projeté en statut et motif fermés ; aucun texte peer conservé |
| Corps d'enveloppe bootstrap | 16 Kio, longueur vérifiée avant allocation |
| Upgrade et échange du préambule | 5 s chacun, dans l'encadrement global |
| Encadrement global | Au plus 15 s, observations comprises |
| Fermeture après la fin | Au plus trois étapes de 1 s chacune |
| Tentatives simultanées de préambule | Deux par runtime ; pas une capacité de 100 sessions |

Seuls les messages binaires sur stdout, stderr et statut sont admis. Un
payload stdout vide est permis ; un message sans octet de canal ou sur
stdin, resize, close ou un canal inconnu est refusé. Le statut est réduit à
`Success`/`Failure` et un motif fermé (`NonZeroExitCode`, `InternalError`,
autre ou absent). Un statut arrivé avant la fin des observations ne prouve
pas un préambule achevé. Interopérabilité avec serveur d'API/kubelet réels,
échéances, mémoire et débit restent non mesurés.

### 11.4 Bibliothèque WebSocket maintenue et compatibilité H21

`tokio-tungstenite` **0.30.0** est épinglé exactement, avec ses fonctionnalités
par défaut désactivées, et utilise `tungstenite` 0.30.0 (respectivement MIT
et MIT-ou-Apache-2.0). Il enveloppe un flux déjà upgradé dans le rôle client.
Il ne réalise ici ni handshake, connexion, TLS ou redirection. ai-guard
conserve validation d'upgrade, politique des canaux et autorisation. Les
erreurs de bibliothèque deviennent des valeurs fermées ; leur texte n'est
ni formaté ni conservé.

H21 conserve explicitement deux cas de compatibilité : un payload de quatre
octets annoncé sur 16 bits et un court message Ready annoncé sur 64 bits
sont acceptés comme le même payload. **Les longueurs non minimales sont
acceptées** ; il ne s'agit pas d'une application stricte de la longueur
minimale de RFC 6455. Les bornes de 64 Kio de trame déclarée et de message
agrégé restent appliquées. Une longueur sur 64 bits dont le bit haut est
levé, ou une trame déclarée au-delà de la borne, est refusée dès l'en-tête
comme trop grande.

Opcodes réservés et tailles de trame déclarées excessives sont refusés dès
l'en-tête. Les autres refus de bibliothèque (bits réservés, trames serveur
masquées, fragmentation ou trames de contrôle invalides) suivent la lecture
du payload borné. Le texte est lu avant son refus par ai-guard. Un peer qui
retient ce payload peut consommer l'échéance. Le fragment qui dépasse la
borne agrégée peut coexister en mémoire avec le message en cours d'assemblage.

Ce sont des bornes de contenu, **pas une borne exacte du tas**. Les tampons
croissent et les allocations infaillibles de la bibliothèque peuvent
abandonner le processus par manque de mémoire ; aucune reprise typée n'est
garantie. Un panic de bibliothèque arrête le propriétaire et rend son slot
avec la cause d'arrêt du propriétaire. La bibliothèque émet des messages
`log` debug/trace pouvant inclure trames et payloads. Cette crate n'installe
aucun logger aujourd'hui ; un futur logger global à ces niveaux pourrait
recevoir Hello, Ready et des octets contrôlés par le peer. Aucun plafond de
journalisation à la compilation n'est implémenté.

### 11.5 Mode d'attente du worker et stdio réels

La commande de Pod rendue utilise le binaire existant avec un argument fixe :

```text
/usr/local/bin/ai-guard-provider kubernetes-session-wait-v2
```

C'est la commande bootstrap du Pod, pas une procédure de lancement de
session. Les arguments bruts la sélectionnent avant décodage en texte et
avant lecture de l'environnement de handoff fournisseur. Tout argument
supplémentaire, même UTF-8 invalide, sort en 64 sans construire de runtime,
lire stdin ni se replier. Les autres premiers arguments conservent le
parcours fournisseur ; UTF-8 invalide y reste une sortie 126.

Le worker attend sans timeout local avant le premier octet et après Ready ;
`activeDeadlineSeconds` du Pod borne le runtime, sans être une échéance
durable du broker. Le premier octet arme cinq secondes pour compléter Hello
et flusher Ready. Aucun programme n'est exécuté et aucune trame suivante
n'est acceptée. Seul Ready va sur stdout ; un diagnostic fixe sans
identifiant ni contenu de trame va sur stderr.

| Sortie | Fin du mode d'attente |
|---|---|
| 64 | Refus de protocole : trame invalide, excessive, tronquée ou inattendue ; tout octet après Ready ; tout argument après le mode. |
| 65 | Hello ou le flush de Ready n'a pas fini dans les cinq secondes suivant le premier octet. |
| 66 | EOF avant Hello ou après Ready, sans handoff. Attendu pour un échange limité au préambule, jamais un succès de session. |
| 70 | Échec local de lecture/écriture, de capacité ou de runtime. |
| 0 | Jamais renvoyé. |

La source appelle `shutdown_background` après l'échange : le lecteur stdin
bloquant de Tokio peut rester en attente quand le délai du handshake
expire ; attendre la destruction du runtime empêcherait alors la sortie.
Sept tests passent sur le vrai binaire Rust normal et ses pipes à
`1e30e5e`, dont un Hello partiel avec stdin encore ouvert lors de la sortie 65
observée. Cela exécute l'adaptateur provider interne, pas un client/modèle IA.
Ces tests stdio n'exercent ni image worker, transport attach ou cluster ; les
quatre contrats de flux distincts utilisent des peers synthétiques : un
contrat HTTPS/TLS loopback et trois contrats de duplex mémoire.
Leurs [preuves liées à la source](../../engineering/evidence/2026-11-14/kubernetes-attach-composition/README.fr.md)
sont complétées par la [preuve distincte 45179db](../../engineering/evidence/2026-11-14/owned-policy-inspection/README.fr.md), de même portée logicielle.

Le mode ne lit lui-même aucun credential ou environnement. La construction
de son runtime Tokio peut lire le réglage Tokio
`TOKIO_UNSTABLE_SHARDED_BLOCKING_QUEUE` ; le refus d'argument supplémentaire
n'en construit aucun. La requête de Pod n'a pas d'environnement.
Le mode n'installe aucun handler de signal : comportement SIGTERM en PID 1,
échéance active et délai de terminaison rendu de 30 secondes restent à observer.

Ready répète sur stdout les identifiants et quatre empreintes de Hello.
Les logs du runtime du nœud peuvent conserver ces métadonnées de session et
stderr hors d'ai-guard ; ce fait n'a pas été observé sur cluster ici.
ai-guard ne masque, ne borne ni ne supprime ces logs du nœud. Aucun secret
ne doit voyager sur ce canal ; la protection des sorties avant persistance
par le runtime reste ouverte.

### 11.6 Prérequis fermés

- **R4 :** aucune trame handoff, Start ou résultat déclaré après Ready.
- **R7 :** aucune activation de production ou exclusivité prouvée des
  permissions attach/exec dans le Namespace du tenant.
- **R10 :** refuser au chargement de configuration le partage d'une autorité
  ou d'un Namespace Kubernetes entre tenants reste un prérequis, pas un
  contrôle livré.
- **S5 / garde scellée :** aucun permis d'effet de production ni chemin
  d'attachement du produit.
- **Qualification cluster :** interopérabilité upgrade/canaux, verbes RBAC
  effectifs (EXT-8), feature gates, `stdinOnce`, logs du nœud, signaux, CNI,
  stockage, échéances et capacité de 100 workers restent BLOCKED. L'étape 6
  reste BLOCKED ; sources et résultats loopback ne qualifient pas un cluster.

## 12. HTTP broker R2 et CLI avant effet

Ces commandes préparent, inspectent et abandonnent des réservations avant tout
effet runtime, avec le ledger existant des schémas11–13. Le chemin broker natif
Source17 reste indisponible : il exige `AuthenticatedExecutionBroker`,
les grants/source/policy courants et leur revalidation dans la même
transaction, et il est refusé avant lecture du corps HTTP. Un `runner` partagé
n'est pas une identité broker ou cloud de remplacement. L'exécution en
production reste indisponible. Aucune commande ci-dessous ne crée de Pod, ne
lève de gate, n'attache de flux ou n'exécute de client IA.

Le gateway doit déjà avoir son identité workload broker courante, son grant
Kubernetes, `session_reservation_policy` et le quota du tenant configurés
([politiques existantes](../kubernetes/README.fr.md#configurer-les-politiques-de-réservation-de-session-v2)).
`session_recovery_policy` est une autorisation distincte, facultative à
l'installation, pour reprendre un abandon retenu avec une incarnation
successeur. Elle vient de la configuration du tenant au gateway ; le client
ne transmet aucune politique. Le mode sans politique refuse cette reprise.

Le [profil JSON consommé par la CLI](examples/session-control-v2.json)
contient uniquement des valeurs publiques d'exemple. Remplacer endpoint,
tenant, namespace de contrôle, broker, incarnation et credential par les
valeurs de l'installation. Le token reste dans son fichier privé désigné ;
aucun secret dans les arguments. Le profil, l'intention et les credentials
doivent être des fichiers privés réguliers, inchangés, sans lien ; les chemins
sont absolus. HTTPS vérifié est exigé hors loopback IP. Omettre
`gateway_ca_file` pour les racines publiques ; une AC privée fournit uniquement
sa chaîne publique. Aucun accès Kubernetes ni credential Kubernetes n'est
nécessaire pour R2.

Une intention V2 fermée est fournie par le constructeur Rust existant
`KubernetesSessionProfileV2::build` puis
`KubernetesSessionReservationIntentV2::new`. Le fichier conserve les mêmes
IDs/empreintes à chaque reprise ; ce lot n'ajoute pas de générateur interactif
de profil ou d'autorisation. Ne pas y insérer de prompt, de credential ou
d'argv métier : seules leurs empreintes prévues par le contrat y figurent.

```console
agentsandbox kubernetes session prepare --control-file /chemin/prive/control-v2.json --intent-file /chemin/prive/intent-v2.json --journal-dir /chemin/prive/session-journal
agentsandbox kubernetes session status --control-file /chemin/prive/control-v2.json --journal-dir /chemin/prive/session-journal
agentsandbox kubernetes session abandon --control-file /chemin/prive/control-v2.json --journal-dir /chemin/prive/session-journal
```

`prepare` réserve l'intention exacte, exige un ACK HTTP vivant de l'état
`Reserved` avec nonce et périmètre broker exacts, puis prépare ou réouvre le
journal privé. Il peut créer le répertoire de journal demandé ; `status` et
`abandon` exigent un répertoire existant. Une réponse perdue conserve une
réception inconnue : rejouer `prepare` avec le même fichier d'intention.
Le Store rejoue l'opération durable sans nouvelle réservation ou quota.

`abandon` retient un UUID d'opération stable dans `abandoned-v2.json` avant
tout POST de clôture. Une coupure après commit se reprend avec la même
commande et le même journal ; aucun UUID de remplacement n'est généré. Une
incarnation successeur peut soumettre uniquement l'abandon déjà retenu et
autorisé par la politique centrale. Sans abandon retenu, utiliser la
procédure de récupération contrôle autorisée. Un audit saturé refuse la
mutation et conserve le quota ; une panne ou un timeout ne libère rien.

`status` distingue phase locale et état central. Tous les rapports portent
`execution_available=false` et `runtime_state="not_observed"`. Une erreur
reste une erreur, sans état vide ou zéro inventé. L'ACK est une provenance
HTTP vérifiée vers le contrôle, pas une preuve indépendante de son stockage,
de NamespaceUID, de PVC, de confinement ou d'identité de processus Kubernetes.
S5, R1/R3/R4/R5/R7/R10 et la qualification cloud restent ouverts.

Le contrat HTTP R2 expose, sous `/api/v1/brokers/kubernetes/` :

| Méthode et suffixe | Contrat |
|---|---|
| GET `session-reservation-protocol` | Identité broker authentifiée, schéma, présence des politiques, effets indisponibles |
| POST `session-reservations` | Intention V2 exacte ; 201 fraîche, 200 rejeu exact |
| GET `session-reservations/{id}` | État descriptif de la réservation du broker authentifié |
| POST `session-reservations/{id}/confirmations` | Version2, nonce `request_id`, `reservation_input_hash` ; ACK lié au même état réservé et au broker |
| POST `session-reservations/{id}/abandonments` | Clôture V2 `preparation_abandoned`, révision attendue1 et UUID retenu ; CAS/audit/rejeu centraux |

Le tenant vient de l'identité authentifiée, avant le corps et le chemin. Les
corps/ACK sont limités à64/128KiB, JSON fermé sans membre dupliqué. Chaque
requête contrôle a un timeout12s, sans redirection, proxy ambiant ou retry
automatique ; la lecture du corps entrant est limitée à5s. Les secrets sont
refusés avant persistance/diffusion, les erreurs transport restent opaques.
