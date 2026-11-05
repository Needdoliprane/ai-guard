# Admission Kubernetes — proposition de création retenue avant exécution

**4 novembre 2026 · direction approuvée pour conception/code/tests locaux ; aucune qualification.**
Lecture initiale de la base `361bf69`. Le mainteneur a autorisé la direction
et le code local décrits ci-dessous. Cet accord n'autorise aucun appel cluster,
start/release ou lancement de worker ; l'admission réelle reste fermée.
Les [frontières d'architecture](ARCHITECTURE_CONTRACT.md), la
[supervision](BROKER_SUPERVISION.md), les [gates](PRODUCTION_READINESS.md) et
la [stratégie de test](TEST_STRATEGY.md) restent applicables.

## Besoin et état existant

### Contrat central suivant — configuration, décision et raccord HTTP

Direction technique locale du5novembre, dans les frontières existantes.
La politique et son chargement, le prédicat d'image partagé, la migration9
et sa validation historique sont vérifiés par les lots locaux correspondants.
La tranche centrale suivante produit les décisions EvidenceV2 Admission et
Reconcile, puis les utilise dans six routes HTTP authentifiées du gateway ;
le [contrat API](../api.md#kubernetes-held-create-http-v1) décrit leurs DTO,
bornes et erreurs. Les capacités mémoire V1 restent refusées sous9 avant
lookup ; une V2 appropriée peut restituer un ancien reçu/snapshot V1 exact,
sans append ni Fresh. Le client HTTP broker, sa preuve d'ACK live et le
Coordinator9 restent à raccorder. Aucun appel cluster n'en découle.

La configuration de chaque tenant porte une option `held_create_policy`.
Absence/null interdit l'admission ; une politique présente vide est une erreur
de configuration. Le format1 est fermé :1à64approbations distinctes du tuple
exact `{authority_fingerprint,image,profile}`, TTL de décision1à300secondes,
politique canonique au plus16Kio. L'image garde la grammaire immutable actuelle
et sa limite256bytes ; aucune normalisation de référence ni wildcard.
Les approbations sont triées par tuple sans changer leurs strings. Le hash
engage toute la politique : domaine ASCII
`ai-guard:kubernetes-held-create-policy:v1`, un NUL, JSON UTF-8 compact à clés
triées récursivement. Aucun secret ou chemin de configuration n'est audité.

La politique courante est celle du `TenantState` immuable du processus, dérivé
de la configuration opérateur et consulté après authentification du broker et
admission dans le worker borné. Elle ne provient jamais du corps d'une demande.
Il n'y a pas de reload implicite ; un redémarrage doit terminer les anciens
issuers. Cette règle ne prouve pas une révocation atomique entre plusieurs
gateways aux configurations divergentes. La provenance déclarée de configuration
ne constitue pas une approbation humaine nominative ni une preuve de confinement.

| Scope de décision | Preuve centrale nécessaire | Opérations permises |
|---|---|---|
| Admission | Politique résolue courante approuvant l'empreinte complète, l'image et le profil ; identité, grant et quota serveur valides. Expiration min(TTLpolitique,binding,grant). | Réserver et demander un nouveau claim ; aucun effet sans les autres preuves et activation. |
| Réconciliation | Intention et réservation réellement relues dans le Store validé, tenant/broker/incarnation courants stricts ; expiration min(60s,binding,grant). Aucune politique retirée présentée comme approbation courante. | Lookup exact du claim historique, résultat et annulation ; aucun Reserve, Claim mutateur, Fresh ou nouvelle allocation. |

Retirer la politique interdit toute admission nouvelle et conserve la possibilité
de tracer/réconcilier un effet déjà possible. Révoquer identité ou grant reste
un refus ; ce lot ne relâche pas les contrôles d'incarnation pour une reprise
froide. Un claim absent reste absent : lookup `NotFound`, annulation préclaim
possible selon l'état réel, sans inventer de preuve d'émission.

EvidenceV2 engage explicitement scope et mode de preuve. Admission contient
snapshot/hash de la politique courante. Réconciliation lie séparément la
réservation originale (`reservation_seq/hash`, `admission_decision_hash`) et
le préfixe de nouvelle émission de cap (`origin_seq/hash`, dates et révisions
d'identité courantes). La réservation précède ce préfixe ; tenant, intention
et décision historique V1/V2 doivent correspondre. Une sérialisation reste
descriptive ; seuls les producteurs privés après ces vérifications créent
les capacités non sérialisables.

La frontière sémantique8→9 exige une sauvegarde neuve et un marqueur de format
unique lié au tenant, namespace et préfixe, sans table supplémentaire ni
activation. Ancien writer8 refusé avant mutation ; schéma9 sans marqueur,
marqueur sur8, V2 avant marqueur et nouveaux V1 après marqueur sont refusés.
Le marqueur est une opération de maintenance, pas un principal broker.
Son kind est `control.kubernetes.create.policy.genesis` et son objet fermé
contient exactement `version=1`, `protocol=kubernetes_held_create_policy_v1`,
`tenant_id`, `control_namespace_uuid`, `create_genesis_seq`,
`create_genesis_hash`, `previous_seq` et `previous_hash`. La première paire
référence le genesis CREATE du format8 ; la seconde référence le head exact
immédiatement avant le marqueur9. Ces origines restent distinctes, même si
elles sont égales dans une base sans autre événement après son genesis8.
Les séquences sont positives, hashes hexadécimaux minuscules complets et
namespace UUID canonique. Le rapport de migration désigne ce marqueur9.
Le timestamp observé du marqueur est canonique et ne dépasse pas son
`received_at` durable ; ce contrôle ne réécrit pas l'historique du format8.
Les quotas Kube partagés, planchers temporels, gardes de schéma et protocole
doivent évoluer ensemble. Les anciens tuples/hashes/records V1 restent
inchangés ; un replay V1 exact rend son reçu historique sans append ni Fresh.
Un nouveau résultat/une annulation sur une réservation V1 s'appuie sur une
décision V2 de réconciliation et ne fabrique pas de politique V1 absente.

Le raccord utilise le broker authentifié et des opérations HTTP v1 fermées,
avec une capacité d'ACK live à implémenter, distincte de la preuve mémoire Store.
Le gateway conserve politique et SQLite ; le broker conserve journal,
HostClock et credentials Kubernetes. Un reçu JSON, le schéma9 ou un grant
seul ne suffit jamais à ouvrir CREATE. Les routes, négociations et producteurs
doivent être vérifiés ensemble avant de présenter ce raccord complet comme livré.

Les six handlers centraux obtiennent identité, politique et quota depuis le
tenant authentifié après admission dans le worker Store borné. Reserve/Claim
exigent Admission ; lookup/résultat/annulation émettent chacun Reconcile depuis
la réservation originale réellement relue. La vérification de scope précède
tout lookup idempotent, y compris un Claim déjà committé. Aucune Reconcile ne
crée d'allocation ni de Claim mutateur. La vérification historique exige
`marker9.seq <= origin_seq < record.seq` et les hashes correspondants.
L'erreur d'un historique invalide reste opaque ; elle ne devient pas un refus
typé de la mutation courante.

L'upload est limité à64Kio/5s sans slot Store conservé ; le décodage fermé
refuse doublons à toute profondeur, profondeur supérieure à64 et suffixes.
Les réponses exactes sont limitées à128Kio, sans troncature. Le dictionnaire
de secrets inclut le bearer courant, pour entrées, données Store dérivées et
sorties, clés JSON comprises. Une réponse nécessitant un masquage est refusée
sans réécrire ses faits ; son commit peut néanmoins déjà être durable.
Claim201 exige le premier commit Store exact et une fenêtre encore valide
avant restitution ; GET/replay restent ObserveOnly. L'arrivée HTTP et sa
fenêtre doivent encore être vérifiées par le futur client broker.

**Point de développement du 5 novembre, base `7155ab3`, worktree dédié non
encore intégré.** Le code local ajoute les DTO fermés, le rendu/projection,
la migration explicite 7→8, les transactions de réservation/claim/résultat/
annulation, leur replay et le quota partagé avec Docker/Podman. Le journal
privé conserve le claim avant l'appel central, puis un candidat exact et son
accusé. Les [preuves](VALIDATION.md#5-novembre--fondation-dadmission-kubernetes-hors-cluster)
distinguent les gels intermédiaires et les 26 tests ciblés du gel de fondation
`composed-green`.
La tranche suivante relie la permission consommable et le transport CREATE,
avec quatre parents de permission/reprise, trois TLS et trois canaris existants
[PASS sur leurs gels](VALIDATION.md#5-novembre--permission-et-transport-create-sur-tls-local).
[Le coordinateur commun](VALIDATION.md) est ensuite vérifié par cinq parents
Store/journal/TLS, sans activation de production. Aucun
nouvel endpoint, commande de création, `RuntimeKind` ou constructeur
d'activation en production n'est ajouté. Le raccord d'exploitation et la
qualification complète restent ouverts ; les paragraphes de conception
ci-dessous ne constituent pas une déclaration de leur achèvement.

L'autorisation locale exacte possède une seule couture `cfg(test)` après
authentification réelle du broker. Son engagement descriptif audité lie aussi
credential/révisions, head d'origine et expiration ; il ne reconstruit aucune
capacité depuis le disque. Le replay vérifie binding/grant aux préfixes
d'établissement de cette décision et de mutation. Une publication du premier claim
après sa fenêtre rend `ObserveOnly`, sans preuve fraîche et sans libérer le
quota. Cette vérification UTC ne remplace pas l'échantillon HostClock
antérieur à la requête et le contrôle final avant POST.

La permission consommable conserve désormais ce même producteur HostClock,
boot/méthode et compteur initial. Le guard final revalide journal et credentials
avant sa dernière lecture d'horloge ; aucun await ne suit avant l'initiation.
Une reprise prepared-only emploie une lecture centrale dédiée, jamais un
claim créateur : l'opération absente reste absente. La couture d'activation
`cfg(test)` retient une copie du listener loopback exact ; aucune URL seule ou
réponse désérialisée ne fabrique la permission.

Le chemin CREATE fixe `fieldValidation=Strict` et confirme seulement201, choix
local conservateur : l'[API Pod officielle](https://v1-36.docs.kubernetes.io/docs/reference/kubernetes-api/core/pod-v1/#post-create)
documente aussi200/202. Ces autres statuts restent des effets inconnus, sans
retry ni adoption. [Strict](https://kubernetes.io/docs/reference/using-api/api-concepts/#field-validation)
ne prouve pas la sûreté de la réponse ni l'absence de mutation d'admission.
La réponse brute est bornée puis lue sans doublons avant Value, et l'autorité
est revérifiée après une réponse201 sûre. Ces GET ne sont pas une précondition
atomique NamespaceUID et ne lèvent pas la fermeture de production.

Le journal rouvre toujours en observation. Un claim historique exact peut être
retenu avec `ObserveOnly`, y compris après perte d'accusé avant `emission.json` ;
cela invalide irréversiblement tout checkpoint d'émission local antérieur.
Les sept noms fixes bornent le contenu logique à 161 Kio, sans garantir une
quota physique du système de fichiers. Un résidu partiel reste conservé et
refusé ; aucune réparation, suppression ou adoption automatique n'est fournie.

L'adaptateur actuel sait diagnostiquer une autorité explicite, inspecter un
Pod retenu et soumettre un dry-run audité ; [guide opérateur](../setup/kubernetes/README.fr.md).
`RuntimeKind` reste Docker/Podman. Leur chemin managed crée une cible avant
l'armement et le claim de start. Un POST Pod ordinaire ne fournit pas cette
même séparation. Le préflight n'accorde aucun start et ne produit aucun
handle possédé : supprimer `dryRun=All` n'est pas un raccord acceptable.
Sources : [runtime](../../src/runtime.rs), [managed](../../src/runtime/managed.rs),
[préflight](../../src/runtime/kubernetes/preflight.rs),
[handle d'inspection](../../src/runtime/kubernetes/observation.rs).

## Approches examinées

| Approche | Intérêt et obstacle |
|---|---|
| CREATE directement autorisé comme lancement | Moins de phases, mais l'UID n'est reçu qu'après l'effet possible. Il faudrait redéfinir l'armement avant UID, la reprise et le fencing ; ce n'est pas compatible avec le raccord actuel. Non retenue. |
| Pod créé avec une `schedulingGate`, libération séparée | Permet de retenir UID et spec admise avant de rendre le Pod planifiable. Recommandée sous les conditions ci-dessous ; la gate seule n'est pas une frontière de sécurité. |
| Bootstrap Rust en attente dans un Pod déjà lancé | Peut préparer un canal privé avant le métier, mais exige image/ENTRYPOINT de confiance, preuve d'attachement et nouveau preset. Dépend d'une décision de canal non reçue ; non retenue pour ce premier lot. |

Kubernetes 1.36 permet d'initialiser les gates à CREATE puis de les retirer,
sans pouvoir en ajouter après création. Elles contrôlent la disponibilité
pour le scheduler, pas l'autorisation ai-guard.
[Contrat officiel des scheduling gates](https://v1-36.docs.kubernetes.io/docs/concepts/scheduling-eviction/pod-scheduling-readiness/).

## Direction approuvée et premier lot à implémenter

Le mainteneur a autorisé la conception détaillée, le code Rust et les tests locaux d'un
**profil Kubernetes versionné de création retenue**, avec admission réelle
toujours fermée jusqu'aux preuves environnementales et à leur revue.
L'accord ne vaut ni autorisation de modifier le cluster, ni accord sur
le canal worker, ni activation d'un start/release.

Le premier lot ajouterait une vraie opération persistante CREATE et sa
reprise, distinctes du préflight. Profil fermé : gate imposée, `nodeName`
absent, scheduler attendu, image approuvée par digest et commande non métier
fixe selon l'addendum ci-dessous ; aucun manifest libre.
Aucune donnée métier, aucun credential, aucun montage du projet original,
hostPath, volume externe ou accès moteur. Aucun transfert workspace,
exec/attach, patch de libération ou commande métier dans ce lot.
Automount de token désactivé, aucun init/sidecar ajouté, profil non privilégié
et ressources bornées ; ces déclarations ne prouvent pas leur enforcement.
Les refus de Pod Security, politiques ou droits ne déclenchent aucun repli.

Cette étape établit la possession d'un Pod pour le futur adaptateur complet ;
elle ne livre pas une session Kubernetes. Le code pourra être développé et
testé hors cluster après accord, sans exposer `--runtime kubernetes` ni
réinterpréter les plans, empreintes et checkpoints Docker/Podman existants.
Périmètre proposé : modules `runtime/kubernetes/admission*`, transport fixe
et journal privé versionné ; API/audit central et permission typée à relier
explicitement. Réutiliser le stockage tenant, sans service ni base nouvelle.

## Contrat durable proposé

1. Authentifier le runner et le tenant ; vérifier grant broker, incarnation,
   autorité/namespace retenus, budgets et génération d'annulation. Retenir
   intention, UUID d'opération, profil et hash exact de requête. Autorisation
   et audit obligatoires durables **avant CREATE**, jamais hérités du dry-run.
2. Consommer une permission fraîche d'émission, après revalidation des
   capacités privées, sans retry HTTP automatique. Un état relu du disque
   ne recrée pas cette permission. Persister l'émission potentielle avant
   l'effet ; le crash entre ces deux étapes laisse une inconnue conservatrice.
3. Sur réponse validée, retenir durablement les véritables PodUID,
   resourceVersion, autorité, namespace, nom et spec post-admission, ainsi
   que le lien à l'intention. Vérifier le profil fermé et l'absence de
   placement ; une mutation dangereuse est un incident, jamais un succès.
   L'UID distingue les incarnations d'objet, sans authentifier un tenant.
   [Identifiants Kubernetes](https://v1-36.docs.kubernetes.io/docs/concepts/overview/working-with-objects/names/#uids).
4. ACK perdu ou réponse invalide : `ownership_unknown`, aucun second CREATE,
   start, adoption ou nettoyage automatique. Nom stable, labels et spec
   ressemblante ne prouvent pas l'ownership. Une reprise peut seulement
   inspecter une identité déjà retenue avec preuve suffisante ; sinon
   réconciliation opérateur documentée, sans promotion implicite.
5. Aucun compteur zéro, terminal ou quota libéré inféré de `Pending`, d'un
   404 ou de la fin du client. Borne de rétention, diagnostic des inconnues
   et procédure de nettoyage exact doivent être définis avant activation.
   Supprimer une cible exigerait une permission distincte liée à son UID ;
   cela ne constituerait pas à soi seul une preuve de quiescence ou D5.

CREATE adresse le Namespace par nom, sans précondition NamespaceUID dans
son contrat HTTP. Les GET avant/après détectent une divergence mais ne
rendent pas le POST atomique avec cette ancre. **Déduction de conception :**
aucune activation sans garantie démontrée contre suppression/recréation
concurrente du Namespace autorisé ; un recheck après effet ne suffit pas.
[API Pod 1.36, CREATE](https://v1-36.docs.kubernetes.io/docs/reference/kubernetes-api/core/pod-v1/#post-create).

## Frontière réelle de la gate et prérequis inconnus

`nodeName` contourne le scheduler : l'interdire dans la requête et dans
l'admission effective est nécessaire. Une inspection après CREATE arrive
trop tard pour empêcher un lancement injecté par admission.
[Placement direct](https://v1-36.docs.kubernetes.io/docs/concepts/scheduling-eviction/assign-pod-node/#nodename),
[phases d'admission](https://v1-36.docs.kubernetes.io/docs/reference/access-authn-authz/admission-controllers/#admission-control-phases).
Précision : en **v1.36.1**, `BindingREST` refuse une gate non vide ; le binding
seul n'est donc pas un contournement démontré. Une identité pouvant retirer
la gate puis binder reste un risque ; contrôler les deux capacités.
[Source officiel épinglé](https://github.com/kubernetes/kubernetes/blob/v1.36.1/pkg/registry/core/pod/storage/storage.go#L192-L225).

À établir : exclusivité effective des droits et acteurs capables de créer,
modifier, binder ou remplacer ces objets, chaîne d'admission préservant le
profil et l'ancre, et stabilité de ces conditions pendant l'opération.
Le broker du premier lot n'a besoin d'aucun droit patch/update/binding,
exec/logs/Secrets ; ses lectures et créations restent strictement bornées.
RBAC ne borne pas un CREATE principal à un `resourceName` ni à cette spec :
un simple Role ne remplace pas ces preuves.
[Portée de RBAC](https://v1-36.docs.kubernetes.io/docs/reference/access-authn-authz/rbac/#referring-to-resources).

L'audit durable ai-guard avant effet reste obligatoire. Les preuves des
mutations côté cluster doivent être indépendantes du rapport du broker,
avec couverture et accès vérifiés ; leur disponibilité n'est pas présumée.
L'audit Kubernetes dépend de sa politique et de son backend configurés.
[Audit officiel](https://v1-36.docs.kubernetes.io/docs/tasks/debug/debug-cluster/audit/#audit-policy).
Cible : Docker Desktop mono-nœud. Aucun CNI installé, politique modifiée,
credential provisionné ou mécanisme d'audit activé par ce document.
Si l'environnement existant ne satisfait pas ces conditions, activation
BLOCKED ; décrire l'écart, sans élargir discrètement les droits.

## Preuves avant activation et suite obligatoire

Tests locaux proposés : zéro POST sans audit/permission, refus de scope,
concurrence/annulation, crash aux frontières durables, ACK perdu sans
réémission/adoption, même nom/autre UID, admission altérée, budgets et
erreurs bornées ; aucune route de release accessible. Ils ne prouvent pas
l'absence d'exécution sur un cluster réel.
Avant essai réel explicitement autorisé : conditions ci-dessus vérifiées,
ressources possédées/bornées et nettoyage exact revu. Ensuite prouver sur
le cluster retenu absence de placement/exécution, refus des contournements,
reprise et attribution indépendante des effets. Tout reste NOT_RUN ici.

Le futur release doit consommer une permission fraîche liée à ce PodUID,
à la spec, à l'armement et à la génération courante. Il exige workspace
filtré, confinement/réseau, streaming privé, délais durables, observations,
reprise et fencing qualifiés ; un ACK de release ne vaut pas Running.
Ces dépendances ne sont ni levées par la gate ni déléguées à D5/Daybreak.
Le nouveau canal reste [une proposition distincte](DECISIONS.md#4-novembre--proposition-de-transport-des-workers-sans-réseau-décision-attendue).
Aucun gate de qualification ne change de statut par cette proposition.

## Addendum du 4 novembre — contrat du lot local CREATE retenu

**Conception dans la direction approuvée ; code, migration et tests ci-dessous
absents de la base inspectée `6b5c834`.** Les noms nouveaux désignent le travail
à réaliser. Aucun endpoint, CLI de création, factory d'activation, effet cluster
ou qualification ne résulte de cet addendum. Le lot doit être revu avant intégration.

### Coutures existantes et périmètre

Réutiliser l'autorité et les capabilities privées de
[`KubernetesAuthorityV1` / `KubernetesCredentials`](../../src/runtime/kubernetes.rs),
le transport HTTPS fixe sans proxy, redirect ni retry de
[`Transport::new`](../../src/runtime/kubernetes/transport.rs), et les engagements
de spec de [`KubernetesPodHandle::configuration_digest`](../../src/runtime/kubernetes/observation.rs).
`KubernetesPodHandle::from_control_checkpoint` n'est aujourd'hui qu'un constructeur
de données d'inspection : il ne doit jamais créer une permission de mutation.
Le plan et le guard de [`preflight`](../../src/runtime/kubernetes/preflight.rs)
restent spécifiques au dry-run ; ne pas les convertir par suppression d'une query.

[`AdmissionV1`, `LaunchSpecV1`, `RuntimeKindV1`](../../src/control.rs),
[`ArmV1`](../../src/control/supervision.rs) et
[`SupervisionOperationV1`](../../src/broker/checkpoint.rs) restent inchangés :
leurs liens au conteneur déjà possédé et leurs hashes Docker/Podman ne décrivent
pas ce CREATE avant UID. Le type privé
[`FreshStartPermission`](../../src/runner/broker.rs) fournit un patron de consommation,
pas une permission réutilisable. Aucun nouveau `RuntimeKind::Kubernetes` exposé.

### Profil et valeurs fermées

`profile = kubernetes_held_pod_v1`, `version = 1`, gate unique
`ai-guard.io/held-create-v1`, `schedulerName = default-scheduler`, aucun `nodeName`.
Nom exact : `ai-guard-held-` suivi des 32 chiffres hexadécimaux minuscules du
`create_id` UUID sans tirets. Un seul conteneur `worker`, `command = ["/usr/bin/id"]`,
aucun `args`, `env`, `envFrom`, init/sidecar/ephemeral container, credential,
imagePullSecret ou manifest libre. La commande fixe est non métier ; son
exécutabilité dans une image donnée reste à vérifier avant tout essai autorisé.

Reprendre explicitement les déclarations de sécurité, ressources, DNS et les
deux `emptyDir` mémoire du profil de préflight existant, sans importer de contenu.
Image approuvée `nom@sha256:<64 hex minuscules>`, `imagePullPolicy = Never`,
`restartPolicy = Never`, `automountServiceAccountToken = false`, aucun volume
externe, hôte ou moteur. Les labels portent uniquement managed-by, session,
opération et `purpose = held-create` ; ils n'authentifient aucune identité.
Le rendu est une fonction fermée ; la spec reçue doit conserver la gate et
respecter la même normalisation explicite des seuls défauts Kubernetes permis
par `preflight::validate_admitted`, sans élargir ce validateur historique.

Bornes du nouveau profil : requête Pod sérialisée ≤ 32 KiB, intention ≤ 16 KiB,
spec admise sérialisée ≤ 32 KiB, réponse HTTPS ≤ 1 MiB, profondeur JSON ≤ 64,
resourceVersion non vide ≤ 128 octets selon le validateur existant. Chaque
enveloppe de mutation ≤ 64 KiB ; l'événement complet doit aussi respecter les
128 KiB de `Store::prepare_event`. Tout dépassement ou redaction changeant le
contenu est refusé, sans troncature. Réutiliser 4 requêtes transport simultanées
au maximum, connexion 3 s, requête totale 8 s, zéro retry automatique.
Les ressources conservent les bornes de `KubernetesPreflightResourcesV1::validate`
et les quotas résolus du tenant, sans nouveaux seuils permissifs.

### DTO et identités immuables

Tous les DTO nouveaux sont `version = 1`, `deny_unknown_fields`, UUID canoniques
non nuls, SHA-256 de 64 caractères hexadécimaux minuscules. Les entiers de révision
et d'epoch sont bornés à `i64::MAX` et utilisent une addition vérifiée.

| DTO proposé | Champs exacts supplémentaires à `version` et rôle |
|---|---|
| `KubernetesCreateIntentV1` | `create_id`, `session_id`, `control_namespace_uuid`, `broker_id`, `incarnation_id`, `authority: KubernetesAuthorityV1`, `profile`, `image`, `resources: KubernetesPreflightResourcesV1`. Entrée immuable privée du contrôle ; aucun tenant sélectionnable ni donnée métier. |
| `KubernetesCreateTargetV1` | `create_id`, `session_id`, `authority_fingerprint`, `pod_name`, `request_sha256`, `configuration_sha256`. Dérivé du rendu exact de l'intention ; jamais accepté comme remplacement du rendu. |
| `KubernetesCreateClaimV1` | `operation_id`, `create_id`, `intent_hash`, `expected_revision`, `expected_create_epoch`, `emitter_id`. Consomme au plus une émission, jamais un start. |
| `KubernetesCreateResultV1` | `operation_id`, `create_id`, `intent_hash`, `claim_operation_id`, `expected_revision`, `expected_create_epoch`, `outcome`. Rapporte le même CREATE possible. |
| `KubernetesCreateCancelV1` | `operation_id`, `create_id`, `intent_hash`, `expected_revision`, `expected_create_epoch`. Invalide les claims futurs ; aucune suppression de Pod. |
| `KubernetesCreateReceiptV1` | `operation_id`, `input_hash`, `control_namespace_uuid`, `audit_seq`, `audit_hash`, `emission_window`. Reçu descriptif, sans pouvoir d'émission ; fenêtre `None` sauf pour un claim. |
| `KubernetesCreateObservationV1` | `api_version`, `kind`, `metadata`, `status`. Projection fermée décrite ci-dessous de la réponse CREATE, rapportée par le broker ; aucun Pod JSON arbitraire. |

`emission_window`, quand présente, contient exactement `issued_at` et
`not_after`, dates UTC RFC3339 canoniques du contrôle. À la transaction du
claim : `not_after = min(grant.not_after, authorization.expires_at, issued_at + 8 secondes)`,
avec une durée strictement positive. L'expiration de l'autorisation est aussi
bornée par celle du binding, aux deux préfixes historiques vérifiés.
C'est un budget de préparation et d'initiation de
la requête par le code Rust, pas une garantie de date du premier octet réseau
ou d'acceptation par le cluster. Il ne modifie ni le timeout HTTPS, ni la durée
de vie ou le quota du Pod.

`outcome` est une union fermée : `created_held` contient `observation`,
`admitted_spec`, `admitted_configuration_sha256`,
`observed_at` ; `ownership_unknown` contient uniquement `reason` parmi
`transport_unknown`, `api_rejected`, `response_invalid`, `profile_changed`,
`authority_changed`, `cancelled_after_claim`, `recovered_after_claim`,
avec `http_status: Option<u16>` présent seulement pour `api_rejected`.
Une réponse HTTP d'erreur ne prouve pas ici l'absence d'effet.
Le résultat local non ACKé est aussi une inconnue : aucun champ secret ou corps
d'erreur serveur n'est conservé. Une mutation dangereuse rapportée produit un
diagnostic local `profile_changed`, sans état de succès ; sa persistance centrale
dépend de l'acceptation effective de la mutation de résultat, jamais présumée.
Une spec non validée ou susceptible de contenir un secret est réduite à ce
motif opaque avant toute persistance, y compris dans le journal privé.

`observation.api_version = v1` et `observation.kind = Pod` sont exigés.
`observation.metadata` contient exactement `name`, `namespace`, `uid`,
`resource_version`, `labels`, `annotation_count`, `owner_reference_count`,
`finalizer_count`, `deletion_requested`, `unexpected_fields_present`.
Nom/namespace/labels sont comparés au rendu exact ; UID et resourceVersion
suivent les validateurs existants. Seules les quatre clés de labels du profil
et leurs valeurs attendues sont retenues, sans valeur inattendue libre.
Les trois compteurs sont des `u32`, exigés à zéro ; les deux booléens sont
exigés à false. Les seuls autres champs de métadonnées tolérés et non persistés
sont les défauts serveur `creationTimestamp` et `managedFields` ; les annotations,
références de propriétaire et finalizers ne sont jamais copiés. Type invalide,
label différent ou ajout hors liste rend la réponse impropre à `created_held`.

`observation.status` vaut `None` si le champ status est absent de la réponse ;
sinon il contient exactement `phase`, `container_status_count`,
`init_container_status_count`, `ephemeral_container_status_count`,
`start_time_present`, `placement_fields_present`, `unexpected_fields_present`,
`pod_scheduled`. `phase` est `None` ou l'une des cinq phases déjà reconnues par
le lecteur de Pod ; seul `None` ou `Pending` est admis. Les trois compteurs
`Option<u32>` valent `None` pour un tableau absent et `Some(0)` pour un tableau
vide ; toute valeur positive est refusée. Les trois booléens décrivent seulement
la présence des champs dans la réponse et doivent être false.
`placement_fields_present` couvre `hostIP`, `hostIPs`, `podIP`, `podIPs` et
`nominatedNodeName` non vides, sans persister leur valeur. `pod_scheduled` est
`None` ou le couple fermé `status = False`, `reason = SchedulingGated` extrait
d'une unique condition `PodScheduled` ; toute autre condition est refusée.
Le seul autre défaut de status toléré et non persisté est `qosClass` parmi
`Guaranteed`, `Burstable`, `BestEffort`. Champs/types inconnus et doublons sont
refusés. Aucun message, IP, corps d'événement ou contenu de status libre n'est
conservé ; projection metadata/status complète ≤ 4 KiB.

Le contrôle central valide à nouveau cette projection et la spec fermée ;
les mêmes contrôles sont rejouables depuis l'entrée auditée. Une extraction
partielle, invalide ou non bornée ne devient pas une observation vide.
Le broker pourrait toutefois déclarer des champs faux : le DTO et sa validation
ne constituent pas une observation indépendante du cluster ni une preuve
d'absence d'exécution. Le résultat audité conserve `provenance = declared` et
`coverage = partial` ; les preuves d'admission/audit cluster restent requises.

`intent_hash` emploie le domaine `ai-guard:kubernetes-create-intent:v1\0` sur
le JSON canonique de l'intention ; `request_sha256` emploie
`ai-guard:kubernetes-create-request:v1\0`, l'empreinte d'autorité puis les
octets exacts conservés pour le POST. La spec conserve le domaine existant de
`configuration_digest`. Le JSON canonique trie récursivement les clés des
objets, conserve l'ordre des tableaux et n'ajoute aucun espace ; pas de float
dans l'intention. Le rendu, les bytes et les domaines doivent avoir un golden
indépendant avant code d'émission.
Les `input_hash` des mutations utilisent le même encodage canonique avec les
domaines distincts `ai-guard:kubernetes-create-claim:v1\0`,
`ai-guard:kubernetes-create-result:v1\0` et
`ai-guard:kubernetes-create-cancel:v1\0` ; la réservation utilise `intent_hash`.
Le snapshot a le domaine `ai-guard:kubernetes-create-snapshot:v1\0`.
Ne pas changer `control::content_hash` ni ses engagements historiques.

**`create_epoch` est un epoch durable propre au CREATE**, initialisé à 1,
incrémenté par une annulation acceptée. Il est distinct de
[`WorkloadSubjectV1::Worker.generation`](../../src/workload.rs), actuellement
contraint à 1, et du compteur mémoire du service broker. Ce protocole d'epoch
reste à implémenter ; aucune garantie de génération centrale n'est déjà livrée.

### Une projection tenant et des transactions auditées

Ajouter une seule table `kubernetes_creates` : `create_id` PRIMARY KEY,
`session_id` UNIQUE, `intent_hash` UNIQUE, `state`, `create_epoch`, `revision`,
`quota_held`, `cpu_millis`, `memory_bytes`, `snapshot_json`, `snapshot_hash`,
`last_event_seq` UNIQUE REFERENCES `events(seq)`, plus un index sur `quota_held`.
Colonnes toutes NOT NULL, `quota_held` CHECK IN (0,1), état fermé aux cinq
valeurs ci-dessous, epoch/révision ≥ 1 et ressources strictement positives.
`snapshot_json` conserve intention, cible dérivée, claim éventuel, outcome
éventuel et métadonnées d'annulation. Cette table permet CAS, lecture et quota
sans nouveau stockage. Chaque colonne est reconstruite et comparée à l'audit.

Réutiliser `events.id` UNIQUE comme `operation_id` de chaque mutation :
réservation avec `operation_id = create_id`, UUID distinct pour claim, résultat
et annulation. L'événement conserve entrée exacte, `input_hash`, preuve
d'identité authentifiée, date de contrôle, révision précédente et snapshot
résultant ; réservation/claim conservent aussi les `QuotaPolicyV1` et engagements
des choix de contrôle effectivement vérifiés, afin de rejouer la décision
historique sans lui appliquer une configuration actuelle. Son kind appartient à `control.kubernetes.create.` suivi de
`reserved`, `emission_possible`, `result` ou `cancelled`. Une table d'opérations
ou un second ledger ne sont pas nécessaires : lookup par `events.id`, puis
validation du kind et du payload exact avant tout replay. Le replay retourne
le résultat historique ; un identifiant réutilisé avec contenu divergent
est un conflit, y compris s'il appartient à une autre famille d'événements.
La preuve d'identité auditable ne contient que tenant retenu, identifiants
broker/incarnation/credential et révisions binding/grant, selon le patron privé
`IdentityEvidence` du registre. Préserver le refus des événements `control.*`
par `Store::append` : l'API générique d'événements ne peut produire ni état
CREATE, ni reçu typé, même avec un payload ressemblant.

| État central | Justification et transitions admises |
|---|---|
| `reserved` | Quota réservé, aucun droit d'émission émis ; vers `emission_possible` ou `cancelled_before_emission`. |
| `emission_possible` | Claim durable pouvant précéder un POST ; vers `created_held` ou `ownership_unknown`, jamais vers `reserved`. |
| `created_held` | Réponse sûre reçue et résultat central durable ; son ACK peut être perdu côté broker. Quota conservé, aucune transition de release dans ce lot. |
| `ownership_unknown` | Effet ou attribution non établi ; quota conservé, aucune adoption, nettoyage ou réémission automatique. |
| `cancelled_before_emission` | Annulation gagnant le CAS avant tout claim ; seule transition qui libère la réservation dans ce lot. |

Chaque transition nouvelle augmente `revision` de 1. L'annulation après claim
augmente aussi `create_epoch`, conserve le quota et conduit à
`ownership_unknown` ; après `created_held`, elle conserve cet état/UID et
invalide seulement l'epoch, sans prétendre arrêter quoi que ce soit.
Une réponse tardive pour un epoch périmé n'est conservée que dans l'emplacement
durable borné du journal privé décrit ci-dessous ; aucun incident central n'est
présumé enregistré. Elle ne promeut pas un état et n'autorise aucune émission.
Une seconde mutation `result` différente est un conflit : pas de remplacement
de l'UID/spec ou d'effacement de l'inconnue dans ce lot.
Une nouvelle annulation sur `cancelled_before_emission` est refusée ; seul
le replay de l'annulation exacte reste permis. Une perte d'ACK laisse le
checkpoint local inconnu sans réécrire un résultat central déjà durable.

Transactions prévues, dans le même Store tenant et sans appel réseau sous lock :

1. `reserve_kubernetes_create` : vérifier l'autorisation centrale explicite de
   cette intention non métier, le tenant authentifié, le contrôle live, le
   namespace de contrôle, les binding/grant/incarnation broker et le scope
   d'autorité Kubernetes ; rendre et hasher le profil exact. Vérifier quota et
   capacité d'audit, puis insérer `reserved`, quota et événement dans une seule
   transaction contrôlée. Un refus ne crée aucune permission ni réservation.
2. `claim_kubernetes_create` : après tous les awaits de préparation locale,
   refaire les contrôles d'identité, de scope, de temps et d'autorisation,
   vérifier `reserved`, les deux CAS et l'intention. Commiter
   `emission_possible` avec audit avant de répondre. Seule la première réponse
   fraîche exacte, dans son budget d'initiation, peut porter une disposition
   `emit_once` ; tout replay porte `observe_only`, même si l'ancien événement
   expose le claim accepté.
3. `record_kubernetes_create_result` : vérifier le lien au claim, les CAS et
   l'identité ; revalider la projection metadata/status et la spec exacte
   avant `created_held`, sans transformer le rapport broker en preuve runtime
   indépendante. Persister résultat et audit atomiquement.
   Perte du reçu ou échec de persistance : conserver la réponse privée si
   disponible et l'inconnue, sans exposer de handle possédé autorisant un effet.
   Le rejeu **du résultat exact** peut obtenir son ACK ; il ne refait pas le POST.
4. `cancel_kubernetes_create` : mêmes contrôles centraux, CAS et audit, puis
   incrément d'epoch et transition ci-dessus. La concurrence avec le claim est
   sérialisée par la transaction ; une annulation perdante ne suppose pas
   l'absence de POST. Les replays n'incrémentent ni révision ni epoch.

Les méthodes nouvelles reprennent
[`Store::begin_checked` / `commit_checked`](../../src/audit.rs),
[`revalidate_broker_identity`](../../src/audit/workload.rs) et les contrôles
grant/temps **avant replay** de
[`Store::persist_broker_change`](../../src/audit/broker_registry.rs).
Les contrôles rôle runner et preuve broker doivent être reliés explicitement
par l'adaptateur de confiance ; aucun champ JSON ni `BrokerIdentityViewV1`
ne les remplace. Aucun raccord HTTP nouveau n'est déclaré disponible ici.
Le reçu d'événement générique du préflight n'est pas cette autorisation typée.

La réservation compte un worker, `cpu_millis` et `memory_mib × 1 048 576`,
additionnés avec arithmétique vérifiée aux lignes `workers.quota_held = 1`.
Modifier **le même** [`quota_usage`](../../src/audit/control.rs) pour inclure
`kubernetes_creates.quota_held = 1` sous schéma 8 : admission Docker/Podman,
admission Kubernetes et lecture du quota voient la même somme transactionnelle.
Les états conservateurs gardent leur réservation, sans CPU consommé, allocation
runtime, durée terminale ou usage zéro inventé. Les politiques d'audit physique,
tenant et broker existantes s'appliquent à chaque append ; le lot ne garantit
pas de capacité future pour un résultat si l'audit se remplit après claim.
Dans ce cas l'effet reste inconnu, le quota tenu et l'ACK de succès impossible.

Migration explicite `Store::migrate_kubernetes_create_v8`, **7→8**, via le
backup neuf vérifié de `migrate_control_version` : création de la projection
vide et événement `control.kubernetes.create.genesis` liant précédent head et
namespace, puis validation schéma/intégrité avant commit. Adapter les listes
fermées de versions et les validateurs existants à 8 sans changer les règles
de 1–7. Rejouer les événements dans l'ordre : identités/grants historiques au
préfixe concerné, rendu/hashes, CAS, quota partagé à la réservation et colonnes
exactes. Table absente, ligne modifiée, suppression d'inconnue ou divergence
audit/projection invalide le Store ; aucune réparation silencieuse.
Migration et réouverture ne recréent aucune autorité live ni permission.

### Journal privé et dernier contrôle avant effet

Le nouveau journal privé versionné du CREATE conserve exactement intention,
cible, bytes du Pod, reçus centraux, révision/epoch et réponse éventuelle ;
aucun credential ni réponse brute. Réutiliser les capabilities privées et barrières de
[`control_fs`](../../src/control_fs.rs), sans charger ce format dans les anciens
plans/checkpoints Docker. Une intention `prepared` est durable avant le claim.
Après son ACK, persister localement `emission_possible` **avant** `send`.
Une erreur de cette persistance interdit le POST, même avec un claim central.

`FreshKubernetesCreatePermission` est un type privé, consommé par valeur,
sans Clone, serde ou constructeur public. Il lie le reçu du claim frais,
son émetteur, le scope, l'epoch, la fenêtre et les bytes exacts. Échantillonner
[`HostClock::sample`](../../src/broker/clock.rs) avant d'envoyer la demande
centrale de claim ; retenir ce boot/méthode/temps continu dans la permission
mémoire. Au dernier guard, le temps écoulé depuis cet échantillon doit être
strictement inférieur à `not_after - issued_at` (au plus 8 s). Compter aussi
l'attente de l'ACK et les fsync ; aucun nouveau délai à réception du reçu.
Boot/méthode différents, horloge régressive, calcul invalide ou fenêtre expirée
interdisent l'émission, sans recréer un claim. Ce raccord d'horloge et ses
propriétés de suspension restent à tester et qualifier.

Le guard final synchrone revalide aussi capabilities privées, annulation locale
et cible après acquisition du permis transport et construction de la requête ;
aucun `await` ne sépare ce guard de l'initiation de `send`. DNS, connexion, TLS,
ordonnancement du futur et traitement serveur peuvent survenir ensuite : ce
guard ne borne ni le premier octet réseau, ni la réception/admission effective
avant `not_after`. Le point central d'autorisation est le claim ; aucune
révocation distante atomique avec le POST n'est promise. Une échéance effective
côté cluster exige des preuves d'admission, d'horloges et d'exclusivité des
acteurs avant activation ; le seul appel Rust `send` ne les fournit pas.

Tout disque relu avec claim ou `emission_possible` donne seulement
`observe_only` / `ownership_unknown` : **aucun second POST, même si zéro octet
semble avoir été envoyé**, même après 404/409 ou reboot. Après résultat validé,
retenir la réponse privée, obtenir l'ACK central, puis commiter le checkpoint
local `created_held` avant de publier le handle pour inspection seulement.
Un crash à chaque frontière peut donc conserver une inconnue ; un même nom,
label ou spec ne permet jamais d'adopter un UID découvert ultérieurement.

Course explicite : claim `revision = 2, create_epoch = 1`, annulation acceptée
`revision = 3, create_epoch = 2`, puis résultat portant les anciens CAS.
Le résultat est rejeté sans nouvel événement central, promotion ni libération
du quota. Le journal privé conserve **un seul candidat de résultat exact**
(DTO borné à 64 KiB avec projection sûre, jamais le Pod brut) et un seul code
de refus fermé `stale_revision`, `stale_epoch`, `control_rejected` ou `unavailable`. Il ne réécrit
pas le candidat avec de nouveaux CAS et n'accumule pas un historique de réponses.
La conservation emploie les vraies barrières durables ; si elle échoue, aucune
conservation réussie n'est déclarée et la reprise reste inconnue.

**Résultat logiciel du 5 novembre.** Le coordinateur local retient le candidat
exact et son premier code de refus fermé avant de retourner l'erreur. Les
[17tests ciblés](VALIDATION.md) vérifient les reprises et la séparation des
causes historiques/courantes. Aucun CAS de rattrapage n'est permis ; le
raccord d'exploitation et les qualifications physiques restent ouverts.

Les motifs sont établis par la transaction, sans les déduire d'un snapshot
client : pour un **Result** lié au hash d'intention et au claim exacts, le
Store compare d'abord l'epoch, puis la révision.
L'epoch attendu inférieur établit `StaleEpoch`, supérieur reste `Conflict` ;
à epoch égal, la révision inférieure établit `StaleRevision`, supérieure
reste `Conflict`. Les
autres conflits restent génériques. Le lookup du replay exact précède cette
transition et doit restituer son reçu historique sans réappliquer le CAS.
Cette distinction est vérifiée au niveau Store/coordinateur local, sans API
ou lancement Kubernetes de production livré.

Les mêmes transitions servent au replay de vérification ; une erreur issue
d'un historique invalide ne doit pas traverser cette frontière comme un
refus stale du candidat courant. La validation doit échouer de manière
opaque/indisponible et invalider le Store, sans conserver une cause anyhow
qui permettrait cette fausse attribution. Les erreurs précises de la
mutation courante restent distinctes, après transaction validée.

Le premier code de refus est un fait historique, attaché au hash du candidat,
pas un état courant. Un nouvel échec d'une autre classe ou un ACK ultérieur
ne l'écrase ni ne l'efface. `control_rejected` représente un autre refus typé
du contrôle ; `unavailable` représente une confirmation indéterminée, y
compris la perte injectée d'un ACK après vrai commit. Un échec de rétention
ou de sync doit rester une erreur de journal sans rétention prétendue.

Le format privé reste fermé : le nouveau lecteur accepte les trois anciens
codes, avec leurs bytes inchangés, et le quatrième. Un ancien lecteur refuse
`control_rejected` sans réparation ni suppression ; cette extension avant
activation n'est pas une compatibilité descendante transparente. Les
consommateurs Rust du type d'erreur fermé doivent aussi traiter les deux
nouvelles variantes ; aucun contrat HTTP de lancement n'est annoncé.

Reprise d'un résultat perdu : si le candidat exact et son `operation_id` sont
durables, seul son replay est permis sous identité encore autorisée. Un ACK
historique déjà commité peut être relu sans réappliquer la transition ; il ne
réactive aucun état ou droit. Si aucun résultat central n'existe et que les
CAS ont été dépassés, conserver le refus/candidat privé et le quota, sans
nouvelle mutation adaptée, nouvel endpoint ou ledger d'incident. Si le candidat
a été perdu avant persistance, il reste inconnu : pas de reconstruction à partir
d'un Pod de même nom. Le diagnostic doit exposer cette limite et demander la
réconciliation opérateur exacte prévue avant activation, sans adopter ni
nettoyer automatiquement.

**Aucune factory de permission de production dans ce lot.** Les prérequis
NamespaceUID stable, droits exclusifs, chaîne d'admission et audit cluster
restent non prouvés : `require_workload_admission` conserve son refus.
La capacité d'activation future sera opaque, indépendante des DTO sérialisables
de preuves et liée à leur périmètre/durée ; une case `enabled`, un bool passé
au constructeur, une variable d'environnement ou un préflight ne peut la
fabriquer. Les tests locaux utilisent uniquement une couture privée `cfg(test)`
et un serveur de test explicitement possédé, sans endpoint cluster réel.
La rétention reste bornée par quotas/capacité d'audit, sans TTL qui libérerait
une inconnue ; diagnostic et procédure de réconciliation/cleanup exact restent
des prérequis d'activation, pas une suppression implémentée par ce lot.

### Répartition et acceptation du lot

| Propriétaire de périmètre | Fichiers proposés ou existants à modifier |
|---|---|
| Contrats/Store/API, agent principal | Nouveaux `src/control/kubernetes_admission.rs`, `src/audit/kubernetes_admission.rs` ; raccords dans `src/control.rs`, `src/audit.rs`, `src/audit/control.rs` et validateurs de schéma concernés. Aucun endpoint sans contrat séparément revu. |
| Runtime CREATE, agent dédié | Nouveau `src/runtime/kubernetes/admission.rs` ; raccords `src/runtime/kubernetes.rs`, `src/runtime/kubernetes/transport.rs` ; nouveau journal privé CREATE dans ce périmètre, sans mutation des anciens checkpoints. |
| Validation ciblée, Luna | Tests produit Rust de ces frontières, sur le gel intégré et une cible de compilation propre au checkout ; aucun moteur ou fournisseur. |
| Documentation/revue indépendante | Ce document, puis guides Kubernetes EN/FR et preuves/écarts lors du lot de code ; second regard sur audit, quota, émission et reprise. |

Scénarios nécessaires, avec vrais Store SQLite/fichiers et transport local
compté lorsque pertinent. Cette liste fixe l'acceptation de la verticale
complète : les sous-propriétés déjà vérifiées figurent dans les
[preuves du 5 novembre](VALIDATION.md#5-novembre--fondation-dadmission-kubernetes-hors-cluster) ;
les parcours cluster et la qualification complète restent **NOT_RUN** ;
les [essais TLS locaux](VALIDATION.md#5-novembre--permission-et-transport-create-sur-tls-local)
ont leur preuve distincte :

1. Sans autorité live, rôle, preuve broker courante, scope, quota, capacité
   d'audit ou activation : zéro POST ; l'erreur d'audit rollbacke réservation
   et projection. Ancien reçu préflight et bool sérialisé restent refusés.
2. Deux claims concurrents donnent au plus un `emit_once` ; annulation avant
   claim donne zéro POST/quota libéré, annulation après claim garde le quota.
   Mauvais epoch/révision, autre broker/incarnation/tenant et contenu divergent
   sont refusés ; le replay exact revalide encore identité et expiration.
   ACK/fsync dépassant la fenêtre, expiration à la borne exacte, changement
   de boot/méthode ou recul du temps continu donnent zéro POST sans nouveau délai.
3. Arrêts injectés après chaque vraie frontière durable : réservation, claim,
   checkpoint d'émission, POST, réponse, ACK central, checkpoint local. Au
   redémarrage, aucun second POST dès qu'un claim a pu réussir. ACK perdu du
   résultat se réconcilie par son replay exact, sans émettre ni adopter.
   Course claim rev2/epoch1 → cancel rev3/epoch2 → résultat rev2/epoch1 :
   refus CAS, aucun événement central ajouté, candidat privé durable borné,
   quota conservé ; candidat perdu ou journal en panne restent inconnus.
4. UID remplacé au même nom, NamespaceUID divergent, gate retirée, `nodeName`,
   conteneur/volume/credential injecté, statut d'exécution, spec trop grande
   ou JSON invalide : jamais `created_held`, diagnostic opaque et quota conservé.
   Le Store rejette séparément labels/flags/counts/status incohérents et rejoue
   les contrôles des résultats acceptés depuis l'audit ; un refus sans commit
   ne crée aucun événement central. Aucun message/status libre n'est persisté.
5. Concurrence réservations Docker/Kubernetes à la dernière unité du quota :
   une seule réservation acceptée. Réouverture/reconstruction gardent la somme
   et les inconnues ; mutation SQL, audit manquant et replay divergent échouent.
6. Migration 7→8 avec backup exact, ancien schéma préservé, table absente refusée,
   reopen sans activation ; limites bytes/profondeur/temps et panne après
   réponse vérifiées sans tronquer ni masquer en succès. Contrôle statique et
   transport compté : aucune méthode PATCH/release/start/DELETE disponible.

Critère du lot logiciel : ces propriétés sont implémentées, vérifiées sur le
gel et relues, avec diagnostics et guides cohérents. Ce résultat laissera
séparément **BLOCKED** l'activation et **NOT_RUN** tous les effets cluster,
sans fermer de gate production ni qualifier une session Kubernetes.

### Revue documentaire de `7e0ea0` et résolution

Trois écarts ont été établis avant code : le résultat ne contenait pas les
métadonnées/status nécessaires au contrôle annoncé ; le budget 8 s pouvait
être lu comme une échéance réseau/cluster ; le résultat tardif après annulation
était appelé incident sans voie centrale admissible. Cet addendum est corrigé
par une projection fermée et revalidable, une borne d'initiation explicitement
limitée, et la seule conservation privée durable du candidat refusé. Aucun
nouveau ledger, endpoint, test exécuté ou comportement Rust n'en résulte.
