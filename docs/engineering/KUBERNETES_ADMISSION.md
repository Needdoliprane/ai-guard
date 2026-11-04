# Admission Kubernetes — proposition de création retenue avant exécution

**4 novembre 2026 · direction approuvée pour conception/code/tests locaux ; aucune qualification.**
Lecture initiale de la base `361bf69`. Le mainteneur a autorisé la direction
et le code local décrits ci-dessous. Cet accord n'autorise aucun appel cluster,
start/release ou lancement de worker ; l'admission réelle reste fermée.
Les [frontières d'architecture](ARCHITECTURE_CONTRACT.md), la
[supervision](BROKER_SUPERVISION.md), les [gates](PRODUCTION_READINESS.md) et
la [stratégie de test](TEST_STRATEGY.md) restent applicables.

## Besoin et état existant

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

## Décision demandée et premier lot

Autoriser la conception détaillée, le code Rust et les tests locaux d'un
**profil Kubernetes versionné de création retenue**, avec admission réelle
toujours fermée jusqu'aux preuves environnementales et à leur revue.
L'accord ne vaudrait ni autorisation de modifier le cluster, ni accord sur
le canal worker, ni activation d'un start/release.

Le premier lot ajouterait une vraie opération persistante CREATE et sa
reprise, distinctes du préflight. Profil fermé : gate imposée, `nodeName`
absent, scheduler attendu, image approuvée par digest et commande non métier
fixe à arrêter lors de la conception détaillée ; aucun manifest libre.
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
